//! Shared UI chrome, owner-draw Direct2D controls, DWM styling and optical layout helpers.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use windows::core::PCWSTR;
use windows::Win32::Foundation::{
    COLORREF, HINSTANCE, HMODULE, HWND, LPARAM, LRESULT, RECT, WPARAM,
};
use windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_BORDER_COLOR, DWMWA_CAPTION_COLOR, DWMWA_SYSTEMBACKDROP_TYPE,
    DWMWA_TEXT_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
    DWM_SYSTEMBACKDROP_TYPE,
};
use windows::Win32::Graphics::Gdi::{
    CreateFontW, GetDC, GetTextMetricsW, InvalidateRect, ReleaseDC, SelectObject,
    CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DEFAULT_PITCH, HDC, HFONT, HGDIOBJ,
    OUT_DEFAULT_PRECIS, TEXTMETRICW,
};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::System::SystemInformation::OSVERSIONINFOW;
use windows::Win32::UI::Controls::{
    SetWindowTheme, DRAWITEMSTRUCT, EM_SETCUEBANNER, ODS_COMBOBOXEDIT, ODS_DISABLED, ODS_FOCUS,
    ODS_SELECTED, WM_MOUSELEAVE,
};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetFocus, ReleaseCapture, SetCapture, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallWindowProcW, DefWindowProcW, GetClientRect, GetParent, GetSystemMetrics, GetWindowLongPtrW,
    GetWindowTextW, LoadIconW, LoadImageW, SendMessageW, SetWindowLongPtrW, CB_GETCOUNT,
    CB_GETLBTEXT, GWLP_WNDPROC, HICON, ICON_BIG, ICON_SMALL, IDI_APPLICATION, IMAGE_ICON,
    LR_DEFAULTCOLOR, LR_SHARED, SM_CXICON, SM_CXSMICON, SM_CYICON, SM_CYSMICON, WM_CAPTURECHANGED,
    WM_CHAR, WM_ENABLE, WM_ERASEBKGND, WM_GETFONT, WM_KEYDOWN, WM_KEYUP, WM_KILLFOCUS,
    WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_NCDESTROY,
    WM_PAINT, WM_PRINTCLIENT, WM_SETFOCUS, WM_SETICON, WM_SIZE, WM_VSCROLL,
};

use crate::d2d::{self, AlignH, AlignV, Icon, TextStyle};
use crate::theme::Palette;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FontRole {
    /// Single-line search input (13.5px Regular).
    Search,
    /// Editor title input (14px SemiBold, matches `TextStyle::Title`).
    TitleInput,
    /// Editor body, category input, and general controls (13px Regular, matches `TextStyle::Body`).
    BodyInput,
}

