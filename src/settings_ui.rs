//! M1-5 settings window: hotkey, tab policy, theme, startup, data dir,
//! clipboard restore.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateFontW, CreateSolidBrush, DeleteObject, GetStockObject, SetBkMode, SetTextColor,
    CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DEFAULT_PITCH, HBRUSH, HFONT, HGDIOBJ,
    OUT_DEFAULT_PRECIS, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForSystem;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, SetFocus, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::hotkey;
use crate::logging;
use crate::platform;
use crate::storage;

const CLASS: PCWSTR = w!("QuickPhrase.SettingsWindow");
const TITLE: PCWSTR = w!("QuickPhrase 设置");

const ID_HOTKEY_TEXT: i32 = 300;
const ID_HOTKEY_RECORD: i32 = 301;
const ID_TAB_POLICY: i32 = 302;
const ID_THEME: i32 = 303;
const ID_STARTUP: i32 = 304;
const ID_DATA_DIR: i32 = 305;
const ID_RESTORE_CLIP: i32 = 306;
const ID_SAVE: i32 = 307;
const ID_CANCEL: i32 = 308;
const ID_STATUS: i32 = 309;
const ID_LBL_HOTKEY: i32 = 310;
const ID_LBL_TAB: i32 = 311;
const ID_LBL_THEME: i32 = 312;
const ID_LBL_DATA: i32 = 313;
const ID_INSERTION: i32 = 314;
const ID_LBL_INSERTION: i32 = 315;

const WIDTH: i32 = 580;
const HEIGHT: i32 = 470;
const MIN_WIDTH: i32 = 500;
const MIN_HEIGHT: i32 = 420;
const MARGIN: i32 = 16;
const ROW: i32 = 30;
const GAP: i32 = 10;
const SS_LEFT: u32 = 0;

struct SettingsUi {
    hwnd: HWND,
    hotkey_text: HWND,
    lbl_hotkey: HWND,
    lbl_tab: HWND,
    lbl_theme: HWND,
    lbl_data: HWND,
    hotkey_record: HWND,
    insertion: HWND,
    lbl_insertion: HWND,
    tab_policy: HWND,
    theme: HWND,
    startup: HWND,
    data_dir: HWND,
    restore_clip: HWND,
    save: HWND,
    cancel: HWND,
    status: HWND,
    font: HFONT,
    brush: HBRUSH,
    dpi: u32,
    capturing: bool,
    hotkey_candidate: String,
    original_hotkey: String,
    loading: bool,
}

thread_local! {
    static UI: RefCell<Option<SettingsUi>> = const { RefCell::new(None) };
}

static QUIT_ON_CLOSE: AtomicBool = AtomicBool::new(false);

fn scaled(value: i32, dpi: u32) -> i32 {
    (value * dpi as i32 + 48) / 96
}

fn rgb(r: u32, g: u32, b: u32) -> COLORREF {
    COLORREF(r | (g << 8) | (b << 16))
}

