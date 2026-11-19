//! Shared drawing helpers: rounded surfaces, owner-draw buttons and DWM chrome.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use windows::Win32::Foundation::{COLORREF, HMODULE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE,
    DWMWCP_ROUND,
};
use windows::Win32::Graphics::Gdi::{
    CreatePen, CreateSolidBrush, DeleteObject, DrawTextW, FillRect, GetDC, GetStockObject,
    InvalidateRect, LineTo, MoveToEx, ReleaseDC, RoundRect, SelectObject, SetBkMode, SetTextColor,
    DRAW_TEXT_FORMAT, DT_CENTER, DT_END_ELLIPSIS, DT_LEFT, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER,
    HBRUSH, HDC, HFONT, HGDIOBJ, NULL_BRUSH, NULL_PEN, PS_SOLID, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::UI::Controls::{
    SetWindowTheme, DRAWITEMSTRUCT, EM_SETCUEBANNER, ODS_COMBOBOXEDIT, ODS_DISABLED, ODS_SELECTED,
    WM_MOUSELEAVE,
};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT};
use windows::Win32::UI::WindowsAndMessaging::{
    CallWindowProcW, DefWindowProcW, GetClientRect, GetWindowLongPtrW, GetWindowTextW,
    SendMessageW, SetWindowLongPtrW, CB_GETCOUNT, CB_GETLBTEXT, GWLP_WNDPROC, WM_GETFONT,
    WM_MOUSEMOVE, WM_NCDESTROY, WM_PAINT,
};

use crate::theme::Palette;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ButtonKind {
    Primary,
    Secondary,
    Danger,
}

thread_local! {
    static OLD_PROCS: RefCell<HashMap<isize, isize>> = RefCell::new(HashMap::new());
    static COMBO_PROCS: RefCell<HashMap<isize, isize>> = RefCell::new(HashMap::new());
    static HOVERED: RefCell<HashSet<isize>> = RefCell::new(HashSet::new());
}

pub fn scaled(value: i32, dpi: u32) -> i32 {
    (value * dpi as i32 + 48) / 96
}

fn round_radius(dpi: u32, height: i32) -> i32 {
    scaled(9, dpi).min((height / 3).max(3))
}

pub fn fill_round(hdc: HDC, rect: &RECT, radius: i32, brush: HBRUSH) {
    unsafe {
        let old_brush = SelectObject(hdc, HGDIOBJ(brush.0));
        let old_pen = SelectObject(hdc, GetStockObject(NULL_PEN));
        let _ = RoundRect(
            hdc,
            rect.left,
            rect.top,
            rect.right,
            rect.bottom,
            radius * 2,
            radius * 2,
        );
        let _ = SelectObject(hdc, old_pen);
        let _ = SelectObject(hdc, old_brush);
    }
}

pub fn stroke_round(hdc: HDC, rect: &RECT, radius: i32, color: COLORREF, width: i32) {
    unsafe {
        let pen = CreatePen(PS_SOLID, width.max(1), color);
        let old_pen = SelectObject(hdc, HGDIOBJ(pen.0));
        let old_brush = SelectObject(hdc, GetStockObject(NULL_BRUSH));
        let _ = RoundRect(
            hdc,
            rect.left,
            rect.top,
            rect.right,
            rect.bottom,
            radius * 2,
            radius * 2,
        );
        let _ = SelectObject(hdc, old_brush);
        let _ = SelectObject(hdc, old_pen);
        let _ = DeleteObject(HGDIOBJ(pen.0));
    }
}

pub fn card(hdc: HDC, rect: &RECT, palette: Palette, dpi: u32) {
    let radius = scaled(12, dpi);
    let brush = unsafe { CreateSolidBrush(palette.card) };
    fill_round(hdc, rect, radius, brush);
    unsafe {
        let _ = DeleteObject(HGDIOBJ(brush.0));
    }
    stroke_round(hdc, rect, radius, palette.border, 1);
}

