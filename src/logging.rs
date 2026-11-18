use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use windows::Win32::Foundation::SYSTEMTIME;
use windows::Win32::System::SystemInformation::GetLocalTime;

const MAX_BYTES: u64 = 500 * 1024;
const KEEP: usize = 3;

struct Inner {
    file: File,
    size: u64,
}

struct Logger {
    path: PathBuf,
    inner: Mutex<Inner>,
}

static LOGGER: OnceLock<Logger> = OnceLock::new();

pub fn init() -> std::io::Result<()> {
    let dir = crate::platform::data_dir().join("logs");
    fs::create_dir_all(&dir)?;

    let path = dir.join(format!("qp-{}.log", date_stamp()));
    let file = OpenOptions::new().create(true).append(true).open(&path)?;
    let size = file.metadata()?.len();

    let _ = LOGGER.set(Logger {
        path,
        inner: Mutex::new(Inner { file, size }),
    });
    Ok(())
}

pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        error(&format!("panic: {info}"));
    }));
}

pub fn info(message: &str) {
    write("INFO", message);
}

pub fn warn(message: &str) {
    write("WARN", message);
}

pub fn error(message: &str) {
    write("ERROR", message);
}

fn write(level: &str, message: &str) {
    let Some(logger) = LOGGER.get() else {
        return;
    };

    let line = format!(
        "{} {:<5} {}\n",
        timestamp(),
        level,
        message.replace('\n', "\\n")
    );

    if let Ok(mut inner) = logger.inner.lock() {
        if inner.size + line.len() as u64 > MAX_BYTES {
            rotate(logger, &mut inner);
        }
        if inner.file.write_all(line.as_bytes()).is_ok() {
            inner.size += line.len() as u64;
        }
    }
}

fn rotate(logger: &Logger, inner: &mut Inner) {
    let Some(name) = logger
        .path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
    else {
        return;
    };
    let Some(dir) = logger.path.parent() else {
        return;
    };

    for index in (1..=KEEP).rev() {
        let source = if index == 1 {
            logger.path.clone()
        } else {
            dir.join(format!("{name}.{}", index - 1))
        };
        let target = dir.join(format!("{name}.{index}"));
        if source.exists() {
            let _ = fs::rename(&source, &target);
        }
    }

    if let Ok(file) = OpenOptions::new()
        .create(true)
        .truncate(true)
        .append(true)
        .open(&logger.path)
    {
        inner.file = file;
        inner.size = 0;
    }
}

fn system_time() -> SYSTEMTIME {
    unsafe { GetLocalTime() }
}

fn date_stamp() -> String {
    let t = system_time();
    format!("{:04}-{:02}-{:02}", t.wYear, t.wMonth, t.wDay)
}

fn timestamp() -> String {
    let t = system_time();
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}",
        t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond, t.wMilliseconds
    )
}
