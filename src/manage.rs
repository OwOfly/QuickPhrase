//! QuickPhrase 2.0 three-pane management window (Direct2D + DirectWrite).

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DeleteObject, EndPaint, InvalidateRect, SetBkColor, SetBkMode,
    SetTextColor, HBRUSH, HDC, HFONT, HGDIOBJ, PAINTSTRUCT, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::{DRAWITEMSTRUCT, ODS_FOCUS, ODS_SELECTED, WM_MOUSELEAVE};
use windows::Win32::UI::HiDpi::{GetDpiForSystem, GetDpiForWindow};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetFocus, SetFocus, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT,
};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::d2d::{self, AlignH, AlignV, Icon, TextStyle};
use crate::search::{self, Doc};
use crate::storage::{self, Snippet};
use crate::{logging, platform, theme, ui};

const CLASS: PCWSTR = w!("QuickPhrase.ManageWindow");
const TITLE: PCWSTR = w!("QuickPhrase 短语管理");

const ID_LEFT: i32 = 200;
const ID_SEARCH: i32 = 201;
const ID_LIST: i32 = 202;
const ID_NEW: i32 = 203;
const ID_DELETE: i32 = 204;
const ID_TITLE: i32 = 205;
const ID_CONTENT: i32 = 206;
const ID_CATEGORY: i32 = 207;
const ID_PINNED: i32 = 208;
const ID_SAVE: i32 = 209;
const ID_CANCEL: i32 = 210;
const ID_CAT_NAME: i32 = 212;
const ID_CAT_ADD: i32 = 213;
const ID_CAT_DELETE: i32 = 214;
const ID_RESTORE: i32 = 215;

const MIN_WIDTH: i32 = 800;
const MIN_HEIGHT: i32 = 540;
const DEFAULT_WIDTH: i32 = 1040;
const DEFAULT_HEIGHT: i32 = 650;

const LEFT_WIDTH: i32 = 206;
const MIDDLE_WIDTH: i32 = 308;

#[allow(non_snake_case)]
unsafe fn SendMessageW(hwnd: HWND, message: u32, wparam: usize, lparam: isize) -> LRESULT {
    windows::Win32::UI::WindowsAndMessaging::SendMessageW(
        hwnd,
        message,
        Some(WPARAM(wparam)),
        Some(LPARAM(lparam)),
    )
}

#[derive(Clone, PartialEq)]
enum View {
    All,
    Category(String),
    Recycle,
}

#[derive(Clone)]
struct NavRow {
    label: String,
    count: usize,
    view: View,
    dot: Option<COLORREF>,
}

#[derive(Clone)]
struct PhraseRow {
    id: String,
    title: String,
    preview: String,
    category: String,
    pinned: bool,
}

struct Fonts {
    body: HFONT,
    title: HFONT,
}

#[derive(Default, Clone, Copy)]
struct Layout {
    left_pane: RECT,
    middle_pane: RECT,
    right_pane: RECT,
    cat_input_shell: RECT,
    search_shell: RECT,
    middle_header_bottom: i32,
    recycle_banner: Option<RECT>,
    editor_header: RECT,
    title_label: RECT,
    title_shell: RECT,
    content_label: RECT,
    content_shell: RECT,
    meta_cat_label: RECT,
    meta_pin_label: RECT,
    pin_card: RECT,
    pin_switch: RECT,
    footer: RECT,
    status_rect: RECT,
}

struct Manage {
    hwnd: HWND,
    left: HWND,
    cat_name: HWND,
    cat_add: HWND,
    cat_delete: HWND,
    search: HWND,
    list: HWND,
    new_button: HWND,
    delete_button: HWND,
    restore_button: HWND,
    title_edit: HWND,
    content_edit: HWND,
    category: HWND,
    pinned: HWND,
    save_button: HWND,
    cancel_button: HWND,
    layout: Layout,
    fonts: Fonts,
    palette: theme::Palette,
    sidebar_brush: HBRUSH,
    card_brush: HBRUSH,
    field_brush: HBRUSH,
    error_field: Option<i32>,
    error_message: Option<String>,
    status_text: String,
    dpi: u32,
    view: View,
    editing: Option<String>,
    editing_deleted: bool,
    pinned_state: bool,
    draft_category: Option<String>,
    pin_hovered: bool,
    tracking_mouse: bool,
    selected: Option<usize>,
    dirty: bool,
    loading: bool,
    nav_rows: Vec<NavRow>,
    phrase_rows: Vec<PhraseRow>,
}

thread_local! {
    static MANAGE: RefCell<Option<Manage>> = const { RefCell::new(None) };
}

static QUIT_ON_CLOSE: AtomicBool = AtomicBool::new(false);

fn scaled(value: i32, dpi: u32) -> i32 {
    (value * dpi as i32 + 48) / 96
}

fn wide_text(control: HWND) -> String {
    let length = unsafe { GetWindowTextLengthW(control) };
    if length <= 0 {
        return String::new();
    }
    let mut buffer = vec![0u16; length as usize + 1];
    let copied = unsafe { GetWindowTextW(control, &mut buffer) };
    String::from_utf16_lossy(&buffer[..copied.max(0) as usize])
}

fn set_text(control: HWND, text: &str) {
    let wide = platform::wide(text);
    unsafe {
        let _ = SetWindowTextW(control, PCWSTR(wide.as_ptr()));
    }
}

fn status_message(manage: &mut Manage, text: &str) {
    manage.status_text = text.to_string();
    unsafe {
        let _ = InvalidateRect(Some(manage.hwnd), Some(&manage.layout.footer), false);
    }
}

fn error_text(code: &str) -> &'static str {
    match code {
        "title_empty" => "标题不能为空",
        "title_too_long" => "标题不能超过 80 个字符",
        "content_empty" => "正文不能为空",
        "content_too_long" => "正文不能超过 10,000 个字符",
        "duplicate_title" => "已存在同名短语",
        "category_missing" => "所选分类不存在",
        "category_empty" => "分类名不能为空",
        "category_too_long" => "分类名不能超过 40 个字符",
        "category_duplicate" => "已存在同名分类",
        "read_only" => "存储为只读，无法保存",
        "snippet_missing" => "短语不存在",
        _ => "操作失败",
    }
}

fn data() -> Option<storage::DataFile> {
    storage::data_clone()
}

fn active_rows(category: Option<&str>, query: &str, recycle: bool) -> Vec<PhraseRow> {
    let Some(data) = data() else {
        return Vec::new();
    };
    let category_names: Vec<(String, String)> = data
        .categories
        .iter()
        .map(|category| (category.id.clone(), category.name.clone()))
        .collect();
    let name_of = |id: Option<&str>| -> String {
        id.and_then(|id| {
            category_names
                .iter()
                .find(|(value, _)| value == id)
                .map(|(_, name)| name.clone())
        })
        .unwrap_or_default()
    };

    let pool: Vec<&Snippet> = data
        .snippets
        .iter()
        .filter(|snippet| {
            (snippet.deleted_at.is_some()) == recycle
                && match category {
                    Some(id) => snippet.category_id.as_deref() == Some(id),
                    None => true,
                }
        })
        .collect();

    let docs: Vec<Doc> = pool
        .iter()
        .map(|snippet| Doc {
            id: &snippet.id,
            title: &snippet.title,
            content: &snippet.content,
            blob: &snippet.search_blob,
            pinned: snippet.pinned,
            last_used_at: snippet.last_used_at.as_deref(),
            updated_at: &snippet.updated_at,
            sort_order: snippet.sort_order,
        })
        .collect();
    let hits = search::rank(&docs, query, usize::MAX);
    hits.into_iter()
        .map(|hit| {
            let snippet = pool[hit.index];
            let preview = snippet.content.replace("\r\n", " ↵ ").replace('\n', " ↵ ");
            PhraseRow {
                id: snippet.id.clone(),
                title: snippet.title.clone(),
                preview,
                category: name_of(snippet.category_id.as_deref()),
                pinned: snippet.pinned,
            }
        })
        .collect()
}

fn reload_left(manage: &mut Manage) {
    let Some(data) = data() else {
        return;
    };
    unsafe {
        let _ = SendMessageW(manage.left, LB_RESETCONTENT, 0, 0);
    }
    let active: Vec<&Snippet> = data
        .snippets
        .iter()
        .filter(|snippet| snippet.deleted_at.is_none())
        .collect();
    let deleted = data
        .snippets
        .iter()
        .filter(|snippet| snippet.deleted_at.is_some())
        .count();

    let mut rows = vec![NavRow {
        label: "全部短语".to_string(),
        count: active.len(),
        view: View::All,
        dot: None,
    }];
    for category in &data.categories {
        let count = active
            .iter()
            .filter(|snippet| snippet.category_id.as_deref() == Some(category.id.as_str()))
            .count();
        rows.push(NavRow {
            label: category.name.clone(),
            count,
            view: View::Category(category.id.clone()),
            dot: Some(theme::category_dot_for_name(&category.name)),
        });
    }
    rows.push(NavRow {
        label: "回收站".to_string(),
        count: deleted,
        view: View::Recycle,
        dot: None,
    });

    for row in &rows {
        let wide = platform::wide(&row.label);
        unsafe {
            let _ = SendMessageW(manage.left, LB_ADDSTRING, 0, wide.as_ptr() as isize);
        }
    }
    let index = rows
        .iter()
        .position(|row| row.view == manage.view)
        .unwrap_or(0);
    manage.nav_rows = rows;
    unsafe {
        let _ = SendMessageW(manage.left, LB_SETCURSEL, index, 0);
        let show_del_cat = matches!(manage.view, View::Category(_));
        let _ = ShowWindow(
            manage.cat_delete,
            if show_del_cat { SW_SHOW } else { SW_HIDE },
        );
    }
}

