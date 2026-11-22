//! QuickPhrase 2.0 two-pane Fluent settings center (Direct2D + DirectWrite).

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use windows::core::{w, PCWSTR, PWSTR};
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DeleteObject, EndPaint, InvalidateRect, SetBkColor, SetBkMode,
    SetTextColor, HBRUSH, HDC, HFONT, HGDIOBJ, OPAQUE, PAINTSTRUCT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::Dialogs::{
    GetOpenFileNameW, GetSaveFileNameW, OFN_FILEMUSTEXIST, OFN_OVERWRITEPROMPT, OFN_PATHMUSTEXIST,
    OPENFILENAMEW,
};
use windows::Win32::UI::Controls::{DRAWITEMSTRUCT, WM_MOUSELEAVE};
use windows::Win32::UI::HiDpi::{GetDpiForSystem, GetDpiForWindow};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetFocus, GetKeyState, SetFocus, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT, VK_CONTROL,
    VK_DOWN, VK_ESCAPE, VK_LEFT, VK_LWIN, VK_MENU, VK_RETURN, VK_RIGHT, VK_RWIN, VK_SHIFT,
    VK_SPACE, VK_TAB, VK_UP,
};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::d2d::{self, AlignH, AlignV, Icon, TextStyle};
use crate::{hotkey, logging, platform, storage, theme, ui};

const CLASS: PCWSTR = w!("QuickPhrase.SettingsWindow");
const TITLE: PCWSTR = w!("QuickPhrase 设置");

const ID_HOTKEY_RECORD: i32 = 301;
const ID_RESTORE_CLIP: i32 = 306;
const ID_SAVE: i32 = 307;
const ID_CANCEL: i32 = 308;
const ID_COPY_DIR: i32 = 320;
const ID_OPEN_DIR: i32 = 321;
const ID_EXPORT_DATA: i32 = 322;
const ID_IMPORT_DATA: i32 = 323;
const ID_RESTORE_BACKUP: i32 = 324;
const ID_SORT_MODE: i32 = 325;
const ID_APPS_ADD: i32 = 326;
const ID_APPS_REMOVE: i32 = 327;
const ID_APPS_EXE: i32 = 328;
const ID_APPS_MODE: i32 = 329;
const ID_APPS_RULES: i32 = 330;

