use std::cell::RefCell;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::time::Instant;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateFontW, CreateSolidBrush, DeleteObject, DrawTextW, EndPaint, FillRect,
    FrameRect, GetMonitorInfoW, MonitorFromPoint, SelectObject, SetBkColor, SetBkMode,
    SetTextColor, UpdateWindow, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET,
    DEFAULT_PITCH, DT_END_ELLIPSIS, DT_LEFT, DT_NOPREFIX, DT_RIGHT, DT_SINGLELINE, HBRUSH, HDC,
    HFONT, HGDIOBJ, MONITORINFO, MONITOR_DEFAULTTONEAREST, OUT_DEFAULT_PRECIS, PAINTSTRUCT,
    TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::Controls::{DRAWITEMSTRUCT, ODS_SELECTED};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, GetDpiForWindow, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SetFocus, VK_DOWN, VK_END, VK_ESCAPE, VK_HOME, VK_RETURN, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::{logging, platform};

const PANEL_CLASS: PCWSTR = w!("QuickPhrase.Panel");
const EDIT_ID: i32 = 100;
const LIST_ID: i32 = 101;
const FOOTER_ID: i32 = 102;

const PANEL_WIDTH: i32 = 520;
const PANEL_HEIGHT: i32 = 420;
const ITEM_HEIGHT: i32 = 64;

const LB_ADDSTRING: u32 = 0x0180;
const LB_RESETCONTENT: u32 = 0x0184;
const LB_SETCURSEL: u32 = 0x0186;
const LB_GETCURSEL: u32 = 0x0188;
const LB_GETCOUNT: u32 = 0x018B;
const LB_SETITEMHEIGHT: u32 = 0x01A0;
const EM_SETSEL: u32 = 0x00B1;
const EM_SETCUEBANNER: u32 = 0x1501;

struct Item {
    title: &'static str,
    content: &'static str,
    category: &'static str,
}

const ITEMS: &[Item] = &[
    Item {
        title: "项目进度回复",
        content: "您好，目前该功能已完成开发，正在进行测试验证，预计测试通过后安排发布。",
        category: "工作沟通",
    },
    Item {
        title: "收到回复",
        content: "好的，已收到，我会尽快处理。",
        category: "工作沟通",
    },
    Item {
        title: "接口开发说明",
        content: "接口采用 RESTful 风格，基于 HTTP 协议，返回格式为 JSON，具体参数如下：",
        category: "开发相关",
    },
];

struct Fonts {
    title: HFONT,
    body: HFONT,
    small: HFONT,
}

struct Brushes {
    background: HBRUSH,
    selected: HBRUSH,
    search: HBRUSH,
    border: HBRUSH,
}

struct Panel {
    hwnd: HWND,
    edit: HWND,
    list: HWND,
    footer: HWND,
    filtered: Vec<usize>,
    target: HWND,
    caret: Option<RECT>,
    summary: String,
    fonts: Fonts,
    brushes: Brushes,
    dpi: u32,
    hiding: bool,
    quit_on_hide: bool,
}

thread_local! {
    static PANEL: RefCell<Option<Panel>> = const { RefCell::new(None) };
}

static OLD_EDIT_PROC: AtomicIsize = AtomicIsize::new(0);

fn rgb(r: u32, g: u32, b: u32) -> COLORREF {
    COLORREF(r | (g << 8) | (b << 16))
}

fn scaled(value: i32, dpi: u32) -> i32 {
    value * dpi as i32 / 96
}