fn make_font(dpi: u32) -> HFONT {
    let face = platform::wide("Microsoft YaHei UI");
    unsafe {
        CreateFontW(
            -scaled(14, dpi),
            0,
            0,
            0,
            400,
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

fn set_text(control: HWND, text: &str) {
    let wide = platform::wide(text);
    unsafe {
        let _ = SetWindowTextW(control, PCWSTR(wide.as_ptr()));
    }
}

fn status(ui: &SettingsUi, text: &str) {
    set_text(ui.status, text);
}

fn add_combo_items(combo: HWND, items: &[&str]) {
    unsafe {
        let _ = SendMessageW(combo, CB_RESETCONTENT, Some(WPARAM(0)), Some(LPARAM(0)));
    }
    for item in items {
        let wide = platform::wide(item);
        unsafe {
            let _ = SendMessageW(
                combo,
                CB_ADDSTRING,
                Some(WPARAM(0)),
                Some(LPARAM(wide.as_ptr() as isize)),
            );
        }
    }
}

fn combo_index(combo: HWND) -> usize {
    let index = unsafe { SendMessageW(combo, CB_GETCURSEL, Some(WPARAM(0)), Some(LPARAM(0))) }.0;
    if index < 0 {
        0
    } else {
        index as usize
    }
}

fn set_combo(combo: HWND, index: usize) {
    unsafe {
        let _ = SendMessageW(combo, CB_SETCURSEL, Some(WPARAM(index)), Some(LPARAM(0)));
    }
}

fn layout(ui: &SettingsUi, width: i32, height: i32) {
    if width <= 0 || height <= 0 {
        return;
    }
    let margin = scaled(MARGIN, ui.dpi);
    let row = scaled(ROW, ui.dpi);
    let gap = scaled(GAP, ui.dpi);
    let label_width = scaled(110, ui.dpi);
    let field_x = margin + label_width;
    let field_width = (width - field_x - margin).max(1);
    let mut y = margin;

    unsafe {
        let _ = MoveWindow(ui.lbl_hotkey, margin, y, label_width - gap, row, true);
        let _ = MoveWindow(
            ui.hotkey_text,
            field_x,
            y,
            (field_width - scaled(90, ui.dpi)).max(1),
            row,
            true,
        );
        let _ = MoveWindow(
            ui.hotkey_record,
            width - margin - scaled(84, ui.dpi),
            y,
            scaled(84, ui.dpi),
            row,
            true,
        );
        y += row + gap;
        let _ = MoveWindow(ui.lbl_insertion, margin, y, label_width - gap, row, true);
        let _ = MoveWindow(
            ui.insertion,
            field_x,
            y,
            scaled(320, ui.dpi),
            scaled(180, ui.dpi),
            true,
        );
        y += row + gap;
        let _ = MoveWindow(ui.lbl_tab, margin, y, label_width - gap, row, true);
        let _ = MoveWindow(
            ui.tab_policy,
            field_x,
            y,
            scaled(260, ui.dpi),
            scaled(180, ui.dpi),
            true,
        );
        y += row + gap;
        let _ = MoveWindow(ui.lbl_theme, margin, y, label_width - gap, row, true);
        let _ = MoveWindow(
            ui.theme,
            field_x,
            y,
            scaled(200, ui.dpi),
            scaled(180, ui.dpi),
            true,
        );
        y += row + gap;
        let _ = MoveWindow(ui.startup, field_x, y, field_width, row, true);
        y += row + gap;
        let _ = MoveWindow(ui.lbl_data, margin, y, label_width - gap, row, true);
        let _ = MoveWindow(ui.data_dir, field_x, y, field_width, row, true);
        y += row + gap;
        let _ = MoveWindow(ui.restore_clip, field_x, y, field_width, row, true);
        let _ = MoveWindow(
            ui.status,
            margin,
            height - margin - row * 2 - gap,
            width - margin * 2,
            row,
            true,
        );
        let _ = MoveWindow(
            ui.save,
            width - margin - scaled(180, ui.dpi),
            height - margin - row,
            scaled(85, ui.dpi),
            row,
            true,
        );
        let _ = MoveWindow(
            ui.cancel,
            width - margin - scaled(85, ui.dpi),
            height - margin - row,
            scaled(85, ui.dpi),
            row,
            true,
        );
    }
}

fn create_control(
    class: PCWSTR,
    text: &str,
    style: WINDOW_STYLE,
    parent: HWND,
    id: i32,
    instance: HINSTANCE,
) -> HWND {
    let text = platform::wide(text);
    unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            class,
            PCWSTR(text.as_ptr()),
            style,
            0,
            0,
            10,
            10,
            Some(parent),
            Some(HMENU(id as isize as *mut core::ffi::c_void)),
            Some(instance),
            None,
        )
        .unwrap_or_default()
    }
}

fn create() -> Option<SettingsUi> {
    let module = unsafe { GetModuleHandleW(None) }.ok()?;
    let instance = HINSTANCE(module.0);
    let class_brush =
        unsafe { HBRUSH(GetStockObject(windows::Win32::Graphics::Gdi::WHITE_BRUSH).0) };
    let class = WNDCLASSW {
        style: Default::default(),
        lpfnWndProc: Some(window_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: instance,
        hIcon: Default::default(),
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW) }.unwrap_or_default(),
        hbrBackground: class_brush,
        lpszMenuName: PCWSTR::null(),
        lpszClassName: CLASS,
    };
    if unsafe { RegisterClassW(&class) } == 0 {
        let error = unsafe { windows::Win32::Foundation::GetLastError() };
        if error != windows::Win32::Foundation::ERROR_CLASS_ALREADY_EXISTS {
            logging::error(&format!("settings: RegisterClassW failed: {error:?}"));
            return None;
        }
    }

    let dpi = unsafe { GetDpiForSystem() };
    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            CLASS,
            TITLE,
            WS_OVERLAPPEDWINDOW,
            120,
            120,
            scaled(WIDTH, dpi),
            scaled(HEIGHT, dpi),
            None,
            None,
            Some(instance),
            None,
        )
    }
    .ok()?;

    let font = make_font(dpi);
    let control_brush = unsafe { CreateSolidBrush(rgb(246, 247, 251)) };
    let hotkey_text = create_control(
        w!("STATIC"),
        "",
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(SS_LEFT),
        hwnd,
        ID_HOTKEY_TEXT,
        instance,
    );
    let lbl_hotkey = create_control(
        w!("STATIC"),
        "全局快捷键",
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(SS_LEFT),
        hwnd,
        ID_LBL_HOTKEY,
        instance,
    );
    let lbl_tab = create_control(
        w!("STATIC"),
        "Tab 键",
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(SS_LEFT),
        hwnd,
        ID_LBL_TAB,
        instance,
    );
    let lbl_theme = create_control(
        w!("STATIC"),
        "主题",
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(SS_LEFT),
        hwnd,
        ID_LBL_THEME,
        instance,
    );
    let lbl_data = create_control(
        w!("STATIC"),
        "数据目录",
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(SS_LEFT),
        hwnd,
        ID_LBL_DATA,
        instance,
    );
    let lbl_insertion = create_control(
        w!("STATIC"),
        "输入方式",
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(SS_LEFT),
        hwnd,
        ID_LBL_INSERTION,
        instance,
    );
    let insertion = create_control(
        w!("COMBOBOX"),
        "",
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(CBS_DROPDOWNLIST as u32),
        hwnd,
        ID_INSERTION,
        instance,
    );
    let hotkey_record = create_control(
        w!("BUTTON"),
        "录制",
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(BS_PUSHBUTTON as u32),
        hwnd,
        ID_HOTKEY_RECORD,
        instance,
    );
    let tab_policy = create_control(
        w!("COMBOBOX"),
        "",
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(CBS_DROPDOWNLIST as u32),
        hwnd,
        ID_TAB_POLICY,
        instance,
    );
    let theme = create_control(
        w!("COMBOBOX"),
        "",
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(CBS_DROPDOWNLIST as u32),
        hwnd,
        ID_THEME,
        instance,
    );
    let startup = create_control(
        w!("BUTTON"),
        "开机启动",
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(BS_AUTOCHECKBOX as u32),
        hwnd,
        ID_STARTUP,
        instance,
    );
    let data_dir = create_control(
        w!("STATIC"),
        "",
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(SS_LEFT),
        hwnd,
        ID_DATA_DIR,
        instance,
    );
    let restore_clip = create_control(
        w!("BUTTON"),
        "恢复上次复制的剪贴板内容",
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(BS_PUSHBUTTON as u32),
        hwnd,
        ID_RESTORE_CLIP,
        instance,
    );
    let save = create_control(
        w!("BUTTON"),
        "保存",
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(BS_DEFPUSHBUTTON as u32),
        hwnd,
        ID_SAVE,
        instance,
    );
    let cancel = create_control(
        w!("BUTTON"),
        "取消",
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(BS_PUSHBUTTON as u32),
        hwnd,
        ID_CANCEL,
        instance,
    );
    let status = create_control(
        w!("STATIC"),
        "就绪",
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(SS_LEFT),
        hwnd,
        ID_STATUS,
        instance,
    );

    let mut ui = SettingsUi {
        hwnd,
        hotkey_text,
        lbl_hotkey,
        lbl_tab,
        lbl_theme,
        lbl_data,
        hotkey_record,
        insertion,
        lbl_insertion,
        tab_policy,
        theme,
        startup,
        data_dir,
        restore_clip,
        save,
        cancel,
        status,
        font,
        brush: control_brush,
        dpi,
        capturing: false,
        hotkey_candidate: String::new(),
        original_hotkey: String::new(),
        loading: true,
    };

    for control in [
        hotkey_text,
        hotkey_record,
        insertion,
        tab_policy,
        theme,
        startup,
        data_dir,
        restore_clip,
        save,
        cancel,
        status,
    ] {
        unsafe {
            let _ = SendMessageW(
                control,
                WM_SETFONT,
                Some(WPARAM(ui.font.0 as usize)),
                Some(LPARAM(1)),
            );
            let style = GetWindowLongW(control, GWL_STYLE);
            let _ = SetWindowLongW(control, GWL_STYLE, style | WS_TABSTOP.0 as i32);
        }
    }

    for label in [lbl_hotkey, lbl_insertion, lbl_tab, lbl_theme, lbl_data] {
        unsafe {
            let _ = SendMessageW(
                label,
                WM_SETFONT,
                Some(WPARAM(ui.font.0 as usize)),
                Some(LPARAM(1)),
            );
        }
    }

    add_combo_items(theme, &["跟随系统", "浅色", "深色"]);

    let mut rect = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut rect);
    }
    layout(&ui, rect.right, rect.bottom);
    load(&mut ui);
    ui.loading = false;
    Some(ui)
}

