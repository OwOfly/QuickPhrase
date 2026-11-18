use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use windows::Win32::Foundation::SYSTEMTIME;
use windows::Win32::System::SystemInformation::GetLocalTime;

const MAX_BYTES: u64 = 500 * 1024;
const KEEP: usize = 3;

struct Inner {
    file: File,
    size: u64,
    date: String,
}

struct Logger {
    dir: PathBuf,
    inner: Mutex<Inner>,
}

static LOGGER: OnceLock<Logger> = OnceLock::new();

pub fn init() -> std::io::Result<()> {
    let dir = crate::platform::data_dir().join("logs");
    fs::create_dir_all(&dir)?;

    let date = date_stamp();
    let path = path_for(&dir, &date);
    let file = OpenOptions::new().create(true).append(true).open(&path)?;
    let size = file.metadata()?.len();

    let _ = LOGGER.set(Logger {
        dir,
        inner: Mutex::new(Inner { file, size, date }),
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

    let Ok(mut inner) = logger.inner.lock() else {
        return;
    };

    let today = date_stamp();
    if inner.date != today {
        let path = path_for(&logger.dir, &today);
        if let Ok(file) = OpenOptions::new().create(true).append(true).open(&path) {
            inner.size = file.metadata().map(|meta| meta.len()).unwrap_or(0);
            inner.file = file;
            inner.date = today;
        }
        // If the new file cannot be opened, keep writing to the current one.
    }

    if inner.size + line.len() as u64 > MAX_BYTES {
        let date = inner.date.clone();
        rotate(&logger.dir, &date, &mut inner);
    }

    if inner.file.write_all(line.as_bytes()).is_ok() {
        inner.size += line.len() as u64;
    }
}

/// Rotate only when every step succeeds. On any failure the current file is
/// kept and appending continues, so a transient lock cannot destroy the log.
fn rotate(dir: &Path, date: &str, inner: &mut Inner) {
    let name = format!("qp-{date}.log");
    let path = dir.join(&name);

    for index in (1..=KEEP).rev() {
        let source = if index == 1 {
            path.clone()
        } else {
            dir.join(format!("{name}.{}", index - 1))
        };
        let target = dir.join(format!("{name}.{index}"));
        if source.exists() && fs::rename(&source, &target).is_err() {
            return;
        }
    }

    if let Ok(file) = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&path)
    {
        inner.file = file;
        inner.size = 0;
    }
}

fn path_for(dir: &Path, date: &str) -> PathBuf {
    dir.join(format!("qp-{date}.log"))
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