fn create_font(size: i32, weight: i32, dpi: u32) -> HFONT {
    let face = platform::wide("Microsoft YaHei UI");
    unsafe {
        CreateFontW(
            -scaled(size, dpi),
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

unsafe fn send(hwnd: HWND, message: u32, wparam: usize, lparam: isize) -> LRESULT {
    SendMessageW(hwnd, message, Some(WPARAM(wparam)), Some(LPARAM(lparam)))
}

fn make_fonts(dpi: u32) -> Fonts {
    Fonts {
        title: create_font(14, 700, dpi),
        body: create_font(12, 400, dpi),
        small: create_font(11, 400, dpi),
    }
}

fn destroy_fonts(fonts: &Fonts) {
    unsafe {
        let _ = DeleteObject(HGDIOBJ(fonts.title.0));
        let _ = DeleteObject(HGDIOBJ(fonts.body.0));
        let _ = DeleteObject(HGDIOBJ(fonts.small.0));
    }
}

fn make_brushes() -> Brushes {
    Brushes {
        background: unsafe { CreateSolidBrush(rgb(255, 255, 255)) },
        selected: unsafe { CreateSolidBrush(rgb(240, 237, 255)) },
        search: unsafe { CreateSolidBrush(rgb(248, 248, 253)) },
        border: unsafe { CreateSolidBrush(rgb(233, 235, 243)) },
    }
}

fn destroy_brushes(brushes: &Brushes) {
    unsafe {
        let _ = DeleteObject(HGDIOBJ(brushes.background.0));
        let _ = DeleteObject(HGDIOBJ(brushes.selected.0));
        let _ = DeleteObject(HGDIOBJ(brushes.search.0));
        let _ = DeleteObject(HGDIOBJ(brushes.border.0));
    }
}

pub fn is_visible() -> bool {
    PANEL.with(|slot| {
        slot.borrow()
            .as_ref()
            .map(|panel| unsafe { IsWindowVisible(panel.hwnd).as_bool() })
            .unwrap_or(false)
    })
}

pub fn show(target: HWND, caret: Option<RECT>, summary: &str) -> u128 {
    present(target, caret, summary, true, false)
}

pub fn show_test() -> u128 {
    present(HWND::default(), None, "panel test", true, true)
}

fn present(
    target: HWND,
    caret: Option<RECT>,
    summary: &str,
    activate: bool,
    quit_on_hide: bool,
) -> u128 {
    let needs_create = PANEL.with(|slot| slot.borrow().is_none());
    if needs_create {
        if let Some(panel) = create() {
            PANEL.with(|slot| *slot.borrow_mut() = Some(panel));
        }
    }

    let (hwnd, edit) = PANEL.with(|slot| {
        let mut guard = slot.borrow_mut();
        let Some(panel) = guard.as_mut() else {
            return (HWND::default(), HWND::default());
        };
        panel.target = target;
        panel.caret = caret;
        panel.summary = summary.to_string();
        panel.hiding = false;
        panel.quit_on_hide = quit_on_hide;
        (panel.hwnd, panel.edit)
    });

    if hwnd.0.is_null() {
        return 0;
    }

    place();
    apply_rounding(hwnd);
    PANEL.with(|slot| {
        if let Some(panel) = slot.borrow_mut().as_mut() {
            populate(panel, "");
        }
    });

    let started = Instant::now();
    unsafe {
        let _ = ShowWindow(hwnd, if activate { SW_SHOW } else { SW_SHOWNOACTIVATE });
        if activate {
            force_foreground(hwnd);
            let _ = SetFocus(Some(edit));
            let _ = send(edit, EM_SETSEL, 0, -1);
            if GetForegroundWindow() != hwnd {
                logging::warn("panel is visible but could not take foreground");
            }
        }
        let _ = UpdateWindow(hwnd);
    }
    started.elapsed().as_micros()
}

pub fn hide_and_restore() {
    hide(true);
}

pub fn hide_only() {
    hide(false);
}

fn hide(restore: bool) {
    let (hwnd, target, quit) = PANEL.with(|slot| {
        let mut guard = slot.borrow_mut();
        let Some(panel) = guard.as_mut() else {
            return (HWND::default(), HWND::default(), false);
        };
        panel.hiding = true;
        (panel.hwnd, panel.target, panel.quit_on_hide)
    });

    if !hwnd.0.is_null() {
        unsafe {
            let _ = ShowWindow(hwnd, SW_HIDE);
        }
    }
    if restore && !target.0.is_null() {
        unsafe {
            let _ = SetForegroundWindow(target);
        }
    }

    PANEL.with(|slot| {
        if let Some(panel) = slot.borrow_mut().as_mut() {
            panel.hiding = false;
        }
    });

    if quit {
        unsafe { PostQuitMessage(0) };
    }
}

fn create() -> Option<Panel> {
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
        lpszClassName: PANEL_CLASS,
    };
    unsafe {
        let _ = RegisterClassW(&class);
    }

    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            PANEL_CLASS,
            w!("QuickPhrase"),
            WS_POPUP,
            0,
            0,
            PANEL_WIDTH,
            PANEL_HEIGHT,
            None,
            None,
            Some(instance),
            None,
        )
    }
    .ok()?;

    let dpi = unsafe { GetDpiForWindow(hwnd) };
    let dpi = if dpi == 0 { 96 } else { dpi };
    let fonts = make_fonts(dpi);
    let brushes = make_brushes();

    let edit = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            w!("EDIT"),
            w!(""),
            WS_CHILD | WS_VISIBLE | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
            0,
            0,
            10,
            10,
            Some(hwnd),
            Some(HMENU(EDIT_ID as isize as *mut core::ffi::c_void)),
            Some(instance),
            None,
        )
    }
    .ok()?;

    let list = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            w!("LISTBOX"),
            w!(""),
            WS_CHILD
                | WS_VISIBLE
                | WS_VSCROLL
                | WINDOW_STYLE(
                    (LBS_OWNERDRAWFIXED | LBS_HASSTRINGS | LBS_NOTIFY | LBS_NOINTEGRALHEIGHT)
                        as u32,
                ),
            0,
            0,
            10,
            10,
            Some(hwnd),
            Some(HMENU(LIST_ID as isize as *mut core::ffi::c_void)),
            Some(instance),
            None,
        )
    }
    .ok()?;

    let footer = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            w!("STATIC"),
            w!("↑↓ 选择 · Enter 输入 · Esc 关闭"),
            WS_CHILD | WS_VISIBLE,
            0,
            0,
            10,
            10,
            Some(hwnd),
            Some(HMENU(FOOTER_ID as isize as *mut core::ffi::c_void)),
            Some(instance),
            None,
        )
    }
    .ok()?;

    for control in [edit, list, footer] {
        unsafe {
            let _ = send(control, WM_SETFONT, fonts.body.0 as usize, 1);
        }
    }

    let cue = platform::wide("搜索短语…");
    unsafe {
        let _ = send(edit, EM_SETCUEBANNER, 1, cue.as_ptr() as isize);
        let old = SetWindowLongPtrW(edit, GWLP_WNDPROC, edit_proc as *const () as usize as isize);
        OLD_EDIT_PROC.store(old, Ordering::Relaxed);
        let _ = send(list, LB_SETITEMHEIGHT, 0, scaled(ITEM_HEIGHT, dpi) as isize);
    }

    let mut panel = Panel {
        hwnd,
        edit,
        list,
        footer,
        filtered: Vec::new(),
        target: HWND::default(),
        caret: None,
        summary: String::new(),
        fonts,
        brushes,
        dpi,
        hiding: false,
        quit_on_hide: false,
    };

    let mut rect = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut rect);
    }
    panel.layout(rect.right, rect.bottom);

    Some(panel)
}