/// Create a GDI `HFONT` using the exact same resolved UI font family and scale
/// as DirectWrite so native `EDIT` controls blend seamlessly with painted UI.
pub fn make_font(role: FontRole, dpi: u32) -> HFONT {
    let (logical_px, weight) = match role {
        FontRole::Search => (13.5_f32, 400),
        FontRole::TitleInput => (14.0_f32, 600),
        FontRole::BodyInput => (13.0_f32, 400),
    };
    let height = -(logical_px * dpi as f32 / 96.0).round() as i32;
    let face = crate::platform::wide(d2d::ui_font_family());
    unsafe {
        CreateFontW(
            height,
            0,
            0,
            0,
            weight,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            DEFAULT_PITCH.0 as u32,
            PCWSTR(face.as_ptr()),
        )
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ButtonKind {
    Primary,
    Secondary,
    Danger,
    Subtle,
}

thread_local! {
    static OLD_PROCS: RefCell<HashMap<isize, isize>> = RefCell::new(HashMap::new());
    static COMBO_PROCS: RefCell<HashMap<isize, isize>> = RefCell::new(HashMap::new());
    static HOVERED: RefCell<HashSet<isize>> = RefCell::new(HashSet::new());
}

pub fn scaled(value: i32, dpi: u32) -> i32 {
    (value * dpi as i32 + 48) / 96
}

fn round_radius(dpi: u32, height: i32) -> f32 {
    d2d::scaled_f(9.0, dpi).min((height as f32 / 2.6).max(4.0))
}

/// Compute an optically vertically-centered rect for a single-line Win32 `EDIT`
/// sitting inside a Direct2D rounded input shell.
pub fn edit_centered_rect(shell: RECT, font: HFONT, pad_x_left: i32, pad_x_right: i32) -> RECT {
    let text_h = font_line_height(font).max(12);
    let shell_h = (shell.bottom - shell.top).max(text_h);
    let top = shell.top + ((shell_h - text_h) / 2).max(1);
    let bottom = (top + text_h + 2).min(shell.bottom - 1);
    RECT {
        left: shell.left + pad_x_left,
        top,
        right: (shell.right - pad_x_right).max(shell.left + pad_x_left + 1),
        bottom: bottom.max(top + 1),
    }
}

pub fn font_line_height(font: HFONT) -> i32 {
    if font.0.is_null() {
        return 16;
    }
    unsafe {
        let hdc = GetDC(None);
        if hdc.0.is_null() {
            return 16;
        }
        let old = SelectObject(hdc, HGDIOBJ(font.0));
        let mut tm = TEXTMETRICW::default();
        let ok = GetTextMetricsW(hdc, &mut tm).as_bool();
        let _ = SelectObject(hdc, old);
        let _ = ReleaseDC(None, hdc);
        if ok && tm.tmHeight > 0 {
            tm.tmHeight
        } else {
            16
        }
    }
}

/// Subclass a button so hover states repaint cleanly. Safe to call more than once.
pub fn install_button(hwnd: HWND) {
    if hwnd.0.is_null() {
        return;
    }
    let key = hwnd.0 as isize;
    if OLD_PROCS.with(|map| map.borrow().contains_key(&key)) {
        return;
    }
    unsafe {
        let old = GetWindowLongPtrW(hwnd, GWLP_WNDPROC);
        if old == 0 {
            return;
        }
        OLD_PROCS.with(|map| map.borrow_mut().insert(key, old));
        let previous = SetWindowLongPtrW(
            hwnd,
            GWLP_WNDPROC,
            button_proc as *const () as usize as isize,
        );
        if previous != old {
            OLD_PROCS.with(|map| {
                map.borrow_mut().remove(&key);
            });
        }
    }
}

unsafe extern "system" fn button_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let key = hwnd.0 as isize;
    let old = OLD_PROCS
        .with(|map| map.borrow().get(&key).copied())
        .unwrap_or(0);
    match message {
        WM_ERASEBKGND => return LRESULT(1),
        WM_MOUSEMOVE => {
            let first = HOVERED.with(|set| set.borrow_mut().insert(key));
            if first {
                let mut track = TRACKMOUSEEVENT {
                    cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                let _ = TrackMouseEvent(&mut track);
                let _ = InvalidateRect(Some(hwnd), None, false);
                if let Ok(parent) = GetParent(hwnd) {
                    if !parent.0.is_null() {
                        let _ = InvalidateRect(Some(parent), None, false);
                    }
                }
            }
        }
        WM_MOUSELEAVE => {
            HOVERED.with(|set| {
                set.borrow_mut().remove(&key);
            });
            let _ = InvalidateRect(Some(hwnd), None, false);
            if let Ok(parent) = GetParent(hwnd) {
                if !parent.0.is_null() {
                    let _ = InvalidateRect(Some(parent), None, false);
                }
            }
        }
        _ => {}
    }

    if old == 0 {
        return DefWindowProcW(hwnd, message, wparam, lparam);
    }
    let proc: unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT =
        std::mem::transmute(old);
    let result = CallWindowProcW(Some(proc), hwnd, message, wparam, lparam);
    if message == WM_NCDESTROY {
        HOVERED.with(|set| {
            set.borrow_mut().remove(&key);
        });
        OLD_PROCS.with(|map| {
            map.borrow_mut().remove(&key);
        });
    }
    result
}

/// Subclass a combo box so its closed face, rounded frame, chevron and focus
/// ring are painted with Direct2D in both light and dark themes.
pub fn install_combo(hwnd: HWND) {
    if hwnd.0.is_null() {
        return;
    }
    let key = hwnd.0 as isize;
    if COMBO_PROCS.with(|map| map.borrow().contains_key(&key)) {
        return;
    }
    unsafe {
        let old = GetWindowLongPtrW(hwnd, GWLP_WNDPROC);
        if old == 0 {
            return;
        }
        COMBO_PROCS.with(|map| map.borrow_mut().insert(key, old));
        let previous = SetWindowLongPtrW(
            hwnd,
            GWLP_WNDPROC,
            combo_proc as *const () as usize as isize,
        );
        if previous != old {
            COMBO_PROCS.with(|map| {
                map.borrow_mut().remove(&key);
            });
        }
    }
}

unsafe extern "system" fn combo_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let key = hwnd.0 as isize;
    let old = COMBO_PROCS
        .with(|map| map.borrow().get(&key).copied())
        .unwrap_or(0);
    if old == 0 {
        return DefWindowProcW(hwnd, message, wparam, lparam);
    }
    let proc: unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT =
        std::mem::transmute(old);
    let result = CallWindowProcW(Some(proc), hwnd, message, wparam, lparam);
    if message == WM_PAINT {
        let palette = crate::theme::current();
        paint_combo_chrome(hwnd, palette);
    }
    if message == WM_SETFOCUS || message == WM_KILLFOCUS {
        let _ = InvalidateRect(Some(hwnd), None, false);
    }
    if message == WM_NCDESTROY {
        COMBO_PROCS.with(|map| {
            map.borrow_mut().remove(&key);
        });
    }
    result
}

fn paint_combo_chrome(hwnd: HWND, palette: Palette) {
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    let dpi = if dpi == 0 { 96 } else { dpi };
    let focused = unsafe { GetFocus() } == hwnd;
    let mut rect = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut rect);
    }
    let w = rect.right - rect.left;
    let h = rect.bottom - rect.top;
    if w <= 8 || h <= 8 {
        return;
    }

    // Read current selected label so we can paint the entire closed combo face
    // cleanly with anti-aliased rounded corners (no Win98 1px square border!).
    let cur = unsafe {
        SendMessageW(
            hwnd,
            windows::Win32::UI::WindowsAndMessaging::CB_GETCURSEL,
            Some(WPARAM(0)),
            Some(LPARAM(0)),
        )
    }
    .0;
    let mut text = String::new();
    if cur >= 0 {
        let mut buf = [0u16; 256];
        let len = unsafe {
            SendMessageW(
                hwnd,
                CB_GETLBTEXT,
                Some(WPARAM(cur as usize)),
                Some(LPARAM(buf.as_mut_ptr() as isize)),
            )
        }
        .0
        .clamp(0, buf.len() as isize) as usize;
        text = String::from_utf16_lossy(&buf[..len]);
    }

    unsafe {
        let hdc = GetDC(Some(hwnd));
        if !hdc.0.is_null() {
            d2d::paint_buffered(hdc, rect, |c| {
                c.fill_rect(&rect, palette.card);
                let r = d2d::scaled_f(9.0, dpi);
                let fill = if focused {
                    palette.field_focus
                } else {
                    palette.field
                };
                let border = if focused {
                    palette.primary
                } else {
                    palette.border
                };
                let stroke = if focused {
                    d2d::scaled_f(1.5, dpi)
                } else {
                    1.0
                };
                c.fill_stroke_round(&rect, r, fill, border, stroke);

                let arrow_w = scaled(28, dpi);
                let text_rect = RECT {
                    left: rect.left + scaled(11, dpi),
                    top: rect.top,
                    right: (rect.right - arrow_w).max(rect.left + scaled(12, dpi)),
                    bottom: rect.bottom,
                };
                c.draw_text(
                    &text,
                    &text_rect,
                    TextStyle::Body,
                    dpi,
                    palette.heading,
                    AlignH::Left,
                    AlignV::Center,
                );
                c.icon(
                    Icon::ChevronDown,
                    (rect.right - arrow_w / 2) as f32,
                    (rect.top + rect.bottom) as f32 * 0.5,
                    d2d::scaled_f(11.0, dpi),
                    palette.secondary,
                );
            });
            let _ = ReleaseDC(Some(hwnd), hdc);
        }
    }
}

pub fn hovered(hwnd: HWND) -> bool {
    let key = hwnd.0 as isize;
    HOVERED.with(|set| set.borrow().contains(&key))
}

pub fn draw_button(
    draw: &DRAWITEMSTRUCT,
    palette: Palette,
    kind: ButtonKind,
    dpi: u32,
    background: COLORREF,
) {
    draw_button_with_icon(draw, palette, kind, None, dpi, background);
}

