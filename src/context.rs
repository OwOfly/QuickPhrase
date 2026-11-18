use std::sync::OnceLock;

use windows::core::PWSTR;
use windows::Win32::Foundation::{CloseHandle, FILETIME, HANDLE, HWND, RECT};
use windows::Win32::Security::{
    GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, TokenIntegrityLevel,
    TOKEN_MANDATORY_LABEL, TOKEN_QUERY,
};
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetProcessTimes, OpenProcess, OpenProcessToken, QueryFullProcessImageNameW,
    PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetAncestor, GetClassNameW, GetForegroundWindow, GetGUIThreadInfo, GetWindowLongW,
    GetWindowTextW, GetWindowThreadProcessId, IsChild, GA_ROOT, GUITHREADINFO, GWL_STYLE,
};

const ES_PASSWORD: i32 = 0x0020;
const ES_READONLY: i32 = 0x0800;

pub fn is_edit_class(name: &str) -> bool {
    EDIT_CLASSES
        .iter()
        .any(|class| name.eq_ignore_ascii_case(class))
}

const EDIT_CLASSES: &[&str] = &[
    "Edit",
    "RichEdit",
    "RichEdit20A",
    "RichEdit20W",
    "RichEdit50W",
    "_WwG",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Integrity {
    Untrusted,
    Low,
    Medium,
    High,
    System,
    Unknown,
}

impl Integrity {
    pub fn label(self) -> &'static str {
        match self {
            Integrity::Untrusted => "untrusted",
            Integrity::Low => "low",
            Integrity::Medium => "medium",
            Integrity::High => "high",
            Integrity::System => "system",
            Integrity::Unknown => "unknown",
        }
    }

    fn rank(self) -> i32 {
        match self {
            Integrity::Untrusted => 0,
            Integrity::Low => 1,
            Integrity::Medium => 2,
            Integrity::High => 3,
            Integrity::System => 4,
            Integrity::Unknown => -1,
        }
    }
}

pub struct WindowContext {
    pub hwnd: HWND,
    pub pid: u32,
    pub tid: u32,
    pub created: u64,
    pub exe: String,
    pub class: String,
    pub title: String,
    pub focused: HWND,
    pub focused_class: String,
    pub focused_is_child: bool,
    pub focused_style: i32,
    pub caret: Option<RECT>,
    pub integrity: Integrity,
    pub own_integrity: Integrity,
    pub target_higher: bool,
}

impl WindowContext {
    pub fn focused_is_editable(&self) -> bool {
        is_edit_class(&self.focused_class)
    }

    pub fn focused_is_password(&self) -> bool {
        // ES_PASSWORD shares the 0x20 bit with BS_LEFTTEXT and must only be
        // applied to edit controls.
        self.focused_is_editable() && self.focused_style & ES_PASSWORD != 0
    }

    pub fn focused_is_readonly(&self) -> bool {
        // Same for ES_READONLY vs CBS_DISABLENOSCROLL.
        self.focused_is_editable() && self.focused_style & ES_READONLY != 0
    }
}

pub fn snapshot() -> Option<WindowContext> {
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.0.is_null() {
        return None;
    }

    let mut pid = 0u32;
    let tid = unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    if pid == 0 {
        return None;
    }

    let mut created = 0u64;
    let mut exe = String::new();
    let mut integrity = Integrity::Unknown;

    if let Ok(process) = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) } {
        created = process_creation(process);
        exe = process_image(process);
        integrity = process_integrity(process);
        unsafe {
            let _ = CloseHandle(process);
        }
    }

    let own_integrity = *OWN_INTEGRITY.get_or_init(|| unsafe {
        let process = GetCurrentProcess();
        process_integrity(process)
    });

    let mut focused = HWND::default();
    let mut caret = None;
    let mut gui = GUITHREADINFO {
        cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
        ..Default::default()
    };
    if unsafe { GetGUIThreadInfo(tid, &mut gui) }.is_ok() {
        focused = gui.hwndFocus;
        if !gui.hwndCaret.0.is_null() {
            caret = Some(gui.rcCaret);
        }
    }

    let focused_is_child = if focused.0.is_null() {
        false
    } else {
        focused == hwnd || unsafe { IsChild(hwnd, focused) }.as_bool()
    };

    Some(WindowContext {
        hwnd,
        pid,
        tid,
        created,
        exe,
        class: window_class(hwnd),
        title: window_text(hwnd),
        focused,
        focused_class: window_class(focused),
        focused_is_child,
        focused_style: unsafe { GetWindowLongW(focused, GWL_STYLE) },
        caret,
        integrity,
        own_integrity,
        target_higher: integrity.rank() > own_integrity.rank() && integrity != Integrity::Unknown,
    })
}

