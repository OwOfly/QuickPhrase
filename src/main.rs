#![windows_subsystem = "windows"]

mod app;
mod hotkey;
mod logging;
mod platform;
mod single_instance;
mod tray;

use std::time::Instant;

fn main() {
    let started = Instant::now();
    let selftest = std::env::args().any(|a| a == "--selftest");
    let bench = std::env::args().any(|a| a == "--bench");

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
        "QuickPhrase {} starting (selftest={selftest}, bench={bench})",
        env!("CARGO_PKG_VERSION")
    ));

    let code = app::run(selftest, bench, started);
    logging::info(&format!("exit with code {code}"));
    std::process::exit(code);
}