fn fill_tab_items(ui: &SettingsUi, paste_mode: bool) {
    if paste_mode {
        add_combo_items(
            ui.tab_policy,
            &["Tab 转 4 空格", "保留制表符（仅粘贴模式）"],
        );
    } else {
        add_combo_items(ui.tab_policy, &["Tab 转 4 空格（Unicode 模式固定）"]);
    }
}

fn load(ui: &mut SettingsUi) {
    let settings = storage::settings_clone().unwrap_or_default();
    ui.capturing = false;
    ui.original_hotkey = settings.hotkey.clone();
    ui.hotkey_candidate = settings.hotkey.clone();
    let paste_mode = settings.insertion_mode == "paste";
    ui.loading = true;
    set_text(ui.hotkey_text, &settings.hotkey);
    add_combo_items(
        ui.insertion,
        &["Unicode 直接输入（默认）", "粘贴模式（Ctrl+V）"],
    );
    set_combo(ui.insertion, if paste_mode { 1 } else { 0 });
    fill_tab_items(ui, paste_mode);
    set_combo(
        ui.tab_policy,
        if paste_mode && settings.tab_policy == "tab" {
            1
        } else {
            0
        },
    );
    set_combo(
        ui.theme,
        match settings.theme.as_str() {
            "light" => 1,
            "dark" => 2,
            _ => 0,
        },
    );
    unsafe {
        let _ = SendMessageW(
            ui.startup,
            BM_SETCHECK,
            Some(WPARAM(if settings.startup { 1 } else { 0 })),
            Some(LPARAM(0)),
        );
    }
    set_text(ui.data_dir, &platform::data_dir().display().to_string());
    ui.loading = false;
}

