//! QuickPhrase 2.0 three-pane management window (Direct2D + DirectWrite).

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DeleteObject, EndPaint, InvalidateRect, SetBkColor, SetBkMode,
    SetTextColor, HBRUSH, HDC, HFONT, HGDIOBJ, OPAQUE, PAINTSTRUCT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::{
    DRAWITEMSTRUCT, EM_SETLIMITTEXT, ODS_FOCUS, ODS_SELECTED, WM_MOUSELEAVE,
};
use windows::Win32::UI::HiDpi::{GetDpiForSystem, GetDpiForWindow};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    EnableWindow, GetFocus, SetFocus, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT,
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
const ID_TAGS: i32 = 211;
const ID_CAT_NAME: i32 = 212;
const ID_CAT_ADD: i32 = 213;
const ID_CAT_DELETE: i32 = 214;
const ID_RESTORE: i32 = 215;
const ID_HOTKEY: i32 = 216;
const ID_TAG_FILTER: i32 = 217;
const ID_SELECT_ALL: i32 = 218;

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

/// Fields needed to populate the editor for one snippet, copied while the
/// storage read lock is held instead of cloning the whole data file.
struct SnippetFields {
    id: String,
    title: String,
    content: String,
    tags: Vec<String>,
    hotkey: Option<String>,
    category_id: Option<String>,
    pinned: bool,
    deleted: bool,
}

impl SnippetFields {
    fn from_snippet(snippet: &Snippet) -> Self {
        Self {
            id: snippet.id.clone(),
            title: snippet.title.clone(),
            content: snippet.content.clone(),
            tags: snippet.tags.clone(),
            hotkey: snippet.hotkey.clone(),
            category_id: snippet.category_id.clone(),
            pinned: snippet.pinned,
            deleted: snippet.deleted_at.is_some(),
        }
    }
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
    tag_shell: RECT,
    middle_header_bottom: i32,
    recycle_banner: Option<RECT>,
    editor_header: RECT,
    title_label: RECT,
    title_shell: RECT,
    content_label: RECT,
    content_shell: RECT,
    tags_label: RECT,
    tags_shell: RECT,
    hotkey_label: RECT,
    hotkey_shell: RECT,
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
    tag_filter: HWND,
    tag_filter_value: Option<String>,
    tag_options: Vec<String>,
    batch_ids: Vec<String>,
    new_button: HWND,
    select_all_button: HWND,
    delete_button: HWND,
    restore_button: HWND,
    title_edit: HWND,
    content_edit: HWND,
    tags_edit: HWND,
    hotkey_edit: HWND,
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
    title_units: usize,
    content_units: usize,
    content_lines: usize,
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
/// A theme refresh arrived while a modal owned the MANAGE borrow.
static REFRESH_PENDING: AtomicBool = AtomicBool::new(false);

fn scaled(value: i32, dpi: u32) -> i32 {
    (value * dpi as i32 + 48) / 96
}

fn parse_tags(text: &str) -> Vec<String> {
    text.split([',', '，'])
        .map(str::trim)
        .filter(|tag| !tag.is_empty())
        .map(str::to_string)
        .collect()
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

/// Refresh the editor length counters after a load or a user edit. Counts use
/// UTF-16 units, matching the storage validation limits (emoji count as two).
fn refresh_editor_metrics(manage: &mut Manage) {
    let title = wide_text(manage.title_edit);
    let content = wide_text(manage.content_edit);
    manage.title_units = title.encode_utf16().count();
    manage.content_units = content.encode_utf16().count();
    manage.content_lines = if content.is_empty() {
        1
    } else {
        content.lines().count().max(1)
    };
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
        "tag_too_long" => "单个标签不能超过 24 个字符",
        "too_many_tags" => "标签最多 10 个",
        "tag_delimiter" => "标签不能包含逗号",
        "tag_empty" => "请输入至少一个标签",
        "storage_unavailable" => "存储未初始化",
        "hotkey_invalid" => "快捷键格式无效，例如 Ctrl+Alt+1",
        "hotkey_duplicate" => "该快捷键已被其他短语使用",
        "hotkey_panel_conflict" => "与全局唤起快捷键冲突",
        "read_only" => "存储为只读，无法保存",
        "snippet_missing" => "短语不存在",
        _ => "操作失败",
    }
}

fn active_rows(
    category: Option<&str>,
    query: &str,
    recycle: bool,
    tag: Option<&str>,
) -> Vec<PhraseRow> {
    storage::with_data(|data| {
        let (query_tags, text_query) = search::split_tag_query(query);
        let mut wanted: Vec<String> = query_tags;
        if let Some(tag) = tag {
            wanted.push(tag.to_lowercase());
        }
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
                    && wanted.iter().all(|tag| {
                        let tag = tag.to_lowercase();
                        snippet.tags.iter().any(|item| item.to_lowercase() == tag)
                    })
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
                use_count: snippet.use_count,
                last_used_at: snippet.last_used_at.as_deref(),
                updated_at: &snippet.updated_at,
                sort_order: snippet.sort_order,
            })
            .collect();
        search::rank_indices(&docs, text_query, usize::MAX)
            .into_iter()
            .map(|index| {
                let snippet = pool[index];
                PhraseRow {
                    id: snippet.id.clone(),
                    title: snippet.title.clone(),
                    preview: preview_text(&snippet.content),
                    category: name_of(snippet.category_id.as_deref()),
                    pinned: snippet.pinned,
                }
            })
            .collect()
    })
    .unwrap_or_default()
}

/// Single-line list preview: skip CR, map LF to the existing arrow marker and
/// bound the length so a long body cannot allocate a full copy on every list
/// rebuild.
fn preview_text(content: &str) -> String {
    const PREVIEW_CHARS: usize = 160;
    let mut preview = String::new();
    let mut count = 0;
    for ch in content.chars() {
        if ch == '\r' {
            continue;
        }
        if count == PREVIEW_CHARS {
            preview.push('…');
            break;
        }
        if ch == '\n' {
            preview.push_str(" ↵ ");
        } else {
            preview.push(ch);
        }
        count += 1;
    }
    preview
}

fn reload_left(manage: &mut Manage) {
    let Some(mut rows) = storage::with_data(|data| {
        let mut active_total = 0usize;
        let mut deleted = 0usize;
        let mut per_category: std::collections::HashMap<&str, usize> =
            std::collections::HashMap::new();
        for snippet in &data.snippets {
            if snippet.deleted_at.is_some() {
                deleted += 1;
                continue;
            }
            active_total += 1;
            if let Some(id) = snippet.category_id.as_deref() {
                *per_category.entry(id).or_default() += 1;
            }
        }
        let mut rows = vec![NavRow {
            label: "全部短语".to_string(),
            count: active_total,
            view: View::All,
            dot: None,
        }];
        for category in &data.categories {
            rows.push(NavRow {
                label: category.name.clone(),
                count: per_category.get(category.id.as_str()).copied().unwrap_or(0),
                view: View::Category(category.id.clone()),
                dot: None,
            });
        }
        rows.push(NavRow {
            label: "回收站".to_string(),
            count: deleted,
            view: View::Recycle,
            dot: None,
        });
        rows
    }) else {
        return;
    };
    // Theme helpers stay outside the storage borrow, even though this one is a
    // pure hash today.
    for row in &mut rows {
        if matches!(row.view, View::Category(_)) {
            row.dot = Some(theme::category_dot_for_name(&row.label));
        }
    }
    unsafe {
        let _ = SendMessageW(manage.left, LB_RESETCONTENT, 0, 0);
    }
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
        // A category delete can shrink the nav; erase the vacated rows.
        let _ = InvalidateRect(Some(manage.left), None, true);
    }
}