pub fn draw_button_with_icon(
    draw: &DRAWITEMSTRUCT,
    palette: Palette,
    kind: ButtonKind,
    icon: Option<Icon>,
    dpi: u32,
    background: COLORREF,
) {
    let rect = draw.rcItem;
    let height = rect.bottom - rect.top;
    let radius = round_radius(dpi, height);
    let pressed = draw.itemState.0 & ODS_SELECTED.0 != 0;
    let disabled = draw.itemState.0 & ODS_DISABLED.0 != 0;
    let focused = draw.itemState.0 & ODS_FOCUS.0 != 0;
    let hover = hovered(draw.hwndItem);

    let (fill, border, text_color) = match kind {
        ButtonKind::Primary => {
            let fill = if disabled {
                palette.border
            } else if pressed {
                palette.primary_active
            } else if hover || palette.dark {
                palette.primary_hover
            } else {
                palette.primary
            };
            let text = if disabled {
                palette.secondary
            } else {
                COLORREF(0xFFFFFF)
            };
            (fill, fill, text)
        }
        ButtonKind::Danger => {
            let fill = if disabled {
                palette.card
            } else {
                palette.danger_soft
            };
            let border = if disabled {
                palette.border
            } else if hover || pressed {
                palette.danger
            } else {
                palette.danger_soft
            };
            let text = if disabled {
                palette.secondary
            } else {
                palette.danger
            };
            (fill, border, text)
        }
        ButtonKind::Secondary => {
            let fill = if disabled {
                palette.page
            } else if pressed {
                palette.selection
            } else if hover {
                palette.hover
            } else {
                palette.card
            };
            let border = if hover && !disabled {
                palette.border_strong
            } else {
                palette.border
            };
            let text = if disabled {
                palette.secondary
            } else {
                palette.heading
            };
            (fill, border, text)
        }
        ButtonKind::Subtle => {
            let fill = if disabled {
                background
            } else if pressed {
                palette.selection
            } else {
                palette.primary_soft
            };
            let border = if hover && !disabled {
                palette.selection_border
            } else {
                fill
            };
            let text = if disabled {
                palette.secondary
            } else {
                palette.primary_text
            };
            (fill, border, text)
        }
    };

    let mut buffer = [0u16; 128];
    let length = unsafe { GetWindowTextW(draw.hwndItem, &mut buffer) };
    let caption = String::from_utf16_lossy(&buffer[..length.max(0) as usize]);

    d2d::paint_buffered(draw.hDC, rect, |c| {
        c.fill_rect(&rect, background);
        let stroke = if kind == ButtonKind::Primary {
            0.0
        } else {
            1.0
        };
        c.fill_stroke_round(&rect, radius, fill, border, stroke);

        if focused && !disabled {
            let ring = scaled(2, dpi);
            let inset = RECT {
                left: rect.left + ring,
                top: rect.top + ring,
                right: rect.right - ring,
                bottom: rect.bottom - ring,
            };
            let ring_color = if kind == ButtonKind::Primary {
                COLORREF(0xFFFFFF)
            } else {
                palette.primary
            };
            c.stroke_round(
                &inset,
                (radius - ring as f32).max(2.0),
                ring_color,
                d2d::scaled_f(1.5, dpi),
            );
        }

        let style = if kind == ButtonKind::Primary {
            TextStyle::Heading
        } else {
            TextStyle::Body
        };
        if let Some(ic) = icon {
            if caption.is_empty() {
                c.icon(
                    ic,
                    (rect.left + rect.right) as f32 * 0.5,
                    (rect.top + rect.bottom) as f32 * 0.5,
                    d2d::scaled_f(14.0, dpi),
                    text_color,
                );
            } else {
                let (tw, _) = c.measure_style(&caption, style, dpi);
                let icon_sz = d2d::scaled_f(13.0, dpi);
                let gap = d2d::scaled_f(6.0, dpi);
                let total = icon_sz + gap + tw;
                let start_x = (rect.left + rect.right) as f32 * 0.5 - total * 0.5;
                c.icon(
                    ic,
                    start_x + icon_sz * 0.5,
                    (rect.top + rect.bottom) as f32 * 0.5,
                    icon_sz,
                    text_color,
                );
                let text_rect = RECT {
                    left: (start_x + icon_sz + gap).round() as i32,
                    top: rect.top,
                    right: rect.right - scaled(4, dpi),
                    bottom: rect.bottom,
                };
                c.draw_text(
                    &caption,
                    &text_rect,
                    style,
                    dpi,
                    text_color,
                    AlignH::Left,
                    AlignV::Center,
                );
            }
        } else {
            let text_rect = RECT {
                left: rect.left + scaled(6, dpi),
                top: rect.top,
                right: rect.right - scaled(6, dpi),
                bottom: rect.bottom,
            };
            c.draw_text(
                &caption,
                &text_rect,
                style,
                dpi,
                text_color,
                AlignH::Center,
                AlignV::Center,
            );
        }
    });
}

/// Window height that makes a combo's drop-down just fit its items.
pub fn combo_height(hwnd: HWND, dpi: u32) -> i32 {
    let count = unsafe { SendMessageW(hwnd, CB_GETCOUNT, Some(WPARAM(0)), Some(LPARAM(0))) }.0;
    let rows = count.clamp(1, 8) as i32;
    scaled(34 + rows * 30 + 6, dpi)
}

/// Owner-draw a combo box row with anti-aliased Direct2D pill selection.
pub fn draw_combo_item(draw: &DRAWITEMSTRUCT, palette: Palette, dpi: u32, _font: HFONT) {
    let rect = draw.rcItem;
    let closed = draw.itemState.0 & ODS_COMBOBOXEDIT.0 != 0;
    let selected = draw.itemState.0 & ODS_SELECTED.0 != 0;
    let disabled = draw.itemState.0 & ODS_DISABLED.0 != 0;

    let mut text = String::new();
    if draw.itemID != u32::MAX {
        let mut buffer = [0u16; 256];
        let length = unsafe {
            SendMessageW(
                draw.hwndItem,
                CB_GETLBTEXT,
                Some(WPARAM(draw.itemID as usize)),
                Some(LPARAM(buffer.as_mut_ptr() as isize)),
            )
        }
        .0
        .clamp(0, buffer.len() as isize) as usize;
        text = String::from_utf16_lossy(&buffer[..length]);
    }

    d2d::paint_buffered(draw.hDC, rect, |c| {
        let bg = if closed { palette.field } else { palette.card };
        c.fill_rect(&rect, bg);
        if selected && !closed {
            let pill = RECT {
                left: rect.left + scaled(4, dpi),
                top: rect.top + scaled(2, dpi),
                right: rect.right - scaled(4, dpi),
                bottom: rect.bottom - scaled(2, dpi),
            };
            c.fill_round(&pill, d2d::scaled_f(7.0, dpi), palette.selection);
            let bar = RECT {
                left: pill.left + scaled(3, dpi),
                top: pill.top + scaled(5, dpi),
                right: pill.left + scaled(6, dpi),
                bottom: pill.bottom - scaled(5, dpi),
            };
            c.fill_round(&bar, d2d::scaled_f(2.0, dpi), palette.primary);
        }
        let text_rect = RECT {
            left: rect.left + scaled(12, dpi),
            top: rect.top,
            right: rect.right - scaled(8, dpi),
            bottom: rect.bottom,
        };
        let color = if disabled {
            palette.secondary
        } else if selected && !closed {
            palette.primary_text
        } else {
            palette.heading
        };
        c.draw_text(
            &text,
            &text_rect,
            if selected && !closed {
                TextStyle::Heading
            } else {
                TextStyle::Body
            },
            dpi,
            color,
            AlignH::Left,
            AlignV::Center,
        );
    });
}

