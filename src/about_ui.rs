//! QuickPhrase 2.0 custom Direct2D About dialog (replaces plain MessageBoxW).

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{
    GetLastError, ERROR_CLASS_ALREADY_EXISTS, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, DeleteObject, EndPaint, GetMonitorInfoW, InvalidateRect, MonitorFromPoint, HBRUSH,
    HDC, HFONT, HGDIOBJ, MONITORINFO, MONITOR_DEFAULTTONEAREST, PAINTSTRUCT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::DRAWITEMSTRUCT;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, GetDpiForWindow, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetFocus, SetFocus, VK_ESCAPE};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::d2d::{self, scaled, AlignH, AlignV, TextStyle};
use crate::{logging, platform, theme, ui};

const CLASS: PCWSTR = w!("QuickPhrase.AboutWindow");
const TITLE: PCWSTR = w!("关于 QuickPhrase");

const ID_COPY: i32 = 401;
const ID_OK: i32 = 402;
const WIDTH: i32 = 380;
const HEIGHT: i32 = 360;

struct AboutUi {
    hwnd: HWND,
    copy_btn: HWND,
    ok_btn: HWND,
    font: HFONT,
    palette: theme::Palette,
    dpi: u32,
    data_dir: String,
    copied: bool,
}

thread_local! {
    static ABOUT: RefCell<Option<AboutUi>> = const { RefCell::new(None) };
}

static QUIT_ON_CLOSE: AtomicBool = AtomicBool::new(false);

fn make_font(dpi: u32) -> HFONT {
    ui::make_font(ui::FontRole::BodyInput, dpi)
}

fn layout(about: &AboutUi, width: i32, height: i32) {
    let dpi = about.dpi;
    let pad = scaled(24, dpi);
    let btn_h = scaled(34, dpi);
    let btn_y = height - pad - btn_h;
    let ok_w = scaled(88, dpi);
    let gap = scaled(8, dpi);
    let copy_w = (width - pad * 2 - ok_w - gap).max(scaled(100, dpi));
    unsafe {
        let _ = MoveWindow(about.copy_btn, pad, btn_y, copy_w, btn_h, true);
        let _ = MoveWindow(about.ok_btn, pad + copy_w + gap, btn_y, ok_w, btn_h, true);
    }
}

fn create() -> Option<AboutUi> {
    let module = unsafe { GetModuleHandleW(None) }.ok()?;
    let instance = HINSTANCE(module.0);
    let palette = theme::current();
    let class = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
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
            logging::error(&format!("about: RegisterClassW failed: {err:?}"));
            return None;
        }
    }

    let mut pt = POINT { x: 0, y: 0 };
    unsafe {
        let _ = GetCursorPos(&mut pt);
    }
    let monitor = unsafe { MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST) };
    let mut dpi_x = 96u32;
    let mut dpi_y = 96u32;
    if unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) }.is_err()
        || dpi_x == 0
    {
        dpi_x = 96;
    }
    let dpi = dpi_x;
    let w = scaled(WIDTH, dpi);
    let h = scaled(HEIGHT, dpi);
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    let (x, y) = if unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
        let rw = info.rcWork;
        (
            rw.left + (rw.right - rw.left - w) / 2,
            rw.top + (rw.bottom - rw.top - h) / 2,
        )
    } else {
        (300, 220)
    };

    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            CLASS,
            TITLE,
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_CLIPCHILDREN,
            x,
            y,
            w,
            h,
            None,
            None,
            Some(instance),
            None,
        )
    }
    .ok()?;

    ui::apply_window_style(hwnd);
    ui::apply_window_icon(hwnd, instance);
    let win_dpi = unsafe { GetDpiForWindow(hwnd) };
    let dpi = if win_dpi == 0 { dpi } else { win_dpi };
    let font = make_font(dpi);

    let copy_text = platform::wide("复制版本与路径信息");
    let ok_text = platform::wide("关闭");
    let copy_btn = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            w!("BUTTON"),
            PCWSTR(copy_text.as_ptr()),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(BS_OWNERDRAW as u32),
            0,
            0,
            10,
            10,
            Some(hwnd),
            Some(HMENU(ID_COPY as isize as *mut core::ffi::c_void)),
            Some(instance),
            None,
        )
        .unwrap_or_default()
    };
    let ok_btn = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            w!("BUTTON"),
            PCWSTR(ok_text.as_ptr()),
            WS_CHILD
                | WS_VISIBLE
                | WS_TABSTOP
                | WINDOW_STYLE((BS_OWNERDRAW | BS_DEFPUSHBUTTON) as u32),
            0,
            0,
            10,
            10,
            Some(hwnd),
            Some(HMENU(ID_OK as isize as *mut core::ffi::c_void)),
            Some(instance),
            None,
        )
        .unwrap_or_default()
    };
    ui::install_button(copy_btn);
    ui::install_button(ok_btn);
    for btn in [copy_btn, ok_btn] {
        unsafe {
            let _ = SendMessageW(
                btn,
                WM_SETFONT,
                Some(WPARAM(font.0 as usize)),
                Some(LPARAM(1)),
            );
        }
    }

    let about = AboutUi {
        hwnd,
        copy_btn,
        ok_btn,
        font,
        palette,
        dpi,
        data_dir: platform::data_dir().display().to_string(),
        copied: false,
    };
    let mut client = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut client);
    }
    layout(&about, client.right, client.bottom);
    Some(about)
}

