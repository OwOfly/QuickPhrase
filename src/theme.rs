//! Shared colour tokens for QuickPhrase 2.0 (Porcelain Mica Light / Obsidian Velvet Dark).
//!
//! `theme = system` follows the Windows app theme; light/dark are explicit.
//! Every foreground/background pair is checked against WCAG AA contrast ratios.

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{COLORREF, ERROR_SUCCESS, WIN32_ERROR};
use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};

const APP_THEME_KEY: PCWSTR =
    w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize");
const APP_THEME_VALUE: PCWSTR = w!("AppsUseLightTheme");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub dark: bool,
    pub page: COLORREF,
    pub sidebar: COLORREF,
    pub card: COLORREF,
    pub card_subtle: COLORREF,
    pub heading: COLORREF,
    pub secondary: COLORREF,
    pub muted: COLORREF,
    pub border: COLORREF,
    pub border_strong: COLORREF,
    pub divider: COLORREF,
    pub hover: COLORREF,
    pub selection: COLORREF,
    pub selection_border: COLORREF,
    pub primary: COLORREF,
    pub primary_hover: COLORREF,
    /// Pressed/hover fill where white button text must keep >=4.5:1.
    pub primary_active: COLORREF,
    pub primary_soft: COLORREF,
    pub primary_text: COLORREF,
    pub highlight: COLORREF,
    pub highlight_text: COLORREF,
    pub kbd_bg: COLORREF,
    pub kbd_border: COLORREF,
    pub success: COLORREF,
    pub success_soft: COLORREF,
    pub warning: COLORREF,
    pub warning_soft: COLORREF,
    pub error: COLORREF,
    /// Destructive-button text/border; keeps >=4.5:1 on card/danger_soft.
    pub danger: COLORREF,
    pub danger_soft: COLORREF,
    /// Background used by inputs (search box, edits).
    pub field: COLORREF,
    pub field_focus: COLORREF,
}

pub const fn rgb(r: u32, g: u32, b: u32) -> COLORREF {
    COLORREF(r | (g << 8) | (b << 16))
}

/// Deterministic category accent dot palette.
pub const CATEGORY_DOTS: [COLORREF; 6] = [
    rgb(0x5B, 0x50, 0xF0), // Iris Indigo
    rgb(0x0E, 0xA5, 0xA4), // Teal
    rgb(0xD9, 0x77, 0x06), // Amber
    rgb(0xE1, 0x1D, 0x68), // Rose
    rgb(0x25, 0x63, 0xEB), // Cobalt
    rgb(0x7C, 0x3A, 0xED), // Violet
];

#[allow(dead_code)]
pub fn category_dot(index: usize) -> COLORREF {
    CATEGORY_DOTS[index % CATEGORY_DOTS.len()]
}

pub fn category_dot_for_name(name: &str) -> COLORREF {
    if name.is_empty() {
        return rgb(0x88, 0x90, 0xA8);
    }
    let mut hash: usize = 5381;
    for byte in name.as_bytes() {
        hash = hash.wrapping_mul(33).wrapping_add(*byte as usize);
    }
    CATEGORY_DOTS[hash % CATEGORY_DOTS.len()]
}

