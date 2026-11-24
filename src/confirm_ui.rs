//! Custom Direct2D confirmation dialog matching the app's Fluent surfaces.

use std::cell::{Cell, RefCell};

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{
    GetLastError, ERROR_CLASS_ALREADY_EXISTS, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, EndPaint, GetMonitorInfoW, InvalidateRect, MonitorFromWindow, UpdateWindow, HBRUSH,
    HDC, MONITORINFO, MONITOR_DEFAULTTONEAREST, PAINTSTRUCT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::DRAWITEMSTRUCT;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, GetFocus, SetFocus, VK_ESCAPE};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::d2d::{self, scaled, AlignH, AlignV, Icon, TextStyle};
use crate::{logging, platform, theme, ui};

const CLASS: PCWSTR = w!("QuickPhrase.Confirm");
const WIDTH: i32 = 452;
const HEIGHT: i32 = 270;
const ID_CANCEL: i32 = 601;
const ID_CONFIRM: i32 = 602;

struct Confirm {
    cancel: HWND,
    confirm: HWND,
    palette: theme::Palette,
    dpi: u32,
    heading: String,
    message: String,
    hint: String,
    danger: bool,
    paint_failures: u32,
}

thread_local! {
    static CONFIRM: RefCell<Option<Confirm>> = const { RefCell::new(None) };
    static RESULT: Cell<bool> = const { Cell::new(false) };
    static DONE: Cell<bool> = const { Cell::new(false) };
}

fn wide(text: &str) -> Vec<u16> {
    platform::wide(text)
}

pub fn is_open() -> bool {
    CONFIRM.with(|slot| slot.borrow().is_some())
}

/// Show a modal confirmation card centred over `owner`.
/// Returns true only when the confirm button is activated.
pub fn confirm(
    owner: HWND,
    heading: &str,
    message: &str,
    hint: &str,
    confirm_label: &str,
    danger: bool,
) -> bool {
    if owner.0.is_null() {
        return false;
    }
    if CONFIRM.with(|slot| slot.borrow().is_some()) {
        logging::warn("confirm: nested confirmation refused");
        return false;
    }
    RESULT.with(|r| r.set(false));
    DONE.with(|d| d.set(false));

    let module = match unsafe { GetModuleHandleW(None) } {
        Ok(module) => module,
        Err(_) => return fallback(owner, heading, message, hint),
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
            logging::error(&format!("confirm: RegisterClassW failed: {err:?}"));
            return fallback(owner, heading, message, hint);
        }
    }

    let dpi = unsafe { GetDpiForWindow(owner) };
    let dpi = if dpi == 0 { 96 } else { dpi };
    let w = scaled(WIDTH, dpi);
    let h = scaled(HEIGHT, dpi);

    let mut owner_rect = RECT::default();
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    unsafe {
        let _ = GetWindowRect(owner, &mut owner_rect);
    }
    let monitor = unsafe { MonitorFromWindow(owner, MONITOR_DEFAULTTONEAREST) };
    let work = if unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
        info.rcWork
    } else {
        owner_rect
    };
    let mut x = owner_rect.left + ((owner_rect.right - owner_rect.left) - w) / 2;
    let mut y = owner_rect.top + ((owner_rect.bottom - owner_rect.top) - h) / 2;
    x = x.clamp(work.left, (work.right - w).max(work.left));
    y = y.clamp(work.top, (work.bottom - h).max(work.top));

    let hwnd = match unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_CONTROLPARENT,
            CLASS,
            PCWSTR(wide(heading).as_ptr()),
            WS_POPUP | WS_CLIPCHILDREN,
            x,
            y,
            w,
            h,
            Some(owner),
            None,
            Some(instance),
            None,
        )
    } {
        Ok(hwnd) => hwnd,
        Err(err) => {
            logging::error(&format!("confirm: CreateWindowExW failed: {err}"));
            return fallback(owner, heading, message, hint);
        }
    };
    ui::apply_popup_style(hwnd, palette);

    let cancel = match unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            w!("BUTTON"),
            PCWSTR(wide("取消").as_ptr()),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(BS_OWNERDRAW as u32),
            0,
            0,
            10,
            10,
            Some(hwnd),
            Some(HMENU(ID_CANCEL as isize as *mut core::ffi::c_void)),
            Some(instance),
            None,
        )
    } {
        Ok(button) => button,
        Err(err) => {
            logging::error(&format!("confirm: cancel button failed: {err}"));
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            return fallback(owner, heading, message, hint);
        }
    };
    let confirm = match unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            w!("BUTTON"),
            PCWSTR(wide(confirm_label).as_ptr()),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(BS_OWNERDRAW as u32),
            0,
            0,
            10,
            10,
            Some(hwnd),
            Some(HMENU(ID_CONFIRM as isize as *mut core::ffi::c_void)),
            Some(instance),
            None,
        )
    } {
        Ok(button) => button,
        Err(err) => {
            logging::error(&format!("confirm: confirm button failed: {err}"));
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            return fallback(owner, heading, message, hint);
        }
    };

    CONFIRM.with(|slot| {
        *slot.borrow_mut() = Some(Confirm {
            cancel,
            confirm,
            palette,
            dpi,
            heading: heading.to_string(),
            message: message.to_string(),
            hint: hint.to_string(),
            danger,
            paint_failures: 0,
        });
    });
    layout(w, h);

    let previous_focus = unsafe { GetFocus() };
    unsafe {
        let _ = EnableWindow(owner, false);
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
        let _ = SetFocus(Some(cancel));
        let _ = UpdateWindow(hwnd);
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
        let handled = unsafe { IsDialogMessageW(hwnd, &msg) }.as_bool();
        if !handled {
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }

    unsafe {
        let _ = EnableWindow(owner, true);
        let _ = SetForegroundWindow(owner);
        if !previous_focus.0.is_null() && IsWindow(Some(previous_focus)).as_bool() {
            let _ = SetFocus(Some(previous_focus));
        } else {
            let _ = SetFocus(Some(owner));
        }
        if IsWindow(Some(hwnd)).as_bool() {
            let _ = DestroyWindow(hwnd);
        }
    }
    RESULT.with(|r| r.get())
}