/// Opt this process into the system's dark common-control mode.
pub fn set_dark_mode(dark: bool) {
    type SetPreferredAppMode = unsafe extern "system" fn(i32) -> i32;
    type FlushMenuThemes = unsafe extern "system" fn();
    if !dark_ordinals_supported() {
        return;
    }
    let module = uxtheme();
    if module.0.is_null() {
        return;
    }
    unsafe {
        if let Some(proc) = GetProcAddress(module, windows::core::PCSTR(135 as *const u8)) {
            let apply: SetPreferredAppMode = std::mem::transmute(proc);
            let _ = apply(if dark { 2 } else { 0 });
        }
        if let Some(proc) = GetProcAddress(module, windows::core::PCSTR(136 as *const u8)) {
            let flush: FlushMenuThemes = std::mem::transmute(proc);
            flush();
        }
    }
}

fn dark_ordinals_supported() -> bool {
    static SUPPORTED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *SUPPORTED.get_or_init(|| unsafe {
        type RtlGetVersion = unsafe extern "system" fn(*mut OSVERSIONINFOW) -> i32;
        let Ok(module) = LoadLibraryW(windows::core::w!("ntdll.dll")) else {
            return false;
        };
        let Some(proc) = GetProcAddress(module, windows::core::s!("RtlGetVersion")) else {
            return false;
        };
        let get_version: RtlGetVersion = std::mem::transmute(proc);
        let mut info = OSVERSIONINFOW {
            dwOSVersionInfoSize: std::mem::size_of::<OSVERSIONINFOW>() as u32,
            ..Default::default()
        };
        if get_version(&mut info) != 0 {
            return false;
        }
        (info.dwMajorVersion > 10) || (info.dwMajorVersion == 10 && info.dwBuildNumber >= 17763)
    })
}

fn uxtheme() -> HMODULE {
    static MODULE: std::sync::OnceLock<isize> = std::sync::OnceLock::new();
    let raw = *MODULE.get_or_init(|| unsafe {
        LoadLibraryW(windows::core::w!("uxtheme.dll"))
            .map(|module| module.0 as isize)
            .unwrap_or(0)
    });
    HMODULE(raw as *mut core::ffi::c_void)
}

pub fn theme_control(hwnd: HWND, dark: bool) {
    if hwnd.0.is_null() {
        return;
    }
    let dark = dark && dark_ordinals_supported();
    type AllowDarkModeForWindow =
        unsafe extern "system" fn(HWND, windows::core::BOOL) -> windows::core::BOOL;
    if dark_ordinals_supported() {
        let module = uxtheme();
        unsafe {
            if !module.0.is_null() {
                if let Some(proc) = GetProcAddress(module, windows::core::PCSTR(133 as *const u8)) {
                    let allow: AllowDarkModeForWindow = std::mem::transmute(proc);
                    let _ = allow(hwnd, windows::core::BOOL::from(dark));
                }
            }
        }
    }
    let name = if dark {
        windows::core::w!("DarkMode_Explorer")
    } else {
        windows::core::w!("Explorer")
    };
    let _ = unsafe { SetWindowTheme(hwnd, name, windows::core::PCWSTR::null()) };
}

pub fn theme_scrollbar(hwnd: HWND, dark: bool) {
    if hwnd.0.is_null() {
        return;
    }
    let dark = dark && dark_ordinals_supported();
    let name = if dark {
        windows::core::w!("DarkMode_Explorer")
    } else {
        windows::core::w!("Explorer")
    };
    let _ = unsafe { SetWindowTheme(hwnd, name, windows::core::w!("ScrollBar")) };
}

pub fn theme_combo(hwnd: HWND, dark: bool) {
    if hwnd.0.is_null() {
        return;
    }
    let dark = dark && dark_ordinals_supported();
    let name = if dark {
        windows::core::w!("DarkMode_CFD")
    } else {
        windows::core::w!("")
    };
    let _ = unsafe { SetWindowTheme(hwnd, name, windows::core::PCWSTR::null()) };
}

pub fn cue_banner(hwnd: HWND, text: &str) {
    if hwnd.0.is_null() {
        return;
    }
    let wide = crate::platform::wide(text);
    unsafe {
        let _ = SendMessageW(
            hwnd,
            EM_SETCUEBANNER,
            Some(WPARAM(1)),
            Some(LPARAM(wide.as_ptr() as isize)),
        );
    }
}

/// Apply Windows 11 DWM rounded corners, immersive dark mode, seamless caption
/// colour, crisp window border, and optional Mica backdrop (ignored on Win10).
pub fn apply_window_style(hwnd: HWND) {
    apply_window_style_for(hwnd, crate::theme::current(), true);
}

pub fn apply_popup_style(hwnd: HWND, palette: Palette) {
    apply_window_style_for(hwnd, palette, false);
}

pub fn apply_window_style_for(hwnd: HWND, palette: Palette, is_main_window: bool) {
    set_dark_mode(palette.dark);
    let dark = windows::core::BOOL::from(palette.dark);
    let _ = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &dark as *const _ as *const core::ffi::c_void,
            std::mem::size_of_val(&dark) as u32,
        )
    };
    let round = DWMWCP_ROUND;
    let _ = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &round as *const _ as *const core::ffi::c_void,
            std::mem::size_of_val(&round) as u32,
        )
    };
    let caption = palette.sidebar;
    let _ = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_CAPTION_COLOR,
            &caption as *const _ as *const core::ffi::c_void,
            std::mem::size_of_val(&caption) as u32,
        )
    };
    let title_text = palette.heading;
    let _ = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_TEXT_COLOR,
            &title_text as *const _ as *const core::ffi::c_void,
            std::mem::size_of_val(&title_text) as u32,
        )
    };
    let border = if palette.dark {
        palette.border_strong
    } else {
        palette.border
    };
    let _ = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_BORDER_COLOR,
            &border as *const _ as *const core::ffi::c_void,
            std::mem::size_of_val(&border) as u32,
        )
    };
    if is_main_window {
        // DWMSBT_MAINWINDOW (2) = Mica on Win11 22H2+; harmlessly ignored on older builds.
        let backdrop = DWM_SYSTEMBACKDROP_TYPE(2);
        let _ = unsafe {
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_SYSTEMBACKDROP_TYPE,
                &backdrop as *const _ as *const core::ffi::c_void,
                std::mem::size_of_val(&backdrop) as u32,
            )
        };
    }
}

/// Load the embedded QuickPhrase application icon (resource ID 1 from `app.rc`),
/// falling back to `IDI_APPLICATION` if unavailable.
pub fn load_app_icon(instance: HINSTANCE) -> HICON {
    unsafe {
        #[allow(clippy::manual_dangling_ptr)]
        let icon_id = PCWSTR(1 as *const u16);
        LoadIconW(Some(instance), icon_id).or_else(|_| LoadIconW(None, IDI_APPLICATION))
    }
    .unwrap_or_default()
}