fn commit() {
    let info = PANEL.with(|slot| {
        let guard = slot.borrow();
        let panel = guard.as_ref()?;
        let index = panel.selected_index()?;
        let item_index = *panel.filtered.get(index)?;
        let item = &ITEMS[item_index];
        Some((item.title, item.category, panel.summary.clone()))
    });

    if let Some((title, category, summary)) = info {
        logging::info(&format!(
            "panel commit: title={title} category={category} (M0-4 will insert)"
        ));
        logging::info(&format!("panel capture context: {summary}"));
    }

    hide_and_restore();
}

fn place() {
    let (hwnd, caret, old_dpi) = PANEL.with(|slot| match slot.borrow().as_ref() {
        Some(panel) => (panel.hwnd, panel.caret, panel.dpi),
        None => (HWND::default(), None, 96),
    });
    if hwnd.0.is_null() {
        return;
    }

    let mut anchor = POINT { x: 0, y: 0 };
    if let Some(caret) = caret {
        anchor = POINT {
            x: caret.left,
            y: caret.bottom,
        };
    } else {
        unsafe {
            let _ = GetCursorPos(&mut anchor);
        }
    }

    let monitor = unsafe { MonitorFromPoint(anchor, MONITOR_DEFAULTTONEAREST) };
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

    let mut dpi = old_dpi;
    let mut dpi_x = 0u32;
    let mut dpi_y = 0u32;
    if unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) }.is_ok()
        && dpi_x != 0
    {
        dpi = dpi_x;
    }

    if dpi != old_dpi {
        let (edit, list, footer, font) = PANEL.with(|slot| {
            let mut guard = slot.borrow_mut();
            let Some(panel) = guard.as_mut() else {
                return (
                    HWND::default(),
                    HWND::default(),
                    HWND::default(),
                    HFONT::default(),
                );
            };
            destroy_fonts(&panel.fonts);
            panel.fonts = make_fonts(dpi);
            panel.dpi = dpi;
            (panel.edit, panel.list, panel.footer, panel.fonts.body)
        });
        unsafe {
            for control in [edit, list, footer] {
                let _ = send(control, WM_SETFONT, font.0 as usize, 1);
            }
            let _ = send(list, LB_SETITEMHEIGHT, 0, scaled(ITEM_HEIGHT, dpi) as isize);
        }
    }

    let width = scaled(PANEL_WIDTH, dpi);
    let height = scaled(PANEL_HEIGHT, dpi);
    let mut x = anchor.x;
    let mut y = anchor.y + scaled(8, dpi);

    if y + height > work.bottom {
        y = anchor.y - height - scaled(ITEM_HEIGHT, dpi);
    }

    x = x.clamp(
        work.left + scaled(8, dpi),
        (work.right - width - scaled(8, dpi)).max(work.left),
    );
    y = y.clamp(
        work.top + scaled(8, dpi),
        (work.bottom - height - scaled(8, dpi)).max(work.top),
    );

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
    }
}

