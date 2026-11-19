use std::sync::Mutex;

use windows::Win32::Foundation::{GlobalFree, HANDLE};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, EnumClipboardFormats, GetClipboardData,
    GetClipboardSequenceNumber, IsClipboardFormatAvailable, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::System::Ole::CF_UNICODETEXT;

const MAX_SNAPSHOT_UNITS: usize = 512 * 1024;
const CF_TEXT: u32 = 1;
const CF_OEMTEXT: u32 = 7;
const CF_UNICODETEXT_FORMAT: u32 = 13;
const CF_LOCALE: u32 = 16;

/// Windows synthesizes CF_TEXT / CF_OEMTEXT / CF_LOCALE alongside
/// CF_UNICODETEXT; they are the same text, not extra non-text content.
fn is_text_format(format: u32) -> bool {
    matches!(
        format,
        CF_TEXT | CF_OEMTEXT | CF_UNICODETEXT_FORMAT | CF_LOCALE
    )
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub text: Option<String>,
    pub had_non_text: bool,
}

static LAST_SNAPSHOT: Mutex<Option<Snapshot>> = Mutex::new(None);

pub fn set_text(text: &str) -> bool {
    write_text(text)
}

/// COPY_ONLY write: keep the previous text for a later restore (§6.5 / D26)
/// and report whether the old clipboard also held non-text formats.
pub fn set_text_with_snapshot(text: &str) -> (bool, Snapshot) {
    let snapshot = read_snapshot();
    let stored = write_text(text);
    if stored {
        if let Ok(mut slot) = LAST_SNAPSHOT.lock() {
            *slot = Some(snapshot.clone());
        }
    }
    (stored, snapshot)
}

#[allow(dead_code)] // the restore entry point lands in M1-5
pub fn last_snapshot() -> Option<Snapshot> {
    LAST_SNAPSHOT.lock().ok().and_then(|slot| slot.clone())
}

#[allow(dead_code)] // the restore entry point lands in M1-5
pub fn restore(snapshot: &Snapshot) -> bool {
    match &snapshot.text {
        Some(text) => write_text(text),
        None => false,
    }
}

fn read_snapshot() -> Snapshot {
    unsafe {
        if OpenClipboard(None).is_err() {
            return Snapshot::default();
        }

        let mut had_non_text = false;
        let mut format = EnumClipboardFormats(0);
        while format != 0 {
            if !is_text_format(format) {
                had_non_text = true;
            }
            format = EnumClipboardFormats(format);
        }

        let text = if IsClipboardFormatAvailable(CF_UNICODETEXT.0 as u32).is_ok() {
            read_unicode_text()
        } else {
            None
        };

        let _ = CloseClipboard();
        Snapshot { text, had_non_text }
    }
}

unsafe fn read_unicode_text() -> Option<String> {
    let handle = GetClipboardData(CF_UNICODETEXT.0 as u32).ok()?;
    let memory = windows::Win32::Foundation::HGLOBAL(handle.0);
    let pointer = GlobalLock(memory) as *const u16;
    if pointer.is_null() {
        return None;
    }
    let mut length = 0usize;
    while length < MAX_SNAPSHOT_UNITS {
        if *pointer.add(length) == 0 {
            break;
        }
        length += 1;
    }
    let units = std::slice::from_raw_parts(pointer, length);
    let text = String::from_utf16_lossy(units);
    let _ = GlobalUnlock(memory);
    Some(text)
}

fn write_text(text: &str) -> bool {
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes = wide.len() * 2;

    unsafe {
        if OpenClipboard(None).is_err() {
            return false;
        }
        if EmptyClipboard().is_err() {
            let _ = CloseClipboard();
            return false;
        }

        let memory = match GlobalAlloc(GMEM_MOVEABLE, bytes) {
            Ok(memory) => memory,
            Err(_) => {
                let _ = CloseClipboard();
                return false;
            }
        };

        let pointer = GlobalLock(memory);
        if pointer.is_null() {
            let _ = GlobalFree(Some(memory));
            let _ = CloseClipboard();
            return false;
        }

        std::ptr::copy_nonoverlapping(wide.as_ptr() as *const u8, pointer as *mut u8, bytes);
        let _ = GlobalUnlock(memory);

        let stored = SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(memory.0))).is_ok();
        if !stored {
            let _ = GlobalFree(Some(memory));
        }
        let _ = CloseClipboard();
        stored
    }
}

#[allow(dead_code)] // used by the M1 clipboard compatibility mode
pub fn sequence() -> u32 {
    unsafe { GetClipboardSequenceNumber() }
}

#[cfg(test)]
mod tests {
    use super::is_text_format;

    #[test]
    fn synthesized_text_formats_are_not_non_text() {
        assert!(is_text_format(1)); // CF_TEXT
        assert!(is_text_format(7)); // CF_OEMTEXT
        assert!(is_text_format(13)); // CF_UNICODETEXT
        assert!(is_text_format(16)); // CF_LOCALE
        assert!(!is_text_format(2)); // CF_BITMAP
        assert!(!is_text_format(8)); // CF_DIB
        assert!(!is_text_format(15)); // CF_HDROP
    }
}