fn fallback(owner: HWND, heading: &str, message: &str, hint: &str) -> bool {
    let text = wide(&format!("{message}\n\n{hint}"));
    let title = wide(heading);
    unsafe {
        MessageBoxW(
            Some(owner),
            PCWSTR(text.as_ptr()),
            PCWSTR(title.as_ptr()),
            MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2,
        ) == IDYES
    }
}

fn layout(w: i32, h: i32) {
    let state = CONFIRM.with(|slot| {
        slot.borrow()
            .as_ref()
            .map(|dialog| (dialog.dpi, dialog.cancel, dialog.confirm))
    });
    let Some((dpi, cancel, confirm)) = state else {
        return;
    };
    let pad = scaled(24, dpi);
    let btn_h = scaled(34, dpi);
    let gap = scaled(10, dpi);
    let cancel_w = scaled(84, dpi);
    let confirm_w = scaled(124, dpi);
    let y = h - pad - btn_h;
    unsafe {
        let _ = MoveWindow(
            cancel,
            w - pad - confirm_w - gap - cancel_w,
            y,
            cancel_w,
            btn_h,
            true,
        );
        let _ = MoveWindow(confirm, w - pad - confirm_w, y, confirm_w, btn_h, true);
    }
}

fn paint(dialog: &Confirm, hdc: HDC, client: RECT) -> bool {
    let dpi = dialog.dpi;
    let p = dialog.palette;
    d2d::paint_buffered(hdc, client, |c| {
        c.clear(p.card);
        let border = RECT {
            left: client.left,
            top: client.top,
            right: client.right - 1,
            bottom: client.bottom - 1,
        };
        c.stroke_round(&border, d2d::scaled_f(12.0, dpi), p.border, 1.0);

        let icon_sz = scaled(40, dpi);
        let icon_rect = RECT {
            left: scaled(24, dpi),
            top: scaled(24, dpi),
            right: scaled(24, dpi) + icon_sz,
            bottom: scaled(24, dpi) + icon_sz,
        };
        let (tone_bg, tone_fg, glyph) = if dialog.danger {
            (p.danger_soft, p.danger, Icon::Trash)
        } else {
            (p.warning_soft, p.warning, Icon::Warning)
        };
        c.fill_round(&icon_rect, d2d::scaled_f(12.0, dpi), tone_bg);
        c.icon(
            glyph,
            (icon_rect.left + icon_rect.right) as f32 * 0.5,
            (icon_rect.top + icon_rect.bottom) as f32 * 0.5,
            d2d::scaled_f(18.0, dpi),
            tone_fg,
        );

        let text_left = icon_rect.right + scaled(14, dpi);
        let heading_rect = RECT {
            left: text_left,
            top: scaled(26, dpi),
            right: client.right - scaled(24, dpi),
            bottom: scaled(50, dpi),
        };
        c.draw_text(
            &dialog.heading,
            &heading_rect,
            TextStyle::Title,
            dpi,
            p.heading,
            AlignH::Left,
            AlignV::Center,
        );
        let message_rect = RECT {
            left: text_left,
            top: scaled(56, dpi),
            right: client.right - scaled(24, dpi),
            bottom: scaled(132, dpi),
        };
        c.draw_text_wrapped(
            &dialog.message,
            &message_rect,
            TextStyle::Body,
            dpi,
            p.secondary,
            AlignH::Left,
            AlignV::Center,
        );
        let hint_rect = RECT {
            left: text_left,
            top: scaled(136, dpi),
            right: client.right - scaled(24, dpi),
            bottom: scaled(158, dpi),
        };
        c.draw_text(
            &dialog.hint,
            &hint_rect,
            TextStyle::Label,
            dpi,
            p.muted,
            AlignH::Left,
            AlignV::Center,
        );
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PaintAction {
    None,
    Retry,
    Close,
}

fn paint_step(ok: bool, failures: u32) -> (u32, PaintAction) {
    if ok {
        return (0, PaintAction::None);
    }
    let count = failures.saturating_add(1);
    if count >= 3 {
        (count, PaintAction::Close)
    } else {
        (count, PaintAction::Retry)
    }
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
            let handled = CONFIRM.with(|slot| match slot.borrow_mut().as_mut() {
                Some(dialog) => {
                    dialog.dpi = new_dpi;
                    true
                }
                None => false,
            });
            if !handled {
                return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
            }
            let width = scaled(WIDTH, new_dpi);
            let height = scaled(HEIGHT, new_dpi);
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
                let mut client = RECT::default();
                let _ = GetClientRect(hwnd, &mut client);
                layout(client.right, client.bottom);
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            LRESULT(0)
        }
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = unsafe { BeginPaint(hwnd, &mut ps) };
            let mut client = RECT::default();
            let _ = unsafe { GetClientRect(hwnd, &mut client) };
            let action = CONFIRM.with(|slot| match slot.try_borrow_mut() {
                Ok(mut guard) => match guard.as_mut() {
                    Some(dialog) => {
                        let ok = paint(dialog, hdc, client);
                        let (failures, action) = paint_step(ok, dialog.paint_failures);
                        dialog.paint_failures = failures;
                        action
                    }
                    None => PaintAction::None,
                },
                Err(_) => PaintAction::None,
            });
            unsafe {
                let _ = EndPaint(hwnd, &ps);
            }
            match action {
                PaintAction::None => {}
                // Drive the next paint ourselves: D2D's retry is one-shot and
                // may already be spent.
                PaintAction::Retry => unsafe {
                    let _ = InvalidateRect(Some(hwnd), None, false);
                },
                PaintAction::Close => unsafe {
                    let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
                },
            }
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_DRAWITEM => {
            let draw = unsafe { &*(lparam.0 as *const DRAWITEMSTRUCT) };
            CONFIRM.with(|slot| {
                if let Some(dialog) = slot.borrow().as_ref() {
                    let kind = if draw.CtlID == ID_CONFIRM as u32 {
                        ui::ButtonKind::Danger
                    } else {
                        ui::ButtonKind::Secondary
                    };
                    ui::draw_button(draw, dialog.palette, kind, dialog.dpi, dialog.palette.card);
                }
            });
            LRESULT(0)
        }
        WM_COMMAND => {
            let id = (wparam.0 & 0xffff) as i32;
            if id == ID_CONFIRM {
                RESULT.with(|r| r.set(true));
                DONE.with(|d| d.set(true));
                unsafe {
                    let _ = DestroyWindow(hwnd);
                }
            } else if id == ID_CANCEL || id == 2 {
                // 2 is IDCANCEL: IsDialogMessage may send it for Esc.
                DONE.with(|d| d.set(true));
                unsafe {
                    let _ = DestroyWindow(hwnd);
                }
            } else if id == 1 {
                // IsDialogMessage maps Enter to IDOK; honour the focused button
                // so the safe default (Cancel) stays safe.
                let focus = unsafe { GetFocus() };
                let confirmed = CONFIRM.with(|slot| {
                    slot.borrow()
                        .as_ref()
                        .map(|dialog| focus == dialog.confirm)
                        .unwrap_or(false)
                });
                if confirmed {
                    RESULT.with(|r| r.set(true));
                }
                DONE.with(|d| d.set(true));
                unsafe {
                    let _ = DestroyWindow(hwnd);
                }
            }
            LRESULT(0)
        }
        WM_KEYDOWN if wparam.0 == VK_ESCAPE.0 as usize => {
            DONE.with(|d| d.set(true));
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            DONE.with(|d| d.set(true));
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            LRESULT(0)
        }
        WM_NCDESTROY => {
            DONE.with(|d| d.set(true));
            CONFIRM.with(|slot| {
                slot.borrow_mut().take();
            });
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

#[cfg(test)]
mod tests {
    use super::{paint_step, PaintAction};

    #[test]
    fn paint_failures_retry_twice_then_fail_closed() {
        let (failures, action) = paint_step(false, 0);
        assert_eq!((failures, action), (1, PaintAction::Retry));
        let (failures, action) = paint_step(false, failures);
        assert_eq!((failures, action), (2, PaintAction::Retry));
        let (failures, action) = paint_step(false, failures);
        assert_eq!((failures, action), (3, PaintAction::Close));
        // A prompt that never gets to close stays closed on later failures.
        let (_, action) = paint_step(false, failures);
        assert_eq!(action, PaintAction::Close);
    }

    #[test]
    fn a_successful_paint_resets_the_failure_count() {
        let (failures, _) = paint_step(false, 0);
        let (failures, _) = paint_step(false, failures);
        let (failures, action) = paint_step(true, failures);
        assert_eq!((failures, action), (0, PaintAction::None));
        // A later failure starts a fresh sequence instead of closing.
        let (failures, action) = paint_step(false, failures);
        assert_eq!((failures, action), (1, PaintAction::Retry));
    }
}
