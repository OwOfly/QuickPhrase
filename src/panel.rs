//! QuickPhrase 2.0 floating quick-insert panel (Direct2D + DirectWrite).

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, AtomicU64, Ordering};
use std::time::Instant;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{
    GetLastError, COLORREF, ERROR_CLASS_ALREADY_EXISTS, HINSTANCE, HWND, LPARAM, LRESULT, POINT,
    RECT, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DeleteObject, EndPaint, GetMonitorInfoW, InvalidateRect,
    MonitorFromPoint, ScreenToClient, SetBkColor, SetBkMode, SetTextColor, UpdateWindow, HBRUSH,
    HDC, HFONT, HGDIOBJ, MONITORINFO, MONITOR_DEFAULTTONEAREST, PAINTSTRUCT, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::Controls::{DRAWITEMSTRUCT, ODS_SELECTED, WM_MOUSELEAVE};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, GetDpiForWindow, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::Ime::{
    ImmGetCompositionStringW, ImmGetContext, ImmReleaseContext, GCS_COMPSTR,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetFocus, GetKeyState, SetFocus, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT, VK_CONTROL,
    VK_DOWN, VK_END, VK_ESCAPE, VK_HOME, VK_MENU, VK_RETURN, VK_SHIFT, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::capture::Report;
use crate::d2d::{self, AlignH, AlignV, Icon, TextStyle};
use crate::{inject, logging, platform, theme, ui};

const PANEL_CLASS: PCWSTR = w!("QuickPhrase.Panel");
const EDIT_ID: i32 = 100;
const LIST_ID: i32 = 101;
const FADE_TIMER: usize = 7;

const PANEL_WIDTH: i32 = 520;
const PANEL_HEIGHT: i32 = 420;
const BANNER_HEIGHT: i32 = 28;
const SEARCH_HEIGHT: i32 = 40;
const CATEGORY_HEIGHT: i32 = 40;
const TAG_HEIGHT: i32 = 34;
const FOOTER_HEIGHT: i32 = 38;
const ITEM_HEIGHT: i32 = 60;

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

#[derive(Clone)]
pub struct Item {
    pub id: String,
    pub title: String,
    pub content: String,
    pub category: String,
    pub category_id: Option<String>,
    pub tags: Vec<String>,
    pub blob: String,
    pub pinned: bool,
    pub last_used_at: Option<String>,
    pub updated_at: String,
    pub sort_order: i32,
    pub use_count: u64,
}

pub struct PanelData {
    pub items: Vec<Item>,
    pub categories: Vec<(String, String)>,
    pub tag_counts: Vec<(String, usize)>,
    pub notice: Option<String>,
    pub copy_only: Option<String>,
    pub paste_mode: bool,
    pub keep_tabs: bool,
    pub hotkey_hint: String,
    pub sort_mode: crate::search::SortMode,
}

/// Tag cloud data: case-insensitive merge (first spelling wins), count desc,
/// then name order. Used by both the production panel and `--panel-test`.
pub fn count_tags(items: &[Item]) -> Vec<(String, usize)> {
    let mut counts: HashMap<String, (String, usize)> = HashMap::new();
    for item in items {
        for tag in &item.tags {
            let entry = counts
                .entry(tag.to_lowercase())
                .or_insert_with(|| (tag.clone(), 0));
            entry.1 += 1;
        }
    }
    let mut counts: Vec<(String, usize)> = counts.into_values().collect();
    counts.sort_by(|left, right| {
        right
            .1
            .cmp(&left.1)
            .then_with(|| left.0.to_lowercase().cmp(&right.0.to_lowercase()))
    });
    counts
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum HoverTarget {
    None,
    Settings,
    Manage,
    Category(usize),
    Tag(usize),
}

struct Brushes {
    background: HBRUSH,
    search: HBRUSH,
}

struct LayoutRects {
    banner: Option<RECT>,
    search_shell: RECT,
    settings_btn: RECT,
    hotkey_kbd: RECT,
    category_bar_bottom: i32,
    tag_bar_bottom: i32,
    list: RECT,
    footer: RECT,
    manage_btn: RECT,
}

struct Panel {
    hwnd: HWND,
    edit: HWND,
    list: HWND,
    edit_font: HFONT,
    filtered: Vec<usize>,
    items: Vec<Item>,
    categories: Vec<(String, String)>,
    tag_counts: Vec<(String, usize)>,
    sort_mode: crate::search::SortMode,
    active_category: Option<String>,
    active_tags: Vec<String>,
    copy_only: Option<String>,
    notice: Option<String>,
    paste_mode: bool,
    keep_tabs: bool,
    hotkey_hint: String,
    palette: theme::Palette,
    title_highlights: Vec<Option<(usize, usize)>>,
    content_highlights: Vec<Option<(usize, usize)>>,
    last_query: String,
    report: Option<Report>,
    notify: HWND,
    brushes: Brushes,
    dpi: u32,
    hiding: bool,
    quit_on_hide: bool,
    monitor: Option<crate::foreground::Monitor>,
    opening: bool,
    deferred_inactive: bool,
    hover: HoverTarget,
    tracking_mouse: bool,
}

thread_local! {
    static PANEL: RefCell<Option<Panel>> = const { RefCell::new(None) };
}

static OLD_EDIT_PROC: AtomicIsize = AtomicIsize::new(0);
static FADE_STEP: AtomicU32 = AtomicU32::new(0);
static CREATE_FAILED: AtomicBool = AtomicBool::new(false);
static ENTER_ARMED: AtomicBool = AtomicBool::new(false);
static SESSION: AtomicU64 = AtomicU64::new(0);
static INJECTING: AtomicBool = AtomicBool::new(false);
static INJECT_RESULT: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

pub const WM_INJECT_DONE: u32 = WM_APP + 0x61;

pub fn take_inject_result() -> Option<String> {
    INJECT_RESULT.lock().ok().and_then(|mut slot| slot.take())
}

pub fn is_injecting() -> bool {
    INJECTING.load(Ordering::Relaxed)
}

fn scaled(value: i32, dpi: u32) -> i32 {
    (value * dpi as i32 + 48) / 96
}

fn animations_enabled() -> bool {
    if std::env::var_os("QUICKPHRASE_TEST_NO_ANIMATION").is_some() {
        return false;
    }
    let mut enabled = 1u32;
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            Some(&mut enabled as *mut u32 as *mut core::ffi::c_void),
            Default::default(),
        )
    }
    .is_ok();
    ok && enabled != 0
}

fn start_fade(hwnd: HWND) {
    if !animations_enabled() {
        return;
    }
    let timer = unsafe { SetTimer(Some(hwnd), FADE_TIMER, 10, None) };
    if timer == 0 {
        logging::warn("panel fade timer unavailable; showing without fade");
        return;
    }
    unsafe {
        let ex = GetWindowLongW(hwnd, GWL_EXSTYLE);
        let _ = SetWindowLongW(hwnd, GWL_EXSTYLE, ex | WS_EX_LAYERED.0 as i32);
        let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 0, LWA_ALPHA);
    }
    FADE_STEP.store(0, Ordering::Relaxed);
}

fn create_edit_font(dpi: u32) -> HFONT {
    ui::make_font(ui::FontRole::Search, dpi)
}

unsafe fn send(hwnd: HWND, message: u32, wparam: usize, lparam: isize) -> LRESULT {
    SendMessageW(hwnd, message, Some(WPARAM(wparam)), Some(LPARAM(lparam)))
}

fn make_brushes(palette: theme::Palette) -> Brushes {
    Brushes {
        background: unsafe { CreateSolidBrush(palette.card) },
        search: unsafe { CreateSolidBrush(palette.field) },
    }
}

fn destroy_brushes(brushes: &Brushes) {
    unsafe {
        let _ = DeleteObject(HGDIOBJ(brushes.background.0));
        let _ = DeleteObject(HGDIOBJ(brushes.search.0));
    }
}

fn has_banner(panel: &Panel) -> bool {
    panel.copy_only.is_some() || panel.notice.is_some() || panel.paste_mode
}

fn compute_layout(panel: &Panel, width: i32, height: i32) -> LayoutRects {
    let dpi = panel.dpi;
    let margin = scaled(14, dpi);
    let mut y = 0;

    let banner = if has_banner(panel) {
        let bh = scaled(BANNER_HEIGHT, dpi);
        let r = RECT {
            left: 1,
            top: 1,
            right: (width - 1).max(2),
            bottom: 1 + bh,
        };
        y = r.bottom;
        Some(r)
    } else {
        None
    };

    let search_top = y + scaled(12, dpi);
    let search_h = scaled(SEARCH_HEIGHT, dpi);
    let search_shell = RECT {
        left: margin,
        top: search_top,
        right: (width - margin).max(margin + 10),
        bottom: search_top + search_h,
    };

    let gear_size = scaled(28, dpi);
    let settings_btn = RECT {
        left: search_shell.right - scaled(6, dpi) - gear_size,
        top: search_shell.top + (search_h - gear_size) / 2,
        right: search_shell.right - scaled(6, dpi),
        bottom: search_shell.top + (search_h + gear_size) / 2,
    };

    let hotkey_kbd = if !panel.hotkey_hint.is_empty() {
        let (kw, _) = d2d::measure_style(&panel.hotkey_hint, TextStyle::Mono, dpi);
        let kbd_w = (kw.ceil() as i32 + scaled(12, dpi)).min(scaled(108, dpi));
        let kbd_h = scaled(22, dpi);
        RECT {
            left: settings_btn.left - scaled(6, dpi) - kbd_w,
            top: search_shell.top + (search_h - kbd_h) / 2,
            right: settings_btn.left - scaled(6, dpi),
            bottom: search_shell.top + (search_h + kbd_h) / 2,
        }
    } else {
        RECT {
            left: settings_btn.left,
            top: search_shell.top,
            right: settings_btn.left,
            bottom: search_shell.bottom,
        }
    };

    let category_bar_bottom = search_shell.bottom + scaled(CATEGORY_HEIGHT, dpi);
    let tag_row = if panel.tag_counts.is_empty() {
        0
    } else {
        scaled(TAG_HEIGHT, dpi)
    };
    let tag_bar_bottom = category_bar_bottom + tag_row;
    let footer_h = scaled(FOOTER_HEIGHT, dpi);
    let footer = RECT {
        left: 1,
        top: (height - footer_h - 1).max(tag_bar_bottom),
        right: (width - 1).max(2),
        bottom: (height - 1).max(1),
    };

    let manage_w = scaled(88, dpi);
    let manage_h = scaled(26, dpi);
    let manage_btn = RECT {
        left: (footer.right - margin - manage_w).max(footer.left + margin),
        top: footer.top + (footer_h - manage_h) / 2,
        right: footer.right - margin,
        bottom: footer.top + (footer_h + manage_h) / 2,
    };

    let list_top = tag_bar_bottom + scaled(6, dpi);
    let list = RECT {
        left: scaled(8, dpi),
        top: list_top,
        right: (width - scaled(8, dpi)).max(scaled(16, dpi)),
        bottom: (footer.top - scaled(6, dpi)).max(list_top),
    };

    LayoutRects {
        banner,
        search_shell,
        settings_btn,
        hotkey_kbd,
        category_bar_bottom,
        tag_bar_bottom,
        list,
        footer,
        manage_btn,
    }
}

