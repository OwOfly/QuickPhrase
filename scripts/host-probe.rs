// Minimal cross-process injection target for M0-5.
// Creates a plain Win32 window with a native EDIT child, then mirrors the edit
// text to a file every 250 ms.
//
// Build:  rustc -O scripts\host-probe.rs -o target\host-probe.exe
// Usage:  host-probe.exe <text-out-file> <hwnd-out-file>

use std::ffi::c_void;
use std::sync::atomic::{AtomicIsize, Ordering};

type Hwnd = isize;
type Hinstance = isize;
type Hbrush = isize;
type Hcursor = isize;
type Hicon = isize;
type Hmenu = isize;
type Wparam = usize;
type Lparam = isize;
type Lresult = isize;
type Uint = u32;
type Dword = u32;

#[repr(C)]
struct WndClassW {
    style: Uint,
    lpfn_wnd_proc: Option<unsafe extern "system" fn(Hwnd, Uint, Wparam, Lparam) -> Lresult>,
    cb_cls_extra: i32,
    cb_wnd_extra: i32,
    h_instance: Hinstance,
    h_icon: Hicon,
    h_cursor: Hcursor,
    hbr_background: Hbrush,
    lpsz_menu_name: *const u16,
    lpsz_class_name: *const u16,
}

#[repr(C)]
struct Msg {
    hwnd: Hwnd,
    message: Uint,
    wparam: Wparam,
    lparam: Lparam,
    time: Dword,
    pt_x: i32,
    pt_y: i32,
    private: Dword,
}

#[link(name = "user32")]
extern "system" {
    fn RegisterClassW(class: *const WndClassW) -> u16;
    fn CreateWindowExW(
        ex_style: Dword,
        class_name: *const u16,
        window_name: *const u16,
        style: Dword,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        parent: Hwnd,
        menu: Hmenu,
        instance: Hinstance,
        param: *mut c_void,
    ) -> Hwnd;
    fn DefWindowProcW(hwnd: Hwnd, message: Uint, wparam: Wparam, lparam: Lparam) -> Lresult;
    fn ShowWindow(hwnd: Hwnd, cmd: i32) -> i32;
    fn UpdateWindow(hwnd: Hwnd) -> i32;
    fn SetFocus(hwnd: Hwnd) -> Hwnd;
    fn GetMessageW(message: *mut Msg, hwnd: Hwnd, min: Uint, max: Uint) -> i32;
    fn TranslateMessage(message: *const Msg) -> i32;
    fn DispatchMessageW(message: *const Msg) -> Lresult;
    fn PostQuitMessage(code: i32);
    fn DestroyWindow(hwnd: Hwnd) -> i32;
    fn SetTimer(hwnd: Hwnd, id: usize, elapse: Uint, func: *const c_void) -> usize;
    fn GetWindowTextLengthW(hwnd: Hwnd) -> i32;
    fn GetWindowTextW(hwnd: Hwnd, buffer: *mut u16, max: i32) -> i32;
    fn LoadCursorW(instance: Hinstance, name: *const u16) -> Hcursor;
}

#[link(name = "kernel32")]
extern "system" {
    fn GetModuleHandleW(name: *const u16) -> Hinstance;
    fn GetLastError() -> Dword;
}

const WS_OVERLAPPEDWINDOW: Dword = 0x00CF_0000;
const WS_CHILD: Dword = 0x4000_0000;
const WS_VISIBLE: Dword = 0x1000_0000;
const WS_VSCROLL: Dword = 0x0020_0000;
const ES_MULTILINE: Dword = 0x0004;
const ES_AUTOVSCROLL: Dword = 0x0040;
const SW_SHOW: i32 = 5;
const WM_CREATE: Uint = 0x0001;
const WM_DESTROY: Uint = 0x0002;
const WM_ACTIVATE: Uint = 0x0006;
const WM_SETFOCUS: Uint = 0x0007;
const WM_CLOSE: Uint = 0x0010;
const WM_TIMER: Uint = 0x0113;
const TIMER_ID: usize = 1;