fn reload_categories(manage: &mut Manage) {
    let Some(data) = data() else {
        return;
    };
    unsafe {
        let _ = SendMessageW(manage.category, CB_RESETCONTENT, 0, 0);
    }
    let uncategorized = platform::wide("未分类");
    unsafe {
        let _ = SendMessageW(
            manage.category,
            CB_ADDSTRING,
            0,
            uncategorized.as_ptr() as isize,
        );
    }
    for category in &data.categories {
        let wide = platform::wide(&category.name);
        unsafe {
            let _ = SendMessageW(manage.category, CB_ADDSTRING, 0, wide.as_ptr() as isize);
        }
    }
    // Restore by stable id, not by the old numeric index: an import can
    // reorder or remove categories while an editor draft is still open.
    let index = manage
        .draft_category
        .as_deref()
        .and_then(|id| {
            data.categories
                .iter()
                .position(|category| category.id == id)
        })
        .map(|position| position + 1);
    if index.is_none() && manage.draft_category.is_some() {
        manage.draft_category = None;
    }
    unsafe {
        let _ = SendMessageW(manage.category, CB_SETCURSEL, index.unwrap_or(0), 0);
    }
}

fn reload_list(manage: &mut Manage) {
    let query = wide_text(manage.search);
    let (category, recycle) = match &manage.view {
        View::All => (None, false),
        View::Category(id) => (Some(id.as_str()), false),
        View::Recycle => (None, true),
    };
    let rows = active_rows(category, &query, recycle);
    unsafe {
        let _ = SendMessageW(manage.list, LB_RESETCONTENT, 0, 0);
    }
    for row in &rows {
        let wide = platform::wide(&row.title);
        unsafe {
            let _ = SendMessageW(manage.list, LB_ADDSTRING, 0, wide.as_ptr() as isize);
        }
    }
    manage.phrase_rows = rows;

    if let Some(editing) = manage.editing.as_deref() {
        if let Some(position) = manage.phrase_rows.iter().position(|r| r.id == editing) {
            manage.selected = Some(position);
            unsafe {
                let _ = SendMessageW(manage.list, LB_SETCURSEL, position, 0);
            }
        }
    }
    if manage.phrase_rows.is_empty() {
        manage.selected = None;
    }
    unsafe {
        let _ = InvalidateRect(Some(manage.hwnd), None, false);
    }
}

fn load_editor(manage: &mut Manage, id: Option<&str>) {
    manage.loading = true;
    manage.error_field = None;
    manage.error_message = None;

    let snippet = id.and_then(|id| {
        data().and_then(|data| data.snippets.into_iter().find(|snippet| snippet.id == id))
    });
    let data = data();
    match snippet {
        Some(snippet) => {
            set_text(manage.title_edit, &snippet.title);
            set_text(manage.content_edit, &snippet.content);
            let index = data
                .as_ref()
                .and_then(|data| {
                    snippet.category_id.as_ref().and_then(|id| {
                        data.categories
                            .iter()
                            .position(|category| &category.id == id)
                            .map(|position| position + 1)
                    })
                })
                .unwrap_or(0);
            unsafe {
                let _ = SendMessageW(manage.category, CB_SETCURSEL, index, 0);
            }
            manage.pinned_state = snippet.pinned;
            manage.draft_category = snippet.category_id.clone();
            unsafe {
                let _ = InvalidateRect(Some(manage.pinned), None, false);
                let _ = InvalidateRect(Some(manage.hwnd), None, false);
            }
            manage.editing = Some(snippet.id);
            manage.editing_deleted = snippet.deleted_at.is_some();
            manage.dirty = false;
            manage.status_text = "已加载短语".to_string();
            set_text(
                manage.delete_button,
                if manage.editing_deleted {
                    "彻底删除"
                } else {
                    "移入回收站"
                },
            );
            unsafe {
                let _ = ShowWindow(manage.delete_button, SW_SHOW);
                let _ = ShowWindow(
                    manage.restore_button,
                    if manage.editing_deleted {
                        SW_SHOW
                    } else {
                        SW_HIDE
                    },
                );
            }
        }
        None => {
            set_text(manage.title_edit, "");
            set_text(manage.content_edit, "");
            let default_cat = match &manage.view {
                View::Category(cat_id) => data
                    .as_ref()
                    .and_then(|d| d.categories.iter().position(|c| &c.id == cat_id))
                    .map(|p| p + 1)
                    .unwrap_or(0),
                _ => 0,
            };
            unsafe {
                let _ = SendMessageW(manage.category, CB_SETCURSEL, default_cat, 0);
                let _ = ShowWindow(manage.delete_button, SW_HIDE);
                let _ = ShowWindow(manage.restore_button, SW_HIDE);
            }
            manage.pinned_state = false;
            // A new snippet in a category view starts in that category. Keep
            // the id in sync because save_editor now uses the stable id, not
            // the combo index.
            manage.draft_category = if default_cat > 0 {
                data.as_ref()
                    .and_then(|d| d.categories.get(default_cat - 1))
                    .map(|category| category.id.clone())
            } else {
                None
            };
            manage.editing = None;
            manage.editing_deleted = false;
            manage.dirty = false;
            manage.status_text = "新建短语".to_string();
        }
    }
    manage.loading = false;
    relayout(manage);
    unsafe {
        let _ = InvalidateRect(Some(manage.hwnd), None, false);
    }
}

fn confirm_discard(owner: HWND) -> bool {
    let text = platform::wide("有未保存的修改，确定放弃吗？");
    let caption = platform::wide("QuickPhrase");
    unsafe {
        MessageBoxW(
            Some(owner),
            PCWSTR(text.as_ptr()),
            PCWSTR(caption.as_ptr()),
            MB_YESNO | MB_ICONWARNING,
        ) == IDYES
    }
}

fn current_category(manage: &Manage) -> Option<String> {
    let index = unsafe { SendMessageW(manage.category, CB_GETCURSEL, 0, 0) }.0;
    if index <= 0 {
        return None;
    }
    let data = data()?;
    data.categories
        .get(index as usize - 1)
        .map(|category| category.id.clone())
}

fn save_editor(manage: &mut Manage) {
    let title = wide_text(manage.title_edit);
    let content = wide_text(manage.content_edit);
    let category = manage.draft_category.clone();
    let pinned = manage.pinned_state;

    let result = storage::with_store(|store| match &manage.editing {
        Some(id) => store
            .update_snippet(id, &title, &content, category.as_deref(), pinned)
            .map(|_| id.clone()),
        None => store.add_snippet(&title, &content, category.as_deref(), pinned),
    });

    match result {
        Some(Ok(id)) => {
            manage.editing = Some(id.clone());
            manage.dirty = false;
            manage.error_field = None;
            manage.error_message = None;
            logging::info("manage: saved snippet");
            reload_left(manage);
            reload_categories(manage);
            reload_list(manage);
            load_editor(manage, Some(&id));
            status_message(manage, "已原子写入本地存储");
        }
        Some(Err(code)) => {
            let message = error_text(&code);
            manage.error_field = match code.as_str() {
                "title_empty" | "title_too_long" | "duplicate_title" => Some(ID_TITLE),
                "content_empty" | "content_too_long" => Some(ID_CONTENT),
                "category_missing" => Some(ID_CATEGORY),
                _ => None,
            };
            manage.error_message = Some(message.to_string());
            status_message(manage, message);
            unsafe {
                let _ = InvalidateRect(Some(manage.hwnd), None, false);
            }
            logging::warn(&format!("manage: save rejected ({code})"));
        }
        None => status_message(manage, "存储未初始化"),
    }
}

fn delete_selected(manage: &mut Manage) {
    let Some(id) = manage.editing.clone() else {
        status_message(manage, "请先选择短语");
        return;
    };
    let recycle = manage.view == View::Recycle || manage.editing_deleted;
    let question = if recycle {
        "彻底删除后无法恢复，继续吗？"
    } else {
        "移入回收站？"
    };
    let text = platform::wide(question);
    let caption = platform::wide("QuickPhrase");
    let answer = unsafe {
        MessageBoxW(
            Some(manage.hwnd),
            PCWSTR(text.as_ptr()),
            PCWSTR(caption.as_ptr()),
            MB_YESNO | MB_ICONQUESTION,
        )
    };
    if answer != IDYES {
        return;
    }
    let result = storage::with_store(|store| {
        if recycle {
            store.purge(&id)
        } else {
            store.soft_delete(&id)
        }
    });
    match result {
        Some(Ok(())) => {
            manage.editing = None;
            manage.dirty = false;
            reload_left(manage);
            reload_list(manage);
            let next_id = manage.phrase_rows.first().map(|r| r.id.clone());
            load_editor(manage, next_id.as_deref());
            status_message(
                manage,
                if recycle {
                    "已彻底删除"
                } else {
                    "已移入回收站"
                },
            );
        }
        Some(Err(code)) => status_message(manage, error_text(&code)),
        None => status_message(manage, "存储未初始化"),
    }
}

fn draw_nav_row(manage: &Manage, draw: &DRAWITEMSTRUCT) {
    if draw.itemID == u32::MAX {
        return;
    }
    let Some(row) = manage.nav_rows.get(draw.itemID as usize) else {
        return;
    };
    let dpi = manage.dpi;
    let palette = manage.palette;
    let rect = draw.rcItem;
    let selected = draw.itemState.0 & ODS_SELECTED.0 != 0;
    let is_recycle = row.view == View::Recycle;

    let right_pad = if ui::listbox_scrollable(draw.hwndItem) {
        scaled(13, dpi)
    } else {
        scaled(6, dpi)
    };
    d2d::paint_buffered(draw.hDC, rect, |c| {
        c.fill_rect(&rect, palette.sidebar);

        let pill = RECT {
            left: rect.left + scaled(6, dpi),
            top: rect.top + scaled(2, dpi),
            right: (rect.right - right_pad).max(rect.left + scaled(20, dpi)),
            bottom: rect.bottom - scaled(2, dpi),
        };
        if selected {
            let fill = if is_recycle {
                palette.danger_soft
            } else {
                palette.selection
            };
            c.fill_round(&pill, d2d::scaled_f(9.0, dpi), fill);
        }

        let icon_cx = (pill.left + scaled(18, dpi)) as f32;
        let cy = (pill.top + pill.bottom) as f32 * 0.5;
        let text_color = if selected {
            if is_recycle {
                palette.danger
            } else {
                palette.primary_text
            }
        } else {
            palette.secondary
        };

        if let Some(dot) = row.dot {
            c.fill_circle(icon_cx, cy, d2d::scaled_f(4.0, dpi), dot);
        } else if is_recycle {
            c.icon(
                Icon::Trash,
                icon_cx,
                cy,
                d2d::scaled_f(13.0, dpi),
                text_color,
            );
        } else {
            c.icon(
                Icon::Grid,
                icon_cx,
                cy,
                d2d::scaled_f(13.0, dpi),
                text_color,
            );
        }

        // Count pill on right
        let count_str = row.count.to_string();
        let (cw, _) = c.measure_style(&count_str, TextStyle::Badge, dpi);
        let badge_w = (cw.ceil() as i32 + scaled(12, dpi)).max(scaled(22, dpi));
        let badge_h = scaled(20, dpi);
        let badge = RECT {
            left: pill.right - scaled(8, dpi) - badge_w,
            top: pill.top + ((pill.bottom - pill.top) - badge_h) / 2,
            right: pill.right - scaled(8, dpi),
            bottom: pill.top + ((pill.bottom - pill.top) + badge_h) / 2,
        };
        let badge_bg = if selected {
            if is_recycle {
                palette.danger_soft
            } else {
                palette.primary_soft
            }
        } else {
            palette.field
        };
        c.fill_round(&badge, d2d::scaled_f(6.0, dpi), badge_bg);
        c.draw_text(
            &count_str,
            &badge,
            TextStyle::Badge,
            dpi,
            text_color,
            AlignH::Center,
            AlignV::Center,
        );

        let label_rect = RECT {
            left: pill.left + scaled(32, dpi),
            top: pill.top,
            right: (badge.left - scaled(6, dpi)).max(pill.left + scaled(40, dpi)),
            bottom: pill.bottom,
        };
        c.draw_text(
            &row.label,
            &label_rect,
            if selected {
                TextStyle::Heading
            } else {
                TextStyle::Body
            },
            dpi,
            text_color,
            AlignH::Left,
            AlignV::Center,
        );
    });
}

