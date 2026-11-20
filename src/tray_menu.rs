//! QuickPhrase 2.0 custom Direct2D Fluent tray popover menu.

use std::cell::RefCell;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{
    GetLastError, ERROR_CLASS_ALREADY_EXISTS, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, EndPaint, GetMonitorInfoW, InvalidateRect, MonitorFromPoint, HBRUSH, HDC,
    MONITORINFO, MONITOR_DEFAULTTONEAREST, PAINTSTRUCT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT, VK_DOWN, VK_ESCAPE, VK_RETURN, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::d2d::{self, AlignH, AlignV, Icon, TextStyle};
use crate::{logging, theme, ui};

const CLASS: PCWSTR = w!("QuickPhrase.TrayMenu");
const MENU_WIDTH: i32 = 274;
const HEADER_HEIGHT: i32 = 42;
const ROW_HEIGHT: i32 = 34;
const SEP_HEIGHT: i32 = 9;
const PAD: i32 = 8;

#[derive(Clone)]
pub struct MenuConfig {
    pub owner: HWND,
    pub hotkey: String,
    pub paused: bool,
    pub hotkey_ok: bool,
    pub cmd_open: u32,
    pub cmd_manage: u32,
    pub cmd_quick_add: u32,
    pub cmd_pause: u32,
    pub cmd_settings: u32,
    pub cmd_restore_clip: u32,
    pub cmd_about: u32,
    pub cmd_exit: u32,
}

#[derive(Clone)]
enum Entry {
    Item {
        id: u32,
        icon: Icon,
        label: String,
        hint: String,
        accent: bool,
        danger: bool,
    },
    Separator,
}

struct TrayMenu {
    hwnd: HWND,
    owner: HWND,
    hotkey: String,
    paused: bool,
    hotkey_ok: bool,
    entries: Vec<Entry>,
    item_rects: Vec<RECT>,
    hovered: Option<usize>,
    dpi: u32,
    palette: theme::Palette,
    tracking: bool,
}

thread_local! {
    static MENU: RefCell<Option<TrayMenu>> = const { RefCell::new(None) };
}

fn scaled(value: i32, dpi: u32) -> i32 {
    (value * dpi as i32 + 48) / 96
}

fn build_entries(cfg: &MenuConfig) -> Vec<Entry> {
    let (pause_icon, pause_label) = if cfg.paused {
        (Icon::Play, "恢复快捷键")
    } else if !cfg.hotkey_ok {
        (Icon::Play, "重试注册快捷键")
    } else {
        (Icon::Pause, "暂停快捷键")
    };
    vec![
        Entry::Item {
            id: cfg.cmd_open,
            icon: Icon::Bolt,
            label: "打开快捷面板".to_string(),
            hint: "唤起".to_string(),
            accent: true,
            danger: false,
        },
        Entry::Item {
            id: cfg.cmd_manage,
            icon: Icon::Grid,
            label: "管理短语".to_string(),
            hint: "双击托盘".to_string(),
            accent: false,
            danger: false,
        },
        Entry::Item {
            id: cfg.cmd_quick_add,
            icon: Icon::Plus,
            label: "新增短语".to_string(),
            hint: "快速添加".to_string(),
            accent: false,
            danger: false,
        },
        Entry::Item {
            id: cfg.cmd_pause,
            icon: pause_icon,
            label: pause_label.to_string(),
            hint: String::new(),
            accent: false,
            danger: false,
        },
        Entry::Item {
            id: cfg.cmd_settings,
            icon: Icon::Gear,
            label: "偏好设置".to_string(),
            hint: String::new(),
            accent: false,
            danger: false,
        },
        Entry::Item {
            id: cfg.cmd_restore_clip,
            icon: Icon::Restore,
            label: "恢复上次剪贴板内容".to_string(),
            hint: String::new(),
            accent: false,
            danger: false,
        },
        Entry::Separator,
        Entry::Item {
            id: cfg.cmd_about,
            icon: Icon::Info,
            label: "关于 QuickPhrase".to_string(),
            hint: format!("v{}", env!("CARGO_PKG_VERSION")),
            accent: false,
            danger: false,
        },
        Entry::Item {
            id: cfg.cmd_exit,
            icon: Icon::Exit,
            label: "退出程序".to_string(),
            hint: String::new(),
            accent: false,
            danger: true,
        },
    ]
}

fn layout_entries(entries: &[Entry], dpi: u32, width: i32) -> (Vec<RECT>, i32) {
    let pad = scaled(PAD, dpi);
    let mut y = pad + scaled(HEADER_HEIGHT, dpi) + scaled(6, dpi);
    let mut rects = Vec::with_capacity(entries.len());
    for entry in entries {
        match entry {
            Entry::Item { .. } => {
                let h = scaled(ROW_HEIGHT, dpi);
                rects.push(RECT {
                    left: pad,
                    top: y,
                    right: width - pad,
                    bottom: y + h,
                });
                y += h + scaled(2, dpi);
            }
            Entry::Separator => {
                let h = scaled(SEP_HEIGHT, dpi);
                rects.push(RECT {
                    left: pad + scaled(6, dpi),
                    top: y,
                    right: width - pad - scaled(6, dpi),
                    bottom: y + h,
                });
                y += h;
            }
        }
    }
    (rects, y + pad)
}

fn ensure_window() -> Option<HWND> {
    if let Some(hwnd) = MENU.with(|s| s.borrow().as_ref().map(|m| m.hwnd)) {
        if !hwnd.0.is_null() && unsafe { IsWindow(Some(hwnd)).as_bool() } {
            return Some(hwnd);
        }
        // The popup may have been destroyed (e.g. Alt+F4). Drop the stale
        // state before creating a replacement window.
        MENU.with(|slot| {
            slot.borrow_mut().take();
        });
    }
    let module = unsafe { GetModuleHandleW(None) }.ok()?;
    let instance = HINSTANCE(module.0);
    let class = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(window_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: instance,
        hIcon: HICON::default(),
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW) }.unwrap_or_default(),
        hbrBackground: HBRUSH::default(),
        lpszMenuName: PCWSTR::null(),
        lpszClassName: CLASS,
    };
    if unsafe { RegisterClassW(&class) } == 0 {
        let err = unsafe { GetLastError() };
        if err != ERROR_CLASS_ALREADY_EXISTS {
            logging::error(&format!("tray_menu: RegisterClassW failed: {err:?}"));
            return None;
        }
    }
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            CLASS,
            w!("QuickPhrase Menu"),
            WS_POPUP,
            0,
            0,
            MENU_WIDTH,
            300,
            None,
            None,
            Some(instance),
            None,
        )
    }
    .ok()?;
    Some(hwnd)
}