pub fn show() {
    let needs = ABOUT.with(|s| s.borrow().is_none());
    if needs {
        if let Some(about) = create() {
            ABOUT.with(|s| *s.borrow_mut() = Some(about));
        }
    }
    let hwnd = ABOUT.with(|s| s.borrow().as_ref().map(|a| a.hwnd));
    let Some(hwnd) = hwnd else {
        return;
    };
    ABOUT.with(|s| {
        if let Some(about) = s.borrow_mut().as_mut() {
            let palette = theme::current();
            if palette != about.palette {
                about.palette = palette;
                ui::apply_window_style(about.hwnd);
            }
            about.copied = false;
            about.data_dir = platform::data_dir().display().to_string();
        }
    });
    let ok_btn = ABOUT.with(|s| s.borrow().as_ref().map(|a| a.ok_btn));
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
        if let Some(ok_btn) = ok_btn {
            let _ = SetFocus(Some(ok_btn));
        }
        let _ = InvalidateRect(Some(hwnd), None, false);
        let _ = windows::Win32::Graphics::Gdi::UpdateWindow(hwnd);
    }
}

/// Re-read the system palette while the About window is open.
pub fn refresh_theme() {
    ABOUT.with(|slot| {
        let Ok(mut guard) = slot.try_borrow_mut() else {
            return;
        };
        let Some(about) = guard.as_mut() else {
            return;
        };
        let palette = theme::current();
        if palette != about.palette {
            about.palette = palette;
            ui::apply_window_style(about.hwnd);
            unsafe {
                let _ = InvalidateRect(Some(about.hwnd), None, true);
            }
        }
    });
}

pub fn show_test() -> bool {
    QUIT_ON_CLOSE.store(true, Ordering::Relaxed);
    show();
    ABOUT.with(|s| {
        s.borrow()
            .as_ref()
            .map(|a| !a.hwnd.0.is_null())
            .unwrap_or(false)
    })
}

/// Let Tab/Enter/Escape traverse the About buttons like a dialog.
pub fn is_dialog_message(message: &MSG) -> bool {
    let hwnd = ABOUT.with(|s| s.borrow().as_ref().map(|a| a.hwnd));
    let Some(hwnd) = hwnd else {
        return false;
    };
    if hwnd.0.is_null() || !unsafe { IsWindowVisible(hwnd).as_bool() } {
        return false;
    }
    let focus = unsafe { GetFocus() };
    if focus != hwnd && !unsafe { IsChild(hwnd, focus).as_bool() } {
        return false;
    }
    unsafe { IsDialogMessageW(hwnd, message).as_bool() }
}

pub fn shutdown() {
    let about = ABOUT.with(|s| s.borrow_mut().take());
    if let Some(about) = about {
        unsafe {
            let _ = DestroyWindow(about.hwnd);
            let _ = DeleteObject(HGDIOBJ(about.font.0));
        }
    }
}

