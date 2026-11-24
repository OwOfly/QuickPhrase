//! Custom Direct2D single-column picker used by the management window's
//! combo boxes (tag filter, category), replacing the native ComboBoxLBox.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{
    GetLastError, ERROR_CLASS_ALREADY_EXISTS, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, EndPaint, GetMonitorInfoW, InvalidateRect, MonitorFromWindow, UpdateWindow, HBRUSH,
    HDC, MONITORINFO, MONITOR_DEFAULTTONEAREST, PAINTSTRUCT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, SetFocus, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT,
    VK_DOWN, VK_END, VK_ESCAPE, VK_F4, VK_HOME, VK_MENU, VK_RETURN, VK_TAB, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::d2d::{self, AlignH, AlignV, Icon, TextStyle};
use crate::{logging, theme, ui};

const CLASS: PCWSTR = w!("QuickPhrase.DropMenu");
const MIN_WIDTH: i32 = 200;
const MAX_VISIBLE: usize = 8;
const ROW_HEIGHT: i32 = 34;
const PAD: i32 = 6;

/// Queued request from the combo subclass to open the custom picker.
pub const OPEN_PICKER: u32 = WM_APP + 0x51;

struct DropMenu {
    items: Vec<String>,
    selected: Option<usize>,
    hovered: Option<usize>,
    top: usize,
    visible: usize,
    dpi: u32,
    palette: theme::Palette,
    tracking: bool,
    active: bool,
    drag: bool,
    drag_offset: i32,
}

thread_local! {
    static MENU: RefCell<Option<DropMenu>> = const { RefCell::new(None) };
    static RESULT: Cell<Option<usize>> = const { Cell::new(None) };
    static DONE: Cell<bool> = const { Cell::new(false) };
    static RESTORE_FOCUS: Cell<bool> = const { Cell::new(true) };
    static COMBOS: RefCell<HashMap<isize, ComboLink>> = RefCell::new(HashMap::new());
}

struct ComboLink {
    old_proc: isize,
    id: i32,
}

/// Route the native combo box's open gestures to the custom picker.
///
/// The native dropdown is not opened at all: the box would set its internal
/// "open" flag after sending CBN_DROPDOWN, and suppressing the list inside the
/// notification left that flag stuck, so the next F4/click toggled a list that
/// was not there. Intercepting the gestures before the box handles them keeps
/// its state untouched; CBN_DROPDOWN is sent to the parent ourselves.
pub unsafe fn attach_combo(combo: HWND, id: i32) {
    let key = combo.0 as isize;
    if COMBOS.with(|map| map.borrow().contains_key(&key)) {
        return;
    }
    let old = GetWindowLongPtrW(combo, GWLP_WNDPROC);
    if old == 0 {
        return;
    }
    COMBOS.with(|map| {
        map.borrow_mut()
            .insert(key, ComboLink { old_proc: old, id });
    });
    let previous = SetWindowLongPtrW(
        combo,
        GWLP_WNDPROC,
        combo_proc as *const () as usize as isize,
    );
    if previous != old {
        COMBOS.with(|map| {
            map.borrow_mut().remove(&key);
        });
    }
}

unsafe extern "system" fn combo_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let key = hwnd.0 as isize;
    let link = COMBOS.with(|map| map.borrow().get(&key).map(|link| (link.old_proc, link.id)));
    let Some((old, id)) = link else {
        return DefWindowProcW(hwnd, message, wparam, lparam);
    };

    let open = match message {
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => true,
        WM_KEYDOWN if wparam.0 as u32 == VK_F4.0 as u32 => true,
        WM_SYSKEYDOWN
            if wparam.0 as u32 == VK_DOWN.0 as u32
                && (GetKeyState(VK_MENU.0 as i32) as u16 & 0x8000) != 0 =>
        {
            true
        }
        _ => false,
    };
    if open {
        let _ = SetFocus(Some(hwnd));
        let parent = GetParent(hwnd).unwrap_or_default();
        let _ = PostMessageW(Some(parent), OPEN_PICKER, WPARAM(id as usize), LPARAM(key));
        return LRESULT(0);
    }
    if message == WM_LBUTTONUP {
        return LRESULT(0);
    }
    if message == WM_NCDESTROY {
        // Child windows receive WM_NCDESTROY; drop the link so a later combo
        // with a recycled HWND value is subclassed again.
        COMBOS.with(|map| {
            map.borrow_mut().remove(&key);
        });
    }

    let proc: unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT =
        std::mem::transmute(old);
    proc(hwnd, message, wparam, lparam)
}