pub fn show(cfg: MenuConfig) {
    let Some(hwnd) = ensure_window() else {
        return;
    };
    let mut pt = POINT { x: 0, y: 0 };
    unsafe {
        let _ = GetCursorPos(&mut pt);
    }
    let monitor = unsafe { MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST) };
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    let work = if unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
        info.rcWork
    } else {
        RECT {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        }
    };
    let mut dpi_x = 96u32;
    let mut dpi_y = 96u32;
    if unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) }.is_err()
        || dpi_x == 0
    {
        dpi_x = 96;
    }

    let palette = theme::current();
    ui::apply_popup_style(hwnd, palette);

    let entries = build_entries(&cfg);
    let width = scaled(MENU_WIDTH, dpi_x);
    let (item_rects, height) = layout_entries(&entries, dpi_x, width);

    let margin = scaled(8, dpi_x);
    let mut x = pt.x - width + scaled(24, dpi_x);
    let mut y = pt.y - height - scaled(8, dpi_x);
    if y < work.top + margin {
        y = pt.y + scaled(8, dpi_x);
    }
    x = x.clamp(
        work.left + margin,
        (work.right - width - margin).max(work.left + margin),
    );
    y = y.clamp(
        work.top + margin,
        (work.bottom - height - margin).max(work.top + margin),
    );

    MENU.with(|slot| {
        *slot.borrow_mut() = Some(TrayMenu {
            hwnd,
            owner: cfg.owner,
            hotkey: cfg.hotkey,
            paused: cfg.paused,
            hotkey_ok: cfg.hotkey_ok,
            entries,
            item_rects,
            hovered: None,
            dpi: dpi_x,
            palette,
            tracking: false,
        });
    });

    unsafe {
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            x,
            y,
            width,
            height,
            SWP_NOACTIVATE,
        );
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
        let _ = InvalidateRect(Some(hwnd), None, false);
    }
}

pub fn shutdown() {
    let hwnd = MENU.with(|s| s.borrow_mut().take().map(|m| m.hwnd));
    if let Some(hwnd) = hwnd {
        if !hwnd.0.is_null() {
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
        }
    }
}

