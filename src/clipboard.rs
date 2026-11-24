use std::sync::Mutex;

use windows::Win32::Foundation::{
    GetLastError, GlobalFree, SetLastError, ERROR_SUCCESS, HANDLE, HGLOBAL, HWND,
};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, EnumClipboardFormats, GetClipboardData,
    GetClipboardSequenceNumber, IsClipboardFormatAvailable, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE,
};
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

impl Snapshot {
    /// Whether the full previous Unicode text was saved with a restore token.
    pub fn can_restore(&self) -> bool {
        self.sequence != 0 && self.text.is_some()
    }

    /// Describe exactly what can be recovered after overwriting the clipboard.
    pub fn previous_text_status(&self) -> &'static str {
        match (self.can_restore(), self.had_non_text, self.read_ok) {
            (true, true, _) => "仅原纯文本可恢复，其他格式已丢失",
            (true, false, false) => "原纯文本可恢复，其他格式无法确认",
            (true, false, true) => "原纯文本可在托盘恢复",
            (false, _, false) => "未能完整读取原内容，无法恢复",
            (false, true, true) => "原内容含非文本格式，无法恢复",
            (false, false, true) => "没有可恢复的原纯文本",
        }
    }
}

/// Write from an application-owned window, keeping the previous Unicode text.
pub fn set_text_with_snapshot(owner: HWND, text: &str) -> (bool, Snapshot) {
    replace_text(owner, text, None)
}

pub fn last_snapshot() -> Option<Snapshot> {
    LAST_SNAPSHOT.lock().ok().and_then(|slot| slot.clone())
}

/// Restore only if our original write is still the current clipboard value.
pub fn restore(owner: HWND, snapshot: &Snapshot) -> bool {
    if !snapshot.can_restore() {
        return false;
    }
    snapshot
        .text
        .as_deref()
        .is_some_and(|text| replace_text(owner, text, Some(snapshot.sequence)).0)
}

fn replace_text(owner: HWND, text: &str, expected: Option<u32>) -> (bool, Snapshot) {
    if owner.0.is_null() {
        return (false, Snapshot::default());
    }
    let Some(memory) = (unsafe { prepare_text(text) }) else {
        return (false, Snapshot::default());
    };
    unsafe {
        // A null owner would become the clipboard owner after EmptyClipboard,
        // making SetClipboardData fail. Use our own HWND, never the target's.
        if OpenClipboard(Some(owner)).is_err() {
            let _ = GlobalFree(Some(memory));
            return (false, Snapshot::default());
        }
        let result = replace_open_clipboard(memory, expected);
        let _ = CloseClipboard();
        result
    }
}

/// Owns `memory` while the clipboard is open. Roll back only after this very
/// operation cleared it, while no other application can write a newer value.
unsafe fn replace_open_clipboard(memory: HGLOBAL, expected: Option<u32>) -> (bool, Snapshot) {
    let mut previous = Snapshot::default();
    let mut rollback = None;
    let stored = (|| {
        if expected.is_some_and(|seq| GetClipboardSequenceNumber() != seq) {
            return false;
        }
        previous = read_open_snapshot();
        if let Some(text) = previous.text.as_deref() {
            rollback = prepare_text(text);
            if rollback.is_none() {
                return false;
            }
        }
        if EmptyClipboard().is_err() {
            return false;
        }
        if !store_handle(memory) {
            if let Some(old) = rollback.take() {
                if !store_handle(old) {
                    let _ = GlobalFree(Some(old));
                }
            }
            return false;
        }
        previous.sequence = GetClipboardSequenceNumber();
        if expected.is_none() {
            if let Ok(mut slot) = LAST_SNAPSHOT.lock() {
                *slot = Some(previous.clone());
            }
        }
        true
    })();
    if !stored {
        let _ = GlobalFree(Some(memory));
    }
    if let Some(old) = rollback {
        let _ = GlobalFree(Some(old));
    }
    (stored, previous)
}

unsafe fn store_handle(memory: HGLOBAL) -> bool {
    SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(memory.0))).is_ok()
}

unsafe fn read_open_snapshot() -> Snapshot {
    let mut had_non_text = false;
    let mut format = 0;
    loop {
        SetLastError(ERROR_SUCCESS);
        format = EnumClipboardFormats(format);
        if format == 0 {
            break;
        }
        had_non_text |= !is_text_format(format);
    }
    let formats_ok = GetLastError() == ERROR_SUCCESS;
    let (text, read_ok) = if IsClipboardFormatAvailable(CF_UNICODETEXT.0 as u32).is_ok() {
        let text = read_unicode_text();
        let read_ok = text.is_some();
        (text, read_ok)
    } else {
        (None, true)
    };
    Snapshot {
        text,
        had_non_text,
        sequence: 0,
        read_ok: formats_ok && read_ok,
    }
}

unsafe fn read_unicode_text() -> Option<String> {
    let handle = GetClipboardData(CF_UNICODETEXT.0 as u32).ok()?;
    let memory = HGLOBAL(handle.0);
    let count = (GlobalSize(memory) / std::mem::size_of::<u16>()).min(MAX_SNAPSHOT_UNITS + 1);
    if count == 0 {
        return None;
    }
    let pointer = GlobalLock(memory) as *const u16;
    if pointer.is_null() {
        return None;
    }
    let text = decode_snapshot(std::slice::from_raw_parts(pointer, count));
    let _ = GlobalUnlock(memory);
    text
}

fn decode_snapshot(units: &[u16]) -> Option<String> {
    // A missing terminator means the allocation or snapshot window only
    // contains a prefix. Never offer that prefix as a full restore.
    let end = units
        .iter()
        .take(MAX_SNAPSHOT_UNITS + 1)
        .position(|&unit| unit == 0)?;
    String::from_utf16(&units[..end]).ok()
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

pub fn sequence() -> u32 {
    unsafe { GetClipboardSequenceNumber() }
}

#[cfg(test)]
mod tests {
    use super::{decode_snapshot, is_text_format, MAX_SNAPSHOT_UNITS};

    #[test]
    fn snapshot_requires_a_terminator_within_its_allocation_and_limit() {
        assert_eq!(decode_snapshot(&[65, 0, 66]).as_deref(), Some("A"));
        assert_eq!(decode_snapshot(&[0xd800, 0]), None);
        assert_eq!(decode_snapshot(&[65]), None);
        let mut full = vec![65; MAX_SNAPSHOT_UNITS];
        full.push(0);
        assert_eq!(decode_snapshot(&full).unwrap().len(), MAX_SNAPSHOT_UNITS);
        full.insert(0, 65);
        assert_eq!(decode_snapshot(&full), None);
    }

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
