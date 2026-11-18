use std::time::{Duration, Instant};

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, SendInput, SetFocus, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT,
    KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, VIRTUAL_KEY, VK_CONTROL, VK_LWIN, VK_MENU, VK_RETURN,
    VK_RWIN, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetForegroundWindow,
    GetWindowTextW, IsHungAppWindow, IsWindow, LoadCursorW, PeekMessageW, RegisterClassW,
    SetForegroundWindow, ShowWindow, TranslateMessage, ES_MULTILINE, HMENU, IDC_ARROW, MSG,
    PM_REMOVE, SW_SHOW, WINDOW_EX_STYLE, WINDOW_STYLE, WNDCLASSW, WS_CHILD, WS_OVERLAPPEDWINDOW,
    WS_VISIBLE, WS_VSCROLL,
};

use crate::capture::{Report, Target, Timings};
use crate::{clipboard, context, foreground, logging, uia};

const MAX_UNICODE_UNITS: usize = 2_000;

enum Plan {
    Insert(String),
    CopyOnly(&'static str),
}

pub enum Outcome {
    Rejected { reason: &'static str, copied: bool },
    Accepted,
    Partial { reason: &'static str },
}

impl Outcome {
    pub fn describe(&self) -> String {
        match self {
            Outcome::Rejected {
                reason,
                copied: true,
            } => {
                format!("未输入，已复制（{reason}）")
            }
            Outcome::Rejected {
                reason,
                copied: false,
            } => format!("未输入，复制失败（{reason}）"),
            Outcome::Accepted => "已发送，请确认目标已收到".to_string(),
            Outcome::Partial { reason } => {
                format!("未确认输入（{reason}），请检查目标")
            }
        }
    }
}

pub fn run(report: &Report, raw: &str, monitor_ok: bool) -> Outcome {
    let target = HWND(report.target.hwnd as *mut core::ffi::c_void);
    unsafe {
        let _ = SetForegroundWindow(target);
    }

    if !monitor_ok {
        let copied = clipboard::set_text(raw);
        logging::warn("inject rejected: foreground monitor unavailable");
        return Outcome::Rejected {
            reason: "monitor_unavailable",
            copied,
        };
    }

    let plan = plan_text(
        &report.identity,
        &report.capability,
        &report.region,
        report.target.target_higher,
        report.target.integrity == "unknown" || report.target.own_integrity == "unknown",
        raw,
    );

    let text = match plan {
        Plan::Insert(text) => text,
        Plan::CopyOnly(reason) => {
            let copied = clipboard::set_text(raw);
            logging::info(&format!("inject copy-only: {reason} copied={copied}"));
            return Outcome::Rejected { reason, copied };
        }
    };

    if let Err(reason) = guard(report) {
        let copied = clipboard::set_text(raw);
        logging::warn(&format!("inject rejected: {reason} copied={copied}"));
        return Outcome::Rejected { reason, copied };
    }

    if !final_check(report, target) {
        let copied = clipboard::set_text(raw);
        logging::warn("inject rejected: foreground_changed_at_send");
        return Outcome::Rejected {
            reason: "foreground_changed_at_send",
            copied,
        };
    }

    let inputs = build_inputs(&text);
    let sent = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };

    if sent == 0 {
        logging::warn("SendInput accepted 0 events (blocked)");
        return Outcome::Partial {
            reason: "sendinput_blocked",
        };
    }

    if sent as usize != inputs.len() {
        logging::warn(&format!(
            "SendInput accepted {sent} of {} events",
            inputs.len()
        ));
        return Outcome::Partial {
            reason: "sendinput_short",
        };
    }

    logging::info(&format!(
        "inject accepted: {} UTF-16 units, {} events",
        text.encode_utf16().count(),
        inputs.len()
    ));
    Outcome::Accepted
}

fn plan_text(
    identity: &str,
    capability: &str,
    region: &str,
    target_higher: bool,
    integrity_unknown: bool,
    raw: &str,
) -> Plan {
    if raw.is_empty() {
        return Plan::CopyOnly("empty");
    }
    if identity != "verified" {
        return Plan::CopyOnly("identity_unknown");
    }
    if capability != "text" {
        return Plan::CopyOnly("capability");
    }
    if target_higher {
        return Plan::CopyOnly("elevated");
    }
    if integrity_unknown {
        return Plan::CopyOnly("integrity_unknown");
    }
    if has_forbidden_control(raw) {
        return Plan::CopyOnly("control_char");
    }

    let mut text = raw.replace("\r\n", "\n").replace('\r', "\n");
    if text.contains('\t') {
        text = text.replace('\t', "    ");
    }

    if text.contains('\n') && region != "text_editor" {
        return Plan::CopyOnly("multiline_region");
    }

    let text = text.replace('\n', "\r");
    if text.encode_utf16().count() > MAX_UNICODE_UNITS {
        return Plan::CopyOnly("too_long");
    }

    Plan::Insert(text)
}

fn has_forbidden_control(text: &str) -> bool {
    text.chars().any(|character| {
        let value = character as u32;
        (value < 0x20 && character != '\t' && character != '\n' && character != '\r')
            || (0x7F..=0x9F).contains(&value)
            || value == 0x2028
            || value == 0x2029
    })
}

fn guard(report: &Report) -> Result<(), &'static str> {
    let target = HWND(report.target.hwnd as *mut core::ffi::c_void);

