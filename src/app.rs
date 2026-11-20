use std::cell::RefCell;
use std::time::{Duration, Instant};

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::HBRUSH;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::{
    capture, clipboard, foreground, hotkey, logging, manage, panel, platform, search, settings_ui,
    single_instance, storage, tray, uia,
};

const ID_OPEN: u32 = 1001;
const ID_MANAGE: u32 = 1002;
const ID_PAUSE: u32 = 1003;
const ID_SETTINGS: u32 = 1004;
const ID_ABOUT: u32 = 1005;
const ID_EXIT: u32 = 1006;
const ID_RESTORE_CLIP: u32 = 1007;

const SELFTEST_TIMER: usize = 1;

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
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

pub fn run(selftest: bool, bench: bool, started: Instant) -> i32 {
    let _mutex = match single_instance::acquire() {
        single_instance::Acquire::First(handle) => handle,
        single_instance::Acquire::AlreadyRunning => {
            if single_instance::signal_existing() {
                logging::info("signalled the running instance");
            } else {
                logging::warn("another instance is running but did not respond to activation");
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
            ..State::default()
        };
    });

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
            if wparam.0 as i32 == hotkey::HOTKEY_ID {
                logging::info("hotkey fired");
                hotkey_panel();
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
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
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
            STATE.with(|state| {
                let mut state = state.borrow_mut();
                state.tray_ok = false;
                state.hotkey_ok = false;
            });
            PostQuitMessage(0);
            LRESULT(0)
        }
        WM_QUERYENDSESSION => LRESULT(1),
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
        ID_RESTORE_CLIP => {
            let hwnd = STATE.with(|state| state.borrow().hwnd);
            match clipboard::last_snapshot() {
                Some(snapshot) if snapshot.text.is_some() => {
                    if clipboard::restore(&snapshot) {
                        if !hwnd.0.is_null() {
                            tray::balloon(hwnd, "QuickPhrase", "已恢复上次剪贴板内容");
                        }
                    } else if !hwnd.0.is_null() {
                        tray::balloon(
                            hwnd,
                            "QuickPhrase",
                            "无法恢复：剪贴板内容已变化（可能已恢复过，或被其他程序修改）",
                        );
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
        ID_EXIT => {
            if panel::is_injecting() {
                logging::warn("exit deferred: injection in flight");
                let hwnd = STATE.with(|state| state.borrow().hwnd);
                if !hwnd.0.is_null() {
                    tray::balloon(hwnd, "QuickPhrase", "正在输入，请稍候再退出");
                }
                return;
            }
            if !manage::confirm_exit() {
                return;
            }
            let hwnd = STATE.with(|state| state.borrow().hwnd);
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
        }
        _ => {}
    }
}

pub fn current_hotkey() -> String {
    storage::settings_clone()
        .map(|settings| settings.hotkey)
        .unwrap_or_else(|| hotkey::DEFAULT_HOTKEY.to_string())
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
            Ok(())
        }
        Err(code) => {
            STATE.with(|state| state.borrow_mut().hotkey_ok = false);
            Err(code)
        }
    }
}

fn hotkey_panel() {
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

/// Menu, tray and second-instance entry: open (or focus) the panel, never close it.
fn open_panel() {
    if panel::is_injecting() {
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
    let paste_mode = paste_mode_setting();

    match capture::once() {
        Some(report) => {
            logging::info(&format!("capture: {}", report.summary()));
            let capture_total = report.timings_ms.total;
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
            let data = panel_data(Some("未捕获到目标窗口".to_string()), paste_mode);
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
    let items = storage::panel_items()
        .into_iter()
        .map(|row| panel::Item {
            id: row.id,
            title: row.title,
            content: row.content,
            category: row.category,
            category_id: row.category_id,
            blob: row.blob,
            pinned: row.pinned,
            last_used_at: row.last_used_at,
            updated_at: row.updated_at,
            sort_order: row.sort_order,
            use_count: row.use_count,
        })
        .collect();
    panel::PanelData {
        items,
        categories: storage::categories(),
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
        match hotkey::register_text(hwnd, &current_hotkey()) {
            Ok(_) => {
                STATE.with(|state| {
                    let mut state = state.borrow_mut();
                    state.paused = false;
                    state.hotkey_ok = true;
                });
                persist_paused(false);
                tray::set_tip(hwnd, TIP_NORMAL);
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
        tray::balloon(hwnd, "QuickPhrase", "快捷键已暂停，可通过托盘菜单恢复。");
        logging::info("hotkey paused");
    }
}

fn about() {
    crate::about_ui::show();
}