pub fn show() {
    let needs_create = UI.with(|slot| match slot.try_borrow() {
        Ok(guard) => guard.is_none(),
        Err(_) => false,
    });
    if needs_create {
        if let Some(ui) = create() {
            UI.with(|slot| {
                if let Ok(mut guard) = slot.try_borrow_mut() {
                    *guard = Some(ui);
                }
            });
        }
    }
    let hwnd = UI.with(|slot| {
        slot.try_borrow()
            .ok()
            .and_then(|guard| guard.as_ref().map(|ui| ui.hwnd))
    });
    let Some(hwnd) = hwnd else {
        return;
    };
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
    }
    UI.with(|slot| {
        let mut guard = match slot.try_borrow_mut() {
            Ok(guard) => guard,
            Err(_) => {
                logging::warn("settings: window is busy (modal dialog?)");
                return;
            }
        };
        if let Some(ui) = guard.as_mut() {
            load(ui);
            status(ui, "就绪");
        }
    });
}

pub fn show_test() -> bool {
    QUIT_ON_CLOSE.store(true, Ordering::Relaxed);
    show();
    UI.with(|slot| {
        slot.try_borrow()
            .ok()
            .and_then(|guard| guard.as_ref().map(|ui| !ui.hwnd.0.is_null()))
            .unwrap_or(false)
    })
}

