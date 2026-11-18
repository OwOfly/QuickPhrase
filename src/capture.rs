use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::context::{self, Integrity, WindowContext};
use crate::foreground::{self, ForegroundEvent};
use crate::platform;
use crate::uia::{self, UiaSnapshot};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Capability {
    Text,
    ValueOnly,
    ReadOnly,
    Disabled,
    Password,
    Elevated,
    IntegrityUnknown,
    NoCaret,
    Document,
    Unknown,
    UiaUnavailable,
}

impl Capability {
    pub fn as_str(self) -> &'static str {
        match self {
            Capability::Text => "text",
            Capability::ValueOnly => "value_only",
            Capability::ReadOnly => "readonly",
            Capability::Disabled => "disabled",
            Capability::Password => "password",
            Capability::Elevated => "elevated",
            Capability::IntegrityUnknown => "integrity_unknown",
            Capability::NoCaret => "no_caret",
            Capability::Document => "document",
            Capability::Unknown => "unknown",
            Capability::UiaUnavailable => "uia_unavailable",
        }
    }

    #[allow(dead_code)] // consumed by the M0-4 insertion gate
    pub fn allows_insert(self) -> bool {
        matches!(self, Capability::Text)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Gate {
    pub target_higher: bool,
    pub integrity_unknown: bool,
}

#[derive(Serialize)]
pub struct Report {
    pub captured_at: String,
    pub identity: String,
    pub identity_source: String,
    pub capability: String,
    pub region: String,
    pub timings_ms: Timings,
    pub target: Target,
    pub uia: Option<UiaSnapshot>,
    pub foreground_events: Vec<ForegroundEvent>,
}

#[derive(Serialize)]
pub struct Timings {
    pub native: u64,
    pub uia: u64,
    pub total: u64,
}

#[derive(Serialize)]
pub struct Target {
    pub hwnd: isize,
    pub pid: u32,
    pub tid: u32,
    pub created: u64,
    pub exe: String,
    pub class: String,
    pub title: String,
    pub focused_hwnd: isize,
    pub focused_class: String,
    pub focused_is_child: bool,
    pub focused_is_editable: bool,
    pub focused_is_password: bool,
    pub focused_is_readonly: bool,
    pub caret: Option<[i32; 4]>,
    pub integrity: String,
    pub own_integrity: String,
    pub target_higher: bool,
}

pub fn once() -> Option<Report> {
    let started = Instant::now();
    let window = context::snapshot()?;

    if window.pid == std::process::id() {
        crate::logging::warn("capture aborted: foreground window belongs to QuickPhrase");
        return None;
    }

    let native_ms = started.elapsed().as_millis() as u64;

    let uia_started = Instant::now();
    let uia_snapshot = uia::capture(window.hwnd, Duration::from_millis(60));
    let uia_ms = uia_started.elapsed().as_millis() as u64;

    let (identity, identity_source) = decide_identity(uia_snapshot.as_ref());
    let gate = Gate {
        target_higher: window.target_higher,
        integrity_unknown: window.integrity == Integrity::Unknown
            || window.own_integrity == Integrity::Unknown,
    };
    let capability = decide_capability(gate, uia_snapshot.as_ref());
    let region = classify(&window, uia_snapshot.as_ref());

    let focused_is_editable = window.focused_is_editable();
    let focused_is_password = window.focused_is_password();
    let focused_is_readonly = window.focused_is_readonly();
    let caret = window
        .caret
        .map(|rect| [rect.left, rect.top, rect.right, rect.bottom]);

    Some(Report {
        captured_at: captured_at(),
        identity: identity.to_string(),
        identity_source: identity_source.to_string(),
        capability: capability.as_str().to_string(),
        region: region.to_string(),
        timings_ms: Timings {
            native: native_ms,
            uia: uia_ms,
            total: started.elapsed().as_millis() as u64,
        },
        target: Target {
            hwnd: window.hwnd.0 as isize,
            pid: window.pid,
            tid: window.tid,
            created: window.created,
            exe: window.exe,
            class: window.class,
            title: window.title,
            focused_hwnd: window.focused.0 as isize,
            focused_class: window.focused_class,
            focused_is_child: window.focused_is_child,
            focused_is_editable,
            focused_is_password,
            focused_is_readonly,
            caret,
            integrity: window.integrity.label().to_string(),
            own_integrity: window.own_integrity.label().to_string(),
            target_higher: window.target_higher,
        },
        uia: uia_snapshot,
        foreground_events: foreground::events(),
    })
}

impl Report {
    pub fn summary(&self) -> String {
        let uia = self
            .uia
            .as_ref()
            .map(|snapshot| {
                format!(
                    "class={} type={} ro={:?} text={} sel={} caret={:?} stable={} ancestor={}",
                    snapshot.class_name,
                    snapshot.control_type,
                    snapshot.is_read_only,
                    snapshot.has_text_pattern,
                    snapshot.selection_count,
                    snapshot.caret_active,
                    snapshot.foreground_stable,
                    snapshot.ancestor_verified
                )
            })
            .unwrap_or_else(|| "none".to_string());

        format!(
            "identity={} source={} capability={} region={} hwnd_class={} focus_class={} uia=[{}] native={}ms uia={}ms total={}ms",
            self.identity,
            self.identity_source,
            self.capability,
            self.region,
            self.target.class,
            self.target.focused_class,
            uia,
            self.timings_ms.native,
            self.timings_ms.uia,
            self.timings_ms.total
        )
    }

    pub fn balloon(&self) -> String {
        if self.identity != "verified" {
            return format!("未能确认原控件，只复制 / {} ms", self.timings_ms.total);
        }

        let message = match self.capability.as_str() {
            "text" => match self.region.as_str() {
                "terminal" => "可单行输入；终端多行只复制".to_string(),
                "text_editor" => "可输入（编辑器）".to_string(),
                _ => "可单行输入；多行只复制".to_string(),
            },
            "password" => "已拒绝：密码框".to_string(),
            "elevated" => "已拒绝：目标权限更高".to_string(),
            "integrity_unknown" => "已拒绝：完整性级别未知".to_string(),
            "readonly" => "只复制：只读控件".to_string(),
            "disabled" => "只复制：控件已禁用".to_string(),
            "no_caret" => "只复制：没有插入点".to_string(),
            "document" => "只复制：整页文档".to_string(),
            "value_only" => "只复制：仅值模式".to_string(),
            "uia_unavailable" => "只复制：识别超时".to_string(),
            _ => "只复制：无法确认可输入".to_string(),
        };

        format!("{message} / {} ms", self.timings_ms.total)
    }
}

pub fn write(report: &Report) -> std::io::Result<PathBuf> {
    let directory = platform::data_dir().join("captures");
    fs::create_dir_all(&directory)?;
    let path = directory.join(format!("capture-{}.json", stamp()));
    let json = serde_json::to_vec_pretty(report).map_err(std::io::Error::other)?;
    fs::write(&path, json)?;
    Ok(path)
}

pub fn decide_identity(facts: Option<&UiaSnapshot>) -> (&'static str, &'static str) {
    match facts {
        None => ("unknown", "none"),
        Some(snapshot) if !snapshot.foreground_stable || !snapshot.ancestor_verified => {
            ("unknown", "uia")
        }
        Some(_) => ("verified", "uia"),
    }
}

pub fn decide_capability(gate: Gate, facts: Option<&UiaSnapshot>) -> Capability {
    if gate.target_higher {
        return Capability::Elevated;
    }
    if gate.integrity_unknown {
        return Capability::IntegrityUnknown;
    }

    let Some(snapshot) = facts else {
        return Capability::UiaUnavailable;
    };

    if !snapshot.foreground_stable || !snapshot.ancestor_verified {
        return Capability::Unknown;
    }
    if snapshot.is_password {
        return Capability::Password;
    }
    if !snapshot.is_enabled {
        return Capability::Disabled;
    }
    if snapshot.control_type == 50030 {
        return Capability::Document;
    }
    match snapshot.is_read_only {
        Some(true) => return Capability::ReadOnly,
        None => return Capability::Unknown,
        Some(false) => {}
    }

    let has_caret = snapshot.caret_active == Some(true) || snapshot.selection_count >= 1;
    if !has_caret {
        return Capability::NoCaret;
    }

    if snapshot.has_text_pattern || snapshot.has_text_pattern2 {
        return Capability::Text;
    }
    if snapshot.has_value_pattern {
        return Capability::ValueOnly;
    }
    Capability::Unknown
}

fn classify(window: &WindowContext, uia_hint: Option<&UiaSnapshot>) -> &'static str {
    region_from(
        &window.exe,
        &window.class,
        &window.focused_class,
        uia_hint.and_then(|snapshot| Some(snapshot.class_name.as_str())),
        uia_hint.and_then(|snapshot| snapshot.region_hint.as_deref()),
    )
}