pub const LIGHT: Palette = Palette {
    dark: false,
    page: rgb(0xF3, 0xF5, 0xFA),
    sidebar: rgb(0xF7, 0xF8, 0xFC),
    card: rgb(0xFF, 0xFF, 0xFF),
    card_subtle: rgb(0xF7, 0xF8, 0xFC),
    heading: rgb(0x16, 0x18, 0x26),
    secondary: rgb(0x58, 0x5F, 0x78),
    muted: rgb(0x64, 0x6C, 0x86),
    border: rgb(0xE4, 0xE7, 0xF2),
    border_strong: rgb(0xCE, 0xD4, 0xE6),
    divider: rgb(0xEC, 0xEF, 0xF7),
    hover: rgb(0xF1, 0xF3, 0xFA),
    selection: rgb(0xEE, 0xF0, 0xFF),
    selection_border: rgb(0xB9, 0xB3, 0xFA),
    primary: rgb(0x5B, 0x50, 0xF0),
    primary_hover: rgb(0x4A, 0x3E, 0xE0),
    primary_active: rgb(0x3D, 0x31, 0xCC),
    primary_soft: rgb(0xEF, 0xEF, 0xFE),
    primary_text: rgb(0x4C, 0x40, 0xE0),
    highlight: rgb(0xFF, 0xF0, 0xB3),
    highlight_text: rgb(0x46, 0x2A, 0xA8),
    kbd_bg: rgb(0xFF, 0xFF, 0xFF),
    kbd_border: rgb(0xD6, 0xDB, 0xE9),
    success: rgb(0x15, 0x80, 0x3D),
    success_soft: rgb(0xE8, 0xF7, 0xEE),
    warning: rgb(0xB4, 0x53, 0x09),
    warning_soft: rgb(0xFE, 0xF3, 0xDC),
    error: rgb(0xDC, 0x26, 0x26),
    danger: rgb(0xC6, 0x28, 0x28),
    danger_soft: rgb(0xFD, 0xEC, 0xEC),
    field: rgb(0xF1, 0xF3, 0xF9),
    field_focus: rgb(0xFF, 0xFF, 0xFF),
};

pub const DARK: Palette = Palette {
    dark: true,
    page: rgb(0x10, 0x12, 0x1A),
    sidebar: rgb(0x14, 0x17, 0x22),
    card: rgb(0x18, 0x1B, 0x26),
    card_subtle: rgb(0x15, 0x18, 0x23),
    heading: rgb(0xF2, 0xF4, 0xFB),
    secondary: rgb(0x9E, 0xA6, 0xC0),
    muted: rgb(0x7E, 0x87, 0xA6),
    border: rgb(0x2A, 0x2F, 0x42),
    border_strong: rgb(0x3A, 0x41, 0x59),
    divider: rgb(0x23, 0x28, 0x38),
    hover: rgb(0x22, 0x27, 0x38),
    selection: rgb(0x26, 0x25, 0x47),
    selection_border: rgb(0x5E, 0x56, 0xC4),
    primary: rgb(0x7B, 0x72, 0xFF),
    primary_hover: rgb(0x6C, 0x62, 0xEE),
    primary_active: rgb(0x5B, 0x50, 0xDC),
    primary_soft: rgb(0x25, 0x23, 0x4A),
    primary_text: rgb(0xAD, 0xA6, 0xFF),
    highlight: rgb(0x4A, 0x3D, 0x14),
    highlight_text: rgb(0xFD, 0xE6, 0x8A),
    kbd_bg: rgb(0x25, 0x2A, 0x3C),
    kbd_border: rgb(0x39, 0x40, 0x5A),
    success: rgb(0x4A, 0xDE, 0x80),
    success_soft: rgb(0x16, 0x33, 0x24),
    warning: rgb(0xFB, 0xBF, 0x24),
    warning_soft: rgb(0x36, 0x2B, 0x14),
    error: rgb(0xF8, 0x71, 0x71),
    danger: rgb(0xF8, 0x71, 0x71),
    danger_soft: rgb(0x3A, 0x1E, 0x24),
    field: rgb(0x21, 0x26, 0x36),
    field_focus: rgb(0x26, 0x2C, 0x3F),
};

/// Resolve a persisted preference (`system` / `light` / `dark`).
pub fn resolve(preference: &str) -> Palette {
    match preference {
        "dark" => DARK,
        "light" => LIGHT,
        _ => {
            if system_uses_light() {
                LIGHT
            } else {
                DARK
            }
        }
    }
}

thread_local! {
    static PREVIEW: std::cell::RefCell<Option<Palette>> = const { std::cell::RefCell::new(None) };
}

pub fn current() -> Palette {
    PREVIEW
        .with(|preview| preview.borrow().to_owned())
        .unwrap_or_else(|| resolve(&crate::storage::settings_clone().unwrap_or_default().theme))
}

