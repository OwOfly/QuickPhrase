//! Persistent store: `data.json`, `settings.json`, `rules-user.json`.
//!
//! Write path (final-plan §6.1): temp file -> flush -> re-read/verify ->
//! rotate a backup -> `MoveFileExW` (first write) or `ReplaceFileW`
//! (existing file), with a `MoveFileExW` fallback. Any failure keeps the
//! original file and removes the temp file.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{Read, Write};
use std::os::windows::{ffi::OsStrExt, io::AsRawHandle};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use pinyin::ToPinyin;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use windows::core::PCWSTR;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Storage::FileSystem::{
    GetFileInformationByHandle, MoveFileExW, ReplaceFileW, BY_HANDLE_FILE_INFORMATION,
    MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, REPLACEFILE_IGNORE_MERGE_ERRORS,
    REPLACEFILE_WRITE_THROUGH,
};

use crate::{logging, platform};

pub const SCHEMA_VERSION: u32 = 1;
pub const BACKUP_KEEP: usize = 5;

const DATA_FILE: &str = "data.json";
const SETTINGS_FILE: &str = "settings.json";
const RULES_FILE: &str = "rules-user.json";
const HWM_FILE: &str = "revision.hwm";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Category {
    pub id: String,
    pub name: String,
    pub sort_order: i32,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Snippet {
    pub id: String,
    pub title: String,
    pub content: String,
    pub category_id: Option<String>,
    pub pinned: bool,
    pub sort_order: i32,
    pub created_at: String,
    pub updated_at: String,
    pub use_count: u64,
    pub last_used_at: Option<String>,
    pub last_insert_status: Option<String>,
    pub deleted_at: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    /// Optional per-snippet global hotkey, normalized to `Ctrl+Alt+1` form.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hotkey: Option<String>,
    #[serde(default, skip_serializing)]
    pub search_blob: String,
}

impl Snippet {
    /// Rebuilds the six-part search blob via `build_search_blob`, per §6.2.
    pub fn rebuild_blob(&mut self) {
        self.search_blob = build_search_blob(&self.title, &self.content, &self.tags);
    }
}

/// Six-part search blob: lowercased title, lowercased content, lowercased
/// tags, spaced full pinyin, compact full pinyin and initials. Pinyin comes
/// from the title only.
pub(crate) fn build_search_blob(title: &str, content: &str, tags: &[String]) -> String {
    let (full, compact, initials) = pinyin_keys(title);
    format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        title.to_lowercase(),
        content.to_lowercase(),
        tags.join(" ").to_lowercase(),
        full,
        compact,
        initials
    )
}