pub fn region_from(
    exe: &str,
    class: &str,
    focused_class: &str,
    uia_class: Option<&str>,
    uia_hint: Option<&str>,
) -> &'static str {
    if is_terminal(exe, class, uia_hint) {
        return "terminal";
    }
    if is_text_editor(class, focused_class, uia_class) {
        return "text_editor";
    }
    "unknown"
}

fn is_terminal(exe: &str, class: &str, uia_hint: Option<&str>) -> bool {
    if class.eq_ignore_ascii_case("ConsoleWindowClass") {
        return true;
    }
    if uia_hint == Some("terminal") {
        return true;
    }

    let name = std::path::Path::new(exe)
        .file_name()
        .map(|name| name.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();

    matches!(
        name.as_str(),
        "windowsterminal.exe"
            | "openconsole.exe"
            | "conhost.exe"
            | "cmd.exe"
            | "powershell.exe"
            | "pwsh.exe"
            | "mintty.exe"
            | "putty.exe"
    )
}

fn is_text_editor(class: &str, focused_class: &str, uia_class: Option<&str>) -> bool {
    if context::is_edit_class(class) || context::is_edit_class(focused_class) {
        return true;
    }

    uia_class
        .map(|value| {
            let lower = value.to_ascii_lowercase();
            lower.contains("richedit") || lower.contains("_wwg")
        })
        .unwrap_or(false)
}

fn captured_at() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    format!("{millis}")
}

