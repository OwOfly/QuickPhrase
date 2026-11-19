use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegSetValueExW, HKEY, HKEY_CURRENT_USER,
    KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ,
};
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

pub fn set_startup(enabled: bool) -> Result<(), String> {
    let subkey = wide("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
    let mut key = HKEY::default();
    let status = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut key,
            None,
        )
    };
    if status != ERROR_SUCCESS {
        return Err(format!("registry_open:{}", status.0));
    }

    let name = wide("QuickPhrase");
    let result = if enabled {
        let exe = std::env::current_exe().map_err(|err| err.to_string())?;
        let value = format!("\"{}\"", exe.display());
        let mut bytes = Vec::with_capacity(value.len() * 2 + 2);
        for unit in value.encode_utf16().chain(std::iter::once(0)) {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        unsafe { RegSetValueExW(key, PCWSTR(name.as_ptr()), None, REG_SZ, Some(&bytes)) }
    } else {
        unsafe { RegDeleteValueW(key, PCWSTR(name.as_ptr())) }
    };
    unsafe {
        let _ = RegCloseKey(key);
    }

    if result == ERROR_SUCCESS || (!enabled && result == ERROR_FILE_NOT_FOUND) {
        Ok(())
    } else {
        Err(format!("registry_write:{}", result.0))
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