/// Apply crisp small (title bar) and big (taskbar / Alt+Tab) app icons to a window.
pub fn apply_window_icon(hwnd: HWND, instance: HINSTANCE) {
    if hwnd.0.is_null() {
        return;
    }
    unsafe {
        #[allow(clippy::manual_dangling_ptr)]
        let icon_id = PCWSTR(1 as *const u16);
        let big_w = GetSystemMetrics(SM_CXICON).max(32);
        let big_h = GetSystemMetrics(SM_CYICON).max(32);
        let sm_w = GetSystemMetrics(SM_CXSMICON).max(16);
        let sm_h = GetSystemMetrics(SM_CYSMICON).max(16);
        let flags = LR_DEFAULTCOLOR | LR_SHARED;

        if let Ok(handle) = LoadImageW(Some(instance), icon_id, IMAGE_ICON, big_w, big_h, flags) {
            let _ = SendMessageW(
                hwnd,
                WM_SETICON,
                Some(WPARAM(ICON_BIG as usize)),
                Some(LPARAM(handle.0 as isize)),
            );
        }
        if let Ok(handle) = LoadImageW(Some(instance), icon_id, IMAGE_ICON, sm_w, sm_h, flags) {
            let _ = SendMessageW(
                hwnd,
                WM_SETICON,
                Some(WPARAM(ICON_SMALL as usize)),
                Some(LPARAM(handle.0 as isize)),
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Direct2D Fluent Pill Overlay Scrollbars (LISTBOX + Multiline EDIT)
// ---------------------------------------------------------------------------

const LB_GETCOUNT_MSG: u32 = 0x018B;
const LB_GETTOPINDEX_MSG: u32 = 0x018E;
const LB_SETTOPINDEX_MSG: u32 = 0x0197;
const LB_GETITEMHEIGHT_MSG: u32 = 0x01A1;
const LB_SETCURSEL_MSG: u32 = 0x0186;
const LB_RESETCONTENT_MSG: u32 = 0x0184;

const EM_GETLINECOUNT_MSG: u32 = 0x00BA;
const EM_GETFIRSTVISIBLELINE_MSG: u32 = 0x00CE;
const EM_LINESCROLL_MSG: u32 = 0x00B6;
const EM_SETMARGINS_MSG: u32 = 0x00D3;
const EC_RIGHTMARGIN_FLAG: usize = 0x0002;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ListSurface {
    Card,
    Sidebar,
}

#[derive(Clone, Copy)]
struct ScrollMetrics {
    max_top: i32,
    top: i32,
    track_top: i32,
    track_bottom: i32,
    thumb_top: i32,
    thumb_bottom: i32,
    hit_left: i32,
}

impl ScrollMetrics {
    fn compute(total: i32, visible: i32, top: i32, client: RECT, dpi: u32) -> Option<Self> {
        let visible = visible.max(1);
        let max_top = total - visible;
        if max_top <= 0 {
            return None;
        }
        let h = (client.bottom - client.top).max(1);
        let pad_y = scaled(5, dpi);
        let track_top = client.top + pad_y;
        let track_bottom = (client.bottom - pad_y).max(track_top + 1);
        let track_h = (track_bottom - track_top).max(1);
        let ratio = (visible as f32 / total.max(1) as f32).clamp(0.08, 1.0);
        let min_thumb = scaled(26, dpi).min(track_h);
        let thumb_h = ((track_h as f32 * ratio).round() as i32).clamp(min_thumb, track_h);
        let travel = (track_h - thumb_h).max(0);
        let top = top.clamp(0, max_top);
        let frac = top as f32 / max_top as f32;
        let thumb_top = track_top + (travel as f32 * frac).round() as i32;
        let thumb_bottom = (thumb_top + thumb_h).min(track_bottom);
        let hit_left = (client.right - scaled(13, dpi)).max(client.left);
        if h <= pad_y * 2 {
            return None;
        }
        Some(Self {
            max_top,
            top,
            track_top,
            track_bottom,
            thumb_top,
            thumb_bottom,
            hit_left,
        })
    }

    fn top_for_thumb_y(&self, thumb_y: i32) -> i32 {
        let thumb_h = (self.thumb_bottom - self.thumb_top).max(1);
        let track_h = (self.track_bottom - self.track_top).max(1);
        let travel = (track_h - thumb_h).max(1);
        let clamped = (thumb_y - self.track_top).clamp(0, travel);
        let frac = clamped as f32 / travel as f32;
        (frac * self.max_top as f32).round() as i32
    }
}

struct SmoothListState {
    old_proc: isize,
    surface: ListSurface,
    hovered: bool,
    dragging: bool,
    drag_offset: i32,
    wheel_acc: i32,
}

struct SmoothEditState {
    old_proc: isize,
    hovered: bool,
    dragging: bool,
    drag_offset: i32,
    wheel_acc: i32,
}

thread_local! {
    static LIST_STATES: RefCell<HashMap<isize, SmoothListState>> = RefCell::new(HashMap::new());
    static EDIT_STATES: RefCell<HashMap<isize, SmoothEditState>> = RefCell::new(HashMap::new());
}

fn listbox_metrics(hwnd: HWND) -> Option<(RECT, u32, ScrollMetrics)> {
    let mut client = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut client);
    }
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    let dpi = if dpi == 0 { 96 } else { dpi };
    let count =
        unsafe { SendMessageW(hwnd, LB_GETCOUNT_MSG, Some(WPARAM(0)), Some(LPARAM(0))) }.0 as i32;
    let item_h =
        unsafe { SendMessageW(hwnd, LB_GETITEMHEIGHT_MSG, Some(WPARAM(0)), Some(LPARAM(0))) }.0
            as i32;
    if count <= 0 || item_h <= 0 {
        return None;
    }
    let visible = ((client.bottom - client.top) / item_h).max(1);
    let top = unsafe { SendMessageW(hwnd, LB_GETTOPINDEX_MSG, Some(WPARAM(0)), Some(LPARAM(0))) }.0
        as i32;
    let m = ScrollMetrics::compute(count, visible, top, client, dpi)?;
    Some((client, dpi, m))
}

/// Returns true when the given `LISTBOX` currently has more rows than fit in its viewport.
pub fn listbox_scrollable(hwnd: HWND) -> bool {
    listbox_metrics(hwnd).is_some()
}

fn paint_pill_scrollbar(
    hdc: HDC,
    client: RECT,
    dpi: u32,
    bg: COLORREF,
    metrics: Option<ScrollMetrics>,
    hovered: bool,
    dragging: bool,
) {
    let strip_w = scaled(10, dpi);
    if client.right - client.left <= strip_w || client.bottom - client.top <= 4 {
        return;
    }
    let strip = RECT {
        left: client.right - strip_w,
        top: client.top,
        right: client.right,
        bottom: client.bottom,
    };
    let palette = crate::theme::current();
    d2d::paint_buffered(hdc, strip, |c| {
        c.fill_rect(&strip, bg);
        let Some(m) = metrics else {
            return;
        };
        let active = hovered || dragging;
        if active {
            let track_w = scaled(6, dpi).max(4);
            let track_r = client.right - scaled(2, dpi);
            let track = RECT {
                left: track_r - track_w,
                top: m.track_top,
                right: track_r,
                bottom: m.track_bottom,
            };
            let track_bg = if bg == palette.field {
                palette.hover
            } else {
                palette.field
            };
            c.fill_round(&track, track_w as f32 * 0.5, track_bg);
        }

        let thumb_w = if active {
            scaled(6, dpi).max(4)
        } else {
            scaled(4, dpi).max(3)
        };
        let thumb_r = client.right
            - if active {
                scaled(2, dpi)
            } else {
                scaled(3, dpi)
            };
        let thumb = RECT {
            left: thumb_r - thumb_w,
            top: m.thumb_top,
            right: thumb_r,
            bottom: m.thumb_bottom,
        };
        let thumb_color = if dragging {
            palette.primary
        } else if hovered {
            palette.muted
        } else {
            palette.border_strong
        };
        c.fill_round(&thumb, thumb_w as f32 * 0.5, thumb_color);
    });
}

fn paint_listbox_scrollbar_on(hwnd: HWND, target_hdc: Option<HDC>) {
    let key = hwnd.0 as isize;
    let state = LIST_STATES.with(|map| {
        map.borrow()
            .get(&key)
            .map(|s| (s.surface, s.hovered, s.dragging))
    });
    let Some((surface, hovered, dragging)) = state else {
        return;
    };
    let mut client = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut client);
    }
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    let dpi = if dpi == 0 { 96 } else { dpi };
    let palette = crate::theme::current();
    let bg = match surface {
        ListSurface::Card => palette.card,
        ListSurface::Sidebar => palette.sidebar,
    };
    let metrics = listbox_metrics(hwnd).map(|(_, _, m)| m);

    if let Some(hdc) = target_hdc {
        if !hdc.0.is_null() {
            paint_pill_scrollbar(hdc, client, dpi, bg, metrics, hovered, dragging);
        }
    } else {
        unsafe {
            let hdc = GetDC(Some(hwnd));
            if !hdc.0.is_null() {
                paint_pill_scrollbar(hdc, client, dpi, bg, metrics, hovered, dragging);
                let _ = ReleaseDC(Some(hwnd), hdc);
            }
        }
    }
}