fn reload_categories(manage: &mut Manage) {
    let draft = manage.draft_category.clone();
    let Some((categories, index)) = storage::with_data(|data| {
        let categories: Vec<(String, String)> = data
            .categories
            .iter()
            .map(|category| (category.id.clone(), category.name.clone()))
            .collect();
        // Restore by stable id, not by the old numeric index: an import can
        // reorder or remove categories while an editor draft is still open.
        let index = draft
            .as_deref()
            .and_then(|id| {
                data.categories
                    .iter()
                    .position(|category| category.id == id)
            })
            .map(|position| position + 1);
        (categories, index)
    }) else {
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
    for (_, name) in &categories {
        let wide = platform::wide(name);
        unsafe {
            let _ = SendMessageW(manage.category, CB_ADDSTRING, 0, wide.as_ptr() as isize);
        }
    }
    if index.is_none() && draft.is_some() {
        manage.draft_category = None;
    }
    unsafe {
        let _ = SendMessageW(manage.category, CB_SETCURSEL, index.unwrap_or(0), 0);
    }
}

fn reload_tag_filter(manage: &mut Manage) {
    let recycle = matches!(manage.view, View::Recycle);
    let selected = manage.tag_filter_value.clone();
    let Some((options, index)) = storage::with_data(|data| {
        let mut options: Vec<String> = Vec::new();
        for snippet in &data.snippets {
            if (snippet.deleted_at.is_some()) != recycle {
                continue;
            }
            for tag in &snippet.tags {
                if !options
                    .iter()
                    .any(|existing| existing.to_lowercase() == tag.to_lowercase())
                {
                    options.push(tag.clone());
                }
            }
        }
        options.sort_by_key(|tag| tag.to_lowercase());
        let index = selected
            .as_deref()
            .and_then(|selected| {
                options
                    .iter()
                    .position(|tag| tag.to_lowercase() == selected.to_lowercase())
            })
            .map(|position| position + 1)
            .unwrap_or(0);
        (options, index)
    }) else {
        return;
    };
    manage.loading = true;
    unsafe {
        let _ = SendMessageW(manage.tag_filter, CB_RESETCONTENT, 0, 0);
        let all = platform::wide("全部标签");
        let _ = SendMessageW(manage.tag_filter, CB_ADDSTRING, 0, all.as_ptr() as isize);
        for tag in &options {
            let wide = platform::wide(tag);
            let _ = SendMessageW(manage.tag_filter, CB_ADDSTRING, 0, wide.as_ptr() as isize);
        }
        let _ = SendMessageW(manage.tag_filter, CB_SETCURSEL, index, 0);
        manage.tag_filter_value = if index == 0 {
            None
        } else {
            options.get(index - 1).cloned()
        };
    }
    manage.tag_options = options;
    manage.loading = false;
}

fn apply_tag_filter(manage: &mut Manage) {
    if manage.loading {
        return;
    }
    let index = unsafe { SendMessageW(manage.tag_filter, CB_GETCURSEL, 0, 0) }.0 as usize;
    manage.tag_filter_value = if index == 0 {
        None
    } else {
        manage.tag_options.get(index - 1).cloned()
    };
    if !manage.batch_ids.is_empty() {
        manage.batch_ids.clear();
        load_editor(manage, None);
    }
    reload_list(manage);
    unsafe {
        let _ = InvalidateRect(Some(manage.hwnd), None, false);
    }
}

fn apply_category(manage: &mut Manage) {
    if manage.error_field == Some(ID_CATEGORY) {
        manage.error_field = None;
        manage.error_message = None;
    }
    if !manage.loading {
        manage.draft_category = current_category(manage);
        manage.dirty = true;
        manage.status_text = "未保存修改".to_string();
        unsafe {
            let _ = InvalidateRect(Some(manage.hwnd), None, false);
        }
    }
}

/// Open the custom picker for one of the two combo boxes.
///
/// Called from a queued message, not from the combo's own notification: the
/// native box sets its "open" flag only after CBN_DROPDOWN returns, so running
/// a modal picker inside that notification left the flag stuck and broke the
/// next keyboard open.
fn open_combo_picker(manage: &mut Manage, id: i32) {
    match id {
        ID_CATEGORY => {
            reload_categories(manage);
            let index = unsafe { SendMessageW(manage.category, CB_GETCURSEL, 0, 0) }.0;
            let mut items = vec!["未分类".to_string()];
            items.extend(
                storage::with_data(|data| {
                    data.categories
                        .iter()
                        .map(|category| category.name.clone())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default(),
            );
            if let Some(pick) = crate::dropmenu::show(
                manage.hwnd,
                manage.category,
                &items,
                if index >= 0 {
                    Some(index as usize)
                } else {
                    None
                },
            ) {
                if index < 0 || pick != index as usize {
                    let _ = unsafe { SendMessageW(manage.category, CB_SETCURSEL, pick, 0) };
                    apply_category(manage);
                }
            }
        }
        ID_TAG_FILTER => {
            let previous_filter = manage
                .tag_filter_value
                .as_deref()
                .map(|tag| tag.to_lowercase());
            reload_tag_filter(manage);
            if manage
                .tag_filter_value
                .as_deref()
                .map(|tag| tag.to_lowercase())
                != previous_filter
            {
                // The saved tag disappeared (e.g. an import removed it), so
                // reload_tag_filter reset the combo to "all"; the middle list
                // must follow even if the user cancels the picker.
                apply_tag_filter(manage);
            }
            let index = unsafe { SendMessageW(manage.tag_filter, CB_GETCURSEL, 0, 0) }.0;
            let mut items = vec!["全部标签".to_string()];
            items.extend(manage.tag_options.iter().cloned());
            if let Some(pick) = crate::dropmenu::show(
                manage.hwnd,
                manage.tag_filter,
                &items,
                if index >= 0 {
                    Some(index as usize)
                } else {
                    None
                },
            ) {
                if index < 0 || pick != index as usize {
                    let _ = unsafe { SendMessageW(manage.tag_filter, CB_SETCURSEL, pick, 0) };
                    apply_tag_filter(manage);
                }
            }
        }
        _ => {}
    }
}

fn reload_list(manage: &mut Manage) {
    let query = wide_text(manage.search);
    let (category, recycle) = match &manage.view {
        View::All => (None, false),
        View::Category(id) => (Some(id.as_str()), false),
        View::Recycle => (None, true),
    };
    let rows = active_rows(
        category,
        &query,
        recycle,
        manage.tag_filter_value.as_deref(),
    );
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
            select_list_row(manage, position);
        }
    }
    if manage.phrase_rows.is_empty() {
        manage.selected = None;
    }
    unsafe {
        // Deleting or filtering can shrink the list; repaint the whole list so
        // rows past the new count are erased instead of leaving ghost cards.
        let _ = InvalidateRect(Some(manage.list), None, true);
        let _ = InvalidateRect(Some(manage.hwnd), None, false);
    }
}

fn load_editor(manage: &mut Manage, id: Option<&str>) {
    manage.loading = true;
    manage.batch_ids.clear();
    set_text(manage.save_button, "保存短语");
    set_text(manage.cancel_button, "重置");
    unsafe {
        let _ = EnableWindow(manage.title_edit, true);
        let _ = EnableWindow(manage.content_edit, true);
        let _ = EnableWindow(manage.tags_edit, true);
        let _ = EnableWindow(manage.hotkey_edit, true);
        let _ = EnableWindow(manage.category, true);
        let _ = EnableWindow(manage.pinned, true);
        let _ = ShowWindow(manage.content_edit, SW_SHOW);
    }
    manage.error_field = None;
    manage.error_message = None;

    let view = manage.view.clone();
    let (snippet, categories) = storage::with_data(|data| {
        let snippet = id
            .and_then(|id| data.snippets.iter().find(|snippet| snippet.id == id))
            .map(SnippetFields::from_snippet);
        let categories: Vec<(String, String)> = data
            .categories
            .iter()
            .map(|category| (category.id.clone(), category.name.clone()))
            .collect();
        (snippet, categories)
    })
    .unwrap_or((None, Vec::new()));
    match snippet {
        Some(snippet) => {
            set_text(manage.title_edit, &snippet.title);
            set_text(manage.content_edit, &snippet.content);
            set_text(manage.tags_edit, &snippet.tags.join(", "));
            set_text(manage.hotkey_edit, snippet.hotkey.as_deref().unwrap_or(""));
            let index = snippet
                .category_id
                .as_ref()
                .and_then(|id| {
                    categories
                        .iter()
                        .position(|(category_id, _)| category_id == id)
                })
                .map(|position| position + 1)
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
            manage.editing_deleted = snippet.deleted;
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
            set_text(manage.tags_edit, "");
            set_text(manage.hotkey_edit, "");
            let default_cat = match &view {
                View::Category(cat_id) => categories
                    .iter()
                    .position(|(category_id, _)| category_id == cat_id)
                    .map(|position| position + 1)
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
                categories
                    .get(default_cat - 1)
                    .map(|(category_id, _)| category_id.clone())
            } else {
                None
            };
            manage.editing = None;
            manage.editing_deleted = false;
            manage.dirty = false;
            manage.status_text = "新建短语".to_string();
        }
    }
    refresh_editor_metrics(manage);
    manage.loading = false;
    relayout(manage);
    unsafe {
        let _ = InvalidateRect(Some(manage.hwnd), None, false);
    }
}

fn select_list_row(manage: &mut Manage, position: usize) {
    unsafe {
        let _ = SendMessageW(manage.list, LB_SETSEL, 0, -1);
        let _ = SendMessageW(manage.list, LB_SETSEL, 1, position as isize);
        let _ = SendMessageW(manage.list, LB_SETCURSEL, position, 0);
    }
}

fn selected_indices(manage: &Manage) -> Vec<usize> {
    let count = unsafe { SendMessageW(manage.list, LB_GETSELCOUNT, 0, 0) }.0;
    if count <= 0 {
        return Vec::new();
    }
    let mut buffer = vec![0i32; count as usize];
    let got = unsafe {
        SendMessageW(
            manage.list,
            LB_GETSELITEMS,
            buffer.len(),
            buffer.as_mut_ptr() as isize,
        )
    }
    .0;
    if got < 0 {
        return Vec::new();
    }
    buffer.truncate((got as usize).min(buffer.len()));
    buffer
        .into_iter()
        .filter_map(|index| usize::try_from(index).ok())
        .collect()
}

fn restore_editor_selection(manage: &mut Manage) {
    unsafe {
        let _ = SendMessageW(manage.list, LB_SETSEL, 0, -1);
        if let Some(position) = manage
            .phrase_rows
            .iter()
            .position(|row| Some(row.id.as_str()) == manage.editing.as_deref())
        {
            let _ = SendMessageW(manage.list, LB_SETSEL, 1, position as isize);
            let _ = SendMessageW(manage.list, LB_SETCURSEL, position, 0);
        } else {
            let _ = SendMessageW(manage.list, LB_SETCURSEL, usize::MAX, 0);
        }
    }
}

fn load_batch(manage: &mut Manage, ids: Vec<String>) {
    if ids.is_empty() {
        load_editor(manage, None);
        return;
    }
    manage.loading = true;
    manage.batch_ids = ids.clone();
    manage.editing = None;
    manage.editing_deleted = false;
    manage.dirty = false;
    manage.error_field = None;
    manage.error_message = None;

    let first_id = ids.first().map(String::as_str);
    let (first, categories) = storage::with_data(|data| {
        let first = first_id
            .and_then(|id| data.snippets.iter().find(|snippet| snippet.id == id))
            .map(SnippetFields::from_snippet);
        let categories: Vec<(String, String)> = data
            .categories
            .iter()
            .map(|category| (category.id.clone(), category.name.clone()))
            .collect();
        (first, categories)
    })
    .unwrap_or((None, Vec::new()));
    if let Some(snippet) = first {
        set_text(manage.title_edit, &snippet.title);
        set_text(manage.content_edit, &snippet.content);
        manage.pinned_state = snippet.pinned;
        manage.draft_category = snippet.category_id.clone();
        let index = snippet
            .category_id
            .as_ref()
            .and_then(|id| {
                categories
                    .iter()
                    .position(|(category_id, _)| category_id == id)
            })
            .map(|position| position + 1)
            .unwrap_or(0);
        unsafe {
            let _ = SendMessageW(manage.category, CB_SETCURSEL, index, 0);
            let _ = InvalidateRect(Some(manage.pinned), None, false);
        }
    }
    refresh_editor_metrics(manage);
    set_text(manage.tags_edit, "");
    set_text(manage.hotkey_edit, "");
    set_text(manage.save_button, "添加标签");
    set_text(manage.cancel_button, "移除标签");
    unsafe {
        let _ = EnableWindow(manage.title_edit, false);
        let _ = EnableWindow(manage.content_edit, false);
        let _ = EnableWindow(manage.hotkey_edit, false);
        let _ = EnableWindow(manage.category, false);
        let _ = EnableWindow(manage.pinned, false);
        let _ = EnableWindow(manage.tags_edit, true);
        let _ = ShowWindow(manage.delete_button, SW_HIDE);
        let _ = ShowWindow(manage.restore_button, SW_HIDE);
    }
    manage.status_text = format!("批量模式：标签将应用到 {} 个短语", ids.len());
    manage.loading = false;
    unsafe {
        let _ = SetFocus(Some(manage.tags_edit));
        let _ = InvalidateRect(Some(manage.hwnd), None, false);
    }
}

fn batch_change_tags(manage: &mut Manage, add: bool) {
    if manage.batch_ids.is_empty() {
        status_message(manage, "请先选择多个短语");
        return;
    }
    let ids = manage.batch_ids.clone();
    let tags = parse_tags(&wide_text(manage.tags_edit));
    let result = if add {
        storage::add_tags(&ids, &tags)
    } else {
        storage::remove_tags(&ids, &tags)
    };
    match result {
        Ok(changed) => {
            let selected = ids.len();
            manage.batch_ids.clear();
            manage.editing = None;
            manage.dirty = false;
            reload_left(manage);
            reload_tag_filter(manage);
            reload_list(manage);
            load_editor(manage, None);
            unsafe {
                let _ = SendMessageW(manage.list, LB_SETSEL, 0, -1);
                let _ = SendMessageW(manage.list, LB_SETCURSEL, usize::MAX, 0);
                let _ = InvalidateRect(Some(manage.hwnd), None, false);
            }
            let action = if add { "添加" } else { "移除" };
            status_message(
                manage,
                &format!("已为 {selected} 个短语{action}标签（实际修改 {changed} 项）"),
            );
        }
        Err(code) => {
            let message = error_text(&code);
            manage.error_field = Some(ID_TAGS);
            manage.error_message = Some(message.to_string());
            status_message(manage, message);
            unsafe {
                let _ = InvalidateRect(Some(manage.hwnd), None, false);
            }
        }
    }
}

fn select_all_filtered(manage: &mut Manage) {
    if manage.phrase_rows.is_empty() {
        status_message(manage, "当前筛选结果为空");
        return;
    }
    if manage.dirty && !confirm_discard(manage.hwnd) {
        return;
    }
    let ids: Vec<String> = manage
        .phrase_rows
        .iter()
        .map(|row| row.id.clone())
        .collect();
    if ids.len() == 1 {
        manage.selected = Some(0);
        select_list_row(manage, 0);
        load_editor(manage, Some(&ids[0]));
        status_message(manage, "已选择 1 个短语");
        return;
    }
    unsafe {
        let _ = SendMessageW(manage.list, LB_SETSEL, 1, -1);
    }
    manage.selected = Some(0);
    load_batch(manage, ids);
}

fn confirm_discard(owner: HWND) -> bool {
    crate::confirm_ui::confirm(
        owner,
        "未保存的修改",
        "确定放弃当前未保存的修改吗？",
        "继续编辑可以保留这些内容。",
        "放弃修改",
        false,
    )
}

fn current_category(manage: &Manage) -> Option<String> {
    let index = unsafe { SendMessageW(manage.category, CB_GETCURSEL, 0, 0) }.0;
    if index <= 0 {
        return None;
    }
    storage::with_data(|data| {
        data.categories
            .get(index as usize - 1)
            .map(|category| category.id.clone())
    })
    .flatten()
}

fn save_editor(manage: &mut Manage) {
    let title = wide_text(manage.title_edit);
    let content = wide_text(manage.content_edit);
    let tags = parse_tags(&wide_text(manage.tags_edit));
    let category = manage.draft_category.clone();
    let pinned = manage.pinned_state;
    let hotkey_text = wide_text(manage.hotkey_edit);
    let hotkey = match storage::validate_hotkey(&hotkey_text) {
        Ok(hotkey) => hotkey,
        Err(code) => {
            let message = error_text(&code);
            manage.error_field = Some(ID_HOTKEY);
            manage.error_message = Some(message.to_string());
            status_message(manage, message);
            unsafe {
                let _ = InvalidateRect(Some(manage.hwnd), None, false);
            }
            return;
        }
    };

    if let Some(candidate) = hotkey.as_deref() {
        let parsed = crate::hotkey::parse(candidate);
        if let Some(panel) =
            storage::settings_clone().and_then(|settings| crate::hotkey::parse(&settings.hotkey))
        {
            if parsed == Some(panel) {
                let message = "与全局唤起快捷键冲突";
                manage.error_field = Some(ID_HOTKEY);
                manage.error_message = Some(message.to_string());
                status_message(manage, message);
                unsafe {
                    let _ = InvalidateRect(Some(manage.hwnd), None, false);
                }
                return;
            }
        }
        let editing = manage.editing.clone();
        let duplicate = storage::with_data(|data| {
            data.snippets.iter().any(|snippet| {
                Some(snippet.id.as_str()) != editing.as_deref()
                    && snippet.hotkey.as_deref() == Some(candidate)
            })
        })
        .unwrap_or(false);
        if duplicate {
            let message = "该快捷键已被其他短语使用";
            manage.error_field = Some(ID_HOTKEY);
            manage.error_message = Some(message.to_string());
            status_message(manage, message);
            unsafe {
                let _ = InvalidateRect(Some(manage.hwnd), None, false);
            }
            return;
        }
        let stored = editing
            .as_deref()
            .and_then(|id| {
                storage::with_store(|store| {
                    store.get(id).and_then(|snippet| snippet.hotkey.clone())
                })
            })
            .flatten();
        if stored.as_deref() != Some(candidate)
            && !crate::hotkey::is_available(manage.hwnd, candidate)
        {
            let message = "该快捷键已被其他程序占用";
            manage.error_field = Some(ID_HOTKEY);
            manage.error_message = Some(message.to_string());
            status_message(manage, message);
            unsafe {
                let _ = InvalidateRect(Some(manage.hwnd), None, false);
            }
            return;
        }
    }

    let result = storage::with_store(|store| match &manage.editing {
        Some(id) => store
            .update_snippet_full(
                id,
                &title,
                &content,
                category.as_deref(),
                pinned,
                &tags,
                hotkey.as_deref(),
            )
            .map(|_| id.clone()),
        None => store.add_snippet_full(
            &title,
            &content,
            category.as_deref(),
            pinned,
            &tags,
            hotkey.as_deref(),
        ),
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
            reload_tag_filter(manage);
            reload_list(manage);
            load_editor(manage, Some(&id));
            crate::app::refresh_snippet_hotkeys();
            status_message(manage, "已原子写入本地存储");
        }
        Some(Err(code)) => {
            let message = error_text(&code);
            manage.error_field = match code.as_str() {
                "title_empty" | "title_too_long" | "duplicate_title" => Some(ID_TITLE),
                "content_empty" | "content_too_long" => Some(ID_CONTENT),
                "tag_too_long" | "too_many_tags" | "tag_delimiter" => Some(ID_TAGS),
                "hotkey_invalid" | "hotkey_duplicate" | "hotkey_panel_conflict" => Some(ID_HOTKEY),
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
    // The destructive action targets the saved snippet, so name it with the
    // stored title; the editor may hold an unsaved rename.
    let stored_title =
        storage::with_store(|store| store.get(&id).map(|snippet| snippet.title.clone())).flatten();
    let mut title = stored_title.unwrap_or_else(|| wide_text(manage.title_edit));
    if title.trim().is_empty() {
        title = "这条短语".to_string();
    }
    let (heading, message, hint, confirm_label) = if recycle {
        (
            "彻底删除",
            format!("彻底删除「{title}」？"),
            "删除后无法恢复。",
            "彻底删除",
        )
    } else {
        (
            "移入回收站",
            format!("将「{title}」移入回收站？"),
            "可在回收站中恢复。",
            "移入回收站",
        )
    };
    if !crate::confirm_ui::confirm(manage.hwnd, heading, &message, hint, confirm_label, true) {
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
            reload_tag_filter(manage);
            reload_list(manage);
            crate::app::refresh_snippet_hotkeys();
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
            c.fill_round(&bar, d2d::scaled_f(2.0, dpi), palette.selection_border);
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
                if selected {
                    palette.selection_border
                } else {
                    palette.primary
                },
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
            if selected {
                palette.primary_text
            } else {
                palette.secondary
            },
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
            if selected {
                palette.primary_text
            } else {
                palette.muted
            },
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
    let select_btn_w = scaled(52, dpi);
    let header_gap = scaled(8, dpi);
    let search_h = scaled(34, dpi);
    let search_shell = RECT {
        left: mid_inner_l,
        top: pad,
        right: (mid_inner_r - new_btn_w - select_btn_w - header_gap * 2).max(mid_inner_l + 40),
        bottom: pad + search_h,
    };
    let search_edit_rect = ui::edit_centered_rect(
        search_shell,
        manage.fonts.body,
        scaled(30, dpi),
        scaled(8, dpi),
    );
    let tag_shell = RECT {
        left: mid_inner_l,
        top: search_shell.bottom + scaled(8, dpi),
        right: mid_inner_r,
        bottom: search_shell.bottom + scaled(8, dpi) + scaled(28, dpi),
    };
    let middle_header_bottom = tag_shell.bottom + scaled(30, dpi);

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
            manage.tag_filter,
            tag_shell.left,
            tag_shell.top,
            (tag_shell.right - tag_shell.left).max(1),
            ui::combo_height(manage.tag_filter, dpi),
            true,
        );
        let _ = MoveWindow(
            manage.select_all_button,
            mid_inner_r - new_btn_w - header_gap - select_btn_w,
            pad,
            select_btn_w,
            search_h,
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

    // Tags and per-snippet hotkey share one row above metadata.
    let tags_h = scaled(32, dpi);
    let row_gap = scaled(14, dpi);
    let tags_w = (((rw - row_gap) * 60 / 100).max(scaled(120, dpi)))
        .min((rw - row_gap - scaled(96, dpi)).max(scaled(120, dpi)));
    let tags_right = rl + tags_w;
    let tags_label = RECT {
        left: rl,
        top: footer.top - scaled(18, dpi) - scaled(38, dpi) - scaled(22, dpi) - scaled(58, dpi),
        right: tags_right,
        bottom: footer.top - scaled(18, dpi) - scaled(38, dpi) - scaled(22, dpi) - scaled(38, dpi),
    };
    let tags_shell = RECT {
        left: rl,
        top: tags_label.bottom + scaled(4, dpi),
        right: tags_right,
        bottom: tags_label.bottom + scaled(4, dpi) + tags_h,
    };
    let hotkey_left = tags_right + row_gap;
    let hotkey_label = RECT {
        left: hotkey_left,
        top: tags_label.top,
        right: rr,
        bottom: tags_label.bottom,
    };
    let hotkey_shell = RECT {
        left: hotkey_left,
        top: tags_shell.top,
        right: rr,
        bottom: tags_shell.bottom,
    };
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
        let tpad = scaled(10, dpi);
        let _ = MoveWindow(
            manage.tags_edit,
            tags_shell.left + tpad,
            tags_shell.top + scaled(5, dpi),
            (tags_shell.right - tags_shell.left - tpad * 2).max(1),
            (tags_shell.bottom - tags_shell.top - scaled(10, dpi)).max(1),
            true,
        );
        let _ = MoveWindow(
            manage.hotkey_edit,
            hotkey_shell.left + tpad,
            hotkey_shell.top + scaled(5, dpi),
            (hotkey_shell.right - hotkey_shell.left - tpad * 2).max(1),
            (hotkey_shell.bottom - hotkey_shell.top - scaled(10, dpi)).max(1),
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
        bottom: (tags_label.top - scaled(10, dpi)).max(content_label.bottom + scaled(60, dpi)),
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
        tag_shell,
        middle_header_bottom,
        recycle_banner,
        editor_header,
        title_label,
        title_shell,
        content_label,
        content_shell,
        tags_label,
        tags_shell,
        hotkey_label,
        hotkey_shell,
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

    let title_chars = manage.title_units;
    let content_chars = manage.content_units;
    let content_lines = manage.content_lines;
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
    let (query_tags, _) = search::split_tag_query(&wide_text(manage.search));
    let mut filter_tags = query_tags;
    if let Some(tag) = &manage.tag_filter_value {
        if !filter_tags
            .iter()
            .any(|existing| existing.to_lowercase() == tag.to_lowercase())
        {
            filter_tags.push(tag.clone());
        }
    }
    let mid_title = if filter_tags.is_empty() {
        mid_title
    } else {
        format!("{mid_title} · #{}", filter_tags.join(" #"))
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
            top: l.tag_shell.bottom + scaled(4, dpi),
            right: l.middle_pane.right - scaled(14, dpi),
            bottom: l.middle_header_bottom,
        };
        let count_str = format!("{} 项", manage.phrase_rows.len());
        let (count_w, _) = c.measure_style(&count_str, TextStyle::Badge, dpi);
        let count_rect = RECT {
            left: (mid_sub.right - count_w.ceil() as i32 - scaled(2, dpi)).max(mid_sub.left),
            top: mid_sub.top,
            right: mid_sub.right,
            bottom: mid_sub.bottom,
        };
        let title_rect = RECT {
            left: mid_sub.left,
            top: mid_sub.top,
            right: (count_rect.left - scaled(8, dpi)).max(mid_sub.left + scaled(20, dpi)),
            bottom: mid_sub.bottom,
        };
        c.draw_text(
            &mid_title,
            &title_rect,
            TextStyle::Badge,
            dpi,
            palette.muted,
            AlignH::Left,
            AlignV::Center,
        );
        c.draw_text(
            &count_str,
            &count_rect,
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
        let batch = !manage.batch_ids.is_empty();
        let mode_title = if batch {
            "批量标签"
        } else if manage.editing_deleted {
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

        let badge_text = if batch {
            Some(format!("已选 {} 项", manage.batch_ids.len()))
        } else if manage.dirty {
            Some("未保存".to_string())
        } else {
            None
        };
        if let Some(text) = badge_text {
            let (text_w, _) = c.measure_style(&text, TextStyle::Badge, dpi);
            let badge_w = (text_w.ceil() as i32 + scaled(24, dpi)).max(scaled(76, dpi));
            let badge_h = scaled(22, dpi);
            let badge = RECT {
                left: hdr_rect.right + scaled(6, dpi),
                top: l.editor_header.top
                    + ((l.editor_header.bottom - l.editor_header.top) - badge_h) / 2,
                right: hdr_rect.right + scaled(6, dpi) + badge_w,
                bottom: l.editor_header.top
                    + ((l.editor_header.bottom - l.editor_header.top) + badge_h) / 2,
            };
            let (badge_bg, badge_fg) = if batch {
                (
                    palette.primary_soft,
                    if crate::theme::high_contrast_cached() {
                        palette.heading
                    } else {
                        palette.primary_text
                    },
                )
            } else {
                (palette.warning_soft, palette.warning)
            };
            c.fill_round(&badge, d2d::scaled_f(11.0, dpi), badge_bg);
            c.fill_circle(
                (badge.left + scaled(10, dpi)) as f32,
                (badge.top + badge.bottom) as f32 * 0.5,
                d2d::scaled_f(3.0, dpi),
                badge_fg,
            );
            let btxt = RECT {
                left: badge.left + scaled(16, dpi),
                top: badge.top,
                right: badge.right - scaled(4, dpi),
                bottom: badge.bottom,
            };
            c.draw_text(
                &text,
                &btxt,
                TextStyle::Badge,
                dpi,
                badge_fg,
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

        c.draw_text(
            if batch {
                "标签（批量）"
            } else {
                "标签"
            },
            &l.tags_label,
            TextStyle::Label,
            dpi,
            if manage.error_field == Some(ID_TAGS) {
                palette.danger
            } else {
                palette.secondary
            },
            AlignH::Left,
            AlignV::Center,
        );
        let tags_focus = focus == manage.tags_edit;
        c.fill_stroke_round(
            &l.tags_shell,
            d2d::scaled_f(10.0, dpi),
            palette.field,
            if manage.error_field == Some(ID_TAGS) {
                palette.error
            } else if tags_focus {
                palette.primary
            } else {
                palette.border
            },
            if manage.error_field == Some(ID_TAGS) || tags_focus {
                d2d::scaled_f(1.5, dpi)
            } else {
                1.0
            },
        );
        c.draw_text(
            "专属快捷键",
            &l.hotkey_label,
            TextStyle::Label,
            dpi,
            if manage.error_field == Some(ID_HOTKEY) {
                palette.danger
            } else {
                palette.secondary
            },
            AlignH::Left,
            AlignV::Center,
        );
        let hotkey_focus = focus == manage.hotkey_edit;
        c.fill_stroke_round(
            &l.hotkey_shell,
            d2d::scaled_f(10.0, dpi),
            palette.field,
            if manage.error_field == Some(ID_HOTKEY) {
                palette.error
            } else if hotkey_focus {
                palette.primary
            } else {
                palette.border
            },
            if manage.error_field == Some(ID_HOTKEY) || hotkey_focus {
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
            WS_EX_APPWINDOW,
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
    let tag_filter = create_control(
        w!("COMBOBOX"),
        "",
        WS_CHILD
            | WS_VISIBLE
            | WINDOW_STYLE((CBS_DROPDOWNLIST | CBS_OWNERDRAWFIXED | CBS_HASSTRINGS) as u32),
        hwnd,
        ID_TAG_FILTER,
        instance,
    );
    let list = create_control(
        w!("LISTBOX"),
        "",
        WS_CHILD
            | WS_VISIBLE
            | WINDOW_STYLE(
                (LBS_NOTIFY
                    | LBS_NOINTEGRALHEIGHT
                    | LBS_OWNERDRAWFIXED
                    | LBS_HASSTRINGS
                    | LBS_EXTENDEDSEL) as u32,
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
    let select_all_button = create_control(
        w!("BUTTON"),
        "全选",
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(BS_OWNERDRAW as u32),
        hwnd,
        ID_SELECT_ALL,
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
            | WINDOW_STYLE((ES_MULTILINE | ES_AUTOVSCROLL | ES_WANTRETURN) as u32),
        hwnd,
        ID_CONTENT,
        instance,
    );
    let tags_edit = create_control(
        w!("EDIT"),
        "",
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
        hwnd,
        ID_TAGS,
        instance,
    );
    let hotkey_edit = create_control(
        w!("EDIT"),
        "",
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
        hwnd,
        ID_HOTKEY,
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
        select_all_button,
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
        tag_filter,
        cat_name,
        search,
        title_edit,
        content_edit,
        tags_edit,
        hotkey_edit,
    ] {
        ui::theme_control(control, palette.dark);
    }
    ui::theme_combo(category, palette.dark);
    ui::theme_combo(tag_filter, palette.dark);
    ui::install_combo(category);
    ui::install_combo(tag_filter);
    ui::install_smooth_listbox(left, ui::ListSurface::Sidebar);
    ui::install_smooth_listbox(list, ui::ListSurface::Card);
    ui::install_smooth_edit(content_edit);
    ui::theme_scrollbar(content_edit, palette.dark);
    ui::cue_banner(search, "搜索标题、正文、标签或拼音；\\# 搜索井号");
    ui::cue_banner(title_edit, "例如：项目进度回复、会议纪要模板…");
    ui::cue_banner(cat_name, "新分类名称…");
    ui::cue_banner(content_edit, "在此输入短语正文，支持多行文本…");
    ui::cue_banner(tags_edit, "多个标签用逗号分隔，如：退款, 客服");
    ui::cue_banner(hotkey_edit, "如 Ctrl+Alt+1");
    unsafe {
        let _ = SendMessageW(hotkey_edit, EM_SETLIMITTEXT, 64, 0);
    }

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
        let _ = SendMessageW(
            tag_filter,
            CB_SETITEMHEIGHT,
            usize::MAX,
            scaled(28, dpi) as isize,
        );
        let _ = SendMessageW(tag_filter, CB_SETITEMHEIGHT, 0, scaled(26, dpi) as isize);
    }

    let mut manage = Manage {
        hwnd,
        left,
        cat_name,
        cat_add,
        cat_delete,
        search,
        list,
        tag_filter,
        tag_filter_value: None,
        tag_options: Vec::new(),
        batch_ids: Vec::new(),
        new_button,
        select_all_button,
        delete_button,
        restore_button,
        title_edit,
        content_edit,
        tags_edit,
        hotkey_edit,
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
        title_units: 0,
        content_units: 0,
        content_lines: 1,
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
        tag_filter,
        new_button,
        select_all_button,
        delete_button,
        restore_button,
        content_edit,
        tags_edit,
        hotkey_edit,
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
        crate::dropmenu::attach_combo(tag_filter, ID_TAG_FILTER);
        crate::dropmenu::attach_combo(category, ID_CATEGORY);
    }

    reload_left(&mut manage);
    reload_categories(&mut manage);
    reload_tag_filter(&mut manage);
    reload_list(&mut manage);
    let first_id = manage.phrase_rows.first().map(|r| r.id.clone());
    load_editor(&mut manage, first_id.as_deref());
    if first_id.is_some() {
        manage.selected = Some(0);
        select_list_row(&mut manage, 0);
    }
    relayout(&mut manage);
    manage.loading = false;
    Some(manage)
}

pub fn show(owner: HWND) {
    if crate::confirm_ui::is_open() || crate::dropmenu::is_open() {
        // A modal confirmation/picker is active; tray activation must not
        // steal its foreground or start a second window transition.
        return;
    }
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
        // A minimized window that was hidden via WM_CLOSE must be restored,
        // otherwise reopening from the tray would only bring back the taskbar
        // button without the window body.
        let cmd = if IsIconic(hwnd).as_bool() {
            SW_RESTORE
        } else {
            SW_SHOW
        };
        let _ = ShowWindow(hwnd, cmd);
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
        apply_palette(manage);
        unsafe {
            let _ = SetFocus(Some(manage.search));
        }
        refresh_library(manage, false);
    });
}

fn apply_palette(manage: &mut Manage) {
    let palette = theme::current();
    if palette == manage.palette {
        return;
    }
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
        manage.tag_filter,
        manage.cat_name,
        manage.search,
        manage.title_edit,
        manage.content_edit,
        manage.tags_edit,
        manage.hotkey_edit,
    ] {
        ui::theme_control(control, palette.dark);
    }
    ui::theme_combo(manage.category, palette.dark);
    ui::theme_combo(manage.tag_filter, palette.dark);
    ui::theme_scrollbar(manage.content_edit, palette.dark);
    ui::apply_window_style(manage.hwnd);
}

/// Re-read the system palette (high contrast / theme change) while open.
pub fn refresh_theme() {
    MANAGE.with(|slot| {
        let mut guard = match slot.try_borrow_mut() {
            Ok(guard) => guard,
            Err(_) => {
                // A modal owns the borrow and pumps messages; retry after it
                // releases instead of dropping the refresh.
                REFRESH_PENDING.store(true, Ordering::Relaxed);
                return;
            }
        };
        let Some(manage) = guard.as_mut() else {
            return;
        };
        REFRESH_PENDING.store(false, Ordering::Relaxed);
        refresh_palette(manage);
    });
}

fn apply_pending_theme() {
    if !REFRESH_PENDING.load(Ordering::Relaxed) {
        return;
    }
    MANAGE.with(|slot| {
        let Ok(mut guard) = slot.try_borrow_mut() else {
            return;
        };
        let Some(manage) = guard.as_mut() else {
            return;
        };
        REFRESH_PENDING.store(false, Ordering::Relaxed);
        refresh_palette(manage);
    });
}

fn refresh_palette(manage: &mut Manage) {
    apply_palette(manage);
    for row in &mut manage.nav_rows {
        if matches!(row.view, View::Category(_)) {
            row.dot = Some(theme::category_dot_for_name(&row.label));
        }
    }
    relayout(manage);
    unsafe {
        let _ = InvalidateRect(Some(manage.hwnd), None, true);
    }
}

/// Refresh an open editor after a confirmed replacement without changing focus.
/// Call only after `confirm_exit` accepted discarding any unsaved draft.
pub fn library_replaced() {
    MANAGE.with(|slot| {
        if let Some(manage) = slot.borrow_mut().as_mut() {
            refresh_library(manage, true);
        }
    });
}

fn refresh_library(manage: &mut Manage, replace_editor: bool) {
    if replace_editor {
        // An imported library can drop the category the list is filtered by;
        // otherwise the window would show an empty list with no selected row.
        if let View::Category(id) = &manage.view {
            let exists = storage::with_data(|data| {
                data.categories.iter().any(|category| &category.id == id)
            })
            .unwrap_or(false);
            if !exists {
                manage.view = View::All;
                manage.selected = None;
            }
        }
    }
    reload_left(manage);
    reload_categories(manage);
    reload_tag_filter(manage);
    reload_list(manage);
    if replace_editor || !manage.dirty {
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
                select_list_row(manage, position);
            }
        }
        load_editor(manage, next.as_deref());
    }
    relayout(manage);
    unsafe {
        let _ = InvalidateRect(Some(manage.hwnd), None, true);
    }
}

/// Open management with a blank editor, used by the tray "quick add" entry.
pub fn show_new(owner: HWND) {
    show(owner);
    MANAGE.with(|slot| {
        let mut guard = match slot.try_borrow_mut() {
            Ok(guard) => guard,
            Err(_) => return,
        };
        let Some(manage) = guard.as_mut() else {
            return;
        };
        if manage.dirty && !confirm_discard(manage.hwnd) {
            return;
        }
        manage.view = View::All;
        manage.tag_filter_value = None;
        reload_left(manage);
        reload_categories(manage);
        reload_tag_filter(manage);
        reload_list(manage);
        manage.selected = None;
        unsafe {
            let _ = SendMessageW(manage.list, LB_SETSEL, 0, -1);
            let _ = SendMessageW(manage.list, LB_SETCURSEL, usize::MAX, 0);
        }
        load_editor(manage, None);
        relayout(manage);
        unsafe {
            let _ = SetFocus(Some(manage.title_edit));
            let _ = InvalidateRect(Some(manage.hwnd), None, false);
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

/// Reset the editor to the saved state after the user accepted discarding
/// changes at session end. The session may still be cancelled by another app,
/// in which case the already-discarded draft must not come back as dirty.
pub fn discard_draft() {
    MANAGE.with(|slot| {
        if let Ok(mut guard) = slot.try_borrow_mut() {
            if let Some(manage) = guard.as_mut() {
                if manage.dirty {
                    let id = manage.editing.clone();
                    load_editor(manage, id.as_deref());
                }
            }
        } else {
            logging::warn("manage: discard_draft skipped while a dialog is open");
        }
    });
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
    apply_pending_theme();
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
                            manage.tag_filter,
                            manage.new_button,
                            manage.select_all_button,
                            manage.delete_button,
                            manage.restore_button,
                            manage.content_edit,
                            manage.tags_edit,
                            manage.hotkey_edit,
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
                        let _ = SendMessageW(
                            manage.tag_filter,
                            CB_SETITEMHEIGHT,
                            usize::MAX,
                            scaled(28, dpi) as isize,
                        );
                        let _ = SendMessageW(
                            manage.tag_filter,
                            CB_SETITEMHEIGHT,
                            0,
                            scaled(26, dpi) as isize,
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
                let next = manage.batch_ids.is_empty() && point_in(&manage.layout.pin_card, x, y);
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
                            if manage.batch_ids.is_empty() {
                                manage.pinned_state = !manage.pinned_state;
                                if !manage.loading {
                                    manage.dirty = true;
                                    manage.status_text = "未保存修改".to_string();
                                }
                                let _ = InvalidateRect(Some(manage.pinned), None, false);
                                let _ = InvalidateRect(Some(hwnd), None, false);
                            }
                        } else if point_in(&manage.layout.search_shell, x, y) {
                            let _ = SetFocus(Some(manage.search));
                        } else if point_in(&manage.layout.title_shell, x, y) {
                            let _ = SetFocus(Some(manage.title_edit));
                        } else if point_in(&manage.layout.content_shell, x, y) {
                            let _ = SetFocus(Some(manage.content_edit));
                        } else if point_in(&manage.layout.tags_shell, x, y) {
                            let _ = SetFocus(Some(manage.tags_edit));
                        } else if point_in(&manage.layout.hotkey_shell, x, y) {
                            let _ = SetFocus(Some(manage.hotkey_edit));
                        } else if point_in(&manage.layout.cat_input_shell, x, y) {
                            let _ = SetFocus(Some(manage.cat_name));
                        }
                    }
                }
            });
            LRESULT(0)
        }
        crate::dropmenu::OPEN_PICKER => {
            let id = wparam.0 as i32;
            MANAGE.with(|slot| {
                let Ok(mut guard) = slot.try_borrow_mut() else {
                    return;
                };
                if let Some(manage) = guard.as_mut() {
                    open_combo_picker(manage, id);
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
                    ID_SEARCH | ID_TITLE | ID_CONTENT | ID_TAGS | ID_HOTKEY | ID_CAT_NAME
                        if notification == EN_SETFOCUS || notification == EN_KILLFOCUS =>
                    {
                        let _ = InvalidateRect(Some(hwnd), None, false);
                    }
                    ID_SEARCH if notification == EN_CHANGE => {
                        if !manage.loading {
                            if !manage.batch_ids.is_empty() {
                                manage.batch_ids.clear();
                                load_editor(manage, None);
                            }
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
                        reload_tag_filter(manage);
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
                            select_list_row(manage, 0);
                        }
                        relayout(manage);
                    }
                    ID_LIST if notification == LBN_SELCHANGE => {
                        let selected = selected_indices(manage);
                        if selected.len() > 1 {
                            if manage.dirty && !confirm_discard(hwnd) {
                                restore_editor_selection(manage);
                                return;
                            }
                            let ids: Vec<String> = selected
                                .iter()
                                .filter_map(|index| {
                                    manage.phrase_rows.get(*index).map(|row| row.id.clone())
                                })
                                .collect();
                            manage.selected = selected.first().copied();
                            load_batch(manage, ids);
                            return;
                        }
                        let Some(index) = selected.first().copied() else {
                            if manage.dirty && !confirm_discard(hwnd) {
                                restore_editor_selection(manage);
                                return;
                            }
                            manage.selected = None;
                            manage.batch_ids.clear();
                            load_editor(manage, None);
                            return;
                        };
                        let Some(id) = manage.phrase_rows.get(index).map(|r| r.id.clone()) else {
                            return;
                        };
                        if manage.editing.as_deref() == Some(id.as_str())
                            && manage.batch_ids.is_empty()
                        {
                            return;
                        }
                        if manage.dirty && !confirm_discard(hwnd) {
                            restore_editor_selection(manage);
                            return;
                        }
                        manage.selected = Some(index);
                        load_editor(manage, Some(&id));
                    }
                    ID_CAT_ADD if notification == BN_CLICKED => {
                        let name = wide_text(manage.cat_name);
                        match storage::with_store(|store| store.add_category(&name)) {
                            Some(Ok(_)) => {
                                set_text(manage.cat_name, "");
                                status_message(manage, "已添加分类");
                                if !manage.batch_ids.is_empty() {
                                    manage.batch_ids.clear();
                                    load_editor(manage, None);
                                }
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
                        let name = storage::with_data(|data| {
                            data.categories
                                .iter()
                                .find(|c| c.id == category)
                                .map(|c| c.name.clone())
                        })
                        .flatten()
                        .unwrap_or_else(|| "该分类".to_string());
                        let message = format!("删除「{name}」后，其短语会移到未分类。");
                        if !crate::confirm_ui::confirm(
                            manage.hwnd,
                            "删除分类",
                            &message,
                            "此操作不可撤销。",
                            "删除分类",
                            true,
                        ) {
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
                        let _ = SendMessageW(manage.list, LB_SETSEL, 0, -1);
                        let _ = SendMessageW(manage.list, LB_SETCURSEL, usize::MAX, 0);
                        load_editor(manage, None);
                        let _ = SetFocus(Some(manage.title_edit));
                    }
                    ID_SELECT_ALL => select_all_filtered(manage),
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
                                reload_tag_filter(manage);
                                reload_list(manage);
                                crate::app::refresh_snippet_hotkeys();
                                let first = manage.phrase_rows.first().map(|r| r.id.clone());
                                load_editor(manage, first.as_deref());
                                status_message(manage, "已恢复短语");
                            }
                            Some(Err(code)) => status_message(manage, error_text(&code)),
                            None => status_message(manage, "存储未初始化"),
                        }
                    }
                    ID_SAVE => {
                        if !manage.batch_ids.is_empty() {
                            batch_change_tags(manage, true);
                            return;
                        }
                        if manage.view == View::Recycle || manage.editing_deleted {
                            status_message(manage, "回收站内不能保存，请先恢复");
                            return;
                        }
                        save_editor(manage);
                    }
                    ID_CANCEL => {
                        if !manage.batch_ids.is_empty() {
                            batch_change_tags(manage, false);
                            return;
                        }
                        let id = manage.editing.clone();
                        load_editor(manage, id.as_deref());
                        status_message(manage, "已重置修改");
                    }
                    ID_TITLE | ID_CONTENT | ID_TAGS | ID_HOTKEY if notification == EN_CHANGE => {
                        if manage.error_field == Some(id as i32) {
                            manage.error_field = None;
                            manage.error_message = None;
                        }
                        if !manage.loading && manage.batch_ids.is_empty() {
                            manage.dirty = true;
                            manage.status_text = "未保存修改".to_string();
                        }
                        if id as i32 == ID_TITLE || id as i32 == ID_CONTENT {
                            refresh_editor_metrics(manage);
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
                    ID_CATEGORY if notification == CBN_SELCHANGE => apply_category(manage),
                    ID_TAG_FILTER if notification == CBN_SELCHANGE => apply_tag_filter(manage),
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
            SetBkMode(hdc, OPAQUE);
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
                    ID_TAG_FILTER => {
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
                    ID_SELECT_ALL => ui::draw_button(
                        draw,
                        manage.palette,
                        ui::ButtonKind::Secondary,
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
        WM_ACTIVATE => {
            // A transient menu/activation loop can leave the editor looking
            // disabled. Re-enable the text controls whenever the window is
            // activated so the content is editable again.
            if (wparam.0 & 0xffff) != 0 {
                MANAGE.with(|slot| {
                    if let Ok(guard) = slot.try_borrow() {
                        if let Some(manage) = guard.as_ref() {
                            if manage.batch_ids.is_empty() {
                                unsafe {
                                    let _ = EnableWindow(manage.title_edit, true);
                                    let _ = EnableWindow(manage.content_edit, true);
                                    let _ = EnableWindow(manage.tags_edit, true);
                                    let _ = EnableWindow(manage.hotkey_edit, true);
                                    let _ = InvalidateRect(Some(manage.content_edit), None, true);
                                }
                            }
                        }
                    }
                });
            }
            LRESULT(0)
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
                Err(_) => (false, false),
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

#[cfg(test)]
mod tests {
    use super::preview_text;

    #[test]
    fn preview_bounds_and_maps_newlines() {
        assert_eq!(preview_text("a\r\nb\nc"), "a ↵ b ↵ c");
        assert_eq!(preview_text("a\rb"), "ab");

        let exact = "x".repeat(160);
        assert_eq!(preview_text(&exact), exact);

        let trailing_cr = format!("{}\r", "x".repeat(160));
        assert_eq!(preview_text(&trailing_cr), "x".repeat(160));

        let longer = "x".repeat(161);
        let preview = preview_text(&longer);
        assert_eq!(preview.chars().count(), 161);
        assert!(preview.ends_with('…'));

        let boundary_lf = format!("{}\n", "x".repeat(160));
        let preview = preview_text(&boundary_lf);
        assert_eq!(preview.chars().count(), 161);
        assert!(preview.ends_with('…'));

        let emoji = "😀".repeat(200);
        let preview = preview_text(&emoji);
        assert_eq!(preview.chars().count(), 161);
        assert!(preview.ends_with('…'));
    }
}