fn scaled(value: i32, dpi: u32) -> i32 {
    (value * dpi as i32 + 48) / 96
}

fn row_height(dpi: u32) -> i32 {
    scaled(ROW_HEIGHT, dpi)
}

fn menu_height(dpi: u32, visible: usize) -> i32 {
    pad(dpi) * 2 + visible as i32 * row_height(dpi)
}

fn menu_width(suggested: i32, dpi: u32) -> i32 {
    suggested.max(scaled(MIN_WIDTH, dpi))
}

fn scale_drag_offset(offset: i32, old_dpi: u32, new_dpi: u32) -> i32 {
    if old_dpi == 0 || old_dpi == new_dpi {
        return offset;
    }
    ((offset as i64 * new_dpi as i64 + old_dpi as i64 / 2) / old_dpi as i64) as i32
}

fn pad(dpi: u32) -> i32 {
    scaled(PAD, dpi)
}

/// Keep either direction of keyboard movement visible after wheel/drag scrolling.
fn keyboard_top(current_top: usize, next: usize, visible: usize, max_top: usize) -> usize {
    if next >= current_top + visible {
        (next + 1).saturating_sub(visible).min(max_top)
    } else if next < current_top {
        next
    } else {
        current_top
    }
}

/// Enter never commits a row hidden by scrolling.
fn visible_pick(index: Option<usize>, top: usize, visible: usize, len: usize) -> Option<usize> {
    index.filter(|&index| index >= top && index < top + visible && index < len)
}

pub fn is_open() -> bool {
    MENU.with(|slot| slot.borrow().is_some())
}

/// Show a Fluent popover list anchored to `anchor`.
/// Returns the chosen index, or None when dismissed.
pub fn show(owner: HWND, anchor: HWND, items: &[String], selected: Option<usize>) -> Option<usize> {
    if owner.0.is_null() || anchor.0.is_null() || items.is_empty() {
        return None;
    }
    RESULT.with(|r| r.set(None));
    DONE.with(|d| d.set(false));
    RESTORE_FOCUS.with(|r| r.set(true));

    let module = match unsafe { GetModuleHandleW(None) } {
        Ok(module) => module,
        Err(_) => return None,
    };
    let instance = HINSTANCE(module.0);
    let palette = theme::current();
    let class = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW | CS_DROPSHADOW,
        lpfnWndProc: Some(window_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: instance,
        hIcon: ui::load_app_icon(instance),
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW) }.unwrap_or_default(),
        hbrBackground: HBRUSH::default(),
        lpszMenuName: PCWSTR::null(),
        lpszClassName: CLASS,
    };
    if unsafe { RegisterClassW(&class) } == 0 {
        let err = unsafe { GetLastError() };
        if err != ERROR_CLASS_ALREADY_EXISTS {
            logging::error(&format!("dropmenu: RegisterClassW failed: {err:?}"));
            return None;
        }
    }

    let dpi = unsafe { GetDpiForWindow(anchor) };
    let dpi = if dpi == 0 { 96 } else { dpi };
    let mut anchor_rect = RECT::default();
    unsafe {
        let _ = GetWindowRect(anchor, &mut anchor_rect);
    }
    let width = menu_width(anchor_rect.right - anchor_rect.left, dpi);
    let visible = items.len().min(MAX_VISIBLE);
    let height = menu_height(dpi, visible);

    let monitor = unsafe { MonitorFromWindow(anchor, MONITOR_DEFAULTTONEAREST) };
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    let work = if unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
        info.rcWork
    } else {
        anchor_rect
    };
    let mut x = anchor_rect.left;
    let mut y = anchor_rect.bottom + scaled(4, dpi);
    if y + height > work.bottom {
        y = anchor_rect.top - height - scaled(4, dpi);
    }
    x = x.clamp(work.left, (work.right - width).max(work.left));
    y = y.clamp(work.top, (work.bottom - height).max(work.top));

    let top = selected
        .map(|index| index.saturating_sub(visible.saturating_sub(1)))
        .unwrap_or(0)
        .min(items.len().saturating_sub(visible));

    let hwnd = match unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            CLASS,
            w!("QuickPhrase"),
            WS_POPUP,
            x,
            y,
            width,
            height,
            Some(owner),
            None,
            Some(instance),
            None,
        )
    } {
        Ok(hwnd) => hwnd,
        Err(err) => {
            logging::error(&format!("dropmenu: CreateWindowExW failed: {err}"));
            return None;
        }
    };
    ui::apply_popup_style(hwnd, palette);

    MENU.with(|slot| {
        *slot.borrow_mut() = Some(DropMenu {
            items: items.to_vec(),
            selected,
            hovered: selected,
            top,
            visible,
            dpi,
            palette,
            tracking: false,
            active: false,
            drag: false,
            drag_offset: 0,
        });
    });

    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
        let _ = SetFocus(Some(hwnd));
        let _ = UpdateWindow(hwnd);
        let _ = SetCapture(hwnd);
    }

    let mut msg = MSG::default();
    loop {
        if DONE.with(|done| done.get()) {
            break;
        }
        let got = unsafe { GetMessageW(&mut msg, None, 0, 0) }.0;
        if got == 0 {
            unsafe {
                PostQuitMessage(msg.wParam.0 as i32);
            }
            break;
        }
        if got < 0 {
            break;
        }
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    unsafe {
        let _ = ReleaseCapture();
        if IsWindow(Some(hwnd)).as_bool() {
            let _ = DestroyWindow(hwnd);
        }
        if RESTORE_FOCUS.with(|r| r.get()) {
            let _ = SetForegroundWindow(owner);
            let _ = SetFocus(Some(anchor));
        }
    }
    RESULT.with(|r| r.get())
}

