use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;
use windows::core::BOOL;
use windows::Win32::Foundation::{HWND, VARIANT_BOOL};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::System::Variant::{VARIANT, VT_BOOL};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationTextPattern,
    IUIAutomationTextPattern2, IUIAutomationValuePattern, UIA_IsReadOnlyAttributeId,
    UIA_TextPattern2Id, UIA_TextPatternId, UIA_ValuePatternId,
};
use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

use crate::context;

#[derive(Clone, Debug, Serialize)]
pub struct UiaSnapshot {
    pub control_type: i32,
    pub class_name: String,
    pub is_password: bool,
    pub is_enabled: bool,
    pub is_read_only: Option<bool>,
    pub has_text_pattern: bool,
    pub has_text_pattern2: bool,
    pub has_value_pattern: bool,
    pub selection_count: i32,
    pub caret_active: Option<bool>,
    pub native_hwnd_zero: bool,
    pub ancestor_verified: bool,
    pub ancestor_depth: u32,
    pub foreground_stable: bool,
    pub region_hint: Option<String>,
    pub elapsed_ms: u64,
}

enum Request {
    Warmup {
        reply: Sender<()>,
    },
    Capture {
        id: u64,
        deadline: Instant,
        foreground: isize,
        reply: Sender<Option<UiaSnapshot>>,
    },
    CompareLast {
        id: u64,
        deadline: Instant,
        foreground: isize,
        reply: Sender<Option<bool>>,
    },
    Shutdown,
}

pub struct UiaClient {
    tx: Option<Sender<Request>>,
    next_id: AtomicU64,
    latest: Arc<AtomicU64>,
    timeouts: AtomicU32,
    unavailable: AtomicBool,
}

impl UiaClient {
    fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        let latest = Arc::new(AtomicU64::new(0));
        let worker_latest = latest.clone();

        let spawned = thread::Builder::new()
            .name("quickphrase-uia".into())
            .spawn(move || worker(rx, worker_latest));

        match spawned {
            Ok(_) => Self {
                tx: Some(tx),
                next_id: AtomicU64::new(0),
                latest,
                timeouts: AtomicU32::new(0),
                unavailable: AtomicBool::new(false),
            },
            Err(err) => {
                crate::logging::error(&format!("failed to spawn UIA worker: {err}"));
                Self {
                    tx: None,
                    next_id: AtomicU64::new(0),
                    latest,
                    timeouts: AtomicU32::new(0),
                    unavailable: AtomicBool::new(true),
                }
            }
        }
    }

    pub fn warmup(&self, timeout: Duration) -> bool {
        if self.unavailable.load(Ordering::Relaxed) {
            return false;
        }
        let Some(tx) = &self.tx else {
            return false;
        };
        let (reply_tx, reply_rx) = mpsc::channel();
        if tx.send(Request::Warmup { reply: reply_tx }).is_err() {
            self.unavailable.store(true, Ordering::Relaxed);
            return false;
        }
        match reply_rx.recv_timeout(timeout) {
            Ok(()) => {
                self.timeouts.store(0, Ordering::Relaxed);
                true
            }
            Err(_) => {
                self.note_timeout();
                false
            }
        }
    }

    pub fn capture(&self, foreground: HWND, timeout: Duration) -> Option<UiaSnapshot> {
        if self.unavailable.load(Ordering::Relaxed) {
            return None;
        }
        let Some(tx) = &self.tx else {
            return None;
        };

        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        self.latest.store(id, Ordering::Relaxed);

        let (reply_tx, reply_rx) = mpsc::channel();
        if tx
            .send(Request::Capture {
                id,
                deadline: Instant::now() + timeout,
                foreground: foreground.0 as isize,
                reply: reply_tx,
            })
            .is_err()
        {
            self.unavailable.store(true, Ordering::Relaxed);
            return None;
        }

        match reply_rx.recv_timeout(timeout) {
            Ok(result) => {
                self.timeouts.store(0, Ordering::Relaxed);
                result
            }
            Err(_) => {
                self.note_timeout();
                None
            }
        }
    }

    pub fn compare_last(&self, foreground: HWND, timeout: Duration) -> Option<bool> {
        if self.unavailable.load(Ordering::Relaxed) {
            return None;
        }
        let Some(tx) = &self.tx else {
            return None;
        };

        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        self.latest.store(id, Ordering::Relaxed);

        let (reply_tx, reply_rx) = mpsc::channel();
        if tx
            .send(Request::CompareLast {
                id,
                deadline: Instant::now() + timeout,
                foreground: foreground.0 as isize,
                reply: reply_tx,
            })
            .is_err()
        {
            self.unavailable.store(true, Ordering::Relaxed);
            return None;
        }

        match reply_rx.recv_timeout(timeout) {
            Ok(result) => {
                self.timeouts.store(0, Ordering::Relaxed);
                result
            }
            Err(_) => {
                self.note_timeout();
                None
            }
        }
    }

    fn note_timeout(&self) {
        let count = self.timeouts.fetch_add(1, Ordering::Relaxed) + 1;
        crate::logging::warn(&format!("UIA request timed out ({count})"));
        if count >= 2 {
            self.unavailable.store(true, Ordering::Relaxed);
            crate::logging::error(
                "UIA marked unavailable for this session; insertions degrade to copy-only",
            );
        }
    }
}