fn draw_pin_switch(manage: &Manage, draw: &DRAWITEMSTRUCT) {
    let rect = draw.rcItem;
    let focused = draw.itemState.0 & ODS_FOCUS.0 != 0;
    let hovered = ui::hovered(draw.hwndItem);
    let inset = scaled(2, manage.dpi);
    let track = RECT {
        left: rect.left + inset,
        top: rect.top + inset,
        right: rect.right - inset,
        bottom: rect.bottom - inset,
    };
    let background = if hovered || manage.pin_hovered {
        manage.palette.hover
    } else {
        manage.palette.card_subtle
    };
    d2d::paint_buffered(draw.hDC, rect, |c| {
        c.fill_rect(&rect, background);
        c.switch(
            &track,
            manage.pinned_state,
            hovered,
            focused,
            manage.palette,
            manage.dpi,
        );
    });
}

fn draw_phrase_row(manage: &Manage, draw: &DRAWITEMSTRUCT) {
    if draw.itemID == u32::MAX {
        return;
    }
    let Some(row) = manage.phrase_rows.get(draw.itemID as usize) else {
        return;
    };
    let dpi = manage.dpi;
    let palette = manage.palette;
    let rect = draw.rcItem;
    let selected = draw.itemState.0 & ODS_SELECTED.0 != 0;

    let right_pad = if ui::listbox_scrollable(draw.hwndItem) {
        scaled(13, dpi)
    } else {
        scaled(6, dpi)
    };
    d2d::paint_buffered(draw.hDC, rect, |c| {
        c.fill_rect(&rect, palette.card);

        let card = RECT {
            left: rect.left + scaled(6, dpi),
            top: rect.top + scaled(3, dpi),
            right: (rect.right - right_pad).max(rect.left + scaled(20, dpi)),
            bottom: rect.bottom - scaled(3, dpi),
        };
        if selected {
            c.fill_stroke_round(
                &card,
                d2d::scaled_f(10.0, dpi),
                palette.selection,
                palette.selection_border,
                1.0,
            );
            let bar = RECT {
                left: card.left + scaled(3, dpi),
                top: card.top + scaled(10, dpi),
                right: card.left + scaled(6, dpi),
                bottom: card.bottom - scaled(10, dpi),
            };
            c.fill_round(&bar, d2d::scaled_f(2.0, dpi), palette.primary);
        } else {
            // Subtle bottom divider on unselected items
            c.line(
                (card.left + scaled(10, dpi)) as f32,
                rect.bottom as f32 - 0.5,
                (card.right - scaled(10, dpi)) as f32,
                rect.bottom as f32 - 0.5,
                palette.divider,
                1.0,
            );
        }

        let mut title_left = card.left + scaled(14, dpi);
        let top_row_y = card.top + scaled(8, dpi);
        if row.pinned {
            c.icon(
                Icon::Pin,
                (title_left + scaled(6, dpi)) as f32,
                (top_row_y + scaled(9, dpi)) as f32,
                d2d::scaled_f(11.5, dpi),
                palette.primary,
            );
            title_left += scaled(16, dpi);
        }

        let title_rect = RECT {
            left: title_left,
            top: top_row_y,
            right: card.right - scaled(12, dpi),
            bottom: top_row_y + scaled(21, dpi),
        };
        c.draw_text(
            &row.title,
            &title_rect,
            TextStyle::Heading,
            dpi,
            if selected {
                palette.primary_text
            } else {
                palette.heading
            },
            AlignH::Left,
            AlignV::Center,
        );

        let preview_rect = RECT {
            left: card.left + scaled(14, dpi),
            top: title_rect.bottom + scaled(2, dpi),
            right: card.right - scaled(12, dpi),
            bottom: title_rect.bottom + scaled(21, dpi),
        };
        c.draw_text(
            &row.preview,
            &preview_rect,
            TextStyle::Caption,
            dpi,
            palette.secondary,
            AlignH::Left,
            AlignV::Center,
        );

        let cat_rect = RECT {
            left: card.left
                + if row.category.is_empty() {
                    scaled(14, dpi)
                } else {
                    scaled(25, dpi)
                },
            top: preview_rect.bottom + scaled(2, dpi),
            right: card.right - scaled(12, dpi),
            bottom: card.bottom - scaled(5, dpi),
        };
        let dot_color = theme::category_dot_for_name(&row.category);
        let cat_label = if row.category.is_empty() {
            "未分类"
        } else {
            row.category.as_str()
        };
        if !row.category.is_empty() {
            c.fill_circle(
                (card.left + scaled(18, dpi)) as f32,
                (cat_rect.top + cat_rect.bottom) as f32 * 0.5,
                d2d::scaled_f(3.0, dpi),
                dot_color,
            );
        }
        c.draw_text(
            cat_label,
            &cat_rect,
            TextStyle::Badge,
            dpi,
            palette.muted,
            AlignH::Left,
            AlignV::Center,
        );
    });
}

fn relayout(manage: &mut Manage) {
    let mut rect = RECT::default();
    unsafe {
        let _ = GetClientRect(manage.hwnd, &mut rect);
    }
    layout(manage, rect.right, rect.bottom);
}