fn paint_menu(menu: &TrayMenu, hdc: HDC, client: RECT) {
    let dpi = menu.dpi;
    let p = menu.palette;
    let pad = scaled(PAD, dpi);

    d2d::paint_buffered(hdc, client, |c| {
        c.clear(p.card);
        c.stroke_round(&client, d2d::scaled_f(12.0, dpi), p.border_strong, 1.0);

        // Status Header Pill
        let header = RECT {
            left: pad,
            top: pad,
            right: client.right - pad,
            bottom: pad + scaled(HEADER_HEIGHT, dpi),
        };
        c.fill_round(&header, d2d::scaled_f(9.0, dpi), p.card_subtle);

        let (dot_color, status_label) = if menu.paused {
            (p.warning, "已暂停")
        } else if !menu.hotkey_ok {
            (p.danger, "热键冲突")
        } else {
            (p.success, "运行中")
        };
        let logo_sz = scaled(22, dpi);
        let logo_rect = RECT {
            left: header.left + scaled(8, dpi),
            top: header.top + ((header.bottom - header.top) - logo_sz) / 2,
            right: header.left + scaled(8, dpi) + logo_sz,
            bottom: header.top + ((header.bottom - header.top) + logo_sz) / 2,
        };
        c.brand_logo(&logo_rect, dpi);

        // Status indicator dot at bottom-right corner of the mini logo
        let dot_r = d2d::scaled_f(3.5, dpi);
        let dot_cx = logo_rect.right as f32 - d2d::scaled_f(1.5, dpi);
        let dot_cy = logo_rect.bottom as f32 - d2d::scaled_f(1.5, dpi);
        c.fill_circle(
            dot_cx,
            dot_cy,
            dot_r + d2d::scaled_f(1.2, dpi),
            p.card_subtle,
        );
        c.fill_circle(dot_cx, dot_cy, dot_r, dot_color);

        let mut lbl_right = header.right - scaled(8, dpi);
        if !menu.hotkey.is_empty() {
            let (kw, _) = c.measure_style(&menu.hotkey, TextStyle::Mono, dpi);
            let kbd_w = (kw.ceil() as i32 + scaled(10, dpi)).min(scaled(96, dpi));
            let kbd_h = scaled(22, dpi);
            let krect = RECT {
                left: header.right - scaled(8, dpi) - kbd_w,
                top: header.top + ((header.bottom - header.top) - kbd_h) / 2,
                right: header.right - scaled(8, dpi),
                bottom: header.top + ((header.bottom - header.top) + kbd_h) / 2,
            };
            c.kbd(&menu.hotkey, &krect, p, dpi);
            lbl_right = krect.left - scaled(6, dpi);
        }

        let title_str = format!("QuickPhrase · {status_label}");
        let lbl = RECT {
            left: logo_rect.right + scaled(8, dpi),
            top: header.top,
            right: lbl_right.max(logo_rect.right + scaled(32, dpi)),
            bottom: header.bottom,
        };
        c.draw_text(
            &title_str,
            &lbl,
            TextStyle::Label,
            dpi,
            p.heading,
            AlignH::Left,
            AlignV::Center,
        );

        // Entries
        for (idx, (entry, rect)) in menu.entries.iter().zip(menu.item_rects.iter()).enumerate() {
            match entry {
                Entry::Separator => {
                    let mid_y = (rect.top + rect.bottom) as f32 * 0.5;
                    c.line(
                        rect.left as f32,
                        mid_y,
                        rect.right as f32,
                        mid_y,
                        p.divider,
                        1.0,
                    );
                }
                Entry::Item {
                    icon,
                    label,
                    hint,
                    accent,
                    danger,
                    ..
                } => {
                    let hovered = menu.hovered == Some(idx);
                    if hovered {
                        let bg = if *danger { p.danger_soft } else { p.hover };
                        c.fill_round(rect, d2d::scaled_f(8.0, dpi), bg);
                    }
                    let icon_color = if *danger {
                        p.danger
                    } else if *accent {
                        p.primary
                    } else {
                        p.secondary
                    };
                    let text_color = if *danger { p.danger } else { p.heading };
                    let rcy = (rect.top + rect.bottom) as f32 * 0.5;
                    c.icon(
                        *icon,
                        (rect.left + scaled(18, dpi)) as f32,
                        rcy,
                        d2d::scaled_f(14.0, dpi),
                        icon_color,
                    );
                    let mut text_right = rect.right - scaled(10, dpi);
                    if !hint.is_empty() {
                        let (hw, _) = c.measure_style(hint, TextStyle::Badge, dpi);
                        let hw_i = hw.ceil() as i32 + scaled(6, dpi);
                        let hrect = RECT {
                            left: (rect.right - scaled(10, dpi) - hw_i)
                                .max(rect.left + scaled(120, dpi)),
                            top: rect.top,
                            right: rect.right - scaled(10, dpi),
                            bottom: rect.bottom,
                        };
                        c.draw_text(
                            hint,
                            &hrect,
                            TextStyle::Badge,
                            dpi,
                            p.muted,
                            AlignH::Right,
                            AlignV::Center,
                        );
                        text_right = hrect.left - scaled(6, dpi);
                    }
                    let lrect = RECT {
                        left: rect.left + scaled(36, dpi),
                        top: rect.top,
                        right: text_right.max(rect.left + scaled(50, dpi)),
                        bottom: rect.bottom,
                    };
                    c.draw_text(
                        label,
                        &lrect,
                        if *accent {
                            TextStyle::Heading
                        } else {
                            TextStyle::Body
                        },
                        dpi,
                        text_color,
                        AlignH::Left,
                        AlignV::Center,
                    );
                }
            }
        }
    });
}