impl Panel {
    fn layout(&mut self, width: i32, height: i32) {
        if width <= 0 || height <= 0 {
            return;
        }
        let margin = scaled(16, self.dpi);
        let edit_height = scaled(34, self.dpi);
        let footer_height = scaled(22, self.dpi);
        let gap = scaled(10, self.dpi);

        unsafe {
            let _ = MoveWindow(
                self.edit,
                margin,
                scaled(14, self.dpi),
                width - margin * 2,
                edit_height,
                true,
            );
            let list_top = scaled(14, self.dpi) + edit_height + gap;
            let list_height = height - list_top - footer_height - scaled(12, self.dpi);
            let _ = MoveWindow(
                self.list,
                scaled(10, self.dpi),
                list_top,
                width - scaled(20, self.dpi),
                list_height,
                true,
            );
            let _ = MoveWindow(
                self.footer,
                margin,
                height - footer_height - scaled(8, self.dpi),
                width - margin * 2,
                footer_height,
                true,
            );
        }
    }

    fn selected_index(&self) -> Option<usize> {
        let index = unsafe { send(self.list, LB_GETCURSEL, 0, 0) }.0;
        if index < 0 {
            None
        } else {
            Some(index as usize)
        }
    }

    fn move_selection(&mut self, delta: i32) {
        let count = unsafe { send(self.list, LB_GETCOUNT, 0, 0) }.0 as i32;
        if count <= 0 {
            return;
        }
        let current = self.selected_index().unwrap_or(0) as i32;
        let next = (current + delta).clamp(0, count - 1);
        unsafe {
            let _ = send(self.list, LB_SETCURSEL, next as usize, 0);
        }
    }
}

