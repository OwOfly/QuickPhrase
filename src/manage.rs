//! M1-4 management window: three panes, CRUD, categories, recycle bin.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateFontW, CreateSolidBrush, DeleteObject, EndPaint, FillRect, InvalidateRect,
    ScreenToClient, SetBkColor, SetBkMode, SetTextColor, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS,
    DEFAULT_CHARSET, DEFAULT_PITCH, DT_END_ELLIPSIS, DT_LEFT, DT_NOPREFIX, DT_SINGLELINE,
    DT_VCENTER, HBRUSH, HFONT, HGDIOBJ, OUT_DEFAULT_PRECIS, PAINTSTRUCT, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::{DRAWITEMSTRUCT, ODS_SELECTED};
use windows::Win32::UI::HiDpi::{GetDpiForSystem, GetDpiForWindow};
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::search::{self, Doc};
use crate::storage::{self, Snippet};
use crate::ui;
use crate::{logging, platform};

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
const ID_STATUS: i32 = 211;
const ID_CAT_NAME: i32 = 212;
const ID_CAT_ADD: i32 = 213;
const ID_CAT_DELETE: i32 = 214;
const ID_RESTORE: i32 = 215;

const MIN_WIDTH: i32 = 800;
const MIN_HEIGHT: i32 = 540;
const DEFAULT_WIDTH: i32 = 1040;
const DEFAULT_HEIGHT: i32 = 650;

const LEFT_WIDTH: i32 = 190;
const MIDDLE_WIDTH: i32 = 320;
const MARGIN: i32 = 12;
const ROW: i32 = 32;
const SS_LEFT: u32 = 0;

/// Local wrapper so every control message can use plain `usize`/`isize`.
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

struct Fonts {
    body: HFONT,
    title: HFONT,
    small: HFONT,
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
    status: HWND,
    cards: [RECT; 3],
    fonts: Fonts,
    palette: crate::theme::Palette,
    page_brush: HBRUSH,
    card_brush: HBRUSH,
    field_brush: HBRUSH,
    error_brush: HBRUSH,
    error_field: Option<i32>,
    dpi: u32,
    view: View,
    editing: Option<String>,
    selected: Option<usize>,
    dirty: bool,
    loading: bool,
}

thread_local! {
    static MANAGE: RefCell<Option<Manage>> = const { RefCell::new(None) };
}

static QUIT_ON_CLOSE: AtomicBool = AtomicBool::new(false);

fn scaled(value: i32, dpi: u32) -> i32 {
    (value * dpi as i32 + 48) / 96
}