fn activate_entry(idx: usize) {
    let info = MENU.with(|slot| {
        let guard = slot.borrow();
        let menu = guard.as_ref()?;
        match menu.entries.get(idx) {
            Some(Entry::Item { id, .. }) => Some((menu.hwnd, menu.owner, *id)),
            _ => None,
        }
    });
    if let Some((hwnd, owner, id)) = info {
        unsafe {
            let _ = ShowWindow(hwnd, SW_HIDE);
            if !owner.0.is_null() {
                let _ = PostMessageW(Some(owner), WM_COMMAND, WPARAM(id as usize), LPARAM(0));
            }
        }
    }
}

fn step_selection(delta: i32) {
    let hwnd = MENU.with(|slot| {
        let mut guard = slot.borrow_mut();
        let menu = guard.as_mut()?;
        let selectable: Vec<usize> = menu
            .entries
            .iter()
            .enumerate()
            .filter_map(|(i, e)| matches!(e, Entry::Item { .. }).then_some(i))
            .collect();
        if selectable.is_empty() {
            return None;
        }
        let next = match menu
            .hovered
            .and_then(|cur| selectable.iter().position(|&i| i == cur))
        {
            Some(pos) => {
                let len = selectable.len() as i32;
                let np = (pos as i32 + delta).rem_euclid(len) as usize;
                selectable[np]
            }
            None => {
                if delta >= 0 {
                    selectable[0]
                } else {
                    *selectable.last().unwrap()
                }
            }
        };
        menu.hovered = Some(next);
        Some(menu.hwnd)
    });
    if let Some(hwnd) = hwnd {
        unsafe {
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            let mut client = RECT::default();
            let _ = GetClientRect(hwnd, &mut client);
            MENU.with(|slot| {
                if let Some(menu) = slot.borrow().as_ref() {
                    paint_menu(menu, hdc, client);
                }
            });
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let x = (lparam.0 & 0xffff) as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xffff) as i16 as i32;
            let redraw = MENU.with(|slot| {
                let mut guard = slot.borrow_mut();
                let Some(menu) = guard.as_mut() else {
                    return false;
                };
                if !menu.tracking {
                    menu.tracking = true;
                    let mut track = TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    let _ = TrackMouseEvent(&mut track);
                }
                let next = menu
                    .entries
                    .iter()
                    .zip(menu.item_rects.iter())
                    .enumerate()
                    .find_map(|(i, (entry, r))| {
                        if matches!(entry, Entry::Item { .. })
                            && x >= r.left
                            && x <= r.right
                            && y >= r.top
                            && y <= r.bottom
                        {
                            Some(i)
                        } else {
                            None
                        }
                    });
                if next != menu.hovered {
                    menu.hovered = next;
                    true
                } else {
                    false
                }
            });
            if redraw {
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            let redraw = MENU.with(|slot| {
                let mut guard = slot.borrow_mut();
                let Some(menu) = guard.as_mut() else {
                    return false;
                };
                menu.tracking = false;
                menu.hovered.take().is_some()
            });
            if redraw {
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let x = (lparam.0 & 0xffff) as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xffff) as i16 as i32;
            let hit = MENU.with(|slot| {
                let guard = slot.borrow();
                let menu = guard.as_ref()?;
                menu.entries
                    .iter()
                    .zip(menu.item_rects.iter())
                    .enumerate()
                    .find_map(|(i, (entry, r))| {
                        if matches!(entry, Entry::Item { .. })
                            && x >= r.left
                            && x <= r.right
                            && y >= r.top
                            && y <= r.bottom
                        {
                            Some(i)
                        } else {
                            None
                        }
                    })
            });
            if let Some(idx) = hit {
                activate_entry(idx);
            }
            LRESULT(0)
        }
        WM_KEYDOWN => {
            let vk = wparam.0 as u16;
            if vk == VK_ESCAPE.0 {
                let _ = ShowWindow(hwnd, SW_HIDE);
            } else if vk == VK_DOWN.0 {
                step_selection(1);
            } else if vk == VK_UP.0 {
                step_selection(-1);
            } else if vk == VK_RETURN.0 {
                if let Some(idx) = MENU.with(|s| s.borrow().as_ref().and_then(|m| m.hovered)) {
                    activate_entry(idx);
                }
            }
            LRESULT(0)
        }
        WM_ACTIVATE => {
            if (wparam.0 & 0xffff) as u32 == WA_INACTIVE {
                let _ = ShowWindow(hwnd, SW_HIDE);
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            MENU.with(|slot| {
                if let Ok(mut guard) = slot.try_borrow_mut() {
                    if guard.as_ref().is_some_and(|menu| menu.hwnd == hwnd) {
                        guard.take();
                    }
                }
            });
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}