fn force_foreground(hwnd: HWND) {
    unsafe {
        let _ = SetForegroundWindow(hwnd);
        if GetForegroundWindow() == hwnd {
            return;
        }

        let foreground = GetForegroundWindow();
        if foreground.0.is_null() {
            return;
        }

        let mut pid = 0u32;
        let foreground_thread = GetWindowThreadProcessId(foreground, Some(&mut pid));
        let current_thread = GetCurrentThreadId();
        if foreground_thread != 0 && foreground_thread != current_thread {
            let _ = AttachThreadInput(current_thread, foreground_thread, true);
            let _ = SetForegroundWindow(hwnd);
            let _ = SetFocus(Some(hwnd));
            let _ = AttachThreadInput(current_thread, foreground_thread, false);
        }
    }
}

fn apply_rounding(hwnd: HWND) {
    let preference = DWMWCP_ROUND;
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &preference as *const _ as *const core::ffi::c_void,
            std::mem::size_of_val(&preference) as u32,
        );
    }
}

fn populate(panel: &mut Panel, query: &str) {
    let query = query.trim().to_lowercase();
    panel.filtered = ITEMS
        .iter()
        .enumerate()
        .filter(|(_, item)| {
            query.is_empty()
                || item.title.to_lowercase().contains(&query)
                || item.content.to_lowercase().contains(&query)
        })
        .map(|(index, _)| index)
        .collect();

    unsafe {
        let _ = send(panel.list, LB_RESETCONTENT, 0, 0);
    }

    for index in &panel.filtered {
        let title = platform::wide(ITEMS[*index].title);
        unsafe {
            let _ = send(panel.list, LB_ADDSTRING, 0, title.as_ptr() as isize);
        }
    }

    if !panel.filtered.is_empty() {
        unsafe {
            let _ = send(panel.list, LB_SETCURSEL, 0, 0);
        }
    }
}

fn read_text(hwnd: HWND) -> String {
    let mut buffer = [0u16; 256];
    let length = unsafe { GetWindowTextW(hwnd, &mut buffer) };
    if length <= 0 {
        String::new()
    } else {
        String::from_utf16_lossy(&buffer[..length as usize])
    }
}