fn make_font(size: i32, weight: i32, dpi: u32) -> HFONT {
    let height = -scaled(size, dpi);
    let face = platform::wide("Microsoft YaHei UI");
    unsafe {
        CreateFontW(
            height,
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

fn status_message(manage: &Manage, text: &str) {
    set_text(manage.status, text);
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

fn active_rows(
    category: Option<&str>,
    query: &str,
    recycle: bool,
) -> Vec<(String, String, String)> {
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
            (
                snippet.id.clone(),
                snippet.title.clone(),
                name_of(snippet.category_id.as_deref()),
            )
        })
        .collect()
}

fn reload_left(manage: &Manage) {
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
    let mut items: Vec<(String, View)> = vec![(format!("全部 ({})", active.len()), View::All)];
    for category in &data.categories {
        let count = active
            .iter()
            .filter(|snippet| snippet.category_id.as_deref() == Some(category.id.as_str()))
            .count();
        items.push((
            format!("{} ({count})", category.name),
            View::Category(category.id.clone()),
        ));
    }
    items.push((format!("回收站 ({deleted})"), View::Recycle));
    for (label, _) in &items {
        let wide = platform::wide(label);
        unsafe {
            let _ = SendMessageW(manage.left, LB_ADDSTRING, 0, wide.as_ptr() as isize);
        }
    }
    let index = items
        .iter()
        .position(|(_, view)| *view == manage.view)
        .unwrap_or(0);
    unsafe {
        let _ = SendMessageW(manage.left, LB_SETCURSEL, index, 0);
    }
}

fn reload_categories(manage: &Manage) {
    let Some(data) = data() else {
        return;
    };
    let previous = unsafe { SendMessageW(manage.category, CB_GETCURSEL, 0, 0) }.0 as i32;
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
    let index = if previous >= 0 {
        previous.min(data.categories.len() as i32)
    } else {
        0
    };
    unsafe {
        let _ = SendMessageW(manage.category, CB_SETCURSEL, index as usize, 0);
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
        let _ = ShowWindow(
            manage.restore_button,
            if recycle { SW_SHOW } else { SW_HIDE },
        );
        let _ = SendMessageW(manage.list, LB_RESETCONTENT, 0, 0);
    }
    for (_, title, _) in &rows {
        let wide = platform::wide(title);
        unsafe {
            let _ = SendMessageW(manage.list, LB_ADDSTRING, 0, wide.as_ptr() as isize);
        }
    }

    // Remember ids and categories per list position for selection handling.
    LIST_IDS.with(|slot| *slot.borrow_mut() = rows.iter().map(|(id, _, _)| id.clone()).collect());
    LIST_CATEGORIES.with(|slot| {
        *slot.borrow_mut() = rows
            .iter()
            .map(|(_, _, category)| category.clone())
            .collect()
    });

    if manage.selected.is_some() {
        if let Some(position) = rows
            .iter()
            .position(|(id, _, _)| manage.editing.as_deref() == Some(id.as_str()))
        {
            unsafe {
                let _ = SendMessageW(manage.list, LB_SETCURSEL, position, 0);
            }
        }
    }
    if rows.is_empty() {
        manage.selected = None;
    }
}

thread_local! {
    static LIST_IDS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    static LIST_CATEGORIES: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

fn load_editor(manage: &mut Manage, id: Option<&str>) {
    manage.loading = true;
    if manage.error_field.take().is_some() {
        unsafe {
            let _ = InvalidateRect(Some(manage.hwnd), None, true);
        }
    }
    let snippet = id.and_then(|id| {
        data().and_then(|data| data.snippets.into_iter().find(|snippet| snippet.id == id))
    });
    // `?` would skip the reset at the end and leave `loading` stuck on.
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
                let _ = SendMessageW(
                    manage.pinned,
                    BM_SETCHECK,
                    if snippet.pinned { 1 } else { 0 },
                    0,
                );
            }
            manage.editing = Some(snippet.id);
            manage.dirty = false;
            status_message(manage, "已加载");
        }
        None => {
            set_text(manage.title_edit, "");
            set_text(manage.content_edit, "");
            unsafe {
                let _ = SendMessageW(manage.category, CB_SETCURSEL, 0, 0);
                let _ = SendMessageW(manage.pinned, BM_SETCHECK, 0, 0);
            }
            manage.editing = None;
            manage.dirty = false;
            status_message(manage, "新建短语");
        }
    }
    manage.loading = false;
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
    let category = current_category(manage);
    let pinned = unsafe { SendMessageW(manage.pinned, BM_GETCHECK, 0, 0) }.0 == 1;

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
            logging::info("manage: saved snippet");
            reload_left(manage);
            reload_categories(manage);
            reload_list(manage);
            // Reload the editor so the category combo matches the saved row.
            load_editor(manage, Some(&id));
            status_message(manage, "已保存");
        }
        Some(Err(code)) => {
            let message = error_text(&code);
            status_message(manage, message);
            manage.error_field = match code.as_str() {
                "title_empty" | "title_too_long" | "duplicate_title" => Some(ID_TITLE),
                "content_empty" | "content_too_long" => Some(ID_CONTENT),
                "category_missing" => Some(ID_CATEGORY),
                _ => None,
            };
            if manage.error_field.is_some() {
                unsafe {
                    let _ = InvalidateRect(Some(manage.hwnd), None, true);
                }
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
    let recycle = manage.view == View::Recycle;
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
            status_message(
                manage,
                if recycle {
                    "已彻底删除"
                } else {
                    "已移入回收站"
                },
            );
            load_editor(manage, None);
            reload_list(manage);
            reload_left(manage);
        }
        Some(Err(code)) => status_message(manage, error_text(&code)),
        None => status_message(manage, "存储未初始化"),
    }
}

