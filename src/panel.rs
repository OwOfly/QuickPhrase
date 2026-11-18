use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::time::Instant;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{
    GetLastError, COLORREF, ERROR_CLASS_ALREADY_EXISTS, HINSTANCE, HWND, LPARAM, LRESULT, POINT,
    RECT, WPARAM,
};
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
use windows::Win32::UI::Input::Ime::{
    ImmGetCompositionStringW, ImmGetContext, ImmReleaseContext, GCS_COMPSTR,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetFocus, GetKeyState, SetFocus, VK_CONTROL, VK_DOWN, VK_END, VK_ESCAPE, VK_HOME, VK_MENU,
    VK_RETURN, VK_SHIFT, VK_UP,
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
const EM_LIMITTEXT: u32 = 0x00C5;
const LBN_SELCHANGE_CODE: u32 = 1;
const LBN_SETFOCUS_CODE: u32 = 4;

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
    monitor: Option<crate::foreground::Monitor>,
}

thread_local! {
    static PANEL: RefCell<Option<Panel>> = const { RefCell::new(None) };
}

static OLD_EDIT_PROC: AtomicIsize = AtomicIsize::new(0);
static CREATE_FAILED: AtomicBool = AtomicBool::new(false);

fn rgb(r: u32, g: u32, b: u32) -> COLORREF {
    COLORREF(r | (g << 8) | (b << 16))
}