/// Full pinyin, compact full pinyin and initials for the search blob.
/// Non-Chinese characters do not enter the pinyin keys; the original title
/// segment still covers literal mixed English/Chinese queries.
fn pinyin_keys(title: &str) -> (String, String, String) {
    let mut full = String::new();
    let mut compact = String::new();
    let mut initials = String::new();
    for ch in title.chars() {
        let Some(value) = ch.to_pinyin() else {
            continue;
        };
        full.push_str(value.plain());
        full.push(' ');
        compact.push_str(value.plain());
        initials.push_str(value.first_letter());
    }
    (full, compact, initials)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportMode {
    Merge,
    Replace,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportSummary {
    pub categories_added: usize,
    pub snippets_added: usize,
    pub snippets_skipped: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DataFile {
    pub schema_version: u32,
    pub revision: u64,
    pub categories: Vec<Category>,
    pub snippets: Vec<Snippet>,
}

impl Default for DataFile {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            revision: 0,
            categories: Vec::new(),
            snippets: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub schema_version: u32,
    pub hotkey: String,
    /// Missing in hand-written or older files; Unicode is always safe.
    #[serde(default = "default_insertion_mode")]
    pub insertion_mode: String,
    pub tab_policy: String,
    pub theme: String,
    pub startup: bool,
    pub paused: bool,
    pub data_dir: Option<String>,
    #[serde(default = "default_panel_sort")]
    pub panel_sort: String,
}

fn default_insertion_mode() -> String {
    "unicode".to_string()
}

fn default_panel_sort() -> String {
    "recent".to_string()
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            hotkey: "Ctrl+Alt+Q".to_string(),
            insertion_mode: "unicode".to_string(),
            tab_policy: "spaces4".to_string(),
            theme: "system".to_string(),
            startup: false,
            paused: false,
            data_dir: None,
            panel_sort: default_panel_sort(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserRule {
    pub exe: String,
    pub action: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RulesFile {
    pub version: u32,
    pub rules: Vec<UserRule>,
}

impl Default for RulesFile {
    fn default() -> Self {
        Self {
            version: 1,
            rules: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DataState {
    Ready,
    Corrupt,
    Incompatible,
    Unavailable,
}

pub struct Store {
    dir: PathBuf,
    data: DataFile,
    settings: Settings,
    rules: RulesFile,
    read_only: bool,
    notice: Option<String>,
    data_state: DataState,
    other_read_only: bool,
}

static STORE: std::sync::OnceLock<std::sync::Mutex<Store>> = std::sync::OnceLock::new();

/// Load the store once at startup and log its state.
pub fn init() {
    let mut store = Store::load();
    match store.purge_expired(30) {
        Ok(removed) if removed > 0 => {
            logging::info(&format!("storage: purged {removed} recycle-bin snippet(s)"))
        }
        Ok(_) => {}
        Err(err) => logging::warn(&format!("storage: recycle-bin purge skipped: {err}")),
    }
    logging::info(&format!(
        "storage: dir={} revision={} snippets={} read_only={} notice={:?}",
        store.dir().display(),
        store.data().revision,
        store.snippets().len(),
        store.is_read_only(),
        store.notice()
    ));
    let _ = STORE.set(std::sync::Mutex::new(store));
}

pub fn with_store<R>(f: impl FnOnce(&mut Store) -> R) -> Option<R> {
    let store = STORE.get()?;
    match store.lock() {
        Ok(mut guard) => Some(f(&mut guard)),
        Err(_) => {
            logging::error("storage: store mutex poisoned");
            None
        }
    }
}

enum UsageEvent {
    Record { id: String, status: String },
    Flush(mpsc::Sender<()>),
}

static USAGE_TX: std::sync::OnceLock<mpsc::Sender<UsageEvent>> = std::sync::OnceLock::new();

fn record_usage_now(id: &str, status: &str) {
    match with_store(|store| store.record_use(id, status)) {
        Some(Ok(())) => {}
        Some(Err(err)) => logging::warn(&format!("usage save failed: {err}")),
        None => logging::warn("usage event dropped: storage unavailable"),
    }
}

fn run_usage_worker(rx: mpsc::Receiver<UsageEvent>, mut apply: impl FnMut(&str, &str)) {
    while let Ok(event) = rx.recv() {
        match event {
            UsageEvent::Record { id, status } => apply(&id, &status),
            UsageEvent::Flush(done) => {
                let _ = done.send(());
            }
        }
    }
}

fn usage_sender() -> &'static mpsc::Sender<UsageEvent> {
    USAGE_TX.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<UsageEvent>();
        if let Err(err) = std::thread::Builder::new()
            .name("quickphrase-usage".into())
            .spawn(move || run_usage_worker(rx, record_usage_now))
        {
            logging::warn(&format!("usage worker spawn failed: {err}"));
        }
        tx
    })
}

/// Queue a usage-stat update so the UI thread does not wait for the full atomic
/// write. Events are applied in FIFO order by one background worker.
///
/// Callers must not hold the store mutex (for example from inside
/// `with_store`); the synchronous fallback locks it.
pub fn record_use_async(id: &str, status: &str) {
    if usage_sender()
        .send(UsageEvent::Record {
            id: id.to_string(),
            status: status.to_string(),
        })
        .is_err()
    {
        // The worker died or failed to start; never drop the update.
        record_usage_now(id, status);
    }
}

/// Wait for usage updates queued before this call to be applied. Called on an
/// orderly shutdown so the last insert/copy counts are persisted.
pub fn flush_usage() {
    let Some(tx) = USAGE_TX.get() else {
        return;
    };
    let (done_tx, done_rx) = mpsc::channel();
    if tx.send(UsageEvent::Flush(done_tx)).is_ok() {
        if done_rx.recv_timeout(Duration::from_millis(2000)).is_err() {
            logging::warn("usage flush timed out; pending stats may be lost");
        }
    } else {
        logging::warn("usage flush failed: worker channel closed");
    }
}

/// Snapshot for the panel: id, title, content, category name.
#[derive(Debug, Clone)]
pub struct PanelRow {
    pub id: String,
    pub title: String,
    pub content: String,
    pub category: String,
    pub category_id: Option<String>,
    pub tags: Vec<String>,
    pub blob: String,
    pub pinned: bool,
    pub sort_order: i32,
    pub last_used_at: Option<String>,
    pub updated_at: String,
    pub use_count: u64,
}

pub fn panel_items() -> Vec<PanelRow> {
    with_store(|store| {
        store
            .snippets()
            .into_iter()
            .map(|snippet| PanelRow {
                id: snippet.id.clone(),
                title: snippet.title.clone(),
                content: snippet.content.clone(),
                category: store.category_name(snippet.category_id.as_deref()),
                category_id: snippet.category_id.clone(),
                tags: snippet.tags.clone(),
                blob: snippet.search_blob.clone(),
                pinned: snippet.pinned,
                sort_order: snippet.sort_order,
                last_used_at: snippet.last_used_at.clone(),
                updated_at: snippet.updated_at.clone(),
                use_count: snippet.use_count,
            })
            .collect()
    })
    .unwrap_or_default()
}

/// Active snippets that carry a hotkey, in data order. Used by the app to
/// rebuild global registrations after data changes.
pub fn hotkey_bindings() -> Vec<(String, String)> {
    with_store(|store| {
        store
            .snippets()
            .into_iter()
            .filter(|snippet| snippet.deleted_at.is_none())
            .filter_map(|snippet| {
                snippet
                    .hotkey
                    .clone()
                    .map(|hotkey| (snippet.id.clone(), hotkey))
            })
            .collect()
    })
    .unwrap_or_default()
}

/// Every stored hotkey, including recycle-bin entries. The recycle bin keeps
/// its combos reserved so restore cannot silently lose one.
pub fn all_hotkeys() -> Vec<String> {
    with_store(|store| {
        store
            .data
            .snippets
            .iter()
            .filter_map(|snippet| snippet.hotkey.clone())
            .collect()
    })
    .unwrap_or_default()
}

/// Content of an active snippet for direct hotkey injection.
pub fn active_content(id: &str) -> Option<String> {
    with_store(|store| {
        store
            .get(id)
            .filter(|snippet| snippet.deleted_at.is_none())
            .map(|snippet| snippet.content.clone())
    })
    .flatten()
}

pub fn add_tags(ids: &[String], tags: &[String]) -> Result<usize, String> {
    with_store(|store| store.add_tags(ids, tags))
        .unwrap_or_else(|| Err("storage_unavailable".to_string()))
}

pub fn remove_tags(ids: &[String], tags: &[String]) -> Result<usize, String> {
    with_store(|store| store.remove_tags(ids, tags))
        .unwrap_or_else(|| Err("storage_unavailable".to_string()))
}

pub fn categories() -> Vec<(String, String)> {
    with_store(|store| {
        let mut categories: Vec<(i32, String, String)> = store
            .data()
            .categories
            .iter()
            .map(|category| {
                (
                    category.sort_order,
                    category.name.clone(),
                    category.id.clone(),
                )
            })
            .collect();
        categories.sort_by(|left, right| {
            left.0
                .cmp(&right.0)
                .then_with(|| left.1.cmp(&right.1))
                .then_with(|| left.2.cmp(&right.2))
        });
        categories
            .into_iter()
            .map(|(_, name, id)| (id, name))
            .collect()
    })
    .unwrap_or_default()
}

pub fn storage_notice() -> Option<String> {
    with_store(|store| store.notice().map(str::to_string)).flatten()
}

pub fn read_only_notice() -> Option<String> {
    with_store(|store| {
        if store.is_read_only() {
            store.notice().map(|notice| notice.to_string())
        } else {
            None
        }
    })
    .flatten()
}

/// Read the current data file without cloning it.
///
/// The closure runs while the store mutex is held, so it must only read and
/// copy the fields it needs; it must not send window messages or call back
/// into storage.
pub fn with_data<R>(f: impl FnOnce(&DataFile) -> R) -> Option<R> {
    with_store(|store| f(store.data()))
}

pub fn active_count() -> usize {
    with_store(|store| {
        store
            .data()
            .snippets
            .iter()
            .filter(|snippet| snippet.deleted_at.is_none())
            .count()
    })
    .unwrap_or(0)
}

/// Export a snapshot outside the store without touching live files or backups.
pub fn export_to(path: &Path) -> Result<(), String> {
    with_store(|store| store.export_to(path)).unwrap_or_else(|| Err("storage_unavailable".into()))
}

/// Import a selected JSON file. Only merge uses the external import quota;
/// a confirmed whole-library replacement can also open large local exports.
pub fn import_file(path: &Path, mode: ImportMode) -> Result<ImportSummary, String> {
    let bytes = if mode == ImportMode::Merge {
        read_merge_file(path)?
    } else {
        fs::read(path).map_err(|err| format!("read:{err}"))?
    };
    with_store(|store| store.import_bytes(&bytes, mode))
        .unwrap_or_else(|| Err("storage_unavailable".into()))
}

/// Restore a user-confirmed backup, returning the number of removed bindings.
pub fn restore_file(path: &Path) -> Result<usize, String> {
    let bytes = fs::read(path).map_err(|err| format!("read:{err}"))?;
    with_store(|store| store.restore_bytes(&bytes))
        .unwrap_or_else(|| Err("storage_unavailable".into()))
}

/// Bound merge input even if the file grows after the metadata check.
fn read_merge_file(path: &Path) -> Result<Vec<u8>, String> {
    let metadata = fs::metadata(path).map_err(|err| format!("read:{err}"))?;
    if metadata.len() > IMPORT_MAX_BYTES as u64 {
        return Err("import_too_large".to_string());
    }
    let file = fs::File::open(path).map_err(|err| format!("read:{err}"))?;
    let mut bytes = Vec::new();
    file.take(IMPORT_MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|err| format!("read:{err}"))?;
    if bytes.len() > IMPORT_MAX_BYTES {
        return Err("import_too_large".to_string());
    }
    Ok(bytes)
}

pub fn backups_dir() -> Option<PathBuf> {
    with_store(|store| store.dir().join("backups"))
}

pub fn rules() -> Vec<UserRule> {
    with_store(|store| store.rules.rules.clone()).unwrap_or_default()
}

pub fn set_rule(exe: &str, action: &str) -> Result<(), String> {
    match with_store(|store| store.set_rule(exe, action)) {
        Some(result) => result,
        None => Err("storage_unavailable".to_string()),
    }
}

pub fn remove_rule(exe: &str) -> Result<(), String> {
    match with_store(|store| store.remove_rule(exe)) {
        Some(result) => result,
        None => Err("storage_unavailable".to_string()),
    }
}

pub fn action_for_exe(exe: &str) -> Option<String> {
    with_store(|store| store.action_for_exe(exe)).flatten()
}

fn normalize_exe(value: &str) -> String {
    value
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(value)
        .trim()
        .to_lowercase()
}

pub fn settings_clone() -> Option<Settings> {
    with_store(|store| store.settings().clone())
}

pub fn save_settings(next: Settings) -> Result<(), String> {
    match with_store(|store| store.save_settings(next)) {
        Some(result) => result,
        None => Err("storage_unavailable".to_string()),
    }
}

const TITLE_MAX_UNITS: usize = 80;
const CONTENT_MAX_UNITS: usize = 10_000;
const CATEGORY_NAME_MAX_UNITS: usize = 40;
const TAGS_MAX: usize = 10;
const TAG_MAX_UNITS: usize = 24;

fn validate_tags(tags: &[String]) -> Result<Vec<String>, String> {
    let mut result = Vec::new();
    let mut seen = HashSet::new();
    for tag in tags {
        let tag = tag.trim();
        if tag.is_empty() {
            continue;
        }
        if tag.encode_utf16().count() > TAG_MAX_UNITS {
            return Err("tag_too_long".to_string());
        }
        if tag.contains([',', '，']) {
            return Err("tag_delimiter".to_string());
        }
        if seen.insert(tag.to_lowercase()) {
            result.push(tag.to_string());
        }
    }
    if result.len() > TAGS_MAX {
        return Err("too_many_tags".to_string());
    }
    Ok(result)
}

/// Canonicalizes a per-snippet hotkey. Empty means "no hotkey".
pub(crate) fn validate_hotkey(raw: &str) -> Result<Option<String>, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(None);
    }
    if raw.encode_utf16().count() > 64 {
        return Err("hotkey_invalid".to_string());
    }
    let spec = crate::hotkey::parse(raw).ok_or_else(|| "hotkey_invalid".to_string())?;
    Ok(Some(spec.text()))
}

fn validate_text(title: &str, content: &str) -> Result<(String, String), String> {
    let title = title.trim().to_string();
    if title.is_empty() {
        return Err("title_empty".to_string());
    }
    if title.encode_utf16().count() > TITLE_MAX_UNITS {
        return Err("title_too_long".to_string());
    }
    if content.trim().is_empty() {
        return Err("content_empty".to_string());
    }
    if content.encode_utf16().count() > CONTENT_MAX_UNITS {
        return Err("content_too_long".to_string());
    }
    // Body keeps trailing spaces/newlines: the user's text is stored verbatim.
    Ok((title, content.to_string()))
}

const IMPORT_MAX_BYTES: usize = 20 * 1024 * 1024;
const IMPORT_MAX_SNIPPETS: usize = 10_000;
const IMPORT_MAX_CATEGORIES: usize = 1_000;

fn parse_import(bytes: &[u8], merge: bool) -> Result<DataFile, String> {
    if merge && bytes.len() > IMPORT_MAX_BYTES {
        return Err("import_too_large".into());
    }
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    let imported = serde_json::from_slice(bytes).map_err(|_| "import_invalid_json".to_string())?;
    validate_import(imported, merge)
}

fn validate_import(mut data: DataFile, merge: bool) -> Result<DataFile, String> {
    if data.schema_version != SCHEMA_VERSION {
        return Err("import_schema".to_string());
    }
    if merge
        && (data.snippets.len() > IMPORT_MAX_SNIPPETS
            || data.categories.len() > IMPORT_MAX_CATEGORIES)
    {
        return Err("import_too_large".to_string());
    }

    let mut category_ids = HashSet::new();
    let mut category_names = HashSet::new();
    for category in &mut data.categories {
        category.name = category.name.trim().to_string();
        if category.id.is_empty() || !category_ids.insert(category.id.clone()) {
            return Err("import_category_id".to_string());
        }
        if category.name.is_empty()
            || category.name.encode_utf16().count() > CATEGORY_NAME_MAX_UNITS
            || !category_names.insert(category.name.to_lowercase())
        {
            return Err("import_category_name".to_string());
        }
        if category.created_at.is_empty() {
            category.created_at = now_iso();
        }
        if category.updated_at.is_empty() {
            category.updated_at = category.created_at.clone();
        }
    }

    let mut snippet_ids = HashSet::new();
    let mut active_titles = HashSet::new();
    for snippet in &mut data.snippets {
        if snippet.id.is_empty() || !snippet_ids.insert(snippet.id.clone()) {
            return Err("import_snippet_id".to_string());
        }
        let (title, content) = validate_text(&snippet.title, &snippet.content)?;
        snippet.title = title;
        snippet.content = content;
        snippet.tags = validate_tags(&snippet.tags).map_err(|_| "import_tags".to_string())?;
        snippet.hotkey = validate_hotkey(snippet.hotkey.as_deref().unwrap_or(""))
            .map_err(|_| "import_hotkey".to_string())?;
        if let Some(category_id) = snippet.category_id.as_deref() {
            if !category_ids.contains(category_id) {
                return Err("import_category_missing".to_string());
            }
        }
        if snippet.deleted_at.is_none() && !active_titles.insert(snippet.title.to_lowercase()) {
            return Err("duplicate_title".to_string());
        }
        if snippet.created_at.is_empty() {
            snippet.created_at = now_iso();
        }
        if snippet.updated_at.is_empty() {
            snippet.updated_at = snippet.created_at.clone();
        }
        snippet.rebuild_blob();
    }
    data.schema_version = SCHEMA_VERSION;
    // The imported revision is not authoritative: the local high-water mark
    // decides the next value. Reset it so a crafted `u64::MAX` cannot overflow
    // the increment in `save_data_with`.
    data.revision = 0;
    Ok(data)
}

impl Store {
    #[allow(dead_code)] // used by tests and the M1-4 editor when reloading by id
    pub fn get(&self, id: &str) -> Option<&Snippet> {
        self.data.snippets.iter().find(|item| item.id == id)
    }

    #[allow(dead_code)] // recycle count UI lands with M1-4 polish
    pub fn deleted_snippets(&self) -> Vec<&Snippet> {
        self.data
            .snippets
            .iter()
            .filter(|item| item.deleted_at.is_some())
            .collect()
    }

    fn export_json(&self) -> Result<String, String> {
        serde_json::to_string_pretty(&self.data).map_err(|err| err.to_string())
    }

    fn export_to(&self, path: &Path) -> Result<(), String> {
        let target = export_target(&self.dir, path)?;
        if self.data_state != DataState::Ready {
            return Err("export_data_unavailable".into());
        }
        let json = self.export_json()?;
        let temp = target.with_file_name(format!(".quickphrase-export-{}.tmp", new_id()));
        write_new_file(&temp, json.as_bytes())?;
        let result = move_file(&temp, &target, true);
        if result.is_err() {
            let _ = fs::remove_file(&temp);
        }
        result
    }

    /// Restore data independently of settings/rules; unrelated read-only
    /// guards remain active. Never rotate an unreadable main into the backups.
    fn restore_bytes(&mut self, bytes: &[u8]) -> Result<usize, String> {
        let corrupt = self.data_state == DataState::Corrupt;
        if self.read_only && !corrupt {
            return Err("read_only".into());
        }
        let mut imported = parse_import(bytes, false)?;
        let panel = crate::hotkey::parse(&self.settings.hotkey);
        let mut used = HashSet::new();
        let mut removed = 0;
        for snippet in &mut imported.snippets {
            if let Some(binding) = &snippet.hotkey {
                if crate::hotkey::parse(binding) == panel || !used.insert(binding.clone()) {
                    snippet.hotkey = None;
                    removed += 1;
                }
            }
        }
        if corrupt {
            let main = self.path(DATA_FILE);
            match fs::read(&main) {
                Ok(original) => {
                    // Recheck the schema before replacing a file changed since startup.
                    if json_version(&original, "schemaVersion")
                        .is_some_and(|version| version != u64::from(SCHEMA_VERSION))
                        || serde_json::from_slice::<DataFile>(&original).is_ok()
                    {
                        return Err("read_only".into());
                    }
                    let preserved = self.path(&format!("data-corrupt-{}.json", new_id()));
                    write_new_file(&preserved, &original)?;
                    logging::warn(&format!("storage: preserved {}", preserved.display()));
                }
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => return Err(err.to_string()),
            }
        }
        self.import_replace(imported, !corrupt)?;
        if corrupt {
            self.data_state = DataState::Ready;
            self.read_only = self.other_read_only;
            if !self.other_read_only {
                self.notice = None;
            }
        }
        Ok(removed)
    }

    fn import_bytes(&mut self, bytes: &[u8], mode: ImportMode) -> Result<ImportSummary, String> {
        if self.read_only {
            return Err("read_only".to_string());
        }
        let imported = parse_import(bytes, mode == ImportMode::Merge)?;
        {
            // Merge skips active title duplicates; their hotkeys must not block
            // the import. Everything actually inserted is then checked for
            // internal and cross-data uniqueness.
            let existing_active_titles: HashSet<String> = self
                .data
                .snippets
                .iter()
                .filter(|snippet| snippet.deleted_at.is_none())
                .map(|snippet| snippet.title.to_lowercase())
                .collect();
            let effective: Vec<&Snippet> = imported
                .snippets
                .iter()
                .filter(|snippet| {
                    mode == ImportMode::Replace
                        || snippet.deleted_at.is_some()
                        || !existing_active_titles.contains(&snippet.title.to_lowercase())
                })
                .collect();

            let mut seen = HashSet::new();
            for snippet in &effective {
                if let Some(hotkey) = snippet.hotkey.as_deref() {
                    if !seen.insert(hotkey.to_string()) {
                        return Err("import_hotkey".to_string());
                    }
                }
            }
            if let Some(panel) = crate::hotkey::parse(&self.settings.hotkey) {
                if effective.iter().any(|snippet| {
                    snippet.hotkey.as_deref().and_then(crate::hotkey::parse) == Some(panel)
                }) {
                    return Err("import_hotkey".to_string());
                }
            }
            if mode == ImportMode::Merge {
                let existing: HashSet<&str> = self
                    .data
                    .snippets
                    .iter()
                    .filter_map(|snippet| snippet.hotkey.as_deref())
                    .collect();
                if effective
                    .iter()
                    .filter_map(|snippet| snippet.hotkey.as_deref())
                    .any(|hotkey| existing.contains(hotkey))
                {
                    return Err("import_hotkey".to_string());
                }
            }
        }
        match mode {
            ImportMode::Replace => self.import_replace(imported, true),
            ImportMode::Merge => self.import_merge(imported),
        }
    }

    fn import_replace(
        &mut self,
        imported: DataFile,
        rotate_backup: bool,
    ) -> Result<ImportSummary, String> {
        let summary = ImportSummary {
            categories_added: imported.categories.len(),
            snippets_added: imported.snippets.len(),
            snippets_skipped: 0,
        };

        let mut imported = imported;
        // Keep the local in-memory revision as a floor in case revision.hwm is
        // missing or unreadable; the next write must never go backwards.
        imported.revision = self.data.revision;
        let previous = std::mem::replace(&mut self.data, imported);
        match self.persist_data(rotate_backup) {
            Ok(()) => Ok(summary),
            Err(err) => {
                self.data = previous;
                Err(err)
            }
        }
    }

    fn import_merge(&mut self, imported: DataFile) -> Result<ImportSummary, String> {
        let mut next = self.data.clone();
        let previous = self.data.clone();
        let mut summary = ImportSummary::default();
        let mut category_map: HashMap<String, String> = HashMap::new();

        for category in imported.categories {
            let original_id = category.id.clone();
            let imported_name = category.name.to_lowercase();
            let existing = next
                .categories
                .iter()
                .find(|item| item.name.to_lowercase() == imported_name)
                .map(|item| item.id.clone());
            let mapped = if let Some(id) = existing {
                id
            } else {
                let mut id = original_id.clone();
                if next.categories.iter().any(|item| item.id == id) {
                    id = new_id();
                }
                next.categories.push(Category {
                    id: id.clone(),
                    name: category.name,
                    sort_order: category.sort_order,
                    created_at: category.created_at,
                    updated_at: category.updated_at,
                });
                summary.categories_added += 1;
                id
            };
            category_map.insert(original_id, mapped);
        }

        let mut active_titles: HashSet<String> = next
            .snippets
            .iter()
            .filter(|snippet| snippet.deleted_at.is_none())
            .map(|snippet| snippet.title.to_lowercase())
            .collect();

        for mut snippet in imported.snippets {
            if snippet.deleted_at.is_none() && active_titles.contains(&snippet.title.to_lowercase())
            {
                summary.snippets_skipped += 1;
                continue;
            }
            if next.snippets.iter().any(|item| item.id == snippet.id) {
                snippet.id = new_id();
            }
            snippet.category_id = snippet
                .category_id
                .and_then(|category_id| category_map.get(&category_id).cloned());
            snippet.rebuild_blob();
            if snippet.deleted_at.is_none() {
                active_titles.insert(snippet.title.to_lowercase());
            }
            next.snippets.push(snippet);
            summary.snippets_added += 1;
        }

        self.data = next;
        match self.save_data() {
            Ok(()) => Ok(summary),
            Err(err) => {
                self.data = previous;
                Err(err)
            }
        }
    }

    #[allow(dead_code)] // convenience wrapper for tag-less callers/tests
    pub fn add_snippet(
        &mut self,
        title: &str,
        content: &str,
        category_id: Option<&str>,
        pinned: bool,
    ) -> Result<String, String> {
        self.add_snippet_tagged(title, content, category_id, pinned, &[])
    }

    pub fn add_snippet_tagged(
        &mut self,
        title: &str,
        content: &str,
        category_id: Option<&str>,
        pinned: bool,
        tags: &[String],
    ) -> Result<String, String> {
        self.add_snippet_full(title, content, category_id, pinned, tags, None)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn add_snippet_full(
        &mut self,
        title: &str,
        content: &str,
        category_id: Option<&str>,
        pinned: bool,
        tags: &[String],
        hotkey: Option<&str>,
    ) -> Result<String, String> {
        if self.read_only {
            return Err("read_only".to_string());
        }
        let (title, content) = validate_text(title, content)?;
        let tags = validate_tags(tags)?;
        let hotkey = validate_hotkey(hotkey.unwrap_or(""))?;
        self.check_category(category_id)?;
        self.check_duplicate(&title, None)?;
        self.check_hotkey(hotkey.as_deref(), None)?;

        let now = now_iso();
        let id = new_id();
        let mut snippet = Snippet {
            id: id.clone(),
            title,
            content,
            category_id: category_id.map(|value| value.to_string()),
            pinned,
            sort_order: self.next_sort_order(),
            created_at: now.clone(),
            updated_at: now,
            use_count: 0,
            last_used_at: None,
            last_insert_status: None,
            deleted_at: None,
            tags,
            hotkey,
            search_blob: String::new(),
        };
        snippet.rebuild_blob();
        self.data.snippets.push(snippet);
        match self.save_data() {
            Ok(()) => Ok(id),
            Err(err) => {
                self.data.snippets.retain(|item| item.id != id);
                Err(err)
            }
        }
    }

    #[allow(dead_code)] // convenience wrapper for tag-less callers/tests
    pub fn update_snippet(
        &mut self,
        id: &str,
        title: &str,
        content: &str,
        category_id: Option<&str>,
        pinned: bool,
    ) -> Result<(), String> {
        self.update_snippet_tagged(id, title, content, category_id, pinned, &[])
    }

    pub fn update_snippet_tagged(
        &mut self,
        id: &str,
        title: &str,
        content: &str,
        category_id: Option<&str>,
        pinned: bool,
        tags: &[String],
    ) -> Result<(), String> {
        // Compatibility wrapper: changing title/content/tags must not silently
        // drop a per-snippet hotkey.
        let hotkey = self
            .data
            .snippets
            .iter()
            .find(|snippet| snippet.id == id)
            .and_then(|snippet| snippet.hotkey.clone());
        self.update_snippet_full(
            id,
            title,
            content,
            category_id,
            pinned,
            tags,
            hotkey.as_deref(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn update_snippet_full(
        &mut self,
        id: &str,
        title: &str,
        content: &str,
        category_id: Option<&str>,
        pinned: bool,
        tags: &[String],
        hotkey: Option<&str>,
    ) -> Result<(), String> {
        if self.read_only {
            return Err("read_only".to_string());
        }
        let (title, content) = validate_text(title, content)?;
        let tags = validate_tags(tags)?;
        let hotkey = validate_hotkey(hotkey.unwrap_or(""))?;
        self.check_category(category_id)?;
        self.check_duplicate(&title, Some(id))?;
        self.check_hotkey(hotkey.as_deref(), Some(id))?;

        let Some(position) = self.data.snippets.iter().position(|item| item.id == id) else {
            return Err("snippet_missing".to_string());
        };
        let previous = self.data.snippets[position].clone();
        let now = now_iso();
        {
            let snippet = &mut self.data.snippets[position];
            snippet.title = title;
            snippet.content = content;
            snippet.category_id = category_id.map(|value| value.to_string());
            snippet.pinned = pinned;
            snippet.tags = tags;
            snippet.hotkey = hotkey;
            snippet.updated_at = now;
            snippet.rebuild_blob();
        }
        match self.save_data() {
            Ok(()) => Ok(()),
            Err(err) => {
                self.data.snippets[position] = previous;
                Err(err)
            }
        }
    }

    /// Adds tags to every selected snippet in one atomic write. Returns the number
    /// of snippets whose tags actually changed.
    pub fn add_tags(&mut self, ids: &[String], tags: &[String]) -> Result<usize, String> {
        self.change_tags(ids, tags, true)
    }

    /// Removes tags from every selected snippet in one atomic write.
    pub fn remove_tags(&mut self, ids: &[String], tags: &[String]) -> Result<usize, String> {
        self.change_tags(ids, tags, false)
    }

    fn change_tags(&mut self, ids: &[String], tags: &[String], add: bool) -> Result<usize, String> {
        if self.read_only {
            return Err("read_only".to_string());
        }
        let tags = validate_tags(tags)?;
        if tags.is_empty() {
            return Err("tag_empty".to_string());
        }

        let mut seen = HashSet::new();
        let mut plans: Vec<(usize, Vec<String>)> = Vec::new();
        for id in ids {
            if !seen.insert(id.as_str()) {
                continue;
            }
            let Some(position) = self.data.snippets.iter().position(|item| item.id == *id) else {
                return Err("snippet_missing".to_string());
            };
            let previous = &self.data.snippets[position].tags;
            let mut next = previous.clone();
            if add {
                for tag in &tags {
                    if !next
                        .iter()
                        .any(|existing| existing.to_lowercase() == tag.to_lowercase())
                    {
                        next.push(tag.clone());
                    }
                }
                next = validate_tags(&next)?;
            } else {
                next.retain(|existing| {
                    !tags
                        .iter()
                        .any(|tag| tag.to_lowercase() == existing.to_lowercase())
                });
            }
            if next != *previous {
                plans.push((position, next));
            }
        }

        if plans.is_empty() {
            return Ok(0);
        }

        let now = now_iso();
        let previous: Vec<(usize, Vec<String>, String)> = plans
            .iter()
            .map(|(position, _)| {
                (
                    *position,
                    self.data.snippets[*position].tags.clone(),
                    self.data.snippets[*position].updated_at.clone(),
                )
            })
            .collect();
        for (position, tags) in &plans {
            let snippet = &mut self.data.snippets[*position];
            snippet.tags = tags.clone();
            snippet.updated_at = now.clone();
            snippet.rebuild_blob();
        }
        match self.save_data() {
            Ok(()) => Ok(plans.len()),
            Err(err) => {
                for (position, tags, updated_at) in previous {
                    let snippet = &mut self.data.snippets[position];
                    snippet.tags = tags;
                    snippet.updated_at = updated_at;
                    snippet.rebuild_blob();
                }
                Err(err)
            }
        }
    }

    pub fn soft_delete(&mut self, id: &str) -> Result<(), String> {
        self.mutate_snippet(id, |snippet, now| {
            snippet.deleted_at = Some(now.to_string());
            snippet.updated_at = now.to_string();
        })
    }

    pub fn restore(&mut self, id: &str) -> Result<(), String> {
        if self.read_only {
            return Err("read_only".to_string());
        }
        let Some(position) = self.data.snippets.iter().position(|item| item.id == id) else {
            return Err("snippet_missing".to_string());
        };
        let title = self.data.snippets[position].title.to_lowercase();
        if self
            .data
            .snippets
            .iter()
            .enumerate()
            .any(|(index, snippet)| {
                index != position
                    && snippet.deleted_at.is_none()
                    && snippet.title.to_lowercase() == title
            })
        {
            return Err("duplicate_title".to_string());
        }
        let restore_hotkey = self.data.snippets[position].hotkey.clone();
        self.check_hotkey(restore_hotkey.as_deref(), Some(id))?;
        let previous = self.data.snippets[position].clone();
        self.data.snippets[position].deleted_at = None;
        self.data.snippets[position].updated_at = now_iso();
        match self.save_data() {
            Ok(()) => Ok(()),
            Err(err) => {
                self.data.snippets[position] = previous;
                Err(err)
            }
        }
    }

    pub fn purge_expired(&mut self, days: i64) -> Result<usize, String> {
        if self.read_only {
            return Err("read_only".to_string());
        }
        let cutoff = utc_now_secs() - days.saturating_mul(86_400);
        let previous = self.data.clone();
        let before = self.data.snippets.len();
        self.data.snippets.retain(|snippet| {
            snippet
                .deleted_at
                .as_deref()
                .map(|deleted| match parse_utc_stamp(deleted) {
                    // Only trust the canonical `YYYY-MM-DDTHH:MM:SSZ` form the
                    // app itself writes. Imported offsets, fractional seconds
                    // or hand-edited values stay in the bin instead of being
                    // purged early by a lexicographic comparison.
                    Some(timestamp) => timestamp >= cutoff,
                    None => true,
                })
                .unwrap_or(true)
        });
        let removed = before - self.data.snippets.len();
        if removed == 0 {
            return Ok(0);
        }
        match self.save_data() {
            Ok(()) => Ok(removed),
            Err(err) => {
                self.data = previous;
                Err(err)
            }
        }
    }

    #[allow(dead_code)] // wired to the recycle toolbar in the manage follow-up
    pub fn empty_recycle(&mut self) -> Result<usize, String> {
        if self.read_only {
            return Err("read_only".to_string());
        }
        let previous = self.data.clone();
        let before = self.data.snippets.len();
        self.data
            .snippets
            .retain(|snippet| snippet.deleted_at.is_none());
        let removed = before - self.data.snippets.len();
        if removed == 0 {
            return Ok(0);
        }
        match self.save_data() {
            Ok(()) => Ok(removed),
            Err(err) => {
                self.data = previous;
                Err(err)
            }
        }
    }

    pub fn purge(&mut self, id: &str) -> Result<(), String> {
        if self.read_only {
            return Err("read_only".to_string());
        }
        let Some(position) = self.data.snippets.iter().position(|item| item.id == id) else {
            return Err("snippet_missing".to_string());
        };
        let removed = self.data.snippets.remove(position);
        match self.save_data() {
            Ok(()) => Ok(()),
            Err(err) => {
                self.data.snippets.insert(position, removed);
                Err(err)
            }
        }
    }

    pub fn add_category(&mut self, name: &str) -> Result<String, String> {
        if self.read_only {
            return Err("read_only".to_string());
        }
        let name = self.validate_category_name(name, None)?;
        let now = now_iso();
        let id = new_id();
        let category = Category {
            id: id.clone(),
            name,
            sort_order: self
                .data
                .categories
                .iter()
                .map(|category| category.sort_order + 1)
                .max()
                .unwrap_or(0),
            created_at: now.clone(),
            updated_at: now,
        };
        self.data.categories.push(category);
        match self.save_data() {
            Ok(()) => Ok(id),
            Err(err) => {
                self.data.categories.retain(|item| item.id != id);
                Err(err)
            }
        }
    }

    #[allow(dead_code)] // rename UI lands with M1-4 polish
    pub fn rename_category(&mut self, id: &str, name: &str) -> Result<(), String> {
        if self.read_only {
            return Err("read_only".to_string());
        }
        let name = self.validate_category_name(name, Some(id))?;
        let Some(position) = self
            .data
            .categories
            .iter()
            .position(|category| category.id == id)
        else {
            return Err("category_missing".to_string());
        };
        let previous = self.data.categories[position].clone();
        self.data.categories[position].name = name;
        self.data.categories[position].updated_at = now_iso();
        match self.save_data() {
            Ok(()) => Ok(()),
            Err(err) => {
                self.data.categories[position] = previous;
                Err(err)
            }
        }
    }

    pub fn delete_category(&mut self, id: &str) -> Result<(), String> {
        if self.read_only {
            return Err("read_only".to_string());
        }
        let Some(position) = self
            .data
            .categories
            .iter()
            .position(|category| category.id == id)
        else {
            return Err("category_missing".to_string());
        };
        let previous = self.data.categories.remove(position);
        let now = now_iso();
        let mut moved: Vec<(usize, Option<String>, String)> = Vec::new();
        for (index, snippet) in self.data.snippets.iter_mut().enumerate() {
            if snippet.category_id.as_deref() == Some(id) {
                let old_updated = snippet.updated_at.clone();
                moved.push((index, snippet.category_id.take(), old_updated));
                snippet.updated_at = now.clone();
            }
        }
        match self.save_data() {
            Ok(()) => Ok(()),
            Err(err) => {
                self.data.categories.insert(position, previous);
                for (index, category_id, updated_at) in moved {
                    self.data.snippets[index].category_id = category_id;
                    self.data.snippets[index].updated_at = updated_at;
                }
                Err(err)
            }
        }
    }

    fn mutate_snippet(
        &mut self,
        id: &str,
        mutate: impl FnOnce(&mut Snippet, &str),
    ) -> Result<(), String> {
        if self.read_only {
            return Err("read_only".to_string());
        }
        let Some(position) = self.data.snippets.iter().position(|item| item.id == id) else {
            return Err("snippet_missing".to_string());
        };
        let previous = self.data.snippets[position].clone();
        let now = now_iso();
        mutate(&mut self.data.snippets[position], &now);
        match self.save_data() {
            Ok(()) => Ok(()),
            Err(err) => {
                self.data.snippets[position] = previous;
                Err(err)
            }
        }
    }

    fn check_category(&self, category_id: Option<&str>) -> Result<(), String> {
        if let Some(id) = category_id {
            if !self
                .data
                .categories
                .iter()
                .any(|category| category.id == id)
            {
                return Err("category_missing".to_string());
            }
        }
        Ok(())
    }

    fn check_duplicate(&self, title: &str, skip: Option<&str>) -> Result<(), String> {
        let needle = title.to_lowercase();
        if self.data.snippets.iter().any(|snippet| {
            snippet.deleted_at.is_none()
                && Some(snippet.id.as_str()) != skip
                && snippet.title.to_lowercase() == needle
        }) {
            return Err("duplicate_title".to_string());
        }
        Ok(())
    }

    /// Hotkeys are unique across all snippets, including the recycle bin, so a
    /// restore can never silently produce two snippets fighting for one combo.
    fn check_hotkey(&self, hotkey: Option<&str>, skip: Option<&str>) -> Result<(), String> {
        let Some(hotkey) = hotkey else {
            return Ok(());
        };
        if self.data.snippets.iter().any(|snippet| {
            Some(snippet.id.as_str()) != skip && snippet.hotkey.as_deref() == Some(hotkey)
        }) {
            return Err("hotkey_duplicate".to_string());
        }
        if let Some(panel) = crate::hotkey::parse(&self.settings.hotkey) {
            if crate::hotkey::parse(hotkey) == Some(panel) {
                return Err("hotkey_panel_conflict".to_string());
            }
        }
        Ok(())
    }

    fn validate_category_name(&self, name: &str, skip: Option<&str>) -> Result<String, String> {
        let name = name.trim().to_string();
        if name.is_empty() {
            return Err("category_empty".to_string());
        }
        if name.encode_utf16().count() > CATEGORY_NAME_MAX_UNITS {
            return Err("category_too_long".to_string());
        }
        if self.data.categories.iter().any(|category| {
            Some(category.id.as_str()) != skip
                && category.name.to_lowercase() == name.to_lowercase()
        }) {
            return Err("category_duplicate".to_string());
        }
        Ok(name)
    }

    fn next_sort_order(&self) -> i32 {
        self.data
            .snippets
            .iter()
            .map(|snippet| snippet.sort_order + 1)
            .max()
            .unwrap_or(0)
    }
}

impl Store {
    pub fn load() -> Self {
        Self::load_from(&platform::data_dir())
    }

    pub fn load_from(dir: &Path) -> Self {
        if let Err(err) = fs::create_dir_all(dir) {
            logging::error(&format!("storage: cannot create data dir: {err}"));
            return Self {
                dir: dir.to_path_buf(),
                data: DataFile::default(),
                settings: Settings::default(),
                rules: RulesFile::default(),
                read_only: true,
                notice: Some("data_dir_unavailable".to_string()),
                data_state: DataState::Unavailable,
                other_read_only: true,
            };
        }

        let mut notice = None;
        let mut read_only = false;
        let mut data_state = DataState::Ready;

        clean_stale_temp(dir);

        // Check even incompatible layouts before considering a backup.
        let incompatible_data = peek_version(&dir.join(DATA_FILE), "schemaVersion")
            .filter(|&version| version != u64::from(SCHEMA_VERSION));
        let future_settings = peek_version(&dir.join(SETTINGS_FILE), "schemaVersion")
            .map(|version| version > u64::from(SCHEMA_VERSION))
            .unwrap_or(false);
        let future_rules = peek_version(&dir.join(RULES_FILE), "version")
            .map(|version| version > 1)
            .unwrap_or(false);

        let (mut data, recovered) = if incompatible_data.is_some() {
            logging::warn("storage: data.json schema unsupported; skipping recovery");
            (
                read_json::<DataFile>(&dir.join(DATA_FILE)).unwrap_or_default(),
                None,
            )
        } else {
            match read_with_recovery::<DataFile>(dir, DATA_FILE) {
                Ok((Some(data), source)) => (data, source),
                Ok((None, _)) => (seed(), None),
                Err(err) => {
                    logging::error(&format!("storage: data.json unusable: {err}"));
                    notice = Some("data_recovery_failed".to_string());
                    data_state = DataState::Corrupt;
                    (DataFile::default(), None)
                }
            }
        };

        let data_version = incompatible_data.unwrap_or(u64::from(data.schema_version));
        if data_version > u64::from(SCHEMA_VERSION) {
            logging::warn(&format!(
                "storage: schema {data_version} is newer than {SCHEMA_VERSION}; read-only"
            ));
            notice = Some("schema_newer".to_string());
            data_state = DataState::Incompatible;
        } else if data_version < u64::from(SCHEMA_VERSION) {
            logging::warn(&format!(
                "storage: schema {data_version} predates {SCHEMA_VERSION}; read-only until migrated"
            ));
            notice = Some("schema_older".to_string());
            data_state = DataState::Incompatible;
        }

        let (settings, settings_recovered) = if future_settings {
            logging::warn("storage: settings schema is newer; skipping recovery");
            (
                read_json::<Settings>(&dir.join(SETTINGS_FILE)).unwrap_or_default(),
                None,
            )
        } else {
            match read_with_recovery::<Settings>(dir, SETTINGS_FILE) {
                Ok((Some(settings), source)) => (settings, source),
                Ok((None, _)) => (Settings::default(), None),
                Err(err) => {
                    logging::warn(&format!("storage: settings.json unusable: {err}"));
                    notice = Some("settings_recovery_failed".to_string());
                    read_only = true;
                    (Settings::default(), None)
                }
            }
        };
        if future_settings {
            notice = Some("settings_schema_newer".to_string());
            read_only = true;
        }
        if settings.schema_version > SCHEMA_VERSION {
            logging::warn("storage: settings schema is newer; read-only");
            notice = Some("settings_schema_newer".to_string());
            read_only = true;
        }

        let (rules, rules_recovered) = if future_rules {
            logging::warn("storage: rules version is newer; skipping recovery");
            (
                read_json::<RulesFile>(&dir.join(RULES_FILE)).unwrap_or_default(),
                None,
            )
        } else {
            match read_with_recovery::<RulesFile>(dir, RULES_FILE) {
                Ok((Some(rules), source)) => (rules, source),
                Ok((None, _)) => (RulesFile::default(), None),
                Err(err) => {
                    logging::warn(&format!("storage: rules-user.json unusable: {err}"));
                    notice = Some("rules_recovery_failed".to_string());
                    read_only = true;
                    (RulesFile::default(), None)
                }
            }
        };
        if future_rules {
            notice = Some("rules_version_newer".to_string());
            read_only = true;
        }
        if rules.version > 1 {
            logging::warn("storage: rules version is newer; read-only");
            notice = Some("rules_version_newer".to_string());
            read_only = true;
        }

        for snippet in &mut data.snippets {
            snippet.rebuild_blob();
        }

        let other_read_only = read_only;
        let read_only = data_state != DataState::Ready || other_read_only;
        let mut store = Self {
            dir: dir.to_path_buf(),
            data,
            settings,
            rules,
            read_only,
            notice,
            data_state,
            other_read_only,
        };
        store.data.revision = store.data.revision.max(read_hwm(dir));

        // Persist a fresh seed, a recovered backup, or default settings so the
        // next start is stable. A recovery rewrite must not rotate the known
        // corrupt main file into a backup slot.
        if !read_only && (recovered.is_some() || !store.path(DATA_FILE).exists()) {
            if let Err(err) = store.save_data_with(false) {
                logging::error(&format!("storage: initial save failed: {err}"));
                store.notice = Some("initial_save_failed".to_string());
            }
        }
        if !read_only && (settings_recovered.is_some() || !store.path(SETTINGS_FILE).exists()) {
            let settings = store.settings.clone();
            if let Err(err) = store.write_typed(SETTINGS_FILE, &settings, false) {
                logging::warn(&format!("storage: settings save failed: {err}"));
                store.notice = Some("settings_write_failed".to_string());
            }
        }
        if !read_only && (rules_recovered.is_some() || !store.path(RULES_FILE).exists()) {
            let rules = store.rules.clone();
            if let Err(err) = store.write_typed(RULES_FILE, &rules, false) {
                logging::warn(&format!("storage: rules save failed: {err}"));
                store.notice = Some("rules_write_failed".to_string());
            }
        }

        store
    }

    pub fn data(&self) -> &DataFile {
        &self.data
    }

    pub fn save_settings(&mut self, next: Settings) -> Result<(), String> {
        if self.read_only {
            return Err("read_only".to_string());
        }
        let previous = self.settings.clone();
        self.settings = next;
        match self.write_typed(SETTINGS_FILE, &self.settings, true) {
            Ok(()) => Ok(()),
            Err(err) => {
                self.settings = previous;
                Err(err)
            }
        }
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    pub fn set_rule(&mut self, exe: &str, action: &str) -> Result<(), String> {
        if self.read_only {
            return Err("read_only".to_string());
        }
        let exe = exe.trim();
        if exe.is_empty() || exe.encode_utf16().count() > 260 || normalize_exe(exe).is_empty() {
            return Err("rule_exe".to_string());
        }
        let action = match action {
            "paste" => "paste",
            "unicode" => "unicode",
            _ => return Err("rule_action".to_string()),
        };
        let normalized = normalize_exe(exe);
        if !self
            .rules
            .rules
            .iter()
            .any(|rule| normalize_exe(&rule.exe) == normalized)
            && self.rules.rules.len() >= 100
        {
            return Err("rule_limit".to_string());
        }
        let previous = self.rules.clone();
        if let Some(rule) = self
            .rules
            .rules
            .iter_mut()
            .find(|rule| normalize_exe(&rule.exe) == normalized)
        {
            rule.exe = exe.to_string();
            rule.action = action.to_string();
        } else {
            self.rules.rules.push(UserRule {
                exe: exe.to_string(),
                action: action.to_string(),
            });
        }
        match self.write_typed(RULES_FILE, &self.rules, true) {
            Ok(()) => Ok(()),
            Err(err) => {
                self.rules = previous;
                Err(err)
            }
        }
    }

    pub fn remove_rule(&mut self, exe: &str) -> Result<(), String> {
        if self.read_only {
            return Err("read_only".to_string());
        }
        let normalized = normalize_exe(exe);
        let previous = self.rules.clone();
        let before = self.rules.rules.len();
        self.rules
            .rules
            .retain(|rule| normalize_exe(&rule.exe) != normalized);
        if self.rules.rules.len() == before {
            return Err("rule_missing".to_string());
        }
        match self.write_typed(RULES_FILE, &self.rules, true) {
            Ok(()) => Ok(()),
            Err(err) => {
                self.rules = previous;
                Err(err)
            }
        }
    }

    pub fn action_for_exe(&self, exe: &str) -> Option<String> {
        let normalized = normalize_exe(exe);
        if normalized.is_empty() {
            return None;
        }
        self.rules
            .rules
            .iter()
            .find(|rule| normalize_exe(&rule.exe) == normalized)
            .map(|rule| rule.action.clone())
    }

    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    pub fn notice(&self) -> Option<&str> {
        self.notice.as_deref()
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Active snippets in panel order: pinned first, then sort order, then
    /// most recently updated.
    pub fn snippets(&self) -> Vec<&Snippet> {
        let mut items: Vec<&Snippet> = self
            .data
            .snippets
            .iter()
            .filter(|snippet| snippet.deleted_at.is_none())
            .collect();
        items.sort_by(|a, b| {
            b.pinned
                .cmp(&a.pinned)
                .then(a.sort_order.cmp(&b.sort_order))
                .then(b.updated_at.cmp(&a.updated_at))
        });
        items
    }

    pub fn category_name(&self, id: Option<&str>) -> String {
        let Some(id) = id else {
            return String::new();
        };
        self.data
            .categories
            .iter()
            .find(|category| category.id == id)
            .map(|category| category.name.clone())
            .unwrap_or_default()
    }

    pub fn save_data(&mut self) -> Result<(), String> {
        self.save_data_with(true)
    }

    fn save_data_with(&mut self, rotate_backup: bool) -> Result<(), String> {
        if self.read_only {
            return Err("read_only".to_string());
        }
        self.persist_data(rotate_backup)
    }

    // The confirmed recovery path may persist data without unlocking the store.
    fn persist_data(&mut self, rotate_backup: bool) -> Result<(), String> {
        // Content mutators rebuild the affected search blob themselves. The
        // blob is derived and is not serialized; `load_from` rebuilds it
        // unconditionally, so persistence never carries a stale blob.
        let previous_revision = self.data.revision;
        let next_revision = self
            .data
            .revision
            .max(read_hwm(&self.dir))
            .checked_add(1)
            .ok_or("revision_exhausted")?;
        write_hwm(&self.dir, next_revision)?;
        self.data.revision = next_revision;
        match self.write_typed(DATA_FILE, &self.data, rotate_backup) {
            Ok(()) => Ok(()),
            Err(err) => {
                // Restore the pre-call value: the high-water mark may already
                // be ahead, so `next_revision - 1` would leak an increment.
                self.data.revision = previous_revision;
                Err(err)
            }
        }
    }

    /// `useCount` counts attempts that reached `API_EVENTS_ACCEPTED` (§7).
    /// Rejections still update `lastInsertStatus` for diagnostics.
    pub fn record_use(&mut self, id: &str, status: &str) -> Result<(), String> {
        if self.read_only {
            return Err("read_only".to_string());
        }
        let now = now_iso();
        let Some(position) = self.data.snippets.iter().position(|item| item.id == id) else {
            return Err("snippet_missing".to_string());
        };
        let previous = self.data.snippets[position].clone();
        {
            let snippet = &mut self.data.snippets[position];
            if status == "accepted" {
                snippet.use_count += 1;
                snippet.last_used_at = Some(now.clone());
                snippet.updated_at = now;
            }
            snippet.last_insert_status = Some(status.to_string());
        }
        // Usage metadata must not consume a content restore point.
        match self.save_data_with(false) {
            Ok(()) => Ok(()),
            Err(err) => {
                self.data.snippets[position] = previous;
                Err(err)
            }
        }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    fn write_typed<T: Serialize>(
        &self,
        name: &str,
        value: &T,
        rotate_backup: bool,
    ) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(value).map_err(|err| err.to_string())?;
        write_atomic(&self.dir, name, &bytes, rotate_backup)
    }
}

fn seed() -> DataFile {
    let now = now_iso();
    let category = Category {
        id: new_id(),
        name: "工作沟通".to_string(),
        sort_order: 0,
        created_at: now.clone(),
        updated_at: now.clone(),
    };
    let development = Category {
        id: new_id(),
        name: "开发相关".to_string(),
        sort_order: 1,
        created_at: now.clone(),
        updated_at: now.clone(),
    };
    let mut snippets = vec![
        snippet(
            "项目进度回复",
            "您好，目前该功能已完成开发，正在进行测试验证，预计测试通过后安排发布。",
            Some(category.id.clone()),
            0,
            &now,
        ),
        snippet(
            "收到回复",
            "好的，已收到，我会尽快处理。",
            Some(category.id.clone()),
            1,
            &now,
        ),
        snippet(
            "接口开发说明",
            "接口采用 RESTful 风格，基于 HTTP 协议，返回格式为 JSON，具体参数如下：",
            Some(development.id.clone()),
            2,
            &now,
        ),
    ];
    for item in &mut snippets {
        item.rebuild_blob();
    }
    DataFile {
        schema_version: SCHEMA_VERSION,
        revision: 0,
        categories: vec![category, development],
        snippets,
    }
}

fn snippet(
    title: &str,
    content: &str,
    category_id: Option<String>,
    sort_order: i32,
    now: &str,
) -> Snippet {
    Snippet {
        id: new_id(),
        title: title.to_string(),
        content: content.to_string(),
        category_id,
        pinned: false,
        sort_order,
        created_at: now.to_string(),
        updated_at: now.to_string(),
        use_count: 0,
        last_used_at: None,
        last_insert_status: None,
        deleted_at: None,
        tags: Vec::new(),
        hotkey: None,
        search_blob: String::new(),
    }
}

/// Read `name`, falling back to backups newest-first. Returns the value and the
/// backup path when recovery was needed. `Ok((None, _))` means the file does not
/// exist.
fn read_with_recovery<T: DeserializeOwned>(
    dir: &Path,
    name: &str,
) -> Result<(Option<T>, Option<PathBuf>), String> {
    let path = dir.join(name);
    if !path.exists() {
        // A missing main file with surviving backups means the user deleted or
        // lost it; recover rather than silently seeding over the old data.
        let candidates = backups(dir, name);
        for backup in &candidates {
            match read_json::<T>(backup) {
                Ok(value) => {
                    logging::warn(&format!(
                        "storage: {name} missing; recovered from {}",
                        backup.display()
                    ));
                    return Ok((Some(value), Some(backup.clone())));
                }
                Err(err) => logging::warn(&format!(
                    "storage: backup {} unusable ({err})",
                    backup.display()
                )),
            }
        }
        if candidates.is_empty() {
            return Ok((None, None));
        }
        return Err("all_backups_failed".to_string());
    }

    match read_json::<T>(&path) {
        Ok(value) => return Ok((Some(value), None)),
        Err(err) => logging::warn(&format!(
            "storage: {name} unreadable ({err}); trying backups"
        )),
    }

    for backup in backups(dir, name) {
        match read_json::<T>(&backup) {
            Ok(value) => {
                logging::warn(&format!(
                    "storage: recovered {name} from {}",
                    backup.display()
                ));
                return Ok((Some(value), Some(backup)));
            }
            Err(err) => logging::warn(&format!(
                "storage: backup {} unusable ({err})",
                backup.display()
            )),
        }
    }

    Err("all_backups_failed".to_string())
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T, String> {
    let bytes = fs::read(path).map_err(|err| err.to_string())?;
    serde_json::from_slice(&bytes).map_err(|err| err.to_string())
}

/// Read only the version field so a file from a newer program is never treated
/// as corrupt and replaced by an older backup.
fn peek_version(path: &Path, key: &str) -> Option<u64> {
    let bytes = fs::read(path).ok()?;
    json_version(&bytes, key)
}

fn json_version(bytes: &[u8], key: &str) -> Option<u64> {
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    let value: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    value.get(key)?.as_u64()
}

fn read_hwm(dir: &Path) -> u64 {
    fs::read_to_string(dir.join(HWM_FILE))
        .ok()
        .and_then(|text| text.trim().parse().ok())
        .unwrap_or(0)
}

/// Reserve a revision before writing the data file. An interrupted write
/// leaves a gap, never a duplicate.
fn write_hwm(dir: &Path, value: u64) -> Result<(), String> {
    write_atomic(dir, HWM_FILE, value.to_string().as_bytes(), false)
}

fn backups(dir: &Path, name: &str) -> Vec<PathBuf> {
    let stem = name.trim_end_matches(".json");
    let directory = dir.join("backups");
    let Ok(entries) = fs::read_dir(&directory) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|value| value.to_str())
                .map(|value| value.starts_with(&format!("{stem}-")) && value.ends_with(".json"))
                .unwrap_or(false)
        })
        .collect();
    paths.sort();
    paths.reverse();
    paths
}

fn backup_file(dir: &Path, name: &str) -> Result<(), String> {
    let source = dir.join(name);
    if !source.exists() {
        return Ok(());
    }
    let directory = dir.join("backups");
    fs::create_dir_all(&directory).map_err(|err| err.to_string())?;
    let stem = name.trim_end_matches(".json");
    let target = directory.join(format!("{stem}-{}-{:04}.json", now_stamp(), backup_seq()));
    let temp = directory.join(format!("{stem}.tmp"));
    fs::copy(&source, &temp).map_err(|err| err.to_string())?;
    if let Err(err) = sync_file(&temp) {
        let _ = fs::remove_file(&temp);
        return Err(err);
    }
    if let Err(err) = move_file(&temp, &target, true) {
        let _ = fs::remove_file(&temp);
        return Err(err);
    }
    sync_file(&target)?;
    Ok(())
}

/// Prune only after the main file was replaced successfully, so a failed
/// replace cannot consume the oldest restore point.
fn prune_backups(dir: &Path, name: &str) {
    let mut existing = backups(dir, name);
    while existing.len() > BACKUP_KEEP {
        let oldest = existing.pop();
        let Some(oldest) = oldest else {
            break;
        };
        if let Err(err) = fs::remove_file(&oldest) {
            logging::warn(&format!(
                "storage: cannot prune backup {}: {err}",
                oldest.display()
            ));
            break;
        }
    }
}

fn backup_seq() -> u64 {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    SEQ.fetch_add(1, Ordering::Relaxed)
}

fn sync_file(path: &Path) -> Result<(), String> {
    let file = fs::OpenOptions::new()
        .write(true)
        .open(path)
        .map_err(|err| err.to_string())?;
    file.sync_all().map_err(|err| err.to_string())
}

fn clean_stale_temp(dir: &Path) {
    for name in [DATA_FILE, SETTINGS_FILE, RULES_FILE, HWM_FILE] {
        let _ = fs::remove_file(dir.join(format!("{name}.tmp")));
    }
}

/// Resolve junctions and alternate spellings before allowing an export.
fn export_target(dir: &Path, path: &Path) -> Result<PathBuf, String> {
    fn inside(path: &Path, dir: &Path) -> bool {
        path.ancestors().any(|part| {
            part.to_string_lossy().to_uppercase() == dir.to_string_lossy().to_uppercase()
        })
    }
    let name = path.file_name().ok_or("export_path")?;
    let name_text = name.to_string_lossy();
    if name_text.contains(':') || name_text.ends_with(['.', ' ']) {
        return Err("export_path".into());
    }
    let resolve = |p: &Path| fs::canonicalize(p).map_err(|err| err.to_string());
    let parent = resolve(path.parent().ok_or("export_path")?)?;
    let root = resolve(dir)?;
    let backup_root = fs::canonicalize(dir.join("backups")).ok();
    let protected =
        |p: &Path| inside(p, &root) || backup_root.as_ref().is_some_and(|b| inside(p, b));
    let target = parent.join(name);
    if protected(&target) {
        return Err("export_protected".into());
    }
    match fs::symlink_metadata(&target) {
        Ok(meta) => {
            if !meta.is_file() || protected(&resolve(&target)?) {
                return Err("export_protected".into());
            }
            // Refuse hard-link aliases too; the export must be a separate file.
            let file = fs::File::open(&target).map_err(|err| err.to_string())?;
            let mut info = BY_HANDLE_FILE_INFORMATION::default();
            unsafe { GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info) }
                .map_err(|err| err.to_string())?;
            if info.nNumberOfLinks > 1 {
                return Err("export_protected".into());
            }
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(err.to_string()),
    }
    Ok(target)
}

/// Create an exclusive, flushed file; never remove a pre-existing name.
fn write_new_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|err| err.to_string())?;
    let result = file.write_all(bytes).and_then(|()| file.sync_all());
    drop(file);
    if result.is_err() {
        let _ = fs::remove_file(path);
    }
    result.map_err(|err| err.to_string())
}

fn write_atomic(dir: &Path, name: &str, bytes: &[u8], rotate_backup: bool) -> Result<(), String> {
    let target = dir.join(name);
    let temp = dir.join(format!("{name}.tmp"));

    let result = (|| -> Result<(), String> {
        let mut file = fs::File::create(&temp).map_err(|err| err.to_string())?;
        file.write_all(bytes).map_err(|err| err.to_string())?;
        file.sync_all().map_err(|err| err.to_string())?;
        drop(file);

        let written = fs::read(&temp).map_err(|err| err.to_string())?;
        if written != bytes {
            return Err("temp_verify_failed".to_string());
        }

        if target.exists() {
            if rotate_backup {
                backup_file(dir, name)?;
            }
            if replace_file(&target, &temp).is_err() {
                // Never fall back if the replace consumed the target: that
                // would turn a recoverable failure into data loss.
                if !target.exists() {
                    return Err("replace_lost_target".to_string());
                }
                move_file(&temp, &target, true)?;
            }
        } else {
            move_file(&temp, &target, false)?;
        }
        if rotate_backup {
            prune_backups(dir, name);
        }
        Ok(())
    })();

    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn replace_file(target: &Path, replacement: &Path) -> Result<(), String> {
    let target = platform::wide(&target.to_string_lossy());
    let replacement = platform::wide(&replacement.to_string_lossy());
    unsafe {
        ReplaceFileW(
            PCWSTR(target.as_ptr()),
            PCWSTR(replacement.as_ptr()),
            PCWSTR::null(),
            REPLACEFILE_IGNORE_MERGE_ERRORS | REPLACEFILE_WRITE_THROUGH,
            None,
            None,
        )
        .map_err(|err| err.to_string())
    }
}

fn move_file(source: &Path, target: &Path, replace: bool) -> Result<(), String> {
    let source: Vec<u16> = source.as_os_str().encode_wide().chain([0]).collect();
    let target: Vec<u16> = target.as_os_str().encode_wide().chain([0]).collect();
    let flags = if replace {
        MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH
    } else {
        MOVEFILE_WRITE_THROUGH
    };
    unsafe {
        MoveFileExW(PCWSTR(source.as_ptr()), PCWSTR(target.as_ptr()), flags)
            .map_err(|err| err.to_string())
    }
}

fn now_iso() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let days = seconds.div_euclid(86_400);
    let rest = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        (rest % 3600) / 60,
        rest % 60
    )
}

fn utc_now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

/// Parse exactly the canonical `YYYY-MM-DDTHH:MM:SSZ` form written by the app.
/// Anything else (offsets, fractional seconds, empty strings) returns `None`
/// so the recycle purge can leave it alone instead of comparing text.
fn parse_utc_stamp(stamp: &str) -> Option<i64> {
    let bytes = stamp.as_bytes();
    if bytes.len() != 20
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b'T'
        || bytes[13] != b':'
        || bytes[16] != b':'
        || bytes[19] != b'Z'
    {
        return None;
    }
    // `parse()` accepts a leading sign; a time stamp is digits only.
    const DIGITS: [usize; 14] = [0, 1, 2, 3, 5, 6, 8, 9, 11, 12, 14, 15, 17, 18];
    if DIGITS.iter().any(|index| !bytes[*index].is_ascii_digit()) {
        return None;
    }
    let year: i64 = stamp.get(0..4)?.parse().ok()?;
    let month: i64 = stamp.get(5..7)?.parse().ok()?;
    let day: i64 = stamp.get(8..10)?.parse().ok()?;
    let hour: i64 = stamp.get(11..13)?.parse().ok()?;
    let minute: i64 = stamp.get(14..16)?.parse().ok()?;
    let second: i64 = stamp.get(17..19)?.parse().ok()?;
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || !(0..=23).contains(&hour)
        || !(0..=59).contains(&minute)
        || !(0..=59).contains(&second)
    {
        return None;
    }
    let days = days_from_civil(year, month, day);
    if civil_from_days(days) != (year as i32, month as u32, day as u32) {
        return None;
    }
    // `parse()` accepts a leading `+`; require the canonical spelling instead
    // of silently normalizing it. Anything else stays out of the purge.
    let normalized = format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z");
    if normalized != stamp {
        return None;
    }
    Some(days * 86_400 + hour * 3_600 + minute * 60 + second)
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn now_stamp() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let seconds = now.as_secs() as i64;
    let millis = now.subsec_millis();
    let days = seconds.div_euclid(86_400);
    let rest = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}{month:02}{day:02}T{:02}{:02}{:02}{millis:03}Z",
        rest / 3600,
        (rest % 3600) / 60,
        rest % 60
    )
}

fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    ((year + i64::from(month <= 2)) as i32, month, day)
}

fn new_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = u64::from(std::process::id());
    let mut state = nanos ^ (counter << 32) ^ (pid << 16) ^ 0x9E37_79B9_7F4A_7C15;
    let mut bytes = [0u8; 16];
    for chunk in bytes.chunks_mut(8) {
        state ^= state >> 30;
        state = state.wrapping_mul(0xBF58_476D_1CE4_E5B9);
        state ^= state >> 27;
        state = state.wrapping_mul(0x94D0_49BB_1331_11EB);
        state ^= state >> 31;
        chunk.copy_from_slice(&state.to_le_bytes());
    }
    // RFC 4122 version 4 layout.
    bytes[6] = (bytes[6] & 0x0F) | 0x40;
    bytes[8] = (bytes[8] & 0x3F) | 0x80;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("qp-store-{tag}-{}", new_id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn export_protects_local_files_even_for_an_unreadable_future_library() {
        let outer = temp_dir("export-protect");
        let dir = outer.join("library");
        let store = Store::load_from(&dir);
        let main = dir.join(DATA_FILE);
        let future = br#"{"schemaVersion":99,"snippets":{"future":true}}"#;
        fs::write(&main, future).unwrap();
        let alias = outer.join("alias.json");
        fs::hard_link(&main, &alias).unwrap();
        let backups = dir.join("backups");
        fs::create_dir_all(&backups).unwrap();
        let unrelated = dir.join("user.tmp");
        fs::write(&unrelated, b"keep me").unwrap();
        let locked = Store::load_from(&dir);
        assert!(locked.is_read_only());
        for dest in [
            main,
            dir.join("DATA.JSON"),
            dir.join(HWM_FILE),
            backups.join("data-example.json"),
            alias.clone(),
        ] {
            assert_eq!(locked.export_to(&dest), Err("export_protected".into()));
        }
        assert_eq!(fs::read(alias).unwrap(), future);
        assert_eq!(fs::read(unrelated).unwrap(), b"keep me");
        let export = outer.join("export.json");
        fs::write(&export, b"previous export").unwrap();
        assert_eq!(
            locked.export_to(&export),
            Err("export_data_unavailable".into())
        );
        assert_eq!(fs::read(&export).unwrap(), b"previous export");
        store.export_to(&export).unwrap();
        assert!(read_json::<DataFile>(&export).is_ok());
        let _ = fs::remove_dir_all(outer);
    }

    #[test]
    fn usage_updates_keep_all_content_restore_points() {
        let dir = temp_dir("usage-backups");
        let mut store = Store::load_from(&dir);
        let id = store
            .add_snippet("usage", "stored text", None, false)
            .unwrap();
        let before = backups(&dir, DATA_FILE);
        assert!(!before.is_empty());
        for _ in 0..BACKUP_KEEP + 1 {
            store.record_use(&id, "rejected").unwrap();
        }
        store.record_use(&id, "accepted").unwrap();
        assert_eq!(backups(&dir, DATA_FILE), before);
        assert_eq!(Store::load_from(&dir).get(&id).unwrap().use_count, 1);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn confirmed_recovery_preserves_corrupt_main_and_removes_conflicting_hotkey() {
        let dir = temp_dir("manual-recovery");
        let original = Store::load_from(&dir);
        let mut backup = original.data.clone();
        backup.snippets[0].hotkey = Some(original.settings.hotkey.clone());
        let bytes = serde_json::to_vec(&backup).unwrap();
        let main = dir.join(DATA_FILE);
        fs::write(&main, b"damaged library").unwrap();
        let mut broken = Store::load_from(&dir);
        assert!(broken.is_read_only());
        assert_eq!(
            broken.import_bytes(&bytes, ImportMode::Replace),
            Err("read_only".into())
        );
        let blocked_temp = dir.join("data.json.tmp");
        fs::create_dir(&blocked_temp).unwrap();
        assert!(broken.restore_bytes(&bytes).is_err());
        assert!(broken.is_read_only());
        assert_eq!(fs::read(&main).unwrap(), b"damaged library");
        fs::remove_dir(&blocked_temp).unwrap();
        assert_eq!(broken.restore_bytes(&bytes), Ok(1));
        assert!(!broken.is_read_only());
        assert!(backups(&dir, DATA_FILE).is_empty());
        assert!(fs::read_dir(&dir).unwrap().flatten().any(|file| {
            file.file_name()
                .to_string_lossy()
                .starts_with("data-corrupt-")
                && fs::read(file.path()).unwrap() == b"damaged library"
        }));
        let reloaded = Store::load_from(&dir);
        assert_eq!(
            reloaded.data.snippets[0].content,
            backup.snippets[0].content
        );
        assert_eq!(reloaded.data.snippets[0].hotkey, None);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn recovery_does_not_unlock_unrelated_settings_protection_or_replace_incompatible_data() {
        let dir = temp_dir("manual-recovery-guards");
        let initial = Store::load_from(&dir);
        let bytes = initial.export_json().unwrap();
        let main = dir.join(DATA_FILE);
        let settings = dir.join(SETTINGS_FILE);
        fs::write(&main, b"damaged library").unwrap();
        fs::write(&settings, b"damaged settings").unwrap();
        let mut broken = Store::load_from(&dir);
        assert_eq!(broken.restore_bytes(bytes.as_bytes()), Ok(0));
        assert!(broken.is_read_only());
        assert_eq!(fs::read(&settings).unwrap(), b"damaged settings");
        assert_eq!(
            fs::read_dir(dir.join("backups"))
                .map(|d| d.count())
                .unwrap_or(0),
            0
        );
        for version in [0, SCHEMA_VERSION + 1] {
            let incompatible = format!(r#"{{"schemaVersion":{version},"snippets":{{}}}}"#);
            fs::write(&main, &incompatible).unwrap();
            let mut locked = Store::load_from(&dir);
            assert_eq!(
                locked.restore_bytes(bytes.as_bytes()),
                Err("read_only".into())
            );
            assert_eq!(fs::read_to_string(&main).unwrap(), incompatible);
        }
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn combined_exports_above_merge_quota_can_be_replaced_or_restored() {
        let dir = temp_dir("large-library");
        let mut store = Store::load_from(&dir);
        let prototype = store.data.snippets[0].clone();
        for batch in 0..2 {
            // Two independently valid merge inputs whose combined UTF-8 body is over 20 MiB.
            let batch = DataFile {
                snippets: (0..360)
                    .map(|index| Snippet {
                        id: format!("bulk-{batch}-{index}"),
                        title: format!("Library {batch} / {index}"),
                        content: "字".repeat(CONTENT_MAX_UNITS),
                        category_id: None,
                        ..prototype.clone()
                    })
                    .collect(),
                ..DataFile::default()
            };
            let bytes = serde_json::to_vec(&batch).unwrap();
            assert!(bytes.len() < IMPORT_MAX_BYTES);
            store.import_bytes(&bytes, ImportMode::Merge).unwrap();
        }
        let export = store.export_json().unwrap();
        let count = store.data.snippets.len();
        assert!(export.len() > IMPORT_MAX_BYTES);
        assert_eq!(
            store.import_bytes(export.as_bytes(), ImportMode::Merge),
            Err("import_too_large".into())
        );
        let target_dir = temp_dir("large-library-target");
        let mut target = Store::load_from(&target_dir);
        assert_eq!(
            target
                .import_bytes(export.as_bytes(), ImportMode::Replace)
                .unwrap()
                .snippets_added,
            count
        );
        assert_eq!(target.restore_bytes(export.as_bytes()), Ok(0));
        assert_eq!(target.data.snippets.len(), count);
        let _ = fs::remove_dir_all(dir);
        let _ = fs::remove_dir_all(target_dir);
    }

    #[test]
    fn old_settings_without_insertion_mode_stay_writable() {
        let dir = temp_dir("oldmode");
        let store = Store::load_from(&dir);
        assert!(!store.is_read_only());
        let path = dir.join(SETTINGS_FILE);
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        value.as_object_mut().unwrap().remove("insertionMode");
        value.as_object_mut().unwrap().remove("panelSort");
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();

        let reloaded = Store::load_from(&dir);
        assert!(!reloaded.is_read_only());
        assert_eq!(reloaded.settings().insertion_mode, "unicode");
        assert_eq!(reloaded.settings().panel_sort, "recent");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn tags_are_normalized_and_searchable() {
        let dir = temp_dir("tags");
        let mut store = Store::load_from(&dir);
        let tags = vec![
            " 退款 ".to_string(),
            "客服".to_string(),
            "support".to_string(),
            "退款".to_string(),
        ];
        store
            .add_snippet_tagged("退款说明", "请按流程处理", None, false, &tags)
            .expect("tagged snippet");
        let snippets = store.snippets();
        let snippet = snippets
            .iter()
            .find(|snippet| snippet.title == "退款说明")
            .expect("stored snippet");
        assert_eq!(snippet.tags, vec!["退款", "客服", "support"]);
        let docs = [crate::search::Doc {
            id: &snippet.id,
            title: &snippet.title,
            content: &snippet.content,
            blob: &snippet.search_blob,
            pinned: false,
            use_count: snippet.use_count,
            last_used_at: None,
            updated_at: &snippet.updated_at,
            sort_order: snippet.sort_order,
        }];
        let tag_hits = crate::search::rank(&docs, "客服", 10);
        assert_eq!(tag_hits.len(), 1);
        assert_eq!(tag_hits[0].score, 85);
        assert_eq!(crate::search::rank(&docs, "support", 10)[0].score, 85);
        assert_eq!(crate::search::rank(&docs, "tui", 10)[0].score, 90);
        assert_eq!(crate::search::rank(&docs, "流程", 10)[0].score, 100);
        assert_eq!(
            store.add_snippet_tagged("超长标签", "正文", None, false, &["x".repeat(25)]),
            Err("tag_too_long".to_string())
        );
        assert_eq!(
            store.add_snippet_tagged("逗号标签", "正文", None, false, &["a,b".to_string()]),
            Err("tag_delimiter".to_string())
        );
        let mut imported = store.data().clone();
        imported.snippets[0].tags = vec!["a,b".to_string()];
        assert_eq!(
            store.import_bytes(&serde_json::to_vec(&imported).unwrap(), ImportMode::Replace),
            Err("import_tags".to_string())
        );
        let too_many: Vec<String> = (0..11).map(|index| format!("t{index}")).collect();
        assert_eq!(
            store.add_snippet_tagged("过多标签", "正文", None, false, &too_many),
            Err("too_many_tags".to_string())
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn batch_tags_add_and_remove_are_atomic() {
        let dir = temp_dir("batchtags");
        let mut store = Store::load_from(&dir);
        let first = store
            .add_snippet_full("甲", "正文", None, false, &["x".to_string()], None)
            .expect("first");
        let second = store
            .add_snippet_full("乙", "正文", None, false, &[], None)
            .expect("second");

        let before_revision = store.data().revision;
        assert_eq!(
            store.add_tags(&[first.clone(), second.clone()], &["y".to_string()]),
            Ok(2)
        );
        assert_eq!(store.data().revision, before_revision + 1);
        assert_eq!(
            store.get(&first).unwrap().tags,
            vec!["x".to_string(), "y".to_string()]
        );
        assert!(store.get(&first).unwrap().search_blob.contains("y"));
        assert_eq!(store.get(&second).unwrap().tags, vec!["y".to_string()]);
        assert_eq!(
            store.add_tags(std::slice::from_ref(&second), &["Y".to_string()]),
            Ok(0)
        );
        assert_eq!(
            store.remove_tags(std::slice::from_ref(&first), &["x".to_string()]),
            Ok(1)
        );
        assert_eq!(store.get(&first).unwrap().tags, vec!["y".to_string()]);

        let too_many: Vec<String> = (0..10).map(|index| format!("z{index}")).collect();
        assert_eq!(
            store.add_tags(std::slice::from_ref(&second), &too_many),
            Err("too_many_tags".to_string())
        );
        assert_eq!(store.get(&second).unwrap().tags, vec!["y".to_string()]);
        assert_eq!(
            store.add_tags(std::slice::from_ref(&first), &[]),
            Err("tag_empty".to_string())
        );
        store.read_only = true;
        assert_eq!(
            store.remove_tags(std::slice::from_ref(&second), &["y".to_string()]),
            Err("read_only".to_string())
        );
        store.read_only = false;
        assert_eq!(
            store.add_tags(&[first.clone(), "missing".to_string()], &["z".to_string()]),
            Err("snippet_missing".to_string())
        );
        assert!(!store.get(&first).unwrap().tags.contains(&"z".to_string()));
        store.soft_delete(&first).expect("delete");
        assert_eq!(
            store.add_tags(std::slice::from_ref(&first), &["deleted".to_string()]),
            Ok(1)
        );
        assert!(store
            .get(&first)
            .unwrap()
            .tags
            .contains(&"deleted".to_string()));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn batch_tags_roll_back_on_write_failure() {
        let dir = temp_dir("batchtagsfail");
        let mut store = Store::load_from(&dir);
        let first = store
            .add_snippet_full("甲", "正文", None, false, &["x".to_string()], None)
            .expect("first");
        let second = store
            .add_snippet_full("乙", "正文", None, false, &["y".to_string()], None)
            .expect("second");
        let before_first = store.get(&first).unwrap().tags.clone();
        let before_second = store.get(&second).unwrap().tags.clone();
        fs::create_dir_all(dir.join(format!("{DATA_FILE}.tmp"))).unwrap();
        assert!(store
            .add_tags(&[first.clone(), second.clone()], &["z".to_string()])
            .is_err());
        assert_eq!(store.get(&first).unwrap().tags, before_first);
        assert_eq!(store.get(&second).unwrap().tags, before_second);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn old_data_without_tags_loads_with_empty_tags() {
        let dir = temp_dir("oldtags");
        let mut store = Store::load_from(&dir);
        store
            .add_snippet_tagged("旧格式", "正文", None, false, &["tag".to_string()])
            .expect("snippet");
        let path = dir.join(DATA_FILE);
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        for snippet in value["snippets"].as_array_mut().unwrap() {
            snippet.as_object_mut().unwrap().remove("tags");
        }
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        let reloaded = Store::load_from(&dir);
        assert!(reloaded
            .snippets()
            .iter()
            .all(|snippet| snippet.tags.is_empty()));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn hotkeys_are_normalized_unique_and_exported() {
        let dir = temp_dir("hotkeys");
        let mut store = Store::load_from(&dir);
        let first = store
            .add_snippet_full("快捷键短语", "正文", None, false, &[], Some(" ctrl+alt+1 "))
            .expect("snippet with hotkey");
        assert_eq!(
            store.get(&first).unwrap().hotkey.as_deref(),
            Some("Ctrl+Alt+1")
        );
        store
            .update_snippet_tagged(&first, "快捷键短语", "正文", None, false, &[])
            .expect("tagged wrapper");
        assert_eq!(
            store.get(&first).unwrap().hotkey.as_deref(),
            Some("Ctrl+Alt+1")
        );
        assert_eq!(
            store.add_snippet_full("重复快捷键", "正文", None, false, &[], Some("CTRL+ALT+1")),
            Err("hotkey_duplicate".to_string())
        );
        assert_eq!(
            store.add_snippet_full("无效快捷键", "正文", None, false, &[], Some("Q")),
            Err("hotkey_invalid".to_string())
        );
        assert_eq!(
            store.add_snippet_full("全局冲突", "正文", None, false, &[], Some("Ctrl+Alt+Q")),
            Err("hotkey_panel_conflict".to_string())
        );
        store
            .update_snippet_full(
                &first,
                "快捷键短语",
                "正文",
                None,
                false,
                &[],
                Some("Ctrl+Alt+1"),
            )
            .expect("same hotkey on update");
        store
            .update_snippet_full(
                &first,
                "快捷键短语",
                "正文",
                None,
                false,
                &[],
                Some("Ctrl+Shift+F2"),
            )
            .expect("change hotkey");
        assert_eq!(
            store.get(&first).unwrap().hotkey.as_deref(),
            Some("Ctrl+Shift+F2")
        );
        let exported = store.export_json().unwrap();
        assert!(exported.contains("\"hotkey\": \"Ctrl+Shift+F2\""));
        store
            .update_snippet_full(&first, "快捷键短语", "正文", None, false, &[], None)
            .expect("clear hotkey");
        assert!(store.get(&first).unwrap().hotkey.is_none());
        assert!(!store.export_json().unwrap().contains("\"hotkey\""));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn hotkey_survives_recycle_and_restore() {
        let dir = temp_dir("hotkeyrecycle");
        let mut store = Store::load_from(&dir);
        let id = store
            .add_snippet_full("快捷键短语", "正文", None, false, &[], Some("Ctrl+Alt+9"))
            .expect("snippet");
        store.soft_delete(&id).expect("delete");
        assert_eq!(
            store.deleted_snippets()[0].hotkey.as_deref(),
            Some("Ctrl+Alt+9")
        );
        store.restore(&id).expect("restore");
        assert_eq!(
            store.get(&id).unwrap().hotkey.as_deref(),
            Some("Ctrl+Alt+9")
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn restore_rejects_panel_hotkey_conflict() {
        let dir = temp_dir("hotkeypanelrestore");
        let mut store = Store::load_from(&dir);
        let id = store
            .add_snippet_full("快捷键短语", "正文", None, false, &[], Some("Ctrl+Alt+9"))
            .expect("snippet");
        store.soft_delete(&id).expect("delete");
        let settings_path = dir.join(SETTINGS_FILE);
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&settings_path).unwrap()).unwrap();
        value["hotkey"] = serde_json::json!("Ctrl+Alt+9");
        fs::write(&settings_path, serde_json::to_vec(&value).unwrap()).unwrap();
        let mut reloaded = Store::load_from(&dir);
        assert_eq!(
            reloaded.restore(&id),
            Err("hotkey_panel_conflict".to_string())
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn restore_rejects_hotkey_collision_between_recycle_bin_entries() {
        let dir = temp_dir("hotkeyrecycledup");
        let mut store = Store::load_from(&dir);
        let first = store
            .add_snippet_full("第一个", "正文", None, false, &[], Some("Ctrl+Alt+8"))
            .expect("first");
        let second = store
            .add_snippet_full("第二个", "正文", None, false, &[], Some("Ctrl+Alt+7"))
            .expect("second");
        store.soft_delete(&first).expect("delete first");
        store.soft_delete(&second).expect("delete second");
        let path = dir.join(DATA_FILE);
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        for snippet in value["snippets"].as_array_mut().unwrap() {
            if snippet["id"] == serde_json::json!(second) {
                snippet["hotkey"] = serde_json::json!("Ctrl+Alt+8");
            }
        }
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        let mut reloaded = Store::load_from(&dir);
        assert_eq!(
            reloaded.restore(&first),
            Err("hotkey_duplicate".to_string())
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn import_rejects_invalid_conflicting_or_duplicate_hotkeys() {
        let dir = temp_dir("hotkeyimport");
        let mut store = Store::load_from(&dir);
        store
            .add_snippet_full("已有", "正文", None, false, &[], Some("Ctrl+Alt+2"))
            .expect("snippet");

        let mut data = store.data().clone();
        data.snippets[0].hotkey = Some("Q".to_string());
        assert_eq!(
            store.import_bytes(&serde_json::to_vec(&data).unwrap(), ImportMode::Replace),
            Err("import_hotkey".to_string())
        );

        let mut data = store.data().clone();
        let mut second = data.snippets[0].clone();
        second.id = "second".to_string();
        second.title = "第二个".to_string();
        second.hotkey = Some("Ctrl+Alt+2".to_string());
        data.snippets.push(second);
        assert_eq!(
            store.import_bytes(&serde_json::to_vec(&data).unwrap(), ImportMode::Replace),
            Err("import_hotkey".to_string())
        );

        let mut data = store.data().clone();
        data.snippets[0].hotkey = Some("Ctrl+Alt+Q".to_string());
        assert_eq!(
            store.import_bytes(&serde_json::to_vec(&data).unwrap(), ImportMode::Replace),
            Err("import_hotkey".to_string())
        );

        let mut data = store.data().clone();
        data.snippets[0].hotkey = Some("Ctrl+Alt+3".to_string());
        store
            .import_bytes(&serde_json::to_vec(&data).unwrap(), ImportMode::Replace)
            .expect("valid hotkey import");
        assert_eq!(store.snippets()[0].hotkey.as_deref(), Some("Ctrl+Alt+3"));

        let mut merge = store.data().clone();
        merge.snippets[0].id = "merge".to_string();
        merge.snippets[0].title = "合并短语".to_string();
        assert_eq!(
            store.import_bytes(&serde_json::to_vec(&merge).unwrap(), ImportMode::Merge),
            Err("import_hotkey".to_string())
        );

        let mut skipped = store.data().clone();
        skipped.snippets[0].id = "skipped".to_string();
        skipped.snippets[0].hotkey = Some("Ctrl+Alt+3".to_string());
        let summary = store
            .import_bytes(&serde_json::to_vec(&skipped).unwrap(), ImportMode::Merge)
            .expect("skipped title duplicate must not block merge");
        assert_eq!(summary.snippets_added, 0);

        let mut pair = store.data().clone();
        let mut skipped_pair = pair.snippets[0].clone();
        skipped_pair.id = "skip-a".to_string();
        skipped_pair.hotkey = Some("Ctrl+Alt+5".to_string());
        let mut inserted_pair = pair.snippets[0].clone();
        inserted_pair.id = "insert-b".to_string();
        inserted_pair.title = "新条目".to_string();
        inserted_pair.hotkey = Some("Ctrl+Alt+5".to_string());
        pair.snippets = vec![skipped_pair, inserted_pair];
        let summary = store
            .import_bytes(&serde_json::to_vec(&pair).unwrap(), ImportMode::Merge)
            .expect("skipped title duplicate must not cause internal hotkey rejection");
        assert_eq!(summary.snippets_added, 1);

        let existing_id = store.snippets()[0].id.clone();
        store.soft_delete(&existing_id).expect("soft delete");
        let mut deleted_conflict = store.data().clone();
        deleted_conflict.snippets[0].id = "deleted-conflict".to_string();
        deleted_conflict.snippets[0].title = "回收站冲突".to_string();
        deleted_conflict.snippets[0].deleted_at = None;
        assert_eq!(
            store.import_bytes(
                &serde_json::to_vec(&deleted_conflict).unwrap(),
                ImportMode::Merge
            ),
            Err("import_hotkey".to_string())
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn old_data_without_hotkey_loads_as_none() {
        let dir = temp_dir("oldhotkey");
        let mut store = Store::load_from(&dir);
        store
            .add_snippet_full("旧格式", "正文", None, false, &[], Some("Ctrl+Alt+4"))
            .expect("snippet");
        let path = dir.join(DATA_FILE);
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        for snippet in value["snippets"].as_array_mut().unwrap() {
            snippet.as_object_mut().unwrap().remove("hotkey");
        }
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        let reloaded = Store::load_from(&dir);
        assert!(reloaded
            .snippets()
            .iter()
            .all(|snippet| snippet.hotkey.is_none()));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn pinyin_keys_support_full_spelling_and_initials() {
        let (full, compact, initials) = pinyin_keys("退款说明");
        assert!(full.contains("tui kuan shuo ming"));
        assert!(compact.contains("tuikuanshuoming"));
        assert!(initials.starts_with("tksm"));
    }

    #[test]
    fn pinyin_initials_and_full_spelling_are_searchable() {
        let dir = temp_dir("pinyin");
        let mut store = Store::load_from(&dir);
        store
            .add_snippet("退款说明", "请按流程处理", None, false)
            .expect("snippet");
        let snippets = store.snippets();
        let snippet = snippets
            .iter()
            .find(|snippet| snippet.title == "退款说明")
            .expect("stored snippet");
        let docs = [crate::search::Doc {
            id: &snippet.id,
            title: &snippet.title,
            content: &snippet.content,
            blob: &snippet.search_blob,
            pinned: false,
            use_count: snippet.use_count,
            last_used_at: None,
            updated_at: &snippet.updated_at,
            sort_order: snippet.sort_order,
        }];
        assert!(
            snippet.search_blob.contains("tksm"),
            "blob={}",
            snippet.search_blob
        );
        assert_eq!(crate::search::rank(&docs, "tk", 10).len(), 1);
        assert_eq!(crate::search::rank(&docs, "tuikuan", 10).len(), 1);
        assert_eq!(crate::search::rank(&docs, "t", 10).len(), 0);
        assert_eq!(crate::search::rank(&docs, "tk\n", 10).len(), 0);
        assert_eq!(crate::search::rank(&docs, "\ntk", 10).len(), 0);
        assert_eq!(crate::search::rank(&docs, "tk\r\n", 10).len(), 0);
        assert_eq!(crate::search::rank(&docs, "\r\ntk", 10).len(), 0);
        assert_eq!(crate::search::rank(&docs, "退款\n", 10).len(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn app_rules_roundtrip_and_match_basename() {
        let dir = temp_dir("rules");
        let mut store = Store::load_from(&dir);
        store.set_rule("chrome.exe", "paste").expect("add rule");
        assert_eq!(
            store.action_for_exe(r"C:\Program Files\Chrome\Chrome.EXE"),
            Some("paste".to_string())
        );
        assert_eq!(store.set_rule(r"C:\", "paste"), Err("rule_exe".to_string()));
        assert_eq!(store.action_for_exe(""), None);
        store
            .set_rule("chrome.exe", "unicode")
            .expect("replace rule");
        assert_eq!(
            store.action_for_exe("chrome.exe"),
            Some("unicode".to_string())
        );
        assert_eq!(store.rules.rules.len(), 1);
        let mut reloaded = Store::load_from(&dir);
        assert_eq!(
            reloaded.action_for_exe("chrome.exe"),
            Some("unicode".to_string())
        );
        assert_eq!(
            reloaded.set_rule("a.exe", "invalid"),
            Err("rule_action".to_string())
        );
        assert_eq!(
            reloaded.remove_rule("missing.exe"),
            Err("rule_missing".to_string())
        );
        reloaded.read_only = true;
        assert_eq!(
            reloaded.set_rule("a.exe", "paste"),
            Err("read_only".to_string())
        );
        assert_eq!(
            reloaded.remove_rule("chrome.exe"),
            Err("read_only".to_string())
        );
        reloaded.read_only = false;
        reloaded.remove_rule("CHROME.EXE").expect("remove rule");
        assert!(reloaded.action_for_exe("chrome.exe").is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn panel_sort_roundtrips() {
        let dir = temp_dir("panelsort");
        let mut store = Store::load_from(&dir);
        let mut settings = store.settings().clone();
        settings.panel_sort = "most_used".to_string();
        store.save_settings(settings).expect("save settings");
        let reloaded = Store::load_from(&dir);
        assert_eq!(reloaded.settings().panel_sort, "most_used");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn insertion_mode_roundtrips_and_defaults_to_unicode() {
        let dir = temp_dir("mode");
        let mut store = Store::load_from(&dir);
        assert_eq!(store.settings().insertion_mode, "unicode");
        let mut settings = store.settings().clone();
        settings.insertion_mode = "paste".to_string();
        settings.tab_policy = "tab".to_string();
        store.save_settings(settings).expect("save settings");
        let reloaded = Store::load_from(&dir);
        assert_eq!(reloaded.settings().insertion_mode, "paste");
        assert_eq!(reloaded.settings().tab_policy, "tab");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn content_keeps_trailing_whitespace() {
        let dir = temp_dir("trailing");
        let mut store = Store::load_from(&dir);
        let id = store
            .add_snippet("标题", "正文  \n缩进\t ", None, false)
            .expect("add");
        assert_eq!(store.get(&id).unwrap().content, "正文  \n缩进\t ");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn restore_rejects_duplicate_active_title() {
        let dir = temp_dir("restoredup");
        let mut store = Store::load_from(&dir);
        let first = store.add_snippet("Foo", "a", None, false).expect("add");
        store.soft_delete(&first).expect("delete");
        let _ = store.add_snippet("foo", "b", None, false).expect("add");
        assert_eq!(store.restore(&first), Err("duplicate_title".to_string()));
        assert!(store.get(&first).unwrap().deleted_at.is_some());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rename_category_to_same_name_is_allowed() {
        let dir = temp_dir("renameself");
        let mut store = Store::load_from(&dir);
        let id = store.add_category("分类 A").expect("add");
        assert_eq!(store.rename_category(&id, "分类 a"), Ok(()));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn delete_category_failure_rolls_back_snippets() {
        let dir = temp_dir("catrollback");
        let mut store = Store::load_from(&dir);
        let category = store.add_category("待删分类").expect("add category");
        let id = store
            .add_snippet("标题", "正文", Some(&category), false)
            .expect("add snippet");
        let before = store.get(&id).unwrap().clone();

        fs::create_dir_all(dir.join(format!("{DATA_FILE}.tmp"))).unwrap();
        assert!(store.delete_category(&category).is_err());

        let after = store.get(&id).unwrap();
        assert_eq!(after.category_id, before.category_id);
        assert_eq!(after.updated_at, before.updated_at);
        assert!(store
            .data()
            .categories
            .iter()
            .any(|item| item.id == category));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn snippet_crud_roundtrip() {
        let dir = temp_dir("crud");
        let mut store = Store::load_from(&dir);
        let id = store
            .add_snippet("  新短语  ", "正文", None, true)
            .expect("add");
        let added = store.get(&id).unwrap();
        assert_eq!(added.title, "新短语");
        assert!(added.pinned);

        store
            .update_snippet(&id, "改名", "新正文", None, false)
            .expect("update");
        let updated = store.get(&id).unwrap();
        assert_eq!(updated.title, "改名");
        assert_eq!(updated.content, "新正文");
        assert!(!updated.pinned);

        store.soft_delete(&id).expect("delete");
        assert!(store.get(&id).unwrap().deleted_at.is_some());
        assert!(!store.snippets().iter().any(|item| item.id == id));
        assert!(store.deleted_snippets().iter().any(|item| item.id == id));

        store.restore(&id).expect("restore");
        assert!(store.snippets().iter().any(|item| item.id == id));

        store.purge(&id).expect("purge");
        assert!(store.get(&id).is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn snippet_validation_rejects_bad_input() {
        let dir = temp_dir("validate");
        let mut store = Store::load_from(&dir);
        assert_eq!(
            store.add_snippet("   ", "正文", None, false),
            Err("title_empty".to_string())
        );
        assert_eq!(
            store.add_snippet("标题", "   ", None, false),
            Err("content_empty".to_string())
        );
        assert_eq!(
            store.add_snippet(&"长".repeat(TITLE_MAX_UNITS + 1), "正文", None, false),
            Err("title_too_long".to_string())
        );

        let _ = store.add_snippet("标题", "正文", None, false);
        assert_eq!(
            store.add_snippet("标题", "另一段正文", None, false),
            Err("duplicate_title".to_string())
        );
        assert_eq!(
            store.add_snippet("新标题", "正文", Some("missing-category"), false),
            Err("category_missing".to_string())
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn category_crud_moves_snippets_to_uncategorized() {
        let dir = temp_dir("categories");
        let mut store = Store::load_from(&dir);
        let category = store.add_category("新分类").expect("add category");
        let id = store
            .add_snippet("分类短语", "正文", Some(&category), false)
            .expect("add snippet");
        assert_eq!(
            store.get(&id).unwrap().category_id.as_deref(),
            Some(category.as_str())
        );

        store
            .rename_category(&category, "改名分类")
            .expect("rename");
        assert_eq!(store.category_name(Some(&category)), "改名分类");

        store.delete_category(&category).expect("delete category");
        assert_eq!(store.get(&id).unwrap().category_id, None);
        assert!(store
            .data()
            .categories
            .iter()
            .all(|item| item.id != category));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn crud_rejected_when_read_only() {
        let dir = temp_dir("crudreadonly");
        let mut store = Store::load_from(&dir);
        store.read_only = true;
        assert_eq!(
            store.add_snippet("标题", "正文", None, false),
            Err("read_only".to_string())
        );
        let id = store.data().snippets[0].id.clone();
        assert_eq!(store.soft_delete(&id), Err("read_only".to_string()));
        assert_eq!(store.add_category("分类"), Err("read_only".to_string()));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn first_load_seeds_and_persists() {
        let dir = temp_dir("seed");
        let store = Store::load_from(&dir);
        assert!(!store.is_read_only());
        assert_eq!(store.snippets().len(), 3);
        assert!(dir.join(DATA_FILE).exists());
        assert!(dir.join(SETTINGS_FILE).exists());
        assert!(dir.join(RULES_FILE).exists());

        let reloaded = Store::load_from(&dir);
        assert_eq!(reloaded.snippets().len(), 3);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_increments_revision_and_persists() {
        let dir = temp_dir("save");
        let mut store = Store::load_from(&dir);
        let before = store.data().revision;
        store.data.snippets[0].title = "改名".to_string();
        store.save_data().unwrap();
        assert_eq!(store.data().revision, before + 1);

        let reloaded = Store::load_from(&dir);
        assert!(reloaded.snippets().iter().any(|item| item.title == "改名"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn record_use_updates_count_and_status() {
        let dir = temp_dir("use");
        let mut store = Store::load_from(&dir);
        let id = store.snippets()[0].id.clone();
        store.record_use(&id, "accepted").unwrap();
        let reloaded = Store::load_from(&dir);
        let snippet = reloaded
            .data()
            .snippets
            .iter()
            .find(|item| item.id == id)
            .unwrap();
        assert_eq!(snippet.use_count, 1);
        assert_eq!(snippet.last_insert_status.as_deref(), Some("accepted"));
        assert!(snippet.last_used_at.is_some());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn record_use_does_not_rebuild_search_blob() {
        let dir = temp_dir("blob-usage");
        let mut store = Store::load_from(&dir);
        let id = store.snippets()[0].id.clone();
        let position = store
            .data
            .snippets
            .iter()
            .position(|snippet| snippet.id == id)
            .unwrap();
        store.data.snippets[position].search_blob = "sentinel".to_string();
        store.record_use(&id, "accepted").unwrap();
        assert_eq!(store.data.snippets[position].search_blob, "sentinel");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn usage_worker_preserves_fifo_and_flushes() {
        let (tx, rx) = mpsc::channel();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let seen_worker = seen.clone();
        let worker = std::thread::spawn(move || {
            run_usage_worker(rx, move |id, status| {
                seen_worker.lock().unwrap().push(format!("{id}:{status}"));
            });
        });
        tx.send(UsageEvent::Record {
            id: "a".to_string(),
            status: "accepted".to_string(),
        })
        .unwrap();
        tx.send(UsageEvent::Record {
            id: "b".to_string(),
            status: "rejected".to_string(),
        })
        .unwrap();
        let (done_tx, done_rx) = mpsc::channel();
        tx.send(UsageEvent::Flush(done_tx)).unwrap();
        assert!(done_rx.recv_timeout(Duration::from_secs(1)).is_ok());
        assert_eq!(*seen.lock().unwrap(), vec!["a:accepted", "b:rejected"]);
        drop(tx);
        worker.join().unwrap();
    }

    #[test]
    fn content_mutations_refresh_search_blob() {
        let dir = temp_dir("blob-mutations");
        let mut store = Store::load_from(&dir);
        let check = |store: &Store, id: &str| {
            let snippet = store
                .data
                .snippets
                .iter()
                .find(|snippet| snippet.id == id)
                .unwrap();
            assert_eq!(
                snippet.search_blob,
                build_search_blob(&snippet.title, &snippet.content, &snippet.tags)
            );
        };
        let id = store.add_snippet("标题A", "正文A", None, false).unwrap();
        check(&store, &id);

        store
            .update_snippet(&id, "标题B", "正文B", None, true)
            .unwrap();
        check(&store, &id);

        store
            .add_tags(&[id.clone()], &["标签".to_string()])
            .unwrap();
        check(&store, &id);

        store
            .remove_tags(&[id.clone()], &["标签".to_string()])
            .unwrap();
        check(&store, &id);

        store.record_use(&id, "accepted").unwrap();
        check(&store, &id);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn backups_are_pruned_to_five() {
        let dir = temp_dir("prune");
        let mut store = Store::load_from(&dir);
        for index in 0..8 {
            store.data.snippets[0].use_count = index;
            store.save_data().unwrap();
        }
        let count = backups(&dir, DATA_FILE).len();
        assert_eq!(count, BACKUP_KEEP, "backups={count}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_main_recovers_from_backup() {
        let dir = temp_dir("missing");
        let mut store = Store::load_from(&dir);
        store.data.snippets[0].title = "第一次".to_string();
        store.save_data().unwrap();
        store.data.snippets[0].title = "第二次".to_string();
        store.save_data().unwrap();
        fs::remove_file(dir.join(DATA_FILE)).unwrap();

        // The newest backup holds the state before the last save.
        let recovered = Store::load_from(&dir);
        assert!(!recovered.is_read_only());
        assert!(recovered
            .data()
            .snippets
            .iter()
            .any(|item| item.title == "第一次"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn recovery_does_not_backup_the_corrupt_main() {
        let dir = temp_dir("nopollute");
        let mut store = Store::load_from(&dir);
        for _ in 0..6 {
            store.save_data().unwrap();
        }
        fs::write(dir.join(DATA_FILE), b"broken").unwrap();

        let recovered = Store::load_from(&dir);
        assert!(!recovered.is_read_only());
        for backup in backups(&dir, DATA_FILE) {
            let bytes = fs::read(&backup).unwrap();
            assert!(
                serde_json::from_slice::<DataFile>(&bytes).is_ok(),
                "corrupt backup {}",
                backup.display()
            );
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejected_use_does_not_increment_count() {
        let dir = temp_dir("rejected");
        let mut store = Store::load_from(&dir);
        let id = store.snippets()[0].id.clone();
        let updated_before = store
            .data()
            .snippets
            .iter()
            .find(|item| item.id == id)
            .unwrap()
            .updated_at
            .clone();
        store.record_use(&id, "rejected").unwrap();
        let snippet = store
            .data()
            .snippets
            .iter()
            .find(|item| item.id == id)
            .unwrap();
        assert_eq!(snippet.use_count, 0);
        assert_eq!(snippet.last_insert_status.as_deref(), Some("rejected"));
        assert!(snippet.last_used_at.is_none());
        // A rejected attempt must not reorder the list via updatedAt.
        assert_eq!(snippet.updated_at, updated_before);

        store.record_use(&id, "accepted").unwrap();
        let snippet = store
            .data()
            .snippets
            .iter()
            .find(|item| item.id == id)
            .unwrap();
        assert_eq!(snippet.use_count, 1);
        assert!(snippet.last_used_at.is_some());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_only_rejects_save() {
        let dir = temp_dir("readonlysave");
        let mut store = Store::load_from(&dir);
        store.read_only = true;
        assert_eq!(store.save_data(), Err("read_only".to_string()));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn older_schema_is_read_only() {
        let dir = temp_dir("olderschema");
        let store = Store::load_from(&dir);
        let mut data = store.data().clone();
        data.schema_version = 0;
        fs::write(
            dir.join(DATA_FILE),
            serde_json::to_vec_pretty(&data).unwrap(),
        )
        .unwrap();
        let older = Store::load_from(&dir);
        assert!(older.is_read_only());
        assert_eq!(older.notice(), Some("schema_older"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn recovery_does_not_reuse_a_revision() {
        let dir = temp_dir("revision");
        let mut store = Store::load_from(&dir);
        store.data.snippets[0].title = "第一版".to_string();
        store.save_data().unwrap();
        store.data.snippets[0].title = "第二版".to_string();
        store.save_data().unwrap();
        let lost_revision = store.data().revision;
        assert!(lost_revision >= 3);

        fs::write(dir.join(DATA_FILE), b"broken").unwrap();
        let recovered = Store::load_from(&dir);
        assert!(!recovered.is_read_only());
        assert!(
            recovered.data().revision > lost_revision,
            "recovered revision {} must exceed lost {}",
            recovered.data().revision,
            lost_revision
        );
        assert!(recovered
            .data()
            .snippets
            .iter()
            .any(|item| item.title == "第一版"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn newer_schema_with_incompatible_body_is_not_overwritten() {
        let dir = temp_dir("future");
        let mut store = Store::load_from(&dir);
        store.save_data().unwrap();

        // A v2 file whose body would not parse as v1 must not trigger recovery.
        fs::write(
            dir.join(DATA_FILE),
            br#"{ "schemaVersion": 2, "unknownShape": true }"#,
        )
        .unwrap();
        let future = Store::load_from(&dir);
        assert!(future.is_read_only());
        assert_eq!(future.notice(), Some("schema_newer"));
        let bytes = fs::read(dir.join(DATA_FILE)).unwrap();
        assert!(String::from_utf8_lossy(&bytes).contains("\"schemaVersion\": 2"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn settings_all_corrupt_enters_read_only() {
        let dir = temp_dir("settingsbad");
        let mut store = Store::load_from(&dir);
        store.save_data().unwrap();
        fs::write(dir.join(SETTINGS_FILE), b"broken").unwrap();
        let broken = Store::load_from(&dir);
        assert!(broken.is_read_only());
        assert_eq!(broken.notice(), Some("settings_recovery_failed"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn failed_write_does_not_leak_revision() {
        let dir = temp_dir("writefail");
        let mut store = Store::load_from(&dir);
        let before = store.data().revision;
        // A directory at the temp path makes File::create fail deterministically.
        fs::create_dir_all(dir.join(format!("{DATA_FILE}.tmp"))).unwrap();
        assert!(store.save_data().is_err());
        assert_eq!(store.data().revision, before);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn failed_write_after_hwm_advance_keeps_revision() {
        let dir = temp_dir("hwmahead");
        let mut store = Store::load_from(&dir);
        let before = store.data().revision;
        fs::write(dir.join(HWM_FILE), "100").unwrap();
        fs::create_dir_all(dir.join(format!("{DATA_FILE}.tmp"))).unwrap();
        assert!(store.save_data().is_err());
        // The old `next_revision - 1` rollback would have left 100 here.
        assert_eq!(store.data().revision, before);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_main_with_all_backups_corrupt_is_read_only() {
        let dir = temp_dir("missingbad");
        let mut store = Store::load_from(&dir);
        store.save_data().unwrap();
        fs::remove_file(dir.join(DATA_FILE)).unwrap();
        for backup in backups(&dir, DATA_FILE) {
            fs::write(backup, b"broken").unwrap();
        }
        let broken = Store::load_from(&dir);
        assert!(broken.is_read_only());
        assert_eq!(broken.notice(), Some("data_recovery_failed"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_settings_with_corrupt_backup_is_read_only() {
        let dir = temp_dir("settingsmissing");
        let store = Store::load_from(&dir);
        assert!(!store.is_read_only());
        fs::create_dir_all(dir.join("backups")).unwrap();
        fs::write(
            dir.join("backups")
                .join("settings-20260101T000000000Z-0001.json"),
            b"broken",
        )
        .unwrap();
        fs::remove_file(dir.join(SETTINGS_FILE)).unwrap();

        let broken = Store::load_from(&dir);
        assert!(broken.is_read_only());
        assert_eq!(broken.notice(), Some("settings_recovery_failed"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_only_record_use_is_rejected_without_mutation() {
        let dir = temp_dir("readonlyuse");
        let mut store = Store::load_from(&dir);
        let id = store.snippets()[0].id.clone();
        store.read_only = true;
        assert_eq!(
            store.record_use(&id, "accepted"),
            Err("read_only".to_string())
        );
        let snippet = store
            .data()
            .snippets
            .iter()
            .find(|item| item.id == id)
            .unwrap();
        assert_eq!(snippet.use_count, 0);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_main_recovers_from_newest_backup() {
        let dir = temp_dir("recover");
        let mut store = Store::load_from(&dir);
        store.data.snippets[0].title = "第一版".to_string();
        store.save_data().unwrap();
        store.data.snippets[0].title = "第二版".to_string();
        store.save_data().unwrap();

        fs::write(dir.join(DATA_FILE), b"{ not json").unwrap();
        let recovered = Store::load_from(&dir);
        assert!(!recovered.is_read_only());
        // The newest backup is the state before the last save.
        assert!(recovered
            .data()
            .snippets
            .iter()
            .any(|item| item.title == "第一版"));
        assert!(!recovered
            .data()
            .snippets
            .iter()
            .any(|item| item.title == "第二版"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn all_copies_corrupt_enters_read_only() {
        let dir = temp_dir("readonly");
        let mut store = Store::load_from(&dir);
        store.save_data().unwrap();
        fs::write(dir.join(DATA_FILE), b"broken").unwrap();
        for backup in backups(&dir, DATA_FILE) {
            fs::write(backup, b"broken").unwrap();
        }
        let broken = Store::load_from(&dir);
        assert!(broken.is_read_only());
        assert_eq!(broken.notice(), Some("data_recovery_failed"));
        assert!(broken.snippets().is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn newer_schema_is_read_only() {
        let dir = temp_dir("schema");
        let store = Store::load_from(&dir);
        let mut data = store.data().clone();
        data.schema_version = SCHEMA_VERSION + 1;
        fs::write(
            dir.join(DATA_FILE),
            serde_json::to_vec_pretty(&data).unwrap(),
        )
        .unwrap();
        let newer = Store::load_from(&dir);
        assert!(newer.is_read_only());
        assert_eq!(newer.notice(), Some("schema_newer"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn atomic_write_leaves_no_temp_file() {
        let dir = temp_dir("temp");
        let mut store = Store::load_from(&dir);
        store.save_data().unwrap();
        assert!(!dir.join(format!("{DATA_FILE}.tmp")).exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn export_import_roundtrip_and_invalid_schema() {
        let dir = temp_dir("importrt");
        let mut store = Store::load_from(&dir);
        let category = store.add_category("导入测试分类").expect("category");
        store
            .add_snippet_tagged(
                "导入测试短语",
                "导入正文\n第二行",
                Some(&category),
                true,
                &["support".to_string(), "refund".to_string()],
            )
            .expect("snippet");
        let json = store.export_json().expect("export");
        assert!(json.contains("导入测试短语"));
        assert!(
            !json.contains("searchBlob"),
            "derived search blobs must not be exported"
        );
        let exported: DataFile = serde_json::from_str(&json).expect("parse export");

        let target_dir = temp_dir("importtarget");
        let mut target = Store::load_from(&target_dir);
        let summary = target
            .import_bytes(json.as_bytes(), ImportMode::Replace)
            .expect("replace import");
        assert_eq!(summary.categories_added, exported.categories.len());
        assert_eq!(summary.snippets_added, exported.snippets.len());
        assert!(target
            .snippets()
            .iter()
            .any(|snippet| snippet.title == "导入测试短语"));
        assert!(target
            .snippets()
            .iter()
            .find(|snippet| snippet.title == "导入测试短语")
            .is_some_and(|snippet| {
                snippet.pinned && snippet.tags == vec!["support".to_string(), "refund".to_string()]
            }));
        for snippet in target.data().snippets.iter() {
            assert_eq!(
                snippet.search_blob,
                build_search_blob(&snippet.title, &snippet.content, &snippet.tags)
            );
        }

        let invalid = br#"{"schemaVersion":99,"revision":0,"categories":[],"snippets":[]}"#;
        assert_eq!(
            target.import_bytes(invalid, ImportMode::Replace),
            Err("import_schema".to_string())
        );
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&target_dir);
    }

    #[test]
    fn merge_import_skips_duplicate_titles_and_remaps_ids() {
        let dir = temp_dir("importmerge");
        let mut store = Store::load_from(&dir);
        let mut imported = store.data().clone();
        let mut category = imported.categories[0].clone();
        category.id = "imported-category".to_string();
        category.name = "导入分类".to_string();
        imported.categories.push(category);
        let mut extra = imported.snippets[0].clone();
        extra.id = "imported-snippet".to_string();
        extra.title = "导入短语".to_string();
        extra.content = "导入正文".to_string();
        extra.rebuild_blob();
        imported.snippets.push(extra);
        let before = store.snippets().len();
        let summary = store
            .import_bytes(&serde_json::to_vec(&imported).unwrap(), ImportMode::Merge)
            .expect("merge import");
        assert_eq!(summary.snippets_added, 1);
        assert_eq!(summary.snippets_skipped, before);
        assert_eq!(summary.categories_added, 1);
        assert!(store
            .snippets()
            .iter()
            .any(|snippet| snippet.title == "导入短语"));
        for snippet in store.data().snippets.iter() {
            assert_eq!(
                snippet.search_blob,
                build_search_blob(&snippet.title, &snippet.content, &snippet.tags)
            );
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn merge_matches_category_names_with_unicode_case_folding() {
        let dir = temp_dir("importcase");
        let mut store = Store::load_from(&dir);
        let category = store.add_category("Équipe").expect("category");
        store
            .add_snippet("Équipe 短语", "正文", Some(&category), false)
            .expect("snippet");
        let mut imported = store.data().clone();
        if let Some(category) = imported
            .categories
            .iter_mut()
            .find(|category| category.name == "Équipe")
        {
            category.name = "éQUIPE".to_string();
        }
        let categories_before = store.data().categories.len();
        let summary = store
            .import_bytes(&serde_json::to_vec(&imported).unwrap(), ImportMode::Merge)
            .expect("merge import");
        assert_eq!(summary.categories_added, 0);
        assert_eq!(store.data().categories.len(), categories_before);
        assert_eq!(
            store
                .data()
                .categories
                .iter()
                .filter(|category| category.name.to_lowercase() == "équipe")
                .count(),
            1
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn replace_import_resets_crafted_revision() {
        let dir = temp_dir("importrev");
        let store = Store::load_from(&dir);
        let mut imported = store.data().clone();
        imported.revision = u64::MAX;
        let target_dir = temp_dir("importrevtarget");
        let mut target = Store::load_from(&target_dir);
        target
            .import_bytes(&serde_json::to_vec(&imported).unwrap(), ImportMode::Replace)
            .expect("replace import");
        assert!(target.data().revision > 0);
        assert!(target.data().revision < 1_000);
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&target_dir);
    }

    #[test]
    fn replace_import_keeps_local_revision_floor_without_hwm() {
        let dir = temp_dir("importfloor");
        let mut store = Store::load_from(&dir);
        let _ = fs::remove_file(dir.join(HWM_FILE));
        store.data.revision = 500;
        let imported = store.data().clone();
        store
            .import_bytes(&serde_json::to_vec(&imported).unwrap(), ImportMode::Replace)
            .expect("replace import");
        assert!(store.data().revision > 500);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn purge_keeps_noncanonical_deleted_timestamps() {
        let dir = temp_dir("purgefmt");
        let mut store = Store::load_from(&dir);
        let offset = store
            .add_snippet("偏移时间", "正文", None, false)
            .expect("offset");
        let empty = store
            .add_snippet("空时间", "正文", None, false)
            .expect("empty");
        let malformed = store
            .add_snippet("畸形时间", "正文", None, false)
            .expect("malformed");
        let plus = store
            .add_snippet("加号时间", "正文", None, false)
            .expect("plus");
        let negative = store
            .add_snippet("负年份", "正文", None, false)
            .expect("negative");
        for id in [&offset, &empty, &malformed, &plus, &negative] {
            store.soft_delete(id).expect("delete");
        }
        let values = [
            (&offset, "2026-10-20T23:30:00-08:00"),
            (&empty, ""),
            (&malformed, "2026-13-40T00:00:00Z"),
            (&plus, "2000-+1-01T00:00:00Z"),
            (&negative, "-001-01-01T00:00:00Z"),
        ];
        for (id, value) in values {
            if let Some(snippet) = store.data.snippets.iter_mut().find(|s| s.id == *id) {
                snippet.deleted_at = Some(value.to_string());
            }
        }
        assert_eq!(store.purge_expired(30), Ok(0));
        assert!(store.get(&offset).is_some());
        assert!(store.get(&empty).is_some());
        assert!(store.get(&malformed).is_some());
        assert!(store.get(&plus).is_some());
        assert!(store.get(&negative).is_some());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn recycle_purge_removes_only_old_deleted_rows() {
        let dir = temp_dir("purge");
        let mut store = Store::load_from(&dir);
        let old = store
            .add_snippet("旧回收项", "旧正文", None, false)
            .expect("old");
        let fresh = store
            .add_snippet("新回收项", "新正文", None, false)
            .expect("fresh");
        store.soft_delete(&old).expect("delete old");
        store.soft_delete(&fresh).expect("delete fresh");
        if let Some(snippet) = store.data.snippets.iter_mut().find(|s| s.id == old) {
            snippet.deleted_at = Some("2000-01-01T00:00:00Z".to_string());
        }
        assert_eq!(store.purge_expired(30), Ok(1));
        assert!(store.get(&old).is_none());
        assert!(store.get(&fresh).is_some());
        assert_eq!(store.empty_recycle(), Ok(1));
        assert!(store.get(&fresh).is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn simulated_fault_injection_never_loses_all_copies() {
        // 100 rounds: write a new value, then corrupt the main file. A valid
        // store or a recovery from backup must still be possible.
        let dir = temp_dir("faults");
        let mut store = Store::load_from(&dir);
        for round in 0..100u32 {
            store.data.snippets[0].use_count = u64::from(round);
            store.save_data().unwrap();
            if round % 3 == 0 {
                let bytes = fs::read(dir.join(DATA_FILE)).unwrap();
                let cut = bytes.len() / 2;
                fs::write(dir.join(DATA_FILE), &bytes[..cut]).unwrap();
            }
            let loaded = Store::load_from(&dir);
            assert!(
                !loaded.is_read_only() || round < 2,
                "round {round} lost every copy"
            );
            if let Some(snippet) = loaded.snippets().first() {
                assert!(snippet.use_count <= u64::from(round));
            }
        }
        let _ = fs::remove_dir_all(&dir);
    }
}