/// Subclass an owner-draw `LISTBOX` (created without `WS_VSCROLL`) to provide
/// smooth mouse-wheel scrolling and a Direct2D Fluent pill overlay scrollbar.
pub fn install_smooth_listbox(hwnd: HWND, surface: ListSurface) {
    if hwnd.0.is_null() {
        return;
    }
    let key = hwnd.0 as isize;
    if LIST_STATES.with(|map| map.borrow().contains_key(&key)) {
        return;
    }
    unsafe {
        let old = GetWindowLongPtrW(hwnd, GWLP_WNDPROC);
        if old == 0 {
            return;
        }
        LIST_STATES.with(|map| {
            map.borrow_mut().insert(
                key,
                SmoothListState {
                    old_proc: old,
                    surface,
                    hovered: false,
                    dragging: false,
                    drag_offset: 0,
                    wheel_acc: 0,
                },
            );
        });
        let previous = SetWindowLongPtrW(
            hwnd,
            GWLP_WNDPROC,
            smooth_listbox_proc as *const () as usize as isize,
        );
        if previous != old {
            LIST_STATES.with(|map| {
                map.borrow_mut().remove(&key);
            });
        }
    }
}

unsafe extern "system" fn smooth_listbox_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let key = hwnd.0 as isize;
    let old = LIST_STATES
        .with(|map| map.borrow().get(&key).map(|s| s.old_proc))
        .unwrap_or(0);
    if old == 0 {
        return DefWindowProcW(hwnd, message, wparam, lparam);
    }
    let proc: unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT =
        std::mem::transmute(old);

    match message {
        WM_MOUSEWHEEL => {
            if let Some((_, _, m)) = listbox_metrics(hwnd) {
                let delta = ((wparam.0 >> 16) as u16 as i16) as i32;
                let mut steps = 0i32;
                LIST_STATES.with(|map| {
                    if let Some(s) = map.borrow_mut().get_mut(&key) {
                        s.wheel_acc += delta;
                        while s.wheel_acc >= 120 {
                            steps -= 1;
                            s.wheel_acc -= 120;
                        }
                        while s.wheel_acc <= -120 {
                            steps += 1;
                            s.wheel_acc += 120;
                        }
                    }
                });
                if steps != 0 {
                    let next = (m.top + steps).clamp(0, m.max_top);
                    if next != m.top {
                        let _ = SendMessageW(
                            hwnd,
                            LB_SETTOPINDEX_MSG,
                            Some(WPARAM(next as usize)),
                            Some(LPARAM(0)),
                        );
                        paint_listbox_scrollbar_on(hwnd, None);
                    }
                }
            }
            return LRESULT(0);
        }
        WM_LBUTTONDOWN => {
            let x = (lparam.0 & 0xffff) as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xffff) as i16 as i32;
            if let Some((_, _, m)) = listbox_metrics(hwnd) {
                if x >= m.hit_left {
                    let thumb_h = (m.thumb_bottom - m.thumb_top).max(1);
                    let drag_offset = if y >= m.thumb_top && y <= m.thumb_bottom {
                        y - m.thumb_top
                    } else {
                        let next = m.top_for_thumb_y(y - thumb_h / 2);
                        let _ = SendMessageW(
                            hwnd,
                            LB_SETTOPINDEX_MSG,
                            Some(WPARAM(next as usize)),
                            Some(LPARAM(0)),
                        );
                        thumb_h / 2
                    };
                    LIST_STATES.with(|map| {
                        if let Some(s) = map.borrow_mut().get_mut(&key) {
                            s.dragging = true;
                            s.hovered = true;
                            s.drag_offset = drag_offset;
                        }
                    });
                    let _ = SetCapture(hwnd);
                    paint_listbox_scrollbar_on(hwnd, None);
                    return LRESULT(0);
                }
            }
        }
        WM_LBUTTONDBLCLK => {
            let x = (lparam.0 & 0xffff) as i16 as i32;
            if let Some((_, _, m)) = listbox_metrics(hwnd) {
                if x >= m.hit_left {
                    return LRESULT(0);
                }
            }
        }
        WM_MOUSEMOVE => {
            let x = (lparam.0 & 0xffff) as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xffff) as i16 as i32;
            let dragging = LIST_STATES.with(|map| {
                map.borrow()
                    .get(&key)
                    .map(|s| (s.dragging, s.drag_offset))
                    .unwrap_or((false, 0))
            });
            if dragging.0 {
                if let Some((_, _, m)) = listbox_metrics(hwnd) {
                    let next = m.top_for_thumb_y(y - dragging.1);
                    if next != m.top {
                        let _ = SendMessageW(
                            hwnd,
                            LB_SETTOPINDEX_MSG,
                            Some(WPARAM(next as usize)),
                            Some(LPARAM(0)),
                        );
                    }
                    paint_listbox_scrollbar_on(hwnd, None);
                }
                return LRESULT(0);
            }
            let over = listbox_metrics(hwnd)
                .map(|(_, _, m)| x >= m.hit_left)
                .unwrap_or(false);
            let changed = LIST_STATES.with(|map| {
                if let Some(s) = map.borrow_mut().get_mut(&key) {
                    if s.hovered != over {
                        s.hovered = over;
                        return true;
                    }
                }
                false
            });
            if over {
                let mut track = TRACKMOUSEEVENT {
                    cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                let _ = TrackMouseEvent(&mut track);
            }
            if changed {
                paint_listbox_scrollbar_on(hwnd, None);
            }
        }
        WM_MOUSELEAVE => {
            let changed = LIST_STATES.with(|map| {
                if let Some(s) = map.borrow_mut().get_mut(&key) {
                    if s.hovered {
                        s.hovered = false;
                        return true;
                    }
                }
                false
            });
            if changed {
                paint_listbox_scrollbar_on(hwnd, None);
            }
        }
        WM_LBUTTONUP => {
            let was_dragging = LIST_STATES.with(|map| {
                map.borrow_mut()
                    .get_mut(&key)
                    .map(|s| std::mem::replace(&mut s.dragging, false))
                    .unwrap_or(false)
            });
            if was_dragging {
                let _ = ReleaseCapture();
                paint_listbox_scrollbar_on(hwnd, None);
                return LRESULT(0);
            }
        }
        WM_CAPTURECHANGED => {
            let was_dragging = LIST_STATES.with(|map| {
                map.borrow_mut()
                    .get_mut(&key)
                    .map(|s| std::mem::replace(&mut s.dragging, false))
                    .unwrap_or(false)
            });
            if was_dragging {
                paint_listbox_scrollbar_on(hwnd, None);
            }
        }
        _ => {}
    }

    let result = CallWindowProcW(Some(proc), hwnd, message, wparam, lparam);

    match message {
        WM_PAINT => paint_listbox_scrollbar_on(hwnd, None),
        WM_PRINTCLIENT => {
            paint_listbox_scrollbar_on(hwnd, Some(HDC(wparam.0 as *mut core::ffi::c_void)))
        }
        LB_SETTOPINDEX_MSG | LB_SETCURSEL_MSG | LB_RESETCONTENT_MSG | WM_VSCROLL | WM_KEYDOWN
        | WM_SIZE => {
            paint_listbox_scrollbar_on(hwnd, None);
        }
        WM_NCDESTROY => {
            LIST_STATES.with(|map| {
                map.borrow_mut().remove(&key);
            });
        }
        _ => {}
    }
    result
}

