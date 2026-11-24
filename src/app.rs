use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::HBRUSH;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::{
    capture, clipboard, foreground, hotkey, inject, logging, manage, panel, platform, search,
    settings_ui, single_instance, storage, template, tray, uia,
};

const ID_OPEN: u32 = 1001;
const ID_MANAGE: u32 = 1002;
const ID_PAUSE: u32 = 1003;
const ID_SETTINGS: u32 = 1004;
const ID_ABOUT: u32 = 1005;
const ID_EXIT: u32 = 1006;
const ID_RESTORE_CLIP: u32 = 1007;
const ID_QUICK_ADD: u32 = 1008;

const SELFTEST_TIMER: usize = 1;
const WM_UIA_DEGRADED: u32 = WM_APP + 0x71;

/// Main window handle readable from the UIA worker thread.
static MAIN_HWND: AtomicIsize = AtomicIsize::new(0);
static UIA_DEGRADED_PENDING: AtomicBool = AtomicBool::new(false);
static UIA_DEGRADED_NOTICE: AtomicBool = AtomicBool::new(false);

const TIP_NORMAL: &str = "QuickPhrase 快语";
const TIP_PAUSED: &str = "QuickPhrase（快捷键已暂停）";

#[derive(Default)]
struct State {
    hwnd: HWND,
    icon: HICON,
    hotkey_ok: bool,
    paused: bool,
    tray_ok: bool,
    selftest: bool,
    activate_message: u32,
    taskbar_message: u32,
    /// Active per-snippet registrations: registration id, snippet id, hotkey text.
    snippet_hotkeys: Vec<(i32, String, String)>,
    /// Monotonic allocator; an id is never reused for another snippet, so a
    /// queued WM_HOTKEY can never be routed to the wrong phrase after a refresh.
    next_snippet_hotkey_id: i32,
}

static DIRECT_INJECTING: AtomicBool = AtomicBool::new(false);

/// Where the app is in the exit/session-end flow. New injections stay blocked
/// from the moment a prompt opens until that prompt is answered or the
/// session-end decision is revoked. There is deliberately no timeout: reopening
/// injections while a shutdown is pending can kill one midway.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ExitGate {
    None,
    Confirming,
    SessionEnding,
}

static EXIT_GATE: Mutex<ExitGate> = Mutex::new(ExitGate::None);

fn exit_gate() -> ExitGate {
    EXIT_GATE.lock().map_or(ExitGate::None, |gate| *gate)
}

fn set_exit_gate(gate: ExitGate) {
    if let Ok(mut slot) = EXIT_GATE.lock() {
        *slot = gate;
    }
}

fn clear_exit_gate(expected: ExitGate) {
    if let Ok(mut slot) = EXIT_GATE.lock() {
        if *slot == expected {
            *slot = ExitGate::None;
        }
    }
}

pub(crate) fn exit_pending() -> bool {
    exit_gate_blocks(exit_gate())
}

/// Posted when insertions degrade to copy-only. The main thread shows at most
/// one balloon per session, and keeps the request pending while the tray is
/// unavailable so a recreated tray can still deliver it.
pub(crate) fn notify_uia_degraded() {
    UIA_DEGRADED_PENDING.store(true, Ordering::Relaxed);
    let raw = MAIN_HWND.load(Ordering::Relaxed);
    if raw == 0 {
        return;
    }
    let hwnd = HWND(raw as *mut core::ffi::c_void);
    unsafe {
        let _ = PostMessageW(Some(hwnd), WM_UIA_DEGRADED, WPARAM(0), LPARAM(0));
    }
}

fn show_uia_degraded_notice(hwnd: HWND) {
    if UIA_DEGRADED_NOTICE.load(Ordering::Relaxed) {
        return;
    }
    let tray_ok = STATE.with(|state| state.borrow().tray_ok);
    if !tray_ok {
        return;
    }
    if UIA_DEGRADED_NOTICE.swap(true, Ordering::Relaxed) {
        return;
    }
    if tray::balloon(hwnd, "QuickPhrase", "输入增强暂不可用，本次将仅复制") {
        UIA_DEGRADED_PENDING.store(false, Ordering::Relaxed);
    } else {
        // Keep the request pending so TaskbarCreated can retry it.
        UIA_DEGRADED_NOTICE.store(false, Ordering::Relaxed);
    }
}

