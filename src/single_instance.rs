use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, GetWindowThreadProcessId, PostMessageW, RegisterWindowMessageW,
};

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

const ACTIVATION_MESSAGE: PCWSTR = w!("QuickPhrase.Activate");

pub fn activation_message() -> u32 {
    unsafe { RegisterWindowMessageW(ACTIVATION_MESSAGE) }
}

/// Activate the main window, restricting smoke runs to their known primary PID.
pub fn signal_existing(expected_pid: Option<u32>) -> bool {
    let message = activation_message();
    if message == 0 {
        return false;
    }

    // The first instance may still be creating its window right after it took
    // the mutex; retry briefly instead of losing the activation.
    for _ in 0..40 {
        if let Ok(hwnd) = unsafe { FindWindowW(w!("QuickPhrase.MainWindow"), PCWSTR::null()) } {
            if let Some(expected) = expected_pid {
                let mut pid = 0;
                unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
                if pid != expected {
                    return false;
                }
            }
            let posted = unsafe { PostMessageW(Some(hwnd), message, WPARAM(0), LPARAM(0)) }.is_ok();
            if posted {
                return true;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    false
}