fn paint_about(about: &AboutUi, hdc: HDC, client: RECT) {
    let dpi = about.dpi;
    let p = about.palette;
    let pad = scaled(24, dpi);
    let cx = client.right / 2;

    d2d::paint_buffered(hdc, client, |c| {
        c.clear(p.card);

        // Brand logo mark (60x60)
        let logo_sz = scaled(60, dpi);
        let logo = RECT {
            left: cx - logo_sz / 2,
            top: scaled(20, dpi),
            right: cx + logo_sz / 2,
            bottom: scaled(20, dpi) + logo_sz,
        };
        c.brand_logo(&logo, dpi);

        let title_r = RECT {
            left: pad,
            top: logo.bottom + scaled(14, dpi),
            right: client.right - pad,
            bottom: logo.bottom + scaled(40, dpi),
        };
        c.draw_text(
            "QuickPhrase 快语",
            &title_r,
            TextStyle::Display,
            dpi,
            p.heading,
            AlignH::Center,
            AlignV::Center,
        );

        let sub_r = RECT {
            left: pad,
            top: title_r.bottom + scaled(2, dpi),
            right: client.right - pad,
            bottom: title_r.bottom + scaled(22, dpi),
        };
        c.draw_text(
            "全局快捷短语输入器 · 极致轻量的原生效率工具",
            &sub_r,
            TextStyle::Caption,
            dpi,
            p.secondary,
            AlignH::Center,
            AlignV::Center,
        );

        // Tech & Version Pills
        let ver = format!("v{}", env!("CARGO_PKG_VERSION"));
        let pills = [
            (ver.as_str(), p.primary_soft, p.primary_text),
            ("Rust · Win32", p.field, p.secondary),
            ("Direct2D", p.field, p.secondary),
        ];
        let pill_h = scaled(22, dpi);
        let pill_y = sub_r.bottom + scaled(14, dpi);
        let pill_gap = scaled(6, dpi);
        let widths: Vec<i32> = pills
            .iter()
            .map(|(txt, _, _)| {
                let (tw, _) = c.measure_style(txt, TextStyle::Badge, dpi);
                tw.ceil() as i32 + scaled(18, dpi)
            })
            .collect();
        let total_w: i32 = widths.iter().sum::<i32>() + pill_gap * (pills.len() as i32 - 1);
        let mut px = cx - total_w / 2;
        for ((txt, bg, fg), w) in pills.iter().zip(widths.iter()) {
            let pr = RECT {
                left: px,
                top: pill_y,
                right: px + *w,
                bottom: pill_y + pill_h,
            };
            c.fill_round(&pr, (pill_h as f32) * 0.5, *bg);
            c.draw_text(
                txt,
                &pr,
                TextStyle::Badge,
                dpi,
                *fg,
                AlignH::Center,
                AlignV::Center,
            );
            px += *w + pill_gap;
        }

        // Data Directory Card
        let dir_card = RECT {
            left: pad,
            top: pill_y + pill_h + scaled(18, dpi),
            right: client.right - pad,
            bottom: pill_y + pill_h + scaled(18, dpi) + scaled(58, dpi),
        };
        c.fill_stroke_round(
            &dir_card,
            d2d::scaled_f(10.0, dpi),
            p.card_subtle,
            p.border,
            1.0,
        );
        let dlbl = RECT {
            left: dir_card.left + scaled(12, dpi),
            top: dir_card.top + scaled(8, dpi),
            right: dir_card.right - scaled(12, dpi),
            bottom: dir_card.top + scaled(24, dpi),
        };
        c.draw_text(
            if about.copied {
                "本地数据目录 · 已复制到剪贴板 ✓"
            } else {
                "本地数据目录"
            },
            &dlbl,
            TextStyle::Badge,
            dpi,
            if about.copied { p.success } else { p.muted },
            AlignH::Left,
            AlignV::Center,
        );
        let dval = RECT {
            left: dlbl.left,
            top: dlbl.bottom + scaled(2, dpi),
            right: dlbl.right,
            bottom: dir_card.bottom - scaled(8, dpi),
        };
        c.draw_text(
            &about.data_dir,
            &dval,
            TextStyle::Mono,
            dpi,
            p.secondary,
            AlignH::Left,
            AlignV::Center,
        );
    });
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_SIZE => {
            let width = (lparam.0 & 0xffff) as i32;
            let height = ((lparam.0 >> 16) & 0xffff) as i32;
            ABOUT.with(|s| {
                if let Some(about) = s.borrow().as_ref() {
                    layout(about, width, height);
                }
            });
            LRESULT(0)
        }
        WM_DPICHANGED => {
            let dpi = crate::ui::dpi_from_wparam(wparam);
            let suggested = &*(lparam.0 as *const RECT);
            let (left, top) = (suggested.left, suggested.top);
            let (width, height) = (
                suggested.right - suggested.left,
                suggested.bottom - suggested.top,
            );
            // Do not hold the ABOUT RefCell borrow across WM_SETFONT / MoveWindow:
            // owner-draw buttons can re-enter WM_DRAWITEM synchronously.
            let mut old_font = HFONT::default();
            let mut buttons = [HWND::default(); 2];
            let mut new_font = HFONT::default();
            ABOUT.with(|s| {
                if let Some(about) = s.borrow_mut().as_mut() {
                    old_font = about.font;
                    about.font = make_font(dpi);
                    about.dpi = dpi;
                    buttons = [about.copy_btn, about.ok_btn];
                    new_font = about.font;
                }
            });
            if !new_font.0.is_null() {
                for btn in buttons {
                    unsafe {
                        let _ = SendMessageW(
                            btn,
                            WM_SETFONT,
                            Some(WPARAM(new_font.0 as usize)),
                            Some(LPARAM(1)),
                        );
                    }
                }
            }
            ABOUT.with(|s| {
                if let Some(about) = s.borrow().as_ref() {
                    layout(about, width, height);
                }
            });
            if !old_font.0.is_null() {
                unsafe {
                    let _ = DeleteObject(HGDIOBJ(old_font.0));
                }
            }
            unsafe {
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    left,
                    top,
                    width,
                    height,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            let mut client = RECT::default();
            let _ = GetClientRect(hwnd, &mut client);
            ABOUT.with(|s| {
                if let Some(about) = s.borrow().as_ref() {
                    paint_about(about, hdc, client);
                }
            });
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        WM_DRAWITEM => {
            let draw = &*(lparam.0 as *const DRAWITEMSTRUCT);
            ABOUT.with(|s| {
                if let Some(about) = s.borrow().as_ref() {
                    let kind = if draw.CtlID as i32 == ID_OK {
                        ui::ButtonKind::Primary
                    } else {
                        ui::ButtonKind::Secondary
                    };
                    ui::draw_button(draw, about.palette, kind, about.dpi, about.palette.card);
                }
            });
            LRESULT(1)
        }
        WM_COMMAND => {
            let id = (wparam.0 & 0xffff) as i32;
            if id == ID_OK || id == IDCANCEL.0 {
                let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
            } else if id == ID_COPY {
                ABOUT.with(|s| {
                    if let Some(about) = s.borrow_mut().as_mut() {
                        let summary = format!(
                            "QuickPhrase {}\nDataDir: {}",
                            env!("CARGO_PKG_VERSION"),
                            about.data_dir
                        );
                        about.copied = crate::clipboard::set_text_with_snapshot(hwnd, &summary).0;
                        let _ = InvalidateRect(Some(hwnd), None, false);
                    }
                });
            }
            LRESULT(0)
        }
        WM_KEYDOWN if wparam.0 as u16 == VK_ESCAPE.0 => {
            let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
            LRESULT(0)
        }
        WM_CLOSE => {
            if QUIT_ON_CLOSE.load(Ordering::Relaxed) {
                let _ = DestroyWindow(hwnd);
            } else {
                let _ = ShowWindow(hwnd, SW_HIDE);
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            ABOUT.with(|s| {
                if let Some(about) = s.borrow_mut().take() {
                    let _ = DeleteObject(HGDIOBJ(about.font.0));
                }
            });
            if QUIT_ON_CLOSE.load(Ordering::Relaxed) {
                PostQuitMessage(0);
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}