fn layout_panel(panel: &Panel, width: i32, height: i32) {
    if width <= 0 || height <= 0 {
        return;
    }
    let rects = compute_layout(panel, width, height);
    let left_pad = scaled(38, panel.dpi);
    let right_edge = if rects.hotkey_kbd.left < rects.settings_btn.left {
        rects.hotkey_kbd.left - scaled(8, panel.dpi)
    } else {
        rects.settings_btn.left - scaled(8, panel.dpi)
    };
    let right_pad = (rects.search_shell.right - right_edge).max(scaled(10, panel.dpi));
    let edit_rect =
        ui::edit_centered_rect(rects.search_shell, panel.edit_font, left_pad, right_pad);

    unsafe {
        let _ = MoveWindow(
            panel.edit,
            edit_rect.left,
            edit_rect.top,
            (edit_rect.right - edit_rect.left).max(1),
            (edit_rect.bottom - edit_rect.top).max(1),
            true,
        );
        let _ = MoveWindow(
            panel.list,
            rects.list.left,
            rects.list.top,
            (rects.list.right - rects.list.left).max(1),
            (rects.list.bottom - rects.list.top).max(0),
            true,
        );
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

pub fn focus() -> bool {
    let (hwnd, edit) = PANEL.with(|slot| {
        let guard = slot.borrow();
        match guard.as_ref() {
            Some(panel) => (panel.hwnd, panel.edit),
            None => (HWND::default(), HWND::default()),
        }
    });
    if hwnd.0.is_null() {
        return false;
    }
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
        if !edit.0.is_null() {
            let _ = SetFocus(Some(edit));
        }
    }
    true
}

pub fn show(
    report: Report,
    monitor: Option<crate::foreground::Monitor>,
    notify: HWND,
    data: PanelData,
) -> u128 {
    present(Some(report), monitor, notify, data, true, false)
}

pub fn show_copy_only(notify: HWND, data: PanelData) -> u128 {
    present(None, None, notify, data, true, false)
}

pub fn show_test() -> u128 {
    crate::storage::init();
    let items: Vec<Item> = crate::storage::panel_items()
        .into_iter()
        .map(|row| Item {
            id: row.id,
            title: row.title,
            content: row.content,
            category: row.category,
            category_id: row.category_id,
            tags: row.tags,
            blob: row.blob,
            pinned: row.pinned,
            last_used_at: row.last_used_at,
            updated_at: row.updated_at,
            sort_order: row.sort_order,
            use_count: row.use_count,
        })
        .collect();
    let tag_counts = count_tags(&items);
    let settings = crate::storage::settings_clone().unwrap_or_default();
    let hotkey_hint = settings.hotkey.clone();
    present(
        None,
        None,
        HWND::default(),
        PanelData {
            items,
            categories: crate::storage::categories(),
            tag_counts,
            notice: crate::storage::read_only_notice(),
            copy_only: None,
            paste_mode: false,
            keep_tabs: false,
            hotkey_hint,
            sort_mode: crate::search::SortMode::from_setting(&settings.panel_sort),
        },
        true,
        true,
    )
}

fn present(
    report: Option<Report>,
    monitor: Option<crate::foreground::Monitor>,
    notify: HWND,
    data: PanelData,
    activate: bool,
    quit_on_hide: bool,
) -> u128 {
    let started = Instant::now();
    SESSION.fetch_add(1, Ordering::Relaxed);
    let needs_create =
        PANEL.with(|slot| slot.borrow().is_none()) && !CREATE_FAILED.load(Ordering::Relaxed);
    if needs_create {
        if let Some(panel) = create() {
            PANEL.with(|slot| *slot.borrow_mut() = Some(panel));
        }
    }

    let (hwnd, edit, retheme) = PANEL.with(|slot| {
        let mut guard = slot.borrow_mut();
        let Some(panel) = guard.as_mut() else {
            return (HWND::default(), HWND::default(), None);
        };
        let mut retheme = None;
        if activate {
            let palette = theme::current();
            if palette != panel.palette {
                destroy_brushes(&panel.brushes);
                panel.brushes = make_brushes(palette);
                panel.palette = palette;
                retheme = Some((panel.edit, panel.list, palette));
            }
        }
        panel.report = report;
        panel.monitor = monitor;
        panel.notify = notify;
        panel.items = data.items;
        panel.categories = data.categories;
        panel.tag_counts = data.tag_counts;
        panel.sort_mode = data.sort_mode;
        panel.notice = data.notice;
        panel.copy_only = data.copy_only;
        panel.paste_mode = data.paste_mode;
        panel.keep_tabs = data.keep_tabs;
        panel.hotkey_hint = data.hotkey_hint;
        panel.active_category = None;
        panel.active_tags.clear();
        panel.hover = HoverTarget::None;
        panel.hiding = false;
        panel.quit_on_hide = quit_on_hide;
        ENTER_ARMED.store(false, Ordering::Relaxed);
        logging::info(&format!(
            "panel data: items={} categories={} copy_only={} notice={:?}",
            panel.items.len(),
            panel.categories.len(),
            panel.copy_only.is_some(),
            panel.notice
        ));
        (panel.hwnd, panel.edit, retheme)
    });

    if hwnd.0.is_null() {
        return 0;
    }
    if let Some((edit, list, palette)) = retheme {
        ui::apply_popup_style(hwnd, palette);
        ui::theme_control(edit, palette.dark);
        ui::theme_control(list, palette.dark);
    }

    place();
    PANEL.with(|slot| {
        if let Some(panel) = slot.borrow().as_ref() {
            let mut client = RECT::default();
            unsafe {
                let _ = GetClientRect(panel.hwnd, &mut client);
            }
            layout_panel(panel, client.right, client.bottom);
        }
    });

    unsafe {
        let _ = SetWindowTextW(edit, w!(""));
    }
    populate("");

    PANEL.with(|slot| {
        if let Some(panel) = slot.borrow_mut().as_mut() {
            panel.opening = true;
        }
    });

    unsafe {
        if activate {
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            force_foreground(hwnd);
            let _ = SetFocus(Some(edit));
            let _ = send(edit, EM_SETSEL, 0, -1);
            if animations_enabled() {
                start_fade(hwnd);
            }
            if GetForegroundWindow() != hwnd || GetFocus() != edit {
                logging::warn("panel could not take foreground or focus");
                if std::env::var_os("QUICKPHRASE_TEST_ALLOW_UNFOCUSED_PANEL").is_none() {
                    PANEL.with(|slot| {
                        if let Some(panel) = slot.borrow_mut().as_mut() {
                            panel.opening = false;
                        }
                    });
                    hide_only();
                    return 0;
                }
            }
        } else {
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }
        let _ = UpdateWindow(hwnd);
    }

    let deferred = PANEL.with(|slot| {
        let mut guard = slot.borrow_mut();
        match guard.as_mut() {
            Some(panel) => {
                panel.opening = false;
                std::mem::take(&mut panel.deferred_inactive)
            }
            None => false,
        }
    });

    if deferred
        && unsafe { GetForegroundWindow() } != hwnd
        && std::env::var_os("QUICKPHRASE_TEST_ALLOW_UNFOCUSED_PANEL").is_none()
    {
        logging::warn("panel lost activation while opening");
        hide_only();
        return 0;
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
        ENTER_ARMED.store(false, Ordering::Relaxed);
        let target = panel
            .report
            .as_ref()
            .map(|report| HWND(report.target.hwnd as *mut core::ffi::c_void))
            .unwrap_or_default();
        (panel.hwnd, target, panel.quit_on_hide)
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
    let module = match unsafe { GetModuleHandleW(None) } {
        Ok(module) => module,
        Err(err) => {
            logging::error(&format!("GetModuleHandleW failed: {err}"));
            CREATE_FAILED.store(true, Ordering::Relaxed);
            return None;
        }
    };
    let instance = HINSTANCE(module.0);

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
            WS_POPUP | WS_CLIPCHILDREN,
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
    let edit_font = create_edit_font(dpi);
    let palette = theme::current();
    let brushes = make_brushes(palette);
    ui::apply_popup_style(hwnd, palette);

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
                let _ = DeleteObject(HGDIOBJ(edit_font.0));
            }
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
                let _ = DeleteObject(HGDIOBJ(edit_font.0));
            }
            destroy_brushes(&brushes);
            CREATE_FAILED.store(true, Ordering::Relaxed);
            return None;
        }
    };

    unsafe {
        let _ = send(edit, WM_SETFONT, edit_font.0 as usize, 1);
        let _ = send(list, WM_SETFONT, edit_font.0 as usize, 1);
    }
    ui::theme_control(list, palette.dark);
    ui::theme_control(edit, palette.dark);
    ui::install_smooth_listbox(list, ui::ListSurface::Card);

    let cue = platform::wide("搜索标题、正文、标签、拼音；#标签 筛选，\\# 搜索井号");
    unsafe {
        let _ = send(edit, EM_SETCUEBANNER, 1, cue.as_ptr() as isize);
        let _ = send(edit, EM_LIMITTEXT, 200, 0);
        let old = SetWindowLongPtrW(edit, GWLP_WNDPROC, edit_proc as *const () as usize as isize);
        if old == 0 {
            logging::error(&format!(
                "SetWindowLongPtrW(EDIT) failed: {:?}",
                GetLastError()
            ));
            let _ = DestroyWindow(hwnd);
            let _ = DeleteObject(HGDIOBJ(edit_font.0));
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
        edit_font,
        filtered: Vec::new(),
        items: Vec::new(),
        categories: Vec::new(),
        tag_counts: Vec::new(),
        sort_mode: crate::search::SortMode::Recent,
        active_category: None,
        active_tags: Vec::new(),
        copy_only: None,
        notice: None,
        paste_mode: false,
        keep_tabs: false,
        hotkey_hint: String::new(),
        palette,
        title_highlights: Vec::new(),
        content_highlights: Vec::new(),
        last_query: String::new(),
        report: None,
        notify: HWND::default(),
        brushes,
        dpi,
        hiding: false,
        quit_on_hide: false,
        monitor: None,
        opening: false,
        deferred_inactive: false,
        hover: HoverTarget::None,
        tracking_mouse: false,
    };

    let mut rect = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut rect);
    }
    layout_panel(&panel, rect.right, rect.bottom);

    Some(panel)
}

