use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, AtomicU64, Ordering};
use std::time::Instant;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{
    GetLastError, COLORREF, ERROR_CLASS_ALREADY_EXISTS, HINSTANCE, HWND, LPARAM, LRESULT, POINT,
    RECT, SIZE, WPARAM,
};
use windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateFontW, CreateSolidBrush, DeleteObject, DrawTextW, EndPaint, FillRect, GetDC,
    GetMonitorInfoW, GetTextExtentPoint32W, InvalidateRect, MonitorFromPoint, ReleaseDC,
    SelectObject, SetBkColor, SetBkMode, SetTextColor, UpdateWindow, CLEARTYPE_QUALITY,
    CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DEFAULT_PITCH, DT_CENTER, DT_END_ELLIPSIS, DT_LEFT,
    DT_NOPREFIX, DT_RIGHT, DT_SINGLELINE, DT_VCENTER, HBRUSH, HDC, HFONT, HGDIOBJ, MONITORINFO,
    MONITOR_DEFAULTTONEAREST, OUT_DEFAULT_PRECIS, PAINTSTRUCT, TRANSPARENT,
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
use windows::Win32::UI::WindowsAndMessaging::{
    SetLayeredWindowAttributes, SystemParametersInfoW, GWL_EXSTYLE, LWA_ALPHA,
    SPI_GETCLIENTAREAANIMATION, WS_EX_LAYERED,
};

use crate::capture::Report;
use crate::{inject, logging, platform, ui};

const PANEL_CLASS: PCWSTR = w!("QuickPhrase.Panel");
const EDIT_ID: i32 = 100;
const LIST_ID: i32 = 101;
const FOOTER_ID: i32 = 102;
const SETTINGS_LINK_ID: i32 = 103;
const MANAGE_LINK_ID: i32 = 104;
const FADE_TIMER: usize = 7;

const PANEL_WIDTH: i32 = 520;
const PANEL_HEIGHT: i32 = 420;
const SEARCH_TOP: i32 = 14;
const SEARCH_HEIGHT: i32 = 34;
const CATEGORY_TOP: i32 = 54;
const CATEGORY_HEIGHT: i32 = 46;
const LIST_TOP: i32 = 104;
const FOOTER_HEIGHT: i32 = 42;
const SS_CENTER: i32 = 0x0001;
const SS_NOTIFY: i32 = 0x0100;
const SS_CENTERIMAGE: i32 = 0x0200;
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

#[derive(Clone)]
pub struct Item {
    pub id: String,
    pub title: String,
    pub content: String,
    pub category: String,
    pub category_id: Option<String>,
    pub blob: String,
    pub pinned: bool,
    pub last_used_at: Option<String>,
    pub updated_at: String,
    pub sort_order: i32,
}

pub struct PanelData {
    pub items: Vec<Item>,
    pub categories: Vec<(String, String)>,
    pub notice: Option<String>,
    pub copy_only: Option<String>,
    pub paste_mode: bool,
    pub keep_tabs: bool,
    pub hotkey_hint: String,
}

struct Fonts {
    title: HFONT,
    body: HFONT,
    small: HFONT,
    icon: HFONT,
}

struct Brushes {
    background: HBRUSH,
    selected: HBRUSH,
    search: HBRUSH,
    border: HBRUSH,
    highlight: HBRUSH,
    accent: HBRUSH,
}

struct Panel {
    hwnd: HWND,
    edit: HWND,
    list: HWND,
    footer: HWND,
    settings_link: HWND,
    manage_link: HWND,
    filtered: Vec<usize>,
    items: Vec<Item>,
    categories: Vec<(String, String)>,
    active_category: Option<String>,
    copy_only: Option<String>,
    notice: Option<String>,
    paste_mode: bool,
    keep_tabs: bool,
    hotkey_hint: String,
    palette: crate::theme::Palette,
    title_highlights: Vec<Option<(usize, usize)>>,
    content_highlights: Vec<Option<(usize, usize)>>,
    last_query: String,
    report: Option<Report>,
    notify: HWND,
    fonts: Fonts,
    brushes: Brushes,
    dpi: u32,
    hiding: bool,
    quit_on_hide: bool,
    monitor: Option<crate::foreground::Monitor>,
    opening: bool,
    deferred_inactive: bool,
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
    // Create the timer first: if it cannot be created, skip the fade entirely
    // instead of making the window transparent forever.
    let timer = unsafe { SetTimer(Some(hwnd), FADE_TIMER, 10, None) };
    if timer == 0 {
        logging::warn("panel fade timer unavailable; showing without fade");
        return;
    }
    // The window is already visible and focused; the fade runs on a timer so
    // keystrokes reach the panel from the first frame.
    unsafe {
        let ex = GetWindowLongW(hwnd, GWL_EXSTYLE);
        let _ = SetWindowLongW(hwnd, GWL_EXSTYLE, ex | WS_EX_LAYERED.0 as i32);
        let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 0, LWA_ALPHA);
    }
    FADE_STEP.store(0, Ordering::Relaxed);
}