fn layout(manage: &mut Manage, width: i32, height: i32) {
    if width <= 0 || height <= 0 {
        return;
    }
    let dpi = manage.dpi;
    let left_w = scaled(LEFT_WIDTH, dpi);
    let mid_w =
        scaled(MIDDLE_WIDTH, dpi).min((width - left_w - scaled(320, dpi)).max(scaled(220, dpi)));

    let left_pane = RECT {
        left: 0,
        top: 0,
        right: left_w,
        bottom: height,
    };
    let middle_pane = RECT {
        left: left_w,
        top: 0,
        right: left_w + mid_w,
        bottom: height,
    };
    let right_pane = RECT {
        left: left_w + mid_w,
        top: 0,
        right: width,
        bottom: height,
    };

    // --- Left Sidebar ---
    let pad = scaled(12, dpi);
    let left_header_h = scaled(40, dpi);
    let row_h = scaled(32, dpi);
    let show_cat_del = matches!(manage.view, View::Category(_));
    let cat_del_top = height - pad - row_h;
    let cat_add_top = if show_cat_del {
        cat_del_top - scaled(8, dpi) - row_h
    } else {
        height - pad - row_h
    };
    let add_btn_w = scaled(54, dpi);
    let cat_input_shell = RECT {
        left: left_pane.left + pad,
        top: cat_add_top,
        right: (left_pane.right - pad - add_btn_w - scaled(6, dpi)).max(left_pane.left + pad + 20),
        bottom: cat_add_top + row_h,
    };
    let cat_edit_rect = ui::edit_centered_rect(
        cat_input_shell,
        manage.fonts.body,
        scaled(9, dpi),
        scaled(8, dpi),
    );

    unsafe {
        let _ = MoveWindow(
            manage.left,
            left_pane.left + scaled(4, dpi),
            left_header_h,
            (left_w - scaled(8, dpi)).max(1),
            (cat_add_top - scaled(10, dpi) - left_header_h).max(1),
            true,
        );
        let _ = MoveWindow(
            manage.cat_name,
            cat_edit_rect.left,
            cat_edit_rect.top,
            (cat_edit_rect.right - cat_edit_rect.left).max(1),
            (cat_edit_rect.bottom - cat_edit_rect.top).max(1),
            true,
        );
        let _ = MoveWindow(
            manage.cat_add,
            left_pane.right - pad - add_btn_w,
            cat_add_top,
            add_btn_w,
            row_h,
            true,
        );
        let _ = MoveWindow(
            manage.cat_delete,
            left_pane.left + pad,
            cat_del_top,
            (left_w - pad * 2).max(1),
            row_h,
            true,
        );
    }

    // --- Middle Pane ---
    let mid_inner_l = middle_pane.left + pad;
    let mid_inner_r = middle_pane.right - pad;
    let new_btn_w = scaled(70, dpi);
    let search_h = scaled(34, dpi);
    let search_shell = RECT {
        left: mid_inner_l,
        top: pad,
        right: (mid_inner_r - new_btn_w - scaled(8, dpi)).max(mid_inner_l + 40),
        bottom: pad + search_h,
    };
    let search_edit_rect = ui::edit_centered_rect(
        search_shell,
        manage.fonts.body,
        scaled(30, dpi),
        scaled(8, dpi),
    );
    let middle_header_bottom = search_shell.bottom + scaled(30, dpi);

    unsafe {
        let _ = MoveWindow(
            manage.search,
            search_edit_rect.left,
            search_edit_rect.top,
            (search_edit_rect.right - search_edit_rect.left).max(1),
            (search_edit_rect.bottom - search_edit_rect.top).max(1),
            true,
        );
        let _ = MoveWindow(
            manage.new_button,
            mid_inner_r - new_btn_w,
            pad,
            new_btn_w,
            search_h,
            true,
        );
        let _ = MoveWindow(
            manage.list,
            middle_pane.left + scaled(4, dpi),
            middle_header_bottom + scaled(4, dpi),
            (mid_w - scaled(8, dpi)).max(1),
            (height - middle_header_bottom - scaled(10, dpi)).max(1),
            true,
        );
    }

    // --- Right Editor Pane ---
    let rpad = scaled(22, dpi);
    let rl = right_pane.left + rpad;
    let rr = (right_pane.right - rpad).max(rl + 40);
    let rw = rr - rl;

    let mut ry = 0;
    let recycle_banner = if manage.editing_deleted {
        let bh = scaled(40, dpi);
        let b = RECT {
            left: right_pane.left + 1,
            top: 0,
            right: right_pane.right,
            bottom: bh,
        };
        let rb_w = scaled(84, dpi);
        let rb_h = scaled(26, dpi);
        unsafe {
            let _ = MoveWindow(
                manage.restore_button,
                rr - rb_w,
                (bh - rb_h) / 2,
                rb_w,
                rb_h,
                true,
            );
        }
        ry = bh;
        Some(b)
    } else {
        None
    };

    let header_h = scaled(50, dpi);
    let editor_header = RECT {
        left: right_pane.left + 1,
        top: ry,
        right: right_pane.right,
        bottom: ry + header_h,
    };
    let del_w = scaled(96, dpi);
    let del_h = scaled(30, dpi);
    unsafe {
        let _ = MoveWindow(
            manage.delete_button,
            rr - del_w,
            editor_header.top + (header_h - del_h) / 2,
            del_w,
            del_h,
            true,
        );
    }

    let footer_h = scaled(52, dpi);
    let footer = RECT {
        left: right_pane.left + 1,
        top: (height - footer_h).max(editor_header.bottom + 80),
        right: right_pane.right,
        bottom: height,
    };
    let save_w = scaled(92, dpi);
    let cancel_w = scaled(72, dpi);
    let btn_h = scaled(32, dpi);
    let btn_y = footer.top + (footer_h - btn_h) / 2;
    unsafe {
        let _ = MoveWindow(manage.save_button, rr - save_w, btn_y, save_w, btn_h, true);
        let _ = MoveWindow(
            manage.cancel_button,
            rr - save_w - scaled(8, dpi) - cancel_w,
            btn_y,
            cancel_w,
            btn_h,
            true,
        );
    }
    let status_rect = RECT {
        left: rl,
        top: footer.top,
        right: (rr - save_w - cancel_w - scaled(20, dpi)).max(rl + 20),
        bottom: footer.bottom,
    };

    // Title field
    let title_label = RECT {
        left: rl,
        top: editor_header.bottom + scaled(14, dpi),
        right: rr,
        bottom: editor_header.bottom + scaled(32, dpi),
    };
    let title_shell = RECT {
        left: rl,
        top: title_label.bottom + scaled(6, dpi),
        right: rr,
        bottom: title_label.bottom + scaled(6, dpi) + scaled(38, dpi),
    };
    let title_inner = ui::edit_centered_rect(
        title_shell,
        manage.fonts.title,
        scaled(12, dpi),
        scaled(12, dpi),
    );
    unsafe {
        let _ = MoveWindow(
            manage.title_edit,
            title_inner.left,
            title_inner.top,
            (title_inner.right - title_inner.left).max(1),
            (title_inner.bottom - title_inner.top).max(1),
            true,
        );
    }

    // Metadata row anchored above footer
    let meta_h = scaled(38, dpi);
    let meta_top = footer.top - scaled(18, dpi) - meta_h;
    let meta_label_top = meta_top - scaled(22, dpi);
    let col_gap = scaled(14, dpi);
    let half_w = ((rw - col_gap) / 2).max(scaled(110, dpi));

    let meta_cat_label = RECT {
        left: rl,
        top: meta_label_top,
        right: rl + half_w,
        bottom: meta_top - scaled(4, dpi),
    };
    let meta_pin_label = RECT {
        left: rl + half_w + col_gap,
        top: meta_label_top,
        right: rr,
        bottom: meta_top - scaled(4, dpi),
    };
    let pin_card = RECT {
        left: rl + half_w + col_gap,
        top: meta_top,
        right: rr,
        bottom: meta_top + meta_h,
    };
    let sw_w = scaled(38, dpi);
    let sw_h = scaled(22, dpi);
    let pin_switch = RECT {
        left: pin_card.right - scaled(12, dpi) - sw_w,
        top: pin_card.top + (meta_h - sw_h) / 2,
        right: pin_card.right - scaled(12, dpi),
        bottom: pin_card.top + (meta_h + sw_h) / 2,
    };

    unsafe {
        let _ = MoveWindow(
            manage.category,
            rl,
            meta_top,
            half_w,
            ui::combo_height(manage.category, dpi),
            true,
        );
        let _ = MoveWindow(
            manage.pinned,
            pin_switch.left,
            pin_switch.top,
            (pin_switch.right - pin_switch.left).max(1),
            (pin_switch.bottom - pin_switch.top).max(1),
            true,
        );
    }

    // Content field fills space between title and metadata
    let content_label = RECT {
        left: rl,
        top: title_shell.bottom + scaled(14, dpi),
        right: rr,
        bottom: title_shell.bottom + scaled(32, dpi),
    };
    let content_shell = RECT {
        left: rl,
        top: content_label.bottom + scaled(6, dpi),
        right: rr,
        bottom: (meta_label_top - scaled(14, dpi)).max(content_label.bottom + scaled(60, dpi)),
    };
    let cpad_x = scaled(12, dpi);
    let cpad_y = scaled(10, dpi);
    unsafe {
        let _ = MoveWindow(
            manage.content_edit,
            content_shell.left + cpad_x,
            content_shell.top + cpad_y,
            (content_shell.right - content_shell.left - cpad_x * 2).max(1),
            (content_shell.bottom - content_shell.top - cpad_y * 2).max(1),
            true,
        );
    }

    manage.layout = Layout {
        left_pane,
        middle_pane,
        right_pane,
        cat_input_shell,
        search_shell,
        middle_header_bottom,
        recycle_banner,
        editor_header,
        title_label,
        title_shell,
        content_label,
        content_shell,
        meta_cat_label,
        meta_pin_label,
        pin_card,
        pin_switch,
        footer,
        status_rect,
    };
}