fn stamp() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> UiaSnapshot {
        UiaSnapshot {
            control_type: 50004,
            class_name: "Edit".to_string(),
            is_password: false,
            is_enabled: true,
            is_read_only: Some(false),
            has_text_pattern: true,
            has_text_pattern2: false,
            has_value_pattern: false,
            selection_count: 1,
            caret_active: Some(true),
            native_hwnd_zero: false,
            ancestor_verified: true,
            ancestor_depth: 0,
            foreground_stable: true,
            region_hint: None,
            elapsed_ms: 1,
        }
    }

    fn gate() -> Gate {
        Gate {
            target_higher: false,
            integrity_unknown: false,
        }
    }

    #[test]
    fn editable_edit_is_text() {
        assert_eq!(decide_capability(gate(), Some(&facts())), Capability::Text);
    }

    #[test]
    fn unknown_read_only_denies() {
        let mut snapshot = facts();
        snapshot.is_read_only = None;
        assert_eq!(
            decide_capability(gate(), Some(&snapshot)),
            Capability::Unknown
        );
    }

    #[test]
    fn read_only_denies() {
        let mut snapshot = facts();
        snapshot.is_read_only = Some(true);
        assert_eq!(
            decide_capability(gate(), Some(&snapshot)),
            Capability::ReadOnly
        );
    }

    #[test]
    fn document_denies() {
        let mut snapshot = facts();
        snapshot.control_type = 50030;
        assert_eq!(
            decide_capability(gate(), Some(&snapshot)),
            Capability::Document
        );
    }

    #[test]
    fn missing_caret_denies() {
        let mut snapshot = facts();
        snapshot.caret_active = None;
        snapshot.selection_count = 0;
        assert_eq!(
            decide_capability(gate(), Some(&snapshot)),
            Capability::NoCaret
        );
    }

    #[test]
    fn selection_without_active_caret_still_allows() {
        let mut snapshot = facts();
        snapshot.caret_active = None;
        snapshot.selection_count = 1;
        assert_eq!(decide_capability(gate(), Some(&snapshot)), Capability::Text);
    }

    #[test]
    fn disabled_denies() {
        let mut snapshot = facts();
        snapshot.is_enabled = false;
        assert_eq!(
            decide_capability(gate(), Some(&snapshot)),
            Capability::Disabled
        );
    }

    #[test]
    fn uia_missing_never_falls_back_to_native() {
        assert_eq!(decide_capability(gate(), None), Capability::UiaUnavailable);
        assert_eq!(decide_identity(None), ("unknown", "none"));
    }

    #[test]
    fn uia_negative_evidence_overrides_native_lookalike() {
        let mut snapshot = facts();
        snapshot.ancestor_verified = false;
        assert_eq!(
            decide_capability(gate(), Some(&snapshot)),
            Capability::Unknown
        );
        assert_eq!(decide_identity(Some(&snapshot)), ("unknown", "uia"));
    }

    #[test]
    fn unstable_foreground_denies() {
        let mut snapshot = facts();
        snapshot.foreground_stable = false;
        assert_eq!(
            decide_capability(gate(), Some(&snapshot)),
            Capability::Unknown
        );
        assert_eq!(decide_identity(Some(&snapshot)), ("unknown", "uia"));
    }

    #[test]
    fn elevated_and_unknown_integrity_deny() {
        let higher = Gate {
            target_higher: true,
            integrity_unknown: false,
        };
        assert_eq!(
            decide_capability(higher, Some(&facts())),
            Capability::Elevated
        );

        let unknown = Gate {
            target_higher: false,
            integrity_unknown: true,
        };
        assert_eq!(
            decide_capability(unknown, Some(&facts())),
            Capability::IntegrityUnknown
        );
    }

    #[test]
    fn region_prefers_terminal() {
        assert_eq!(
            region_from(
                "cmd.exe",
                "ConsoleWindowClass",
                "ConsoleWindowClass",
                None,
                None
            ),
            "terminal"
        );
        assert_eq!(
            region_from(
                "Code.exe",
                "Chrome_WidgetWin_1",
                "Chrome_WidgetWin_1",
                Some("xterm-helper-textarea"),
                Some("terminal")
            ),
            "terminal"
        );
        assert_eq!(
            region_from("notepad.exe", "Notepad", "Edit", None, None),
            "text_editor"
        );
        assert_eq!(
            region_from(
                "chrome.exe",
                "Chrome_WidgetWin_1",
                "Chrome_WidgetWin_1",
                Some("cm-content"),
                None
            ),
            "unknown"
        );
    }
}