fn commit() {
    let info = PANEL.with(|slot| {
        let guard = slot.borrow();
        let panel = guard.as_ref()?;
        let index = panel.selected_index()?;
        let item_index = *panel.filtered.get(index)?;
        let item = panel.items.get(item_index)?;
        Some((
            item_index,
            item.id.clone(),
            item.category_id.clone().unwrap_or_default(),
            crate::template::render(&item.content),
            panel.paste_mode,
            panel.keep_tabs,
        ))
    });

    let Some((item_index, item_id, category_id, content, paste_mode, keep_tabs)) = info else {
        logging::info("panel commit ignored: no matching item");
        return;
    };

    let monitor_ok = PANEL.with(|slot| {
        slot.borrow()
            .as_ref()
            .map(|panel| panel.monitor.is_some())
            .unwrap_or(false)
    });

    let (report, notify, hwnd, copy_only) = PANEL.with(|slot| {
        let mut guard = slot.borrow_mut();
        let Some(panel) = guard.as_mut() else {
            return (None, HWND::default(), HWND::default(), None);
        };
        panel.hiding = true;
        let copy_only = panel.copy_only.clone();
        (panel.report.take(), panel.notify, panel.hwnd, copy_only)
    });

    if copy_only.is_some() || report.is_none() {
        unsafe {
            let _ = ShowWindow(hwnd, SW_HIDE);
        }
        let reason = copy_only.as_deref().unwrap_or("no_target");
        let (copied, snapshot) = crate::clipboard::set_text_with_snapshot(hwnd, &content);
        crate::storage::record_use(&item_id, "rejected");
        logging::info(&format!(
            "panel copy-only commit: index={item_index} category={category_id} copied={copied} non_text={} read_ok={}",
            snapshot.had_non_text,
            snapshot.read_ok
        ));
        if !notify.0.is_null() {
            let message = if !copied {
                format!("复制失败（{reason}）")
            } else {
                format!("已复制（{reason}；{}）", snapshot.previous_text_status())
            };
            crate::tray::balloon(notify, "QuickPhrase", &message);
        }
        finish_insert(SESSION.load(Ordering::Relaxed));
        return;
    }

    let report = match report {
        Some(report) => report,
        None => {
            finish_insert(SESSION.load(Ordering::Relaxed));
            return;
        }
    };

    logging::info(&format!(
        "panel commit: index={item_index} category={category_id}"
    ));
    logging::info(&format!("panel capture context: {}", report.summary()));

    unsafe {
        let _ = ShowWindow(hwnd, SW_HIDE);
    }

    if std::env::var_os("QUICKPHRASE_TEST_NO_INJECT").is_some() {
        logging::info("inject skipped by test flag");
        let target = HWND(report.target.hwnd as *mut core::ffi::c_void);
        if unsafe { IsWindow(Some(target)) }.as_bool() {
            unsafe {
                let _ = SetForegroundWindow(target);
            }
        }
        finish_insert(SESSION.load(Ordering::Relaxed));
        return;
    }

    let session = SESSION.load(Ordering::Relaxed);
    INJECTING.store(true, Ordering::Relaxed);
    let notify_raw = notify.0 as isize;
    let options = inject::Options {
        mode: if paste_mode {
            inject::InsertionMode::Paste
        } else {
            inject::InsertionMode::Unicode
        },
        keep_tabs,
    };
    let spawned = std::thread::Builder::new()
        .name("quickphrase-inject".into())
        .spawn(move || {
            let outcome = inject::run(
                &report,
                &content,
                monitor_ok,
                options,
                HWND(notify_raw as *mut core::ffi::c_void),
            );
            let message = outcome.describe();
            logging::info(&format!("inject outcome: {message}"));
            crate::storage::record_use(&item_id, outcome.status());
            if let Ok(mut slot) = INJECT_RESULT.lock() {
                *slot = Some(message);
            }
            if notify_raw != 0 {
                unsafe {
                    let _ = PostMessageW(
                        Some(HWND(notify_raw as *mut core::ffi::c_void)),
                        WM_INJECT_DONE,
                        WPARAM(session as usize),
                        LPARAM(0),
                    );
                }
            }
        });

    if spawned.is_err() {
        logging::error("failed to spawn injection thread");
        INJECTING.store(false, Ordering::Relaxed);
        finish_insert(session);
    }
}

pub fn finish_insert(session: u64) {
    if SESSION.load(Ordering::Relaxed) != session {
        return;
    }
    let monitor = PANEL.with(|slot| {
        slot.borrow_mut()
            .as_mut()
            .and_then(|panel| panel.monitor.take())
    });
    drop(monitor);
    INJECTING.store(false, Ordering::Relaxed);

    PANEL.with(|slot| {
        if let Some(panel) = slot.borrow_mut().as_mut() {
            panel.hiding = false;
        }
    });
}

fn apply_dpi(dpi: u32) {
    if dpi == 0 {
        return;
    }
    let changed = PANEL.with(|slot| {
        let mut guard = slot.borrow_mut();
        let panel = guard.as_mut()?;
        if panel.dpi == dpi {
            return None;
        }
        unsafe {
            let _ = DeleteObject(HGDIOBJ(panel.edit_font.0));
        }
        panel.edit_font = create_edit_font(dpi);
        panel.dpi = dpi;
        Some((panel.edit, panel.list, panel.edit_font))
    });

    if let Some((edit, list, font)) = changed {
        unsafe {
            let _ = send(edit, WM_SETFONT, font.0 as usize, 1);
            let _ = send(list, WM_SETFONT, font.0 as usize, 1);
            let _ = send(list, LB_SETITEMHEIGHT, 0, scaled(ITEM_HEIGHT, dpi) as isize);
        }
    }
}

