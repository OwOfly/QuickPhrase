//! Shared colour tokens for the panel, management and settings windows.
//!
//! `theme = system` follows the Windows app theme; light/dark are explicit.
//! Values come from PRD §14.2; dark values keep the same roles and contrast.

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
    pub card: COLORREF,
    pub heading: COLORREF,
    pub secondary: COLORREF,
    pub border: COLORREF,
    pub selection: COLORREF,
    pub primary: COLORREF,
    pub primary_hover: COLORREF,
    /// Pressed/hover fill where white button text must keep >=4.5:1.
    pub primary_active: COLORREF,
    pub highlight: COLORREF,
    pub highlight_text: COLORREF,
    pub success: COLORREF,
    pub warning: COLORREF,
    pub error: COLORREF,
    /// Destructive-button text/border; darker than `error` so white cards
    /// keep >=4.5:1.
    pub danger: COLORREF,
    /// Background used by inputs (search box, edits).
    pub field: COLORREF,
}

const fn c(r: u32, g: u32, b: u32) -> COLORREF {
    COLORREF(r | (g << 8) | (b << 16))
}

pub const LIGHT: Palette = Palette {
    dark: false,
    page: c(0xF5, 0xF6, 0xFB),
    card: c(0xFF, 0xFF, 0xFF),
    heading: c(0x22, 0x24, 0x3E),
    secondary: c(0x77, 0x7D, 0x92),
    border: c(0xE9, 0xEB, 0xF3),
    selection: c(0xF0, 0xED, 0xFF),
    primary: c(0x76, 0x55, 0xE9),
    primary_hover: c(0x5E, 0x45, 0xD7),
    primary_active: c(0x5E, 0x45, 0xD7),
    highlight: c(0xFF, 0xF1, 0xB8),
    highlight_text: c(0x4A, 0x2C, 0xB8),
    success: c(0x22, 0xC5, 0x5E),
    warning: c(0xF5, 0x9E, 0x0B),
    error: c(0xEF, 0x44, 0x44),
    danger: c(0xC6, 0x28, 0x28),
    field: c(0xF1, 0xF2, 0xF9),
};

pub const DARK: Palette = Palette {
    dark: true,
    page: c(0x14, 0x15, 0x1F),
    card: c(0x1D, 0x1F, 0x2B),
    heading: c(0xF2, 0xF3, 0xFA),
    secondary: c(0xA6, 0xAC, 0xC4),
    border: c(0x2C, 0x30, 0x40),
    selection: c(0x2A, 0x24, 0x50),
    primary: c(0x8B, 0x6C, 0xF2),
    primary_hover: c(0x7A, 0x58, 0xE8),
    primary_active: c(0x6D, 0x4A, 0xD8),
    highlight: c(0x4B, 0x3D, 0x12),
    highlight_text: c(0xFF, 0xE9, 0xA8),
    success: c(0x4A, 0xDE, 0x80),
    warning: c(0xFB, 0xBF, 0x24),
    error: c(0xF8, 0x71, 0x71),
    danger: c(0xF8, 0x71, 0x71),
    field: c(0x23, 0x26, 0x34),
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

pub fn current() -> Palette {
    resolve(&crate::storage::settings_clone().unwrap_or_default().theme)
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
    use super::{contrast_ratio, resolve, COLORREF, DARK, LIGHT};

    #[test]
    fn explicit_preferences_resolve() {
        assert_eq!(resolve("light").dark, false);
        assert_eq!(resolve("dark").dark, true);
        assert_eq!(contrast_ratio(LIGHT.heading, LIGHT.card) >= 4.5, true);
        assert_eq!(contrast_ratio(DARK.heading, DARK.card) >= 4.5, true);
    }

    #[test]
    fn key_text_pairs_meet_contrast() {
        for palette in [LIGHT, DARK] {
            assert!(
                contrast_ratio(palette.heading, palette.card) >= 4.5,
                "heading on card"
            );
            assert!(
                contrast_ratio(palette.secondary, palette.card) >= 3.0,
                "secondary on card"
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
                contrast_ratio(palette.secondary, palette.page) >= 3.0,
                "secondary on page"
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
        }
    }
}