// --- Multiline EDIT smooth mouse-wheel & Fluent pill scrollbar ---

fn edit_metrics(hwnd: HWND) -> Option<(RECT, u32, ScrollMetrics)> {
    let mut client = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut client);
    }
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    let dpi = if dpi == 0 { 96 } else { dpi };
    let line_count =
        unsafe { SendMessageW(hwnd, EM_GETLINECOUNT_MSG, Some(WPARAM(0)), Some(LPARAM(0))) }.0
            as i32;
    if line_count <= 1 {
        return None;
    }
    let font_raw = unsafe { SendMessageW(hwnd, WM_GETFONT, Some(WPARAM(0)), Some(LPARAM(0))) }.0;
    let line_h = font_line_height(HFONT(font_raw as *mut core::ffi::c_void)).max(12);
    let visible = ((client.bottom - client.top) / line_h).max(1);
    let first = unsafe {
        SendMessageW(
            hwnd,
            EM_GETFIRSTVISIBLELINE_MSG,
            Some(WPARAM(0)),
            Some(LPARAM(0)),
        )
    }
    .0 as i32;
    let m = ScrollMetrics::compute(line_count, visible, first, client, dpi)?;
    Some((client, dpi, m))
}

fn paint_edit_scrollbar_on(hwnd: HWND, target_hdc: Option<HDC>) {
    let key = hwnd.0 as isize;
    let state = EDIT_STATES.with(|map| map.borrow().get(&key).map(|s| (s.hovered, s.dragging)));
    let Some((hovered, dragging)) = state else {
        return;
    };
    let mut client = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut client);
    }
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    let dpi = if dpi == 0 { 96 } else { dpi };
    let palette = crate::theme::current();
    let metrics = edit_metrics(hwnd).map(|(_, _, m)| m);

    if let Some(hdc) = target_hdc {
        if !hdc.0.is_null() {
            paint_pill_scrollbar(hdc, client, dpi, palette.field, metrics, hovered, dragging);
        }
    } else {
        unsafe {
            let hdc = GetDC(Some(hwnd));
            if !hdc.0.is_null() {
                paint_pill_scrollbar(hdc, client, dpi, palette.field, metrics, hovered, dragging);
                let _ = ReleaseDC(Some(hwnd), hdc);
            }
        }
    }
}

fn apply_edit_right_margin(hwnd: HWND) {
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    let dpi = if dpi == 0 { 96 } else { dpi };
    let right_margin = scaled(12, dpi).clamp(0, 0xFFFF) as usize;
    unsafe {
        let _ = SendMessageW(
            hwnd,
            EM_SETMARGINS_MSG,
            Some(WPARAM(EC_RIGHTMARGIN_FLAG)),
            Some(LPARAM((right_margin << 16) as isize)),
        );
    }
}

/// Subclass a multiline `EDIT` (created without `WS_VSCROLL`) to support mouse-wheel
/// scrolling and a matching Direct2D Fluent pill overlay scrollbar when text overflows.
pub fn install_smooth_edit(hwnd: HWND) {
    if hwnd.0.is_null() {
        return;
    }
    let key = hwnd.0 as isize;
    if EDIT_STATES.with(|map| map.borrow().contains_key(&key)) {
        return;
    }
    unsafe {
        let old = GetWindowLongPtrW(hwnd, GWLP_WNDPROC);
        if old == 0 {
            return;
        }
        EDIT_STATES.with(|map| {
            map.borrow_mut().insert(
                key,
                SmoothEditState {
                    old_proc: old,
                    hovered: false,
                    dragging: false,
                    drag_offset: 0,
                    wheel_acc: 0,
                },
            );
        });
        let previous = SetWindowLongPtrW(
            hwnd,
            GWLP_WNDPROC,
            smooth_edit_proc as *const () as usize as isize,
        );
        if previous != old {
            EDIT_STATES.with(|map| {
                map.borrow_mut().remove(&key);
            });
            return;
        }
    }
    apply_edit_right_margin(hwnd);
}