    if !unsafe { IsWindow(Some(target)) }.as_bool() {
        return Err("target_gone");
    }

    if context::process_created(report.target.pid) != Some(report.target.created) {
        return Err("process_changed");
    }

    if foreground::overflowed() {
        return Err("foreground_overflow");
    }

    for event in foreground::events() {
        if event.pid == std::process::id() || event.hwnd == report.target.hwnd {
            continue;
        }
        return Err("foreground_sequence");
    }

    unsafe {
        let _ = SetForegroundWindow(target);
    }
    if !wait_foreground(target, Duration::from_millis(300)) {
        return Err("foreground_restore");
    }

    let focused = context::focused_window(report.target.tid).ok_or("focus_unknown")?;
    if report.target.focused_hwnd != 0 && focused.0 as isize != report.target.focused_hwnd {
        return Err("focus_changed");
    }

    if report.target.selection.is_none() {
        logging::info("position_unverified");
    }
    if report.target.focused_is_editable && context::native_edit_read_only(focused) != Some(false) {
        // Fail closed: a native edit whose style cannot be read must not be
        // treated as writable.
        return Err("read_only_unknown");
    }

    if report.identity_source == "uia" {
        match uia::compare_last(target, Duration::from_millis(50)) {
            Some(true) => {}
            Some(false) => return Err("identity_changed"),
            None => return Err("identity_unknown"),
        }
    }

    if !wait_modifiers_released(Duration::from_millis(300)) {
        return Err("modifiers_held");
    }

    if let Some([start, end]) = report.target.selection {
        let edit = HWND(report.target.focused_hwnd as *mut core::ffi::c_void);
        if let Some((current_start, current_end)) = context::read_selection(edit) {
            if current_start != start || current_end != end {
                return Err("position_changed");
            }
        }
    }