fn paint_manage(manage: &Manage, hdc: HDC, client: RECT) {
    let dpi = manage.dpi;
    let palette = manage.palette;
    let l = manage.layout;
    let focus = unsafe { GetFocus() };

    let title_text = wide_text(manage.title_edit);
    let content_text = wide_text(manage.content_edit);
    let title_chars = title_text.chars().count();
    let content_chars = content_text.chars().count();
    let content_lines = if content_text.is_empty() {
        1
    } else {
        content_text.lines().count().max(1)
    };
    let active_total = manage.nav_rows.first().map(|r| r.count).unwrap_or(0);
    let mid_title = match &manage.view {
        View::All => "全部短语".to_string(),
        View::Recycle => "回收站".to_string(),
        View::Category(id) => manage
            .nav_rows
            .iter()
            .find(|r| r.view == View::Category(id.clone()))
            .map(|r| r.label.clone())
            .unwrap_or_else(|| "分类".to_string()),
    };

    d2d::paint_buffered(hdc, client, |c| {
        // 1. Unified 3-column surfaces
        c.fill_rect(&l.left_pane, palette.sidebar);
        c.fill_rect(&l.middle_pane, palette.card);
        c.fill_rect(&l.right_pane, palette.card);

        // Crisp vertical column dividers
        c.line(
            l.left_pane.right as f32 - 0.5,
            0.0,
            l.left_pane.right as f32 - 0.5,
            client.bottom as f32,
            palette.divider,
            1.0,
        );
        c.line(
            l.middle_pane.right as f32 - 0.5,
            0.0,
            l.middle_pane.right as f32 - 0.5,
            client.bottom as f32,
            palette.divider,
            1.0,
        );

        // 2. Left Sidebar Header & New-Category Shell
        let left_hdr = RECT {
            left: scaled(16, dpi),
            top: scaled(12, dpi),
            right: l.left_pane.right - scaled(14, dpi),
            bottom: scaled(34, dpi),
        };
        c.draw_text(
            "短语库",
            &left_hdr,
            TextStyle::Badge,
            dpi,
            palette.muted,
            AlignH::Left,
            AlignV::Center,
        );
        let total_txt = format!("{active_total} 条");
        c.draw_text(
            &total_txt,
            &left_hdr,
            TextStyle::Badge,
            dpi,
            palette.muted,
            AlignH::Right,
            AlignV::Center,
        );

        let cat_focused = focus == manage.cat_name;
        c.fill_stroke_round(
            &l.cat_input_shell,
            d2d::scaled_f(9.0, dpi),
            palette.field,
            if cat_focused {
                palette.primary
            } else {
                palette.border
            },
            if cat_focused {
                d2d::scaled_f(1.5, dpi)
            } else {
                1.0
            },
        );

        // 3. Middle Search Shell & Subheader
        let search_focused = focus == manage.search;
        c.fill_stroke_round(
            &l.search_shell,
            d2d::scaled_f(10.0, dpi),
            palette.field,
            if search_focused {
                palette.primary
            } else {
                palette.border
            },
            if search_focused {
                d2d::scaled_f(1.5, dpi)
            } else {
                1.0
            },
        );
        c.icon(
            Icon::Search,
            (l.search_shell.left + scaled(16, dpi)) as f32,
            (l.search_shell.top + l.search_shell.bottom) as f32 * 0.5,
            d2d::scaled_f(13.0, dpi),
            palette.muted,
        );

        let mid_sub = RECT {
            left: l.middle_pane.left + scaled(14, dpi),
            top: l.search_shell.bottom + scaled(4, dpi),
            right: l.middle_pane.right - scaled(14, dpi),
            bottom: l.middle_header_bottom,
        };
        c.draw_text(
            &mid_title,
            &mid_sub,
            TextStyle::Badge,
            dpi,
            palette.muted,
            AlignH::Left,
            AlignV::Center,
        );
        let count_str = format!("{} 项", manage.phrase_rows.len());
        c.draw_text(
            &count_str,
            &mid_sub,
            TextStyle::Badge,
            dpi,
            palette.muted,
            AlignH::Right,
            AlignV::Center,
        );
        c.line(
            l.middle_pane.left as f32,
            l.middle_header_bottom as f32 - 0.5,
            l.middle_pane.right as f32,
            l.middle_header_bottom as f32 - 0.5,
            palette.divider,
            1.0,
        );

        // 4. Right Pane: Optional Recycle Banner
        if let Some(banner) = &l.recycle_banner {
            c.fill_rect(banner, palette.warning_soft);
            c.line(
                banner.left as f32,
                banner.bottom as f32 - 0.5,
                banner.right as f32,
                banner.bottom as f32 - 0.5,
                palette.divider,
                1.0,
            );
            c.icon(
                Icon::Trash,
                (banner.left + scaled(26, dpi)) as f32,
                (banner.top + banner.bottom) as f32 * 0.5,
                d2d::scaled_f(13.5, dpi),
                palette.warning,
            );
            let btxt = RECT {
                left: banner.left + scaled(40, dpi),
                top: banner.top,
                right: banner.right - scaled(120, dpi),
                bottom: banner.bottom,
            };
            c.draw_text(
                "此短语位于回收站中，可恢复至原分类或彻底删除",
                &btxt,
                TextStyle::Label,
                dpi,
                palette.warning,
                AlignH::Left,
                AlignV::Center,
            );
        }

        // 5. Editor Header
        let mode_title = if manage.editing_deleted {
            "预览已删除短语"
        } else if manage.editing.is_some() {
            "编辑短语"
        } else {
            "新建短语"
        };
        let (mtw, _) = c.measure_style(mode_title, TextStyle::Title, dpi);
        let hdr_rect = RECT {
            left: l.title_label.left,
            top: l.editor_header.top,
            right: l.title_label.left + mtw.ceil() as i32 + scaled(8, dpi),
            bottom: l.editor_header.bottom,
        };
        c.draw_text(
            mode_title,
            &hdr_rect,
            TextStyle::Title,
            dpi,
            palette.heading,
            AlignH::Left,
            AlignV::Center,
        );

        if manage.dirty {
            let badge_w = scaled(76, dpi);
            let badge_h = scaled(22, dpi);
            let badge = RECT {
                left: hdr_rect.right + scaled(6, dpi),
                top: l.editor_header.top
                    + ((l.editor_header.bottom - l.editor_header.top) - badge_h) / 2,
                right: hdr_rect.right + scaled(6, dpi) + badge_w,
                bottom: l.editor_header.top
                    + ((l.editor_header.bottom - l.editor_header.top) + badge_h) / 2,
            };
            c.fill_round(&badge, d2d::scaled_f(11.0, dpi), palette.warning_soft);
            c.fill_circle(
                (badge.left + scaled(10, dpi)) as f32,
                (badge.top + badge.bottom) as f32 * 0.5,
                d2d::scaled_f(3.0, dpi),
                palette.warning,
            );
            let btxt = RECT {
                left: badge.left + scaled(16, dpi),
                top: badge.top,
                right: badge.right - scaled(4, dpi),
                bottom: badge.bottom,
            };
            c.draw_text(
                "未保存",
                &btxt,
                TextStyle::Badge,
                dpi,
                palette.warning,
                AlignH::Center,
                AlignV::Center,
            );
        }

        c.line(
            l.editor_header.left as f32,
            l.editor_header.bottom as f32 - 0.5,
            l.editor_header.right as f32,
            l.editor_header.bottom as f32 - 0.5,
            palette.divider,
            1.0,
        );

        // 6. Title Label + Counter + Rounded Input Shell
        c.draw_text(
            "标题 *",
            &l.title_label,
            TextStyle::Label,
            dpi,
            palette.secondary,
            AlignH::Left,
            AlignV::Center,
        );
        let t_counter = format!("{title_chars} / 80");
        c.draw_text(
            &t_counter,
            &l.title_label,
            TextStyle::Badge,
            dpi,
            if title_chars > 80 {
                palette.danger
            } else {
                palette.muted
            },
            AlignH::Right,
            AlignV::Center,
        );
        let title_err = manage.error_field == Some(ID_TITLE);
        let title_focus = focus == manage.title_edit;
        c.fill_stroke_round(
            &l.title_shell,
            d2d::scaled_f(10.0, dpi),
            palette.field,
            if title_err {
                palette.error
            } else if title_focus {
                palette.primary
            } else {
                palette.border
            },
            if title_err || title_focus {
                d2d::scaled_f(1.5, dpi)
            } else {
                1.0
            },
        );

        // 7. Content Label + Counter + Rounded Input Shell
        c.draw_text(
            "短语正文 *",
            &l.content_label,
            TextStyle::Label,
            dpi,
            palette.secondary,
            AlignH::Left,
            AlignV::Center,
        );
        let c_counter = format!("{content_chars} / 10,000 字符 · {content_lines} 行");
        c.draw_text(
            &c_counter,
            &l.content_label,
            TextStyle::Badge,
            dpi,
            if content_chars > 10_000 {
                palette.danger
            } else {
                palette.muted
            },
            AlignH::Right,
            AlignV::Center,
        );
        let content_err = manage.error_field == Some(ID_CONTENT);
        let content_focus = focus == manage.content_edit;
        c.fill_stroke_round(
            &l.content_shell,
            d2d::scaled_f(10.0, dpi),
            palette.field,
            if content_err {
                palette.error
            } else if content_focus {
                palette.primary
            } else {
                palette.border
            },
            if content_err || content_focus {
                d2d::scaled_f(1.5, dpi)
            } else {
                1.0
            },
        );

        // 8. Metadata Labels + Tactile Pin Switch Card
        c.draw_text(
            "所属分类",
            &l.meta_cat_label,
            TextStyle::Label,
            dpi,
            palette.secondary,
            AlignH::Left,
            AlignV::Center,
        );
        c.draw_text(
            "置顶",
            &l.meta_pin_label,
            TextStyle::Label,
            dpi,
            palette.secondary,
            AlignH::Left,
            AlignV::Center,
        );
        c.fill_stroke_round(
            &l.pin_card,
            d2d::scaled_f(10.0, dpi),
            if manage.pin_hovered || ui::hovered(manage.pinned) {
                palette.hover
            } else {
                palette.card_subtle
            },
            if manage.pinned_state {
                palette.selection_border
            } else {
                palette.border
            },
            1.0,
        );
        c.icon(
            Icon::Pin,
            (l.pin_card.left + scaled(20, dpi)) as f32,
            (l.pin_card.top + l.pin_card.bottom) as f32 * 0.5,
            d2d::scaled_f(13.5, dpi),
            if manage.pinned_state {
                palette.primary
            } else {
                palette.secondary
            },
        );
        let pin_lbl = RECT {
            left: l.pin_card.left + scaled(34, dpi),
            top: l.pin_card.top,
            right: (l.pin_switch.left - scaled(8, dpi)).max(l.pin_card.left + scaled(44, dpi)),
            bottom: l.pin_card.bottom,
        };
        let pin_text = if l.pin_card.right - l.pin_card.left < scaled(158, dpi) {
            "置顶"
        } else {
            "置顶此短语"
        };
        c.draw_text(
            pin_text,
            &pin_lbl,
            TextStyle::Body,
            dpi,
            palette.heading,
            AlignH::Left,
            AlignV::Center,
        );
        c.switch(
            &l.pin_switch,
            manage.pinned_state,
            manage.pin_hovered,
            false,
            palette,
            dpi,
        );

        // 9. Footer Bar
        c.fill_rect(&l.footer, palette.card_subtle);
        c.line(
            l.footer.left as f32,
            l.footer.top as f32 + 0.5,
            l.footer.right as f32,
            l.footer.top as f32 + 0.5,
            palette.divider,
            1.0,
        );
        let status_color = if manage.error_message.is_some() {
            palette.danger
        } else {
            palette.muted
        };
        c.draw_text(
            &manage.status_text,
            &l.status_rect,
            if manage.error_message.is_some() {
                TextStyle::Label
            } else {
                TextStyle::Caption
            },
            dpi,
            status_color,
            AlignH::Left,
            AlignV::Center,
        );
    });
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

fn create(owner: HWND) -> Option<Manage> {
    let module = match unsafe { GetModuleHandleW(None) } {
        Ok(module) => module,
        Err(err) => {
            logging::error(&format!("manage: GetModuleHandleW failed: {err}"));
            return None;
        }
    };
    let instance = HINSTANCE(module.0);
    let palette = theme::current();
    let sidebar_brush = unsafe { CreateSolidBrush(palette.sidebar) };
    let card_brush = unsafe { CreateSolidBrush(palette.card) };
    let field_brush = unsafe { CreateSolidBrush(palette.field) };

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
            logging::error(&format!("manage: RegisterClassW failed: {error:?}"));
            unsafe {
                let _ = DeleteObject(HGDIOBJ(sidebar_brush.0));
                let _ = DeleteObject(HGDIOBJ(card_brush.0));
                let _ = DeleteObject(HGDIOBJ(field_brush.0));
            }
            return None;
        }
    }

    let hwnd = match unsafe {
        let initial_dpi = GetDpiForSystem();
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            CLASS,
            TITLE,
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            80,
            80,
            scaled(DEFAULT_WIDTH, initial_dpi),
            scaled(DEFAULT_HEIGHT, initial_dpi),
            Some(owner),
            None,
            Some(instance),
            None,
        )
    } {
        Ok(hwnd) => hwnd,
        Err(err) => {
            logging::error(&format!("manage: CreateWindowExW failed: {err}"));
            unsafe {
                let _ = DeleteObject(HGDIOBJ(sidebar_brush.0));
                let _ = DeleteObject(HGDIOBJ(card_brush.0));
                let _ = DeleteObject(HGDIOBJ(field_brush.0));
            }
            return None;
        }
    };

    ui::apply_window_style(hwnd);
    ui::apply_window_icon(hwnd, instance);
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    let dpi = if dpi == 0 { 96 } else { dpi };
    let fonts = Fonts {
        body: ui::make_font(ui::FontRole::BodyInput, dpi),
        title: ui::make_font(ui::FontRole::TitleInput, dpi),
    };

    let left = create_control(
        w!("LISTBOX"),
        "",
        WS_CHILD
            | WS_VISIBLE
            | WINDOW_STYLE(
                (LBS_NOTIFY | LBS_NOINTEGRALHEIGHT | LBS_OWNERDRAWFIXED | LBS_HASSTRINGS) as u32,
            ),
        hwnd,
        ID_LEFT,
        instance,
    );
    let cat_name = create_control(
        w!("EDIT"),
        "",
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
        hwnd,
        ID_CAT_NAME,
        instance,
    );
    let cat_add = create_control(
        w!("BUTTON"),
        "添加",
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(BS_OWNERDRAW as u32),
        hwnd,
        ID_CAT_ADD,
        instance,
    );
    let cat_delete = create_control(
        w!("BUTTON"),
        "删除当前分类",
        WS_CHILD | WINDOW_STYLE(BS_OWNERDRAW as u32),
        hwnd,
        ID_CAT_DELETE,
        instance,
    );
    let search = create_control(
        w!("EDIT"),
        "",
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
        hwnd,
        ID_SEARCH,
        instance,
    );
    let list = create_control(
        w!("LISTBOX"),
        "",
        WS_CHILD
            | WS_VISIBLE
            | WINDOW_STYLE(
                (LBS_NOTIFY | LBS_NOINTEGRALHEIGHT | LBS_OWNERDRAWFIXED | LBS_HASSTRINGS) as u32,
            ),
        hwnd,
        ID_LIST,
        instance,
    );
    let new_button = create_control(
        w!("BUTTON"),
        "新建",
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(BS_OWNERDRAW as u32),
        hwnd,
        ID_NEW,
        instance,
    );
    let delete_button = create_control(
        w!("BUTTON"),
        "移入回收站",
        WS_CHILD | WINDOW_STYLE(BS_OWNERDRAW as u32),
        hwnd,
        ID_DELETE,
        instance,
    );
    let restore_button = create_control(
        w!("BUTTON"),
        "立即恢复",
        WS_CHILD | WINDOW_STYLE(BS_OWNERDRAW as u32),
        hwnd,
        ID_RESTORE,
        instance,
    );
    let title_edit = create_control(
        w!("EDIT"),
        "",
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
        hwnd,
        ID_TITLE,
        instance,
    );
    let content_edit = create_control(
        w!("EDIT"),
        "",
        WS_CHILD
            | WS_VISIBLE
            | WINDOW_STYLE((ES_MULTILINE | ES_AUTOVSCROLL | ES_WANTRETURN | ES_NOHIDESEL) as u32),
        hwnd,
        ID_CONTENT,
        instance,
    );
    let category = create_control(
        w!("COMBOBOX"),
        "",
        WS_CHILD
            | WS_VISIBLE
            | WINDOW_STYLE((CBS_DROPDOWNLIST | CBS_OWNERDRAWFIXED | CBS_HASSTRINGS) as u32),
        hwnd,
        ID_CATEGORY,
        instance,
    );
    let pinned = create_control(
        w!("BUTTON"),
        "置顶此短语",
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(BS_OWNERDRAW as u32),
        hwnd,
        ID_PINNED,
        instance,
    );
    let save_button = create_control(
        w!("BUTTON"),
        "保存短语",
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(BS_OWNERDRAW as u32),
        hwnd,
        ID_SAVE,
        instance,
    );
    let cancel_button = create_control(
        w!("BUTTON"),
        "重置",
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(BS_OWNERDRAW as u32),
        hwnd,
        ID_CANCEL,
        instance,
    );

    for button in [
        cat_add,
        cat_delete,
        new_button,
        delete_button,
        restore_button,
        pinned,
        save_button,
        cancel_button,
    ] {
        ui::install_button(button);
    }
    for control in [
        left,
        list,
        category,
        cat_name,
        search,
        title_edit,
        content_edit,
    ] {
        ui::theme_control(control, palette.dark);
    }
    ui::theme_combo(category, palette.dark);
    ui::install_combo(category);
    ui::install_smooth_listbox(left, ui::ListSurface::Sidebar);
    ui::install_smooth_listbox(list, ui::ListSurface::Card);
    ui::install_smooth_edit(content_edit);
    ui::theme_scrollbar(content_edit, palette.dark);
    ui::cue_banner(search, "搜索标题或正文…");
    ui::cue_banner(title_edit, "例如：项目进度回复、会议纪要模板…");
    ui::cue_banner(cat_name, "新分类名称…");
    ui::cue_banner(content_edit, "在此输入短语正文，支持多行文本…");

    unsafe {
        let _ = SendMessageW(left, LB_SETITEMHEIGHT, 0, scaled(36, dpi) as isize);
        let _ = SendMessageW(list, LB_SETITEMHEIGHT, 0, scaled(76, dpi) as isize);
        let _ = SendMessageW(
            category,
            CB_SETITEMHEIGHT,
            usize::MAX,
            scaled(34, dpi) as isize,
        );
        let _ = SendMessageW(category, CB_SETITEMHEIGHT, 0, scaled(30, dpi) as isize);
    }

    let mut manage = Manage {
        hwnd,
        left,
        cat_name,
        cat_add,
        cat_delete,
        search,
        list,
        new_button,
        delete_button,
        restore_button,
        title_edit,
        content_edit,
        category,
        pinned,
        save_button,
        cancel_button,
        layout: Layout::default(),
        fonts,
        palette,
        sidebar_brush,
        card_brush,
        field_brush,
        error_field: None,
        error_message: None,
        status_text: "就绪".to_string(),
        dpi,
        view: View::All,
        editing: None,
        editing_deleted: false,
        pinned_state: false,
        pin_hovered: false,
        tracking_mouse: false,
        selected: None,
        draft_category: None,
        dirty: false,
        loading: true,
        nav_rows: Vec::new(),
        phrase_rows: Vec::new(),
    };

    for control in [
        left,
        cat_name,
        cat_add,
        cat_delete,
        search,
        list,
        new_button,
        delete_button,
        restore_button,
        content_edit,
        category,
        pinned,
        save_button,
        cancel_button,
    ] {
        unsafe {
            let _ = SendMessageW(control, WM_SETFONT, manage.fonts.body.0 as usize, 1);
            let style = GetWindowLongW(control, GWL_STYLE);
            let _ = SetWindowLongW(control, GWL_STYLE, style | WS_TABSTOP.0 as i32);
        }
    }
    unsafe {
        let _ = SendMessageW(title_edit, WM_SETFONT, manage.fonts.title.0 as usize, 1);
        let style = GetWindowLongW(title_edit, GWL_STYLE);
        let _ = SetWindowLongW(title_edit, GWL_STYLE, style | WS_TABSTOP.0 as i32);
    }

    reload_left(&mut manage);
    reload_categories(&mut manage);
    reload_list(&mut manage);
    let first_id = manage.phrase_rows.first().map(|r| r.id.clone());
    load_editor(&mut manage, first_id.as_deref());
    if first_id.is_some() {
        manage.selected = Some(0);
        unsafe {
            let _ = SendMessageW(manage.list, LB_SETCURSEL, 0, 0);
        }
    }
    relayout(&mut manage);
    manage.loading = false;
    Some(manage)
}