pub fn label(
    hdc: HDC,
    text: &str,
    rect: &mut RECT,
    font: HFONT,
    color: COLORREF,
    flags: DRAW_TEXT_FORMAT,
) {
    let mut wide = crate::platform::wide(text);
    unsafe {
        let old = SelectObject(hdc, HGDIOBJ(font.0));
        SetBkMode(hdc, TRANSPARENT);
        SetTextColor(hdc, color);
        DrawTextW(hdc, &mut wide, rect, flags);
        let _ = SelectObject(hdc, old);
    }
}

/// Subclass a button so hover can be drawn. Safe to call more than once.
pub fn install_button(hwnd: HWND) {
    if hwnd.0.is_null() {
        return;
    }
    let key = hwnd.0 as isize;
    let already = OLD_PROCS.with(|map| map.borrow().contains_key(&key));
    if already {
        return;
    }
    unsafe {
        let old = GetWindowLongPtrW(hwnd, GWLP_WNDPROC);
        if old == 0 {
            return;
        }
        OLD_PROCS.with(|map| map.borrow_mut().insert(key, old));
        SetWindowLongPtrW(
            hwnd,
            GWLP_WNDPROC,
            button_proc as *const () as usize as isize,
        );
    }
}

unsafe extern "system" fn button_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let key = hwnd.0 as isize;
    match message {
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
            }
        }
        WM_MOUSELEAVE => {
            HOVERED.with(|set| {
                set.borrow_mut().remove(&key);
            });
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
        WM_NCDESTROY => {
            HOVERED.with(|set| {
                set.borrow_mut().remove(&key);
            });
            OLD_PROCS.with(|map| {
                map.borrow_mut().remove(&key);
            });
        }
        _ => {}
    }

    let old = OLD_PROCS
        .with(|map| map.borrow().get(&key).copied())
        .unwrap_or(0);
    if old == 0 {
        return DefWindowProcW(hwnd, message, wparam, lparam);
    }
    let proc: unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT =
        std::mem::transmute(old);
    CallWindowProcW(Some(proc), hwnd, message, wparam, lparam)
}

/// Subclass a combo box so its drop-down button follows the theme instead of
/// keeping the native light grey square in dark mode.
pub fn install_combo(hwnd: HWND) {
    if hwnd.0.is_null() {
        return;
    }
    let key = hwnd.0 as isize;
    let already = COMBO_PROCS.with(|map| map.borrow().contains_key(&key));
    if already {
        return;
    }
    unsafe {
        let old = GetWindowLongPtrW(hwnd, GWLP_WNDPROC);
        if old == 0 {
            return;
        }
        COMBO_PROCS.with(|map| map.borrow_mut().insert(key, old));
        SetWindowLongPtrW(
            hwnd,
            GWLP_WNDPROC,
            combo_proc as *const () as usize as isize,
        );
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
        paint_combo_arrow(hwnd);
    }

    if message == WM_NCDESTROY {
        COMBO_PROCS.with(|map| {
            map.borrow_mut().remove(&key);
        });
    }
    result
}

fn paint_combo_arrow(hwnd: HWND) {
    let palette = crate::theme::current();
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    let dpi = if dpi == 0 { 96 } else { dpi };
    let arrow = scaled(22, dpi);
    let mut rect = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut rect);
    }
    if rect.right - rect.left <= arrow + scaled(8, dpi) {
        return;
    }
    unsafe {
        let hdc = GetDC(Some(hwnd));
        let strip = RECT {
            left: rect.right - arrow,
            top: rect.top + 1,
            right: rect.right - 1,
            bottom: rect.bottom - 1,
        };
        let brush = CreateSolidBrush(palette.field);
        FillRect(hdc, &strip, brush);
        let _ = DeleteObject(HGDIOBJ(brush.0));

        let pen = CreatePen(PS_SOLID, 1, palette.secondary);
        let old_pen = SelectObject(hdc, HGDIOBJ(pen.0));
        let cx = rect.right - arrow / 2;
        let cy = (rect.top + rect.bottom) / 2;
        let half = scaled(4, dpi);
        let _ = MoveToEx(hdc, cx - half, cy - scaled(2, dpi), None);
        let _ = LineTo(hdc, cx, cy + scaled(2, dpi));
        let _ = LineTo(hdc, cx + half, cy - scaled(2, dpi));
        let _ = SelectObject(hdc, old_pen);
        let _ = DeleteObject(HGDIOBJ(pen.0));
        let _ = ReleaseDC(Some(hwnd), hdc);
    }
}