impl Drop for UiaClient {
    fn drop(&mut self) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(Request::Shutdown);
        }
    }
}

static CLIENT: OnceLock<Mutex<UiaClient>> = OnceLock::new();

fn client() -> &'static Mutex<UiaClient> {
    CLIENT.get_or_init(|| Mutex::new(UiaClient::new()))
}

pub fn warmup(timeout: Duration) -> bool {
    match client().lock() {
        Ok(guard) => guard.warmup(timeout),
        Err(_) => false,
    }
}

pub fn capture(foreground: HWND, timeout: Duration) -> Option<UiaSnapshot> {
    let guard = client().lock().ok()?;
    guard.capture(foreground, timeout)
}

pub fn compare_last(foreground: HWND, timeout: Duration) -> Option<bool> {
    let guard = client().lock().ok()?;
    guard.compare_last(foreground, timeout)
}

/// Any explicit `true` wins; only two explicit `false` values prove writable.
/// A single `false` plus an unavailable source stays unknown and therefore denies.
pub fn combine_read_only(value: Option<bool>, text: Option<bool>) -> Option<bool> {
    match (value, text) {
        (Some(true), _) | (_, Some(true)) => Some(true),
        (Some(false), Some(false)) => Some(false),
        _ => None,
    }
}

fn worker(rx: Receiver<Request>, latest: Arc<AtomicU64>) {
    let init = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    if init.is_err() {
        crate::logging::error(&format!("CoInitializeEx failed: 0x{:08X}", init.0 as u32));
        return;
    }

    let automation: IUIAutomation =
        match unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) } {
            Ok(automation) => automation,
            Err(err) => {
                crate::logging::error(&format!("CoCreateInstance(CUIAutomation) failed: {err}"));
                unsafe { CoUninitialize() };
                return;
            }
        };

    let mut last: Option<IUIAutomationElement> = None;

    while let Ok(request) = rx.recv() {
        match request {
            Request::Warmup { reply } => {
                let _ = reply.send(());
            }
            Request::Capture {
                id,
                deadline,
                foreground,
                reply,
            } => {
                if is_stale(id, deadline, &latest) {
                    let _ = reply.send(None);
                    continue;
                }

                let target = HWND(foreground as *mut core::ffi::c_void);
                let (snapshot, element) = capture_element(&automation, target);

                if is_stale(id, deadline, &latest) {
                    let _ = reply.send(None);
                    continue;
                }
                if let Some(element) = element {
                    last = Some(element);
                }
                let _ = reply.send(snapshot);
            }
            Request::CompareLast {
                id,
                deadline,
                foreground,
                reply,
            } => {
                if is_stale(id, deadline, &latest) {
                    let _ = reply.send(None);
                    continue;
                }
                let target = HWND(foreground as *mut core::ffi::c_void);
                let result = compare_element(&automation, target, &last);
                if is_stale(id, deadline, &latest) {
                    let _ = reply.send(None);
                    continue;
                }
                let _ = reply.send(result);
            }
            Request::Shutdown => break,
        }
    }

    unsafe { CoUninitialize() };
}

fn is_stale(id: u64, deadline: Instant, latest: &AtomicU64) -> bool {
    id < latest.load(Ordering::Relaxed) || Instant::now() > deadline
}