const WIDTH: i32 = 740;
// The Input page needs 430 logical px of body plus the 52 px footer; keep
// enough client height at the minimum size so it never slides under the footer.
const HEIGHT: i32 = 560;
const MIN_WIDTH: i32 = 500;
const MIN_HEIGHT: i32 = 540;
const NAV_WIDTH: i32 = 172;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    General,
    Input,
    Appearance,
    Data,
    Apps,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Hover {
    None,
    Nav(usize),
    Startup,
    ModeUnicode,
    ModePaste,
    TabSpaces,
    TabKeep,
    ThemeCard(usize),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FocusItem {
    Custom(Hover),
    HotkeyRecord,
    RestoreClip,
    CopyDir,
    OpenDir,
    ExportData,
    ImportData,
    RestoreBackup,
    SortMode,
    AppsExe,
    AppsMode,
    AppsRules,
    AppsAdd,
    AppsRemove,
    Save,
    Cancel,
}

#[derive(Default, Clone, Copy)]
struct Rects {
    nav_pane: RECT,
    content_pane: RECT,
    footer: RECT,
    status: RECT,
    nav_items: [RECT; 5],
    // General
    hotkey_card: RECT,
    hotkey_kbd_box: RECT,
    startup_card: RECT,
    startup_switch: RECT,
    // Input
    mode_unicode: RECT,
    mode_paste: RECT,
    paste_warn: RECT,
    tab_card: RECT,
    tab_spaces: RECT,
    tab_keep: RECT,
    clip_card: RECT,
    // Appearance
    theme_cards: [RECT; 3],
    sort_card: RECT,
    // Data
    data_card: RECT,
    data_path_box: RECT,
    stat_boxes: [RECT; 3],
    // Apps
    apps_card: RECT,
    apps_exe_box: RECT,
}

struct SettingsUi {
    hwnd: HWND,
    hotkey_record: HWND,
    restore_clip: HWND,
    copy_dir: HWND,
    open_dir: HWND,
    export_data: HWND,
    import_data: HWND,
    restore_backup: HWND,
    sort_mode: HWND,
    apps_exe: HWND,
    apps_mode: HWND,
    apps_rules: HWND,
    apps_add: HWND,
    apps_remove: HWND,
    field_brush: HBRUSH,
    save: HWND,
    cancel: HWND,
    font: HFONT,
    palette: theme::Palette,
    dpi: u32,
    tab: Tab,
    hover: Hover,
    focus: Hover,
    active: bool,
    tracking_mouse: bool,
    rects: Rects,
    capturing: bool,
    hotkey_candidate: String,
    original_hotkey: String,
    paste_mode: bool,
    keep_tabs: bool,
    theme_pref: String,
    panel_sort: String,
    startup: bool,
    status_text: String,
    data_dir: String,
}

thread_local! {
    static UI: RefCell<Option<SettingsUi>> = const { RefCell::new(None) };
}

static QUIT_ON_CLOSE: AtomicBool = AtomicBool::new(false);

fn scaled(value: i32, dpi: u32) -> i32 {
    (value * dpi as i32 + 48) / 96
}

fn make_font(dpi: u32) -> HFONT {
    ui::make_font(ui::FontRole::BodyInput, dpi)
}

fn set_status(ui: &mut SettingsUi, text: &str) {
    ui.status_text = text.to_string();
    unsafe {
        let _ = InvalidateRect(Some(ui.hwnd), None, false);
    }
}

fn set_button_text(hwnd: HWND, text: &str) {
    let w = platform::wide(text);
    unsafe {
        let _ = SetWindowTextW(hwnd, PCWSTR(w.as_ptr()));
    }
}

fn layout(ui: &mut SettingsUi, width: i32, height: i32) {
    if width <= 0 || height <= 0 {
        return;
    }
    let dpi = ui.dpi;
    let nav_w = scaled(NAV_WIDTH, dpi).min((width / 3).max(scaled(124, dpi)));
    let footer_h = scaled(52, dpi);

    let nav_pane = RECT {
        left: 0,
        top: 0,
        right: nav_w,
        bottom: height,
    };
    let content_pane = RECT {
        left: nav_w,
        top: 0,
        right: width,
        bottom: (height - footer_h).max(1),
    };
    let footer = RECT {
        left: nav_w,
        top: content_pane.bottom,
        right: width,
        bottom: height,
    };

    // Nav items (below top-left brand logo header)
    let npad = scaled(10, dpi);
    let item_h = scaled(38, dpi);
    let mut nav_items = [RECT::default(); 5];
    for (i, item) in nav_items.iter_mut().enumerate() {
        let top = scaled(74, dpi) + (item_h + scaled(4, dpi)) * i as i32;
        *item = RECT {
            left: npad,
            top,
            right: nav_w - npad,
            bottom: top + item_h,
        };
    }

    let cpad = scaled(22, dpi);
    let cl = content_pane.left + cpad;
    let cr = (content_pane.right - cpad).max(cl + 100);
    let cw = cr - cl;
    let body_top = scaled(68, dpi);

    // --- Tab 0: General ---
    let compact = cw < scaled(380, dpi);
    let hk_card_h = if ui.capturing {
        if compact {
            scaled(142, dpi)
        } else {
            scaled(112, dpi)
        }
    } else if compact {
        scaled(104, dpi)
    } else {
        scaled(74, dpi)
    };
    let hotkey_card = RECT {
        left: cl,
        top: body_top,
        right: cr,
        bottom: body_top + hk_card_h,
    };
    let rec_w = scaled(76, dpi);
    let rec_h = scaled(32, dpi);
    let (rec_x, rec_y, hotkey_kbd_box) = if compact {
        let ctrl_y = hotkey_card.top + scaled(58, dpi);
        let kbd_l = hotkey_card.left + scaled(16, dpi);
        let rx = hotkey_card.right - scaled(16, dpi) - rec_w;
        let kbd_r = (rx - scaled(8, dpi)).max(kbd_l + scaled(80, dpi));
        (
            rx,
            ctrl_y,
            RECT {
                left: kbd_l,
                top: ctrl_y,
                right: kbd_r,
                bottom: ctrl_y + rec_h,
            },
        )
    } else {
        let rx = hotkey_card.right - scaled(16, dpi) - rec_w;
        let ry = hotkey_card.top + scaled(21, dpi);
        let kbd_w = scaled(128, dpi).min((cw / 3).max(scaled(86, dpi)));
        (
            rx,
            ry,
            RECT {
                left: rx - scaled(8, dpi) - kbd_w,
                top: ry,
                right: rx - scaled(8, dpi),
                bottom: ry + rec_h,
            },
        )
    };
    let startup_top = hotkey_card.bottom + scaled(12, dpi);
    let startup_h = scaled(74, dpi);
    let startup_card = RECT {
        left: cl,
        top: startup_top,
        right: cr,
        bottom: startup_top + startup_h,
    };
    let sw_w = scaled(40, dpi);
    let sw_h = scaled(22, dpi);
    let startup_switch = RECT {
        left: startup_card.right - scaled(18, dpi) - sw_w,
        top: startup_card.top + (startup_h - sw_h) / 2,
        right: startup_card.right - scaled(18, dpi),
        bottom: startup_card.top + (startup_h + sw_h) / 2,
    };

    // --- Tab 1: Input ---
    let radio_h = scaled(70, dpi);
    let mode_unicode = RECT {
        left: cl,
        top: body_top,
        right: cr,
        bottom: body_top + radio_h,
    };
    let mode_paste = RECT {
        left: cl,
        top: mode_unicode.bottom + scaled(8, dpi),
        right: cr,
        bottom: mode_unicode.bottom + scaled(8, dpi) + radio_h,
    };
    let paste_warn = if ui.paste_mode {
        RECT {
            left: cl,
            top: mode_paste.bottom + scaled(8, dpi),
            right: cr,
            bottom: mode_paste.bottom + scaled(8, dpi) + scaled(38, dpi),
        }
    } else {
        RECT {
            left: cl,
            top: mode_paste.bottom,
            right: cr,
            bottom: mode_paste.bottom,
        }
    };
    let tab_top = paste_warn.bottom + scaled(10, dpi);
    let tab_card = RECT {
        left: cl,
        top: tab_top,
        right: cr,
        bottom: tab_top + scaled(84, dpi),
    };
    let seg_pad = scaled(14, dpi);
    let seg_top = tab_card.top + scaled(40, dpi);
    let seg_h = scaled(32, dpi);
    let seg_mid = cl + cw / 2;
    let tab_spaces = RECT {
        left: cl + seg_pad,
        top: seg_top,
        right: seg_mid - scaled(3, dpi),
        bottom: seg_top + seg_h,
    };
    let tab_keep = RECT {
        left: seg_mid + scaled(3, dpi),
        top: seg_top,
        right: cr - seg_pad,
        bottom: seg_top + seg_h,
    };
    let clip_top = tab_card.bottom + scaled(10, dpi);
    let clip_card = RECT {
        left: cl,
        top: clip_top,
        right: cr,
        bottom: clip_top + scaled(64, dpi),
    };
    let clip_btn_w = scaled(166, dpi).min((cw / 2).max(scaled(110, dpi)));
    let clip_btn_h = scaled(32, dpi);

    // --- Tab 2: Appearance ---
    let card_gap = scaled(12, dpi);
    let tcard_w = ((cw - card_gap * 2) / 3).max(scaled(80, dpi));
    let tcard_h = scaled(136, dpi);
    let mut theme_cards = [RECT::default(); 3];
    for (i, tc) in theme_cards.iter_mut().enumerate() {
        let x = cl + (tcard_w + card_gap) * i as i32;
        *tc = RECT {
            left: x,
            top: body_top,
            right: x + tcard_w,
            bottom: body_top + tcard_h,
        };
    }
    let sort_top = theme_cards[0].bottom + scaled(12, dpi);
    let sort_card = RECT {
        left: cl,
        top: sort_top,
        right: cr,
        bottom: sort_top + scaled(74, dpi),
    };

    // --- Tab 3: Data ---
    let dpad = scaled(16, dpi);
    let btn_sm_w = scaled(76, dpi);
    let data_left = cl;
    let data_right = cr;
    let data_top = body_top;
    let path_top = data_top + scaled(42, dpi);
    let path_h = scaled(34, dpi);
    let data_path_box = RECT {
        left: data_left + dpad,
        top: path_top,
        right: data_right - dpad,
        bottom: path_top + path_h,
    };
    let copy_open_top = path_top + path_h + scaled(10, dpi);
    let action_top = copy_open_top + scaled(30, dpi) + scaled(8, dpi);
    let action_h = scaled(30, dpi);
    let action_gap = scaled(8, dpi);
    let action_w = scaled(80, dpi);
    let action_x = data_left + dpad;
    let mut data_actions = [RECT::default(); 3];
    for (index, action) in data_actions.iter_mut().enumerate() {
        let left = action_x + (action_w + action_gap) * index as i32;
        *action = RECT {
            left,
            top: action_top,
            right: left + action_w,
            bottom: action_top + action_h,
        };
    }
    let stat_top = action_top + action_h + scaled(12, dpi);
    let stat_h = scaled(72, dpi);
    let stat_gap = scaled(10, dpi);
    let stat_w = ((cw - dpad * 2 - stat_gap * 2) / 3).max(scaled(64, dpi));
    let mut stat_boxes = [RECT::default(); 3];
    for (i, sb) in stat_boxes.iter_mut().enumerate() {
        let x = data_left + dpad + (stat_w + stat_gap) * i as i32;
        *sb = RECT {
            left: x,
            top: stat_top,
            right: x + stat_w,
            bottom: stat_top + stat_h,
        };
    }
    let data_card = RECT {
        left: data_left,
        top: data_top,
        right: data_right,
        bottom: stat_top + stat_h + scaled(14, dpi),
    };

    // --- Tab 4: Apps ---
    let apps_card = RECT {
        left: cl,
        top: body_top,
        right: cr,
        bottom: body_top + scaled(380, dpi),
    };
    let apad = scaled(16, dpi);
    let app_row_top = apps_card.top + scaled(64, dpi);
    let app_row_h = scaled(32, dpi);
    let app_combo_w = scaled(150, dpi);
    let app_combo_h = ui::combo_height(ui.apps_mode, dpi);
    let app_add_w = scaled(80, dpi);
    let app_gap = scaled(8, dpi);
    let apps_exe_box = RECT {
        left: apps_card.left + apad,
        top: app_row_top,
        right: apps_card.right - apad,
        bottom: app_row_top + app_row_h,
    };
    let app_mode_top = app_row_top + app_row_h + app_gap;
    let apps_mode_box = RECT {
        left: apps_exe_box.left,
        top: app_mode_top,
        right: apps_exe_box.left + app_combo_w,
        bottom: app_mode_top + app_combo_h,
    };
    let apps_remove_w = scaled(92, dpi);
    let apps_remove_top = app_mode_top + app_combo_h + app_gap;
    let apps_rules_top = apps_remove_top + app_row_h + scaled(10, dpi);
    let apps_rules_box = RECT {
        left: apps_card.left + apad,
        top: apps_rules_top,
        right: apps_card.right - apad,
        bottom: apps_card.bottom - apad,
    };

    // Footer buttons
    let save_w = scaled(92, dpi);
    let cancel_w = scaled(72, dpi);
    let fbtn_h = scaled(32, dpi);
    let fbtn_y = footer.top + (footer_h - fbtn_h) / 2;
    let status = RECT {
        left: cl,
        top: footer.top,
        right: (cr - save_w - cancel_w - scaled(20, dpi)).max(cl + 20),
        bottom: footer.bottom,
    };

    unsafe {
        let _ = MoveWindow(ui.hotkey_record, rec_x, rec_y, rec_w, rec_h, true);
        let _ = MoveWindow(
            ui.restore_clip,
            clip_card.right - scaled(16, dpi) - clip_btn_w,
            clip_card.top + ((clip_card.bottom - clip_card.top) - clip_btn_h) / 2,
            clip_btn_w,
            clip_btn_h,
            true,
        );
        let sort_w = scaled(148, dpi);
        let sort_h = scaled(32, dpi);
        let _ = MoveWindow(
            ui.sort_mode,
            sort_card.right - scaled(16, dpi) - sort_w,
            sort_card.top + ((sort_card.bottom - sort_card.top) - sort_h) / 2,
            sort_w,
            sort_h,
            true,
        );
        let _ = MoveWindow(
            ui.apps_exe,
            apps_exe_box.left,
            apps_exe_box.top,
            apps_exe_box.right - apps_exe_box.left,
            apps_exe_box.bottom - apps_exe_box.top,
            true,
        );
        let _ = MoveWindow(
            ui.apps_mode,
            apps_mode_box.left,
            apps_mode_box.top,
            apps_mode_box.right - apps_mode_box.left,
            ui::combo_height(ui.apps_mode, dpi),
            true,
        );
        let _ = MoveWindow(
            ui.apps_add,
            apps_card.right - apad - app_add_w,
            apps_mode_box.top,
            app_add_w,
            app_row_h,
            true,
        );
        let _ = MoveWindow(
            ui.apps_remove,
            apps_exe_box.left,
            apps_remove_top,
            apps_remove_w,
            app_row_h,
            true,
        );
        let _ = MoveWindow(
            ui.apps_rules,
            apps_rules_box.left,
            apps_rules_box.top,
            apps_rules_box.right - apps_rules_box.left,
            apps_rules_box.bottom - apps_rules_box.top,
            true,
        );
        let _ = MoveWindow(
            ui.copy_dir,
            data_left + dpad,
            copy_open_top,
            btn_sm_w,
            path_h,
            true,
        );
        let _ = MoveWindow(
            ui.open_dir,
            data_left + dpad + btn_sm_w + scaled(6, dpi),
            copy_open_top,
            btn_sm_w,
            path_h,
            true,
        );
        for (control, rect) in [
            (ui.export_data, data_actions[0]),
            (ui.import_data, data_actions[1]),
            (ui.restore_backup, data_actions[2]),
        ] {
            let _ = MoveWindow(
                control,
                rect.left,
                rect.top,
                rect.right - rect.left,
                rect.bottom - rect.top,
                true,
            );
        }
        let _ = MoveWindow(ui.save, cr - save_w, fbtn_y, save_w, fbtn_h, true);
        let _ = MoveWindow(
            ui.cancel,
            cr - save_w - scaled(8, dpi) - cancel_w,
            fbtn_y,
            cancel_w,
            fbtn_h,
            true,
        );

        let _ = ShowWindow(
            ui.hotkey_record,
            if ui.tab == Tab::General {
                SW_SHOW
            } else {
                SW_HIDE
            },
        );
        let _ = ShowWindow(
            ui.restore_clip,
            if ui.tab == Tab::Input {
                SW_SHOW
            } else {
                SW_HIDE
            },
        );
        let _ = ShowWindow(
            ui.sort_mode,
            if ui.tab == Tab::Appearance {
                SW_SHOW
            } else {
                SW_HIDE
            },
        );
        for control in [
            ui.apps_exe,
            ui.apps_mode,
            ui.apps_rules,
            ui.apps_add,
            ui.apps_remove,
        ] {
            let _ = ShowWindow(
                control,
                if ui.tab == Tab::Apps {
                    SW_SHOW
                } else {
                    SW_HIDE
                },
            );
        }
        let _ = ShowWindow(
            ui.copy_dir,
            if ui.tab == Tab::Data {
                SW_SHOW
            } else {
                SW_HIDE
            },
        );
        let _ = ShowWindow(
            ui.open_dir,
            if ui.tab == Tab::Data {
                SW_SHOW
            } else {
                SW_HIDE
            },
        );
        for control in [ui.export_data, ui.import_data, ui.restore_backup] {
            let _ = ShowWindow(
                control,
                if ui.tab == Tab::Data {
                    SW_SHOW
                } else {
                    SW_HIDE
                },
            );
        }
    }

    ui.rects = Rects {
        nav_pane,
        content_pane,
        footer,
        status,
        nav_items,
        hotkey_card,
        hotkey_kbd_box,
        startup_card,
        startup_switch,
        mode_unicode,
        mode_paste,
        paste_warn,
        tab_card,
        tab_spaces,
        tab_keep,
        clip_card,
        theme_cards,
        sort_card,
        data_card,
        data_path_box,
        stat_boxes,
        apps_card,
        apps_exe_box,
    };
}

fn relayout(ui: &mut SettingsUi) {
    let mut rect = RECT::default();
    unsafe {
        let _ = GetClientRect(ui.hwnd, &mut rect);
    }
    layout(ui, rect.right, rect.bottom);
    unsafe {
        let _ = InvalidateRect(Some(ui.hwnd), None, false);
    }
}

fn paint_settings(ui: &SettingsUi, hdc: HDC, client: RECT) {
    let dpi = ui.dpi;
    let p = ui.palette;
    let r = ui.rects;

    d2d::paint_buffered(hdc, client, |c| {
        // 1. Background surfaces
        c.fill_rect(&r.nav_pane, p.sidebar);
        c.fill_rect(&r.content_pane, p.page);
        c.fill_rect(&r.footer, p.card_subtle);
        c.line(
            r.nav_pane.right as f32 - 0.5,
            0.0,
            r.nav_pane.right as f32 - 0.5,
            client.bottom as f32,
            p.divider,
            1.0,
        );
        c.line(
            r.footer.left as f32,
            r.footer.top as f32 + 0.5,
            r.footer.right as f32,
            r.footer.top as f32 + 0.5,
            p.divider,
            1.0,
        );

        // 2. Top-left Brand Logo Header + Navigation Rail
        let badge_sz = scaled(32, dpi);
        let badge_l = scaled(14, dpi);
        let badge_t = scaled(17, dpi);
        let badge = RECT {
            left: badge_l,
            top: badge_t,
            right: badge_l + badge_sz,
            bottom: badge_t + badge_sz,
        };
        c.brand_logo(&badge, dpi);
        let brand_name = RECT {
            left: badge.right + scaled(9, dpi),
            top: badge.top - scaled(1, dpi),
            right: r.nav_pane.right - scaled(8, dpi),
            bottom: badge.top + scaled(17, dpi),
        };
        let brand_sub = RECT {
            left: badge.right + scaled(9, dpi),
            top: brand_name.bottom,
            right: r.nav_pane.right - scaled(8, dpi),
            bottom: badge.bottom + scaled(2, dpi),
        };
        c.draw_text(
            "QuickPhrase",
            &brand_name,
            TextStyle::Heading,
            dpi,
            p.heading,
            AlignH::Left,
            AlignV::Center,
        );
        c.draw_text(
            "偏好设置",
            &brand_sub,
            TextStyle::Badge,
            dpi,
            p.muted,
            AlignH::Left,
            AlignV::Center,
        );
        c.line(
            scaled(14, dpi) as f32,
            scaled(62, dpi) as f32 - 0.5,
            (r.nav_pane.right - scaled(14, dpi)) as f32,
            scaled(62, dpi) as f32 - 0.5,
            p.divider,
            1.0,
        );

        let nav_defs = [
            (Tab::General, Icon::Keyboard, "热键与启动"),
            (Tab::Input, Icon::EditPen, "输入与剪贴板"),
            (Tab::Appearance, Icon::Palette, "外观与主题"),
            (Tab::Data, Icon::Folder, "数据与存储"),
            (Tab::Apps, Icon::Grid, "应用输入例外"),
        ];
        for (idx, (tab, icon, label)) in nav_defs.iter().enumerate() {
            let item = r.nav_items[idx];
            let selected = ui.tab == *tab;
            let hovered = ui.hover == Hover::Nav(idx);
            if selected {
                c.fill_round(&item, d2d::scaled_f(10.0, dpi), p.selection);
                let bar = RECT {
                    left: item.left + scaled(3, dpi),
                    top: item.top + scaled(9, dpi),
                    right: item.left + scaled(6, dpi),
                    bottom: item.bottom - scaled(9, dpi),
                };
                c.fill_round(&bar, d2d::scaled_f(2.0, dpi), p.primary);
            } else if hovered {
                c.fill_round(&item, d2d::scaled_f(10.0, dpi), p.hover);
            }
            let fg = if selected {
                p.primary_text
            } else {
                p.secondary
            };
            c.icon(
                *icon,
                (item.left + scaled(22, dpi)) as f32,
                (item.top + item.bottom) as f32 * 0.5,
                d2d::scaled_f(15.5, dpi),
                fg,
            );
            let txt = RECT {
                left: item.left + scaled(38, dpi),
                top: item.top,
                right: item.right - scaled(8, dpi),
                bottom: item.bottom,
            };
            c.draw_text(
                label,
                &txt,
                if selected {
                    TextStyle::Heading
                } else {
                    TextStyle::Body
                },
                dpi,
                fg,
                AlignH::Left,
                AlignV::Center,
            );
        }

        // 3. Section Header
        let (title, subtitle) = match ui.tab {
            Tab::General => ("热键与启动", "配置全局唤起快捷键与系统自启动行为"),
            Tab::Input => ("输入与剪贴板", "选择文本注入策略与剪贴板安全保护"),
            Tab::Appearance => ("外观与主题", "点击卡片即时预览全部窗口的视觉主题"),
            Tab::Data => (
                "数据与存储",
                "本地 JSON 原子写入 · 自动保留最近 5 份滚动备份",
            ),
            Tab::Apps => (
                "应用输入例外",
                "为单个可执行文件指定 Unicode 或粘贴模式；未命中时使用全局模式",
            ),
        };
        let hdr_l = r.content_pane.left + scaled(22, dpi);
        let hdr_r = r.content_pane.right - scaled(22, dpi);
        let title_r = RECT {
            left: hdr_l,
            top: scaled(16, dpi),
            right: hdr_r,
            bottom: scaled(40, dpi),
        };
        let sub_r = RECT {
            left: hdr_l,
            top: scaled(40, dpi),
            right: hdr_r,
            bottom: scaled(58, dpi),
        };
        c.draw_text(
            title,
            &title_r,
            TextStyle::Display,
            dpi,
            p.heading,
            AlignH::Left,
            AlignV::Center,
        );
        c.draw_text(
            subtitle,
            &sub_r,
            TextStyle::Caption,
            dpi,
            p.secondary,
            AlignH::Left,
            AlignV::Center,
        );

        // 4. Active Tab Body
        match ui.tab {
            Tab::General => paint_tab_general(ui, c, p, r, dpi),
            Tab::Input => paint_tab_input(ui, c, p, r, dpi),
            Tab::Appearance => paint_tab_appearance(ui, c, p, r, dpi),
            Tab::Data => paint_tab_data(ui, c, p, r, dpi),
            Tab::Apps => paint_tab_apps(c, p, r, dpi),
        }

        // 5. Keyboard focus ring for custom cards/navigation.
        if let Some(rect) = focus_rect(ui) {
            c.stroke_round(
                &rect,
                d2d::scaled_f(10.0, dpi),
                p.primary,
                d2d::scaled_f(1.5, dpi),
            );
        }

        // 6. Footer Status
        c.draw_text(
            &ui.status_text,
            &r.status,
            TextStyle::Caption,
            dpi,
            p.muted,
            AlignH::Left,
            AlignV::Center,
        );
    });
}

fn paint_tab_general(
    ui: &SettingsUi,
    c: &mut d2d::Canvas<'_>,
    p: theme::Palette,
    r: Rects,
    dpi: u32,
) {
    let rad = d2d::scaled_f(12.0, dpi);
    let pad = scaled(16, dpi);

    // Hotkey Card
    c.fill_stroke_round(
        &r.hotkey_card,
        rad,
        p.card,
        if ui.capturing { p.primary } else { p.border },
        if ui.capturing {
            d2d::scaled_f(1.5, dpi)
        } else {
            1.0
        },
    );
    let title_right = if r.hotkey_kbd_box.top > r.hotkey_card.top + scaled(40, dpi) {
        r.hotkey_card.right - pad
    } else {
        r.hotkey_kbd_box.left - scaled(10, dpi)
    };
    let t1 = RECT {
        left: r.hotkey_card.left + pad,
        top: r.hotkey_card.top + scaled(14, dpi),
        right: title_right,
        bottom: r.hotkey_card.top + scaled(34, dpi),
    };
    let d1 = RECT {
        left: t1.left,
        top: t1.bottom + scaled(2, dpi),
        right: t1.right,
        bottom: t1.bottom + scaled(20, dpi),
    };
    c.draw_text(
        "全局唤起快捷键",
        &t1,
        TextStyle::Heading,
        dpi,
        p.heading,
        AlignH::Left,
        AlignV::Center,
    );
    c.draw_text(
        "在任意输入框按下即可弹出短语面板",
        &d1,
        TextStyle::Caption,
        dpi,
        p.secondary,
        AlignH::Left,
        AlignV::Center,
    );

    // Hotkey KBD Container
    c.fill_stroke_round(
        &r.hotkey_kbd_box,
        d2d::scaled_f(9.0, dpi),
        p.field,
        p.border,
        1.0,
    );
    let hk_display = if ui.hotkey_candidate.is_empty() {
        ui.original_hotkey.as_str()
    } else {
        ui.hotkey_candidate.as_str()
    };
    let inner_kbd = RECT {
        left: r.hotkey_kbd_box.left + scaled(6, dpi),
        top: r.hotkey_kbd_box.top + scaled(5, dpi),
        right: r.hotkey_kbd_box.right - scaled(6, dpi),
        bottom: r.hotkey_kbd_box.bottom - scaled(5, dpi),
    };
    c.kbd(hk_display, &inner_kbd, p, dpi);

    if ui.capturing {
        let hint_box = RECT {
            left: r.hotkey_card.left + pad,
            top: r.hotkey_card.bottom - scaled(38, dpi),
            right: r.hotkey_card.right - pad,
            bottom: r.hotkey_card.bottom - scaled(12, dpi),
        };
        c.fill_round(&hint_box, d2d::scaled_f(8.0, dpi), p.primary_soft);
        let ht = RECT {
            left: hint_box.left + scaled(10, dpi),
            top: hint_box.top,
            right: hint_box.right - scaled(10, dpi),
            bottom: hint_box.bottom,
        };
        c.draw_text(
            "正在录制：请直接按下新的组合键（需含 Ctrl / Alt / Shift / Win），按 Esc 取消",
            &ht,
            TextStyle::Label,
            dpi,
            p.primary_text,
            AlignH::Left,
            AlignV::Center,
        );
    }

    // Startup Card
    c.fill_stroke_round(
        &r.startup_card,
        rad,
        if ui.hover == Hover::Startup {
            p.hover
        } else {
            p.card
        },
        p.border,
        1.0,
    );
    let t2 = RECT {
        left: r.startup_card.left + pad,
        top: r.startup_card.top + scaled(14, dpi),
        right: r.startup_switch.left - scaled(12, dpi),
        bottom: r.startup_card.top + scaled(34, dpi),
    };
    let d2 = RECT {
        left: t2.left,
        top: t2.bottom + scaled(2, dpi),
        right: t2.right,
        bottom: t2.bottom + scaled(20, dpi),
    };
    c.draw_text(
        "随 Windows 开机启动",
        &t2,
        TextStyle::Heading,
        dpi,
        p.heading,
        AlignH::Left,
        AlignV::Center,
    );
    c.draw_text(
        "登录系统后自动在托盘静默就绪，无需管理员权限",
        &d2,
        TextStyle::Caption,
        dpi,
        p.secondary,
        AlignH::Left,
        AlignV::Center,
    );
    c.switch(
        &r.startup_switch,
        ui.startup,
        ui.hover == Hover::Startup,
        false,
        p,
        dpi,
    );
}

fn paint_tab_input(
    ui: &SettingsUi,
    c: &mut d2d::Canvas<'_>,
    p: theme::Palette,
    r: Rects,
    dpi: u32,
) {
    let rad = d2d::scaled_f(12.0, dpi);
    let pad = scaled(14, dpi);

    // Radio Card 1: Unicode
    let uni_sel = !ui.paste_mode;
    c.fill_stroke_round(
        &r.mode_unicode,
        rad,
        if uni_sel {
            p.selection
        } else if ui.hover == Hover::ModeUnicode {
            p.hover
        } else {
            p.card
        },
        if uni_sel { p.primary } else { p.border },
        if uni_sel {
            d2d::scaled_f(1.5, dpi)
        } else {
            1.0
        },
    );
    let rcx = (r.mode_unicode.left + scaled(22, dpi)) as f32;
    let rcy = (r.mode_unicode.top + scaled(24, dpi)) as f32;
    c.stroke_circle(
        rcx,
        rcy,
        d2d::scaled_f(7.0, dpi),
        if uni_sel { p.primary } else { p.border_strong },
        d2d::scaled_f(1.6, dpi),
    );
    if uni_sel {
        c.fill_circle(rcx, rcy, d2d::scaled_f(3.6, dpi), p.primary);
    }
    let ut = RECT {
        left: r.mode_unicode.left + scaled(38, dpi),
        top: r.mode_unicode.top + scaled(12, dpi),
        right: r.mode_unicode.left + scaled(160, dpi),
        bottom: r.mode_unicode.top + scaled(34, dpi),
    };
    c.draw_text(
        "Unicode 直接输入",
        &ut,
        TextStyle::Heading,
        dpi,
        p.heading,
        AlignH::Left,
        AlignV::Center,
    );
    let rec_pill = RECT {
        left: ut.right + scaled(2, dpi),
        top: r.mode_unicode.top + scaled(14, dpi),
        right: ut.right + scaled(40, dpi),
        bottom: r.mode_unicode.top + scaled(32, dpi),
    };
    c.fill_round(&rec_pill, d2d::scaled_f(6.0, dpi), p.success_soft);
    c.draw_text(
        "推荐",
        &rec_pill,
        TextStyle::Badge,
        dpi,
        p.success,
        AlignH::Center,
        AlignV::Center,
    );
    let ud = RECT {
        left: ut.left,
        top: ut.bottom + scaled(2, dpi),
        right: r.mode_unicode.right - pad,
        bottom: r.mode_unicode.bottom - scaled(8, dpi),
    };
    c.draw_text(
        "通过 SendInput 模拟键盘输入，完全不占用或污染剪贴板，适合绝大多数场景",
        &ud,
        TextStyle::Caption,
        dpi,
        p.secondary,
        AlignH::Left,
        AlignV::Center,
    );

    // Radio Card 2: Paste Mode
    let pas_sel = ui.paste_mode;
    c.fill_stroke_round(
        &r.mode_paste,
        rad,
        if pas_sel {
            p.selection
        } else if ui.hover == Hover::ModePaste {
            p.hover
        } else {
            p.card
        },
        if pas_sel { p.primary } else { p.border },
        if pas_sel {
            d2d::scaled_f(1.5, dpi)
        } else {
            1.0
        },
    );
    let pcx = (r.mode_paste.left + scaled(22, dpi)) as f32;
    let pcy = (r.mode_paste.top + scaled(24, dpi)) as f32;
    c.stroke_circle(
        pcx,
        pcy,
        d2d::scaled_f(7.0, dpi),
        if pas_sel { p.primary } else { p.border_strong },
        d2d::scaled_f(1.6, dpi),
    );
    if pas_sel {
        c.fill_circle(pcx, pcy, d2d::scaled_f(3.6, dpi), p.primary);
    }
    let pt = RECT {
        left: r.mode_paste.left + scaled(38, dpi),
        top: r.mode_paste.top + scaled(12, dpi),
        right: r.mode_paste.right - pad,
        bottom: r.mode_paste.top + scaled(34, dpi),
    };
    c.draw_text(
        "粘贴兼容模式 (Ctrl+V)",
        &pt,
        TextStyle::Heading,
        dpi,
        p.heading,
        AlignH::Left,
        AlignV::Center,
    );
    let pd = RECT {
        left: pt.left,
        top: pt.bottom + scaled(2, dpi),
        right: r.mode_paste.right - pad,
        bottom: r.mode_paste.bottom - scaled(8, dpi),
    };
    c.draw_text(
        "通过剪贴板粘贴输入，对特殊富文本控件兼容性更好；终端与未知区域仍仅复制",
        &pd,
        TextStyle::Caption,
        dpi,
        p.secondary,
        AlignH::Left,
        AlignV::Center,
    );

    // Optional Paste Warning Banner
    if ui.paste_mode && r.paste_warn.bottom > r.paste_warn.top {
        c.fill_round(&r.paste_warn, d2d::scaled_f(9.0, dpi), p.warning_soft);
        c.icon(
            Icon::Warning,
            (r.paste_warn.left + scaled(18, dpi)) as f32,
            (r.paste_warn.top + r.paste_warn.bottom) as f32 * 0.5,
            d2d::scaled_f(13.5, dpi),
            p.warning,
        );
        let wt = RECT {
            left: r.paste_warn.left + scaled(32, dpi),
            top: r.paste_warn.top,
            right: r.paste_warn.right - scaled(10, dpi),
            bottom: r.paste_warn.bottom,
        };
        c.draw_text(
            "粘贴模式会替换当前剪贴板；原纯文本可在下方或托盘一键恢复，非文本无法恢复",
            &wt,
            TextStyle::Label,
            dpi,
            p.warning,
            AlignH::Left,
            AlignV::Center,
        );
    }

    // Tab Policy Segmented Card
    c.fill_stroke_round(&r.tab_card, rad, p.card, p.border, 1.0);
    let ttl = RECT {
        left: r.tab_card.left + pad,
        top: r.tab_card.top + scaled(10, dpi),
        right: r.tab_card.right - pad,
        bottom: r.tab_card.top + scaled(32, dpi),
    };
    c.draw_text(
        "制表符 (Tab) 策略",
        &ttl,
        TextStyle::Heading,
        dpi,
        p.heading,
        AlignH::Left,
        AlignV::Center,
    );
    let seg_well = RECT {
        left: r.tab_spaces.left - scaled(3, dpi),
        top: r.tab_spaces.top - scaled(3, dpi),
        right: r.tab_keep.right + scaled(3, dpi),
        bottom: r.tab_keep.bottom + scaled(3, dpi),
    };
    c.fill_round(&seg_well, d2d::scaled_f(9.0, dpi), p.field);

    let keep_active = ui.paste_mode && ui.keep_tabs;
    if !keep_active {
        c.fill_stroke_round(
            &r.tab_spaces,
            d2d::scaled_f(7.0, dpi),
            p.card,
            p.border,
            1.0,
        );
    } else if ui.hover == Hover::TabSpaces {
        c.fill_round(&r.tab_spaces, d2d::scaled_f(7.0, dpi), p.hover);
    }
    if keep_active {
        c.fill_stroke_round(&r.tab_keep, d2d::scaled_f(7.0, dpi), p.card, p.border, 1.0);
    } else if ui.paste_mode && ui.hover == Hover::TabKeep {
        c.fill_round(&r.tab_keep, d2d::scaled_f(7.0, dpi), p.hover);
    }
    c.draw_text(
        "转换为 4 空格",
        &r.tab_spaces,
        TextStyle::Label,
        dpi,
        if !keep_active { p.heading } else { p.secondary },
        AlignH::Center,
        AlignV::Center,
    );
    c.draw_text(
        if ui.paste_mode {
            "保留原始 Tab"
        } else {
            "保留原始 Tab（仅粘贴模式）"
        },
        &r.tab_keep,
        TextStyle::Label,
        dpi,
        if !ui.paste_mode {
            p.muted
        } else if keep_active {
            p.heading
        } else {
            p.secondary
        },
        AlignH::Center,
        AlignV::Center,
    );

    // Clipboard Card
    c.fill_stroke_round(&r.clip_card, rad, p.card, p.border, 1.0);
    let ct = RECT {
        left: r.clip_card.left + pad,
        top: r.clip_card.top + scaled(11, dpi),
        right: r.clip_card.right - scaled(188, dpi),
        bottom: r.clip_card.top + scaled(31, dpi),
    };
    let cd = RECT {
        left: ct.left,
        top: ct.bottom + scaled(2, dpi),
        right: ct.right,
        bottom: r.clip_card.bottom - scaled(8, dpi),
    };
    c.draw_text(
        "剪贴板快照恢复",
        &ct,
        TextStyle::Heading,
        dpi,
        p.heading,
        AlignH::Left,
        AlignV::Center,
    );
    c.draw_text(
        "还原上次快捷复制或粘贴前的纯文本内容",
        &cd,
        TextStyle::Caption,
        dpi,
        p.secondary,
        AlignH::Left,
        AlignV::Center,
    );
}

fn paint_tab_appearance(
    ui: &SettingsUi,
    c: &mut d2d::Canvas<'_>,
    p: theme::Palette,
    r: Rects,
    dpi: u32,
) {
    let rad = d2d::scaled_f(12.0, dpi);
    let pad = scaled(16, dpi);
    let cards = [
        ("light", "浅色 Porcelain", false, false),
        ("dark", "深色 Obsidian", true, false),
        ("system", "跟随 Windows", false, true),
    ];

    for (idx, (key, title, is_dark, is_split)) in cards.iter().enumerate() {
        let card = r.theme_cards[idx];
        let active = ui.theme_pref == *key;
        let hovered = ui.hover == Hover::ThemeCard(idx);

        c.fill_stroke_round(
            &card,
            d2d::scaled_f(12.0, dpi),
            if hovered && !active { p.hover } else { p.card },
            if active { p.primary } else { p.border },
            if active { d2d::scaled_f(2.0, dpi) } else { 1.0 },
        );

        // Miniature window preview
        let mpad = scaled(10, dpi);
        let mini = RECT {
            left: card.left + mpad,
            top: card.top + mpad,
            right: card.right - mpad,
            bottom: card.bottom - scaled(38, dpi),
        };
        let mrad = d2d::scaled_f(8.0, dpi);
        if *is_split {
            let mid = (mini.left + mini.right) / 2;
            let left_half = RECT { right: mid, ..mini };
            let right_half = RECT { left: mid, ..mini };
            c.fill_round(&mini, mrad, theme::LIGHT.page);
            c.fill_rect(&right_half, theme::DARK.page);
            // Tiny light pill
            let lp = RECT {
                left: left_half.left + scaled(6, dpi),
                top: left_half.top + scaled(10, dpi),
                right: mid - scaled(4, dpi),
                bottom: left_half.top + scaled(24, dpi),
            };
            c.fill_round(&lp, d2d::scaled_f(4.0, dpi), theme::LIGHT.card);
            let lp2 = RECT {
                top: lp.bottom + scaled(6, dpi),
                bottom: lp.bottom + scaled(22, dpi),
                ..lp
            };
            c.fill_round(&lp2, d2d::scaled_f(4.0, dpi), theme::LIGHT.selection);
            // Tiny dark pill
            let rp = RECT {
                left: mid + scaled(4, dpi),
                top: right_half.top + scaled(10, dpi),
                right: right_half.right - scaled(6, dpi),
                bottom: right_half.top + scaled(24, dpi),
            };
            c.fill_round(&rp, d2d::scaled_f(4.0, dpi), theme::DARK.card);
            let rp2 = RECT {
                top: rp.bottom + scaled(6, dpi),
                bottom: rp.bottom + scaled(22, dpi),
                ..rp
            };
            c.fill_round(&rp2, d2d::scaled_f(4.0, dpi), theme::DARK.selection);
            c.stroke_round(&mini, mrad, p.border, 1.0);
        } else {
            let mp = if *is_dark { theme::DARK } else { theme::LIGHT };
            c.fill_stroke_round(&mini, mrad, mp.page, mp.border, 1.0);
            let sbar = RECT {
                left: mini.left + scaled(8, dpi),
                top: mini.top + scaled(10, dpi),
                right: mini.right - scaled(8, dpi),
                bottom: mini.top + scaled(24, dpi),
            };
            c.fill_stroke_round(&sbar, d2d::scaled_f(4.0, dpi), mp.card, mp.border, 1.0);
            c.fill_circle(
                (sbar.left + scaled(6, dpi)) as f32,
                (sbar.top + sbar.bottom) as f32 * 0.5,
                d2d::scaled_f(2.5, dpi),
                mp.primary,
            );
            let sel = RECT {
                left: mini.left + scaled(8, dpi),
                top: sbar.bottom + scaled(8, dpi),
                right: mini.right - scaled(8, dpi),
                bottom: sbar.bottom + scaled(26, dpi),
            };
            c.fill_stroke_round(
                &sel,
                d2d::scaled_f(5.0, dpi),
                mp.selection,
                mp.selection_border,
                1.0,
            );
            let r2 = RECT {
                left: mini.left + scaled(8, dpi),
                top: sel.bottom + scaled(6, dpi),
                right: mini.right - scaled(28, dpi),
                bottom: (sel.bottom + scaled(18, dpi)).min(mini.bottom - scaled(6, dpi)),
            };
            c.fill_round(&r2, d2d::scaled_f(4.0, dpi), mp.card);
        }

        // Card Label + Active Checkmark
        let lbl = RECT {
            left: card.left + scaled(12, dpi),
            top: mini.bottom + scaled(4, dpi),
            right: card.right - scaled(28, dpi),
            bottom: card.bottom - scaled(4, dpi),
        };
        c.draw_text(
            title,
            &lbl,
            TextStyle::Label,
            dpi,
            p.heading,
            AlignH::Left,
            AlignV::Center,
        );
        if active {
            let cx = (card.right - scaled(18, dpi)) as f32;
            let cy = (lbl.top + lbl.bottom) as f32 * 0.5;
            c.fill_circle(cx, cy, d2d::scaled_f(8.0, dpi), p.primary);
            c.icon(
                Icon::Check,
                cx,
                cy,
                d2d::scaled_f(9.0, dpi),
                COLORREF(0xFFFFFF),
            );
        }
    }

    c.fill_stroke_round(&r.sort_card, rad, p.card, p.border, 1.0);
    let st = RECT {
        left: r.sort_card.left + pad,
        top: r.sort_card.top + scaled(14, dpi),
        right: r.sort_card.right - scaled(184, dpi),
        bottom: r.sort_card.top + scaled(34, dpi),
    };
    let sd = RECT {
        left: st.left,
        top: st.bottom + scaled(2, dpi),
        right: st.right,
        bottom: r.sort_card.bottom - scaled(10, dpi),
    };
    c.draw_text(
        "面板排序",
        &st,
        TextStyle::Heading,
        dpi,
        p.heading,
        AlignH::Left,
        AlignV::Center,
    );
    c.draw_text(
        "最近使用优先，或按累计使用次数从多到少",
        &sd,
        TextStyle::Caption,
        dpi,
        p.secondary,
        AlignH::Left,
        AlignV::Center,
    );
}

fn paint_tab_apps(c: &mut d2d::Canvas<'_>, p: theme::Palette, r: Rects, dpi: u32) {
    let rad = d2d::scaled_f(12.0, dpi);
    let pad = scaled(16, dpi);
    c.fill_stroke_round(&r.apps_card, rad, p.card, p.border, 1.0);
    let title = RECT {
        left: r.apps_card.left + pad,
        top: r.apps_card.top + scaled(14, dpi),
        right: r.apps_card.right - pad,
        bottom: r.apps_card.top + scaled(34, dpi),
    };
    let desc = RECT {
        left: title.left,
        top: title.bottom + scaled(2, dpi),
        right: title.right,
        bottom: r.apps_exe_box.top - scaled(4, dpi),
    };
    c.draw_text(
        "应用输入例外",
        &title,
        TextStyle::Heading,
        dpi,
        p.heading,
        AlignH::Left,
        AlignV::Center,
    );
    c.draw_text(
        "输入可执行文件名（如 chrome.exe），为该应用单独选择输入模式；未添加规则的应用使用全局设置",
        &desc,
        TextStyle::Caption,
        dpi,
        p.secondary,
        AlignH::Left,
        AlignV::Center,
    );
}

fn paint_tab_data(ui: &SettingsUi, c: &mut d2d::Canvas<'_>, p: theme::Palette, r: Rects, dpi: u32) {
    let rad = d2d::scaled_f(12.0, dpi);
    let pad = scaled(16, dpi);
    c.fill_stroke_round(&r.data_card, rad, p.card, p.border, 1.0);

    let title_r = RECT {
        left: r.data_card.left + pad,
        top: r.data_card.top + scaled(14, dpi),
        right: r.data_card.right - pad,
        bottom: r.data_card.top + scaled(34, dpi),
    };
    c.draw_text(
        "本地数据目录",
        &title_r,
        TextStyle::Heading,
        dpi,
        p.heading,
        AlignH::Left,
        AlignV::Center,
    );

    c.fill_stroke_round(
        &r.data_path_box,
        d2d::scaled_f(9.0, dpi),
        p.field,
        p.border,
        1.0,
    );
    let ptxt = RECT {
        left: r.data_path_box.left + scaled(10, dpi),
        top: r.data_path_box.top,
        right: r.data_path_box.right - scaled(10, dpi),
        bottom: r.data_path_box.bottom,
    };
    c.draw_text(
        &ui.data_dir,
        &ptxt,
        TextStyle::Mono,
        dpi,
        p.secondary,
        AlignH::Left,
        AlignV::Center,
    );

    let active_count = storage::active_count();
    let read_only = storage::read_only_notice().is_some();
    let has_notice = storage::storage_notice().is_some();
    let stats = [
        (format!("{active_count}"), "有效短语", p.heading),
        ("5 份".to_string(), "滚动备份", p.heading),
        (
            if read_only {
                "只读".to_string()
            } else if has_notice {
                "异常".to_string()
            } else {
                "正常".to_string()
            },
            "存储状态",
            if read_only || has_notice {
                p.warning
            } else {
                p.success
            },
        ),
    ];

    for (idx, (val, label, val_color)) in stats.iter().enumerate() {
        let sb = r.stat_boxes[idx];
        c.fill_round(&sb, d2d::scaled_f(10.0, dpi), p.card_subtle);
        let vr = RECT {
            left: sb.left + scaled(6, dpi),
            top: sb.top + scaled(12, dpi),
            right: sb.right - scaled(6, dpi),
            bottom: sb.top + scaled(40, dpi),
        };
        let lr = RECT {
            left: sb.left + scaled(6, dpi),
            top: vr.bottom,
            right: sb.right - scaled(6, dpi),
            bottom: sb.bottom - scaled(8, dpi),
        };
        c.draw_text(
            val,
            &vr,
            TextStyle::Display,
            dpi,
            *val_color,
            AlignH::Center,
            AlignV::Center,
        );
        c.draw_text(
            label,
            &lr,
            TextStyle::Caption,
            dpi,
            p.muted,
            AlignH::Center,
            AlignV::Center,
        );
    }
}

fn create_button(text: &str, parent: HWND, id: i32, instance: HINSTANCE, visible: bool) -> HWND {
    let text = platform::wide(text);
    let mut style = WS_CHILD | WS_TABSTOP | WINDOW_STYLE(BS_OWNERDRAW as u32);
    if visible {
        style |= WS_VISIBLE;
    }
    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            w!("BUTTON"),
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
    };
    ui::install_button(hwnd);
    hwnd
}

