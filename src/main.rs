#![windows_subsystem = "windows"]

mod app;
mod capture;
mod clipboard;
mod context;
mod foreground;
mod hotkey;
mod inject;
mod logging;
mod manage;
mod panel;
mod platform;
mod search;
mod settings_ui;
mod single_instance;
mod storage;
mod tray;
mod uia;

use std::time::{Duration, Instant};

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, TranslateMessage, MSG,
};

fn main() {
    let started = Instant::now();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let selftest = has(&args, "--selftest");
    let bench = has(&args, "--bench");
    let capture_test = has(&args, "--capture-test");
    let panel_bench = has(&args, "--panel-bench");
    let panel_test = has(&args, "--panel-test");
    let inject_test = has(&args, "--inject-test");
    let search_bench = has(&args, "--search-bench");
    let manage_test = has(&args, "--manage-test");
    let settings_test = has(&args, "--settings-test");
    let delay = value(&args, "--delay")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(1500);

    if let Err(err) = logging::init() {
        platform::error_box("QuickPhrase 启动失败", &format!("无法初始化日志：{err}"));
        std::process::exit(2);
    }
    logging::install_panic_hook();

    // Must happen before the first window is created.
    match platform::set_dpi_awareness() {
        Ok(()) => logging::info("dpi awareness set at runtime"),
        Err(code) => logging::info(&format!(
            "runtime dpi call returned {code}; manifest PerMonitorV2 is expected to apply"
        )),
    }

    logging::info(&format!(
        "QuickPhrase {} starting (selftest={selftest}, bench={bench}, capture_test={capture_test})",
        env!("CARGO_PKG_VERSION")
    ));

    if search_bench {
        let (p50, p95, max) = search::bench(1_000, 100);
        logging::info(&format!(
            "search bench: n=1000 queries=100 p50={p50}us p95={p95}us max={max}us"
        ));
        return;
    }

    if settings_test {
        let _guard = match single_instance::acquire() {
            single_instance::Acquire::First(handle) => handle,
            single_instance::Acquire::AlreadyRunning => {
                logging::error("settings-test: another QuickPhrase instance is running");
                std::process::exit(4);
            }
            single_instance::Acquire::Failed(code) => {
                logging::error(&format!("settings-test: mutex failed ({code})"));
                std::process::exit(4);
            }
        };
        storage::init();
        if !settings_ui::show_test() {
            logging::error("settings-test: window creation failed");
            std::process::exit(4);
        }
        let mut message = MSG::default();
        while unsafe { GetMessageW(&mut message, None, 0, 0) }.0 > 0 {
            unsafe {
                if settings_ui::is_dialog_message(&message) {
                    continue;
                }
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        settings_ui::shutdown();
        logging::info("settings-test done");
        return;
    }

    if manage_test {
        let _guard = match single_instance::acquire() {
            single_instance::Acquire::First(handle) => handle,
            single_instance::Acquire::AlreadyRunning => {
                logging::error("manage-test: another QuickPhrase instance is running");
                std::process::exit(4);
            }
            single_instance::Acquire::Failed(code) => {
                logging::error(&format!("manage-test: mutex failed ({code})"));
                std::process::exit(4);
            }
        };
        storage::init();
        if !manage::show_test(HWND::default()) {
            logging::error("manage-test: window creation failed");
            std::process::exit(4);
        }
        let mut message = MSG::default();
        while unsafe { GetMessageW(&mut message, None, 0, 0) }.0 > 0 {
            unsafe {
                if manage::is_dialog_message(&message) {
                    continue;
                }
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        manage::shutdown();
        logging::info("manage-test done");
        return;
    }

    if capture_test {
        run_capture_test(delay);
        logging::info("capture-test done");
        return;
    }

    if inject_test {
        let ok = inject::selftest();
        logging::info(&format!(
            "inject-test: {}",
            if ok { "PASS" } else { "FAIL" }
        ));
        std::process::exit(if ok { 0 } else { 3 });
    }

    if panel_bench {
        let (p50, p95, max) = panel::bench(100);
        logging::info(&format!(
            "panel bench: n=100 p50={p50}us p95={p95}us max={max}us"
        ));
        panel::shutdown();
        return;
    }

    if panel_test {
        if panel::show_test() == 0 {
            logging::error("panel-test failed to create or show the panel");
            panel::shutdown();
            std::process::exit(3);
        }
        let mut message = MSG::default();
        while unsafe { GetMessageW(&mut message, None, 0, 0) }.0 > 0 {
            unsafe {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        panel::shutdown();
        logging::info("panel-test done");
        return;
    }

    let code = app::run(selftest, bench, started);
    logging::info(&format!("exit with code {code}"));
    std::process::exit(code);
}

fn run_capture_test(delay_ms: u64) {
    let warmed = uia::warmup(Duration::from_millis(800));
    logging::info(&format!("uia warmup: {warmed}"));

    let monitor = foreground::start();
    if monitor.is_none() {
        logging::warn("foreground monitor unavailable");
    }

    logging::info(&format!(
        "capture-test: waiting {delay_ms} ms; focus the target window now"
    ));
    std::thread::sleep(Duration::from_millis(delay_ms));

    match capture::once() {
        Some(report) => {
            logging::info(&format!("capture-test: {}", report.summary()));
            let target =
                windows::Win32::Foundation::HWND(report.target.hwnd as *mut core::ffi::c_void);
            let same = uia::compare_last(target, Duration::from_millis(100));
            logging::info(&format!("capture-test: compare_last={same:?}"));
            match capture::write(&report) {
                Ok(path) => logging::info(&format!("capture written: {}", path.display())),
                Err(err) => logging::error(&format!("capture write failed: {err}")),
            }
        }
        None => logging::warn("capture-test: no foreground window"),
    }

    drop(monitor);
}

fn has(args: &[String], name: &str) -> bool {
    args.iter().any(|argument| argument == name)
}

fn value(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|argument| argument == name)
        .and_then(|index| args.get(index + 1))
        .cloned()
}