fn hovered(hwnd: HWND) -> bool {
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
    let hdc = draw.hDC;
    let rect = draw.rcItem;
    // Clear the native button face so its light corners do not show around
    // the rounded fill in dark mode.
    unsafe {
        let brush = CreateSolidBrush(background);
        FillRect(hdc, &rect, brush);
        let _ = DeleteObject(HGDIOBJ(brush.0));
    }
    let height = rect.bottom - rect.top;
    let radius = round_radius(dpi, height);
    let pressed = draw.itemState.0 & ODS_SELECTED.0 != 0;
    let disabled = draw.itemState.0 & ODS_DISABLED.0 != 0;
    let hover = hovered(draw.hwndItem);

    let (fill, border, text) = match kind {
        ButtonKind::Primary => {
            let fill = if disabled {
                palette.border
            } else if pressed || hover {
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
            } else if pressed || hover {
                palette.selection
            } else {
                palette.card
            };
            let border = if disabled {
                palette.border
            } else {
                palette.error
            };
            let text = if disabled {
                palette.secondary
            } else {
                palette.error
            };
            (fill, border, text)
        }
        ButtonKind::Secondary => {
            let fill = if disabled {
                palette.page
            } else if pressed || hover {
                palette.selection
            } else {
                palette.card
            };
            let border = palette.border;
            let text = if disabled {
                palette.secondary
            } else {
                palette.heading
            };
            (fill, border, text)
        }
    };

    let brush = unsafe { CreateSolidBrush(fill) };
    fill_round(hdc, &rect, radius, brush);
    unsafe {
        let _ = DeleteObject(HGDIOBJ(brush.0));
    }
    if kind != ButtonKind::Primary {
        let mut inset = rect;
        inset.left += 1;
        inset.top += 1;
        inset.right -= 1;
        inset.bottom -= 1;
        stroke_round(hdc, &inset, radius, border, 1);
    }

    let mut buffer = [0u16; 128];
    let length = unsafe { GetWindowTextW(draw.hwndItem, &mut buffer) };
    let font = unsafe { SendMessageW(draw.hwndItem, WM_GETFONT, Some(WPARAM(0)), Some(LPARAM(0))) };
    unsafe {
        let old = SelectObject(hdc, HGDIOBJ(font.0 as *mut core::ffi::c_void));
        SetBkMode(hdc, TRANSPARENT);
        SetTextColor(hdc, text);
        let mut label = RECT {
            left: rect.left + 4,
            right: rect.right - 4,
            ..rect
        };
        DrawTextW(
            hdc,
            &mut buffer[..length.max(0) as usize],
            &mut label,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
        );
        let _ = SelectObject(hdc, old);
    }
}

/// Window height that makes a combo's drop-down just fit its items instead of
/// leaving a light empty strip below them.
pub fn combo_height(hwnd: HWND, dpi: u32) -> i32 {
    let count = unsafe { SendMessageW(hwnd, CB_GETCOUNT, Some(WPARAM(0)), Some(LPARAM(0))) }.0;
    let rows = count.clamp(1, 8) as i32;
    scaled(31 + rows * 28 + 4, dpi)
}

