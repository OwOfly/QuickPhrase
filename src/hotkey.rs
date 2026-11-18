use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT,
};

pub const HOTKEY_ID: i32 = 1;
pub const HOTKEY_VK_Q: u32 = 0x51;

pub fn register(hwnd: HWND) -> Result<(), u32> {
    let modifiers: HOT_KEY_MODIFIERS = MOD_CONTROL | MOD_ALT | MOD_NOREPEAT;
    unsafe {
        RegisterHotKey(Some(hwnd), HOTKEY_ID, modifiers, HOTKEY_VK_Q)
            .map_err(|err| crate::platform::win32_code(err.code().0 as u32))
    }
}

pub fn unregister(hwnd: HWND) {
    unsafe {
        let _ = UnregisterHotKey(Some(hwnd), HOTKEY_ID);
    }
}