fn draw_list_row(
    draw: &DRAWITEMSTRUCT,
    palette: crate::theme::Palette,
    dpi: u32,
    fonts: (HFONT, HFONT, HFONT),
    two_line: bool,
) {
    let hdc = draw.hDC;
    let rect = draw.rcItem;
    unsafe {
        let brush = CreateSolidBrush(palette.card);
        FillRect(hdc, &rect, brush);
        let _ = DeleteObject(HGDIOBJ(brush.0));
    }
    if draw.itemID == u32::MAX {
        return;
    }
    let selected = draw.itemState.0 & ODS_SELECTED.0 != 0;
    if selected {
        let mut pill = rect;
        pill.left += scaled(4, dpi);
        pill.top += scaled(4, dpi);
        pill.right -= scaled(4, dpi);
        pill.bottom -= scaled(4, dpi);
        let brush = unsafe { CreateSolidBrush(palette.selection) };
        ui::fill_round(hdc, &pill, scaled(8, dpi), brush);
        unsafe {
            let _ = DeleteObject(HGDIOBJ(brush.0));
        }
    }
    let mut buffer = [0u16; 256];
    let length = unsafe {
        SendMessageW(
            draw.hwndItem,
            LB_GETTEXT,
            draw.itemID as usize,
            buffer.as_mut_ptr() as isize,
        )
    }
    .0;
    let title = String::from_utf16_lossy(&buffer[..length.max(0) as usize]);
    let mut text_rect = rect;
    text_rect.left += scaled(12, dpi);
    text_rect.right -= scaled(10, dpi);
    let title_color = if selected {
        palette.primary
    } else {
        palette.heading
    };
    if two_line {
        let category = LIST_CATEGORIES.with(|slot| {
            slot.borrow()
                .get(draw.itemID as usize)
                .cloned()
                .unwrap_or_default()
        });
        let mut title_rect = RECT {
            top: rect.top + scaled(6, dpi),
            bottom: rect.top + scaled(28, dpi),
            ..text_rect
        };
        ui::label(
            hdc,
            &title,
            &mut title_rect,
            fonts.1,
            title_color,
            DT_LEFT | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS,
        );
        if !category.is_empty() {
            let mut category_rect = RECT {
                top: rect.top + scaled(28, dpi),
                bottom: rect.bottom - scaled(4, dpi),
                ..text_rect
            };
            ui::label(
                hdc,
                &category,
                &mut category_rect,
                fonts.2,
                palette.secondary,
                DT_LEFT | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS,
            );
        }
    } else {
        ui::label(
            hdc,
            &title,
            &mut text_rect,
            fonts.0,
            title_color,
            DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS,
        );
    }
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
    let margin = scaled(MARGIN, manage.dpi);
    let gap = scaled(14, manage.dpi);
    let row = scaled(ROW, manage.dpi);
    let left_width = scaled(LEFT_WIDTH, manage.dpi);
    let middle_width = scaled(MIDDLE_WIDTH, manage.dpi).min(
        (width - margin * 2 - left_width - gap * 2 - scaled(220, manage.dpi))
            .max(scaled(200, manage.dpi)),
    );
    let card_top = margin;
    let card_height = (height - margin * 2).max(1);
    let left_card = RECT {
        left: margin,
        top: card_top,
        right: margin + left_width,
        bottom: card_top + card_height,
    };
    let middle_card = RECT {
        left: left_card.right + gap,
        top: card_top,
        right: left_card.right + gap + middle_width,
        bottom: card_top + card_height,
    };
    let right_card = RECT {
        left: middle_card.right + gap,
        top: card_top,
        right: width - margin,
        bottom: card_top + card_height,
    };
    manage.cards = [left_card, middle_card, right_card];

    let pad = scaled(14, manage.dpi);
    let header = scaled(40, manage.dpi);

    // Left card: category navigation.
    let left_inner = left_card.left + pad;
    let left_inner_width = (left_card.right - left_card.left - pad * 2).max(1);
    let cat_delete_top = left_card.bottom - pad - row;
    let cat_name_top = cat_delete_top - scaled(8, manage.dpi) - row;
    let list_top = left_card.top + header;
    unsafe {
        let _ = MoveWindow(
            manage.left,
            left_inner,
            list_top,
            left_inner_width,
            (cat_name_top - scaled(10, manage.dpi) - list_top).max(1),
            true,
        );
        let _ = MoveWindow(
            manage.cat_name,
            left_inner,
            cat_name_top,
            (left_inner_width - scaled(68, manage.dpi)).max(1),
            row,
            true,
        );
        let _ = MoveWindow(
            manage.cat_add,
            left_inner + left_inner_width - scaled(64, manage.dpi),
            cat_name_top,
            scaled(64, manage.dpi),
            row,
            true,
        );
        let _ = MoveWindow(
            manage.cat_delete,
            left_inner,
            cat_delete_top,
            left_inner_width,
            row,
            true,
        );
    }

    // Middle card: search, list and actions.
    let middle_inner = middle_card.left + pad;
    let middle_inner_width = (middle_card.right - middle_card.left - pad * 2).max(1);
    let search_top = middle_card.top + header;
    let actions_top = middle_card.bottom - pad - row;
    unsafe {
        let _ = MoveWindow(
            manage.search,
            middle_inner,
            search_top,
            middle_inner_width,
            row,
            true,
        );
    }
    let list_top = search_top + row + scaled(10, manage.dpi);
    unsafe {
        let _ = MoveWindow(
            manage.list,
            middle_inner,
            list_top,
            middle_inner_width,
            (actions_top - scaled(12, manage.dpi) - list_top).max(1),
            true,
        );
    }
    let button_width = scaled(84, manage.dpi);
    let button_gap = scaled(8, manage.dpi);
    for (index, control) in [
        manage.new_button,
        manage.delete_button,
        manage.restore_button,
    ]
    .into_iter()
    .enumerate()
    {
        unsafe {
            let _ = MoveWindow(
                control,
                middle_inner + (button_width + button_gap) * index as i32,
                actions_top,
                button_width,
                row,
                true,
            );
        }
    }

    // Right card: title, body, metadata and save actions.
    let right_inner = right_card.left + pad;
    let right_inner_width = (right_card.right - right_card.left - pad * 2).max(1);
    let title_label_top = right_card.top + pad;
    let title_top = title_label_top + scaled(20, manage.dpi);
    unsafe {
        let _ = MoveWindow(
            manage.title_edit,
            right_inner,
            title_top,
            right_inner_width,
            scaled(34, manage.dpi),
            true,
        );
    }
    let content_label_top = title_top + scaled(34, manage.dpi) + scaled(10, manage.dpi);
    let content_top = content_label_top + scaled(20, manage.dpi);
    let actions_top = right_card.bottom - pad - row;
    // Status gets its own line above the buttons: at the 800 px minimum width
    // the right card is too narrow to share a row with them.
    let status_top = actions_top - scaled(24, manage.dpi);
    let meta_top = status_top - scaled(8, manage.dpi) - scaled(30, manage.dpi);
    unsafe {
        let _ = MoveWindow(
            manage.content_edit,
            right_inner,
            content_top,
            right_inner_width,
            (meta_top - scaled(10, manage.dpi) - content_top).max(1),
            true,
        );
    }
    let pinned_width = scaled(100, manage.dpi);
    // Never let the combo overlap the pinned checkbox: at the minimum window
    // width the remaining space is only ~94 logical px.
    let combo_width = (right_inner_width - pinned_width - scaled(8, manage.dpi))
        .clamp(scaled(80, manage.dpi), scaled(220, manage.dpi));
    unsafe {
        let _ = MoveWindow(
            manage.category,
            right_inner,
            meta_top,
            combo_width,
            ui::combo_height(manage.category, manage.dpi),
            true,
        );
        let _ = MoveWindow(
            manage.pinned,
            right_inner + right_inner_width - pinned_width,
            meta_top + scaled(4, manage.dpi),
            pinned_width,
            row,
            true,
        );
        let _ = MoveWindow(
            manage.save_button,
            right_inner + right_inner_width - scaled(176, manage.dpi),
            actions_top,
            scaled(84, manage.dpi),
            row,
            true,
        );
        let _ = MoveWindow(
            manage.cancel_button,
            right_inner + right_inner_width - scaled(84, manage.dpi),
            actions_top,
            scaled(84, manage.dpi),
            row,
            true,
        );
        let _ = MoveWindow(
            manage.status,
            right_inner,
            status_top,
            right_inner_width,
            scaled(20, manage.dpi),
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

fn create(owner: HWND) -> Option<Manage> {
    let module = match unsafe { GetModuleHandleW(None) } {
        Ok(module) => module,
        Err(err) => {
            logging::error(&format!("manage: GetModuleHandleW failed: {err}"));
            return None;
        }
    };
    let instance = HINSTANCE(module.0);
    let palette = crate::theme::current();
    let page_brush = unsafe { CreateSolidBrush(palette.page) };
    let card_brush = unsafe { CreateSolidBrush(palette.card) };
    let field_brush = unsafe { CreateSolidBrush(palette.field) };
    let error_brush = unsafe { CreateSolidBrush(palette.error) };
    let class = WNDCLASSW {
        style: Default::default(),
        lpfnWndProc: Some(window_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: instance,
        hIcon: Default::default(),
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
                let _ = DeleteObject(HGDIOBJ(page_brush.0));
                let _ = DeleteObject(HGDIOBJ(card_brush.0));
                let _ = DeleteObject(HGDIOBJ(field_brush.0));
                let _ = DeleteObject(HGDIOBJ(error_brush.0));
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
            WS_OVERLAPPEDWINDOW,
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
                let _ = DeleteObject(HGDIOBJ(page_brush.0));
                let _ = DeleteObject(HGDIOBJ(card_brush.0));
                let _ = DeleteObject(HGDIOBJ(field_brush.0));
                let _ = DeleteObject(HGDIOBJ(error_brush.0));
            }
            return None;
        }
    };

    ui::apply_window_style(hwnd);
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    let dpi = if dpi == 0 { 96 } else { dpi };
    let fonts = Fonts {
        body: make_font(14, 400, dpi),
        title: make_font(16, 600, dpi),
        small: make_font(11, 400, dpi),
    };

    let left = create_control(
        w!("LISTBOX"),
        "",
        WS_CHILD
            | WS_VISIBLE
            | WS_VSCROLL
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
        "删除分类",
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(BS_OWNERDRAW as u32),
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
            | WS_VSCROLL
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
        "删除",
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(BS_OWNERDRAW as u32),
        hwnd,
        ID_DELETE,
        instance,
    );
    let restore_button = create_control(
        w!("BUTTON"),
        "恢复",
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(BS_OWNERDRAW as u32),
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
            | WS_VSCROLL
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
        "置顶",
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(BS_AUTOCHECKBOX as u32),
        hwnd,
        ID_PINNED,
        instance,
    );
    let save_button = create_control(
        w!("BUTTON"),
        "保存",
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(BS_OWNERDRAW as u32),
        hwnd,
        ID_SAVE,
        instance,
    );
    let cancel_button = create_control(
        w!("BUTTON"),
        "取消",
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(BS_OWNERDRAW as u32),
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

    for button in [
        cat_add,
        cat_delete,
        new_button,
        delete_button,
        restore_button,
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
    ui::theme_scrollbar(content_edit, palette.dark);
    ui::cue_banner(search, "搜索标题或正文");
    ui::cue_banner(title_edit, "标题（必填）");
    ui::cue_banner(cat_name, "新分类名称");
    ui::cue_banner(content_edit, "在此输入短语正文");
    unsafe {
        let _ = SendMessageW(left, LB_SETITEMHEIGHT, 0, scaled(34, dpi) as isize);
        let _ = SendMessageW(list, LB_SETITEMHEIGHT, 0, scaled(50, dpi) as isize);
        let _ = SendMessageW(
            category,
            CB_SETITEMHEIGHT,
            usize::MAX,
            scaled(31, dpi) as isize,
        );
        let _ = SendMessageW(category, CB_SETITEMHEIGHT, 0, scaled(28, dpi) as isize);
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
        status,
        cards: [RECT::default(); 3],
        fonts,
        palette,
        page_brush,
        card_brush,
        field_brush,
        error_brush,
        error_field: None,
        dpi,
        view: View::All,
        editing: None,
        selected: None,
        dirty: false,
        loading: true,
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
        let _ = SendMessageW(status, WM_SETFONT, manage.fonts.body.0 as usize, 1);
    }

    let mut rect = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut rect);
    }
    layout(&mut manage, rect.right, rect.bottom);
    reload_left(&manage);
    reload_categories(&manage);
    reload_list(&mut manage);
    load_editor(&mut manage, None);
    // The category combo height depends on its item count, so it can only be
    // sized after the categories have been loaded.
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

    // ShowWindow dispatches WM_SIZE synchronously; do it outside the borrow.
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
        let palette = crate::theme::current();
        if palette != manage.palette {
            unsafe {
                let _ = DeleteObject(HGDIOBJ(manage.page_brush.0));
                let _ = DeleteObject(HGDIOBJ(manage.card_brush.0));
                let _ = DeleteObject(HGDIOBJ(manage.field_brush.0));
                let _ = DeleteObject(HGDIOBJ(manage.error_brush.0));
            }
            manage.page_brush = unsafe { CreateSolidBrush(palette.page) };
            manage.card_brush = unsafe { CreateSolidBrush(palette.card) };
            manage.field_brush = unsafe { CreateSolidBrush(palette.field) };
            manage.error_brush = unsafe { CreateSolidBrush(palette.error) };
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
            unsafe {
                let _ = InvalidateRect(Some(manage.hwnd), None, true);
            }
        }
        unsafe {
            let _ = SetFocus(Some(manage.search));
        }
        reload_left(manage);
        reload_categories(manage);
        reload_list(manage);
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

/// Let Tab/Enter traverse the management controls like a dialog.
pub fn is_dialog_message(message: &MSG) -> bool {
    let hwnd = MANAGE.with(|slot| {
        slot.try_borrow()
            .ok()
            .and_then(|guard| guard.as_ref().map(|manage| manage.hwnd))
    });
    match hwnd {
        Some(hwnd) if !hwnd.0.is_null() => unsafe { IsDialogMessageW(hwnd, message).as_bool() },
        _ => false,
    }
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
        let _ = DeleteObject(HGDIOBJ(manage.fonts.small.0));
        let _ = DeleteObject(HGDIOBJ(manage.page_brush.0));
        let _ = DeleteObject(HGDIOBJ(manage.card_brush.0));
        let _ = DeleteObject(HGDIOBJ(manage.field_brush.0));
        let _ = DeleteObject(HGDIOBJ(manage.error_brush.0));
    }
}

/// True when the user may leave (no unsaved edits, or they accepted discarding).
pub fn confirm_exit() -> bool {
    MANAGE.with(|slot| match slot.try_borrow() {
        Ok(guard) => match guard.as_ref() {
            Some(manage) if manage.dirty => confirm_discard(manage.hwnd),
            _ => true,
        },
        Err(_) => {
            // A modal dialog owns the state; refusing is safer than exiting
            // without confirming.
            logging::warn("manage: exit blocked while a dialog is open");
            false
        }
    })
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
                let mut guard = match slot.try_borrow_mut() {
                    Ok(guard) => guard,
                    Err(_) => return,
                };
                if let Some(manage) = guard.as_mut() {
                    layout(manage, width, height);
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
                let mut guard = match slot.try_borrow_mut() {
                    Ok(guard) => guard,
                    Err(_) => return,
                };
                let Some(manage) = guard.as_mut() else {
                    return;
                };
                unsafe {
                    let _ = DeleteObject(HGDIOBJ(manage.fonts.body.0));
                    let _ = DeleteObject(HGDIOBJ(manage.fonts.title.0));
                    let _ = DeleteObject(HGDIOBJ(manage.fonts.small.0));
                }
                manage.fonts = Fonts {
                    body: make_font(14, 400, dpi),
                    title: make_font(16, 600, dpi),
                    small: make_font(11, 400, dpi),
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
                    manage.status,
                ] {
                    unsafe {
                        let _ = SendMessageW(control, WM_SETFONT, manage.fonts.body.0 as usize, 1);
                    }
                }
                unsafe {
                    let _ = SendMessageW(
                        manage.title_edit,
                        WM_SETFONT,
                        manage.fonts.title.0 as usize,
                        1,
                    );
                    let _ =
                        SendMessageW(manage.left, LB_SETITEMHEIGHT, 0, scaled(34, dpi) as isize);
                    let _ =
                        SendMessageW(manage.list, LB_SETITEMHEIGHT, 0, scaled(50, dpi) as isize);
                    let _ = SendMessageW(
                        manage.category,
                        CB_SETITEMHEIGHT,
                        usize::MAX,
                        scaled(31, dpi) as isize,
                    );
                    let _ = SendMessageW(
                        manage.category,
                        CB_SETITEMHEIGHT,
                        0,
                        scaled(28, dpi) as isize,
                    );
                }
                layout(manage, width, height);
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
        WM_COMMAND => {
            let id = wparam.0 & 0xffff;
            let notification = ((wparam.0 >> 16) & 0xffff) as u32;
            MANAGE.with(|slot| {
                let mut guard = match slot.try_borrow_mut() {
                    Ok(guard) => guard,
                    Err(_) => return,
                };
                let Some(manage) = guard.as_mut() else {
                    return;
                };
                match id as i32 {
                    ID_SEARCH if notification == EN_CHANGE => {
                        if !manage.loading {
                            reload_list(manage);
                        }
                    }
                    ID_LEFT if notification == LBN_SELCHANGE => {
                        let index =
                            unsafe { SendMessageW(manage.left, LB_GETCURSEL, 0, 0) }.0 as i32;
                        let Some(data) = data() else {
                            return;
                        };
                        let view = if index <= 0 {
                            View::All
                        } else {
                            let category_index = index as usize - 1;
                            if category_index < data.categories.len() {
                                View::Category(data.categories[category_index].id.clone())
                            } else {
                                View::Recycle
                            }
                        };
                        if manage.dirty && !confirm_discard(hwnd) {
                            let previous = match &manage.view {
                                View::All => 0,
                                View::Category(id) => data
                                    .categories
                                    .iter()
                                    .position(|category| &category.id == id)
                                    .map(|value| value + 1)
                                    .unwrap_or(0),
                                View::Recycle => data.categories.len() + 1,
                            };
                            unsafe {
                                let _ = SendMessageW(manage.left, LB_SETCURSEL, previous, 0);
                            }
                            return;
                        }
                        manage.view = view;
                        manage.editing = None;
                        manage.dirty = false;
                        load_editor(manage, None);
                        reload_list(manage);
                    }
                    ID_LIST if notification == LBN_SELCHANGE => {
                        let index = unsafe { SendMessageW(manage.list, LB_GETCURSEL, 0, 0) }.0;
                        if index < 0 {
                            return;
                        }
                        let id = LIST_IDS.with(|slot| slot.borrow().get(index as usize).cloned());
                        let Some(id) = id else {
                            return;
                        };
                        if manage.editing.as_deref() == Some(id.as_str()) {
                            return;
                        }
                        if manage.dirty && !confirm_discard(hwnd) {
                            // Restore the highlighted row to the item still open;
                            // an unsaved new snippet clears the selection.
                            let previous = LIST_IDS.with(|slot| {
                                slot.borrow().iter().position(|value| {
                                    Some(value.as_str()) == manage.editing.as_deref()
                                })
                            });
                            unsafe {
                                let _ = SendMessageW(
                                    manage.list,
                                    LB_SETCURSEL,
                                    previous.unwrap_or(usize::MAX),
                                    0,
                                );
                            }
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
                        let category = match &manage.view {
                            View::Category(id) => Some(id.clone()),
                            _ => None,
                        };
                        let Some(category) = category else {
                            status_message(manage, "请先在左侧选择要删除的分类");
                            return;
                        };
                        if manage.dirty && !confirm_discard(hwnd) {
                            return;
                        }
                        let text = platform::wide("删除分类后，其短语会移到未分类。继续吗？");
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
                        match storage::with_store(|store| store.delete_category(&category)) {
                            Some(Ok(())) => {
                                manage.view = View::All;
                                manage.editing = None;
                                manage.dirty = false;
                                status_message(manage, "已删除分类");
                                load_editor(manage, None);
                                reload_left(manage);
                                reload_categories(manage);
                                reload_list(manage);
                                relayout(manage);
                            }
                            Some(Err(code)) => status_message(manage, error_text(&code)),
                            None => status_message(manage, "存储未初始化"),
                        }
                    }
                    ID_NEW => {
                        if manage.view == View::Recycle {
                            status_message(manage, "回收站内不能新建，请先恢复或切换分类");
                            return;
                        }
                        if manage.dirty && !confirm_discard(hwnd) {
                            return;
                        }
                        manage.selected = None;
                        load_editor(manage, None);
                        unsafe {
                            let _ = SetFocus(Some(manage.title_edit));
                        }
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
                                status_message(manage, "已恢复");
                                reload_list(manage);
                                load_editor(manage, None);
                                reload_left(manage);
                            }
                            Some(Err(code)) => status_message(manage, error_text(&code)),
                            None => status_message(manage, "存储未初始化"),
                        }
                    }
                    ID_SAVE => {
                        if manage.view == View::Recycle {
                            status_message(manage, "回收站内不能保存，请先恢复");
                            return;
                        }
                        save_editor(manage);
                    }
                    ID_CANCEL => {
                        let id = manage.editing.clone();
                        load_editor(manage, id.as_deref());
                    }
                    ID_TITLE | ID_CONTENT if notification == EN_CHANGE => {
                        if manage.error_field == Some(id as i32) {
                            manage.error_field = None;
                            unsafe {
                                let _ = InvalidateRect(Some(manage.hwnd), None, true);
                            }
                        }
                        if !manage.loading {
                            manage.dirty = true;
                            status_message(manage, "未保存");
                        }
                    }
                    ID_CATEGORY if notification == CBN_SELCHANGE => {
                        if manage.error_field == Some(ID_CATEGORY) {
                            manage.error_field = None;
                            unsafe {
                                let _ = InvalidateRect(Some(manage.hwnd), None, true);
                            }
                        }
                        if !manage.loading {
                            manage.dirty = true;
                            status_message(manage, "未保存");
                        }
                    }
                    ID_PINNED if notification == BN_CLICKED => {
                        if !manage.loading {
                            manage.dirty = true;
                            status_message(manage, "未保存");
                        }
                    }
                    _ => {}
                }
            });
            LRESULT(0)
        }
        WM_ERASEBKGND => {
            let hdc = windows::Win32::Graphics::Gdi::HDC(wparam.0 as *mut core::ffi::c_void);
            let mut rect = RECT::default();
            unsafe {
                let _ = GetClientRect(hwnd, &mut rect);
            }
            let brush = MANAGE.with(|slot| match slot.try_borrow() {
                Ok(guard) => guard
                    .as_ref()
                    .map(|manage| manage.page_brush)
                    .unwrap_or_default(),
                Err(_) => HBRUSH::default(),
            });
            if !brush.0.is_null() {
                unsafe {
                    FillRect(hdc, &rect, brush);
                }
                return LRESULT(1);
            }
            LRESULT(0)
        }
        WM_CTLCOLORSTATIC | WM_CTLCOLORLISTBOX => {
            let hdc = windows::Win32::Graphics::Gdi::HDC(wparam.0 as *mut core::ffi::c_void);
            let (heading, secondary, card, field, field_color) =
                MANAGE.with(|slot| match slot.try_borrow() {
                    Ok(guard) => match guard.as_ref() {
                        Some(manage) => (
                            manage.palette.heading,
                            manage.palette.secondary,
                            manage.card_brush,
                            manage.field_brush,
                            manage.palette.field,
                        ),
                        None => (
                            COLORREF(0),
                            COLORREF(0),
                            HBRUSH::default(),
                            HBRUSH::default(),
                            COLORREF(0),
                        ),
                    },
                    Err(_) => (
                        COLORREF(0),
                        COLORREF(0),
                        HBRUSH::default(),
                        HBRUSH::default(),
                        COLORREF(0),
                    ),
                });
            unsafe {
                SetBkMode(hdc, TRANSPARENT);
            }
            if message == WM_CTLCOLORLISTBOX {
                unsafe {
                    SetBkColor(hdc, field_color);
                    SetTextColor(hdc, heading);
                }
                return LRESULT(field.0 as isize);
            }
            unsafe {
                SetTextColor(hdc, secondary);
            }
            if !card.0.is_null() {
                return LRESULT(card.0 as isize);
            }
            LRESULT(0)
        }
        WM_CTLCOLOREDIT => {
            let hdc = windows::Win32::Graphics::Gdi::HDC(wparam.0 as *mut core::ffi::c_void);
            let (heading, field, field_color) = MANAGE.with(|slot| match slot.try_borrow() {
                Ok(guard) => match guard.as_ref() {
                    Some(manage) => (
                        manage.palette.heading,
                        manage.field_brush,
                        manage.palette.field,
                    ),
                    None => (COLORREF(0), HBRUSH::default(), COLORREF(0)),
                },
                Err(_) => (COLORREF(0), HBRUSH::default(), COLORREF(0)),
            });
            unsafe {
                SetBkMode(hdc, TRANSPARENT);
                SetBkColor(hdc, field_color);
                SetTextColor(hdc, heading);
            }
            LRESULT(field.0 as isize)
        }
        WM_DRAWITEM => {
            let draw = &*(lparam.0 as *const DRAWITEMSTRUCT);
            let (palette, fonts, dpi) = MANAGE.with(|slot| match slot.try_borrow() {
                Ok(guard) => match guard.as_ref() {
                    Some(manage) => (
                        manage.palette,
                        (manage.fonts.body, manage.fonts.title, manage.fonts.small),
                        manage.dpi,
                    ),
                    None => (
                        crate::theme::LIGHT,
                        (HFONT::default(), HFONT::default(), HFONT::default()),
                        96,
                    ),
                },
                Err(_) => (
                    crate::theme::LIGHT,
                    (HFONT::default(), HFONT::default(), HFONT::default()),
                    96,
                ),
            });
            match draw.CtlID as i32 {
                ID_LEFT => draw_list_row(draw, palette, dpi, fonts, false),
                ID_LIST => draw_list_row(draw, palette, dpi, fonts, true),
                ID_CATEGORY => ui::draw_combo_item(draw, palette, dpi, fonts.0),
                ID_SAVE => {
                    ui::draw_button(draw, palette, ui::ButtonKind::Primary, dpi, palette.card)
                }
                ID_DELETE | ID_CAT_DELETE => {
                    ui::draw_button(draw, palette, ui::ButtonKind::Danger, dpi, palette.card)
                }
                _ => ui::draw_button(draw, palette, ui::ButtonKind::Secondary, dpi, palette.card),
            }
            LRESULT(1)
        }
        WM_PAINT => {
            let mut paint = PAINTSTRUCT::default();
            unsafe {
                let hdc = BeginPaint(hwnd, &mut paint);
                let (cards, palette, fonts, dpi) = MANAGE.with(|slot| match slot.try_borrow() {
                    Ok(guard) => match guard.as_ref() {
                        Some(manage) => (
                            manage.cards,
                            manage.palette,
                            (manage.fonts.small, manage.fonts.body),
                            manage.dpi,
                        ),
                        None => (
                            [RECT::default(); 3],
                            crate::theme::LIGHT,
                            (HFONT::default(), HFONT::default()),
                            96,
                        ),
                    },
                    Err(_) => (
                        [RECT::default(); 3],
                        crate::theme::LIGHT,
                        (HFONT::default(), HFONT::default()),
                        96,
                    ),
                });
                for card in &cards {
                    if card.right > card.left && card.bottom > card.top {
                        ui::card(hdc, card, palette, dpi);
                    }
                }
                let pad = scaled(14, dpi);
                let mut header = RECT {
                    left: cards[0].left + pad,
                    top: cards[0].top + scaled(12, dpi),
                    right: cards[0].right - pad,
                    bottom: cards[0].top + scaled(34, dpi),
                };
                ui::label(
                    hdc,
                    "分类",
                    &mut header,
                    fonts.0,
                    palette.secondary,
                    DT_LEFT | DT_SINGLELINE | DT_NOPREFIX,
                );
                let mut header = RECT {
                    left: cards[1].left + pad,
                    top: cards[1].top + scaled(12, dpi),
                    right: cards[1].right - pad,
                    bottom: cards[1].top + scaled(34, dpi),
                };
                ui::label(
                    hdc,
                    "短语",
                    &mut header,
                    fonts.0,
                    palette.secondary,
                    DT_LEFT | DT_SINGLELINE | DT_NOPREFIX,
                );
                let right_card = cards[2];
                let mut title_label = RECT {
                    left: right_card.left + pad,
                    top: right_card.top + scaled(14, dpi),
                    right: right_card.right - pad,
                    bottom: right_card.top + scaled(32, dpi),
                };
                ui::label(
                    hdc,
                    "标题",
                    &mut title_label,
                    fonts.0,
                    palette.secondary,
                    DT_LEFT | DT_SINGLELINE | DT_NOPREFIX,
                );
                let mut content_label = RECT {
                    left: right_card.left + pad,
                    top: right_card.top + scaled(78, dpi),
                    right: right_card.right - pad,
                    bottom: right_card.top + scaled(96, dpi),
                };
                ui::label(
                    hdc,
                    "正文",
                    &mut content_label,
                    fonts.0,
                    palette.secondary,
                    DT_LEFT | DT_SINGLELINE | DT_NOPREFIX,
                );
                let (error_field, error_brush) = MANAGE.with(|slot| match slot.try_borrow() {
                    Ok(guard) => match guard.as_ref() {
                        Some(manage) => (manage.error_field, manage.error_brush),
                        None => (None, HBRUSH::default()),
                    },
                    Err(_) => (None, HBRUSH::default()),
                });
                if let Some(id) = error_field {
                    if let Ok(control) = GetDlgItem(Some(hwnd), id) {
                        let mut rect = RECT::default();
                        if GetWindowRect(control, &mut rect).is_ok() && !error_brush.0.is_null() {
                            let mut top_left = POINT {
                                x: rect.left,
                                y: rect.top,
                            };
                            let mut bottom_right = POINT {
                                x: rect.right,
                                y: rect.bottom,
                            };
                            let _ = ScreenToClient(hwnd, &mut top_left);
                            let _ = ScreenToClient(hwnd, &mut bottom_right);
                            let dpi = MANAGE.with(|slot| {
                                slot.try_borrow()
                                    .ok()
                                    .and_then(|guard| guard.as_ref().map(|manage| manage.dpi))
                                    .unwrap_or(96)
                            });
                            let thick = scaled(2, dpi);
                            // Four solid bands: FrameRect would draw 1 px
                            // regardless of DPI.
                            let bands = [
                                RECT {
                                    left: top_left.x - thick,
                                    top: top_left.y - thick,
                                    right: bottom_right.x + thick,
                                    bottom: top_left.y,
                                },
                                RECT {
                                    left: top_left.x - thick,
                                    top: bottom_right.y,
                                    right: bottom_right.x + thick,
                                    bottom: bottom_right.y + thick,
                                },
                                RECT {
                                    left: top_left.x - thick,
                                    top: top_left.y,
                                    right: top_left.x,
                                    bottom: bottom_right.y,
                                },
                                RECT {
                                    left: bottom_right.x,
                                    top: top_left.y,
                                    right: bottom_right.x + thick,
                                    bottom: bottom_right.y,
                                },
                            ];
                            for band in bands {
                                let _ = FillRect(hdc, &band, error_brush);
                            }
                        }
                    }
                }
                let _ = EndPaint(hwnd, &paint);
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            let (should_close, had_dirty) = MANAGE.with(|slot| match slot.try_borrow() {
                Ok(guard) => match guard.as_ref() {
                    Some(manage) if manage.dirty => (confirm_discard(manage.hwnd), true),
                    Some(_) => (true, false),
                    None => (true, false),
                },
                Err(_) => (true, false),
            });
            if should_close {
                if had_dirty {
                    // Discarding means going back to the persisted content.
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
                    unsafe {
                        let _ = DestroyWindow(hwnd);
                    }
                } else {
                    unsafe {
                        let _ = ShowWindow(hwnd, SW_HIDE);
                    }
                }
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            MANAGE.with(|slot| {
                let mut guard = match slot.try_borrow_mut() {
                    Ok(guard) => guard,
                    Err(_) => return,
                };
                if let Some(manage) = guard.take() {
                    unsafe {
                        let _ = DeleteObject(HGDIOBJ(manage.fonts.body.0));
                        let _ = DeleteObject(HGDIOBJ(manage.fonts.title.0));
                        let _ = DeleteObject(HGDIOBJ(manage.fonts.small.0));
                        let _ = DeleteObject(HGDIOBJ(manage.page_brush.0));
                        let _ = DeleteObject(HGDIOBJ(manage.card_brush.0));
                        let _ = DeleteObject(HGDIOBJ(manage.field_brush.0));
                        let _ = DeleteObject(HGDIOBJ(manage.error_brush.0));
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