/// Owner-draw a combo box row (closed display or dropped list item).
pub fn draw_combo_item(draw: &DRAWITEMSTRUCT, palette: Palette, dpi: u32, font: HFONT) {
    let hdc = draw.hDC;
    let rect = draw.rcItem;
    let closed = draw.itemState.0 & ODS_COMBOBOXEDIT.0 != 0;
    let selected = draw.itemState.0 & ODS_SELECTED.0 != 0;
    let disabled = draw.itemState.0 & ODS_DISABLED.0 != 0;
    let fill = if selected && !closed {
        palette.selection
    } else {
        palette.field
    };
    unsafe {
        let brush = CreateSolidBrush(fill);
        FillRect(hdc, &rect, brush);
        let _ = DeleteObject(HGDIOBJ(brush.0));
    }
    if draw.itemID == u32::MAX {
        return;
    }
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
    let mut text_rect = rect;
    text_rect.left += scaled(8, dpi);
    text_rect.right -= scaled(4, dpi);
    let color = if disabled {
        palette.secondary
    } else {
        palette.heading
    };
    label(
        hdc,
        &String::from_utf16_lossy(&buffer[..length]),
        &mut text_rect,
        font,
        color,
        DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS,
    );
}

/// Opt this process into the system's dark common-control mode. Without it
/// `DarkMode_Explorer` themes leave combo boxes and scroll bars light.
/// Ordinal 135 is `SetPreferredAppMode` (undocumented, Win10 1809+); a missing
/// export is ignored so older systems keep the plain themed controls.
pub fn set_dark_mode(dark: bool) {
    type SetPreferredAppMode = unsafe extern "system" fn(i32) -> i32;
    type FlushMenuThemes = unsafe extern "system" fn();
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

fn uxtheme() -> HMODULE {
    static MODULE: std::sync::OnceLock<isize> = std::sync::OnceLock::new();
    let raw = *MODULE.get_or_init(|| unsafe {
        LoadLibraryW(windows::core::w!("uxtheme.dll"))
            .map(|module| module.0 as isize)
            .unwrap_or(0)
    });
    HMODULE(raw as *mut core::ffi::c_void)
}

/// Ask the control's theme to follow dark mode. This darkens combo boxes and
/// scroll bars, which plain WM_CTLCOLOR* messages cannot reach.
pub fn theme_control(hwnd: HWND, dark: bool) {
    if hwnd.0.is_null() {
        return;
    }
    type AllowDarkModeForWindow =
        unsafe extern "system" fn(HWND, windows::core::BOOL) -> windows::core::BOOL;
    let module = uxtheme();
    unsafe {
        if !module.0.is_null() {
            if let Some(proc) = GetProcAddress(module, windows::core::PCSTR(133 as *const u8)) {
                let allow: AllowDarkModeForWindow = std::mem::transmute(proc);
                let _ = allow(hwnd, windows::core::BOOL::from(dark));
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

/// Multi-line edits keep a classic scroll bar unless the ScrollBar theme part
/// is named explicitly.
pub fn theme_scrollbar(hwnd: HWND, dark: bool) {
    if hwnd.0.is_null() {
        return;
    }
    let name = if dark {
        windows::core::w!("DarkMode_Explorer")
    } else {
        windows::core::w!("Explorer")
    };
    let _ = unsafe { SetWindowTheme(hwnd, name, windows::core::w!("ScrollBar")) };
}

/// Combo boxes use a different theme part than Explorer controls.
pub fn theme_combo(hwnd: HWND, dark: bool) {
    if hwnd.0.is_null() {
        return;
    }
    let name = if dark {
        windows::core::w!("DarkMode_CFD")
    } else {
        windows::core::w!("")
    };
    let _ = unsafe { SetWindowTheme(hwnd, name, windows::core::PCWSTR::null()) };
}

/// Grey placeholder shown while an empty edit control is unfocused.
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

pub fn apply_window_style(hwnd: HWND) {
    let palette = crate::theme::current();
    set_dark_mode(palette.dark);
    let dark = windows::core::BOOL::from(palette.dark);
    let size = std::mem::size_of::<windows::core::BOOL>() as u32;
    let _ = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &dark as *const _ as *const core::ffi::c_void,
            size,
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
}