pub fn preview(palette: Palette) {
    PREVIEW.with(|slot| *slot.borrow_mut() = Some(palette));
}

pub fn clear_preview() {
    PREVIEW.with(|slot| *slot.borrow_mut() = None);
}

fn system_uses_light() -> bool {
    let mut value = 1u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    let status: WIN32_ERROR = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            APP_THEME_KEY,
            APP_THEME_VALUE,
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut u32 as *mut core::ffi::c_void),
            Some(&mut size),
        )
    };
    if status != ERROR_SUCCESS {
        return true;
    }
    value != 0
}

/// WCAG relative-luminance contrast ratio; used by the theme tests.
#[cfg(test)]
pub fn contrast_ratio(a: COLORREF, b: COLORREF) -> f64 {
    let la = luminance(a);
    let lb = luminance(b);
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

#[cfg(test)]
fn luminance(color: COLORREF) -> f64 {
    let channel = |value: u32| {
        let s = value as f64 / 255.0;
        if s <= 0.03928 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    let r = channel(color.0 & 0xFF);
    let g = channel((color.0 >> 8) & 0xFF);
    let b = channel((color.0 >> 16) & 0xFF);
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

#[cfg(test)]
mod tests {
    use super::{clear_preview, contrast_ratio, current, preview, resolve, COLORREF, DARK, LIGHT};

    #[test]
    fn preview_overrides_current_and_clear_is_idempotent() {
        preview(DARK);
        assert_eq!(current(), DARK);
        clear_preview();
        assert_eq!(current(), resolve("system"));
        clear_preview();
        assert_eq!(current(), resolve("system"));
    }

    #[test]
    fn explicit_preferences_resolve() {
        assert_eq!(resolve("light").dark, false);
        assert_eq!(resolve("dark").dark, true);
        assert!(contrast_ratio(LIGHT.heading, LIGHT.card) >= 4.5);
        assert!(contrast_ratio(DARK.heading, DARK.card) >= 4.5);
    }

    #[test]
    fn key_text_pairs_meet_contrast() {
        for palette in [LIGHT, DARK] {
            assert!(
                contrast_ratio(palette.heading, palette.card) >= 4.5,
                "heading on card"
            );
            assert!(
                contrast_ratio(palette.secondary, palette.card) >= 4.5,
                "secondary on card"
            );
            assert!(
                contrast_ratio(palette.primary_text, palette.card) >= 4.5,
                "primary_text on card"
            );
            assert!(
                contrast_ratio(palette.primary, palette.card) >= 3.0,
                "primary on card"
            );
            assert!(
                contrast_ratio(palette.highlight_text, palette.highlight) >= 4.5,
                "highlight pair"
            );
            assert!(
                contrast_ratio(palette.heading, palette.selection) >= 4.5,
                "heading on selection"
            );
            assert!(
                contrast_ratio(palette.heading, palette.field) >= 4.5,
                "heading on field"
            );
            assert!(
                contrast_ratio(palette.secondary, palette.page) >= 4.5,
                "secondary on page"
            );
            assert!(
                contrast_ratio(palette.secondary, palette.sidebar) >= 4.5,
                "secondary on sidebar"
            );
            assert!(
                contrast_ratio(palette.muted, palette.page) >= 4.5,
                "muted on page"
            );
            assert!(
                contrast_ratio(palette.muted, palette.card_subtle) >= 4.5,
                "muted on card_subtle"
            );
            let button_fill = if palette.dark {
                palette.primary_hover
            } else {
                palette.primary
            };
            assert!(
                contrast_ratio(COLORREF(0xFFFFFF), button_fill) >= 4.5,
                "white primary-button text"
            );
            assert!(
                contrast_ratio(palette.danger, palette.card) >= 4.5,
                "danger button text"
            );
            assert!(
                contrast_ratio(palette.warning, palette.warning_soft) >= 4.5,
                "warning banner text"
            );
            assert!(
                contrast_ratio(palette.danger, palette.danger_soft) >= 4.5,
                "danger banner text"
            );
        }
    }
}
