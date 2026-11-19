//! Persistent store: `data.json`, `settings.json`, `rules-user.json`.
//!
//! Write path (final-plan §6.1): temp file -> flush -> re-read/verify ->
//! rotate a backup -> `MoveFileExW` (first write) or `ReplaceFileW`
//! (existing file), with a `MoveFileExW` fallback. Any failure keeps the
//! original file and removes the temp file.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use windows::core::PCWSTR;
use windows::Win32::Storage::FileSystem::{
    MoveFileExW, ReplaceFileW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    REPLACEFILE_IGNORE_MERGE_ERRORS, REPLACEFILE_WRITE_THROUGH,
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
    pub search_blob: String,
}

impl Snippet {
    /// Lowercased title + newline + lowercased body, per §6.2.
    pub fn rebuild_blob(&mut self) {
        self.search_blob = format!(
            "{}\n{}",
            self.title.to_lowercase(),
            self.content.to_lowercase()
        );
    }
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
    pub insertion_mode: String,
    pub tab_policy: String,
    pub theme: String,
    pub startup: bool,
    pub paused: bool,
    pub data_dir: Option<String>,
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

pub struct Store {
    dir: PathBuf,
    data: DataFile,
    settings: Settings,
    rules: RulesFile,
    read_only: bool,
    notice: Option<String>,
}

static STORE: std::sync::OnceLock<std::sync::Mutex<Store>> = std::sync::OnceLock::new();

/// Load the store once at startup and log its state.
pub fn init() {
    let store = Store::load();
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

/// Snapshot for the panel: id, title, content, category name.
#[derive(Debug, Clone)]
pub struct PanelRow {
    pub id: String,
    pub title: String,
    pub content: String,
    pub category: String,
    pub category_id: Option<String>,
    pub blob: String,
    pub pinned: bool,
    pub sort_order: i32,
    pub last_used_at: Option<String>,
    pub updated_at: String,
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
                blob: snippet.search_blob.clone(),
                pinned: snippet.pinned,
                sort_order: snippet.sort_order,
                last_used_at: snippet.last_used_at.clone(),
                updated_at: snippet.updated_at.clone(),
            })
            .collect()
    })
    .unwrap_or_default()
}

pub fn categories() -> Vec<(String, String)> {
    with_store(|store| {
        let mut categories: Vec<(String, String)> = store
            .data()
            .categories
            .iter()
            .map(|category| (category.id.clone(), category.name.clone()))
            .collect();
        categories.sort_by(|left, right| left.1.cmp(&right.1));
        categories
    })
    .unwrap_or_default()
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

pub fn record_use(id: &str, status: &str) {
    match with_store(|store| store.record_use(id, status)) {
        Some(Ok(())) => {}
        Some(Err(err)) => logging::warn(&format!("storage: record_use failed: {err}")),
        None => logging::warn("storage: record_use before init"),
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
            };
        }

        let mut notice = None;
        let mut read_only = false;

        clean_stale_temp(dir);

        // Peek the version before any recovery: a file written by a newer
        // program must never be replaced by an older backup.
        let future_data = peek_version(&dir.join(DATA_FILE), "schemaVersion")
            .map(|version| version > u64::from(SCHEMA_VERSION))
            .unwrap_or(false);
        let future_settings = peek_version(&dir.join(SETTINGS_FILE), "schemaVersion")
            .map(|version| version > u64::from(SCHEMA_VERSION))
            .unwrap_or(false);
        let future_rules = peek_version(&dir.join(RULES_FILE), "version")
            .map(|version| version > 1)
            .unwrap_or(false);

        let (mut data, recovered) = if future_data {
            logging::warn("storage: data.json schema is newer; skipping recovery");
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
                    read_only = true;
                    (DataFile::default(), None)
                }
            }
        };

        if future_data {
            notice = Some("schema_newer".to_string());
            read_only = true;
        }
        if data.schema_version > SCHEMA_VERSION {
            logging::warn(&format!(
                "storage: schema {} is newer than {SCHEMA_VERSION}; read-only",
                data.schema_version
            ));
            notice = Some("schema_newer".to_string());
            read_only = true;
        } else if data.schema_version < SCHEMA_VERSION {
            logging::warn(&format!(
                "storage: schema {} predates {SCHEMA_VERSION}; read-only until migrated",
                data.schema_version
            ));
            notice = Some("schema_older".to_string());
            read_only = true;
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

        let mut store = Self {
            dir: dir.to_path_buf(),
            data,
            settings,
            rules,
            read_only,
            notice,
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

    #[allow(dead_code)] // consumed by M1-5 settings UI
    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    #[allow(dead_code)] // consumed by M1-7 rule evaluation
    pub fn rules(&self) -> &RulesFile {
        &self.rules
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
        for snippet in &mut self.data.snippets {
            snippet.rebuild_blob();
        }
        let previous_revision = self.data.revision;
        let next_revision = self.data.revision.max(read_hwm(&self.dir)) + 1;
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
            }
            snippet.last_insert_status = Some(status.to_string());
            snippet.updated_at = now;
        }
        match self.save_data() {
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
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
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

    // Keep the newest BACKUP_KEEP files. A failed delete must not loop: the
    // list would not change and the write path holds the store lock.
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
    Ok(())
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
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path
            .file_name()
            .and_then(|value| value.to_str())
            .map(|value| value.ends_with(".tmp"))
            .unwrap_or(false)
        {
            let _ = fs::remove_file(path);
        }
    }
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
    let source = platform::wide(&source.to_string_lossy());
    let target = platform::wide(&target.to_string_lossy());
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