pub fn show(owner: HWND) {
    let needs_create = MANAGE.with(|slot| match slot.try_borrow() {
        Ok(guard) => guard.is_none(),
        Err(_) => false,
    });
    if needs_create {
        if let Some(manage) = create(owner) {
            MANAGE.with(|slot| {
                if let Ok(mut guard) = slot.try_borrow_mut() {
                    *guard = Some(manage);
                }
            });
        }
    }

    let hwnd = MANAGE.with(|slot| {
        slot.try_borrow()
            .ok()
            .and_then(|guard| guard.as_ref().map(|manage| manage.hwnd))
    });
    let Some(hwnd) = hwnd else {
        return;
    };
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
    }

    MANAGE.with(|slot| {
        let mut guard = match slot.try_borrow_mut() {
            Ok(guard) => guard,
            Err(_) => {
                logging::warn("manage: busy while opening (modal dialog?)");
                return;
            }
        };
        let Some(manage) = guard.as_mut() else {
            return;
        };
        let palette = theme::current();
        if palette != manage.palette {
            unsafe {
                let _ = DeleteObject(HGDIOBJ(manage.sidebar_brush.0));
                let _ = DeleteObject(HGDIOBJ(manage.card_brush.0));
                let _ = DeleteObject(HGDIOBJ(manage.field_brush.0));
            }
            manage.sidebar_brush = unsafe { CreateSolidBrush(palette.sidebar) };
            manage.card_brush = unsafe { CreateSolidBrush(palette.card) };
            manage.field_brush = unsafe { CreateSolidBrush(palette.field) };
            manage.palette = palette;
            for control in [
                manage.left,
                manage.list,
                manage.category,
                manage.cat_name,
                manage.search,
                manage.title_edit,
                manage.content_edit,
            ] {
                ui::theme_control(control, palette.dark);
            }
            ui::theme_combo(manage.category, palette.dark);
            ui::theme_scrollbar(manage.content_edit, palette.dark);
            ui::apply_window_style(manage.hwnd);
        }
        unsafe {
            let _ = SetFocus(Some(manage.search));
        }
        reload_left(manage);
        reload_categories(manage);
        reload_list(manage);
        // Reopen a clean editor from the store, so an import or restore cannot
        // leave stale text that overwrites a new snippet on save. An unsaved
        // draft is left untouched: silently replacing it here would lose the
        // user's edits, and other navigation paths already ask before
        // discarding.
        if !manage.dirty {
            let editing = manage.editing.clone();
            let still_exists = editing
                .as_deref()
                .map(|id| manage.phrase_rows.iter().any(|row| row.id == id))
                .unwrap_or(false);
            let next = if still_exists {
                editing
            } else {
                manage.phrase_rows.first().map(|row| row.id.clone())
            };
            if let Some(id) = next.as_deref() {
                if let Some(position) = manage.phrase_rows.iter().position(|row| row.id == id) {
                    manage.selected = Some(position);
                    unsafe {
                        let _ = SendMessageW(manage.list, LB_SETCURSEL, position, 0);
                    }
                }
            }
            load_editor(manage, next.as_deref());
        }
        relayout(manage);
        unsafe {
            let _ = InvalidateRect(Some(manage.hwnd), None, true);
        }
    });
}

