use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS, HANDLE, LPARAM, WPARAM};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, PostMessageW, RegisterWindowMessageW};

pub enum Acquire {
    First(HANDLE),
    AlreadyRunning,
}

pub fn acquire() -> Acquire {
    match unsafe { CreateMutexW(None, false, w!("Local\\QuickPhrase.SingleInstance")) } {
        Ok(handle) => {
            if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
                Acquire::AlreadyRunning
            } else {
                Acquire::First(handle)
            }
        }
        Err(_) => Acquire::First(HANDLE::default()),
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
