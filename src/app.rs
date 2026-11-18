use std::cell::RefCell;
use std::time::{Duration, Instant};

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::HBRUSH;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::{capture, foreground, hotkey, logging, panel, platform, single_instance, tray, uia};

const ID_OPEN: u32 = 1001;
const ID_MANAGE: u32 = 1002;
const ID_PAUSE: u32 = 1003;
const ID_SETTINGS: u32 = 1004;
const ID_ABOUT: u32 = 1005;
const ID_EXIT: u32 = 1006;

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
            logging::warn("another instance is running; signalling it");
            single_instance::signal_existing();
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

    let icon = unsafe { LoadIconW(None, IDI_APPLICATION) }.unwrap_or_default();
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

    let activate_message = unsafe { RegisterWindowMessageW(w!("QuickPhrase.Activate")) };
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

    let tray_ok = tray::add(hwnd, icon, TIP_NORMAL);
    STATE.with(|state| state.borrow_mut().tray_ok = tray_ok);
    if tray_ok {
        logging::info("tray icon added");
    } else {
        logging::error("Shell_NotifyIconW(NIM_ADD) failed");
        platform::error_box("QuickPhrase", "无法添加托盘图标。");
    }

    let mut hotkey_registered = false;
    match hotkey::register(hwnd) {
        Ok(()) => {
            hotkey_registered = true;
            STATE.with(|state| {
                let mut state = state.borrow_mut();
                state.hotkey_ok = true;
                state.paused = false;
            });
            logging::info("hotkey registered: Ctrl+Alt+Q");
        }
        Err(code) => {
            logging::error(&format!("RegisterHotKey failed: {code}"));
            if tray_ok {
                tray::balloon(hwnd, "快捷键注册失败", "Ctrl+Alt+Q 已被其他程序占用。");
            }
            platform::error_box(
                "快捷键冲突",
                &format!(
                    "无法注册 Ctrl+Alt+Q（错误码 {code}）。\n\
                     请关闭占用该快捷键的程序，或在后续版本的设置中修改。"
                ),
            );
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
        if STATE.with(|state| state.borrow().tray_ok) {
            tray::balloon(hwnd, "QuickPhrase", "QuickPhrase 已在运行。");
        }
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
        STATE.with(|state| state.borrow_mut().tray_ok = ok);
        return LRESULT(0);
    }

    match message {
        panel::WM_INJECT_DONE => {
            panel::finish_insert();
            if let Some(text) = panel::take_inject_result() {
                tray::balloon(hwnd, "QuickPhrase", &text);
            }
            LRESULT(0)
        }
        WM_HOTKEY => {
            if wparam.0 as i32 == hotkey::HOTKEY_ID {
                logging::info("hotkey fired");
                activate_panel();
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
                WM_LBUTTONDBLCLK => activate_panel(),
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
    let paused = STATE.with(|state| state.borrow().paused);
    let Ok(menu) = CreatePopupMenu() else {
        return;
    };

    let _ = AppendMenuW(menu, MF_STRING, ID_OPEN as usize, w!("打开快捷面板"));
    let _ = AppendMenuW(menu, MF_STRING, ID_MANAGE as usize, w!("管理短语"));
    let _ = AppendMenuW(
        menu,
        MF_STRING,
        ID_PAUSE as usize,
        if paused {
            w!("恢复快捷键")
        } else {
            w!("暂停快捷键")
        },
    );
    let _ = AppendMenuW(menu, MF_STRING, ID_SETTINGS as usize, w!("设置"));
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
    let _ = AppendMenuW(menu, MF_STRING, ID_ABOUT as usize, w!("关于 QuickPhrase"));
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
    let _ = AppendMenuW(menu, MF_STRING, ID_EXIT as usize, w!("退出"));

    let mut point = POINT { x: 0, y: 0 };
    let _ = GetCursorPos(&mut point);
    let _ = SetForegroundWindow(hwnd);
    let _ = TrackPopupMenu(
        menu,
        TPM_RIGHTBUTTON | TPM_BOTTOMALIGN,
        point.x,
        point.y,
        Some(0),
        hwnd,
        None,
    );
    let _ = DestroyMenu(menu);
    let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
}

fn handle_command(id: u32) {
    match id {
        ID_OPEN => activate_panel(),
        ID_MANAGE => platform::info_box("QuickPhrase", "短语管理将在 M1 实现。"),
        ID_PAUSE => toggle_hotkey(),
        ID_SETTINGS => platform::info_box("QuickPhrase", "设置将在 M1 实现。"),
        ID_ABOUT => about(),
        ID_EXIT => {
            let hwnd = STATE.with(|state| state.borrow().hwnd);
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
        }
        _ => {}
    }
}

fn activate_panel() {
    if panel::is_visible() {
        panel::hide_and_restore();
        logging::info("panel closed");
        return;
    }

    let started = Instant::now();
    let main = STATE.with(|state| state.borrow().hwnd);

    // Install the foreground monitor before capturing, so a switch between the
    // hotkey and the panel becoming visible is still observed by G3.
    let monitor = foreground::start();

    match capture::once() {
        Some(report) => {
            logging::info(&format!("capture: {}", report.summary()));
            let capture_total = report.timings_ms.total;
            let micros = panel::show(report, monitor, main);

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
            if !main.0.is_null() {
                tray::balloon(main, "QuickPhrase", "未捕获到前台窗口");
            }
        }
    }

    logging::info(&format!(
        "capture pipeline took {} ms",
        started.elapsed().as_millis()
    ));
}

fn toggle_hotkey() {
    let (hwnd, paused) = STATE.with(|state| {
        let state = state.borrow();
        (state.hwnd, state.paused)
    });

    if paused {
        match hotkey::register(hwnd) {
            Ok(()) => {
                STATE.with(|state| {
                    let mut state = state.borrow_mut();
                    state.paused = false;
                    state.hotkey_ok = true;
                });
                tray::set_tip(hwnd, TIP_NORMAL);
                logging::info("hotkey resumed");
            }
            Err(code) => {
                logging::error(&format!("resume hotkey failed: {code}"));
                platform::error_box(
                    "快捷键冲突",
                    &format!("无法恢复 Ctrl+Alt+Q（错误码 {code}）。"),
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
        tray::set_tip(hwnd, TIP_PAUSED);
        tray::balloon(hwnd, "QuickPhrase", "快捷键已暂停，可通过托盘菜单恢复。");
        logging::info("hotkey paused");
    }
}

fn about() {
    platform::info_box(
        "关于 QuickPhrase",
        &format!(
            "QuickPhrase 快语 {}\n\n\
             全局快捷短语输入器（M0-4 文本注入）\n\
             数据目录：{}",
            env!("CARGO_PKG_VERSION"),
            platform::data_dir().display()
        ),
    );
}