fn create_input(
    class: PCWSTR,
    parent: HWND,
    id: i32,
    style: WINDOW_STYLE,
    instance: HINSTANCE,
) -> HWND {
    unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            class,
            PCWSTR::null(),
            style | WS_CHILD | WS_VISIBLE | WS_TABSTOP,
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

fn control_text(hwnd: HWND) -> String {
    let length = unsafe { SendMessageW(hwnd, WM_GETTEXTLENGTH, Some(WPARAM(0)), Some(LPARAM(0))) }
        .0
        .max(0) as usize;
    if length == 0 {
        return String::new();
    }
    let mut buffer = vec![0u16; length + 1];
    unsafe {
        SendMessageW(
            hwnd,
            WM_GETTEXT,
            Some(WPARAM(buffer.len())),
            Some(LPARAM(buffer.as_mut_ptr() as isize)),
        );
    }
    String::from_utf16_lossy(&buffer[..length])
}

fn reload_apps_rules(ui: &SettingsUi) {
    let mut text = String::new();
    for rule in storage::rules() {
        if !text.is_empty() {
            text.push_str("\r\n");
        }
        let mode = if rule.action == "paste" {
            "粘贴"
        } else {
            "Unicode"
        };
        text.push_str(&format!("{}  ·  {}", rule.exe, mode));
    }
    if text.is_empty() {
        text.push_str("暂无应用例外");
    }
    set_button_text(ui.apps_rules, &text);
}

fn create() -> Option<SettingsUi> {
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
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
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

    ui::apply_window_style(hwnd);
    ui::apply_window_icon(hwnd, instance);
    let win_dpi = unsafe { GetDpiForWindow(hwnd) };
    let dpi = if win_dpi == 0 { dpi } else { win_dpi };
    let font = make_font(dpi);

    let hotkey_record = create_button("录制", hwnd, ID_HOTKEY_RECORD, instance, true);
    let restore_clip = create_button("恢复上次剪贴板内容", hwnd, ID_RESTORE_CLIP, instance, false);
    let copy_dir = create_button("复制路径", hwnd, ID_COPY_DIR, instance, false);
    let open_dir = create_button("打开目录", hwnd, ID_OPEN_DIR, instance, false);
    let export_data = create_button("导出…", hwnd, ID_EXPORT_DATA, instance, false);
    let import_data = create_button("导入…", hwnd, ID_IMPORT_DATA, instance, false);
    let restore_backup = create_button("恢复备份…", hwnd, ID_RESTORE_BACKUP, instance, false);
    let sort_mode = create_button("排序：最近使用", hwnd, ID_SORT_MODE, instance, false);
    let apps_exe = create_input(
        w!("EDIT"),
        hwnd,
        ID_APPS_EXE,
        WINDOW_STYLE(ES_AUTOHSCROLL as u32),
        instance,
    );
    let apps_mode = create_input(
        w!("COMBOBOX"),
        hwnd,
        ID_APPS_MODE,
        WINDOW_STYLE((CBS_DROPDOWNLIST | WS_VSCROLL.0 as i32) as u32),
        instance,
    );
    for label in ["Unicode 输入", "粘贴兼容"] {
        let wide = platform::wide(label);
        unsafe {
            SendMessageW(
                apps_mode,
                CB_ADDSTRING,
                Some(WPARAM(0)),
                Some(LPARAM(wide.as_ptr() as isize)),
            );
            let _ = SendMessageW(apps_mode, CB_SETCURSEL, Some(WPARAM(0)), Some(LPARAM(0)));
        }
    }
    let apps_rules = create_input(
        w!("EDIT"),
        hwnd,
        ID_APPS_RULES,
        WINDOW_STYLE((ES_MULTILINE | ES_READONLY | ES_AUTOVSCROLL) as u32),
        instance,
    );
    let apps_add = create_button("添加/更新", hwnd, ID_APPS_ADD, instance, false);
    let apps_remove = create_button("删除此应用", hwnd, ID_APPS_REMOVE, instance, false);
    let save = create_button("保存设置", hwnd, ID_SAVE, instance, true);
    let cancel = create_button("取消", hwnd, ID_CANCEL, instance, true);

    for control in [
        hotkey_record,
        restore_clip,
        copy_dir,
        open_dir,
        export_data,
        import_data,
        restore_backup,
        sort_mode,
        apps_exe,
        apps_mode,
        apps_rules,
        apps_add,
        apps_remove,
        save,
        cancel,
    ] {
        unsafe {
            let _ = SendMessageW(
                control,
                WM_SETFONT,
                Some(WPARAM(font.0 as usize)),
                Some(LPARAM(1)),
            );
        }
    }

    let field_brush = unsafe { CreateSolidBrush(palette.field) };
    let mut ui = SettingsUi {
        hwnd,
        hotkey_record,
        restore_clip,
        copy_dir,
        open_dir,
        export_data,
        import_data,
        restore_backup,
        sort_mode,
        apps_exe,
        apps_mode,
        apps_rules,
        apps_add,
        apps_remove,
        field_brush,
        save,
        cancel,
        font,
        palette,
        dpi,
        tab: Tab::General,
        hover: Hover::None,
        focus: Hover::None,
        active: false,
        tracking_mouse: false,
        rects: Rects::default(),
        capturing: false,
        hotkey_candidate: String::new(),
        original_hotkey: String::new(),
        paste_mode: false,
        keep_tabs: false,
        theme_pref: "system".to_string(),
        panel_sort: "recent".to_string(),
        startup: false,
        status_text: "就绪".to_string(),
        data_dir: platform::data_dir().display().to_string(),
    };
    ui::theme_control(ui.apps_exe, ui.palette.dark);
    ui::theme_control(ui.apps_rules, ui.palette.dark);
    ui::theme_combo(ui.apps_mode, ui.palette.dark);
    ui::install_smooth_edit(ui.apps_rules);
    ui::theme_scrollbar(ui.apps_rules, ui.palette.dark);
    ui::install_combo(ui.apps_mode);
    ui::cue_banner(ui.apps_exe, "例如 chrome.exe");
    reload_apps_rules(&ui);

    load(&mut ui);
    relayout(&mut ui);
    Some(ui)
}

fn load(ui: &mut SettingsUi) {
    let settings = storage::settings_clone().unwrap_or_default();
    ui.capturing = false;
    set_button_text(ui.hotkey_record, "录制");
    ui.original_hotkey = settings.hotkey.clone();
    ui.hotkey_candidate = settings.hotkey.clone();
    ui.paste_mode = settings.insertion_mode == "paste";
    ui.keep_tabs = ui.paste_mode && settings.tab_policy == "tab";
    ui.theme_pref = settings.theme.clone();
    ui.panel_sort = settings.panel_sort.clone();
    set_button_text(ui.sort_mode, sort_label(&ui.panel_sort));
    ui.startup = settings.startup;
    ui.data_dir = platform::data_dir().display().to_string();
    reload_apps_rules(ui);
    apply_live_theme(ui, &ui.theme_pref.clone());
}

fn apply_live_theme(ui: &mut SettingsUi, pref: &str) {
    let palette = theme::resolve(pref);
    if palette != ui.palette {
        ui.palette = palette;
        theme::preview(palette);
        unsafe {
            let _ = DeleteObject(HGDIOBJ(ui.field_brush.0));
        }
        ui.field_brush = unsafe { CreateSolidBrush(palette.field) };
        ui::theme_control(ui.apps_exe, palette.dark);
        ui::theme_control(ui.apps_rules, palette.dark);
        ui::theme_combo(ui.apps_mode, palette.dark);
        ui::theme_scrollbar(ui.apps_rules, palette.dark);
        ui::apply_window_style_for(ui.hwnd, palette, true);
    }
    unsafe {
        let _ = InvalidateRect(Some(ui.hwnd), None, false);
    }
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
        if let Ok(mut guard) = slot.try_borrow_mut() {
            if let Some(ui) = guard.as_mut() {
                load(ui);
                ui.status_text = "就绪".to_string();
                relayout(ui);
            }
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
    theme::clear_preview();
    let Some(ui) = ui else {
        return;
    };
    unsafe {
        let _ = DestroyWindow(ui.hwnd);
        let _ = DeleteObject(HGDIOBJ(ui.font.0));
        let _ = DeleteObject(HGDIOBJ(ui.field_brush.0));
    }
}

pub fn is_dialog_message(message: &MSG) -> bool {
    let hwnd = UI.with(|slot| {
        slot.try_borrow()
            .ok()
            .and_then(|guard| guard.as_ref().map(|ui| ui.hwnd))
    });
    let Some(hwnd) = hwnd.filter(|hwnd| !hwnd.0.is_null()) else {
        return false;
    };
    if !unsafe { IsWindowVisible(hwnd).as_bool() } {
        return false;
    }
    let focus = unsafe { GetFocus() };
    if focus != hwnd && !unsafe { IsChild(hwnd, focus).as_bool() } {
        return false;
    }

    if message.message == WM_KEYDOWN && message.wParam.0 as u32 == VK_TAB.0 as u32 {
        let focused = unsafe { GetFocus() };
        let owns_focus = focused == hwnd || unsafe { IsChild(hwnd, focused).as_bool() };
        if !unsafe { IsWindowVisible(hwnd).as_bool() } || !owns_focus {
            return false;
        }
        let capturing = UI.with(|slot| {
            slot.try_borrow()
                .ok()
                .and_then(|guard| guard.as_ref().map(|ui| ui.capturing))
                .unwrap_or(false)
        });
        let backwards = unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0;
        if capturing {
            // A Tab press abandons recording instead of being stored as an
            // invalid hotkey, then continues with normal focus navigation.
            let next = UI.with(|slot| {
                let Ok(mut guard) = slot.try_borrow_mut() else {
                    return None;
                };
                guard.as_mut().and_then(|ui| {
                    ui.capturing = false;
                    set_button_text(ui.hotkey_record, "录制");
                    relayout(ui);
                    set_status(ui, "已取消录制");
                    handle_tab(ui, backwards)
                })
            });
            if let Some(next) = next {
                unsafe {
                    let _ = SetFocus(Some(next));
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
            }
            return true;
        }
        let next = UI.with(|slot| {
            let Ok(mut guard) = slot.try_borrow_mut() else {
                return None;
            };
            guard.as_mut().and_then(|ui| handle_tab(ui, backwards))
        });
        if let Some(next) = next {
            unsafe {
                let _ = SetFocus(Some(next));
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
        }
        return true;
    }

    // Custom-drawn controls own the keyboard while the parent window is focused.
    let custom = UI.with(|slot| {
        slot.try_borrow()
            .ok()
            .and_then(|guard| {
                guard
                    .as_ref()
                    .map(|ui| ui.focus != Hover::None && unsafe { GetFocus() } == ui.hwnd)
            })
            .unwrap_or(false)
    });
    if custom {
        return false;
    }
    unsafe { IsDialogMessageW(hwnd, message).as_bool() }
}

fn capture_key(ui: &mut SettingsUi, vk: u32) {
    if vk == VK_TAB.0 as u32 {
        return;
    }
    if vk == 0x1B {
        ui.capturing = false;
        set_button_text(ui.hotkey_record, "录制");
        relayout(ui);
        set_status(ui, "已取消录制");
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
        set_status(
            ui,
            "无效组合：请同时按下 Ctrl/Alt/Shift/Win 与字母、数字或 F1–F24",
        );
        return;
    }
    let same_as_current = hotkey::parse(&ui.original_hotkey) == Some(spec);
    if !same_as_current && !hotkey::is_available(ui.hwnd, &text) {
        set_status(ui, "该快捷键已被其他程序占用");
        return;
    }
    ui.hotkey_candidate = text;
    ui.capturing = false;
    set_button_text(ui.hotkey_record, "录制");
    relayout(ui);
    set_status(ui, "快捷键已录制，保存后生效");
}

fn sort_label(value: &str) -> &'static str {
    if value == "most_used" {
        "排序：最常使用"
    } else {
        "排序：最近使用"
    }
}

fn save(ui: &mut SettingsUi) {
    let original = storage::settings_clone().unwrap_or_default();
    let mut settings = original.clone();
    let candidate = if ui.hotkey_candidate.is_empty() {
        ui.original_hotkey.clone()
    } else {
        ui.hotkey_candidate.clone()
    };
    let insertion = if ui.paste_mode { "paste" } else { "unicode" };
    let tab_policy = if ui.paste_mode && ui.keep_tabs {
        "tab"
    } else {
        "spaces4"
    };
    settings.hotkey = candidate.clone();
    settings.insertion_mode = insertion.to_string();
    settings.tab_policy = tab_policy.to_string();
    settings.theme = ui.theme_pref.clone();
    settings.panel_sort = ui.panel_sort.clone();
    settings.startup = ui.startup;

    if let Some(error) = storage::save_settings(settings.clone()).err() {
        set_status(ui, &format!("保存失败（{error}）"));
        return;
    }

    if let Err(code) = crate::app::apply_hotkey(&candidate) {
        let reapply = crate::app::apply_hotkey(&original.hotkey).is_ok();
        let rewrite = storage::save_settings(original.clone()).is_ok();
        ui.hotkey_candidate = original.hotkey.clone();
        if reapply && rewrite {
            set_status(ui, &format!("快捷键注册失败（{code}），已恢复原设置"));
        } else {
            set_status(ui, "回滚失败：请重启程序并检查热键设置");
        }
        return;
    }
    ui.original_hotkey = candidate;

    if let Err(code) = platform::set_startup(ui.startup) {
        logging::warn(&format!("settings: startup change failed ({code})"));
        let mut revert = settings.clone();
        revert.startup = original.startup;
        let rollback = storage::save_settings(revert);
        ui.startup = original.startup;
        if rollback.is_ok() {
            set_status(ui, "开机启动未生效，已恢复原设置");
        } else {
            set_status(ui, "开机启动未生效，且回滚写盘失败");
        }
        return;
    }
    apply_live_theme(ui, &ui.theme_pref.clone());
    set_status(ui, "全部设置已保存并生效");
}

fn point_in(r: &RECT, x: i32, y: i32) -> bool {
    x >= r.left && x <= r.right && y >= r.top && y <= r.bottom
}

/// Ordered Tab stops for the active page. Custom-drawn cards and native
/// buttons share one list so keyboard order stays predictable.
fn focus_items(ui: &SettingsUi) -> Vec<FocusItem> {
    let mut items = vec![
        FocusItem::Custom(Hover::Nav(0)),
        FocusItem::Custom(Hover::Nav(1)),
        FocusItem::Custom(Hover::Nav(2)),
        FocusItem::Custom(Hover::Nav(3)),
        FocusItem::Custom(Hover::Nav(4)),
    ];
    match ui.tab {
        Tab::General => {
            items.push(FocusItem::Custom(Hover::Startup));
            items.push(FocusItem::HotkeyRecord);
        }
        Tab::Input => {
            items.push(FocusItem::Custom(Hover::ModeUnicode));
            items.push(FocusItem::Custom(Hover::ModePaste));
            if ui.paste_mode {
                items.push(FocusItem::Custom(Hover::TabSpaces));
                items.push(FocusItem::Custom(Hover::TabKeep));
            }
            items.push(FocusItem::RestoreClip);
        }
        Tab::Appearance => {
            for index in 0..3 {
                items.push(FocusItem::Custom(Hover::ThemeCard(index)));
            }
            items.push(FocusItem::SortMode);
        }
        Tab::Data => {
            items.push(FocusItem::CopyDir);
            items.push(FocusItem::OpenDir);
            items.push(FocusItem::ExportData);
            items.push(FocusItem::ImportData);
            items.push(FocusItem::RestoreBackup);
        }
        Tab::Apps => {
            items.push(FocusItem::AppsExe);
            items.push(FocusItem::AppsMode);
            items.push(FocusItem::AppsRules);
            items.push(FocusItem::AppsAdd);
            items.push(FocusItem::AppsRemove);
        }
    }
    items.push(FocusItem::Save);
    items.push(FocusItem::Cancel);
    items
}

fn native_focus_hwnd(ui: &SettingsUi, item: FocusItem) -> Option<HWND> {
    let hwnd = match item {
        FocusItem::HotkeyRecord => ui.hotkey_record,
        FocusItem::RestoreClip => ui.restore_clip,
        FocusItem::CopyDir => ui.copy_dir,
        FocusItem::OpenDir => ui.open_dir,
        FocusItem::ExportData => ui.export_data,
        FocusItem::ImportData => ui.import_data,
        FocusItem::RestoreBackup => ui.restore_backup,
        FocusItem::SortMode => ui.sort_mode,
        FocusItem::AppsExe => ui.apps_exe,
        FocusItem::AppsMode => ui.apps_mode,
        FocusItem::AppsRules => ui.apps_rules,
        FocusItem::AppsAdd => ui.apps_add,
        FocusItem::AppsRemove => ui.apps_remove,
        FocusItem::Save => ui.save,
        FocusItem::Cancel => ui.cancel,
        FocusItem::Custom(_) => return None,
    };
    (!hwnd.0.is_null()).then_some(hwnd)
}

fn current_focus_item(ui: &SettingsUi) -> Option<FocusItem> {
    let focused = unsafe { GetFocus() };
    if focused == ui.hwnd {
        return (ui.focus != Hover::None).then_some(FocusItem::Custom(ui.focus));
    }
    focus_items(ui)
        .into_iter()
        .find(|item| native_focus_hwnd(ui, *item) == Some(focused))
}

/// Move the keyboard focus to the next/previous Tab stop and return the HWND
/// that must receive SetFocus outside the UI RefCell borrow.
fn handle_tab(ui: &mut SettingsUi, backwards: bool) -> Option<HWND> {
    let items = focus_items(ui);
    if items.is_empty() {
        return None;
    }
    let current = current_focus_item(ui).and_then(|item| items.iter().position(|it| *it == item));
    let next = match current {
        Some(index) => {
            let len = items.len();
            if backwards {
                (index + len - 1) % len
            } else {
                (index + 1) % len
            }
        }
        None if backwards => items.len() - 1,
        None => 0,
    };
    match items[next] {
        FocusItem::Custom(target) => {
            ui.focus = target;
            Some(ui.hwnd)
        }
        native => {
            ui.focus = Hover::None;
            native_focus_hwnd(ui, native)
        }
    }
}

/// Rectangle for the focused custom control, or None when a native control
/// owns focus (owner-draw buttons already paint their own focus ring).
fn focus_rect(ui: &SettingsUi) -> Option<RECT> {
    if !ui.active || unsafe { GetFocus() } != ui.hwnd {
        return None;
    }
    match ui.focus {
        Hover::Nav(index) => ui.rects.nav_items.get(index).copied(),
        Hover::Startup => Some(ui.rects.startup_card),
        Hover::ModeUnicode => Some(ui.rects.mode_unicode),
        Hover::ModePaste => Some(ui.rects.mode_paste),
        Hover::TabSpaces => Some(ui.rects.tab_spaces),
        Hover::TabKeep => Some(ui.rects.tab_keep),
        Hover::ThemeCard(index) => ui.rects.theme_cards.get(index).copied(),
        Hover::None => None,
    }
}

fn confirm_paste_mode(hwnd: HWND) -> bool {
    let text = platform::wide(
        "粘贴模式会用 Ctrl+V 插入，并替换当前剪贴板。\n\
         原纯文本内容可在托盘或设置中恢复；非文本内容无法恢复。\n\
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
    answer == IDYES
}

/// Apply a card/nav action. Returns true when the caller must confirm paste
/// mode after releasing the UI borrow.
fn activate_hover(ui: &mut SettingsUi, target: Hover) -> bool {
    match target {
        Hover::Nav(index) => {
            let next = match index {
                0 => Tab::General,
                1 => Tab::Input,
                2 => Tab::Appearance,
                3 => Tab::Data,
                _ => Tab::Apps,
            };
            if ui.tab != next {
                ui.tab = next;
                ui.capturing = false;
                set_button_text(ui.hotkey_record, "录制");
                relayout(ui);
            }
        }
        Hover::Startup => {
            ui.startup = !ui.startup;
            set_status(ui, "未保存更改");
        }
        Hover::ModeUnicode => {
            if ui.paste_mode {
                ui.paste_mode = false;
                ui.keep_tabs = false;
                relayout(ui);
                set_status(ui, "未保存：已切换为 Unicode 直接输入");
            }
        }
        Hover::ModePaste => {
            if !ui.paste_mode {
                return true;
            }
        }
        Hover::TabSpaces => {
            if ui.keep_tabs {
                ui.keep_tabs = false;
                set_status(ui, "未保存更改");
            }
        }
        Hover::TabKeep => {
            if ui.paste_mode && !ui.keep_tabs {
                ui.keep_tabs = true;
                set_status(ui, "未保存更改");
            }
        }
        Hover::ThemeCard(index) => {
            let pref = match index {
                0 => "light",
                1 => "dark",
                _ => "system",
            };
            ui.theme_pref = pref.to_string();
            apply_live_theme(ui, pref);
            set_status(ui, "主题已即时预览（保存后持久化）");
        }
        Hover::None => {}
    }
    false
}

fn hit_hover(ui: &SettingsUi, x: i32, y: i32) -> Hover {
    for (idx, item) in ui.rects.nav_items.iter().enumerate() {
        if point_in(item, x, y) {
            return Hover::Nav(idx);
        }
    }
    match ui.tab {
        Tab::General => {
            if point_in(&ui.rects.startup_card, x, y) {
                return Hover::Startup;
            }
        }
        Tab::Input => {
            if point_in(&ui.rects.mode_unicode, x, y) {
                return Hover::ModeUnicode;
            }
            if point_in(&ui.rects.mode_paste, x, y) {
                return Hover::ModePaste;
            }
            if point_in(&ui.rects.tab_spaces, x, y) {
                return Hover::TabSpaces;
            }
            if point_in(&ui.rects.tab_keep, x, y) {
                return Hover::TabKeep;
            }
        }
        Tab::Appearance => {
            for (idx, card) in ui.rects.theme_cards.iter().enumerate() {
                if point_in(card, x, y) {
                    return Hover::ThemeCard(idx);
                }
            }
        }
        Tab::Data | Tab::Apps => {}
    }
    Hover::None
}

#[derive(Clone, Copy)]
enum DataAction {
    Export,
    Import,
    RestoreBackup,
}

fn pick_data_file(hwnd: HWND, save: bool, initial_dir: &Path, title: &str) -> Option<PathBuf> {
    let mut filter: Vec<u16> = "QuickPhrase JSON".encode_utf16().collect();
    filter.push(0);
    filter.extend("*.json".encode_utf16());
    filter.push(0);
    filter.extend("所有文件".encode_utf16());
    filter.push(0);
    filter.extend("*.*".encode_utf16());
    filter.push(0);
    filter.push(0);
    let title = platform::wide(title);
    let initial = platform::wide(&initial_dir.display().to_string());
    let mut buffer = [0u16; 1024];
    let mut ofn = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: hwnd,
        lpstrFilter: PCWSTR(filter.as_ptr()),
        lpstrFile: PWSTR(buffer.as_mut_ptr()),
        nMaxFile: buffer.len() as u32,
        lpstrInitialDir: PCWSTR(initial.as_ptr()),
        lpstrTitle: PCWSTR(title.as_ptr()),
        lpstrDefExt: w!("json"),
        Flags: if save {
            OFN_OVERWRITEPROMPT | OFN_PATHMUSTEXIST
        } else {
            OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST
        },
        ..Default::default()
    };
    let ok = unsafe {
        if save {
            GetSaveFileNameW(&mut ofn)
        } else {
            GetOpenFileNameW(&mut ofn)
        }
    };
    if !ok.as_bool() {
        return None;
    }
    let length = buffer.iter().position(|unit| *unit == 0).unwrap_or(0);
    if length == 0 {
        return None;
    }
    Some(PathBuf::from(String::from_utf16_lossy(&buffer[..length])))
}

fn set_data_status(hwnd: HWND, text: &str) {
    UI.with(|slot| {
        if let Ok(mut guard) = slot.try_borrow_mut() {
            if let Some(ui) = guard.as_mut() {
                ui.status_text = text.to_string();
                unsafe {
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
            }
        }
    });
}

fn rule_error_text(code: &str) -> &'static str {
    match code {
        "read_only" => "只读模式，无法修改应用例外",
        "rule_exe" => "请输入有效的可执行文件名",
        "rule_action" => "输入模式无效",
        "rule_limit" => "应用例外数量已达上限",
        "rule_missing" => "该应用没有例外规则",
        _ => "应用例外操作失败，请查看日志",
    }
}

fn data_error_text(code: &str) -> &'static str {
    match code {
        "read_only" => "只读模式，无法导入",
        "import_too_large" => "导入文件过大或条目过多",
        "import_invalid_json" => "文件不是有效的 QuickPhrase JSON",
        "import_schema" => "文件版本不受支持",
        "import_category_id" | "import_category_name" | "import_category_missing" => {
            "导入分类数据无效"
        }
        "import_tags" => "导入标签不合规（不能含逗号、单个不超 24 字、最多 10 个）",
        "import_snippet_id" => "导入短语 ID 无效",
        "duplicate_title" => "导入数据包含重复标题",
        "title_empty" | "title_too_long" | "content_empty" | "content_too_long" => {
            "导入短语内容不符合校验规则"
        }
        _ => "导入失败，请检查文件或日志",
    }
}

fn handle_data_action(hwnd: HWND, action: DataAction) {
    let (save, initial, title) = match action {
        DataAction::Export => (true, platform::data_dir(), "导出 QuickPhrase 数据"),
        DataAction::Import => (false, platform::data_dir(), "导入 QuickPhrase 数据"),
        DataAction::RestoreBackup => {
            let dir = storage::backups_dir().unwrap_or_else(platform::data_dir);
            (false, dir, "恢复 QuickPhrase 备份")
        }
    };
    let Some(path) = pick_data_file(hwnd, save, &initial, title) else {
        return;
    };

    match action {
        DataAction::Export => {
            let result = storage::export_json()
                .and_then(|json| std::fs::write(&path, json).map_err(|err| format!("write:{err}")));
            match result {
                Ok(()) => set_data_status(hwnd, &format!("已导出：{}", path.display())),
                Err(err) => {
                    logging::error(&format!("settings: export failed: {err}"));
                    set_data_status(hwnd, "导出失败，请检查路径或权限");
                }
            }
        }
        DataAction::Import => {
            let bytes = match storage::read_import_file(&path) {
                Ok(bytes) => bytes,
                Err(err) if err == "import_too_large" => {
                    set_data_status(hwnd, "导入文件过大或条目过多");
                    return;
                }
                Err(err) => {
                    logging::error(&format!("settings: import read failed: {err}"));
                    set_data_status(hwnd, "无法读取导入文件");
                    return;
                }
            };
            let choice = unsafe {
                MessageBoxW(
                    Some(hwnd),
                    w!("导入方式：\n\n是：合并到当前数据（按标题去重）\n否：替换当前全部数据\n取消：不导入"),
                    w!("导入 QuickPhrase 数据"),
                    MB_YESNOCANCEL | MB_ICONQUESTION,
                )
            };
            let mode = if choice == IDYES {
                storage::ImportMode::Merge
            } else if choice == IDNO {
                storage::ImportMode::Replace
            } else {
                return;
            };
            match storage::import_bytes(&bytes, mode) {
                Ok(summary) => set_data_status(
                    hwnd,
                    &format!(
                        "导入完成：分类 +{}，短语 +{}，跳过 {}；重开面板/管理窗口生效",
                        summary.categories_added, summary.snippets_added, summary.snippets_skipped
                    ),
                ),
                Err(err) => {
                    logging::error(&format!("settings: import failed: {err}"));
                    set_data_status(hwnd, data_error_text(&err));
                }
            }
        }
        DataAction::RestoreBackup => {
            let bytes = match storage::read_import_file(&path) {
                Ok(bytes) => bytes,
                Err(err) if err == "import_too_large" => {
                    set_data_status(hwnd, "备份文件过大，无法恢复");
                    return;
                }
                Err(err) => {
                    logging::error(&format!("settings: backup read failed: {err}"));
                    set_data_status(hwnd, "无法读取备份文件");
                    return;
                }
            };
            let answer = unsafe {
                MessageBoxW(
                    Some(hwnd),
                    w!("恢复备份会用备份内容替换当前数据，且无法撤销。继续吗？"),
                    w!("恢复 QuickPhrase 备份"),
                    MB_YESNO | MB_ICONWARNING,
                )
            };
            if answer != IDYES {
                return;
            }
            match storage::import_bytes(&bytes, storage::ImportMode::Replace) {
                Ok(_) => set_data_status(hwnd, "备份已恢复；重开面板/管理窗口生效"),
                Err(err) => {
                    logging::error(&format!("settings: restore failed: {err}"));
                    set_data_status(hwnd, data_error_text(&err));
                }
            }
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
        WM_ACTIVATE | WM_KILLFOCUS | WM_SETFOCUS => {
            if message == WM_ACTIVATE {
                let active = (wparam.0 & 0xffff) as u32 != WA_INACTIVE;
                UI.with(|slot| {
                    if let Ok(mut guard) = slot.try_borrow_mut() {
                        if let Some(ui) = guard.as_mut() {
                            ui.active = active;
                        }
                    }
                });
            }
            unsafe {
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
        }
        WM_SIZE => {
            let width = (lparam.0 & 0xffff) as i32;
            let height = ((lparam.0 >> 16) & 0xffff) as i32;
            UI.with(|slot| {
                if let Ok(mut guard) = slot.try_borrow_mut() {
                    if let Some(ui) = guard.as_mut() {
                        layout(ui, width, height);
                    }
                }
            });
            LRESULT(0)
        }
        WM_GETMINMAXINFO => {
            let info = &mut *(lparam.0 as *mut MINMAXINFO);
            UI.with(|slot| {
                let Ok(guard) = slot.try_borrow() else {
                    return;
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
                if let Ok(mut guard) = slot.try_borrow_mut() {
                    if let Some(ui) = guard.as_mut() {
                        let _ = DeleteObject(HGDIOBJ(ui.font.0));
                        ui.font = make_font(dpi);
                        ui.dpi = dpi;
                        for control in [
                            ui.hotkey_record,
                            ui.restore_clip,
                            ui.copy_dir,
                            ui.open_dir,
                            ui.export_data,
                            ui.import_data,
                            ui.restore_backup,
                            ui.sort_mode,
                            ui.apps_exe,
                            ui.apps_mode,
                            ui.apps_rules,
                            ui.apps_add,
                            ui.apps_remove,
                            ui.save,
                            ui.cancel,
                        ] {
                            let _ = SendMessageW(
                                control,
                                WM_SETFONT,
                                Some(WPARAM(ui.font.0 as usize)),
                                Some(LPARAM(1)),
                            );
                        }
                        layout(ui, width, height);
                    }
                }
            });
            let _ = SetWindowPos(
                hwnd,
                None,
                left,
                top,
                width,
                height,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
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
        WM_KEYDOWN
            if UI.with(|slot| {
                slot.try_borrow()
                    .ok()
                    .and_then(|guard| {
                        guard.as_ref().map(|ui| {
                            !ui.capturing
                                && ui.focus != Hover::None
                                && unsafe { GetFocus() } == ui.hwnd
                        })
                    })
                    .unwrap_or(false)
            }) =>
        {
            let mut ask_paste = false;
            let mut focus_after = None;
            UI.with(|slot| {
                let Ok(mut guard) = slot.try_borrow_mut() else {
                    return;
                };
                let Some(ui) = guard.as_mut() else {
                    return;
                };
                let vk = wparam.0 as u16;
                let repeated = lparam.0 & (1 << 30) != 0;
                if (vk == VK_SPACE.0 || vk == VK_RETURN.0) && !repeated {
                    ask_paste = activate_hover(ui, ui.focus);
                } else if vk == VK_ESCAPE.0 {
                    ui.focus = Hover::None;
                    focus_after = Some(hwnd);
                } else if vk == VK_LEFT.0 || vk == VK_UP.0 {
                    focus_after = handle_tab(ui, true);
                } else if vk == VK_RIGHT.0 || vk == VK_DOWN.0 {
                    focus_after = handle_tab(ui, false);
                }
            });
            if let Some(next) = focus_after {
                unsafe {
                    let _ = SetFocus(Some(next));
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
            }
            if ask_paste && confirm_paste_mode(hwnd) {
                UI.with(|slot| {
                    if let Ok(mut guard) = slot.try_borrow_mut() {
                        if let Some(ui) = guard.as_mut() {
                            ui.paste_mode = true;
                            relayout(ui);
                            set_status(ui, "未保存：粘贴模式会替换剪贴板");
                        }
                    }
                });
            }
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let x = (lparam.0 & 0xffff) as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xffff) as i16 as i32;
            let redraw = UI.with(|slot| {
                let Ok(mut guard) = slot.try_borrow_mut() else {
                    return false;
                };
                let Some(ui) = guard.as_mut() else {
                    return false;
                };
                if !ui.tracking_mouse {
                    ui.tracking_mouse = true;
                    let mut track = TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    let _ = TrackMouseEvent(&mut track);
                }
                let next = hit_hover(ui, x, y);
                if next != ui.hover {
                    ui.hover = next;
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
            let redraw = UI.with(|slot| {
                let Ok(mut guard) = slot.try_borrow_mut() else {
                    return false;
                };
                let Some(ui) = guard.as_mut() else {
                    return false;
                };
                ui.tracking_mouse = false;
                if ui.hover != Hover::None {
                    ui.hover = Hover::None;
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
        WM_LBUTTONDOWN => {
            let x = (lparam.0 & 0xffff) as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xffff) as i16 as i32;
            let mut ask_paste = false;
            let mut focus_parent = false;
            UI.with(|slot| {
                let Ok(mut guard) = slot.try_borrow_mut() else {
                    return;
                };
                let Some(ui) = guard.as_mut() else {
                    return;
                };
                let target = hit_hover(ui, x, y);
                if target != Hover::None {
                    ui.focus = target;
                    focus_parent = true;
                }
                ask_paste = activate_hover(ui, target);
            });
            if focus_parent {
                unsafe {
                    if GetFocus() != hwnd {
                        let _ = SetFocus(Some(hwnd));
                    }
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
            }
            if ask_paste && confirm_paste_mode(hwnd) {
                UI.with(|slot| {
                    if let Ok(mut guard) = slot.try_borrow_mut() {
                        if let Some(ui) = guard.as_mut() {
                            ui.paste_mode = true;
                            relayout(ui);
                            set_status(ui, "未保存：粘贴模式会替换剪贴板");
                        }
                    }
                });
            }
            LRESULT(0)
        }
        WM_COMMAND => {
            let id = (wparam.0 & 0xffff) as i32;
            let notification = ((wparam.0 >> 16) & 0xffff) as u32;
            let mut data_action = None;
            UI.with(|slot| {
                let Ok(mut guard) = slot.try_borrow_mut() else {
                    return;
                };
                let Some(ui) = guard.as_mut() else {
                    return;
                };
                match id {
                    ID_HOTKEY_RECORD if notification == BN_CLICKED => {
                        ui.focus = Hover::None;
                        ui.capturing = !ui.capturing;
                        set_button_text(
                            ui.hotkey_record,
                            if ui.capturing {
                                "录制中…"
                            } else {
                                "录制"
                            },
                        );
                        if ui.capturing {
                            let _ = SetFocus(Some(hwnd));
                            ui.status_text =
                                "请按下新的组合键（含 Ctrl/Alt/Shift/Win）".to_string();
                        } else {
                            ui.status_text = "已取消录制".to_string();
                        }
                        relayout(ui);
                    }
                    ID_SAVE => save(ui),
                    ID_CANCEL => {
                        load(ui);
                        relayout(ui);
                        set_status(ui, "已取消修改");
                    }
                    ID_RESTORE_CLIP if notification == BN_CLICKED => {
                        match crate::clipboard::last_snapshot() {
                            Some(snapshot) if snapshot.text.is_some() => {
                                if crate::clipboard::restore(&snapshot) {
                                    set_status(ui, "已恢复上次剪贴板内容");
                                } else {
                                    set_status(ui, "无法恢复：剪贴板内容已变化");
                                }
                            }
                            _ => set_status(ui, "没有可恢复的剪贴板内容"),
                        }
                    }
                    ID_COPY_DIR if notification == BN_CLICKED => {
                        let _ = crate::clipboard::set_text_with_snapshot(&ui.data_dir);
                        set_status(ui, "已复制本地数据目录路径");
                    }
                    ID_OPEN_DIR if notification == BN_CLICKED => {
                        let path = platform::wide(&ui.data_dir);
                        let _ = ShellExecuteW(
                            Some(hwnd),
                            w!("open"),
                            PCWSTR(path.as_ptr()),
                            PCWSTR::null(),
                            PCWSTR::null(),
                            SW_SHOWNORMAL,
                        );
                        set_status(ui, "已打开本地数据目录");
                    }
                    ID_EXPORT_DATA if notification == BN_CLICKED => {
                        data_action = Some(DataAction::Export)
                    }
                    ID_IMPORT_DATA if notification == BN_CLICKED => {
                        data_action = Some(DataAction::Import)
                    }
                    ID_RESTORE_BACKUP if notification == BN_CLICKED => {
                        data_action = Some(DataAction::RestoreBackup)
                    }
                    ID_APPS_ADD if notification == BN_CLICKED => {
                        let exe = control_text(ui.apps_exe);
                        let index = unsafe {
                            SendMessageW(
                                ui.apps_mode,
                                CB_GETCURSEL,
                                Some(WPARAM(0)),
                                Some(LPARAM(0)),
                            )
                        }
                        .0;
                        let action = if index == 1 { "paste" } else { "unicode" };
                        logging::info(&format!(
                            "settings: app rule add exe={exe:?} action={action}"
                        ));
                        match storage::set_rule(&exe, action) {
                            Ok(()) => {
                                set_status(ui, "应用例外已保存");
                                reload_apps_rules(ui);
                            }
                            Err(code) => {
                                logging::warn(&format!("settings: app rule add failed: {code}"));
                                set_status(ui, rule_error_text(&code));
                            }
                        }
                    }
                    ID_APPS_REMOVE if notification == BN_CLICKED => {
                        let exe = control_text(ui.apps_exe);
                        match storage::remove_rule(&exe) {
                            Ok(()) => {
                                set_status(ui, "应用例外已删除");
                                reload_apps_rules(ui);
                            }
                            Err(code) => {
                                logging::warn(&format!("settings: app rule remove failed: {code}"));
                                set_status(ui, rule_error_text(&code));
                            }
                        }
                    }
                    ID_SORT_MODE if notification == BN_CLICKED => {
                        ui.panel_sort = if ui.panel_sort == "most_used" {
                            "recent".to_string()
                        } else {
                            "most_used".to_string()
                        };
                        set_button_text(ui.sort_mode, sort_label(&ui.panel_sort));
                        set_status(ui, "面板排序已切换，保存后生效");
                        unsafe {
                            let _ = InvalidateRect(Some(hwnd), None, false);
                        }
                    }
                    _ if notification == BN_SETFOCUS || notification == BN_KILLFOCUS => unsafe {
                        let _ = InvalidateRect(Some(hwnd), None, false);
                    },
                    _ => {}
                }
            });
            if let Some(action) = data_action {
                handle_data_action(hwnd, action);
            }
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_CTLCOLOREDIT | WM_CTLCOLORSTATIC => {
            let hdc = HDC(wparam.0 as *mut core::ffi::c_void);
            let (heading, field, brush) = UI.with(|slot| match slot.try_borrow() {
                Ok(guard) => match guard.as_ref() {
                    Some(ui) => (ui.palette.heading, ui.palette.field, ui.field_brush),
                    None => {
                        let palette = theme::current();
                        (palette.heading, palette.field, HBRUSH::default())
                    }
                },
                Err(_) => {
                    let palette = theme::current();
                    (palette.heading, palette.field, HBRUSH::default())
                }
            });
            unsafe {
                SetBkMode(hdc, OPAQUE);
                SetBkColor(hdc, field);
                SetTextColor(hdc, heading);
            }
            LRESULT(brush.0 as isize)
        }
        WM_DRAWITEM => {
            let draw = &*(lparam.0 as *const DRAWITEMSTRUCT);
            UI.with(|slot| {
                let Ok(guard) = slot.try_borrow() else {
                    return;
                };
                let Some(ui) = guard.as_ref() else {
                    return;
                };
                match draw.CtlID as i32 {
                    ID_SAVE => ui::draw_button(
                        draw,
                        ui.palette,
                        ui::ButtonKind::Primary,
                        ui.dpi,
                        ui.palette.card_subtle,
                    ),
                    ID_CANCEL => ui::draw_button(
                        draw,
                        ui.palette,
                        ui::ButtonKind::Secondary,
                        ui.dpi,
                        ui.palette.card_subtle,
                    ),
                    ID_HOTKEY_RECORD => ui::draw_button(
                        draw,
                        ui.palette,
                        if ui.capturing {
                            ui::ButtonKind::Primary
                        } else {
                            ui::ButtonKind::Subtle
                        },
                        ui.dpi,
                        ui.palette.card,
                    ),
                    _ => ui::draw_button(
                        draw,
                        ui.palette,
                        ui::ButtonKind::Secondary,
                        ui.dpi,
                        ui.palette.card,
                    ),
                }
            });
            LRESULT(1)
        }
        WM_PAINT => {
            let mut paint = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut paint);
            let mut client = RECT::default();
            let _ = GetClientRect(hwnd, &mut client);
            UI.with(|slot| {
                if let Ok(guard) = slot.try_borrow() {
                    if let Some(ui) = guard.as_ref() {
                        paint_settings(ui, hdc, client);
                    }
                }
            });
            let _ = EndPaint(hwnd, &paint);
            LRESULT(0)
        }
        WM_CLOSE => {
            if QUIT_ON_CLOSE.load(Ordering::Relaxed) {
                let _ = DestroyWindow(hwnd);
            } else {
                let _ = ShowWindow(hwnd, SW_HIDE);
                theme::clear_preview();
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            UI.with(|slot| {
                if let Ok(mut guard) = slot.try_borrow_mut() {
                    if let Some(ui) = guard.take() {
                        let _ = DeleteObject(HGDIOBJ(ui.font.0));
                        let _ = DeleteObject(HGDIOBJ(ui.field_brush.0));
                    }
                }
            });
            if QUIT_ON_CLOSE.load(Ordering::Relaxed) {
                PostQuitMessage(0);
            }
            theme::clear_preview();
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}