    Ok(())
}

fn final_check(report: &Report, target: HWND) -> bool {
    if unsafe { GetForegroundWindow() } != target {
        return false;
    }

    if report.identity_source == "uia"
        && uia::compare_last(target, Duration::from_millis(50)) != Some(true)
    {
        return false;
    }

    // Re-check focus after the last blocking wait (UIA compare).
    match context::focused_window(report.target.tid) {
        Some(focused)
            if report.target.focused_hwnd == 0
                || focused.0 as isize == report.target.focused_hwnd => {}
        _ => return false,
    }

    if report.target.focused_is_editable {
        let focused = HWND(report.target.focused_hwnd as *mut core::ffi::c_void);
        if context::native_edit_read_only(focused) != Some(false) {
            return false;
        }
    }

    unsafe { GetForegroundWindow() == target }
}

fn wait_foreground(target: HWND, timeout: Duration) -> bool {
    let started = Instant::now();
    while started.elapsed() < timeout {
        if unsafe { GetForegroundWindow() } == target {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    unsafe { GetForegroundWindow() == target }
}

fn wait_modifiers_released(timeout: Duration) -> bool {
    let started = Instant::now();
    while started.elapsed() < timeout {
        if !modifier_down(VK_CONTROL)
            && !modifier_down(VK_MENU)
            && !modifier_down(VK_SHIFT)
            && !modifier_down(VK_LWIN)
            && !modifier_down(VK_RWIN)
            && !modifier_down(VK_RETURN)
        {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

fn modifier_down(key: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY) -> bool {
    unsafe { GetAsyncKeyState(key.0 as i32) < 0 }
}

fn build_inputs(text: &str) -> Vec<INPUT> {
    let mut inputs = Vec::new();
    for unit in text.encode_utf16() {
        inputs.push(make_input(unit, false));
        inputs.push(make_input(unit, true));
    }
    inputs
}

fn make_input(scan: u16, key_up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(0),
                wScan: scan,
                dwFlags: if key_up {
                    KEYEVENTF_UNICODE | KEYEVENTF_KEYUP
                } else {
                    KEYEVENTF_UNICODE
                },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

unsafe extern "system" fn test_wndproc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    DefWindowProcW(hwnd, message, wparam, lparam)
}

pub fn selftest() -> bool {
    unsafe {
        let module = match GetModuleHandleW(None) {
            Ok(module) => module,
            Err(_) => return false,
        };
        let instance = HINSTANCE(module.0);
        let class = w!("QuickPhrase.InjectTest");

        let window_class = WNDCLASSW {
            style: Default::default(),
            lpfnWndProc: Some(test_wndproc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: instance,
            hIcon: Default::default(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hbrBackground: Default::default(),
            lpszMenuName: PCWSTR::null(),
            lpszClassName: class,
        };
        let _ = RegisterClassW(&window_class);

        let window = match CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            class,
            w!("QuickPhrase inject test"),
            WS_OVERLAPPEDWINDOW,
            100,
            100,
            520,
            220,
            None,
            None,
            Some(instance),
            None,
        ) {
            Ok(window) => window,
            Err(_) => return false,
        };

        let edit = match CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            w!("EDIT"),
            w!(""),
            WS_CHILD | WS_VISIBLE | WS_VSCROLL | WINDOW_STYLE(ES_MULTILINE as u32),
            10,
            10,
            480,
            150,
            Some(window),
            Some(HMENU(std::ptr::without_provenance_mut(1))),
            Some(instance),
            None,
        ) {
            Ok(edit) => edit,
            Err(_) => {
                let _ = DestroyWindow(window);
                return false;
            }
        };

        let _ = ShowWindow(window, SW_SHOW);
        force_foreground(window);
        let _ = SetFocus(Some(edit));

        let pid = std::process::id();
        let tid = GetCurrentThreadId();
        let report = Report {
            captured_at: String::new(),
            identity: "verified".to_string(),
            identity_source: "hwnd".to_string(),
            capability: "text".to_string(),
            region: "text_editor".to_string(),
            timings_ms: Timings {
                native: 0,
                uia: 0,
                total: 0,
            },
            target: Target {
                hwnd: window.0 as isize,
                pid,
                tid,
                created: context::process_created(pid).unwrap_or(0),
                exe: String::new(),
                class: "QuickPhrase.InjectTest".to_string(),
                title: String::new(),
                focused_hwnd: edit.0 as isize,
                focused_class: "Edit".to_string(),
                focused_is_child: true,
                focused_is_editable: true,
                focused_is_password: false,
                focused_is_readonly: false,
                selection: context::read_selection(edit).map(|(start, end)| [start, end]),
                caret: None,
                integrity: "medium".to_string(),
                own_integrity: "medium".to_string(),
                target_higher: false,
            },
            uia: None,
            foreground_events: Vec::new(),
        };

        let outcome = run(&report, "Injected 测试\r\n第二行", true);
        pump(Duration::from_millis(400));

        let mut buffer = [0u16; 512];
        let length = GetWindowTextW(edit, &mut buffer);
        let text = String::from_utf16_lossy(&buffer[..length.max(0) as usize]);
        let _ = DestroyWindow(window);

        let ok = matches!(outcome, Outcome::Accepted) && text == "Injected 测试\r\n第二行";
        logging::info(&format!(
            "inject selftest: outcome={} text={text:?}",
            outcome.describe()
        ));
        ok
    }
}

fn force_foreground(window: HWND) {
    unsafe {
        let _ = SetForegroundWindow(window);
        if GetForegroundWindow() == window {
            return;
        }
        let foreground = GetForegroundWindow();
        if foreground.0.is_null() || IsHungAppWindow(foreground).as_bool() {
            return;
        }
        let mut pid = 0u32;
        let foreground_thread = windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId(
            foreground,
            Some(&mut pid),
        );
        let current_thread = GetCurrentThreadId();
        if foreground_thread != 0 && foreground_thread != current_thread {
            let _ = AttachThreadInput(current_thread, foreground_thread, true);
            let _ = SetForegroundWindow(window);
            let _ = SetFocus(Some(window));
            let _ = AttachThreadInput(current_thread, foreground_thread, false);
        }
    }
}

fn pump(duration: Duration) {
    let end = Instant::now() + duration;
    let mut message = MSG::default();
    unsafe {
        while Instant::now() < end {
            while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{plan_text, Plan};

    fn plan(raw: &str) -> Plan {
        plan_text("verified", "text", "text_editor", false, false, raw)
    }

    #[test]
    fn multiline_text_editor_uses_cr() {
        match plan("a\nb") {
            Plan::Insert(text) => assert_eq!(text, "a\rb"),
            _ => panic!("expected insert"),
        }
    }

    #[test]
    fn multiline_unknown_region_is_copy_only() {
        match plan_text("verified", "text", "unknown", false, false, "a\nb") {
            Plan::CopyOnly(reason) => assert_eq!(reason, "multiline_region"),
            _ => panic!("expected copy-only"),
        }
    }

    #[test]
    fn tabs_become_four_spaces() {
        match plan("a\tb") {
            Plan::Insert(text) => assert_eq!(text, "a    b"),
            _ => panic!("expected insert"),
        }
    }

    #[test]
    fn forbidden_control_char_is_copy_only() {
        match plan("a\u{1b}b") {
            Plan::CopyOnly(reason) => assert_eq!(reason, "control_char"),
            _ => panic!("expected copy-only"),
        }
    }

    #[test]
    fn long_text_is_copy_only() {
        let long = "a".repeat(2_001);
        match plan(&long) {
            Plan::CopyOnly(reason) => assert_eq!(reason, "too_long"),
            _ => panic!("expected copy-only"),
        }
    }

    #[test]
    fn non_text_capability_is_copy_only() {
        match plan_text("verified", "readonly", "text_editor", false, false, "a") {
            Plan::CopyOnly(reason) => assert_eq!(reason, "capability"),
            _ => panic!("expected copy-only"),
        }
    }

    #[test]
    fn exactly_two_thousand_units_is_allowed() {
        match plan(&"a".repeat(2_000)) {
            Plan::Insert(text) => assert_eq!(text.encode_utf16().count(), 2_000),
            _ => panic!("expected insert"),
        }
    }

    #[test]
    fn emoji_counts_as_two_units() {
        let thousand = "\u{1F600}".repeat(1_000);
        match plan(&thousand) {
            Plan::Insert(text) => assert_eq!(text.encode_utf16().count(), 2_000),
            _ => panic!("expected insert"),
        }
        match plan(&"\u{1F600}".repeat(1_001)) {
            Plan::CopyOnly(reason) => assert_eq!(reason, "too_long"),
            _ => panic!("expected copy-only"),
        }
    }

    #[test]
    fn crlf_and_lone_cr_become_cr() {
        for raw in ["a\r\nb", "a\rb"] {
            match plan(raw) {
                Plan::Insert(text) => assert_eq!(text, "a\rb"),
                _ => panic!("expected insert"),
            }
        }
    }

    #[test]
    fn c1_and_unicode_separators_are_copy_only() {
        for raw in ["a\u{85}b", "a\u{2028}b", "a\u{2029}b"] {
            match plan(raw) {
                Plan::CopyOnly(reason) => assert_eq!(reason, "control_char"),
                _ => panic!("expected copy-only"),
            }
        }
    }

    #[test]
    fn terminal_single_line_is_allowed() {
        match plan_text("verified", "text", "terminal", false, false, "Get-Process") {
            Plan::Insert(text) => assert_eq!(text, "Get-Process"),
            _ => panic!("expected insert"),
        }
    }

    #[test]
    fn identity_elevation_and_integrity_are_copy_only() {
        assert!(matches!(
            plan_text("unknown", "text", "text_editor", false, false, "a"),
            Plan::CopyOnly("identity_unknown")
        ));
        assert!(matches!(
            plan_text("verified", "text", "text_editor", true, false, "a"),
            Plan::CopyOnly("elevated")
        ));
        assert!(matches!(
            plan_text("verified", "text", "text_editor", false, true, "a"),
            Plan::CopyOnly("integrity_unknown")
        ));
    }

    #[test]
    fn empty_text_is_copy_only() {
        assert!(matches!(plan(""), Plan::CopyOnly("empty")));
    }
}