fn exit_gate_blocks(gate: ExitGate) -> bool {
    !matches!(gate, ExitGate::None)
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

pub fn run(selftest: bool, bench: bool, started: Instant) -> i32 {
    let _mutex = match single_instance::acquire() {
        single_instance::Acquire::First(handle) => handle,
        single_instance::Acquire::AlreadyRunning => {
            // Smoke runs may signal only their own known primary process.
            let expected_pid = std::env::var_os("QUICKPHRASE_TEST_WAKE_PID").map(|value| {
                value
                    .to_str()
                    .and_then(|text| text.parse::<u32>().ok())
                    .unwrap_or(0)
            });
            if expected_pid == Some(0) {
                logging::warn("another instance is running; isolated test will not signal it");
                return 2;
            }
            if single_instance::signal_existing(expected_pid) {
                logging::info("signalled the running instance");
            } else {
                logging::warn("another instance is running but did not respond to activation");
                if expected_pid.is_some() {
                    return 2;
                }
                platform::error_box(
                    "QuickPhrase 已在运行",
                    "另一个 QuickPhrase 实例正在运行，但未能唤起它。请通过托盘图标打开。",
                );
            }
            return 0;
        }
        single_instance::Acquire::Failed(code) => {
            logging::error(&format!("CreateMutexW failed: {code}"));
            platform::error_box(
                "QuickPhrase 启动失败",
                &format!("无法创建单实例互斥量（错误码 {code}）。"),
            );
            return 2;
        }
    };

    let module = unsafe { GetModuleHandleW(None) }.unwrap_or_default();
    let instance = HINSTANCE(module.0);

    // Icon resource id 1 from app.rc; fall back to the system icon if missing.
    let icon = unsafe {
        // Resource id 1 is the MAKEINTRESOURCE form: a small integer as a
        // pointer, which is the documented Win32 idiom, not a real address.
        #[allow(clippy::manual_dangling_ptr)]
        let icon_id = PCWSTR(1 as *const u16);
        LoadIconW(Some(instance), icon_id).or_else(|_| LoadIconW(None, IDI_APPLICATION))
    }
    .unwrap_or_default();
    let cursor = unsafe { LoadCursorW(None, IDC_ARROW) }.unwrap_or_default();

    let window_class = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(window_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: instance,
        hIcon: icon,
        hCursor: cursor,
        hbrBackground: HBRUSH::default(),
        lpszMenuName: PCWSTR::null(),
        lpszClassName: w!("QuickPhrase.MainWindow"),
    };

    if unsafe { RegisterClassW(&window_class) } == 0 {
        logging::error("RegisterClassW failed");
        platform::error_box("QuickPhrase 启动失败", "无法注册窗口类。");
        return 2;
    }

    let hwnd = match unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW,
            w!("QuickPhrase.MainWindow"),
            w!("QuickPhrase"),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance),
            None,
        )
    } {
        Ok(hwnd) => hwnd,
        Err(err) => {
            logging::error(&format!("CreateWindowExW failed: {err}"));
            platform::error_box("QuickPhrase 启动失败", &format!("无法创建窗口：{err}"));
            return 2;
        }
    };

    let activate_message = single_instance::activation_message();
    let taskbar_message = unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) };

    STATE.with(|state| {
        *state.borrow_mut() = State {
            hwnd,
            icon,
            selftest,
            activate_message,
            taskbar_message,
            next_snippet_hotkey_id: hotkey::SNIPPET_ID_BASE,
            ..State::default()
        };
    });
    MAIN_HWND.store(hwnd.0 as isize, Ordering::Relaxed);

    storage::init();
    // Diagnostic runs always exercise hotkey registration, even when the user
    // left the app paused; the smoke scripts assert that path.
    let paused_at_start = !(selftest || bench)
        && storage::settings_clone()
            .map(|settings| settings.paused)
            .unwrap_or(false);
    let tray_ok = tray::add(
        hwnd,
        icon,
        if paused_at_start {
            TIP_PAUSED
        } else {
            TIP_NORMAL
        },
    );
    STATE.with(|state| state.borrow_mut().tray_ok = tray_ok);
    if tray_ok {
        logging::info("tray icon added");
    } else {
        logging::error("Shell_NotifyIconW(NIM_ADD) failed");
        platform::error_box("QuickPhrase", "无法添加托盘图标。");
    }

    let mut hotkey_registered = false;
    let hotkey_text = current_hotkey();
    if paused_at_start {
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.paused = true;
            state.hotkey_ok = false;
        });
        logging::info("startup paused by settings");
    } else {
        match hotkey::register_text(hwnd, &hotkey_text) {
            Ok(_) => {
                hotkey_registered = true;
                STATE.with(|state| {
                    let mut state = state.borrow_mut();
                    state.hotkey_ok = true;
                    state.paused = false;
                });
                logging::info(&format!("hotkey registered: {hotkey_text}"));
            }
            Err(code) => {
                logging::error(&format!("RegisterHotKey failed: {code}"));
                if tray_ok {
                    tray::balloon(
                        hwnd,
                        "快捷键注册失败",
                        &format!("{hotkey_text} 已被其他程序占用。"),
                    );
                }
                platform::error_box(
                    "快捷键冲突",
                    &format!(
                        "无法注册 {hotkey_text}（错误码 {code}）。\n\
                         请关闭占用该快捷键的程序，或在设置中修改。"
                    ),
                );
            }
        }
    }

    if uia::warmup(Duration::from_millis(800)) {
        logging::info("uia warmup ok");
    } else {
        logging::warn("uia warmup failed; captures will be copy-only");
    }

    refresh_snippet_hotkeys();

    panel::precreate();

    logging::info(&format!(
        "ready in {} ms (pid {})",
        started.elapsed().as_millis(),
        std::process::id()
    ));

    if bench {
        // Startup benchmark: exit as soon as the app is ready.
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
    } else if selftest {
        unsafe {
            let _ = SetTimer(Some(hwnd), SELFTEST_TIMER, 600, None);
        }
    }

    let mut message = MSG::default();
    loop {
        let result = unsafe { GetMessageW(&mut message, None, 0, 0) };
        if result.0 <= 0 {
            break;
        }
        unsafe {
            if manage::is_dialog_message(&message)
                || settings_ui::is_dialog_message(&message)
                || crate::about_ui::is_dialog_message(&message)
            {
                continue;
            }
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }

    if STATE.with(|state| state.borrow().tray_ok) {
        tray::remove(hwnd);
    }
    if STATE.with(|state| state.borrow().hotkey_ok) {
        hotkey::unregister(hwnd);
    }
    logging::info("message loop ended");

    if (selftest || bench) && !hotkey_registered {
        logging::error("startup selftest failed: hotkey was not registered");
        return 3;
    }

    0
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let (activate_message, taskbar_message) = STATE.with(|state| {
        let state = state.borrow();
        (state.activate_message, state.taskbar_message)
    });

    if activate_message != 0 && message == activate_message {
        logging::info("second instance activation requested");
        open_panel();
        return LRESULT(0);
    }

    if taskbar_message != 0 && message == taskbar_message {
        logging::info("TaskbarCreated received; re-adding tray icon");
        let (icon, paused) = STATE.with(|state| {
            let state = state.borrow();
            (state.icon, state.paused)
        });
        let tip = if paused { TIP_PAUSED } else { TIP_NORMAL };
        let ok = tray::add(hwnd, icon, tip);
        if !ok {
            logging::error("TaskbarCreated: re-adding the tray icon failed");
        }
        STATE.with(|state| state.borrow_mut().tray_ok = ok);
        if ok && UIA_DEGRADED_PENDING.load(Ordering::Relaxed) {
            show_uia_degraded_notice(hwnd);
        }
        return LRESULT(0);
    }

    match message {
        panel::WM_INJECT_DONE => {
            panel::finish_insert(wparam.0 as u64);
            if let Some(text) = panel::take_inject_result() {
                tray::balloon(hwnd, "QuickPhrase", &text);
            }
            LRESULT(0)
        }
        WM_HOTKEY => {
            let id = wparam.0 as i32;
            if id == hotkey::HOTKEY_ID {
                logging::info("hotkey fired");
                hotkey_panel();
            } else if id >= hotkey::SNIPPET_ID_BASE {
                let snippet_id = STATE.with(|state| {
                    state
                        .borrow()
                        .snippet_hotkeys
                        .iter()
                        .find(|(registered, _, _)| *registered == id)
                        .map(|(_, snippet_id, _)| snippet_id.clone())
                });
                if let Some(snippet_id) = snippet_id {
                    logging::info(&format!(
                        "snippet hotkey fired id={id} snippet={snippet_id}"
                    ));
                    hotkey_snippet(&snippet_id);
                } else {
                    logging::warn(&format!("snippet hotkey fired for stale id={id}"));
                }
            }
            LRESULT(0)
        }
        WM_COMMAND => {
            handle_command((wparam.0 & 0xffff) as u32);
            LRESULT(0)
        }
        tray::WM_TRAY => {
            let event = (lparam.0 & 0xffff) as u32;
            match event {
                WM_RBUTTONUP | WM_CONTEXTMENU => show_menu(hwnd),
                WM_LBUTTONDBLCLK => {
                    let hwnd = STATE.with(|state| state.borrow().hwnd);
                    manage::show(hwnd);
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_TIMER => {
            if STATE.with(|state| state.borrow().selftest) && wparam.0 == SELFTEST_TIMER {
                logging::info("selftest finished");
                let _ = DestroyWindow(hwnd);
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            request_exit();
            LRESULT(0)
        }
        WM_DESTROY => {
            MAIN_HWND.store(0, Ordering::Relaxed);
            panel::shutdown();
            manage::shutdown();
            settings_ui::shutdown();
            crate::tray_menu::shutdown();
            crate::about_ui::shutdown();
            let (tray_ok, hotkey_ok) = STATE.with(|state| {
                let state = state.borrow();
                (state.tray_ok, state.hotkey_ok)
            });
            if tray_ok {
                tray::remove(hwnd);
            }
            if hotkey_ok {
                hotkey::unregister(hwnd);
            }
            let snippet_hotkeys =
                STATE.with(|state| std::mem::take(&mut state.borrow_mut().snippet_hotkeys));
            for (id, _, _) in snippet_hotkeys {
                hotkey::unregister_id(hwnd, id);
            }
            STATE.with(|state| {
                let mut state = state.borrow_mut();
                state.tray_ok = false;
                state.hotkey_ok = false;
            });
            storage::flush_usage();
            PostQuitMessage(0);
            LRESULT(0)
        }
        WM_QUERYENDSESSION => {
            if exit_pending() {
                logging::warn("session end deferred: exit or session end already pending");
                return LRESULT(0);
            }
            if panel::is_injecting() || DIRECT_INJECTING.load(Ordering::Relaxed) {
                logging::warn("session end deferred: injection in flight");
                return LRESULT(0);
            }
            set_exit_gate(ExitGate::Confirming);
            if !manage::confirm_exit() {
                clear_exit_gate(ExitGate::Confirming);
                return LRESULT(0);
            }
            // The confirmation card pumps messages; a snippet hotkey can start
            // an injection while it is open. Re-check before allowing the end.
            if panel::is_injecting() || DIRECT_INJECTING.load(Ordering::Relaxed) {
                clear_exit_gate(ExitGate::Confirming);
                logging::warn("session end deferred: injection started during confirmation");
                return LRESULT(0);
            }
            manage::discard_draft();
            // Keep blocking new injections until WM_ENDSESSION says whether
            // the end is real; a new injection before then could be killed
            // midway by the system.
            set_exit_gate(ExitGate::SessionEnding);
            LRESULT(1)
        }
        WM_SETTINGCHANGE | WM_SYSCOLORCHANGE | WM_THEMECHANGED => {
            // Settings owns the global preview; refresh it first so the other
            // windows read the same palette.
            settings_ui::refresh_theme();
            manage::refresh_theme();
            panel::refresh_theme();
            crate::about_ui::refresh_theme();
            LRESULT(0)
        }
        WM_UIA_DEGRADED => {
            show_uia_degraded_notice(hwnd);
            LRESULT(0)
        }
        WM_ENDSESSION => {
            if wparam.0 != 0 {
                storage::flush_usage();
            } else {
                // Another app cancelled the session end; resume normal use.
                clear_exit_gate(ExitGate::SessionEnding);
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}

unsafe fn show_menu(hwnd: HWND) {
    let (paused, hotkey_ok) = STATE.with(|state| {
        let state = state.borrow();
        (state.paused, state.hotkey_ok)
    });
    crate::tray_menu::show(crate::tray_menu::MenuConfig {
        owner: hwnd,
        hotkey: current_hotkey(),
        paused,
        hotkey_ok,
        cmd_open: ID_OPEN,
        cmd_manage: ID_MANAGE,
        cmd_quick_add: ID_QUICK_ADD,
        cmd_pause: ID_PAUSE,
        cmd_settings: ID_SETTINGS,
        cmd_restore_clip: ID_RESTORE_CLIP,
        cmd_about: ID_ABOUT,
        cmd_exit: ID_EXIT,
    });
}

fn handle_command(id: u32) {
    match id {
        ID_OPEN => open_panel(),
        ID_MANAGE => {
            let hwnd = STATE.with(|state| state.borrow().hwnd);
            manage::show(hwnd);
        }
        ID_PAUSE => toggle_hotkey(),
        ID_SETTINGS => settings_ui::show(),
        ID_QUICK_ADD => {
            let hwnd = STATE.with(|state| state.borrow().hwnd);
            manage::show_new(hwnd);
        }
        ID_RESTORE_CLIP => {
            let hwnd = STATE.with(|state| state.borrow().hwnd);
            match clipboard::last_snapshot() {
                Some(snapshot) if snapshot.can_restore() => {
                    if clipboard::restore(hwnd, &snapshot) {
                        if !hwnd.0.is_null() {
                            tray::balloon(hwnd, "QuickPhrase", "已恢复上次剪贴板内容");
                        }
                    } else if !hwnd.0.is_null() {
                        tray::balloon(hwnd, "QuickPhrase", "无法恢复：剪贴板已变化或当前不可用");
                    }
                }
                _ => {
                    if !hwnd.0.is_null() {
                        tray::balloon(hwnd, "QuickPhrase", "没有可恢复的剪贴板内容");
                    }
                }
            }
        }
        ID_ABOUT => about(),
        ID_EXIT => request_exit(),
        _ => {}
    }
}

/// One exit path for the tray command and an external `WM_CLOSE`: defer while
/// an injection is in flight, ask about unsaved management changes, then
/// destroy the main window (which triggers `WM_DESTROY` and the usage flush).
fn request_exit() {
    if exit_pending() {
        logging::warn("exit ignored: exit or session end already pending");
        return;
    }
    if panel::is_injecting() || DIRECT_INJECTING.load(Ordering::Relaxed) {
        logging::warn("exit deferred: injection in flight");
        let hwnd = STATE.with(|state| state.borrow().hwnd);
        if !hwnd.0.is_null() {
            tray::balloon(hwnd, "QuickPhrase", "正在输入，请稍候再退出");
        }
        return;
    }
    set_exit_gate(ExitGate::Confirming);
    if !manage::confirm_exit() {
        clear_exit_gate(ExitGate::Confirming);
        return;
    }
    // The confirmation card pumps messages; a snippet hotkey can start an
    // injection while it is open. Re-check before destroying the window.
    if panel::is_injecting() || DIRECT_INJECTING.load(Ordering::Relaxed) {
        clear_exit_gate(ExitGate::Confirming);
        logging::warn("exit deferred: injection started during confirmation");
        let hwnd = STATE.with(|state| state.borrow().hwnd);
        if !hwnd.0.is_null() {
            tray::balloon(hwnd, "QuickPhrase", "正在输入，请稍候再退出");
        }
        return;
    }
    let hwnd = STATE.with(|state| state.borrow().hwnd);
    unsafe {
        let _ = DestroyWindow(hwnd);
    }
}

pub fn current_hotkey() -> String {
    storage::settings_clone()
        .map(|settings| settings.hotkey)
        .unwrap_or_else(|| hotkey::DEFAULT_HOTKEY.to_string())
}

/// Rebuild every per-snippet registration from storage. Safe to call after any
/// data change; a conflict is logged and only that snippet's hotkey is left
/// inactive until the user picks another combo.
pub fn refresh_snippet_hotkeys() {
    let (hwnd, paused) = STATE.with(|state| {
        let state = state.borrow();
        (state.hwnd, state.paused)
    });
    if hwnd.0.is_null() {
        return;
    }

    let previous = STATE.with(|state| std::mem::take(&mut state.borrow_mut().snippet_hotkeys));
    if paused {
        for (id, _, _) in previous {
            hotkey::unregister_id(hwnd, id);
        }
        return;
    }

    let bindings: HashMap<String, String> = storage::hotkey_bindings().into_iter().collect();
    let mut active: Vec<(i32, String, String)> = Vec::new();
    let mut keep: HashMap<String, (i32, String)> = HashMap::new();

    // Preserve ids for unchanged snippets; drop and free changed registrations.
    for (id, snippet_id, text) in previous {
        if bindings.get(&snippet_id) == Some(&text) {
            keep.insert(snippet_id.clone(), (id, text.clone()));
            active.push((id, snippet_id, text));
        } else {
            hotkey::unregister_id(hwnd, id);
        }
    }

    for (snippet_id, text) in bindings {
        if keep.contains_key(&snippet_id) {
            continue;
        }
        let id = STATE.with(|state| {
            let mut state = state.borrow_mut();
            let id = state.next_snippet_hotkey_id;
            if id <= 0xBFFF {
                state.next_snippet_hotkey_id = id + 1;
            }
            id
        });
        if id > 0xBFFF {
            logging::warn("snippet hotkey id space exhausted");
            break;
        }
        let Some(spec) = hotkey::parse(&text) else {
            logging::warn(&format!(
                "snippet hotkey skipped (invalid) snippet={snippet_id} combo={text}"
            ));
            continue;
        };
        match hotkey::register_spec_id(hwnd, id, spec) {
            Ok(()) => {
                logging::info(&format!(
                    "snippet hotkey registered id={id} snippet={snippet_id} combo={text}"
                ));
                active.push((id, snippet_id, text));
            }
            Err(code) => logging::warn(&format!(
                "snippet hotkey failed id={id} snippet={snippet_id} combo={text} code={code}"
            )),
        }
    }
    STATE.with(|state| state.borrow_mut().snippet_hotkeys = active);
}

/// Re-register the global hotkey on the main window.
pub fn apply_hotkey(text: &str) -> Result<(), String> {
    let (hwnd, paused) = STATE.with(|state| {
        let state = state.borrow();
        (state.hwnd, state.paused)
    });
    if hwnd.0.is_null() {
        return Err("no_main_window".to_string());
    }
    if let Some(spec) = hotkey::parse(text) {
        if storage::all_hotkeys()
            .iter()
            .any(|binding| hotkey::parse(binding) == Some(spec))
        {
            return Err("snippet_hotkey_conflict".to_string());
        }
    }
    hotkey::unregister(hwnd);
    if paused {
        // Stay paused: the new combo is used when the user resumes.
        STATE.with(|state| state.borrow_mut().hotkey_ok = false);
        logging::info(&format!("hotkey updated while paused: {text}"));
        return Ok(());
    }
    match hotkey::register_text(hwnd, text) {
        Ok(_) => {
            STATE.with(|state| state.borrow_mut().hotkey_ok = true);
            logging::info(&format!("hotkey changed: {text}"));
            refresh_snippet_hotkeys();
            Ok(())
        }
        Err(code) => {
            STATE.with(|state| state.borrow_mut().hotkey_ok = false);
            Err(code)
        }
    }
}

fn hotkey_panel() {
    if exit_pending() {
        logging::warn("panel activation ignored: exit or session end pending");
        return;
    }
    if DIRECT_INJECTING.load(Ordering::Relaxed) {
        logging::warn("panel activation ignored: snippet injection in flight");
        let main = STATE.with(|state| state.borrow().hwnd);
        if !main.0.is_null() {
            tray::balloon(main, "QuickPhrase", "正在输入，请稍候");
        }
        return;
    }
    if panel::is_injecting() {
        logging::warn("panel activation ignored: injection in flight");
        let main = STATE.with(|state| state.borrow().hwnd);
        if !main.0.is_null() {
            tray::balloon(main, "QuickPhrase", "正在输入，请稍候");
        }
        return;
    }

    if panel::is_visible() {
        panel::hide_and_restore();
        logging::info("panel closed");
        return;
    }

    open_panel();
}

/// Direct injection for a per-snippet hotkey. The panel is deliberately skipped,
/// so the same capture/report/guard pipeline must still run: the hotkey press is
/// the confirmation, and an uncertain target is copied instead of typed.
fn hotkey_snippet(snippet_id: &str) {
    let paused = STATE.with(|state| state.borrow().paused);
    if paused {
        logging::warn("snippet hotkey ignored: hotkeys are paused");
        return;
    }
    if exit_pending() {
        logging::warn("snippet hotkey ignored: exit or session end pending");
        return;
    }
    if DIRECT_INJECTING.load(Ordering::Relaxed) || panel::is_injecting() {
        logging::warn("snippet hotkey ignored: injection already in flight");
        let main = STATE.with(|state| state.borrow().hwnd);
        if !main.0.is_null() {
            tray::balloon(main, "QuickPhrase", "正在输入，请稍候");
        }
        return;
    }
    if panel::is_visible() {
        logging::warn("snippet hotkey ignored: panel is open");
        let main = STATE.with(|state| state.borrow().hwnd);
        if !main.0.is_null() {
            tray::balloon(main, "QuickPhrase", "面板已打开，请先关闭面板");
        }
        return;
    }

    let Some(raw) = storage::active_content(snippet_id) else {
        logging::warn(&format!(
            "snippet hotkey skipped: snippet missing or deleted id={snippet_id}"
        ));
        refresh_snippet_hotkeys();
        return;
    };
    let content = template::render(&raw);
    let main = STATE.with(|state| state.borrow().hwnd);
    if foreground_is_self() {
        logging::warn("snippet hotkey ignored: foreground is QuickPhrase itself");
        if !main.0.is_null() {
            tray::balloon(main, "QuickPhrase", "请在目标应用中使用短语快捷键");
        }
        return;
    }

    let settings = storage::settings_clone().unwrap_or_default();
    let keep_tabs = settings.tab_policy == "tab";
    let default_paste_mode = settings.insertion_mode == "paste";

    let started = Instant::now();
    let monitor = foreground::start();
    let report = match capture::once() {
        Some(report) => report,
        None => {
            if foreground_is_self() {
                logging::warn("snippet hotkey ignored: foreground is QuickPhrase itself");
                if !main.0.is_null() {
                    tray::balloon(main, "QuickPhrase", "请在目标应用中使用短语快捷键");
                }
                return;
            }
            logging::warn("snippet hotkey capture returned no usable foreground window");
            copy_only_direct(main, snippet_id, &content, "未捕获到目标窗口");
            return;
        }
    };
    logging::info(&format!("snippet hotkey capture: {}", report.summary()));
    if report.target.pid == std::process::id() {
        logging::warn("snippet hotkey ignored: target is QuickPhrase itself");
        if !main.0.is_null() {
            tray::balloon(main, "QuickPhrase", "请在目标应用中使用短语快捷键");
        }
        return;
    }
    let paste_mode = storage::action_for_exe(&report.target.exe)
        .map(|action| action == "paste")
        .unwrap_or(default_paste_mode);
    let monitor_ok = monitor.is_some();

    DIRECT_INJECTING.store(true, Ordering::Relaxed);
    let notify_raw = main.0 as isize;
    let item_id = snippet_id.to_string();
    let spawned = std::thread::Builder::new()
        .name("quickphrase-snippet-inject".into())
        .spawn(move || {
            let options = inject::Options {
                mode: if paste_mode {
                    inject::InsertionMode::Paste
                } else {
                    inject::InsertionMode::Unicode
                },
                keep_tabs,
            };
            let outcome = inject::run(
                &report,
                &content,
                monitor_ok,
                options,
                HWND(notify_raw as *mut core::ffi::c_void),
            );
            let message = outcome.describe();
            logging::info(&format!(
                "snippet hotkey inject outcome: {message} ({} ms)",
                started.elapsed().as_millis()
            ));
            storage::record_use_async(&item_id, outcome.status());
            if notify_raw != 0 {
                tray::balloon(
                    HWND(notify_raw as *mut core::ffi::c_void),
                    "QuickPhrase",
                    &message,
                );
            }
            drop(monitor);
            DIRECT_INJECTING.store(false, Ordering::Relaxed);
        });

    if spawned.is_err() {
        logging::error("failed to spawn snippet injection thread");
        DIRECT_INJECTING.store(false, Ordering::Relaxed);
    }
}

fn foreground_is_self() -> bool {
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.0.is_null() {
        return false;
    }
    let mut pid = 0u32;
    unsafe {
        let _ = GetWindowThreadProcessId(hwnd, Some(&mut pid));
    }
    pid == std::process::id()
}

fn copy_only_direct(main: HWND, snippet_id: &str, content: &str, reason: &str) {
    let (copied, snapshot) = clipboard::set_text_with_snapshot(main, content);
    storage::record_use_async(snippet_id, "rejected");
    logging::info(&format!(
        "snippet hotkey copy-only: reason={reason} copied={copied} non_text={} read_ok={}",
        snapshot.had_non_text, snapshot.read_ok
    ));
    if main.0.is_null() {
        return;
    }
    let message = if !copied {
        format!("复制失败（{reason}）")
    } else {
        format!("已复制（{reason}；{}）", snapshot.previous_text_status())
    };
    tray::balloon(main, "QuickPhrase", &message);
}

/// Menu, tray and second-instance entry: open (or focus) the panel, never close it.
fn open_panel() {
    if exit_pending() {
        logging::warn("panel activation ignored: exit or session end pending");
        return;
    }
    if panel::is_injecting() || DIRECT_INJECTING.load(Ordering::Relaxed) {
        logging::warn("panel activation ignored: injection in flight");
        let main = STATE.with(|state| state.borrow().hwnd);
        if !main.0.is_null() {
            tray::balloon(main, "QuickPhrase", "正在输入，请稍候");
        }
        return;
    }

    if panel::is_visible() {
        if panel::focus() {
            logging::info("panel focused (already open)");
        }
        return;
    }

    let started = Instant::now();
    let main = STATE.with(|state| state.borrow().hwnd);

    // Install the foreground monitor before capturing, so a switch between the
    // hotkey and the panel becoming visible is still observed by G3.
    let monitor = foreground::start();
    let default_paste_mode = paste_mode_setting();

    match capture::once() {
        Some(report) => {
            logging::info(&format!("capture: {}", report.summary()));
            let capture_total = report.timings_ms.total;
            let paste_mode = storage::action_for_exe(&report.target.exe)
                .map(|action| action == "paste")
                .unwrap_or(default_paste_mode);
            let copy_only = copy_only_reason(&report, paste_mode);
            if let Some(reason) = &copy_only {
                logging::info(&format!("panel opens copy-only: {reason}"));
            }
            let data = panel_data(copy_only, paste_mode);
            let micros = panel::show(report, monitor, main, data);

            if micros == 0 {
                logging::warn("panel show failed (creation or foreground)");
                if !main.0.is_null() {
                    tray::balloon(main, "QuickPhrase", "无法显示面板（创建或焦点失败）");
                }
            } else {
                logging::info(&format!(
                    "panel shown in {micros} us; capture total {capture_total} ms"
                ));
            }
        }
        None => {
            logging::warn("capture returned no usable foreground window");
            // Per PRD 14.3 the panel still opens; Enter copies instead of typing.
            let data = panel_data(Some("未捕获到目标窗口".to_string()), default_paste_mode);
            let micros = panel::show_copy_only(main, data);
            if micros == 0 {
                logging::warn("copy-only panel show failed");
                if !main.0.is_null() {
                    tray::balloon(main, "QuickPhrase", "无法显示面板（创建或焦点失败）");
                }
            }
        }
    }

    logging::info(&format!(
        "capture pipeline took {} ms",
        started.elapsed().as_millis()
    ));
}

fn copy_only_reason(report: &capture::Report, paste_mode: bool) -> Option<String> {
    if report.identity != "verified" {
        return Some("无法验证目标身份".to_string());
    }
    if report.capability != "text" {
        return Some(format!("目标不可输入（{}）", report.capability));
    }
    if report.target.target_higher {
        return Some("目标权限更高".to_string());
    }
    if report.target.integrity == "unknown" || report.target.own_integrity == "unknown" {
        return Some("无法确认完整性".to_string());
    }
    // Paste mode is forbidden for terminals and unverified regions; say so up
    // front so the footer never promises a paste that cannot happen.
    if paste_mode && report.region != "text_editor" {
        return Some("粘贴模式不支持该区域".to_string());
    }
    None
}

fn paste_mode_setting() -> bool {
    storage::settings_clone()
        .map(|settings| settings.insertion_mode == "paste")
        .unwrap_or(false)
}

fn panel_data(copy_only: Option<String>, paste_mode: bool) -> panel::PanelData {
    let settings = storage::settings_clone().unwrap_or_default();
    let rows = storage::panel_items();
    let items: Vec<panel::Item> = rows
        .into_iter()
        .map(|row| panel::Item {
            id: row.id,
            title: row.title,
            content: row.content,
            category: row.category,
            category_id: row.category_id,
            tags: row.tags,
            blob: row.blob,
            pinned: row.pinned,
            last_used_at: row.last_used_at,
            updated_at: row.updated_at,
            sort_order: row.sort_order,
            use_count: row.use_count,
        })
        .collect();
    let tag_counts = panel::count_tags(&items);
    panel::PanelData {
        items,
        categories: storage::categories(),
        tag_counts,
        notice: storage::read_only_notice(),
        copy_only,
        paste_mode,
        keep_tabs: settings.tab_policy == "tab",
        hotkey_hint: current_hotkey(),
        sort_mode: search::SortMode::from_setting(&settings.panel_sort),
    }
}

fn persist_paused(paused: bool) {
    match storage::settings_clone() {
        Some(mut settings) => {
            settings.paused = paused;
            if let Err(err) = storage::save_settings(settings) {
                logging::warn(&format!("persist paused state failed: {err}"));
            }
        }
        None => logging::warn("persist paused state skipped: storage unavailable"),
    }
}

fn toggle_hotkey() {
    let (hwnd, paused, hotkey_ok) = STATE.with(|state| {
        let state = state.borrow();
        (state.hwnd, state.paused, state.hotkey_ok)
    });

    if paused || !hotkey_ok {
        // Either the user is resuming, or startup registration failed and this
        // entry retries it instead of pretending to pause an inactive hotkey.
        let text = current_hotkey();
        if let Some(spec) = hotkey::parse(&text) {
            if storage::all_hotkeys()
                .iter()
                .any(|binding| hotkey::parse(binding) == Some(spec))
            {
                logging::error("resume blocked: global hotkey is used by a snippet");
                platform::error_box(
                    "快捷键冲突",
                    &format!(
                        "无法恢复 {text}：该组合已被某个短语的专属快捷键占用。\n\
                         请先在管理窗口修改该短语的快捷键，再恢复。"
                    ),
                );
                return;
            }
        }
        match hotkey::register_text(hwnd, &text) {
            Ok(_) => {
                STATE.with(|state| {
                    let mut state = state.borrow_mut();
                    state.paused = false;
                    state.hotkey_ok = true;
                });
                persist_paused(false);
                tray::set_tip(hwnd, TIP_NORMAL);
                refresh_snippet_hotkeys();
                logging::info(if paused {
                    "hotkey resumed"
                } else {
                    "hotkey retry succeeded"
                });
            }
            Err(code) => {
                logging::error(&format!("register hotkey failed: {code}"));
                platform::error_box(
                    "快捷键冲突",
                    &format!("无法注册 {}（错误码 {code}）。", current_hotkey()),
                );
            }
        }
    } else {
        hotkey::unregister(hwnd);
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.paused = true;
            state.hotkey_ok = false;
        });
        persist_paused(true);
        tray::set_tip(hwnd, TIP_PAUSED);
        refresh_snippet_hotkeys();
        tray::balloon(hwnd, "QuickPhrase", "快捷键已暂停，可通过托盘菜单恢复。");
        logging::info("hotkey paused");
    }
}

fn about() {
    crate::about_ui::show();
}

#[cfg(test)]
mod tests {
    use super::{exit_gate_blocks, ExitGate};

    #[test]
    fn exit_gate_blocks_until_the_flow_is_resolved() {
        assert!(!exit_gate_blocks(ExitGate::None));
        assert!(exit_gate_blocks(ExitGate::Confirming));
        assert!(exit_gate_blocks(ExitGate::SessionEnding));
    }
}
