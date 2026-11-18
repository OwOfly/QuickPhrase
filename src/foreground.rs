use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::Mutex;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde::Serialize;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, GetWindowThreadProcessId, PeekMessageW, PostThreadMessageW,
    TranslateMessage, EVENT_SYSTEM_FOREGROUND, MSG, PM_NOREMOVE, WINEVENT_OUTOFCONTEXT, WM_QUIT,
};

#[derive(Clone, Debug, Serialize)]
pub struct ForegroundEvent {
    pub seq: u64,
    pub hwnd: isize,
    pub pid: u32,
}

static EVENTS: Mutex<Vec<ForegroundEvent>> = Mutex::new(Vec::new());
static SEQ: AtomicU64 = AtomicU64::new(0);

pub struct Monitor {
    thread: Option<JoinHandle<()>>,
    thread_id: u32,
}

pub fn start() -> Option<Monitor> {
    if let Ok(mut events) = EVENTS.lock() {
        events.clear();
    }
    SEQ.store(0, Ordering::Relaxed);

    let (tx, rx) = mpsc::channel();
    let handle = thread::Builder::new()
        .name("quickphrase-foreground".into())
        .spawn(move || {
            let mut message = MSG::default();
            unsafe {
                let _ = PeekMessageW(&mut message, None, 0, 0, PM_NOREMOVE);
            }

            let thread_id = unsafe { GetCurrentThreadId() };

            let hook = unsafe {
                SetWinEventHook(
                    EVENT_SYSTEM_FOREGROUND,
                    EVENT_SYSTEM_FOREGROUND,
                    None,
                    Some(callback),
                    0,
                    0,
                    WINEVENT_OUTOFCONTEXT,
                )
            };

            if hook == HWINEVENTHOOK::default() {
                crate::logging::error("SetWinEventHook failed");
                let _ = tx.send(None);
                return;
            }
            let _ = tx.send(Some(thread_id));

            loop {
                let result = unsafe { GetMessageW(&mut message, None, 0, 0) };
                if result.0 <= 0 {
                    break;
                }
                unsafe {
                    let _ = TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }

            if hook != HWINEVENTHOOK::default() {
                unsafe {
                    let _ = UnhookWinEvent(hook);
                }
            }
        })
        .ok()?;

    let thread_id = rx.recv_timeout(Duration::from_millis(500)).ok()??;
    Some(Monitor {
        thread: Some(handle),
        thread_id,
    })
}

impl Drop for Monitor {
    fn drop(&mut self) {
        unsafe {
            let _ = PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
        }
        if let Some(handle) = self.thread.take() {
            // Detach instead of join: a hook thread stuck in a provider callback
            // must never block panel hide or application shutdown.
            drop(handle);
        }
    }
}

unsafe extern "system" fn callback(
    _hook: HWINEVENTHOOK,
    _event: u32,
    hwnd: HWND,
    _id_object: i32,
    _id_child: i32,
    _thread: u32,
    _time: u32,
) {
    let mut pid = 0u32;
    unsafe {
        let _ = GetWindowThreadProcessId(hwnd, Some(&mut pid));
    }

    if let Ok(mut events) = EVENTS.lock() {
        if events.len() >= 128 {
            events.remove(0);
        }
        events.push(ForegroundEvent {
            seq: SEQ.fetch_add(1, Ordering::Relaxed),
            hwnd: hwnd.0 as isize,
            pid,
        });
    }
}

pub fn events() -> Vec<ForegroundEvent> {
    EVENTS
        .lock()
        .map(|events| events.clone())
        .unwrap_or_default()
}
