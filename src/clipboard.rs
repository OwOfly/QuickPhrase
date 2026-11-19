use std::sync::Mutex;

use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL};
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
    pub sequence: u32,
    pub read_ok: bool,
}

static LAST_SNAPSHOT: Mutex<Option<Snapshot>> = Mutex::new(None);

/// COPY_ONLY write: keep the previous text for a later restore (§6.5 / D26)
/// and report whether the old clipboard also held non-text formats.
pub fn set_text_with_snapshot(text: &str) -> (bool, Snapshot) {
    let previous = read_snapshot();
    match write_text(text) {
        Some(sequence) => {
            let stored = Snapshot {
                text: previous.text,
                had_non_text: previous.had_non_text,
                sequence,
                read_ok: previous.read_ok,
            };
            if let Ok(mut slot) = LAST_SNAPSHOT.lock() {
                *slot = Some(stored.clone());
            }
            (true, stored)
        }
        None => {
            // The write can fail after EmptyClipboard; put the previous plain
            // text back so a failed paste never silently eats the clipboard.
            if let Some(previous_text) = previous.text.as_deref() {
                let _ = write_text(previous_text);
            }
            (false, previous)
        }
    }
}

pub fn last_snapshot() -> Option<Snapshot> {
    LAST_SNAPSHOT.lock().ok().and_then(|slot| slot.clone())
}

pub fn restore(snapshot: &Snapshot) -> bool {
    if snapshot.sequence == 0 {
        return false;
    }
    let Some(text) = snapshot.text.as_deref() else {
        return false;
    };
    let Some(memory) = (unsafe { prepare_text(text) }) else {
        return false;
    };
    unsafe {
        if OpenClipboard(None).is_err() {
            let _ = GlobalFree(Some(memory));
            return false;
        }
        // Check and replace under the same clipboard lock: another program
        // cannot slip a new value between the check and the write.
        if GetClipboardSequenceNumber() != snapshot.sequence {
            let _ = GlobalFree(Some(memory));
            let _ = CloseClipboard();
            return false;
        }
        if EmptyClipboard().is_err() {
            let _ = GlobalFree(Some(memory));
            let _ = CloseClipboard();
            return false;
        }
        let stored = SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(memory.0))).is_ok();
        if !stored {
            let _ = GlobalFree(Some(memory));
        }
        let _ = CloseClipboard();
        stored
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

        let mut read_ok = true;
        let text = if IsClipboardFormatAvailable(CF_UNICODETEXT.0 as u32).is_ok() {
            match read_unicode_text() {
                Some(text) => Some(text),
                None => {
                    // The format was advertised but the handle could not be
                    // read; distinguish that from "no text at all".
                    read_ok = false;
                    None
                }
            }
        } else {
            None
        };

        let _ = CloseClipboard();
        Snapshot {
            text,
            had_non_text,
            // Filled only after *our* write; 0 means "no restore target".
            sequence: 0,
            read_ok,
        }
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

/// Allocate and fill the CF_UNICODETEXT block before touching the clipboard.
/// A failed allocation must never destroy the current clipboard contents.
unsafe fn prepare_text(text: &str) -> Option<HGLOBAL> {
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes = wide.len() * 2;

    let memory = GlobalAlloc(GMEM_MOVEABLE, bytes).ok()?;
    let pointer = GlobalLock(memory);
    if pointer.is_null() {
        let _ = GlobalFree(Some(memory));
        return None;
    }
    std::ptr::copy_nonoverlapping(wide.as_ptr() as *const u8, pointer as *mut u8, bytes);
    let _ = GlobalUnlock(memory);
    Some(memory)
}

fn write_text(text: &str) -> Option<u32> {
    let memory = unsafe { prepare_text(text) }?;

    unsafe {
        if OpenClipboard(None).is_err() {
            let _ = GlobalFree(Some(memory));
            return None;
        }
        if EmptyClipboard().is_err() {
            let _ = GlobalFree(Some(memory));
            let _ = CloseClipboard();
            return None;
        }

        let stored = SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(memory.0))).is_ok();
        if !stored {
            let _ = GlobalFree(Some(memory));
            let _ = CloseClipboard();
            return None;
        }

        // Capture the sequence while the clipboard is still ours; reading it
        // after CloseClipboard could bind the snapshot to another program's
        // write and make a later restore overwrite that content.
        let sequence = GetClipboardSequenceNumber();
        let _ = CloseClipboard();
        Some(sequence)
    }
}

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