fn scaled(value: i32, dpi: u32) -> i32 {
    (value * dpi as i32 + 48) / 96
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

pub fn precreate() {
    let needs_create =
        PANEL.with(|slot| slot.borrow().is_none()) && !CREATE_FAILED.load(Ordering::Relaxed);
    if needs_create {
        if let Some(panel) = create() {
            PANEL.with(|slot| *slot.borrow_mut() = Some(panel));
        }
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
    let started = Instant::now();
    let needs_create =
        PANEL.with(|slot| slot.borrow().is_none()) && !CREATE_FAILED.load(Ordering::Relaxed);
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
        if activate && panel.monitor.is_none() {
            panel.monitor = crate::foreground::start();
        }
        (panel.hwnd, panel.edit)
    });

    if hwnd.0.is_null() {
        return 0;
    }

    place();
    apply_rounding(hwnd);
    unsafe {
        let _ = SetWindowTextW(edit, w!(""));
    }
    populate("");

    unsafe {
        let _ = ShowWindow(hwnd, if activate { SW_SHOW } else { SW_SHOWNOACTIVATE });
        if activate {
            force_foreground(hwnd);
            let _ = SetFocus(Some(edit));
            let _ = send(edit, EM_SETSEL, 0, -1);
            if GetForegroundWindow() != hwnd || GetFocus() != edit {
                logging::warn("panel could not take foreground or focus");
                if std::env::var_os("QUICKPHRASE_TEST_ALLOW_UNFOCUSED_PANEL").is_none() {
                    hide_only();
                    return 0;
                }
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

    if hwnd.0.is_null() {
        return;
    }

    if !unsafe { IsWindowVisible(hwnd) }.as_bool() {
        PANEL.with(|slot| {
            if let Some(panel) = slot.borrow_mut().as_mut() {
                panel.hiding = false;
            }
        });
        return;
    }

    unsafe {
        let _ = ShowWindow(hwnd, SW_HIDE);
    }

    let monitor = PANEL.with(|slot| {
        slot.borrow_mut()
            .as_mut()
            .and_then(|panel| panel.monitor.take())
    });
    drop(monitor);

    if restore && !target.0.is_null() {
        if unsafe { IsWindow(Some(target)) }.as_bool() {
            if !unsafe { SetForegroundWindow(target) }.as_bool() {
                logging::warn("restore foreground failed");
            }
        } else {
            logging::warn("restore target no longer exists");
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
    if unsafe { RegisterClassW(&class) } == 0 {
        let error = unsafe { GetLastError() };
        if error != ERROR_CLASS_ALREADY_EXISTS {
            logging::error(&format!("RegisterClassW(Panel) failed: {error:?}"));
            CREATE_FAILED.store(true, Ordering::Relaxed);
            return None;
        }
    }

    let hwnd = match unsafe {
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
    } {
        Ok(hwnd) => hwnd,
        Err(err) => {
            logging::error(&format!("CreateWindowExW(PANEL) failed: {err}"));
            CREATE_FAILED.store(true, Ordering::Relaxed);
            return None;
        }
    };

    let dpi = unsafe { GetDpiForWindow(hwnd) };
    let dpi = if dpi == 0 { 96 } else { dpi };
    let fonts = make_fonts(dpi);
    let brushes = make_brushes();

    let edit = match unsafe {
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
    } {
        Ok(edit) => edit,
        Err(err) => {
            logging::error(&format!("CreateWindowExW(EDIT) failed: {err}"));
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            destroy_fonts(&fonts);
            destroy_brushes(&brushes);
            CREATE_FAILED.store(true, Ordering::Relaxed);
            return None;
        }
    };

    let list = match unsafe {
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
    } {
        Ok(list) => list,
        Err(err) => {
            logging::error(&format!("CreateWindowExW(LISTBOX) failed: {err}"));
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            destroy_fonts(&fonts);
            destroy_brushes(&brushes);
            CREATE_FAILED.store(true, Ordering::Relaxed);
            return None;
        }
    };

    let footer = match unsafe {
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
    } {
        Ok(footer) => footer,
        Err(err) => {
            logging::error(&format!("CreateWindowExW(STATIC) failed: {err}"));
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            destroy_fonts(&fonts);
            destroy_brushes(&brushes);
            CREATE_FAILED.store(true, Ordering::Relaxed);
            return None;
        }
    };

    for control in [edit, list, footer] {
        unsafe {
            let _ = send(control, WM_SETFONT, fonts.body.0 as usize, 1);
        }
    }

    let cue = platform::wide("搜索短语…");
    unsafe {
        let _ = send(edit, EM_SETCUEBANNER, 1, cue.as_ptr() as isize);
        let _ = send(edit, EM_LIMITTEXT, 200, 0);
        let old = SetWindowLongPtrW(edit, GWLP_WNDPROC, edit_proc as *const () as usize as isize);
        if old == 0 {
            logging::error("SetWindowLongPtrW(EDIT) failed");
            let _ = DestroyWindow(hwnd);
            destroy_fonts(&fonts);
            destroy_brushes(&brushes);
            CREATE_FAILED.store(true, Ordering::Relaxed);
            return None;
        }
        OLD_EDIT_PROC.store(old, Ordering::Relaxed);
        let _ = send(list, LB_SETITEMHEIGHT, 0, scaled(ITEM_HEIGHT, dpi) as isize);
    }

    let panel = Panel {
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
        monitor: None,
    };

    let mut rect = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut rect);
    }
    layout_controls(
        panel.edit,
        panel.list,
        panel.footer,
        panel.dpi,
        rect.right,
        rect.bottom,
    );

    Some(panel)
}

fn commit() {
    let info = PANEL.with(|slot| {
        let guard = slot.borrow();
        let panel = guard.as_ref()?;
        let index = panel.selected_index()?;
        let item_index = *panel.filtered.get(index)?;
        Some((
            item_index,
            ITEMS[item_index].category,
            panel.summary.clone(),
        ))
    });

    let Some((item_index, category, summary)) = info else {
        logging::info("panel commit ignored: no matching item");
        return;
    };

    logging::info(&format!(
        "panel commit: index={item_index} category={category} (M0-4 will insert)"
    ));
    logging::info(&format!("panel capture context: {summary}"));

    hide_and_restore();
}

fn apply_dpi(dpi: u32) {
    if dpi == 0 {
        return;
    }

    let changed = PANEL.with(|slot| {
        let mut guard = slot.borrow_mut();
        let Some(panel) = guard.as_mut() else {
            return false;
        };
        if panel.dpi == dpi {
            return false;
        }
        destroy_fonts(&panel.fonts);
        panel.fonts = make_fonts(dpi);
        panel.dpi = dpi;
        true
    });

    if !changed {
        return;
    }

    let (edit, list, footer, font) = PANEL.with(|slot| {
        let guard = slot.borrow();
        let Some(panel) = guard.as_ref() else {
            return (
                HWND::default(),
                HWND::default(),
                HWND::default(),
                HFONT::default(),
            );
        };
        (panel.edit, panel.list, panel.footer, panel.fonts.body)
    });

    unsafe {
        for control in [edit, list, footer] {
            let _ = send(control, WM_SETFONT, font.0 as usize, 1);
        }
        let _ = send(list, LB_SETITEMHEIGHT, 0, scaled(ITEM_HEIGHT, dpi) as isize);
    }
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

    apply_dpi(dpi);

    let work_width = (work.right - work.left).max(1);
    let work_height = (work.bottom - work.top).max(1);
    let width = scaled(PANEL_WIDTH, dpi).min((work_width - scaled(16, dpi)).max(1));
    let height = scaled(PANEL_HEIGHT, dpi).min((work_height - scaled(16, dpi)).max(1));
    let mut x = anchor.x;
    let mut y = anchor.y + scaled(8, dpi);

    if y + height > work.bottom {
        y = anchor.y - height - scaled(ITEM_HEIGHT, dpi);
    }

    let min_x = work.left + scaled(8, dpi);
    let max_x = (work.right - width - scaled(8, dpi)).max(min_x);
    let min_y = work.top + scaled(8, dpi);
    let max_y = (work.bottom - height - scaled(8, dpi)).max(min_y);
    x = x.clamp(min_x, max_x);
    y = y.clamp(min_y, max_y);

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

fn layout_controls(edit: HWND, list: HWND, footer: HWND, dpi: u32, width: i32, height: i32) {
    if width <= 0 || height <= 0 {
        return;
    }
    let margin = scaled(16, dpi);
    let edit_height = scaled(34, dpi);
    let footer_height = scaled(22, dpi);
    let gap = scaled(10, dpi);

    unsafe {
        let _ = MoveWindow(
            edit,
            margin,
            scaled(14, dpi),
            width - margin * 2,
            edit_height,
            true,
        );
        let list_top = scaled(14, dpi) + edit_height + gap;
        let list_height = (height - list_top - footer_height - scaled(12, dpi)).max(0);
        let _ = MoveWindow(
            list,
            scaled(10, dpi),
            list_top,
            width - scaled(20, dpi),
            list_height,
            true,
        );
        let _ = MoveWindow(
            footer,
            margin,
            height - footer_height - scaled(8, dpi),
            width - margin * 2,
            footer_height,
            true,
        );
    }
}

impl Panel {
    fn selected_index(&self) -> Option<usize> {
        let index = unsafe { send(self.list, LB_GETCURSEL, 0, 0) }.0;
        if index < 0 {
            None
        } else {
            Some(index as usize)
        }
    }
}

fn move_selection(delta: i32) {
    let (list, current, count) = PANEL.with(|slot| {
        let guard = slot.borrow();
        let Some(panel) = guard.as_ref() else {
            return (HWND::default(), 0, 0);
        };
        let count = unsafe { send(panel.list, LB_GETCOUNT, 0, 0) }.0 as i32;
        let current = panel.selected_index().unwrap_or(0) as i32;
        (panel.list, current, count)
    });

    if list.0.is_null() || count <= 0 {
        return;
    }
    let next = (current + delta).clamp(0, count - 1);
    unsafe {
        let _ = send(list, LB_SETCURSEL, next as usize, 0);
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
        if IsHungAppWindow(foreground).as_bool() {
            return;
        }

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

fn filter_items(query: &str) -> Vec<usize> {
    let query = query.trim().to_lowercase();
    ITEMS
        .iter()
        .enumerate()
        .filter(|(_, item)| {
            query.is_empty()
                || item.title.to_lowercase().contains(&query)
                || item.content.to_lowercase().contains(&query)
        })
        .map(|(index, _)| index)
        .collect()
}

fn populate(query: &str) {
    let filtered = filter_items(query);
    let titles: Vec<Vec<u16>> = filtered
        .iter()
        .map(|index| platform::wide(ITEMS[*index].title))
        .collect();

    let list = PANEL.with(|slot| {
        let mut guard = slot.borrow_mut();
        let panel = guard.as_mut()?;
        panel.filtered = filtered;
        Some(panel.list)
    });

    let Some(list) = list else {
        return;
    };

    unsafe {
        let _ = send(list, LB_RESETCONTENT, 0, 0);
        for title in &titles {
            let _ = send(list, LB_ADDSTRING, 0, title.as_ptr() as isize);
        }
        if !titles.is_empty() {
            let _ = send(list, LB_SETCURSEL, 0, 0);
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

fn modifiers_down() -> bool {
    unsafe {
        GetKeyState(VK_CONTROL.0 as i32) < 0
            || GetKeyState(VK_SHIFT.0 as i32) < 0
            || GetKeyState(VK_MENU.0 as i32) < 0
    }
}

fn ime_composing(hwnd: HWND) -> bool {
    unsafe {
        let context = ImmGetContext(hwnd);
        if context.0.is_null() {
            return false;
        }
        let length = ImmGetCompositionStringW(context, GCS_COMPSTR, None, 0);
        let _ = ImmReleaseContext(hwnd, context);
        length > 0
    }
}

unsafe extern "system" fn edit_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_KEYDOWN || message == WM_SYSKEYDOWN {
        let key = wparam.0 as u16;

        if key == VK_RETURN.0 {
            if ime_composing(hwnd) {
                return call_default_edit(hwnd, message, wparam, lparam);
            }
            commit();
            return LRESULT(0);
        }

        if key == VK_ESCAPE.0 {
            hide_and_restore();
            return LRESULT(0);
        }

        if modifiers_down() {
            return call_default_edit(hwnd, message, wparam, lparam);
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
            move_selection(delta);
            return LRESULT(0);
        }
    }

    call_default_edit(hwnd, message, wparam, lparam)
}

fn call_default_edit(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let old = OLD_EDIT_PROC.load(Ordering::Relaxed);
    if old == 0 {
        return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
    }
    let previous: WNDPROC = unsafe { std::mem::transmute::<isize, WNDPROC>(old) };
    unsafe { CallWindowProcW(previous, hwnd, message, wparam, lparam) }
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
            let parts = PANEL.with(|slot| {
                slot.borrow()
                    .as_ref()
                    .map(|panel| (panel.edit, panel.list, panel.footer, panel.dpi))
            });
            if let Some((edit, list, footer, dpi)) = parts {
                layout_controls(edit, list, footer, dpi, width, height);
            }
            LRESULT(0)
        }
        WM_COMMAND => {
            let control = (wparam.0 & 0xffff) as i32;
            let code = ((wparam.0 >> 16) & 0xffff) as u32;
            if control == EDIT_ID && code == EN_CHANGE {
                let edit = HWND(lparam.0 as *mut core::ffi::c_void);
                let text = read_text(edit);
                populate(&text);
            } else if control == LIST_ID && code == LBN_DBLCLK {
                commit();
            } else if control == LIST_ID
                && (code == LBN_SELCHANGE_CODE || code == LBN_SETFOCUS_CODE)
            {
                let edit = PANEL.with(|slot| {
                    slot.borrow()
                        .as_ref()
                        .map(|panel| panel.edit)
                        .unwrap_or_default()
                });
                if !edit.0.is_null() {
                    unsafe {
                        let _ = SetFocus(Some(edit));
                    }
                }
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
                return LRESULT(1);
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
            let dpi = (wparam.0 & 0xffff) as u32;
            apply_dpi(dpi);
            let suggested = &*(lparam.0 as *const RECT);
            unsafe {
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    suggested.left,
                    suggested.top,
                    suggested.right - suggested.left,
                    suggested.bottom - suggested.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
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
    if iterations == 0 {
        return (0, 0, 0);
    }
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

pub fn shutdown() {
    // Take the panel out first: DestroyWindow dispatches messages that would
    // re-enter the window procedure and try to borrow PANEL again.
    let panel = PANEL.with(|slot| slot.borrow_mut().take());
    let Some(panel) = panel else {
        return;
    };

    unsafe {
        let old = OLD_EDIT_PROC.load(Ordering::Relaxed);
        if old != 0 {
            let _ = SetWindowLongPtrW(panel.edit, GWLP_WNDPROC, old);
            OLD_EDIT_PROC.store(0, Ordering::Relaxed);
        }
        let _ = DestroyWindow(panel.hwnd);
    }
    destroy_fonts(&panel.fonts);
    destroy_brushes(&panel.brushes);
}

#[cfg(test)]
mod tests {
    use super::{filter_items, scaled};

    #[test]
    fn empty_query_returns_every_item() {
        assert_eq!(filter_items(""), vec![0, 1, 2]);
    }

    #[test]
    fn query_matches_title_and_content() {
        assert_eq!(filter_items("项目"), vec![0]);
        assert_eq!(filter_items("JSON"), vec![2]);
        assert!(filter_items("does-not-exist").is_empty());
    }

    #[test]
    fn scaling_rounds_to_nearest_pixel() {
        assert_eq!(scaled(520, 96), 520);
        assert_eq!(scaled(520, 120), 650);
        assert_eq!(scaled(420, 200), 875);
        assert_eq!(scaled(34, 125), 44);
    }
}