fn item_at(menu: &DropMenu, x: i32, y: i32) -> Option<usize> {
    let p = pad(menu.dpi);
    let rh = row_height(menu.dpi);
    if x < 0 || y < p || y >= p + menu.visible as i32 * rh {
        return None;
    }
    let row = (y - p) / rh;
    if row < 0 || row >= menu.visible as i32 {
        return None;
    }
    let index = menu.top + row as usize;
    if index < menu.items.len() {
        Some(index)
    } else {
        None
    }
}

/// Returns (track_top, track_bottom, thumb_top, thumb_h, travel, max_top).
fn scroll_geometry(menu: &DropMenu, client: RECT) -> Option<(i32, i32, i32, i32, i32, usize)> {
    if menu.items.len() <= menu.visible {
        return None;
    }
    let p = pad(menu.dpi);
    let track_top = p;
    let track_bottom = (client.bottom - p).max(track_top + 1);
    let track_h = (track_bottom - track_top).max(1);
    let thumb_h = ((track_h as f32 * menu.visible as f32 / menu.items.len() as f32).round() as i32)
        .max(scaled(18, menu.dpi));
    let max_top = menu.items.len().saturating_sub(menu.visible).max(1);
    let travel = (track_h - thumb_h).max(0);
    let thumb_top = track_top + (travel as f32 * menu.top as f32 / max_top as f32).round() as i32;
    Some((track_top, track_bottom, thumb_top, thumb_h, travel, max_top))
}

