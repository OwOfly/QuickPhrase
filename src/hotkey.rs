use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT,
    MOD_SHIFT, MOD_WIN,
};

pub const HOTKEY_ID: i32 = 1;
/// Registration ids for per-snippet hotkeys. The Win32 range ends at 0xBFFF.
pub const SNIPPET_ID_BASE: i32 = 1000;
pub const DEFAULT_HOTKEY: &str = "Ctrl+Alt+Q";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Spec {
    pub modifiers: u32,
    pub vk: u32,
}

impl Spec {
    pub fn text(&self) -> String {
        let mut parts: Vec<&str> = Vec::new();
        if self.modifiers & MOD_CONTROL.0 != 0 {
            parts.push("Ctrl");
        }
        if self.modifiers & MOD_ALT.0 != 0 {
            parts.push("Alt");
        }
        if self.modifiers & MOD_SHIFT.0 != 0 {
            parts.push("Shift");
        }
        if self.modifiers & MOD_WIN.0 != 0 {
            parts.push("Win");
        }
        let key = key_name(self.vk);
        if key.is_empty() {
            parts.join("+")
        } else {
            parts.push(&key);
            parts.join("+")
        }
    }
}

fn key_name(vk: u32) -> String {
    match vk {
        0x30..=0x39 | 0x41..=0x5A => char::from_u32(vk)
            .map(|c| c.to_string())
            .unwrap_or_default(),
        0x70..=0x87 => format!("F{}", vk - 0x6F),
        _ => String::new(),
    }
}

fn key_from_name(name: &str) -> Option<u32> {
    let upper = name.to_ascii_uppercase();
    if upper.len() == 1 {
        let ch = upper.chars().next()?;
        if ch.is_ascii_alphanumeric() {
            return Some(ch as u32);
        }
    }
    if let Some(rest) = upper.strip_prefix('F') {
        if let Ok(number) = rest.parse::<u32>() {
            if (1..=24).contains(&number) {
                return Some(0x6F + number);
            }
        }
    }
    None
}

pub fn parse(text: &str) -> Option<Spec> {
    if text.trim().is_empty() {
        return None;
    }
    let mut modifiers = 0u32;
    let mut key = None;
    for part in text.split('+') {
        let part = part.trim();
        if part.is_empty() {
            return None;
        }
        match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => modifiers |= MOD_CONTROL.0,
            "alt" => modifiers |= MOD_ALT.0,
            "shift" => modifiers |= MOD_SHIFT.0,
            "win" | "super" => modifiers |= MOD_WIN.0,
            other => {
                let parsed = key_from_name(other)?;
                if key.replace(parsed).is_some() {
                    return None;
                }
            }
        }
    }
    let vk = key?;
    if modifiers == 0 {
        return None;
    }
    Some(Spec { modifiers, vk })
}

/// Parses `text` and registers it under `HOTKEY_ID`.
pub fn register_text(hwnd: HWND, text: &str) -> Result<Spec, String> {
    let spec = parse(text).ok_or_else(|| "hotkey_invalid".to_string())?;
    register_spec(hwnd, spec)?;
    Ok(spec)
}

pub fn register_spec(hwnd: HWND, spec: Spec) -> Result<(), String> {
    register_spec_id(hwnd, HOTKEY_ID, spec)
}

pub fn register_spec_id(hwnd: HWND, id: i32, spec: Spec) -> Result<(), String> {
    let modifiers = HOT_KEY_MODIFIERS(spec.modifiers) | MOD_NOREPEAT;
    unsafe {
        RegisterHotKey(Some(hwnd), id, modifiers, spec.vk)
            .map_err(|err| crate::platform::win32_code(err.code().0 as u32).to_string())
    }
}

pub fn unregister(hwnd: HWND) {
    unregister_id(hwnd, HOTKEY_ID);
}

pub fn unregister_id(hwnd: HWND, id: i32) {
    unsafe {
        let _ = UnregisterHotKey(Some(hwnd), id);
    }
}

/// Try a candidate without disturbing the active registration.
pub fn is_available(hwnd: HWND, text: &str) -> bool {
    let Some(spec) = parse(text) else {
        return false;
    };
    unsafe {
        let ok =
            RegisterHotKey(Some(hwnd), 999, HOT_KEY_MODIFIERS(spec.modifiers), spec.vk).is_ok();
        if ok {
            let _ = UnregisterHotKey(Some(hwnd), 999);
        }
        ok
    }
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn parses_default_combo() {
        let spec = parse("Ctrl+Alt+Q").expect("parse");
        assert_eq!(spec.vk, 0x51);
        assert_eq!(spec.text(), "Ctrl+Alt+Q");
    }

    #[test]
    fn rejects_invalid_combos() {
        assert!(parse("Q").is_none());
        assert!(parse("Ctrl+").is_none());
        assert!(parse("Ctrl+Alt+Nope").is_none());
    }

    #[test]
    fn parses_function_keys_and_shift() {
        let spec = parse("Shift+Ctrl+F5").expect("parse");
        assert_eq!(spec.vk, 0x74);
        assert!(spec.text().contains("Shift"));
    }

    #[test]
    fn rejects_multiple_keys() {
        assert!(parse("Ctrl+A+B").is_none());
        assert!(parse("Ctrl+Nope+Q").is_none());
    }

    #[test]
    fn rejects_empty_segments() {
        assert!(parse("Ctrl++A").is_none());
        assert!(parse("+Ctrl+A").is_none());
        assert!(parse("Ctrl+A+").is_none());
        assert!(parse("Ctrl + Alt + Q").is_some());
    }
}