pub fn shutdown() {
    let ui = UI.with(|slot| match slot.try_borrow_mut() {
        Ok(mut guard) => guard.take(),
        Err(_) => {
            logging::warn("settings: shutdown skipped while a dialog is open");
            None
        }
    });
    let Some(ui) = ui else {
        return;
    };
    unsafe {
        let _ = DestroyWindow(ui.hwnd);
        let _ = DeleteObject(HGDIOBJ(ui.font.0));
        let _ = DeleteObject(HGDIOBJ(ui.brush.0));
    }
}

/// Let Tab/Enter traverse the settings controls like a dialog.
pub fn is_dialog_message(message: &MSG) -> bool {
    let hwnd = UI.with(|slot| {
        slot.try_borrow()
            .ok()
            .and_then(|guard| guard.as_ref().map(|ui| ui.hwnd))
    });
    match hwnd {
        Some(hwnd) if !hwnd.0.is_null() => unsafe { IsDialogMessageW(hwnd, message).as_bool() },
        _ => false,
    }
}

fn capture_key(ui: &mut SettingsUi, vk: u32) {
    if vk == 0x1B {
        ui.capturing = false;
        status(ui, "已取消录制");
        return;
    }
    if matches!(vk, 0x10 | 0x11 | 0x12 | 0x5B | 0x5C | 0x5D) {
        return;
    }
    let mut modifiers = 0u32;
    unsafe {
        if GetKeyState(VK_CONTROL.0 as i32) < 0 {
            modifiers |= windows::Win32::UI::Input::KeyboardAndMouse::MOD_CONTROL.0;
        }
        if GetKeyState(VK_MENU.0 as i32) < 0 {
            modifiers |= windows::Win32::UI::Input::KeyboardAndMouse::MOD_ALT.0;
        }
        if GetKeyState(VK_SHIFT.0 as i32) < 0 {
            modifiers |= windows::Win32::UI::Input::KeyboardAndMouse::MOD_SHIFT.0;
        }
        if GetKeyState(VK_LWIN.0 as i32) < 0 || GetKeyState(VK_RWIN.0 as i32) < 0 {
            modifiers |= windows::Win32::UI::Input::KeyboardAndMouse::MOD_WIN.0;
        }
    }
    let spec = hotkey::Spec { modifiers, vk };
    let text = spec.text();
    if hotkey::parse(&text).is_none() {
        status(
            ui,
            "无效组合：请同时按下 Ctrl/Alt/Shift/Win 与字母、数字或 F1–F24",
        );
        return;
    }
    let same_as_current = hotkey::parse(&ui.original_hotkey) == Some(spec);
    if !same_as_current && !hotkey::is_available(ui.hwnd, &text) {
        status(ui, "该快捷键已被其他程序占用");
        return;
    }
    ui.hotkey_candidate = text.clone();
    set_text(ui.hotkey_text, &text);
    ui.capturing = false;
    status(ui, "快捷键已录制，保存后生效");
}