fn create_static_link(parent: HWND, instance: HINSTANCE, id: i32, text: &str) -> Option<HWND> {
    let text = platform::wide(text);
    unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            w!("STATIC"),
            PCWSTR(text.as_ptr()),
            WS_CHILD | WS_VISIBLE | WINDOW_STYLE((SS_CENTER | SS_CENTERIMAGE | SS_NOTIFY) as u32),
            0,
            0,
            10,
            10,
            Some(parent),
            Some(HMENU(id as isize as *mut core::ffi::c_void)),
            Some(instance),
            None,
        )
        .ok()
    }
}

fn create_font_face(size: i32, weight: i32, face: &str, dpi: u32) -> HFONT {
    let face = platform::wide(face);
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

fn create_font(size: i32, weight: i32, dpi: u32) -> HFONT {
    create_font_face(size, weight, "Microsoft YaHei UI", dpi)
}

unsafe fn send(hwnd: HWND, message: u32, wparam: usize, lparam: isize) -> LRESULT {
    SendMessageW(hwnd, message, Some(WPARAM(wparam)), Some(LPARAM(lparam)))
}

fn make_fonts(dpi: u32) -> Fonts {
    Fonts {
        title: create_font(14, 700, dpi),
        body: create_font(12, 400, dpi),
        small: create_font(11, 400, dpi),
        icon: create_font_face(13, 400, "Segoe MDL2 Assets", dpi),
    }
}

fn destroy_fonts(fonts: &Fonts) {
    unsafe {
        let _ = DeleteObject(HGDIOBJ(fonts.title.0));
        let _ = DeleteObject(HGDIOBJ(fonts.body.0));
        let _ = DeleteObject(HGDIOBJ(fonts.small.0));
        let _ = DeleteObject(HGDIOBJ(fonts.icon.0));
    }
}

fn make_brushes(palette: crate::theme::Palette) -> Brushes {
    use crate::theme::Palette;
    let Palette {
        page: _,
        card,
        selection,
        field,
        border,
        highlight,
        primary,
        ..
    } = palette;
    Brushes {
        background: unsafe { CreateSolidBrush(card) },
        selected: unsafe { CreateSolidBrush(selection) },
        search: unsafe { CreateSolidBrush(field) },
        border: unsafe { CreateSolidBrush(border) },
        highlight: unsafe { CreateSolidBrush(highlight) },
        accent: unsafe { CreateSolidBrush(primary) },
    }
}

fn destroy_brushes(brushes: &Brushes) {
    unsafe {
        let _ = DeleteObject(HGDIOBJ(brushes.background.0));
        let _ = DeleteObject(HGDIOBJ(brushes.selected.0));
        let _ = DeleteObject(HGDIOBJ(brushes.search.0));
        let _ = DeleteObject(HGDIOBJ(brushes.border.0));
        let _ = DeleteObject(HGDIOBJ(brushes.highlight.0));
        let _ = DeleteObject(HGDIOBJ(brushes.accent.0));
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

/// Bring an already open panel back to the front without re-capturing.
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

/// Capture failed: open the panel anyway, but Enter copies instead of typing.
pub fn show_copy_only(notify: HWND, data: PanelData) -> u128 {
    present(None, None, notify, data, true, false)
}

pub fn show_test() -> u128 {
    present(
        None,
        None,
        HWND::default(),
        PanelData {
            items: Vec::new(),
            categories: Vec::new(),
            notice: None,
            copy_only: None,
            paste_mode: false,
            keep_tabs: false,
            hotkey_hint: String::new(),
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
            let palette = crate::theme::current();
            if palette != panel.palette {
                destroy_brushes(&panel.brushes);
                panel.brushes = make_brushes(palette);
                panel.palette = palette;
                // Applying a control theme can synchronously touch child
                // windows, so do it after this RefCell borrow is released.
                retheme = Some((panel.edit, panel.list, palette.dark));
            }
        }
        panel.report = report;
        panel.monitor = monitor;
        panel.notify = notify;
        panel.items = data.items;
        panel.categories = data.categories;
        panel.notice = data.notice;
        panel.copy_only = data.copy_only;
        panel.paste_mode = data.paste_mode;
        panel.keep_tabs = data.keep_tabs;
        panel.hotkey_hint = data.hotkey_hint;
        panel.active_category = None;
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
    if let Some((edit, list, dark)) = retheme {
        ui::set_dark_mode(dark);
        ui::theme_control(edit, dark);
        ui::theme_control(list, dark);
    }

    place();
    apply_rounding(hwnd);
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
            // Take foreground and focus *before* any animation: during a fade
            // the user must never type into the previous window.
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
    let palette = crate::theme::current();
    let brushes = make_brushes(palette);

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
            WS_CHILD | WS_VISIBLE | WINDOW_STYLE(SS_CENTERIMAGE as u32),
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
    // Dark scroll bars and combo chrome do not follow WM_CTLCOLOR*.
    ui::set_dark_mode(palette.dark);
    ui::theme_control(list, palette.dark);
    ui::theme_control(edit, palette.dark);

    let settings_link = match create_static_link(hwnd, instance, SETTINGS_LINK_ID, "设置") {
        Some(link) => link,
        None => {
            logging::error("CreateWindowExW(settings link) failed");
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            destroy_fonts(&fonts);
            destroy_brushes(&brushes);
            CREATE_FAILED.store(true, Ordering::Relaxed);
            return None;
        }
    };
    let manage_link = match create_static_link(hwnd, instance, MANAGE_LINK_ID, "管理短语") {
        Some(link) => link,
        None => {
            logging::error("CreateWindowExW(manage link) failed");
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            destroy_fonts(&fonts);
            destroy_brushes(&brushes);
            CREATE_FAILED.store(true, Ordering::Relaxed);
            return None;
        }
    };
    unsafe {
        let _ = send(settings_link, WM_SETFONT, fonts.small.0 as usize, 1);
        let _ = send(manage_link, WM_SETFONT, fonts.small.0 as usize, 1);
    }

    let cue = platform::wide("搜索短语…");
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
        settings_link,
        manage_link,
        filtered: Vec::new(),
        items: Vec::new(),
        categories: Vec::new(),
        active_category: None,
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
        fonts,
        brushes,
        dpi,
        hiding: false,
        quit_on_hide: false,
        monitor: None,
        opening: false,
        deferred_inactive: false,
    };

    let mut rect = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut rect);
    }
    layout_controls(
        panel.edit,
        panel.list,
        panel.footer,
        panel.settings_link,
        panel.manage_link,
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
        let item = panel.items.get(item_index)?;
        Some((
            item_index,
            item.id.clone(),
            item.category_id.clone().unwrap_or_default(),
            item.content.clone(),
        ))
    });

    let Some((item_index, item_id, category_id, content)) = info else {
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
        let (copied, snapshot) = crate::clipboard::set_text_with_snapshot(&content);
        crate::storage::record_use(&item_id, "rejected");
        logging::info(&format!(
            "panel copy-only commit: index={item_index} category={category_id} copied={copied} non_text={} read_ok={}",
            snapshot.had_non_text,
            snapshot.read_ok
        ));
        if !notify.0.is_null() {
            let message = if !copied {
                format!("复制失败（{reason}）")
            } else if !snapshot.read_ok {
                format!("已复制（{reason}；原剪贴板内容无法确认，可能不可恢复）")
            } else if snapshot.had_non_text {
                format!("已复制（{reason}；原剪贴板含非文本内容，仅文本可恢复）")
            } else if snapshot.text.is_some() {
                format!("已复制（{reason}；原剪贴板内容已被替换）")
            } else {
                format!("已复制（{reason}）")
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

    // Keep the UI thread responsive: guards can wait up to ~650 ms on a hung
    // target. The worker reports back through WM_INJECT_DONE with its session.
    let session = SESSION.load(Ordering::Relaxed);
    INJECTING.store(true, Ordering::Relaxed);
    let notify_raw = notify.0 as isize;
    // Read the insertion mode once on the UI thread; the worker never touches
    // the store beyond the standard use-count update.
    let options = inject::Options::from_settings();
    let spawned = std::thread::Builder::new()
        .name("quickphrase-inject".into())
        .spawn(move || {
            let outcome = inject::run(&report, &content, monitor_ok, options);
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

    let (edit, list, footer, settings_link, manage_link, body, small) = PANEL.with(|slot| {
        let guard = slot.borrow();
        let Some(panel) = guard.as_ref() else {
            return (
                HWND::default(),
                HWND::default(),
                HWND::default(),
                HWND::default(),
                HWND::default(),
                HFONT::default(),
                HFONT::default(),
            );
        };
        (
            panel.edit,
            panel.list,
            panel.footer,
            panel.settings_link,
            panel.manage_link,
            panel.fonts.body,
            panel.fonts.small,
        )
    });

    unsafe {
        for control in [edit, list, footer] {
            let _ = send(control, WM_SETFONT, body.0 as usize, 1);
        }
        for control in [settings_link, manage_link] {
            let _ = send(control, WM_SETFONT, small.0 as usize, 1);
        }
        let _ = send(list, LB_SETITEMHEIGHT, 0, scaled(ITEM_HEIGHT, dpi) as isize);
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

#[allow(clippy::too_many_arguments)]
fn layout_controls(
    edit: HWND,
    list: HWND,
    footer: HWND,
    settings_link: HWND,
    manage_link: HWND,
    dpi: u32,
    width: i32,
    height: i32,
) {
    if width <= 0 || height <= 0 {
        return;
    }
    let margin = scaled(16, dpi);
    let edit_height = scaled(SEARCH_HEIGHT, dpi);
    let footer_height = scaled(FOOTER_HEIGHT, dpi);
    let icon_width = scaled(26, dpi);
    let hint_width = scaled(84, dpi);
    let link_width = scaled(52, dpi);

    unsafe {
        let edit_left = margin + icon_width + scaled(6, dpi);
        let edit_width =
            (width - edit_left - margin - hint_width - link_width - scaled(8, dpi)).max(1);
        let _ = MoveWindow(
            edit,
            edit_left,
            scaled(SEARCH_TOP, dpi),
            edit_width,
            edit_height,
            true,
        );
        let _ = MoveWindow(
            settings_link,
            width - margin - link_width,
            scaled(SEARCH_TOP, dpi) + scaled(1, dpi),
            link_width,
            scaled(32, dpi),
            true,
        );
        let list_top = scaled(LIST_TOP, dpi);
        let list_height = (height - list_top - footer_height - scaled(12, dpi)).max(0);
        let _ = MoveWindow(
            list,
            scaled(10, dpi),
            list_top,
            width - scaled(20, dpi),
            list_height,
            true,
        );
        let footer_width = (width - margin * 2 - scaled(94, dpi)).max(1);
        let _ = MoveWindow(
            footer,
            margin,
            height - footer_height - scaled(8, dpi),
            footer_width,
            footer_height,
            true,
        );
        let _ = MoveWindow(
            manage_link,
            margin + footer_width,
            height - footer_height - scaled(8, dpi),
            scaled(94, dpi),
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
    refresh_footer();
}

fn refresh_footer() {
    let (footer, label) = PANEL.with(|slot| {
        let guard = slot.borrow();
        match guard.as_ref() {
            Some(panel) => (panel.footer, footer_text(panel)),
            None => (HWND::default(), String::new()),
        }
    });
    if footer.0.is_null() {
        return;
    }
    let wide = platform::wide(&label);
    unsafe {
        let _ = SetWindowTextW(footer, PCWSTR(wide.as_ptr()));
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

fn category_chips(
    hdc: HDC,
    panel: &Panel,
    right_limit: i32,
    dpi: u32,
) -> Vec<(Option<String>, String, RECT)> {
    let mut chips = Vec::new();
    let top = scaled(CATEGORY_TOP, dpi) + scaled((CATEGORY_HEIGHT - 32) / 2, dpi);
    let height = scaled(32, dpi);
    let mut x = scaled(16, dpi);
    let mut entries: Vec<(Option<String>, String)> = vec![(None, "全部".to_string())];
    entries.extend(
        panel
            .categories
            .iter()
            .map(|(id, name)| (Some(id.clone()), name.clone())),
    );

    unsafe {
        for (id, name) in entries {
            let text = platform::wide(&name);
            let mut size = SIZE::default();
            if !GetTextExtentPoint32W(hdc, &text, &mut size).as_bool() {
                continue;
            }
            let width = size.cx + scaled(22, dpi);
            if x + width > right_limit {
                break;
            }
            let chip = RECT {
                left: x,
                top,
                right: x + width,
                bottom: top + height,
            };
            chips.push((id, name, chip));
            x += width + scaled(8, dpi);
        }
    }
    chips
}

fn draw_categories(hdc: HDC, panel: &Panel, right_limit: i32, dpi: u32) {
    let font = panel.fonts.body;
    unsafe {
        let previous = SelectObject(hdc, HGDIOBJ(font.0));
        SetBkMode(hdc, TRANSPARENT);
        for (id, name, mut rect) in category_chips(hdc, panel, right_limit, dpi) {
            let selected = match (&id, &panel.active_category) {
                (None, None) => true,
                (Some(left), Some(right)) => left == right,
                _ => false,
            };
            let mut label = platform::wide(&name);
            let brush = if selected {
                panel.brushes.selected
            } else {
                panel.brushes.search
            };
            let radius = (rect.bottom - rect.top) / 2;
            ui::fill_round(hdc, &rect, radius, brush);
            let mut outline = rect;
            outline.left += 1;
            outline.top += 1;
            outline.right -= 1;
            outline.bottom -= 1;
            ui::stroke_round(
                hdc,
                &outline,
                radius - 1,
                if selected {
                    panel.palette.primary
                } else {
                    panel.palette.border
                },
                1,
            );
            SetTextColor(
                hdc,
                if selected {
                    panel.palette.primary
                } else {
                    panel.palette.secondary
                },
            );
            DrawTextW(
                hdc,
                &mut label,
                &mut rect,
                DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
            );
        }
        let _ = SelectObject(hdc, previous);
    }
}

fn draw_empty_message(hdc: HDC, panel: &Panel, rect: &RECT, dpi: u32) {
    let Some(message) = empty_message(panel) else {
        return;
    };
    let mut message_rect = RECT {
        left: rect.left,
        top: scaled(LIST_TOP, dpi),
        right: rect.right,
        bottom: rect.bottom - scaled(FOOTER_HEIGHT + 8, dpi),
    };
    let mut text = platform::wide(message);
    unsafe {
        let previous = SelectObject(hdc, HGDIOBJ(panel.fonts.body.0));
        SetBkMode(hdc, TRANSPARENT);
        SetTextColor(hdc, panel.palette.secondary);
        DrawTextW(
            hdc,
            &mut text,
            &mut message_rect,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
        );
        let _ = SelectObject(hdc, previous);
    }
}

fn draw_match(
    hdc: HDC,
    panel: &Panel,
    text: &str,
    rect: &mut RECT,
    highlight: Option<(usize, usize)>,
    normal: COLORREF,
) {
    let Some((start, end)) = highlight.filter(|(start, end)| start < end && *end <= text.len())
    else {
        let mut wide = platform::wide(text);
        unsafe {
            SetTextColor(hdc, normal);
            DrawTextW(
                hdc,
                &mut wide,
                rect,
                DT_SINGLELINE | DT_LEFT | DT_NOPREFIX | DT_END_ELLIPSIS,
            );
        }
        return;
    };

    let prefix = &text[..start];
    let matched = &text[start..end];
    let suffix = &text[end..];
    let mut prefix_wide = platform::wide(prefix);
    let mut matched_wide = platform::wide(matched);
    let mut suffix_wide = platform::wide(suffix);
    let mut prefix_size = SIZE::default();
    let mut matched_size = SIZE::default();
    unsafe {
        if !GetTextExtentPoint32W(hdc, &prefix_wide, &mut prefix_size).as_bool()
            || !GetTextExtentPoint32W(hdc, &matched_wide, &mut matched_size).as_bool()
        {
            return;
        }
    }

    let mut prefix_rect = RECT {
        left: rect.left,
        top: rect.top,
        right: (rect.left + prefix_size.cx).min(rect.right),
        bottom: rect.bottom,
    };
    let mut match_rect = RECT {
        left: rect.left + prefix_size.cx,
        top: rect.top,
        right: (rect.left + prefix_size.cx + matched_size.cx + 2).min(rect.right),
        bottom: rect.bottom,
    };
    let mut suffix_rect = RECT {
        left: match_rect.right,
        top: rect.top,
        right: rect.right,
        bottom: rect.bottom,
    };

    unsafe {
        SetTextColor(hdc, normal);
        DrawTextW(
            hdc,
            &mut prefix_wide,
            &mut prefix_rect,
            DT_SINGLELINE | DT_LEFT | DT_NOPREFIX | DT_END_ELLIPSIS,
        );
        if match_rect.right > match_rect.left {
            ui::fill_round(
                hdc,
                &match_rect,
                scaled(3, panel.dpi),
                panel.brushes.highlight,
            );
            SetTextColor(hdc, panel.palette.highlight_text);
            DrawTextW(
                hdc,
                &mut matched_wide,
                &mut match_rect,
                DT_SINGLELINE | DT_LEFT | DT_NOPREFIX,
            );
        }
        if suffix_rect.right > suffix_rect.left {
            SetTextColor(hdc, normal);
            DrawTextW(
                hdc,
                &mut suffix_wide,
                &mut suffix_rect,
                DT_SINGLELINE | DT_LEFT | DT_NOPREFIX | DT_END_ELLIPSIS,
            );
        }
    }
}

type Highlight = Option<(usize, usize)>;
type FilterResult = (Vec<usize>, Vec<Highlight>, Vec<Highlight>);

fn filter_items(items: &[Item], category: Option<&str>, query: &str) -> FilterResult {
    let subset: Vec<usize> = items
        .iter()
        .enumerate()
        .filter(|(_, item)| match category {
            Some(id) => item.category_id.as_deref() == Some(id),
            None => true,
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
                last_used_at: item.last_used_at.as_deref(),
                updated_at: &item.updated_at,
                sort_order: item.sort_order,
            }
        })
        .collect();

    let hits = crate::search::rank(&docs, query, crate::search::DEFAULT_LIMIT);
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

fn enter_verb(paste_mode: bool, paste_ok: Option<bool>) -> &'static str {
    if !paste_mode {
        return "Enter 输入";
    }
    match paste_ok {
        Some(false) => "Enter 复制（不可粘贴）",
        _ => "Enter 粘贴（Ctrl+V）",
    }
}

fn footer_text_for(copy_only: Option<&str>, notice: Option<&str>, verb: &str) -> String {
    match (copy_only, notice) {
        (Some(reason), Some(token)) => format!(
            // Keep this rare double-warning short enough for the footer.
            "只读（{}） · 仅复制（{reason}）",
            notice_label(token)
        ),
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
        &item.content,
        panel.keep_tabs,
    ))
}

fn footer_text(panel: &Panel) -> String {
    let verb = enter_verb(panel.paste_mode, selected_paste_ok(panel));
    footer_text_for(panel.copy_only.as_deref(), panel.notice.as_deref(), verb)
}

fn empty_message_for(
    items: usize,
    filtered: usize,
    query: &str,
    has_category: bool,
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
    if !query.is_empty() {
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
        let (filtered, title_highlights, content_highlights) =
            filter_items(&panel.items, panel.active_category.as_deref(), query);
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
        // The list is a child window and would paint over the empty-state text.
        let _ = ShowWindow(list, if titles.is_empty() { SW_HIDE } else { SW_SHOW });
        let _ = InvalidateRect(Some(hwnd), None, false);
    }
    refresh_footer();
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
    if (message == WM_KEYUP || message == WM_SYSKEYUP) && wparam.0 as u16 == VK_RETURN.0 {
        if ime_composing(hwnd) {
            ENTER_ARMED.store(false, Ordering::Relaxed);
            return call_default_edit(hwnd, message, wparam, lparam);
        }
        // Only commit when the matching key-down was observed and was not
        // consumed by IME composition.
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
            // Commit on key-up so Enter is physically released before the
            // panel hides and focus returns to the target.
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

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_TIMER if wparam.0 == FADE_TIMER => {
            let step = FADE_STEP.fetch_add(1, Ordering::Relaxed) + 1;
            unsafe {
                let alpha = ((step.min(10) * 255) / 10) as u8;
                let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), alpha, LWA_ALPHA);
            }
            if step >= 10 {
                unsafe {
                    let _ = KillTimer(Some(hwnd), FADE_TIMER);
                }
            }
            LRESULT(0)
        }
        WM_SHOWWINDOW if wparam.0 == 0 => {
            // Hiding during the fade must not leave the next show transparent.
            unsafe {
                let _ = KillTimer(Some(hwnd), FADE_TIMER);
                let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA);
            }
            LRESULT(0)
        }
        WM_SIZE => {
            let width = (lparam.0 & 0xffff) as i32;
            let height = ((lparam.0 >> 16) & 0xffff) as i32;
            let parts = PANEL.with(|slot| {
                slot.borrow().as_ref().map(|panel| {
                    (
                        panel.edit,
                        panel.list,
                        panel.footer,
                        panel.settings_link,
                        panel.manage_link,
                        panel.dpi,
                    )
                })
            });
            if let Some((edit, list, footer, settings_link, manage_link, dpi)) = parts {
                layout_controls(
                    edit,
                    list,
                    footer,
                    settings_link,
                    manage_link,
                    dpi,
                    width,
                    height,
                );
            }
            LRESULT(0)
        }
        WM_COMMAND => {
            let control = (wparam.0 & 0xffff) as i32;
            let code = ((wparam.0 >> 16) & 0xffff) as u32;
            if (control == SETTINGS_LINK_ID || control == MANAGE_LINK_ID) && code == 0 {
                let notify = PANEL.with(|slot| {
                    slot.borrow()
                        .as_ref()
                        .map(|panel| panel.notify)
                        .unwrap_or_default()
                });
                let id = if control == SETTINGS_LINK_ID {
                    1004 // ID_SETTINGS
                } else {
                    1002 // ID_MANAGE
                };
                if !notify.0.is_null() {
                    unsafe {
                        let _ = PostMessageW(Some(notify), WM_COMMAND, WPARAM(id), LPARAM(0));
                    }
                }
                hide_and_restore();
                logging::info(if control == SETTINGS_LINK_ID {
                    "panel settings link"
                } else {
                    "panel manage link"
                });
            } else if control == EDIT_ID && code == EN_CHANGE {
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
                refresh_footer();
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
            let (field, heading) = PANEL.with(|slot| {
                slot.borrow()
                    .as_ref()
                    .map(|panel| (panel.palette.field, panel.palette.heading))
                    .unwrap_or((COLORREF(0xFFFFFF), COLORREF(0)))
            });
            unsafe {
                SetBkMode(hdc, TRANSPARENT);
                SetBkColor(hdc, field);
                SetTextColor(hdc, heading);
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
            let control = HWND(lparam.0 as *mut core::ffi::c_void);
            let id = unsafe { GetDlgCtrlID(control) };
            let color = PANEL.with(|slot| {
                slot.borrow()
                    .as_ref()
                    .map(|panel| {
                        if id == SETTINGS_LINK_ID || id == MANAGE_LINK_ID {
                            panel.palette.primary
                        } else {
                            panel.palette.secondary
                        }
                    })
                    .unwrap_or(COLORREF(0))
            });
            unsafe {
                SetBkMode(hdc, TRANSPARENT);
                SetTextColor(hdc, color);
            }
            let brush = PANEL.with(|slot| {
                slot.borrow()
                    .as_ref()
                    .map(|panel| {
                        if id == SETTINGS_LINK_ID {
                            // The settings link sits inside the search band.
                            panel.brushes.search.0 as isize
                        } else {
                            panel.brushes.background.0 as isize
                        }
                    })
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
                PANEL.with(|slot| {
                    let guard = slot.borrow();
                    let Some(panel) = guard.as_ref() else {
                        return;
                    };
                    let margin = scaled(16, panel.dpi);
                    FillRect(hdc, &rect, panel.brushes.background);

                    let search_rect = RECT {
                        left: margin - 1,
                        top: scaled(SEARCH_TOP, panel.dpi) - 1,
                        right: rect.right - margin + 1,
                        bottom: scaled(SEARCH_TOP + SEARCH_HEIGHT, panel.dpi) + 1,
                    };
                    // Rounded search surface.
                    ui::fill_round(
                        hdc,
                        &search_rect,
                        scaled(10, panel.dpi),
                        panel.brushes.search,
                    );
                    let mut search_border = search_rect;
                    search_border.left += 1;
                    search_border.top += 1;
                    search_border.right -= 1;
                    search_border.bottom -= 1;
                    ui::stroke_round(
                        hdc,
                        &search_border,
                        scaled(9, panel.dpi),
                        panel.palette.border,
                        1,
                    );

                    // Search glyph at the left of the field.
                    let previous = SelectObject(hdc, HGDIOBJ(panel.fonts.icon.0));
                    SetTextColor(hdc, panel.palette.secondary);
                    SetBkMode(hdc, TRANSPARENT);
                    let mut search_glyph = platform::wide("\u{E721}");
                    let mut glyph_rect = RECT {
                        left: margin - 1,
                        top: search_rect.top,
                        right: margin + scaled(26, panel.dpi),
                        bottom: search_rect.bottom,
                    };
                    DrawTextW(
                        hdc,
                        &mut search_glyph,
                        &mut glyph_rect,
                        DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
                    );
                    let _ = SelectObject(hdc, previous);

                    // Shortcut hint sits between the field and the settings link.
                    if !panel.hotkey_hint.is_empty() {
                        let previous = SelectObject(hdc, HGDIOBJ(panel.fonts.small.0));
                        SetTextColor(hdc, panel.palette.secondary);
                        let mut hint = platform::wide(&panel.hotkey_hint);
                        let mut hint_rect = RECT {
                            left: rect.right
                                - margin
                                - scaled(52, panel.dpi)
                                - scaled(84, panel.dpi),
                            top: search_rect.top,
                            right: rect.right
                                - margin
                                - scaled(52, panel.dpi)
                                - scaled(2, panel.dpi),
                            bottom: search_rect.bottom,
                        };
                        DrawTextW(
                            hdc,
                            &mut hint,
                            &mut hint_rect,
                            DT_RIGHT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS,
                        );
                        let _ = SelectObject(hdc, previous);
                    }

                    draw_categories(hdc, panel, rect.right - margin, panel.dpi);
                    draw_empty_message(hdc, panel, &rect, panel.dpi);
                });
                let _ = EndPaint(hwnd, &paint);
            }
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let x = (lparam.0 & 0xffff) as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xffff) as i16 as i32;
            // Clicking the search band (except the settings link) focuses the
            // input, so the visible band is not a dead zone.
            let focused_search = PANEL.with(|slot| {
                let guard = slot.borrow();
                let Some(panel) = guard.as_ref() else {
                    return false;
                };
                let mut client = RECT::default();
                unsafe {
                    let _ = GetClientRect(hwnd, &mut client);
                }
                let top = scaled(SEARCH_TOP, panel.dpi);
                let bottom = top + scaled(SEARCH_HEIGHT, panel.dpi);
                let link_left = client.right - scaled(16, panel.dpi) - scaled(52, panel.dpi);
                if y >= top && y <= bottom && x >= scaled(16, panel.dpi) && x < link_left {
                    unsafe {
                        let _ = SetFocus(Some(panel.edit));
                    }
                    return true;
                }
                false
            });
            if focused_search {
                return LRESULT(0);
            }
            let chosen = PANEL.with(|slot| {
                let guard = slot.borrow();
                let panel = guard.as_ref()?;
                let hdc = unsafe { GetDC(Some(hwnd)) };
                let previous = unsafe { SelectObject(hdc, HGDIOBJ(panel.fonts.body.0)) };
                let chips = category_chips(
                    hdc,
                    panel,
                    {
                        let mut rect = RECT::default();
                        unsafe {
                            let _ = GetClientRect(hwnd, &mut rect);
                        }
                        rect.right - scaled(16, panel.dpi)
                    },
                    panel.dpi,
                );
                unsafe {
                    let _ = SelectObject(hdc, previous);
                    let _ = ReleaseDC(Some(hwnd), hdc);
                }
                chips
                    .into_iter()
                    .find(|(_, _, rect)| {
                        x >= rect.left && x <= rect.right && y >= rect.top && y <= rect.bottom
                    })
                    .map(|(id, _, _)| id)
            });

            if let Some(id) = chosen {
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
                let edit = PANEL.with(|slot| {
                    slot.borrow()
                        .as_ref()
                        .map(|panel| panel.edit)
                        .unwrap_or_default()
                });
                if changed && !edit.0.is_null() {
                    let query = read_text(edit);
                    populate(&query);
                }
                // A click on the parent window moves keyboard focus away from
                // the search box; the panel is keyboard-first.
                if !edit.0.is_null() {
                    unsafe {
                        let _ = SetFocus(Some(edit));
                    }
                }
            }
            LRESULT(0)
        }
        WM_ACTIVATE => {
            let active = (wparam.0 & 0xffff) as u32;
            if active == WA_INACTIVE {
                // Attaching to the previous foreground thread and detaching can
                // bounce activation right after the panel appears. Record the
                // loss instead of dropping it, then decide once opening ends.
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
    let Some(item) = panel
        .filtered
        .get(draw.itemID as usize)
        .and_then(|index| panel.items.get(*index))
    else {
        return;
    };
    let hdc = draw.hDC;
    let rect = draw.rcItem;

    unsafe {
        let selected = draw.itemState.0 & ODS_SELECTED.0 != 0;
        // Clear the whole row: the pill is inset, so the corners must still be
        // the current card colour after a theme switch.
        FillRect(hdc, &rect, panel.brushes.background);
        if selected {
            let mut pill = rect;
            pill.left += scaled(4, panel.dpi);
            pill.top += scaled(4, panel.dpi);
            pill.right -= scaled(4, panel.dpi);
            pill.bottom -= scaled(4, panel.dpi);
            ui::fill_round(hdc, &pill, scaled(10, panel.dpi), panel.brushes.selected);
            if GetFocus() == panel.list || GetFocus() == panel.edit {
                // Keyboard focus must be visible and not colour-only (§14.2).
                ui::stroke_round(
                    hdc,
                    &pill,
                    scaled(10, panel.dpi),
                    panel.palette.primary,
                    scaled(1, panel.dpi),
                );
            }
        }
        SetBkMode(hdc, TRANSPARENT);
    }

    let pad = scaled(12, panel.dpi);

    // 36 px type icon: a tinted square with a letter, per PRD §14.3.
    let icon_size = scaled(36, panel.dpi);
    let icon_rect = RECT {
        left: rect.left + pad,
        top: rect.top + ((rect.bottom - rect.top) - icon_size) / 2,
        right: rect.left + pad + icon_size,
        bottom: rect.top + ((rect.bottom - rect.top) - icon_size) / 2 + icon_size,
    };
    unsafe {
        ui::fill_round(
            hdc,
            &icon_rect,
            scaled(8, panel.dpi),
            panel.brushes.selected,
        );
        let previous = SelectObject(hdc, HGDIOBJ(panel.fonts.title.0));
        SetTextColor(hdc, panel.palette.primary);
        let mut glyph = platform::wide("T");
        let mut glyph_rect = icon_rect;
        DrawTextW(
            hdc,
            &mut glyph,
            &mut glyph_rect,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
        );
        let _ = SelectObject(hdc, previous);
    }
    let text_left = icon_rect.right + scaled(10, panel.dpi);

    let title_highlight = panel
        .title_highlights
        .get(draw.itemID as usize)
        .copied()
        .flatten();
    let mut title_rect = RECT {
        left: text_left,
        top: rect.top + scaled(8, panel.dpi),
        right: rect.right - scaled(90, panel.dpi),
        bottom: rect.top + scaled(28, panel.dpi),
    };
    unsafe {
        let previous = SelectObject(hdc, HGDIOBJ(panel.fonts.title.0));
        draw_match(
            hdc,
            panel,
            &item.title,
            &mut title_rect,
            title_highlight,
            panel.palette.heading,
        );
        let _ = SelectObject(hdc, previous);
    }

    let content_highlight = panel
        .content_highlights
        .get(draw.itemID as usize)
        .copied()
        .flatten();
    let mut preview_rect = RECT {
        left: text_left,
        top: rect.top + scaled(30, panel.dpi),
        right: rect.right - scaled(12, panel.dpi),
        bottom: rect.bottom - scaled(6, panel.dpi),
    };
    unsafe {
        let previous = SelectObject(hdc, HGDIOBJ(panel.fonts.body.0));
        draw_match(
            hdc,
            panel,
            &item.content,
            &mut preview_rect,
            content_highlight,
            panel.palette.secondary,
        );
        let _ = SelectObject(hdc, previous);
    }

    let mut tag = platform::wide(&item.category);
    let mut tag_rect = RECT {
        left: rect.right - scaled(84, panel.dpi),
        top: rect.top + scaled(10, panel.dpi),
        right: rect.right - scaled(12, panel.dpi),
        bottom: rect.top + scaled(28, panel.dpi),
    };
    unsafe {
        let previous = SelectObject(hdc, HGDIOBJ(panel.fonts.small.0));
        SetTextColor(hdc, panel.palette.primary);
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
    // A representative list: the panel draws icons, titles and previews, so an
    // empty list would hide most of the paint cost.
    let base_items: Vec<Item> = (0..50)
        .map(|index| Item {
            id: format!("bench-{index}"),
            title: format!("项目更新说明 {index}"),
            content: "这是第 {index} 条正文，包含接口、测试与发布计划。".to_string(),
            category: "工作沟通".to_string(),
            category_id: None,
            blob: format!(
                "项目更新说明 {index}\n这是第 {index} 条正文，包含接口、测试与发布计划。"
            ),
            pinned: index % 7 == 0,
            last_used_at: None,
            updated_at: "2026-01-01T00:00:00Z".to_string(),
            sort_order: index,
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
                notice: None,
                copy_only: None,
                paste_mode: false,
                keep_tabs: false,
                hotkey_hint: "Ctrl+Alt+Q".to_string(),
            },
            false,
            false,
        );
        samples.push(elapsed);
        hide_and_restore();
    }
    samples.sort_unstable();
    let p50 = samples[samples.len() / 2];
    // Nearest-rank p95: ceil(0.95 * n) -> zero-based index.
    let p95_rank = (samples.len() * 95).div_ceil(100);
    let p95 = samples[p95_rank.saturating_sub(1).min(samples.len() - 1)];
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
    use super::{filter_items, scaled, Item};

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
                blob: format!("{}\n{}", title.to_lowercase(), content.to_lowercase()),
                pinned: false,
                last_used_at: None,
                updated_at: "2026-01-01T00:00:00Z".to_string(),
                sort_order,
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
            super::empty_message_for(0, 0, "", false, false),
            Some("还没有短语")
        );
        assert_eq!(
            super::empty_message_for(0, 0, "", false, true),
            Some("加载失败，已进入只读")
        );
        assert_eq!(
            super::empty_message_for(3, 0, "abc", false, false),
            Some("没有匹配的短语")
        );
        assert_eq!(
            super::empty_message_for(3, 0, "", true, false),
            Some("该分类下还没有短语")
        );
        assert_eq!(super::empty_message_for(3, 0, "", false, false), None);
        assert_eq!(super::empty_message_for(3, 1, "abc", true, false), None);
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
        // Copy-only still wins over the configured insertion mode.
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