unsafe extern "system" fn edit_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_KEYDOWN {
        let key = wparam.0 as u16;

        if key == VK_RETURN.0 {
            commit();
            return LRESULT(0);
        }

        if key == VK_ESCAPE.0 {
            hide_and_restore();
            return LRESULT(0);
        }

        let delta = if key == VK_UP.0 {
            Some(-1)
        } else if key == VK_DOWN.0 {
            Some(1)
        } else if key == VK_HOME.0 {
            Some(-1000)
        } else if key == VK_END.0 {
            Some(1000)
        } else {
            None
        };

        if let Some(delta) = delta {
            PANEL.with(|slot| {
                if let Some(panel) = slot.borrow_mut().as_mut() {
                    panel.move_selection(delta);
                }
            });
            return LRESULT(0);
        }
    }

    let old = OLD_EDIT_PROC.load(Ordering::Relaxed);
    if old == 0 {
        return DefWindowProcW(hwnd, message, wparam, lparam);
    }
    let previous: WNDPROC = std::mem::transmute::<isize, WNDPROC>(old);
    CallWindowProcW(previous, hwnd, message, wparam, lparam)
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_COMMAND => {
            let control = (wparam.0 & 0xffff) as i32;
            let code = ((wparam.0 >> 16) & 0xffff) as u32;
            if control == EDIT_ID && code == EN_CHANGE {
                let edit = HWND(lparam.0 as *mut core::ffi::c_void);
                let text = read_text(edit);
                PANEL.with(|slot| {
                    if let Some(panel) = slot.borrow_mut().as_mut() {
                        populate(panel, &text);
                    }
                });
            } else if control == LIST_ID && code == LBN_DBLCLK {
                commit();
            }
            LRESULT(0)
        }
        WM_DRAWITEM => {
            let draw = &*(lparam.0 as *const DRAWITEMSTRUCT);
            if draw.CtlID as i32 == LIST_ID {
                PANEL.with(|slot| {
                    if let Some(panel) = slot.borrow().as_ref() {
                        draw_item(panel, draw);
                    }
                });
            }
            LRESULT(0)
        }
        WM_CTLCOLOREDIT => {
            let hdc = HDC(wparam.0 as *mut core::ffi::c_void);
            unsafe {
                SetBkMode(hdc, TRANSPARENT);
                SetBkColor(hdc, rgb(248, 248, 253));
                SetTextColor(hdc, rgb(0x22, 0x24, 0x3E));
            }
            let brush = PANEL.with(|slot| {
                slot.borrow()
                    .as_ref()
                    .map(|panel| panel.brushes.search.0 as isize)
                    .unwrap_or(0)
            });
            LRESULT(brush)
        }
        WM_CTLCOLORSTATIC => {
            let hdc = HDC(wparam.0 as *mut core::ffi::c_void);
            unsafe {
                SetBkMode(hdc, TRANSPARENT);
                SetTextColor(hdc, rgb(0x77, 0x7D, 0x92));
            }
            let brush = PANEL.with(|slot| {
                slot.borrow()
                    .as_ref()
                    .map(|panel| panel.brushes.background.0 as isize)
                    .unwrap_or(0)
            });
            LRESULT(brush)
        }
        WM_ERASEBKGND => {
            let hdc = HDC(wparam.0 as *mut core::ffi::c_void);
            let mut rect = RECT::default();
            unsafe {
                let _ = GetClientRect(hwnd, &mut rect);
                if let Some(brush) =
                    PANEL.with(|slot| slot.borrow().as_ref().map(|panel| panel.brushes.background))
                {
                    FillRect(hdc, &rect, brush);
                }
            }
            LRESULT(1)
        }
        WM_PAINT => {
            let mut paint = PAINTSTRUCT::default();
            unsafe {
                let hdc = BeginPaint(hwnd, &mut paint);
                let mut rect = RECT::default();
                let _ = GetClientRect(hwnd, &mut rect);
                if let Some((brush, border, search, dpi)) = PANEL.with(|slot| {
                    slot.borrow().as_ref().map(|panel| {
                        (
                            panel.brushes.background,
                            panel.brushes.border,
                            panel.brushes.search,
                            panel.dpi,
                        )
                    })
                }) {
                    FillRect(hdc, &rect, brush);

                    let margin = scaled(16, dpi);
                    let search_rect = RECT {
                        left: margin - 1,
                        top: scaled(14, dpi) - 1,
                        right: rect.right - margin + 1,
                        bottom: scaled(14, dpi) + scaled(34, dpi) + 1,
                    };
                    FillRect(hdc, &search_rect, search);
                    let _ = FrameRect(hdc, &search_rect, border);
                }
                let _ = EndPaint(hwnd, &paint);
            }
            LRESULT(0)
        }
        WM_ACTIVATE => {
            let active = (wparam.0 & 0xffff) as u32;
            if active == WA_INACTIVE {
                let hiding = PANEL.with(|slot| {
                    slot.borrow()
                        .as_ref()
                        .map(|panel| panel.hiding)
                        .unwrap_or(true)
                });
                if !hiding {
                    hide_only();
                }
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            hide_and_restore();
            LRESULT(0)
        }
        WM_DPICHANGED => {
            let mut rect = RECT::default();
            unsafe {
                let _ = GetClientRect(hwnd, &mut rect);
            }
            PANEL.with(|slot| {
                if let Some(panel) = slot.borrow_mut().as_mut() {
                    panel.layout(rect.right, rect.bottom);
                }
            });
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}

fn draw_item(panel: &Panel, draw: &DRAWITEMSTRUCT) {
    if draw.itemID == u32::MAX {
        return;
    }
    let Some(item_index) = panel.filtered.get(draw.itemID as usize) else {
        return;
    };
    let item = &ITEMS[*item_index];
    let hdc = draw.hDC;
    let rect = draw.rcItem;

    unsafe {
        let background = if draw.itemState.0 & ODS_SELECTED.0 != 0 {
            panel.brushes.selected
        } else {
            panel.brushes.background
        };
        FillRect(hdc, &rect, background);
        SetBkMode(hdc, TRANSPARENT);
    }

    let pad = scaled(12, panel.dpi);

    let mut title = platform::wide(item.title);
    let mut title_rect = RECT {
        left: rect.left + pad,
        top: rect.top + scaled(8, panel.dpi),
        right: rect.right - scaled(90, panel.dpi),
        bottom: rect.top + scaled(28, panel.dpi),
    };
    unsafe {
        let previous = SelectObject(hdc, HGDIOBJ(panel.fonts.title.0));
        SetTextColor(hdc, rgb(0x22, 0x24, 0x3E));
        DrawTextW(
            hdc,
            &mut title,
            &mut title_rect,
            DT_SINGLELINE | DT_LEFT | DT_NOPREFIX | DT_END_ELLIPSIS,
        );
        let _ = SelectObject(hdc, previous);
    }

    let mut preview = platform::wide(item.content);
    let mut preview_rect = RECT {
        left: rect.left + pad,
        top: rect.top + scaled(30, panel.dpi),
        right: rect.right - scaled(12, panel.dpi),
        bottom: rect.bottom - scaled(6, panel.dpi),
    };
    unsafe {
        let previous = SelectObject(hdc, HGDIOBJ(panel.fonts.body.0));
        SetTextColor(hdc, rgb(0x77, 0x7D, 0x92));
        DrawTextW(
            hdc,
            &mut preview,
            &mut preview_rect,
            DT_SINGLELINE | DT_LEFT | DT_NOPREFIX | DT_END_ELLIPSIS,
        );
        let _ = SelectObject(hdc, previous);
    }

    let mut tag = platform::wide(item.category);
    let mut tag_rect = RECT {
        left: rect.right - scaled(84, panel.dpi),
        top: rect.top + scaled(10, panel.dpi),
        right: rect.right - scaled(12, panel.dpi),
        bottom: rect.top + scaled(28, panel.dpi),
    };
    unsafe {
        let previous = SelectObject(hdc, HGDIOBJ(panel.fonts.small.0));
        SetTextColor(hdc, rgb(0x76, 0x55, 0xE9));
        DrawTextW(
            hdc,
            &mut tag,
            &mut tag_rect,
            DT_SINGLELINE | DT_RIGHT | DT_NOPREFIX | DT_END_ELLIPSIS,
        );
        let _ = SelectObject(hdc, previous);
    }
}

pub fn bench(iterations: u32) -> (u128, u128, u128) {
    let mut samples = Vec::with_capacity(iterations as usize);
    for _ in 0..iterations {
        let elapsed = present(HWND::default(), None, "bench", false, false);
        samples.push(elapsed);
        hide_and_restore();
    }
    samples.sort_unstable();
    let p50 = samples[samples.len() / 2];
    let p95 = samples[(samples.len() * 95 / 100).min(samples.len() - 1)];
    let max = *samples.last().unwrap_or(&0);
    (p50, p95, max)
}

#[allow(dead_code)]
fn shutdown() {
    PANEL.with(|slot| {
        if let Some(panel) = slot.borrow_mut().take() {
            destroy_fonts(&panel.fonts);
            destroy_brushes(&panel.brushes);
        }
    });
}
