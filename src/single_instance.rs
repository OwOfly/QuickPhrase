use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, PostMessageW, RegisterWindowMessageW};

use windows::Win32::Foundation::{LPARAM, WPARAM};

pub enum Acquire {
    First(HANDLE),
    AlreadyRunning,
    Failed(u32),
}

pub fn acquire() -> Acquire {
    match unsafe { CreateMutexW(None, false, w!("Local\\QuickPhrase.SingleInstance")) } {
        Ok(handle) => {
            if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
                unsafe {
                    let _ = CloseHandle(handle);
                }
                Acquire::AlreadyRunning
            } else {
                Acquire::First(handle)
            }
        }
        Err(err) => Acquire::Failed(err.code().0 as u32),
    }
}

pub fn signal_existing() {
    let message = unsafe { RegisterWindowMessageW(w!("QuickPhrase.Activate")) };
    if message == 0 {
        return;
    }

    if let Ok(hwnd) = unsafe { FindWindowW(w!("QuickPhrase.MainWindow"), PCWSTR::null()) } {
        unsafe {
            let _ = PostMessageW(Some(hwnd), message, WPARAM(0), LPARAM(0));
        }
    }
}