fn save(ui: &mut SettingsUi) {
    let original = storage::settings_clone().unwrap_or_default();
    let mut settings = original.clone();
    let candidate = if ui.hotkey_candidate.is_empty() {
        ui.original_hotkey.clone()
    } else {
        ui.hotkey_candidate.clone()
    };
    // Tab stays as spaces unless the user opted into paste mode.
    let insertion = if combo_index(ui.insertion) == 1 {
        "paste"
    } else {
        "unicode"
    };
    let tab_policy = if insertion == "paste" && combo_index(ui.tab_policy) == 1 {
        "tab"
    } else {
        "spaces4"
    };
    let theme = match combo_index(ui.theme) {
        1 => "light",
        2 => "dark",
        _ => "system",
    };
    let startup =
        unsafe { SendMessageW(ui.startup, BM_GETCHECK, Some(WPARAM(0)), Some(LPARAM(0))).0 == 1 };

    settings.hotkey = candidate.clone();
    settings.insertion_mode = insertion.to_string();
    settings.tab_policy = tab_policy.to_string();
    settings.theme = theme.to_string();
    settings.startup = startup;

    if let Some(error) = storage::save_settings(settings.clone()).err() {
        status(ui, &format!("保存失败（{error}）"));
        return;
    }

    // Apply the hotkey on the main window; revert if the system refuses it.
    if let Err(code) = crate::app::apply_hotkey(&candidate) {
        let reapply = crate::app::apply_hotkey(&original.hotkey).is_ok();
        let rewrite = match storage::save_settings(original.clone()) {
            Ok(()) => true,
            Err(err) => {
                logging::error(&format!("settings: rollback write failed ({err})"));
                false
            }
        };
        ui.hotkey_candidate = original.hotkey.clone();
        set_text(ui.hotkey_text, &original.hotkey);
        if reapply && rewrite {
            status(ui, &format!("快捷键注册失败（{code}），已恢复原设置"));
        } else {
            status(ui, "回滚失败：请重启程序并检查热键设置");
        }
        return;
    }
    ui.original_hotkey = candidate;

    if let Err(code) = platform::set_startup(startup) {
        logging::warn(&format!("settings: startup change failed ({code})"));
        let mut revert = settings.clone();
        revert.startup = original.startup;
        let rollback = storage::save_settings(revert);
        if let Err(err) = &rollback {
            logging::error(&format!("settings: startup rollback write failed ({err})"));
        }
        unsafe {
            let _ = SendMessageW(
                ui.startup,
                BM_SETCHECK,
                Some(WPARAM(if original.startup { 1 } else { 0 })),
                Some(LPARAM(0)),
            );
        }
        if rollback.is_ok() {
            status(ui, "开机启动未生效，已恢复原设置");
        } else {
            status(ui, "开机启动未生效，且回滚写盘失败，请重启后检查");
        }
        return;
    }
    status(ui, "已保存");
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
            UI.with(|slot| {
                if let Ok(guard) = slot.try_borrow() {
                    if let Some(ui) = guard.as_ref() {
                        layout(ui, width, height);
                    }
                }
            });
            LRESULT(0)
        }
        WM_GETMINMAXINFO => {
            let info = &mut *(lparam.0 as *mut MINMAXINFO);
            UI.with(|slot| {
                let guard = match slot.try_borrow() {
                    Ok(guard) => guard,
                    Err(_) => return,
                };
                let dpi = guard
                    .as_ref()
                    .map(|ui| ui.dpi)
                    .unwrap_or_else(|| unsafe { GetDpiForSystem() });
                info.ptMinTrackSize.x = scaled(MIN_WIDTH, dpi);
                info.ptMinTrackSize.y = scaled(MIN_HEIGHT, dpi);
            });
            LRESULT(0)
        }
        WM_DPICHANGED => {
            let dpi = (wparam.0 & 0xffff) as u32;
            let suggested = &*(lparam.0 as *const RECT);
            let (left, top) = (suggested.left, suggested.top);
            let (width, height) = (
                suggested.right - suggested.left,
                suggested.bottom - suggested.top,
            );
            UI.with(|slot| {
                let mut guard = match slot.try_borrow_mut() {
                    Ok(guard) => guard,
                    Err(_) => return,
                };
                let Some(ui) = guard.as_mut() else {
                    return;
                };
                unsafe {
                    let _ = DeleteObject(HGDIOBJ(ui.font.0));
                }
                ui.font = make_font(dpi);
                ui.dpi = dpi;
                for control in [
                    ui.hotkey_text,
                    ui.lbl_hotkey,
                    ui.lbl_insertion,
                    ui.lbl_tab,
                    ui.lbl_theme,
                    ui.lbl_data,
                    ui.hotkey_record,
                    ui.insertion,
                    ui.tab_policy,
                    ui.theme,
                    ui.startup,
                    ui.data_dir,
                    ui.restore_clip,
                    ui.save,
                    ui.cancel,
                    ui.status,
                ] {
                    unsafe {
                        let _ = SendMessageW(
                            control,
                            WM_SETFONT,
                            Some(WPARAM(ui.font.0 as usize)),
                            Some(LPARAM(1)),
                        );
                    }
                }
                layout(ui, width, height);
            });
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
            }
            LRESULT(0)
        }
        WM_KEYDOWN | WM_SYSKEYDOWN
            if UI.with(|slot| {
                slot.try_borrow()
                    .ok()
                    .and_then(|guard| guard.as_ref().map(|ui| ui.capturing))
                    .unwrap_or(false)
            }) =>
        {
            UI.with(|slot| {
                if let Ok(mut guard) = slot.try_borrow_mut() {
                    if let Some(ui) = guard.as_mut() {
                        capture_key(ui, wparam.0 as u32);
                    }
                }
            });
            LRESULT(0)
        }
        WM_COMMAND => {
            let id = wparam.0 & 0xffff;
            let notification = ((wparam.0 >> 16) & 0xffff) as u32;
            let mut ask_paste_warning = false;
            UI.with(|slot| {
                let mut guard = match slot.try_borrow_mut() {
                    Ok(guard) => guard,
                    Err(_) => return,
                };
                let Some(ui) = guard.as_mut() else {
                    return;
                };
                match id as i32 {
                    ID_HOTKEY_RECORD if notification == BN_CLICKED => {
                        ui.capturing = true;
                        unsafe {
                            let _ = SetFocus(Some(hwnd));
                        }
                        status(ui, "请按下新的组合键（含 Ctrl/Alt/Shift/Win）");
                    }
                    ID_SAVE => save(ui),
                    ID_CANCEL => {
                        load(ui);
                        status(ui, "已取消");
                    }
                    ID_RESTORE_CLIP if notification == BN_CLICKED => {
                        match crate::clipboard::last_snapshot() {
                            Some(snapshot) if snapshot.text.is_some() => {
                                if crate::clipboard::restore(&snapshot) {
                                    status(ui, "已恢复上次剪贴板内容");
                                } else {
                                    status(ui, "无法恢复：剪贴板内容已变化（可能已恢复过，或被其他程序修改）");
                                }
                            }
                            _ => status(ui, "没有可恢复的剪贴板内容"),
                        }
                    }
                    ID_INSERTION if notification == CBN_SELCHANGE => {
                        if ui.loading {
                            return;
                        }
                        if combo_index(ui.insertion) == 1 {
                            // Ask outside this borrow: MessageBoxW pumps messages.
                            ask_paste_warning = true;
                        } else {
                            fill_tab_items(ui, false);
                            set_combo(ui.tab_policy, 0);
                            status(ui, "未保存：已选择 Unicode");
                        }
                    }
                    ID_TAB_POLICY | ID_THEME if notification == CBN_SELCHANGE => {
                        if !ui.loading {
                            status(ui, "未保存");
                        }
                    }
                    ID_STARTUP if notification == BN_CLICKED => {
                        if !ui.loading {
                            status(ui, "未保存");
                        }
                    }
                    _ => {}
                }
            });

            if ask_paste_warning {
                let text = platform::wide(
                    "粘贴模式会用 Ctrl+V 插入，并替换当前剪贴板。\n\
                     原纯文本内容可在托盘恢复；非文本内容无法恢复。\n\
                     终端、未知区域与多行文本仍只会复制，不会粘贴。\n\n\
                     启用粘贴模式吗？",
                );
                let caption = platform::wide("QuickPhrase 粘贴模式");
                let answer = unsafe {
                    MessageBoxW(
                        Some(hwnd),
                        PCWSTR(text.as_ptr()),
                        PCWSTR(caption.as_ptr()),
                        MB_YESNO | MB_ICONWARNING,
                    )
                };
                let persisted = storage::settings_clone().unwrap_or_default();
                let persisted_paste = persisted.insertion_mode == "paste";
                UI.with(|slot| {
                    let mut guard = match slot.try_borrow_mut() {
                        Ok(guard) => guard,
                        Err(_) => {
                            logging::warn(
                                "settings: window closed while the paste warning was open",
                            );
                            return;
                        }
                    };
                    let Some(ui) = guard.as_mut() else {
                        return;
                    };
                    if answer != IDYES {
                        set_combo(ui.insertion, if persisted_paste { 1 } else { 0 });
                        fill_tab_items(ui, persisted_paste);
                        set_combo(
                            ui.tab_policy,
                            if persisted_paste && persisted.tab_policy == "tab" {
                                1
                            } else {
                                0
                            },
                        );
                        status(
                            ui,
                            if persisted_paste {
                                "保持原设置：粘贴模式"
                            } else {
                                "未启用粘贴模式"
                            },
                        );
                    } else {
                        // Re-assert the accepted mode: a modal-time reopen may
                        // have reloaded the persisted (Unicode) value.
                        set_combo(ui.insertion, 1);
                        fill_tab_items(ui, true);
                        set_combo(ui.tab_policy, 0);
                        status(ui, "未保存：粘贴模式会替换剪贴板");
                    }
                });
            }
            LRESULT(0)
        }
        WM_CTLCOLORSTATIC => {
            let hdc = windows::Win32::Graphics::Gdi::HDC(wparam.0 as *mut core::ffi::c_void);
            unsafe {
                SetBkMode(hdc, TRANSPARENT);
                SetTextColor(hdc, rgb(0x55, 0x5B, 0x70));
            }
            let brush = UI.with(|slot| {
                slot.try_borrow()
                    .ok()
                    .and_then(|guard| guard.as_ref().map(|ui| ui.brush.0 as isize))
                    .unwrap_or(0)
            });
            if brush != 0 {
                return LRESULT(brush);
            }
            let stock = unsafe { GetStockObject(windows::Win32::Graphics::Gdi::WHITE_BRUSH) };
            LRESULT(stock.0 as isize)
        }
        WM_CLOSE => {
            if QUIT_ON_CLOSE.load(Ordering::Relaxed) {
                unsafe {
                    let _ = DestroyWindow(hwnd);
                }
            } else {
                unsafe {
                    let _ = ShowWindow(hwnd, SW_HIDE);
                }
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            UI.with(|slot| {
                let mut guard = match slot.try_borrow_mut() {
                    Ok(guard) => guard,
                    Err(_) => return,
                };
                if let Some(ui) = guard.take() {
                    unsafe {
                        let _ = DeleteObject(HGDIOBJ(ui.font.0));
                        let _ = DeleteObject(HGDIOBJ(ui.brush.0));
                    }
                }
            });
            if QUIT_ON_CLOSE.load(Ordering::Relaxed) {
                unsafe { PostQuitMessage(0) };
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}