unsafe extern "system" fn smooth_edit_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let key = hwnd.0 as isize;
    let old = EDIT_STATES
        .with(|map| map.borrow().get(&key).map(|s| s.old_proc))
        .unwrap_or(0);
    if old == 0 {
        return DefWindowProcW(hwnd, message, wparam, lparam);
    }
    let proc: unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT =
        std::mem::transmute(old);

    match message {
        WM_MOUSEWHEEL => {
            if edit_metrics(hwnd).is_some() {
                let delta = ((wparam.0 >> 16) as u16 as i16) as i32;
                let mut lines = 0i32;
                EDIT_STATES.with(|map| {
                    if let Some(s) = map.borrow_mut().get_mut(&key) {
                        s.wheel_acc += delta;
                        while s.wheel_acc >= 120 {
                            lines -= 3;
                            s.wheel_acc -= 120;
                        }
                        while s.wheel_acc <= -120 {
                            lines += 3;
                            s.wheel_acc += 120;
                        }
                    }
                });
                if lines != 0 {
                    let _ = SendMessageW(
                        hwnd,
                        EM_LINESCROLL_MSG,
                        Some(WPARAM(0)),
                        Some(LPARAM(lines as isize)),
                    );
                    paint_edit_scrollbar_on(hwnd, None);
                }
            }
            return LRESULT(0);
        }
        WM_LBUTTONDOWN => {
            let x = (lparam.0 & 0xffff) as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xffff) as i16 as i32;
            if let Some((_, _, m)) = edit_metrics(hwnd) {
                if x >= m.hit_left {
                    let thumb_h = (m.thumb_bottom - m.thumb_top).max(1);
                    let drag_offset = if y >= m.thumb_top && y <= m.thumb_bottom {
                        y - m.thumb_top
                    } else {
                        let next = m.top_for_thumb_y(y - thumb_h / 2);
                        let delta = next - m.top;
                        if delta != 0 {
                            let _ = SendMessageW(
                                hwnd,
                                EM_LINESCROLL_MSG,
                                Some(WPARAM(0)),
                                Some(LPARAM(delta as isize)),
                            );
                        }
                        thumb_h / 2
                    };
                    EDIT_STATES.with(|map| {
                        if let Some(s) = map.borrow_mut().get_mut(&key) {
                            s.dragging = true;
                            s.hovered = true;
                            s.drag_offset = drag_offset;
                        }
                    });
                    let _ = SetCapture(hwnd);
                    paint_edit_scrollbar_on(hwnd, None);
                    return LRESULT(0);
                }
            }
        }
        WM_MOUSEMOVE => {
            let x = (lparam.0 & 0xffff) as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xffff) as i16 as i32;
            let dragging = EDIT_STATES.with(|map| {
                map.borrow()
                    .get(&key)
                    .map(|s| (s.dragging, s.drag_offset))
                    .unwrap_or((false, 0))
            });
            if dragging.0 {
                if let Some((_, _, m)) = edit_metrics(hwnd) {
                    let next = m.top_for_thumb_y(y - dragging.1);
                    let delta = next - m.top;
                    if delta != 0 {
                        let _ = SendMessageW(
                            hwnd,
                            EM_LINESCROLL_MSG,
                            Some(WPARAM(0)),
                            Some(LPARAM(delta as isize)),
                        );
                    }
                    paint_edit_scrollbar_on(hwnd, None);
                }
                return LRESULT(0);
            }
            let over = edit_metrics(hwnd)
                .map(|(_, _, m)| x >= m.hit_left)
                .unwrap_or(false);
            let changed = EDIT_STATES.with(|map| {
                if let Some(s) = map.borrow_mut().get_mut(&key) {
                    if s.hovered != over {
                        s.hovered = over;
                        return true;
                    }
                }
                false
            });
            if over {
                let mut track = TRACKMOUSEEVENT {
                    cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                let _ = TrackMouseEvent(&mut track);
            }
            if changed {
                paint_edit_scrollbar_on(hwnd, None);
            }
        }
        WM_MOUSELEAVE => {
            let changed = EDIT_STATES.with(|map| {
                if let Some(s) = map.borrow_mut().get_mut(&key) {
                    if s.hovered {
                        s.hovered = false;
                        return true;
                    }
                }
                false
            });
            if changed {
                paint_edit_scrollbar_on(hwnd, None);
            }
        }
        WM_LBUTTONUP => {
            let was_dragging = EDIT_STATES.with(|map| {
                map.borrow_mut()
                    .get_mut(&key)
                    .map(|s| std::mem::replace(&mut s.dragging, false))
                    .unwrap_or(false)
            });
            if was_dragging {
                let _ = ReleaseCapture();
                paint_edit_scrollbar_on(hwnd, None);
                return LRESULT(0);
            }
        }
        WM_CAPTURECHANGED => {
            let was_dragging = EDIT_STATES.with(|map| {
                map.borrow_mut()
                    .get_mut(&key)
                    .map(|s| std::mem::replace(&mut s.dragging, false))
                    .unwrap_or(false)
            });
            if was_dragging {
                paint_edit_scrollbar_on(hwnd, None);
            }
        }
        _ => {}
    }

    let result = CallWindowProcW(Some(proc), hwnd, message, wparam, lparam);

    match message {
        WM_PAINT => paint_edit_scrollbar_on(hwnd, None),
        WM_PRINTCLIENT => {
            paint_edit_scrollbar_on(hwnd, Some(HDC(wparam.0 as *mut core::ffi::c_void)))
        }
        WM_SIZE => {
            apply_edit_right_margin(hwnd);
            paint_edit_scrollbar_on(hwnd, None);
        }
        WM_VSCROLL | WM_CHAR => {
            paint_edit_scrollbar_on(hwnd, None);
        }
        WM_KEYUP => {
            if matches!(wparam.0 as u32, 0x23..=0x28) {
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            paint_edit_scrollbar_on(hwnd, None);
        }
        WM_SETFOCUS | WM_KILLFOCUS | WM_ENABLE => {
            let _ = InvalidateRect(Some(hwnd), None, false);
            paint_edit_scrollbar_on(hwnd, None);
        }
        WM_NCDESTROY => {
            EDIT_STATES.with(|map| {
                map.borrow_mut().remove(&key);
            });
        }
        _ => {}
    }
    result
}