fn place() {
    let (hwnd, caret, old_dpi) = PANEL.with(|slot| match slot.borrow().as_ref() {
        Some(panel) => {
            let caret = panel
                .report
                .as_ref()
                .and_then(|report| report.target.caret)
                .map(|caret| RECT {
                    left: caret[0],
                    top: caret[1],
                    right: caret[2],
                    bottom: caret[3],
                });
            (panel.hwnd, caret, panel.dpi)
        }
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
    let (hwnd, list, current, count) = PANEL.with(|slot| {
        let guard = slot.borrow();
        let Some(panel) = guard.as_ref() else {
            return (HWND::default(), HWND::default(), 0, 0);
        };
        let count = unsafe { send(panel.list, LB_GETCOUNT, 0, 0) }.0 as i32;
        let current = panel.selected_index().unwrap_or(0) as i32;
        (panel.hwnd, panel.list, current, count)
    });

    if list.0.is_null() || count <= 0 {
        return;
    }
    let next = (current + delta).clamp(0, count - 1);
    unsafe {
        let _ = send(list, LB_SETCURSEL, next as usize, 0);
        let _ = InvalidateRect(Some(hwnd), None, false);
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

struct CategoryChip {
    id: Option<String>,
    name: String,
    color: Option<COLORREF>,
    rect: RECT,
}

fn category_chips(panel: &Panel, rects: &LayoutRects, width: i32) -> Vec<CategoryChip> {
    let dpi = panel.dpi;
    let margin = scaled(14, dpi);
    let right_limit = width - margin;
    let height = scaled(26, dpi);
    let top = rects.search_shell.bottom + (scaled(CATEGORY_HEIGHT, dpi) - height) / 2;
    let mut x = margin;

    let mut entries: Vec<(Option<String>, String, Option<COLORREF>)> =
        vec![(None, "全部".to_string(), None)];
    for (id, name) in &panel.categories {
        entries.push((
            Some(id.clone()),
            name.clone(),
            Some(theme::category_dot_for_name(name)),
        ));
    }

    let mut chips = Vec::new();
    for (id, name, color) in entries {
        let (tw, _) = d2d::measure_style(&name, TextStyle::Label, dpi);
        let dot_extra = if color.is_some() { scaled(11, dpi) } else { 0 };
        let chip_w = tw.ceil() as i32 + scaled(22, dpi) + dot_extra;
        if x + chip_w > right_limit {
            break;
        }
        chips.push(CategoryChip {
            id,
            name,
            color,
            rect: RECT {
                left: x,
                top,
                right: x + chip_w,
                bottom: top + height,
            },
        });
        x += chip_w + scaled(6, dpi);
    }
    chips
}

struct TagChip {
    name: String,
    count: usize,
    rect: RECT,
}

fn tag_chips(panel: &Panel, rects: &LayoutRects, width: i32) -> Vec<TagChip> {
    if panel.tag_counts.is_empty() {
        return Vec::new();
    }
    let dpi = panel.dpi;
    let margin = scaled(14, dpi);
    let right_limit = width - margin;
    let height = scaled(22, dpi);
    let top = rects.category_bar_bottom + (scaled(TAG_HEIGHT, dpi) - height) / 2;
    let mut x = margin;
    let mut chips = Vec::new();
    for (name, count) in &panel.tag_counts {
        let label = format!("{name} {count}");
        let (tw, _) = d2d::measure_style(&label, TextStyle::Badge, dpi);
        let chip_w = tw.ceil() as i32 + scaled(20, dpi);
        if x + chip_w > right_limit {
            break;
        }
        chips.push(TagChip {
            name: name.clone(),
            count: *count,
            rect: RECT {
                left: x,
                top,
                right: x + chip_w,
                bottom: top + height,
            },
        });
        x += chip_w + scaled(6, dpi);
    }
    chips
}

fn point_in(rect: &RECT, x: i32, y: i32) -> bool {
    x >= rect.left && x <= rect.right && y >= rect.top && y <= rect.bottom
}

fn paint_panel(panel: &Panel, hdc: HDC, client: RECT) {
    let dpi = panel.dpi;
    let palette = panel.palette;
    let rects = compute_layout(panel, client.right, client.bottom);
    let chips = category_chips(panel, &rects, client.right);
    let tags = tag_chips(panel, &rects, client.right);
    let query_tags = crate::search::split_tag_query(&panel.last_query).0;
    let edit_focused = unsafe { GetFocus() } == panel.edit;

    d2d::paint_buffered(hdc, client, |c| {
        // 1. Window base + crisp outer border
        c.clear(palette.card);
        c.stroke_round(&client, d2d::scaled_f(12.0, dpi), palette.border, 1.0);

        // Subtle top drag pill when no status banner is shown
        if rects.banner.is_none() {
            let grip_w = scaled(36, dpi);
            let grip_h = scaled(3, dpi).max(2);
            let grip_l = (client.left + client.right - grip_w) / 2;
            let grip_t = scaled(4, dpi);
            let grip = RECT {
                left: grip_l,
                top: grip_t,
                right: grip_l + grip_w,
                bottom: grip_t + grip_h,
            };
            c.fill_round(&grip, grip_h as f32 * 0.5, palette.border_strong);
        }

        // 2. Optional top status banner
        if let Some(banner) = &rects.banner {
            let (bg, fg, icon, text) = if let Some(reason) = &panel.copy_only {
                (
                    palette.warning_soft,
                    palette.warning,
                    Icon::Warning,
                    format!("仅复制模式（{reason}）· 按 Enter 安全复制到剪贴板"),
                )
            } else if let Some(token) = &panel.notice {
                (
                    palette.danger_soft,
                    palette.danger,
                    Icon::Lock,
                    format!("存储只读（{}）· 仍可正常检索与输入", notice_label(token)),
                )
            } else {
                (
                    palette.primary_soft,
                    palette.primary_text,
                    Icon::Clipboard,
                    "粘贴兼容模式（Ctrl+V）· 原纯文本剪贴板受保护".to_string(),
                )
            };
            c.fill_rect(banner, bg);
            c.line(
                banner.left as f32,
                banner.bottom as f32 - 0.5,
                banner.right as f32,
                banner.bottom as f32 - 0.5,
                palette.divider,
                1.0,
            );
            c.icon(
                icon,
                (banner.left + scaled(22, dpi)) as f32,
                (banner.top + banner.bottom) as f32 * 0.5,
                d2d::scaled_f(13.0, dpi),
                fg,
            );
            let txt_rect = RECT {
                left: banner.left + scaled(34, dpi),
                top: banner.top,
                right: banner.right - scaled(12, dpi),
                bottom: banner.bottom,
            };
            c.draw_text(
                &text,
                &txt_rect,
                TextStyle::Label,
                dpi,
                fg,
                AlignH::Left,
                AlignV::Center,
            );
        }

        // 3. Search Capsule (rounded pill with soft focus accent)
        let search_r = d2d::scaled_f(11.0, dpi);
        let search_border = if edit_focused {
            palette.selection_border
        } else {
            palette.border
        };
        c.fill_stroke_round(
            &rects.search_shell,
            search_r,
            palette.field,
            search_border,
            if edit_focused {
                d2d::scaled_f(1.25, dpi)
            } else {
                1.0
            },
        );

        // Brand-tinted search icon tile on the left
        let icon_tile_sz = scaled(24, dpi);
        let icon_tile = RECT {
            left: rects.search_shell.left + scaled(8, dpi),
            top: rects.search_shell.top
                + (rects.search_shell.bottom - rects.search_shell.top - icon_tile_sz) / 2,
            right: rects.search_shell.left + scaled(8, dpi) + icon_tile_sz,
            bottom: rects.search_shell.top
                + (rects.search_shell.bottom - rects.search_shell.top + icon_tile_sz) / 2,
        };
        c.fill_round(&icon_tile, d2d::scaled_f(7.0, dpi), palette.primary_soft);
        c.icon(
            Icon::Search,
            (icon_tile.left + icon_tile.right) as f32 * 0.5,
            (icon_tile.top + icon_tile.bottom) as f32 * 0.5,
            d2d::scaled_f(14.0, dpi),
            palette.primary,
        );

        // Hotkey tactile keycap
        if !panel.hotkey_hint.is_empty() && rects.hotkey_kbd.right > rects.hotkey_kbd.left {
            c.kbd(&panel.hotkey_hint, &rects.hotkey_kbd, palette, dpi);
        }

        // Settings gear icon button
        if panel.hover == HoverTarget::Settings {
            c.fill_round(
                &rects.settings_btn,
                d2d::scaled_f(8.0, dpi),
                palette.primary_soft,
            );
        }
        c.icon(
            Icon::Gear,
            (rects.settings_btn.left + rects.settings_btn.right) as f32 * 0.5,
            (rects.settings_btn.top + rects.settings_btn.bottom) as f32 * 0.5,
            d2d::scaled_f(16.0, dpi),
            if panel.hover == HoverTarget::Settings {
                palette.primary
            } else {
                palette.secondary
            },
        );

        // 4. Category Chips Row + Hairline Divider
        for (idx, chip) in chips.iter().enumerate() {
            let selected = match (&chip.id, &panel.active_category) {
                (None, None) => true,
                (Some(a), Some(b)) => a == b,
                _ => false,
            };
            let hovered = panel.hover == HoverTarget::Category(idx);
            let radius = (chip.rect.bottom - chip.rect.top) as f32 * 0.5;

            if selected {
                let fill = if palette.dark {
                    palette.primary_hover
                } else {
                    palette.primary
                };
                c.fill_round(&chip.rect, radius, fill);
            } else if hovered {
                c.fill_stroke_round(
                    &chip.rect,
                    radius,
                    palette.hover,
                    palette.border_strong,
                    1.0,
                );
            } else {
                c.fill_round(&chip.rect, radius, palette.field);
            }

            let mut text_rect = chip.rect;
            if let Some(dot_color) = chip.color {
                if !selected {
                    c.fill_circle(
                        (chip.rect.left + scaled(12, dpi)) as f32,
                        (chip.rect.top + chip.rect.bottom) as f32 * 0.5,
                        d2d::scaled_f(3.0, dpi),
                        dot_color,
                    );
                    text_rect.left += scaled(10, dpi);
                }
            }
            c.draw_text(
                &chip.name,
                &text_rect,
                TextStyle::Label,
                dpi,
                if selected {
                    COLORREF(0xFFFFFF)
                } else {
                    palette.secondary
                },
                AlignH::Center,
                AlignV::Center,
            );
        }

        c.line(
            1.0,
            rects.category_bar_bottom as f32 - 0.5,
            (client.right - 1) as f32,
            rects.category_bar_bottom as f32 - 0.5,
            palette.divider,
            1.0,
        );

        // 4b. Tag cloud row
        for (idx, chip) in tags.iter().enumerate() {
            let selected = panel
                .active_tags
                .iter()
                .chain(query_tags.iter())
                .any(|tag| tag.to_lowercase() == chip.name.to_lowercase());
            let hovered = panel.hover == HoverTarget::Tag(idx);
            let radius = (chip.rect.bottom - chip.rect.top) as f32 * 0.5;
            if selected {
                let fill = if palette.dark {
                    palette.primary_hover
                } else {
                    palette.primary
                };
                c.fill_round(&chip.rect, radius, fill);
            } else if hovered {
                c.fill_stroke_round(
                    &chip.rect,
                    radius,
                    palette.hover,
                    palette.border_strong,
                    1.0,
                );
            } else {
                c.fill_round(&chip.rect, radius, palette.field);
            }
            c.draw_text(
                &format!("{} {}", chip.name, chip.count),
                &chip.rect,
                TextStyle::Badge,
                dpi,
                if selected {
                    COLORREF(0xFFFFFF)
                } else {
                    palette.secondary
                },
                AlignH::Center,
                AlignV::Center,
            );
        }
        if rects.tag_bar_bottom > rects.category_bar_bottom {
            c.line(
                1.0,
                rects.tag_bar_bottom as f32 - 0.5,
                (client.right - 1) as f32,
                rects.tag_bar_bottom as f32 - 0.5,
                palette.divider,
                1.0,
            );
        }

        // 5. Empty State Illustration when list has no matches
        if let Some(message) = empty_message(panel) {
            let cx = (rects.list.left + rects.list.right) as f32 * 0.5;
            let cy = (rects.list.top + rects.list.bottom) as f32 * 0.5 - scaled(18, dpi) as f32;
            let badge_sz = scaled(44, dpi);
            let badge = RECT {
                left: cx as i32 - badge_sz / 2,
                top: cy as i32 - badge_sz / 2,
                right: cx as i32 + badge_sz / 2,
                bottom: cy as i32 + badge_sz / 2,
            };
            c.fill_round(&badge, d2d::scaled_f(14.0, dpi), palette.field);
            c.icon(
                Icon::Search,
                cx,
                cy,
                d2d::scaled_f(20.0, dpi),
                palette.muted,
            );

            let title_rect = RECT {
                left: rects.list.left + scaled(20, dpi),
                top: badge.bottom + scaled(10, dpi),
                right: rects.list.right - scaled(20, dpi),
                bottom: badge.bottom + scaled(32, dpi),
            };
            c.draw_text(
                message,
                &title_rect,
                TextStyle::Heading,
                dpi,
                palette.heading,
                AlignH::Center,
                AlignV::Center,
            );

            let hint = if !panel.active_tags.is_empty() {
                "当前标签筛选无结果，点击标签芯片取消选择"
            } else if !panel.last_query.is_empty() {
                "尝试更换关键词，或点击「全部」分类"
            } else {
                "点击右下角「管理短语」添加你的常用文本"
            };
            let sub_rect = RECT {
                left: rects.list.left + scaled(20, dpi),
                top: title_rect.bottom + scaled(2, dpi),
                right: rects.list.right - scaled(20, dpi),
                bottom: title_rect.bottom + scaled(22, dpi),
            };
            c.draw_text(
                hint,
                &sub_rect,
                TextStyle::Caption,
                dpi,
                palette.muted,
                AlignH::Center,
                AlignV::Center,
            );
        }

        // 6. Tactile Footer Bar
        c.fill_rect(&rects.footer, palette.card_subtle);
        c.line(
            1.0,
            rects.footer.top as f32 + 0.5,
            (client.right - 1) as f32,
            rects.footer.top as f32 + 0.5,
            palette.divider,
            1.0,
        );

        let verb = if panel.copy_only.is_some() {
            "复制"
        } else if panel.paste_mode {
            match selected_paste_ok(panel) {
                Some(false) => "复制",
                _ => "粘贴",
            }
        } else {
            "输入"
        };

        let mut fx = scaled(14, dpi);
        let kbd_h = scaled(20, dpi);
        let kbd_top = rects.footer.top + (rects.footer.bottom - rects.footer.top - kbd_h) / 2;
        let max_fx = rects.manage_btn.left - scaled(8, dpi);

        for (key, desc) in [("↑↓", "选择"), ("↵", verb), ("Esc", "关闭")] {
            let (kw, _) = c.measure_style(key, TextStyle::Mono, dpi);
            let (dw, _) = c.measure_style(desc, TextStyle::Caption, dpi);
            let kbd_w = (kw.ceil() as i32 + scaled(10, dpi)).max(scaled(22, dpi));
            let desc_w = dw.ceil() as i32 + scaled(4, dpi);
            if fx + kbd_w + desc_w > max_fx {
                break;
            }
            let krect = RECT {
                left: fx,
                top: kbd_top,
                right: fx + kbd_w,
                bottom: kbd_top + kbd_h,
            };
            c.kbd(key, &krect, palette, dpi);
            fx = krect.right + scaled(5, dpi);
            let drect = RECT {
                left: fx,
                top: rects.footer.top,
                right: fx + desc_w,
                bottom: rects.footer.bottom,
            };
            c.draw_text(
                desc,
                &drect,
                TextStyle::Caption,
                dpi,
                palette.muted,
                AlignH::Left,
                AlignV::Center,
            );
            fx = drect.right + scaled(10, dpi);
        }

        // Right-aligned "管理短语" pill button
        let manage_hover = panel.hover == HoverTarget::Manage;
        c.fill_stroke_round(
            &rects.manage_btn,
            d2d::scaled_f(8.0, dpi),
            if manage_hover {
                palette.selection
            } else {
                palette.primary_soft
            },
            if manage_hover {
                palette.selection_border
            } else {
                palette.primary_soft
            },
            1.0,
        );
        c.icon(
            Icon::Grid,
            (rects.manage_btn.left + scaled(15, dpi)) as f32,
            (rects.manage_btn.top + rects.manage_btn.bottom) as f32 * 0.5,
            d2d::scaled_f(12.0, dpi),
            palette.primary_text,
        );
        let mtxt = RECT {
            left: rects.manage_btn.left + scaled(25, dpi),
            top: rects.manage_btn.top,
            right: rects.manage_btn.right - scaled(6, dpi),
            bottom: rects.manage_btn.bottom,
        };
        c.draw_text(
            "管理短语",
            &mtxt,
            TextStyle::Label,
            dpi,
            palette.primary_text,
            AlignH::Center,
            AlignV::Center,
        );
    });
}

fn draw_item(panel: &Panel, draw: &DRAWITEMSTRUCT) {
    if draw.itemID == u32::MAX {
        return;
    }
    let idx = draw.itemID as usize;
    let Some(item) = panel
        .filtered
        .get(idx)
        .and_then(|index| panel.items.get(*index))
    else {
        return;
    };

    let dpi = panel.dpi;
    let palette = panel.palette;
    let rect = draw.rcItem;
    let selected = draw.itemState.0 & ODS_SELECTED.0 != 0;
    let title_hl = panel.title_highlights.get(idx).copied().flatten();
    let content_hl = panel.content_highlights.get(idx).copied().flatten();
    let rendered = crate::template::render(&item.content);
    let preview = rendered.replace("\r\n", " ↵ ").replace('\n', " ↵ ");

    let right_pad = if ui::listbox_scrollable(draw.hwndItem) {
        scaled(13, dpi)
    } else {
        scaled(6, dpi)
    };
    d2d::paint_buffered(draw.hDC, rect, |c| {
        c.fill_rect(&rect, palette.card);

        let pill = RECT {
            left: rect.left + scaled(6, dpi),
            top: rect.top + scaled(2, dpi),
            right: (rect.right - right_pad).max(rect.left + scaled(20, dpi)),
            bottom: rect.bottom - scaled(2, dpi),
        };
        if selected {
            c.fill_stroke_round(
                &pill,
                d2d::scaled_f(10.0, dpi),
                palette.selection,
                palette.selection_border,
                1.0,
            );
            // 3px left accent indicator pill
            let accent_bar = RECT {
                left: pill.left + scaled(3, dpi),
                top: pill.top + scaled(10, dpi),
                right: pill.left + scaled(6, dpi),
                bottom: pill.bottom - scaled(10, dpi),
            };
            c.fill_round(&accent_bar, d2d::scaled_f(2.0, dpi), palette.primary);
        }

        // Semantic 36x36 rounded icon tile (matches demo w-9 h-9 rounded-[10px])
        let icon_sz = scaled(36, dpi);
        let icon_rect = RECT {
            left: pill.left + scaled(12, dpi),
            top: pill.top + ((pill.bottom - pill.top) - icon_sz) / 2,
            right: pill.left + scaled(12, dpi) + icon_sz,
            bottom: pill.top + ((pill.bottom - pill.top) + icon_sz) / 2,
        };
        let cat_dot = theme::category_dot_for_name(&item.category);
        let (tile_bg, glyph_icon, glyph_color) = if item.pinned {
            (palette.primary_soft, Icon::Pin, palette.primary)
        } else if item.content.contains('\n') {
            (palette.field, Icon::Multiline, cat_dot)
        } else {
            (palette.field, Icon::Chat, cat_dot)
        };
        c.fill_round(&icon_rect, d2d::scaled_f(10.0, dpi), tile_bg);
        c.icon(
            glyph_icon,
            (icon_rect.left + icon_rect.right) as f32 * 0.5,
            (icon_rect.top + icon_rect.bottom) as f32 * 0.5,
            d2d::scaled_f(16.0, dpi),
            glyph_color,
        );

        let text_left = icon_rect.right + scaled(11, dpi);

        // Right-side indicator: ↵ keycap for selected, or index 1..9
        let mut right_edge = pill.right - scaled(10, dpi);
        if selected {
            let kbd_w = scaled(22, dpi);
            let kbd_h = scaled(20, dpi);
            let krect = RECT {
                left: right_edge - kbd_w,
                top: pill.top + scaled(8, dpi),
                right: right_edge,
                bottom: pill.top + scaled(8, dpi) + kbd_h,
            };
            c.kbd("↵", &krect, palette, dpi);
            right_edge = krect.left - scaled(6, dpi);
        } else if idx < 9 {
            let num_w = scaled(16, dpi);
            let nrect = RECT {
                left: right_edge - num_w,
                top: pill.top + scaled(8, dpi),
                right: right_edge,
                bottom: pill.top + scaled(28, dpi),
            };
            c.draw_text(
                &(idx + 1).to_string(),
                &nrect,
                TextStyle::Mono,
                dpi,
                palette.muted,
                AlignH::Center,
                AlignV::Center,
            );
            right_edge = nrect.left - scaled(6, dpi);
        }

        // Category pill badge
        if !item.category.is_empty() {
            let (cw, _) = c.measure_style(&item.category, TextStyle::Badge, dpi);
            let tag_w = (cw.ceil() as i32 + scaled(14, dpi)).min(scaled(86, dpi));
            let tag_h = scaled(20, dpi);
            let tag_rect = RECT {
                left: (right_edge - tag_w).max(text_left + scaled(60, dpi)),
                top: pill.top + scaled(8, dpi),
                right: right_edge,
                bottom: pill.top + scaled(8, dpi) + tag_h,
            };
            c.fill_stroke_round(
                &tag_rect,
                d2d::scaled_f(6.0, dpi),
                palette.card_subtle,
                palette.divider,
                1.0,
            );
            c.draw_text(
                &item.category,
                &tag_rect,
                TextStyle::Badge,
                dpi,
                palette.secondary,
                AlignH::Center,
                AlignV::Center,
            );
            right_edge = tag_rect.left - scaled(8, dpi);
        }

        // Title
        let title_rect = RECT {
            left: text_left,
            top: pill.top + scaled(7, dpi),
            right: right_edge.max(text_left + 10),
            bottom: pill.top + scaled(29, dpi),
        };
        c.draw_text_highlighted(
            &item.title,
            &title_rect,
            TextStyle::Heading,
            dpi,
            palette.heading,
            title_hl,
            palette.highlight,
            palette.highlight_text,
            d2d::scaled_f(4.0, dpi),
        );

        // Preview line
        let preview_rect = RECT {
            left: text_left,
            top: pill.top + scaled(30, dpi),
            right: pill.right - scaled(12, dpi),
            bottom: pill.bottom - scaled(6, dpi),
        };
        // Highlight offsets were computed against the original `item.content`;
        // only reuse them when neither template rendering nor CRLF replacement
        // changed the text.
        let safe_hl = if rendered == item.content && rendered == preview {
            content_hl
        } else {
            None
        };
        c.draw_text_highlighted(
            &preview,
            &preview_rect,
            TextStyle::Caption,
            dpi,
            palette.secondary,
            safe_hl,
            palette.highlight,
            palette.highlight_text,
            d2d::scaled_f(3.0, dpi),
        );
    });
}

type Highlight = Option<(usize, usize)>;
type FilterResult = (Vec<usize>, Vec<Highlight>, Vec<Highlight>);

#[cfg(test)]
fn filter_items(items: &[Item], category: Option<&str>, query: &str) -> FilterResult {
    filter_items_mode(items, category, query, &[], crate::search::SortMode::Recent)
}

fn filter_items_mode(
    items: &[Item],
    category: Option<&str>,
    query: &str,
    active_tags: &[String],
    sort_mode: crate::search::SortMode,
) -> FilterResult {
    let (query_tags, text_query) = crate::search::split_tag_query(query);
    let mut wanted = active_tags.to_vec();
    wanted.extend(query_tags);

    let subset: Vec<usize> = items
        .iter()
        .enumerate()
        .filter(|(_, item)| {
            let category_ok = match category {
                Some(id) => item.category_id.as_deref() == Some(id),
                None => true,
            };
            let tag_ok = wanted.iter().all(|tag| {
                let tag = tag.to_lowercase();
                item.tags
                    .iter()
                    .any(|item_tag| item_tag.to_lowercase() == tag)
            });
            category_ok && tag_ok
        })
        .map(|(index, _)| index)
        .collect();

    let docs: Vec<crate::search::Doc> = subset
        .iter()
        .map(|&index| {
            let item = &items[index];
            crate::search::Doc {
                id: &item.id,
                title: &item.title,
                content: &item.content,
                blob: &item.blob,
                pinned: item.pinned,
                use_count: item.use_count,
                last_used_at: item.last_used_at.as_deref(),
                updated_at: &item.updated_at,
                sort_order: item.sort_order,
            }
        })
        .collect();

    let hits =
        crate::search::rank_with_mode(&docs, text_query, crate::search::DEFAULT_LIMIT, sort_mode);
    let mut filtered = Vec::with_capacity(hits.len());
    let mut title_highlights = Vec::with_capacity(hits.len());
    let mut content_highlights = Vec::with_capacity(hits.len());
    for hit in hits {
        filtered.push(subset[hit.index]);
        title_highlights.push(hit.title_highlight);
        content_highlights.push(hit.content_highlight);
    }
    (filtered, title_highlights, content_highlights)
}

fn notice_label(token: &str) -> &str {
    match token {
        "schema_newer" => "数据由更新版本写入",
        "schema_older" => "数据版本过旧",
        "settings_schema_newer" => "设置由更新版本写入",
        "rules_version_newer" => "规则由更新版本写入",
        "data_recovery_failed" => "数据恢复失败",
        "settings_recovery_failed" => "设置恢复失败",
        "rules_recovery_failed" => "规则恢复失败",
        "initial_save_failed" => "首次保存失败",
        "settings_write_failed" => "设置保存失败",
        "rules_write_failed" => "规则保存失败",
        "data_dir_unavailable" => "数据目录不可用",
        other => other,
    }
}

#[allow(dead_code)]
fn enter_verb(paste_mode: bool, paste_ok: Option<bool>) -> &'static str {
    if !paste_mode {
        return "Enter 输入";
    }
    match paste_ok {
        Some(false) => "Enter 复制（不可粘贴）",
        _ => "Enter 粘贴（Ctrl+V）",
    }
}

#[allow(dead_code)]
fn footer_text_for(copy_only: Option<&str>, notice: Option<&str>, verb: &str) -> String {
    match (copy_only, notice) {
        (Some(reason), Some(token)) => {
            format!("只读（{}） · 仅复制（{reason}）", notice_label(token))
        }
        (Some(reason), None) => format!("仅复制（{reason}） · Enter 复制 · Esc 关闭"),
        (None, Some(token)) => format!(
            "存储只读（{}） · ↑↓ 选择 · {} · Esc 关闭",
            notice_label(token),
            verb
        ),
        (None, None) => format!("↑↓ 选择 · {} · Esc 关闭", verb),
    }
}

fn selected_paste_ok(panel: &Panel) -> Option<bool> {
    let index = panel.selected_index()?;
    let item_index = *panel.filtered.get(index)?;
    let item = panel.items.get(item_index)?;
    Some(crate::inject::paste_preview_ok(
        &crate::template::render(&item.content),
        panel.keep_tabs,
    ))
}

fn empty_message_for(
    items: usize,
    filtered: usize,
    query: &str,
    has_category: bool,
    has_tags: bool,
    load_failed: bool,
) -> Option<&'static str> {
    if filtered != 0 {
        return None;
    }
    if items == 0 {
        if load_failed {
            return Some("加载失败，已进入只读");
        }
        return Some("还没有短语");
    }
    if !query.is_empty() || has_tags {
        return Some("没有匹配的短语");
    }
    if has_category {
        return Some("该分类下还没有短语");
    }
    None
}

fn data_load_failed(notice: Option<&str>) -> bool {
    matches!(
        notice,
        Some(
            "data_recovery_failed"
                | "schema_newer"
                | "schema_older"
                | "data_dir_unavailable"
                | "initial_save_failed"
        )
    )
}

fn empty_message(panel: &Panel) -> Option<&'static str> {
    empty_message_for(
        panel.items.len(),
        panel.filtered.len(),
        &panel.last_query,
        panel.active_category.is_some(),
        !panel.active_tags.is_empty(),
        data_load_failed(panel.notice.as_deref()),
    )
}

fn populate(query: &str) {
    let (hwnd, list, filtered, title_highlights, content_highlights) = PANEL.with(|slot| {
        let guard = slot.borrow();
        let Some(panel) = guard.as_ref() else {
            return (
                HWND::default(),
                HWND::default(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
            );
        };
        let (filtered, title_highlights, content_highlights) = filter_items_mode(
            &panel.items,
            panel.active_category.as_deref(),
            query,
            &panel.active_tags,
            panel.sort_mode,
        );
        (
            panel.hwnd,
            panel.list,
            filtered,
            title_highlights,
            content_highlights,
        )
    });

    if list.0.is_null() {
        return;
    }

    let titles: Vec<Vec<u16>> = PANEL.with(|slot| {
        let guard = slot.borrow();
        let Some(panel) = guard.as_ref() else {
            return Vec::new();
        };
        filtered
            .iter()
            .map(|index| platform::wide(&panel.items[*index].title))
            .collect()
    });

    PANEL.with(|slot| {
        if let Some(panel) = slot.borrow_mut().as_mut() {
            panel.filtered = filtered;
            panel.title_highlights = title_highlights;
            panel.content_highlights = content_highlights;
            panel.last_query = query.trim().to_string();
            logging::info(&format!("panel filter: matches={}", panel.filtered.len()));
        }
    });

    unsafe {
        let _ = send(list, LB_RESETCONTENT, 0, 0);
        for title in &titles {
            let _ = send(list, LB_ADDSTRING, 0, title.as_ptr() as isize);
        }
        if !titles.is_empty() {
            let _ = send(list, LB_SETCURSEL, 0, 0);
        }
        let _ = ShowWindow(list, if titles.is_empty() { SW_HIDE } else { SW_SHOW });
        // Filtering can shrink the list; erase the list itself so rows past the
        // new match count cannot leave ghost cards behind.
        let _ = InvalidateRect(Some(list), None, true);
        let _ = InvalidateRect(Some(hwnd), None, false);
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
    if message == WM_MOUSEWHEEL {
        let list = PANEL.with(|slot| slot.borrow().as_ref().map(|p| p.list).unwrap_or_default());
        if !list.0.is_null() {
            let _ = SendMessageW(list, message, Some(wparam), Some(lparam));
            return LRESULT(0);
        }
    }

    if (message == WM_KEYUP || message == WM_SYSKEYUP) && wparam.0 as u16 == VK_RETURN.0 {
        if ime_composing(hwnd) {
            ENTER_ARMED.store(false, Ordering::Relaxed);
            return call_default_edit(hwnd, message, wparam, lparam);
        }
        if ENTER_ARMED.swap(false, Ordering::Relaxed) {
            commit();
        }
        return LRESULT(0);
    }

    if message == WM_KEYDOWN || message == WM_SYSKEYDOWN {
        let key = wparam.0 as u16;
        if key == VK_RETURN.0 {
            if ime_composing(hwnd) {
                ENTER_ARMED.store(false, Ordering::Relaxed);
                return call_default_edit(hwnd, message, wparam, lparam);
            }
            ENTER_ARMED.store(true, Ordering::Relaxed);
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

fn hit_test_hover(panel: &Panel, width: i32, height: i32, x: i32, y: i32) -> HoverTarget {
    let rects = compute_layout(panel, width, height);
    if point_in(&rects.settings_btn, x, y) {
        return HoverTarget::Settings;
    }
    if point_in(&rects.manage_btn, x, y) {
        return HoverTarget::Manage;
    }
    for (idx, chip) in category_chips(panel, &rects, width).iter().enumerate() {
        if point_in(&chip.rect, x, y) {
            return HoverTarget::Category(idx);
        }
    }
    for (idx, chip) in tag_chips(panel, &rects, width).iter().enumerate() {
        if point_in(&chip.rect, x, y) {
            return HoverTarget::Tag(idx);
        }
    }
    HoverTarget::None
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_TIMER if wparam.0 == FADE_TIMER => {
            let step = FADE_STEP.fetch_add(1, Ordering::Relaxed) + 1;
            let alpha = ((step.min(10) * 255) / 10) as u8;
            let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), alpha, LWA_ALPHA);
            if step >= 10 {
                let _ = KillTimer(Some(hwnd), FADE_TIMER);
            }
            LRESULT(0)
        }
        WM_SHOWWINDOW if wparam.0 == 0 => {
            let _ = KillTimer(Some(hwnd), FADE_TIMER);
            let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA);
            LRESULT(0)
        }
        WM_SIZE => {
            let width = (lparam.0 & 0xffff) as i32;
            let height = ((lparam.0 >> 16) & 0xffff) as i32;
            PANEL.with(|slot| {
                if let Some(panel) = slot.borrow().as_ref() {
                    layout_panel(panel, width, height);
                }
            });
            LRESULT(0)
        }
        WM_COMMAND => {
            let control = (wparam.0 & 0xffff) as i32;
            let code = ((wparam.0 >> 16) & 0xffff) as u32;
            if control == EDIT_ID && code == EN_CHANGE {
                let edit = HWND(lparam.0 as *mut core::ffi::c_void);
                let text = read_text(edit);
                populate(&text);
            } else if control == EDIT_ID && (code == EN_SETFOCUS || code == EN_KILLFOCUS) {
                let _ = InvalidateRect(Some(hwnd), None, false);
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
                    let _ = SetFocus(Some(edit));
                }
                let _ = InvalidateRect(Some(hwnd), None, false);
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
            let (field, heading, brush) = PANEL.with(|slot| {
                slot.borrow()
                    .as_ref()
                    .map(|panel| {
                        (
                            panel.palette.field,
                            panel.palette.heading,
                            panel.brushes.search.0 as isize,
                        )
                    })
                    .unwrap_or((COLORREF(0xFFFFFF), COLORREF(0), 0))
            });
            SetBkMode(hdc, TRANSPARENT);
            SetBkColor(hdc, field);
            SetTextColor(hdc, heading);
            LRESULT(brush)
        }
        WM_CTLCOLORLISTBOX => {
            let hdc = HDC(wparam.0 as *mut core::ffi::c_void);
            let (card, brush) = PANEL.with(|slot| {
                slot.borrow()
                    .as_ref()
                    .map(|panel| (panel.palette.card, panel.brushes.background.0 as isize))
                    .unwrap_or((COLORREF(0xFFFFFF), 0))
            });
            SetBkColor(hdc, card);
            LRESULT(brush)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            let mut paint = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut paint);
            let mut client = RECT::default();
            let _ = GetClientRect(hwnd, &mut client);
            PANEL.with(|slot| {
                if let Some(panel) = slot.borrow().as_ref() {
                    paint_panel(panel, hdc, client);
                }
            });
            let _ = EndPaint(hwnd, &paint);
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let x = (lparam.0 & 0xffff) as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xffff) as i16 as i32;
            let mut client = RECT::default();
            let _ = GetClientRect(hwnd, &mut client);
            let need_redraw = PANEL.with(|slot| {
                let mut guard = slot.borrow_mut();
                let Some(panel) = guard.as_mut() else {
                    return false;
                };
                if !panel.tracking_mouse {
                    panel.tracking_mouse = true;
                    let mut track = TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    let _ = TrackMouseEvent(&mut track);
                }
                let next = hit_test_hover(panel, client.right, client.bottom, x, y);
                if next != panel.hover {
                    panel.hover = next;
                    true
                } else {
                    false
                }
            });
            if need_redraw {
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            let need_redraw = PANEL.with(|slot| {
                let mut guard = slot.borrow_mut();
                let Some(panel) = guard.as_mut() else {
                    return false;
                };
                panel.tracking_mouse = false;
                if panel.hover != HoverTarget::None {
                    panel.hover = HoverTarget::None;
                    true
                } else {
                    false
                }
            });
            if need_redraw {
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let x = (lparam.0 & 0xffff) as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xffff) as i16 as i32;
            let mut client = RECT::default();
            let _ = GetClientRect(hwnd, &mut client);

            enum ClickAction {
                None,
                FocusSearch(HWND),
                OpenSettings(HWND),
                OpenManage(HWND),
                Category(Option<String>, HWND),
                Tag(String, HWND),
            }

            let action = PANEL.with(|slot| {
                let guard = slot.borrow();
                let Some(panel) = guard.as_ref() else {
                    return ClickAction::None;
                };
                let rects = compute_layout(panel, client.right, client.bottom);
                if point_in(&rects.settings_btn, x, y) {
                    return ClickAction::OpenSettings(panel.notify);
                }
                if point_in(&rects.manage_btn, x, y) {
                    return ClickAction::OpenManage(panel.notify);
                }
                if point_in(&rects.search_shell, x, y) {
                    return ClickAction::FocusSearch(panel.edit);
                }
                for chip in category_chips(panel, &rects, client.right) {
                    if point_in(&chip.rect, x, y) {
                        return ClickAction::Category(chip.id, panel.edit);
                    }
                }
                for chip in tag_chips(panel, &rects, client.right) {
                    if point_in(&chip.rect, x, y) {
                        return ClickAction::Tag(chip.name, panel.edit);
                    }
                }
                ClickAction::None
            });

            match action {
                ClickAction::OpenSettings(notify) => {
                    if !notify.0.is_null() {
                        let _ = PostMessageW(Some(notify), WM_COMMAND, WPARAM(1004), LPARAM(0));
                    }
                    hide_and_restore();
                    logging::info("panel settings link");
                }
                ClickAction::OpenManage(notify) => {
                    if !notify.0.is_null() {
                        let _ = PostMessageW(Some(notify), WM_COMMAND, WPARAM(1002), LPARAM(0));
                    }
                    hide_and_restore();
                    logging::info("panel manage link");
                }
                ClickAction::FocusSearch(edit) => {
                    let _ = SetFocus(Some(edit));
                }
                ClickAction::Category(id, edit) => {
                    logging::info(&format!("panel category: {id:?}"));
                    let changed = PANEL.with(|slot| {
                        let mut guard = slot.borrow_mut();
                        let Some(panel) = guard.as_mut() else {
                            return false;
                        };
                        if panel.active_category == id {
                            return false;
                        }
                        panel.active_category = id;
                        true
                    });
                    if changed && !edit.0.is_null() {
                        let query = read_text(edit);
                        populate(&query);
                    }
                    if !edit.0.is_null() {
                        let _ = SetFocus(Some(edit));
                    }
                }
                ClickAction::Tag(name, edit) => {
                    let changed = PANEL.with(|slot| {
                        let mut guard = slot.borrow_mut();
                        let Some(panel) = guard.as_mut() else {
                            return false;
                        };
                        if let Some(position) = panel
                            .active_tags
                            .iter()
                            .position(|tag| tag.to_lowercase() == name.to_lowercase())
                        {
                            panel.active_tags.remove(position);
                        } else {
                            panel.active_tags.push(name.to_lowercase());
                        }
                        true
                    });
                    if changed && !edit.0.is_null() {
                        let query = read_text(edit);
                        populate(&query);
                    }
                    if !edit.0.is_null() {
                        let _ = SetFocus(Some(edit));
                    }
                }
                ClickAction::None => {}
            }
            LRESULT(0)
        }
        WM_NCHITTEST => {
            let mut pt = POINT {
                x: (lparam.0 & 0xffff) as i16 as i32,
                y: ((lparam.0 >> 16) & 0xffff) as i16 as i32,
            };
            if ScreenToClient(hwnd, &mut pt).as_bool() {
                let mut client = RECT::default();
                let _ = GetClientRect(hwnd, &mut client);
                let is_interactive = PANEL.with(|slot| {
                    let guard = slot.borrow();
                    let Some(panel) = guard.as_ref() else {
                        return true;
                    };
                    let rects = compute_layout(panel, client.right, client.bottom);
                    if point_in(&rects.settings_btn, pt.x, pt.y)
                        || point_in(&rects.manage_btn, pt.x, pt.y)
                    {
                        return true;
                    }
                    for chip in category_chips(panel, &rects, client.right) {
                        if point_in(&chip.rect, pt.x, pt.y) {
                            return true;
                        }
                    }
                    for chip in tag_chips(panel, &rects, client.right) {
                        if point_in(&chip.rect, pt.x, pt.y) {
                            return true;
                        }
                    }
                    if point_in(&rects.search_shell, pt.x, pt.y)
                        && !point_in(&rects.hotkey_kbd, pt.x, pt.y)
                    {
                        return true;
                    }
                    false
                });
                if !is_interactive {
                    return LRESULT(HTCAPTION as isize);
                }
            }
            LRESULT(HTCLIENT as isize)
        }
        WM_NCLBUTTONDBLCLK => LRESULT(0),
        WM_EXITSIZEMOVE => {
            let edit = PANEL.with(|slot| {
                slot.borrow()
                    .as_ref()
                    .map(|panel| panel.edit)
                    .unwrap_or_default()
            });
            if !edit.0.is_null() {
                let _ = SetFocus(Some(edit));
            }
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            let list = PANEL.with(|slot| {
                slot.borrow()
                    .as_ref()
                    .map(|panel| panel.list)
                    .unwrap_or_default()
            });
            if !list.0.is_null() {
                let _ = SendMessageW(list, message, Some(wparam), Some(lparam));
                return LRESULT(0);
            }
            LRESULT(0)
        }
        WM_ACTIVATE => {
            let active = (wparam.0 & 0xffff) as u32;
            if active == WA_INACTIVE {
                let (hiding, opening) = PANEL.with(|slot| {
                    slot.borrow()
                        .as_ref()
                        .map(|panel| (panel.hiding, panel.opening))
                        .unwrap_or((true, false))
                });
                if hiding {
                    return LRESULT(0);
                }
                if opening {
                    PANEL.with(|slot| {
                        if let Some(panel) = slot.borrow_mut().as_mut() {
                            panel.deferred_inactive = true;
                        }
                    });
                } else if std::env::var_os("QUICKPHRASE_TEST_ALLOW_UNFOCUSED_PANEL").is_none() {
                    hide_only();
                }
            } else {
                PANEL.with(|slot| {
                    if let Some(panel) = slot.borrow_mut().as_mut() {
                        panel.deferred_inactive = false;
                    }
                });
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
            let _ = SetWindowPos(
                hwnd,
                None,
                suggested.left,
                suggested.top,
                suggested.right - suggested.left,
                suggested.bottom - suggested.top,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}

pub fn bench(iterations: u32) -> (u128, u128, u128) {
    if iterations == 0 {
        return (0, 0, 0);
    }
    let base_items: Vec<Item> = (0..50)
        .map(|index| Item {
            id: format!("bench-{index}"),
            title: format!("项目更新说明 {index}"),
            content: "这是第 {index} 条正文，包含接口、测试与发布计划。".to_string(),
            category: "工作沟通".to_string(),
            category_id: None,
            tags: Vec::new(),
            blob: format!(
                "项目更新说明 {index}\n这是第 {index} 条正文，包含接口、测试与发布计划。"
            ),
            pinned: index % 7 == 0,
            last_used_at: None,
            updated_at: "2026-01-01T00:00:00Z".to_string(),
            sort_order: index,
            use_count: index as u64,
        })
        .collect();
    let mut samples = Vec::with_capacity(iterations as usize);
    for _ in 0..iterations {
        let elapsed = present(
            None,
            None,
            HWND::default(),
            PanelData {
                items: base_items.clone(),
                categories: vec![("bench".to_string(), "工作沟通".to_string())],
                tag_counts: Vec::new(),
                notice: None,
                copy_only: None,
                paste_mode: false,
                keep_tabs: false,
                hotkey_hint: "Ctrl+Alt+Q".to_string(),
                sort_mode: crate::search::SortMode::Recent,
            },
            false,
            false,
        );
        samples.push(elapsed);
        hide_and_restore();
    }
    samples.sort_unstable();
    let p50 = samples[samples.len() / 2];
    let p95_rank = (samples.len() * 95).div_ceil(100);
    let p95 = samples[p95_rank.saturating_sub(1).min(samples.len() - 1)];
    let max = *samples.last().unwrap_or(&0);
    (p50, p95, max)
}

pub fn shutdown() {
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
        let _ = DeleteObject(HGDIOBJ(panel.edit_font.0));
    }
    destroy_brushes(&panel.brushes);
}

#[cfg(test)]
mod tests {
    use super::{count_tags, filter_items, filter_items_mode, scaled, Item};

    fn rank_ids(items: &[Item], query: &str) -> Vec<usize> {
        filter_items(items, None, query).0
    }

    fn items() -> Vec<Item> {
        fn item(
            id: &str,
            title: &str,
            content: &str,
            category: &str,
            category_id: &str,
            sort_order: i32,
        ) -> Item {
            Item {
                id: id.to_string(),
                title: title.to_string(),
                content: content.to_string(),
                category: category.to_string(),
                category_id: Some(category_id.to_string()),
                tags: Vec::new(),
                blob: format!("{}\n{}", title.to_lowercase(), content.to_lowercase()),
                pinned: false,
                last_used_at: None,
                updated_at: "2026-01-01T00:00:00Z".to_string(),
                sort_order,
                use_count: 0,
            }
        }
        vec![
            item(
                "1",
                "项目进度回复",
                "您好，目前该功能已完成开发。",
                "工作沟通",
                "c1",
                0,
            ),
            item("2", "收到回复", "好的，已收到。", "工作沟通", "c1", 1),
            item(
                "3",
                "接口开发说明",
                "返回格式为 JSON。",
                "开发相关",
                "c2",
                2,
            ),
        ]
    }

    #[test]
    fn empty_query_returns_every_item() {
        assert_eq!(rank_ids(&items(), ""), vec![0, 1, 2]);
    }

    #[test]
    fn most_used_mode_orders_by_use_count_then_score() {
        let mut items = items();
        items[0].use_count = 1;
        items[1].use_count = 9;
        items[2].use_count = 4;
        let (filtered, _, _) =
            filter_items_mode(&items, None, "", &[], crate::search::SortMode::MostUsed);
        assert_eq!(filtered, vec![1, 2, 0]);
    }

    #[test]
    fn most_used_considers_items_beyond_the_recent_limit() {
        let count = crate::search::DEFAULT_LIMIT + 5;
        let sample = items().remove(0);
        let mut many: Vec<_> = (0..count)
            .map(|index| {
                let mut item = sample.clone();
                item.id = index.to_string();
                item.sort_order = index as i32;
                item
            })
            .collect();
        many[count - 1].use_count = 10;
        let (filtered, highlights, _) =
            filter_items_mode(&many, None, "项目", &[], crate::search::SortMode::MostUsed);
        assert_eq!(filtered.len(), crate::search::DEFAULT_LIMIT);
        assert_eq!(filtered[0], count - 1);
        assert_eq!(highlights[0], Some((0, "项目".len())));
    }

    #[test]
    fn most_used_mode_keeps_recent_order_for_ties() {
        let mut items = items();
        items[0].use_count = 5;
        items[0].last_used_at = Some("2020-01-01T00:00:00Z".to_string());
        items[1].use_count = 5;
        items[1].last_used_at = Some("2026-01-01T00:00:00Z".to_string());
        let (filtered, _, _) =
            filter_items_mode(&items, None, "", &[], crate::search::SortMode::MostUsed);
        assert_eq!(filtered, vec![1, 0, 2]);
    }

    #[test]
    fn most_used_mode_keeps_pinned_items_first() {
        let mut items = items();
        items[0].pinned = true;
        items[0].use_count = 1;
        items[1].use_count = 9;
        items[2].use_count = 4;
        let (filtered, _, _) =
            filter_items_mode(&items, None, "", &[], crate::search::SortMode::MostUsed);
        assert_eq!(filtered, vec![0, 1, 2]);
    }

    #[test]
    fn query_matches_title_and_content() {
        assert_eq!(rank_ids(&items(), "项目"), vec![0]);
        assert_eq!(rank_ids(&items(), "JSON"), vec![2]);
        assert!(rank_ids(&items(), "does-not-exist").is_empty());
    }

    #[test]
    fn category_filter_limits_the_subset() {
        let items = items();
        let (filtered, _, _) = filter_items(&items, Some("c1"), "");
        assert_eq!(filtered, vec![0, 1]);
        let (filtered, _, _) = filter_items(&items, Some("c2"), "项目");
        assert!(filtered.is_empty());
    }

    #[test]
    fn tag_prefix_filters_by_tag_and_optional_text() {
        let mut items = items();
        items[0].tags = vec!["support".to_string(), "refund".to_string()];
        items[1].tags = vec!["refund".to_string()];
        assert_eq!(rank_ids(&items, "#support"), vec![0]);
        assert_eq!(rank_ids(&items, "# support"), vec![0]);
        assert_eq!(rank_ids(&items, "#SUPPORT"), vec![0]);
        assert_eq!(rank_ids(&items, "#support #refund"), vec![0]);
        assert_eq!(rank_ids(&items, "#refund"), vec![0, 1]);
        assert_eq!(rank_ids(&items, "#support 项目"), vec![0]);
        assert!(rank_ids(&items, "#").is_empty());
        items[0].content.push_str(" #include <stdio.h>");
        items[0].blob.push_str(" #include <stdio.h>");
        assert_eq!(rank_ids(&items, "#"), vec![0]);
        assert_eq!(rank_ids(&items, r"#refund \#include"), vec![0]);
        assert!(rank_ids(&items, r"#unknown \#include").is_empty());
        let (filtered, _, _) = filter_items(&items, Some("c1"), "#support");
        assert_eq!(filtered, vec![0]);
        let (filtered, _, _) = filter_items_mode(
            &items,
            None,
            "",
            &["refund".to_string()],
            crate::search::SortMode::Recent,
        );
        assert_eq!(filtered, vec![0, 1]);
        let (filtered, _, _) = filter_items_mode(
            &items,
            None,
            "",
            &["support".to_string(), "refund".to_string()],
            crate::search::SortMode::Recent,
        );
        assert_eq!(filtered, vec![0]);
        assert!(rank_ids(&items, "#missing").is_empty());
    }

    #[test]
    fn tag_chip_filter_is_case_insensitive() {
        let mut items = items();
        items[0].tags = vec!["VIP".to_string()];
        let (filtered, _, _) = filter_items_mode(
            &items,
            None,
            "",
            &["VIP".to_string()],
            crate::search::SortMode::Recent,
        );
        assert_eq!(filtered, vec![0]);
        let (filtered, _, _) = filter_items_mode(
            &items,
            None,
            "",
            &["vip".to_string()],
            crate::search::SortMode::Recent,
        );
        assert_eq!(filtered, vec![0]);
    }

    #[test]
    fn tag_counts_merge_case_insensitively() {
        let mut items = items();
        items[0].tags = vec!["VIP".to_string(), "support".to_string()];
        items[1].tags = vec!["vip".to_string()];
        assert_eq!(
            count_tags(&items),
            vec![("VIP".to_string(), 2), ("support".to_string(), 1)]
        );
    }

    #[test]
    fn highlights_follow_the_filtered_order() {
        let items = items();
        let (filtered, titles, contents) = filter_items(&items, None, "接口");
        assert_eq!(filtered, vec![2]);
        assert!(titles[0].is_some());
        assert!(contents[0].is_none());

        let (filtered, titles, contents) = filter_items(&items, None, "JSON");
        assert_eq!(filtered, vec![2]);
        assert!(titles[0].is_none());
        assert!(contents[0].is_some());
    }

    #[test]
    fn category_and_query_highlights_align() {
        let items = items();
        let (filtered, titles, contents) = filter_items(&items, Some("c2"), "JSON");
        assert_eq!(filtered, vec![2]);
        assert!(titles[0].is_none());
        assert!(contents[0].is_some());
    }

    #[test]
    fn empty_message_priority() {
        assert_eq!(
            super::empty_message_for(0, 0, "", false, false, false),
            Some("还没有短语")
        );
        assert_eq!(
            super::empty_message_for(0, 0, "", false, false, true),
            Some("加载失败，已进入只读")
        );
        assert_eq!(
            super::empty_message_for(3, 0, "abc", false, false, false),
            Some("没有匹配的短语")
        );
        assert_eq!(
            super::empty_message_for(3, 0, "", true, false, false),
            Some("该分类下还没有短语")
        );
        assert_eq!(
            super::empty_message_for(3, 0, "", false, true, false),
            Some("没有匹配的短语")
        );
        assert_eq!(
            super::empty_message_for(3, 0, "", false, false, false),
            None
        );
        assert_eq!(
            super::empty_message_for(3, 1, "abc", true, false, false),
            None
        );
    }

    #[test]
    fn load_failure_notice_maps_to_failed_state() {
        assert!(super::data_load_failed(Some("data_recovery_failed")));
        assert!(super::data_load_failed(Some("schema_newer")));
        assert!(super::data_load_failed(Some("schema_older")));
        assert!(super::data_load_failed(Some("data_dir_unavailable")));
        assert!(super::data_load_failed(Some("initial_save_failed")));
        assert!(!super::data_load_failed(Some("rules_recovery_failed")));
        assert!(!super::data_load_failed(None));
    }

    #[test]
    fn footer_priority_and_notice_mapping() {
        assert_eq!(
            super::footer_text_for(Some("未捕获到目标窗口"), None, "Enter 输入"),
            "仅复制（未捕获到目标窗口） · Enter 复制 · Esc 关闭"
        );
        assert_eq!(
            super::footer_text_for(Some("身份未知"), Some("schema_newer"), "Enter 输入"),
            "只读（数据由更新版本写入） · 仅复制（身份未知）"
        );
        assert!(
            super::footer_text_for(None, Some("schema_newer"), "Enter 输入")
                .contains("数据由更新版本写入")
        );
        assert!(
            super::footer_text_for(None, Some("unknown_token"), "Enter 输入")
                .contains("unknown_token")
        );
        assert_eq!(
            super::footer_text_for(None, None, "Enter 输入"),
            "↑↓ 选择 · Enter 输入 · Esc 关闭"
        );
    }

    #[test]
    fn footer_verb_tracks_paste_availability() {
        assert_eq!(super::enter_verb(false, None), "Enter 输入");
        assert_eq!(super::enter_verb(true, None), "Enter 粘贴（Ctrl+V）");
        assert_eq!(super::enter_verb(true, Some(true)), "Enter 粘贴（Ctrl+V）");
        assert_eq!(
            super::enter_verb(true, Some(false)),
            "Enter 复制（不可粘贴）"
        );
        assert_eq!(
            super::footer_text_for(None, None, "Enter 粘贴（Ctrl+V）"),
            "↑↓ 选择 · Enter 粘贴（Ctrl+V） · Esc 关闭"
        );
        assert_eq!(
            super::footer_text_for(Some("未捕获到目标窗口"), None, "Enter 粘贴（Ctrl+V）"),
            "仅复制（未捕获到目标窗口） · Enter 复制 · Esc 关闭"
        );
    }

    #[test]
    fn scaling_rounds_to_nearest_pixel() {
        assert_eq!(scaled(520, 96), 520);
        assert_eq!(scaled(520, 120), 650);
        assert_eq!(scaled(420, 200), 875);
        assert_eq!(scaled(34, 125), 44);
    }
}
