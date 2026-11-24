use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_INFO, NIM_ADD, NIM_DELETE,
    NIM_MODIFY, NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::{HICON, WM_APP};

pub const TRAY_ID: u32 = 1;
pub const WM_TRAY: u32 = WM_APP + 1;

fn base(hwnd: HWND) -> NOTIFYICONDATAW {
    let mut data = unsafe { std::mem::zeroed::<NOTIFYICONDATAW>() };
    data.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    data.hWnd = hwnd;
    data.uID = TRAY_ID;
    data
}

fn copy_into<const N: usize>(target: &mut [u16; N], text: &str) {
    let encoded: Vec<u16> = text.encode_utf16().take(N - 1).collect();
    target[..encoded.len()].copy_from_slice(&encoded);
}

pub fn add(hwnd: HWND, icon: HICON, tip: &str) -> bool {
    let mut data = base(hwnd);
    data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    data.uCallbackMessage = WM_TRAY;
    data.hIcon = icon;
    copy_into(&mut data.szTip, tip);

    unsafe { Shell_NotifyIconW(NIM_ADD, &data).as_bool() }
}

pub fn remove(hwnd: HWND) {
    let data = base(hwnd);
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &data);
    }
}

pub fn set_tip(hwnd: HWND, tip: &str) {
    let mut data = base(hwnd);
    data.uFlags = NIF_TIP;
    copy_into(&mut data.szTip, tip);
    unsafe {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &data);
    }
}

pub fn balloon(hwnd: HWND, title: &str, text: &str) -> bool {
    let mut data = base(hwnd);
    data.uFlags = NIF_INFO;
    data.dwInfoFlags = NIIF_INFO;
    copy_into(&mut data.szInfoTitle, title);
    copy_into(&mut data.szInfo, text);
    unsafe { Shell_NotifyIconW(NIM_MODIFY, &data).as_bool() }
}
