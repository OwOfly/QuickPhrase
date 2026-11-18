use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::context::{self, WindowContext};
use crate::foreground::{self, ForegroundEvent};
use crate::platform;
use crate::uia::{self, UiaSnapshot};

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
    let native_ms = started.elapsed().as_millis() as u64;

    let uia_started = Instant::now();
    let uia_snapshot = uia::capture(window.hwnd, Duration::from_millis(60));
    let uia_ms = uia_started.elapsed().as_millis() as u64;

    let identity_verified = uia_snapshot
        .as_ref()
        .map(|snapshot| snapshot.ancestor_verified)
        .unwrap_or(false);

    let (identity, identity_source) = if identity_verified {
        ("verified", "uia")
    } else if native_identity(&window) {
        ("verified", "hwnd")
    } else {
        ("unknown", "none")
    };

    let region = classify(&window, uia_snapshot.as_ref());
    let capability = capability(&window, uia_snapshot.as_ref());

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
        capability: capability.to_string(),
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
                    "class={} type={} ro={:?} text={} sel={}",
                    snapshot.class_name,
                    snapshot.control_type,
                    snapshot.is_read_only,
                    snapshot.has_text_pattern,
                    snapshot.selection_count
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

        let capability = match self.capability.as_str() {
            "text" | "native_edit" => "可输入",
            "value_only" => "仅值模式，只复制",
            "readonly" => "只读控件，只复制",
            "password" => "密码框，拒绝",
            _ => "焦点不可输入",
        };

        format!(
            "{} / {} / {} ms",
            capability,
            match self.region.as_str() {
                "terminal" => "终端（只复制）",
                "text_editor" => "编辑器",
                _ => "未验证区域",
            },
            self.timings_ms.total
        )
    }
}

pub fn write(report: &Report) -> std::io::Result<PathBuf> {
    let directory = platform::data_dir().join("captures");
    fs::create_dir_all(&directory)?;
    let path = directory.join(format!("capture-{}.json", stamp()));
    let json = serde_json::to_vec_pretty(report)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, err))?;
    fs::write(&path, json)?;
    Ok(path)
}

fn native_identity(window: &WindowContext) -> bool {
    window.focused_is_child && window.focused_is_editable() && !window.focused_is_password()
}

fn capability(window: &WindowContext, uia: Option<&UiaSnapshot>) -> &'static str {
    if window.focused_is_password() {
        return "password";
    }
    if window.focused_is_readonly() {
        return "readonly";
    }

    let Some(snapshot) = uia else {
        return if window.focused_is_editable() {
            "native_edit"
        } else {
            "none"
        };
    };

    if snapshot.is_password {
        return "password";
    }
    if snapshot.is_read_only == Some(true) {
        return "readonly";
    }
    if snapshot.has_text_pattern || snapshot.has_text_pattern2 {
        return "text";
    }
    if snapshot.has_value_pattern {
        return "value_only";
    }
    if window.focused_is_editable() {
        return "native_edit";
    }
    "none"
}

fn classify(window: &WindowContext, uia: Option<&UiaSnapshot>) -> &'static str {
    if is_terminal(window, uia) {
        return "terminal";
    }
    if is_text_editor(window, uia) {
        return "text_editor";
    }
    "unknown"
}

fn is_terminal(window: &WindowContext, uia: Option<&UiaSnapshot>) -> bool {
    if window.class.eq_ignore_ascii_case("ConsoleWindowClass") {
        return true;
    }

    if let Some(snapshot) = uia {
        if snapshot.region_hint.as_deref() == Some("terminal") {
            return true;
        }
    }

    let exe = std::path::Path::new(&window.exe)
        .file_name()
        .map(|name| name.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();

    matches!(
        exe.as_str(),
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

fn is_text_editor(window: &WindowContext, uia: Option<&UiaSnapshot>) -> bool {
    if context::is_edit_class(&window.class) || context::is_edit_class(&window.focused_class) {
        return true;
    }

    uia.and_then(|snapshot| {
        let class = snapshot.class_name.to_ascii_lowercase();
        if class.contains("richedit") || class.contains("_wwg") {
            Some(())
        } else {
            None
        }
    })
    .is_some()
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