pub fn show_test(owner: HWND) -> bool {
    QUIT_ON_CLOSE.store(true, Ordering::Relaxed);
    show(owner);
    MANAGE.with(|slot| {
        slot.try_borrow()
            .ok()
            .and_then(|guard| guard.as_ref().map(|manage| !manage.hwnd.0.is_null()))
            .unwrap_or(false)
    })
}

pub fn is_dialog_message(message: &MSG) -> bool {
    let hwnd = MANAGE.with(|slot| {
        slot.try_borrow()
            .ok()
            .and_then(|guard| guard.as_ref().map(|manage| manage.hwnd))
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
    unsafe { IsDialogMessageW(hwnd, message).as_bool() }
}

pub fn shutdown() {
    let manage = MANAGE.with(|slot| match slot.try_borrow_mut() {
        Ok(mut guard) => guard.take(),
        Err(_) => {
            logging::warn("manage: shutdown skipped while a dialog is open");
            None
        }
    });
    let Some(manage) = manage else {
        return;
    };
    unsafe {
        let _ = DestroyWindow(manage.hwnd);
        let _ = DeleteObject(HGDIOBJ(manage.fonts.body.0));
        let _ = DeleteObject(HGDIOBJ(manage.fonts.title.0));
        let _ = DeleteObject(HGDIOBJ(manage.sidebar_brush.0));
        let _ = DeleteObject(HGDIOBJ(manage.card_brush.0));
        let _ = DeleteObject(HGDIOBJ(manage.field_brush.0));
    }
}

pub fn confirm_exit() -> bool {
    MANAGE.with(|slot| match slot.try_borrow() {
        Ok(guard) => match guard.as_ref() {
            Some(manage) if manage.dirty => confirm_discard(manage.hwnd),
            _ => true,
        },
        Err(_) => {
            logging::warn("manage: exit blocked while a dialog is open");
            false
        }
    })
}

fn point_in(r: &RECT, x: i32, y: i32) -> bool {
    x >= r.left && x <= r.right && y >= r.top && y <= r.bottom
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
            MANAGE.with(|slot| {
                if let Ok(mut guard) = slot.try_borrow_mut() {
                    if let Some(manage) = guard.as_mut() {
                        layout(manage, width, height);
                    }
                }
            });
            LRESULT(0)
        }
        WM_GETMINMAXINFO => {
            let info = &mut *(lparam.0 as *mut MINMAXINFO);
            MANAGE.with(|slot| {
                let guard = match slot.try_borrow() {
                    Ok(guard) => guard,
                    Err(_) => return,
                };
                let dpi = guard
                    .as_ref()
                    .map(|manage| manage.dpi)
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
            MANAGE.with(|slot| {
                if let Ok(mut guard) = slot.try_borrow_mut() {
                    if let Some(manage) = guard.as_mut() {
                        let _ = DeleteObject(HGDIOBJ(manage.fonts.body.0));
                        let _ = DeleteObject(HGDIOBJ(manage.fonts.title.0));
                        manage.fonts = Fonts {
                            body: ui::make_font(ui::FontRole::BodyInput, dpi),
                            title: ui::make_font(ui::FontRole::TitleInput, dpi),
                        };
                        manage.dpi = dpi;
                        for control in [
                            manage.left,
                            manage.cat_name,
                            manage.cat_add,
                            manage.cat_delete,
                            manage.search,
                            manage.list,
                            manage.new_button,
                            manage.delete_button,
                            manage.restore_button,
                            manage.content_edit,
                            manage.category,
                            manage.pinned,
                            manage.save_button,
                            manage.cancel_button,
                        ] {
                            let _ =
                                SendMessageW(control, WM_SETFONT, manage.fonts.body.0 as usize, 1);
                        }
                        let _ = SendMessageW(
                            manage.title_edit,
                            WM_SETFONT,
                            manage.fonts.title.0 as usize,
                            1,
                        );
                        let _ = SendMessageW(
                            manage.left,
                            LB_SETITEMHEIGHT,
                            0,
                            scaled(36, dpi) as isize,
                        );
                        let _ = SendMessageW(
                            manage.list,
                            LB_SETITEMHEIGHT,
                            0,
                            scaled(76, dpi) as isize,
                        );
                        let _ = SendMessageW(
                            manage.category,
                            CB_SETITEMHEIGHT,
                            usize::MAX,
                            scaled(34, dpi) as isize,
                        );
                        let _ = SendMessageW(
                            manage.category,
                            CB_SETITEMHEIGHT,
                            0,
                            scaled(30, dpi) as isize,
                        );
                        layout(manage, width, height);
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
        WM_MOUSEMOVE => {
            let x = (lparam.0 & 0xffff) as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xffff) as i16 as i32;
            let redraw = MANAGE.with(|slot| {
                let mut guard = slot.try_borrow_mut().ok()?;
                let manage = guard.as_mut()?;
                if !manage.tracking_mouse {
                    manage.tracking_mouse = true;
                    let mut track = TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    let _ = TrackMouseEvent(&mut track);
                }
                let next = point_in(&manage.layout.pin_card, x, y);
                if next != manage.pin_hovered {
                    manage.pin_hovered = next;
                    unsafe {
                        let _ = InvalidateRect(Some(manage.pinned), None, false);
                    }
                    Some(manage.layout.pin_card)
                } else {
                    None
                }
            });
            if let Some(rect) = redraw {
                let _ = InvalidateRect(Some(hwnd), Some(&rect), false);
            }
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            let redraw = MANAGE.with(|slot| {
                let mut guard = slot.try_borrow_mut().ok()?;
                let manage = guard.as_mut()?;
                manage.tracking_mouse = false;
                if manage.pin_hovered {
                    manage.pin_hovered = false;
                    unsafe {
                        let _ = InvalidateRect(Some(manage.pinned), None, false);
                    }
                    Some(manage.layout.pin_card)
                } else {
                    None
                }
            });
            if let Some(rect) = redraw {
                let _ = InvalidateRect(Some(hwnd), Some(&rect), false);
            }
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let x = (lparam.0 & 0xffff) as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xffff) as i16 as i32;
            MANAGE.with(|slot| {
                if let Ok(mut guard) = slot.try_borrow_mut() {
                    if let Some(manage) = guard.as_mut() {
                        if point_in(&manage.layout.pin_card, x, y) {
                            manage.pinned_state = !manage.pinned_state;
                            if !manage.loading {
                                manage.dirty = true;
                                manage.status_text = "未保存修改".to_string();
                            }
                            let _ = InvalidateRect(Some(manage.pinned), None, false);
                            let _ = InvalidateRect(Some(hwnd), None, false);
                        } else if point_in(&manage.layout.search_shell, x, y) {
                            let _ = SetFocus(Some(manage.search));
                        } else if point_in(&manage.layout.title_shell, x, y) {
                            let _ = SetFocus(Some(manage.title_edit));
                        } else if point_in(&manage.layout.content_shell, x, y) {
                            let _ = SetFocus(Some(manage.content_edit));
                        } else if point_in(&manage.layout.cat_input_shell, x, y) {
                            let _ = SetFocus(Some(manage.cat_name));
                        }
                    }
                }
            });
            LRESULT(0)
        }
        WM_COMMAND => {
            let id = wparam.0 & 0xffff;
            let notification = ((wparam.0 >> 16) & 0xffff) as u32;
            MANAGE.with(|slot| {
                let Ok(mut guard) = slot.try_borrow_mut() else {
                    return;
                };
                let Some(manage) = guard.as_mut() else {
                    return;
                };
                match id as i32 {
                    ID_SEARCH | ID_TITLE | ID_CONTENT | ID_CAT_NAME
                        if notification == EN_SETFOCUS || notification == EN_KILLFOCUS =>
                    {
                        let _ = InvalidateRect(Some(hwnd), None, false);
                    }
                    ID_SEARCH if notification == EN_CHANGE => {
                        if !manage.loading {
                            reload_list(manage);
                        }
                    }
                    ID_LEFT if notification == LBN_SELCHANGE => {
                        let index = SendMessageW(manage.left, LB_GETCURSEL, 0, 0).0 as i32;
                        let Some(next_view) =
                            manage.nav_rows.get(index as usize).map(|r| r.view.clone())
                        else {
                            return;
                        };
                        if next_view == manage.view {
                            return;
                        }
                        if manage.dirty && !confirm_discard(hwnd) {
                            let previous = manage
                                .nav_rows
                                .iter()
                                .position(|r| r.view == manage.view)
                                .unwrap_or(0);
                            let _ = SendMessageW(manage.left, LB_SETCURSEL, previous, 0);
                            return;
                        }
                        manage.view = next_view;
                        manage.editing = None;
                        manage.dirty = false;
                        let show_del_cat = matches!(manage.view, View::Category(_));
                        let _ = ShowWindow(
                            manage.cat_delete,
                            if show_del_cat { SW_SHOW } else { SW_HIDE },
                        );
                        reload_list(manage);
                        let first = manage.phrase_rows.first().map(|r| r.id.clone());
                        load_editor(manage, first.as_deref());
                        if first.is_some() {
                            manage.selected = Some(0);
                            let _ = SendMessageW(manage.list, LB_SETCURSEL, 0, 0);
                        }
                        relayout(manage);
                    }
                    ID_LIST if notification == LBN_SELCHANGE => {
                        let index = SendMessageW(manage.list, LB_GETCURSEL, 0, 0).0;
                        if index < 0 {
                            return;
                        }
                        let Some(id) = manage.phrase_rows.get(index as usize).map(|r| r.id.clone())
                        else {
                            return;
                        };
                        if manage.editing.as_deref() == Some(id.as_str()) {
                            return;
                        }
                        if manage.dirty && !confirm_discard(hwnd) {
                            let previous = manage
                                .phrase_rows
                                .iter()
                                .position(|r| Some(r.id.as_str()) == manage.editing.as_deref());
                            let _ = SendMessageW(
                                manage.list,
                                LB_SETCURSEL,
                                previous.unwrap_or(usize::MAX),
                                0,
                            );
                            return;
                        }
                        manage.selected = Some(index as usize);
                        load_editor(manage, Some(&id));
                    }
                    ID_CAT_ADD if notification == BN_CLICKED => {
                        let name = wide_text(manage.cat_name);
                        match storage::with_store(|store| store.add_category(&name)) {
                            Some(Ok(_)) => {
                                set_text(manage.cat_name, "");
                                status_message(manage, "已添加分类");
                                reload_left(manage);
                                reload_categories(manage);
                                reload_list(manage);
                                relayout(manage);
                            }
                            Some(Err(code)) => status_message(manage, error_text(&code)),
                            None => status_message(manage, "存储未初始化"),
                        }
                    }
                    ID_CAT_DELETE if notification == BN_CLICKED => {
                        let View::Category(category) = manage.view.clone() else {
                            status_message(manage, "请先在左侧选择要删除的分类");
                            return;
                        };
                        if manage.dirty && !confirm_discard(hwnd) {
                            return;
                        }
                        let text = platform::wide("删除分类后，其短语会移到未分类。继续吗？");
                        let caption = platform::wide("QuickPhrase");
                        let answer = MessageBoxW(
                            Some(manage.hwnd),
                            PCWSTR(text.as_ptr()),
                            PCWSTR(caption.as_ptr()),
                            MB_YESNO | MB_ICONQUESTION,
                        );
                        if answer != IDYES {
                            return;
                        }
                        match storage::with_store(|store| store.delete_category(&category)) {
                            Some(Ok(())) => {
                                manage.view = View::All;
                                manage.editing = None;
                                manage.dirty = false;
                                reload_left(manage);
                                reload_categories(manage);
                                reload_list(manage);
                                let first = manage.phrase_rows.first().map(|r| r.id.clone());
                                load_editor(manage, first.as_deref());
                                status_message(manage, "已删除分类");
                            }
                            Some(Err(code)) => status_message(manage, error_text(&code)),
                            None => status_message(manage, "存储未初始化"),
                        }
                    }
                    ID_NEW => {
                        if manage.view == View::Recycle {
                            status_message(manage, "回收站内不能新建，请先切换分类");
                            return;
                        }
                        if manage.dirty && !confirm_discard(hwnd) {
                            return;
                        }
                        manage.selected = None;
                        let _ = SendMessageW(manage.list, LB_SETCURSEL, usize::MAX, 0);
                        load_editor(manage, None);
                        let _ = SetFocus(Some(manage.title_edit));
                    }
                    ID_DELETE => delete_selected(manage),
                    ID_RESTORE if notification == BN_CLICKED => {
                        let Some(id) = manage.editing.clone() else {
                            status_message(manage, "请先选择要恢复的短语");
                            return;
                        };
                        if manage.dirty && !confirm_discard(hwnd) {
                            return;
                        }
                        match storage::with_store(|store| store.restore(&id)) {
                            Some(Ok(())) => {
                                reload_left(manage);
                                reload_list(manage);
                                let first = manage.phrase_rows.first().map(|r| r.id.clone());
                                load_editor(manage, first.as_deref());
                                status_message(manage, "已恢复短语");
                            }
                            Some(Err(code)) => status_message(manage, error_text(&code)),
                            None => status_message(manage, "存储未初始化"),
                        }
                    }
                    ID_SAVE => {
                        if manage.view == View::Recycle || manage.editing_deleted {
                            status_message(manage, "回收站内不能保存，请先恢复");
                            return;
                        }
                        save_editor(manage);
                    }
                    ID_CANCEL => {
                        let id = manage.editing.clone();
                        load_editor(manage, id.as_deref());
                        status_message(manage, "已重置修改");
                    }
                    ID_TITLE | ID_CONTENT if notification == EN_CHANGE => {
                        if manage.error_field == Some(id as i32) {
                            manage.error_field = None;
                            manage.error_message = None;
                        }
                        if !manage.loading {
                            manage.dirty = true;
                            manage.status_text = "未保存修改".to_string();
                        }
                        let _ = InvalidateRect(Some(hwnd), None, false);
                    }
                    ID_PINNED if notification == BN_CLICKED => {
                        manage.pinned_state = !manage.pinned_state;
                        if !manage.loading {
                            manage.dirty = true;
                            manage.status_text = "未保存修改".to_string();
                        }
                        unsafe {
                            let _ = InvalidateRect(Some(manage.pinned), None, false);
                            let _ = InvalidateRect(Some(hwnd), None, false);
                        }
                    }
                    ID_CATEGORY if notification == CBN_SELCHANGE => {
                        if manage.error_field == Some(ID_CATEGORY) {
                            manage.error_field = None;
                            manage.error_message = None;
                        }
                        if !manage.loading {
                            manage.draft_category = current_category(manage);
                            manage.dirty = true;
                            manage.status_text = "未保存修改".to_string();
                            let _ = InvalidateRect(Some(hwnd), None, false);
                        }
                    }
                    _ => {}
                }
            });
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_CTLCOLORLISTBOX => {
            let hdc = HDC(wparam.0 as *mut core::ffi::c_void);
            let control = HWND(lparam.0 as *mut core::ffi::c_void);
            let (bg, brush) = MANAGE.with(|slot| match slot.try_borrow() {
                Ok(guard) => match guard.as_ref() {
                    Some(manage) => {
                        if control == manage.left {
                            (manage.palette.sidebar, manage.sidebar_brush)
                        } else {
                            (manage.palette.card, manage.card_brush)
                        }
                    }
                    None => (COLORREF(0xFFFFFF), HBRUSH::default()),
                },
                Err(_) => (COLORREF(0xFFFFFF), HBRUSH::default()),
            });
            SetBkColor(hdc, bg);
            LRESULT(brush.0 as isize)
        }
        WM_CTLCOLOREDIT => {
            let hdc = HDC(wparam.0 as *mut core::ffi::c_void);
            let (heading, field, brush) = MANAGE.with(|slot| match slot.try_borrow() {
                Ok(guard) => match guard.as_ref() {
                    Some(manage) => (
                        manage.palette.heading,
                        manage.palette.field,
                        manage.field_brush,
                    ),
                    None => (COLORREF(0), COLORREF(0), HBRUSH::default()),
                },
                Err(_) => (COLORREF(0), COLORREF(0), HBRUSH::default()),
            });
            SetBkMode(hdc, TRANSPARENT);
            SetBkColor(hdc, field);
            SetTextColor(hdc, heading);
            LRESULT(brush.0 as isize)
        }
        WM_DRAWITEM => {
            let draw = &*(lparam.0 as *const DRAWITEMSTRUCT);
            MANAGE.with(|slot| {
                let Ok(guard) = slot.try_borrow() else {
                    return;
                };
                let Some(manage) = guard.as_ref() else {
                    return;
                };
                match draw.CtlID as i32 {
                    ID_LEFT => draw_nav_row(manage, draw),
                    ID_LIST => draw_phrase_row(manage, draw),
                    ID_CATEGORY => {
                        ui::draw_combo_item(draw, manage.palette, manage.dpi, manage.fonts.body)
                    }
                    ID_PINNED => draw_pin_switch(manage, draw),
                    ID_NEW => ui::draw_button_with_icon(
                        draw,
                        manage.palette,
                        ui::ButtonKind::Primary,
                        Some(Icon::Plus),
                        manage.dpi,
                        manage.palette.card,
                    ),
                    ID_SAVE => ui::draw_button(
                        draw,
                        manage.palette,
                        ui::ButtonKind::Primary,
                        manage.dpi,
                        manage.palette.card_subtle,
                    ),
                    ID_CANCEL => ui::draw_button(
                        draw,
                        manage.palette,
                        ui::ButtonKind::Secondary,
                        manage.dpi,
                        manage.palette.card_subtle,
                    ),
                    ID_RESTORE => ui::draw_button(
                        draw,
                        manage.palette,
                        ui::ButtonKind::Primary,
                        manage.dpi,
                        manage.palette.warning_soft,
                    ),
                    ID_DELETE => ui::draw_button_with_icon(
                        draw,
                        manage.palette,
                        ui::ButtonKind::Danger,
                        Some(Icon::Trash),
                        manage.dpi,
                        manage.palette.card,
                    ),
                    ID_CAT_DELETE => ui::draw_button(
                        draw,
                        manage.palette,
                        ui::ButtonKind::Danger,
                        manage.dpi,
                        manage.palette.sidebar,
                    ),
                    ID_CAT_ADD => ui::draw_button(
                        draw,
                        manage.palette,
                        ui::ButtonKind::Subtle,
                        manage.dpi,
                        manage.palette.sidebar,
                    ),
                    _ => ui::draw_button(
                        draw,
                        manage.palette,
                        ui::ButtonKind::Secondary,
                        manage.dpi,
                        manage.palette.card,
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
            MANAGE.with(|slot| {
                if let Ok(guard) = slot.try_borrow() {
                    if let Some(manage) = guard.as_ref() {
                        paint_manage(manage, hdc, client);
                    }
                }
            });
            let _ = EndPaint(hwnd, &paint);
            LRESULT(0)
        }
        WM_CLOSE => {
            let (should_close, had_dirty) = MANAGE.with(|slot| match slot.try_borrow() {
                Ok(guard) => match guard.as_ref() {
                    Some(manage) if manage.dirty => (confirm_discard(manage.hwnd), true),
                    _ => (true, false),
                },
                Err(_) => (true, false),
            });
            if should_close {
                if had_dirty {
                    MANAGE.with(|slot| {
                        if let Ok(mut guard) = slot.try_borrow_mut() {
                            if let Some(manage) = guard.as_mut() {
                                let id = manage.editing.clone();
                                load_editor(manage, id.as_deref());
                            }
                        }
                    });
                }
                if QUIT_ON_CLOSE.load(Ordering::Relaxed) {
                    let _ = DestroyWindow(hwnd);
                } else {
                    let _ = ShowWindow(hwnd, SW_HIDE);
                }
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            MANAGE.with(|slot| {
                if let Ok(mut guard) = slot.try_borrow_mut() {
                    if let Some(manage) = guard.take() {
                        let _ = DeleteObject(HGDIOBJ(manage.fonts.body.0));
                        let _ = DeleteObject(HGDIOBJ(manage.fonts.title.0));
                        let _ = DeleteObject(HGDIOBJ(manage.sidebar_brush.0));
                        let _ = DeleteObject(HGDIOBJ(manage.card_brush.0));
                        let _ = DeleteObject(HGDIOBJ(manage.field_brush.0));
                    }
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