static EDIT: AtomicIsize = AtomicIsize::new(0);
static OUT_TEXT: std::sync::OnceLock<String> = std::sync::OnceLock::new();

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

unsafe extern "system" fn window_proc(
    hwnd: Hwnd,
    message: Uint,
    wparam: Wparam,
    lparam: Lparam,
) -> Lresult {
    match message {
        WM_CREATE => {
            let class = wide("EDIT");
            let empty = wide("");
            let edit = CreateWindowExW(
                0,
                class.as_ptr(),
                empty.as_ptr(),
                WS_CHILD | WS_VISIBLE | WS_VSCROLL | ES_MULTILINE | ES_AUTOVSCROLL,
                10,
                10,
                660,
                280,
                hwnd,
                1,
                GetModuleHandleW(std::ptr::null()),
                std::ptr::null_mut(),
            );
            EDIT.store(edit, Ordering::Relaxed);
            SetFocus(edit);
            SetTimer(hwnd, TIMER_ID, 250, std::ptr::null());
            0
        }
        WM_TIMER if wparam == TIMER_ID => {
            let edit = EDIT.load(Ordering::Relaxed);
            let length = GetWindowTextLengthW(edit);
            let mut buffer = vec![0u16; (length.max(0) as usize) + 1];
            let copied = GetWindowTextW(edit, buffer.as_mut_ptr(), buffer.len() as i32);
            let text = String::from_utf16_lossy(&buffer[..copied.max(0) as usize]);
            if let Some(path) = OUT_TEXT.get() {
                let _ = std::fs::write(path, text);
            }
            0
        }
        WM_ACTIVATE => {
            // Real hosts restore the inner control focus when reactivated.
            if (wparam & 0xffff) != 0 {
                let edit = EDIT.load(Ordering::Relaxed);
                if edit != 0 {
                    SetFocus(edit);
                }
            }
            0
        }
        WM_SETFOCUS => {
            let edit = EDIT.load(Ordering::Relaxed);
            if edit != 0 {
                SetFocus(edit);
            }
            0
        }
        WM_CLOSE => {
            DestroyWindow(hwnd);
            0
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: host-probe.exe <text-out-file> <hwnd-out-file>");
        std::process::exit(2);
    }
    let _ = OUT_TEXT.set(args[1].clone());
    let hwnd_out = args[2].clone();

    unsafe {
        let instance = GetModuleHandleW(std::ptr::null());
        let class_name = wide("QuickPhrase.HostProbe");
        let class = WndClassW {
            style: 0,
            lpfn_wnd_proc: Some(window_proc),
            cb_cls_extra: 0,
            cb_wnd_extra: 0,
            h_instance: instance,
            h_icon: 0,
            h_cursor: LoadCursorW(0, 32512 as *const u16),
            hbr_background: 0,
            lpsz_menu_name: std::ptr::null(),
            lpsz_class_name: class_name.as_ptr(),
        };
        if RegisterClassW(&class) == 0 {
            eprintln!("RegisterClassW failed: {}", GetLastError());
            std::process::exit(4);
        }

        let title = wide("QuickPhrase Host Probe");
        let hwnd = CreateWindowExW(
            0,
            class_name.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPEDWINDOW,
            120,
            120,
            720,
            360,
            0,
            0,
            instance,
            std::ptr::null_mut(),
        );
        if hwnd == 0 {
            eprintln!("CreateWindowExW failed: {}", GetLastError());
            std::process::exit(3);
        }
        ShowWindow(hwnd, SW_SHOW);
        UpdateWindow(hwnd);
        let _ = std::fs::write(&hwnd_out, hwnd.to_string());

        let mut message: Msg = std::mem::zeroed();
        while GetMessageW(&mut message, 0, 0, 0) > 0 {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}