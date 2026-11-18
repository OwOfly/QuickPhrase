#![windows_subsystem = "windows"]

mod app;
mod capture;
mod context;
mod foreground;
mod hotkey;
mod logging;
mod platform;
mod single_instance;
mod tray;
mod uia;

use std::time::{Duration, Instant};

fn main() {
    let started = Instant::now();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let selftest = has(&args, "--selftest");
    let bench = has(&args, "--bench");
    let capture_test = has(&args, "--capture-test");
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

    if capture_test {
        run_capture_test(delay);
        logging::info("capture-test done");
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