fn capture_element(
    automation: &IUIAutomation,
    foreground: HWND,
) -> (Option<UiaSnapshot>, Option<IUIAutomationElement>) {
    let started = Instant::now();

    let before = unsafe { GetForegroundWindow() };
    let element = match unsafe { automation.GetFocusedElement() } {
        Ok(element) => element,
        Err(_) => return (None, None),
    };
    let after = unsafe { GetForegroundWindow() };
    let foreground_stable = before == foreground && after == foreground;

    let native = unsafe { element.CurrentNativeWindowHandle() }.unwrap_or_default();
    let native_hwnd_zero = native.0.is_null();
    let mut ancestor_verified = false;
    let mut ancestor_depth = 0u32;

    if !foreground_stable {
        // The window changed while we were querying; no ancestor claim is trustworthy.
    } else if !native_hwnd_zero {
        ancestor_verified = context::belongs_to(foreground, native);
    } else if let Ok(walker) = unsafe { automation.RawViewWalker() } {
        let mut current = element.clone();
        for depth in 1..=64u32 {
            let Ok(parent) = (unsafe { walker.GetParentElement(&current) }) else {
                break;
            };
            let handle = unsafe { parent.CurrentNativeWindowHandle() }.unwrap_or_default();
            if !handle.0.is_null() {
                ancestor_verified = context::belongs_to(foreground, handle);
                ancestor_depth = depth;
                break;
            }
            current = parent;
        }
    }

    let control_type = unsafe { element.CurrentControlType() }
        .map(|value| value.0)
        .unwrap_or(0);
    let class_name = unsafe { element.CurrentClassName() }
        .map(|value| value.to_string())
        .unwrap_or_default();
    let is_password = unsafe { element.CurrentIsPassword() }
        .map(|value| value.as_bool())
        .unwrap_or(false);
    let is_enabled = unsafe { element.CurrentIsEnabled() }
        .map(|value| value.as_bool())
        .unwrap_or(false);

    let text =
        unsafe { element.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId) }.ok();
    let text2 =
        unsafe { element.GetCurrentPatternAs::<IUIAutomationTextPattern2>(UIA_TextPattern2Id) }
            .ok();
    let value =
        unsafe { element.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId) }
            .ok();

    let mut selection_count = 0;
    let mut text_read_only = None;
    if let Some(pattern) = &text {
        if let Ok(array) = unsafe { pattern.GetSelection() } {
            selection_count = unsafe { array.Length() }.unwrap_or(0);
            if selection_count > 0 {
                if let Ok(range) = unsafe { array.GetElement(0) } {
                    if let Ok(variant) =
                        unsafe { range.GetAttributeValue(UIA_IsReadOnlyAttributeId) }
                    {
                        text_read_only = variant_bool(&variant);
                    }
                }
            }
        }
    }

    let mut caret_active = None;
    if let Some(pattern) = &text2 {
        let mut active = BOOL::default();
        if unsafe { pattern.GetCaretRange(&mut active) }.is_ok() {
            caret_active = Some(active.as_bool());
        }
    }

    let value_read_only = value
        .as_ref()
        .and_then(|pattern| unsafe { pattern.CurrentIsReadOnly().ok() })
        .map(|value| value.as_bool());

    let is_read_only = combine_read_only(value_read_only, text_read_only);

    let class_lower = class_name.to_ascii_lowercase();
    let region_hint = if class_lower.contains("xterm") || class_lower.contains("terminal") {
        Some("terminal".to_string())
    } else {
        None
    };

    let snapshot = UiaSnapshot {
        control_type,
        class_name,
        is_password,
        is_enabled,
        is_read_only,
        has_text_pattern: text.is_some(),
        has_text_pattern2: text2.is_some(),
        has_value_pattern: value.is_some(),
        selection_count,
        caret_active,
        native_hwnd_zero,
        ancestor_verified,
        ancestor_depth,
        foreground_stable,
        region_hint,
        elapsed_ms: started.elapsed().as_millis() as u64,
    };

    (Some(snapshot), Some(element))
}

fn compare_element(
    automation: &IUIAutomation,
    foreground: HWND,
    last: &Option<IUIAutomationElement>,
) -> Option<bool> {
    let previous = last.as_ref()?;
    let current = unsafe { automation.GetFocusedElement() }.ok()?;

    let handle = unsafe { current.CurrentNativeWindowHandle() }.unwrap_or_default();
    if !handle.0.is_null() && !context::belongs_to(foreground, handle) {
        return Some(false);
    }

    unsafe { automation.CompareElements(previous, &current) }
        .map(|value| value.as_bool())
        .ok()
}

fn variant_bool(variant: &VARIANT) -> Option<bool> {
    unsafe {
        let inner = &variant.Anonymous.Anonymous;
        if inner.vt != VT_BOOL {
            return None;
        }
        let value: VARIANT_BOOL = inner.Anonymous.boolVal;
        Some(value.0 != 0)
    }
}

#[cfg(test)]
mod tests {
    use super::combine_read_only;

    #[test]
    fn read_only_any_true_denies() {
        assert_eq!(combine_read_only(Some(true), None), Some(true));
        assert_eq!(combine_read_only(None, Some(true)), Some(true));
        assert_eq!(combine_read_only(Some(true), Some(false)), Some(true));
    }

    #[test]
    fn read_only_needs_two_clear_false() {
        assert_eq!(combine_read_only(Some(false), Some(false)), Some(false));
        assert_eq!(combine_read_only(Some(false), None), None);
        assert_eq!(combine_read_only(None, Some(false)), None);
        assert_eq!(combine_read_only(None, None), None);
    }
}