static OWN_INTEGRITY: OnceLock<Integrity> = OnceLock::new();

pub fn belongs_to(foreground: HWND, candidate: HWND) -> bool {
    if candidate.0.is_null() {
        return false;
    }
    candidate == foreground
        || unsafe { IsChild(foreground, candidate) }.as_bool()
        || unsafe { GetAncestor(candidate, GA_ROOT) == foreground }
}

fn process_creation(process: HANDLE) -> u64 {
    let mut creation = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    if unsafe { GetProcessTimes(process, &mut creation, &mut exit, &mut kernel, &mut user) }
        .is_err()
    {
        return 0;
    }
    ((creation.dwHighDateTime as u64) << 32) | creation.dwLowDateTime as u64
}

fn process_image(process: HANDLE) -> String {
    let mut buffer = [0u16; 1024];
    let mut length = buffer.len() as u32;
    if unsafe {
        QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut length,
        )
    }
    .is_err()
    {
        return String::new();
    }
    String::from_utf16_lossy(&buffer[..length as usize])
}

fn process_integrity(process: HANDLE) -> Integrity {
    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(process, TOKEN_QUERY, &mut token).is_err() {
            return Integrity::Unknown;
        }

        let mut length = 0u32;
        let _ = GetTokenInformation(token, TokenIntegrityLevel, None, 0, &mut length);
        if length == 0 {
            let _ = CloseHandle(token);
            return Integrity::Unknown;
        }

        let mut buffer = vec![0u8; length as usize];
        let ok = GetTokenInformation(
            token,
            TokenIntegrityLevel,
            Some(buffer.as_mut_ptr() as *mut _),
            length,
            &mut length,
        )
        .is_ok();
        let _ = CloseHandle(token);
        if !ok {
            return Integrity::Unknown;
        }

        let label = &*(buffer.as_ptr() as *const TOKEN_MANDATORY_LABEL);
        let sid = label.Label.Sid;
        if sid.0.is_null() {
            return Integrity::Unknown;
        }

        let count = *GetSidSubAuthorityCount(sid);
        if count == 0 {
            return Integrity::Unknown;
        }
        let rid = *GetSidSubAuthority(sid, (count - 1) as u32);

        match rid {
            0x0000..=0x0FFF => Integrity::Untrusted,
            0x1000..=0x1FFF => Integrity::Low,
            0x2000..=0x2FFF => Integrity::Medium,
            0x3000..=0x3FFF => Integrity::High,
            _ => Integrity::System,
        }
    }
}

fn window_class(hwnd: HWND) -> String {
    if hwnd.0.is_null() {
        return String::new();
    }
    let mut buffer = [0u16; 256];
    let length = unsafe { GetClassNameW(hwnd, &mut buffer) };
    if length <= 0 {
        return String::new();
    }
    String::from_utf16_lossy(&buffer[..length as usize])
}

fn window_text(hwnd: HWND) -> String {
    if hwnd.0.is_null() {
        return String::new();
    }
    let mut buffer = [0u16; 512];
    let length = unsafe { GetWindowTextW(hwnd, &mut buffer) };
    if length <= 0 {
        return String::new();
    }
    String::from_utf16_lossy(&buffer[..length as usize])
}