fn paint(menu: &DropMenu, hdc: HDC, client: RECT) {
    let dpi = menu.dpi;
    let p = menu.palette;
    let pad = pad(dpi);
    let rh = row_height(dpi);
    d2d::paint_buffered(hdc, client, |c| {
        c.clear(p.card);
        let border = RECT {
            left: client.left,
            top: client.top,
            right: client.right - 1,
            bottom: client.bottom - 1,
        };
        c.stroke_round(&border, d2d::scaled_f(10.0, dpi), p.border_strong, 1.0);

        for row in 0..menu.visible {
            let index = menu.top + row;
            if index >= menu.items.len() {
                break;
            }
            let rect = RECT {
                left: pad,
                top: pad + row as i32 * rh,
                right: client.right - pad,
                bottom: pad + row as i32 * rh + rh,
            };
            let selected = menu.selected == Some(index);
            let hovered = menu.hovered == Some(index);
            if selected {
                c.fill_round(&rect, d2d::scaled_f(7.0, dpi), p.selection);
            } else if hovered {
                c.fill_round(&rect, d2d::scaled_f(7.0, dpi), p.field);
            }
            let text_rect = RECT {
                left: rect.left + scaled(10, dpi),
                top: rect.top,
                right: rect.right - scaled(28, dpi),
                bottom: rect.bottom,
            };
            c.draw_text(
                &menu.items[index],
                &text_rect,
                if selected {
                    TextStyle::Heading
                } else {
                    TextStyle::Body
                },
                dpi,
                if selected { p.primary_text } else { p.heading },
                AlignH::Left,
                AlignV::Center,
            );
            if selected {
                c.icon(
                    Icon::Check,
                    (rect.right - scaled(14, dpi)) as f32,
                    (rect.top + rect.bottom) as f32 * 0.5,
                    d2d::scaled_f(12.0, dpi),
                    p.primary,
                );
            }
        }

        if let Some((_, _, thumb_top, thumb_h, _, _)) = scroll_geometry(menu, client) {
            let thumb = RECT {
                left: client.right - scaled(8, dpi),
                top: thumb_top,
                right: client.right - scaled(5, dpi),
                bottom: thumb_top + thumb_h,
            };
            c.fill_round(&thumb, d2d::scaled_f(2.0, dpi), p.border_strong);
        }
    });
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_DPICHANGED => {
            let new_dpi = crate::ui::dpi_from_wparam(wparam);
            let suggested = unsafe { &*(lparam.0 as *const RECT) };
            let geometry = MENU.with(|slot| {
                let mut borrow = slot.borrow_mut();
                let menu = borrow.as_mut()?;
                let old_dpi = menu.dpi;
                if menu.drag {
                    menu.drag_offset = scale_drag_offset(menu.drag_offset, old_dpi, new_dpi);
                }
                menu.dpi = new_dpi;
                let width = menu_width(suggested.right - suggested.left, new_dpi);
                let height = menu_height(new_dpi, menu.visible);
                Some((width, height))
            });
            let Some((width, height)) = geometry else {
                return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
            };
            let mut x = suggested.left;
            let mut y = suggested.top;
            let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
            let mut info = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            if unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
                let work = info.rcWork;
                x = x.clamp(work.left, (work.right - width).max(work.left));
                y = y.clamp(work.top, (work.bottom - height).max(work.top));
            }
            unsafe {
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    x,
                    y,
                    width,
                    height,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            LRESULT(0)
        }
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = unsafe { BeginPaint(hwnd, &mut ps) };
            let mut client = RECT::default();
            let _ = unsafe { GetClientRect(hwnd, &mut client) };
            MENU.with(|slot| {
                if let Some(menu) = slot.borrow().as_ref() {
                    paint(menu, hdc, client);
                }
            });
            unsafe {
                let _ = EndPaint(hwnd, &ps);
            }
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_MOUSEMOVE => {
            let x = (lparam.0 & 0xffff) as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xffff) as i16 as i32;
            let mut client = RECT::default();
            let _ = unsafe { GetClientRect(hwnd, &mut client) };
            MENU.with(|slot| {
                if let Some(menu) = slot.borrow_mut().as_mut() {
                    if menu.drag {
                        if let Some((track_top, _, _, _, travel, max_top)) =
                            scroll_geometry(menu, client)
                        {
                            let thumb_y =
                                (y - menu.drag_offset).clamp(track_top, track_top + travel);
                            let frac = if travel > 0 {
                                (thumb_y - track_top) as f32 / travel as f32
                            } else {
                                0.0
                            };
                            menu.top = (frac * max_top as f32).round() as usize;
                            let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
                        }
                    } else {
                        let in_lane = menu.items.len() > menu.visible
                            && x >= client.right - scaled(10, menu.dpi);
                        let hovered = if in_lane { None } else { item_at(menu, x, y) };
                        if hovered != menu.hovered {
                            menu.hovered = hovered;
                            let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
                        }
                        if !menu.tracking {
                            menu.tracking = true;
                            let mut track = TRACKMOUSEEVENT {
                                cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                                dwFlags: TME_LEAVE,
                                hwndTrack: hwnd,
                                dwHoverTime: 0,
                            };
                            let _ = unsafe { TrackMouseEvent(&mut track) };
                        }
                    }
                }
            });
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            MENU.with(|slot| {
                if let Some(menu) = slot.borrow_mut().as_mut() {
                    menu.tracking = false;
                    if menu.hovered != menu.selected {
                        menu.hovered = menu.selected;
                        let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
                    }
                }
            });
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let x = (lparam.0 & 0xffff) as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xffff) as i16 as i32;
            let mut client = RECT::default();
            let _ = unsafe { GetClientRect(hwnd, &mut client) };
            let inside =
                x >= client.left && x < client.right && y >= client.top && y < client.bottom;
            if inside {
                let mut dragging_scroll = false;
                MENU.with(|slot| {
                    if let Some(menu) = slot.borrow_mut().as_mut() {
                        if menu.items.len() > menu.visible
                            && x >= client.right - scaled(10, menu.dpi)
                        {
                            if let Some((track_top, _, thumb_top, thumb_h, travel, max_top)) =
                                scroll_geometry(menu, client)
                            {
                                let on_thumb = y >= thumb_top && y <= thumb_top + thumb_h;
                                menu.drag = true;
                                menu.drag_offset =
                                    if on_thumb { y - thumb_top } else { thumb_h / 2 };
                                let thumb_y =
                                    (y - menu.drag_offset).clamp(track_top, track_top + travel);
                                let frac = if travel > 0 {
                                    (thumb_y - track_top) as f32 / travel as f32
                                } else {
                                    0.0
                                };
                                menu.top = (frac * max_top as f32).round() as usize;
                                let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
                                dragging_scroll = true;
                            }
                        }
                    }
                });
                if dragging_scroll {
                    return LRESULT(0);
                }
                let picked =
                    MENU.with(|slot| slot.borrow().as_ref().and_then(|menu| item_at(menu, x, y)));
                if let Some(index) = picked {
                    RESULT.with(|r| r.set(Some(index)));
                }
                RESTORE_FOCUS.with(|r| r.set(true));
            } else {
                RESTORE_FOCUS.with(|r| r.set(false));
            }
            DONE.with(|d| d.set(true));
            let _ = unsafe { ShowWindow(hwnd, SW_HIDE) };
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            MENU.with(|slot| {
                if let Some(menu) = slot.borrow_mut().as_mut() {
                    menu.drag = false;
                }
            });
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            let delta = ((wparam.0 >> 16) as u16 as i16) as i32;
            MENU.with(|slot| {
                if let Some(menu) = slot.borrow_mut().as_mut() {
                    let max_top = menu.items.len().saturating_sub(menu.visible);
                    if delta > 0 {
                        menu.top = menu.top.saturating_sub(1);
                    } else if delta < 0 {
                        menu.top = (menu.top + 1).min(max_top);
                    }
                    let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
                }
            });
            LRESULT(0)
        }
        WM_KEYDOWN => {
            let mut picked = None;
            MENU.with(|slot| {
                let mut borrow = slot.borrow_mut();
                let Some(menu) = borrow.as_mut() else {
                    return;
                };
                let max_top = menu.items.len().saturating_sub(menu.visible);
                let key = wparam.0 as u16;
                match key {
                    key if key == VK_DOWN.0 => {
                        let current = menu
                            .hovered
                            .or(menu.selected)
                            .map(|index| index + 1)
                            .unwrap_or(0);
                        let next = current.min(menu.items.len().saturating_sub(1));
                        menu.hovered = Some(next);
                        menu.top = keyboard_top(menu.top, next, menu.visible, max_top);
                    }
                    key if key == VK_UP.0 => {
                        let current = menu.hovered.or(menu.selected).unwrap_or(0);
                        let next = current.saturating_sub(1);
                        menu.hovered = Some(next);
                        menu.top = keyboard_top(menu.top, next, menu.visible, max_top);
                    }
                    key if key == VK_HOME.0 => {
                        menu.hovered = Some(0);
                        menu.top = 0;
                    }
                    key if key == VK_END.0 => {
                        let last = menu.items.len().saturating_sub(1);
                        menu.hovered = Some(last);
                        menu.top = max_top;
                    }
                    key if key == VK_RETURN.0 => {
                        picked = visible_pick(
                            menu.hovered.or(menu.selected),
                            menu.top,
                            menu.visible,
                            menu.items.len(),
                        );
                    }
                    key if key == VK_ESCAPE.0 || key == VK_TAB.0 => {
                        picked = None;
                        DONE.with(|d| d.set(true));
                    }
                    _ => {}
                }
            });
            if let Some(index) = picked {
                RESULT.with(|r| r.set(Some(index)));
                DONE.with(|d| d.set(true));
            }
            if DONE.with(|d| d.get()) {
                let _ = unsafe { ShowWindow(hwnd, SW_HIDE) };
            } else {
                let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
            }
            LRESULT(0)
        }
        WM_ACTIVATE => {
            let state = (wparam.0 & 0xffff) as u32;
            let mut close = false;
            MENU.with(|slot| {
                if let Some(menu) = slot.borrow_mut().as_mut() {
                    if state == WA_ACTIVE || state == WA_CLICKACTIVE {
                        menu.active = true;
                    } else if state == WA_INACTIVE && menu.active {
                        close = true;
                    }
                }
            });
            if close {
                if !DONE.with(|d| d.get()) {
                    // Genuine external deactivation (Alt+Tab): the window we
                    // switched to keeps the focus. Hiding the picker after we
                    // already decided to close also sends WA_INACTIVE, and must
                    // not cancel focus restoration to the anchor.
                    RESTORE_FOCUS.with(|r| r.set(false));
                }
                DONE.with(|d| d.set(true));
                let _ = unsafe { ShowWindow(hwnd, SW_HIDE) };
            }
            LRESULT(0)
        }
        WM_KILLFOCUS => {
            // Capture owns the mouse and outside clicks close the picker;
            // keep the popover alive if focus is briefly lost during show.
            LRESULT(0)
        }
        WM_CLOSE => {
            RESTORE_FOCUS.with(|r| r.set(false));
            DONE.with(|d| d.set(true));
            let _ = unsafe { ShowWindow(hwnd, SW_HIDE) };
            LRESULT(0)
        }
        WM_NCDESTROY => {
            if !DONE.with(|d| d.get()) {
                RESTORE_FOCUS.with(|r| r.set(false));
            }
            DONE.with(|d| d.set(true));
            MENU.with(|slot| {
                slot.borrow_mut().take();
            });
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

#[cfg(test)]
mod tests {
    use super::{keyboard_top, menu_height, menu_width, scale_drag_offset, visible_pick};

    #[test]
    fn menu_height_scales_with_dpi() {
        assert_eq!(menu_height(96, 8), 284);
        assert_eq!(menu_height(120, 8), 360);
        assert_eq!(menu_height(144, 8), 426);
        assert_eq!(menu_height(96, 0), 12);
    }

    #[test]
    fn menu_width_keeps_the_minimum_scaled() {
        assert_eq!(menu_width(260, 96), 260);
        assert_eq!(menu_width(100, 96), 200);
        assert_eq!(menu_width(100, 120), 250);
        assert_eq!(menu_width(200, 144), 300);
    }

    #[test]
    fn drag_offset_follows_the_dpi_ratio() {
        assert_eq!(scale_drag_offset(10, 96, 144), 15);
        assert_eq!(scale_drag_offset(10, 144, 96), 7);
        assert_eq!(scale_drag_offset(10, 96, 96), 10);
        assert_eq!(scale_drag_offset(10, 0, 144), 10);
    }

    #[test]
    fn down_scrolls_back_to_offscreen_highlight() {
        // 23 items, 8 visible, wheel/drag left the view at the bottom while
        // the highlight was still on index 0; Down moves to 1.
        assert_eq!(keyboard_top(15, 1, 8, 15), 1);
    }

    #[test]
    fn down_scrolls_forward_past_the_last_visible_row() {
        assert_eq!(keyboard_top(0, 8, 8, 15), 1);
        assert_eq!(keyboard_top(5, 15, 8, 15), 8);
    }

    #[test]
    fn down_keeps_a_visible_highlight_in_place() {
        assert_eq!(keyboard_top(3, 4, 8, 15), 3);
        assert_eq!(keyboard_top(15, 15, 8, 15), 15);
    }
    #[test]
    fn enter_only_commits_a_visible_row() {
        assert_eq!(visible_pick(Some(2), 5, 4, 12), None);
        assert_eq!(visible_pick(Some(9), 5, 4, 12), None);
        assert_eq!(visible_pick(Some(8), 5, 4, 12), Some(8));
        assert_eq!(visible_pick(Some(8), 5, 4, 8), None);
    }
}
