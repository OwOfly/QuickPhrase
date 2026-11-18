use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;

use windows::core::PCWSTR;
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::WindowsAndMessaging::{
    MessageBoxW, MB_ICONERROR, MB_ICONINFORMATION, MB_OK,
};

pub fn wide(text: &str) -> Vec<u16> {
    OsStr::new(text)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

pub fn set_dpi_awareness() -> Result<(), u32> {
    // Expected to fail with ERROR_ACCESS_DENIED when the manifest already set it.
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)
            .map_err(|err| win32_code(err.code().0 as u32))
    }
}

/// HRESULT_FROM_WIN32 prefixes the 16-bit Win32 code with 0x8007.
/// User-facing messages should show the Win32 code, not the HRESULT value.
pub fn win32_code(hresult: u32) -> u32 {
    if hresult & 0xFFFF_0000 == 0x8007_0000 {
        hresult & 0xFFFF
    } else {
        hresult
    }
}

pub fn data_dir() -> PathBuf {
    match std::env::var_os("LOCALAPPDATA") {
        Some(base) => PathBuf::from(base).join("QuickPhrase"),
        None => PathBuf::from(".").join("QuickPhrase"),
    }
}

pub fn error_box(title: &str, text: &str) {
    message_box(title, text, MB_OK | MB_ICONERROR);
}

pub fn info_box(title: &str, text: &str) {
    message_box(title, text, MB_OK | MB_ICONINFORMATION);
}

fn message_box(
    title: &str,
    text: &str,
    flags: windows::Win32::UI::WindowsAndMessaging::MESSAGEBOX_STYLE,
) {
    let title = wide(title);
    let text = wide(text);
    unsafe {
        MessageBoxW(None, PCWSTR(text.as_ptr()), PCWSTR(title.as_ptr()), flags);
    }
}
