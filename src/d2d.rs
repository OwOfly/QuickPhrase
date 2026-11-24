//! Direct2D + DirectWrite anti-aliased rendering engine for QuickPhrase 2.0.
//!
//! Uses a thread-local `ID2D1Factory`, `IDWriteFactory`, and `ID2D1DCRenderTarget`
//! bound to a 32-bpp DIB double buffer, giving flicker-free, sub-pixel anti-aliased
//! vector shapes, icons, gradients, and ClearType DirectWrite text across all Win32
//! windows and owner-draw controls.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HINSTANCE, RECT};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_IGNORE, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_BEZIER_SEGMENT, D2D1_COLOR_F,
    D2D1_FIGURE_BEGIN_HOLLOW, D2D1_FIGURE_END_CLOSED, D2D1_FIGURE_END_OPEN, D2D1_GRADIENT_STOP,
    D2D1_PIXEL_FORMAT, D2D_RECT_F, D2D_SIZE_F, D2D_SIZE_U,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1Bitmap, ID2D1DCRenderTarget, ID2D1Factory, ID2D1GeometrySink,
    ID2D1PathGeometry, ID2D1SolidColorBrush, ID2D1StrokeStyle, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
    D2D1_ARC_SEGMENT, D2D1_ARC_SIZE_LARGE, D2D1_ARC_SIZE_SMALL,
    D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, D2D1_BITMAP_PROPERTIES, D2D1_CAP_STYLE_ROUND,
    D2D1_ELLIPSE, D2D1_EXTEND_MODE_CLAMP, D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_GAMMA_2_2,
    D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES, D2D1_LINE_JOIN_ROUND, D2D1_RENDER_TARGET_PROPERTIES,
    D2D1_RENDER_TARGET_TYPE_DEFAULT, D2D1_RENDER_TARGET_USAGE_NONE, D2D1_ROUNDED_RECT,
    D2D1_STROKE_STYLE_PROPERTIES, D2D1_SWEEP_DIRECTION_CLOCKWISE,
    D2D1_SWEEP_DIRECTION_COUNTER_CLOCKWISE, D2D1_TEXT_ANTIALIAS_MODE_CLEARTYPE,
};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteFontCollection, IDWriteTextFormat,
    IDWriteTextLayout, DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL,
    DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT, DWRITE_HIT_TEST_METRICS,
    DWRITE_PARAGRAPH_ALIGNMENT_CENTER, DWRITE_PARAGRAPH_ALIGNMENT_FAR,
    DWRITE_PARAGRAPH_ALIGNMENT_NEAR, DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING,
    DWRITE_TEXT_ALIGNMENT_TRAILING, DWRITE_TRIMMING, DWRITE_TRIMMING_GRANULARITY_CHARACTER,
    DWRITE_WORD_WRAPPING_NO_WRAP, DWRITE_WORD_WRAPPING_WRAP,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, GetDIBits,
    InvalidateRect, PatBlt, ReleaseDC, SelectObject, WindowFromDC, BITMAPINFO, BITMAPINFOHEADER,
    BI_RGB, BLACKNESS, DIB_RGB_COLORS, HDC, HGDIOBJ, SRCCOPY,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    DestroyIcon, GetIconInfo, LoadImageW, HICON, ICONINFO, IMAGE_ICON, LR_DEFAULTCOLOR,
};
use windows_numerics::{Matrix3x2, Vector2};

use crate::theme::Palette;

#[inline]
fn pt(x: f32, y: f32) -> Vector2 {
    Vector2 { X: x, Y: y }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum AlignH {
    Left,
    Center,
    Right,
}

#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum AlignV {
    Top,
    Center,
    Bottom,
}

#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum Icon {
    Bolt,
    Search,
    Gear,
    Pin,
    Multiline,
    Chat,
    Grid,
    Trash,
    Restore,
    Plus,
    Pause,
    Play,
    Info,
    Exit,
    Check,
    Warning,
    Keyboard,
    EditPen,
    Palette,
    Folder,
    ChevronDown,
    Close,
    Lock,
    Clipboard,
}

/// Unified QuickPhrase 2.0 Fluent typography scale.
/// Every window shares this single ramp (sizes in 96-DPI logical pixels).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum TextStyle {
    /// 16.5px SemiBold — page headers, About title, stat KPIs.
    Display,
    /// 14.0px SemiBold — pane/card headers (e.g. "编辑短语").
    Title,
    /// 13.0px SemiBold — phrase row titles, setting card titles, primary buttons, active nav.
    Heading,
    /// 13.0px Regular — body text, secondary buttons, combo rows, inactive nav, tray rows.
    Body,
    /// 12.0px Medium — form field labels, category chips, alert banners, subtle pills.
    Label,
    /// 12.0px Regular — phrase previews, setting descriptions, footer hints, status text.
    Caption,
    /// 11.0px Medium — count badges, category tags, counters, sidebar section headers.
    Badge,
    /// 11.5px Medium Mono — `<kbd>` keycaps, shortcut indices, file paths.
    Mono,
}

impl TextStyle {
    pub const fn logical_size(self) -> f32 {
        match self {
            Self::Display => 16.5,
            Self::Title => 14.0,
            Self::Heading | Self::Body => 13.0,
            Self::Label | Self::Caption => 12.0,
            Self::Badge => 11.0,
            Self::Mono => 11.5,
        }
    }

    pub const fn weight(self) -> i32 {
        match self {
            Self::Display | Self::Title | Self::Heading => 600,
            Self::Label | Self::Badge | Self::Mono => 500,
            Self::Body | Self::Caption => 400,
        }
    }

    pub const fn is_mono(self) -> bool {
        matches!(self, Self::Mono)
    }

    pub fn spec(self, dpi: u32) -> (f32, i32, bool) {
        (
            scaled_f(self.logical_size(), dpi),
            self.weight(),
            self.is_mono(),
        )
    }
}

fn find_installed_family(candidates: &[&'static str], fallback: &'static str) -> &'static str {
    unsafe {
        let Ok(dwrite): Result<IDWriteFactory, _> = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)
        else {
            return fallback;
        };
        let mut collection: Option<IDWriteFontCollection> = None;
        if dwrite
            .GetSystemFontCollection(&mut collection, false)
            .is_err()
        {
            return fallback;
        }
        let Some(collection) = collection else {
            return fallback;
        };
        for &name in candidates {
            let wide = crate::platform::wide(name);
            let mut index = 0u32;
            let mut exists = windows::core::BOOL(0);
            if collection
                .FindFamilyName(PCWSTR(wide.as_ptr()), &mut index, &mut exists)
                .is_ok()
                && exists.as_bool()
            {
                return name;
            }
        }
    }
    fallback
}

/// Primary UI font family shared by both DirectWrite and Win32 GDI `EDIT` controls.
/// Win11 resolves to `Segoe UI Variable Text` (linked to `Microsoft YaHei UI` for CJK);
/// older builds fall back cleanly to `Segoe UI` -> `Microsoft YaHei UI`.
pub fn ui_font_family() -> &'static str {
    static FAMILY: OnceLock<&'static str> = OnceLock::new();
    FAMILY.get_or_init(|| {
        find_installed_family(
            &["Segoe UI Variable Text", "Segoe UI", "Microsoft YaHei UI"],
            "Microsoft YaHei UI",
        )
    })
}

/// Monospace font family for `<kbd>` keycaps, shortcuts, and file paths.
pub fn mono_font_family() -> &'static str {
    static FAMILY: OnceLock<&'static str> = OnceLock::new();
    FAMILY.get_or_init(|| find_installed_family(&["Cascadia Mono", "Consolas"], "Consolas"))
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct FormatKey {
    size_tenths: u32,
    weight: i32,
    mono: bool,
    align_h: AlignH,
    align_v: AlignV,
    wrap: bool,
}

struct Engine {
    factory: ID2D1Factory,
    dwrite: IDWriteFactory,
    target: ID2D1DCRenderTarget,
    formats: HashMap<FormatKey, IDWriteTextFormat>,
    icons: HashMap<Icon, ID2D1PathGeometry>,
    brushes: RefCell<HashMap<(u32, u32), ID2D1SolidColorBrush>>,
    buffers: HashMap<(i32, i32), Buffer>,
    buffer_clock: u64,
    round_stroke: Option<ID2D1StrokeStyle>,
    brand_bitmap: Option<ID2D1Bitmap>,
    brand_loaded: bool,
}

/// Reusable offscreen DC + DIB for one paint size.
struct Buffer {
    mem_dc: HDC,
    dib: HGDIOBJ,
    old_bmp: HGDIOBJ,
    last_used: u64,
}

impl Drop for Buffer {
    fn drop(&mut self) {
        unsafe {
            let _ = SelectObject(self.mem_dc, self.old_bmp);
            let _ = DeleteObject(self.dib);
            let _ = DeleteDC(self.mem_dc);
        }
    }
}

thread_local! {
    static ENGINE: RefCell<Option<Engine>> = const { RefCell::new(None) };
}

#[cfg(test)]
thread_local! {
    static BUFFER_CREATES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn load_brand_bitmap(target: &ID2D1DCRenderTarget) -> Option<ID2D1Bitmap> {
    const SIZE: u32 = 128;
    unsafe {
        let module = GetModuleHandleW(None).ok()?;
        let instance = HINSTANCE(module.0);
        #[allow(clippy::manual_dangling_ptr)]
        let icon_id = PCWSTR(1 as *const u16);
        let handle = LoadImageW(
            Some(instance),
            icon_id,
            IMAGE_ICON,
            SIZE as i32,
            SIZE as i32,
            LR_DEFAULTCOLOR,
        )
        .ok()?;
        let hicon = HICON(handle.0);
        let mut info = ICONINFO::default();
        if GetIconInfo(hicon, &mut info).is_err() {
            let _ = DestroyIcon(hicon);
            return None;
        }
        let mut pixels = vec![0u8; (SIZE * SIZE * 4) as usize];
        let mut bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: SIZE as i32,
                biHeight: -(SIZE as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let hdc = GetDC(None);
        let rows = if !hdc.0.is_null() && !info.hbmColor.0.is_null() {
            GetDIBits(
                hdc,
                info.hbmColor,
                0,
                SIZE,
                Some(pixels.as_mut_ptr() as *mut core::ffi::c_void),
                &mut bmi,
                DIB_RGB_COLORS,
            )
        } else {
            0
        };
        if !hdc.0.is_null() {
            let _ = ReleaseDC(None, hdc);
        }
        if !info.hbmColor.0.is_null() {
            let _ = DeleteObject(HGDIOBJ(info.hbmColor.0));
        }
        if !info.hbmMask.0.is_null() {
            let _ = DeleteObject(HGDIOBJ(info.hbmMask.0));
        }
        let _ = DestroyIcon(hicon);
        if rows <= 0 {
            return None;
        }
        // Premultiply RGB by alpha for Direct2D `D2D1_ALPHA_MODE_PREMULTIPLIED`.
        for chunk in pixels.chunks_exact_mut(4) {
            let a = chunk[3] as u32;
            if a < 255 {
                chunk[0] = ((chunk[0] as u32 * a + 127) / 255) as u8;
                chunk[1] = ((chunk[1] as u32 * a + 127) / 255) as u8;
                chunk[2] = ((chunk[2] as u32 * a + 127) / 255) as u8;
            }
        }
        let props = D2D1_BITMAP_PROPERTIES {
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: 96.0,
            dpiY: 96.0,
        };
        target
            .CreateBitmap(
                D2D_SIZE_U {
                    width: SIZE,
                    height: SIZE,
                },
                Some(pixels.as_ptr() as *const core::ffi::c_void),
                SIZE * 4,
                &props,
            )
            .ok()
    }
}

fn create_engine() -> Option<Engine> {
    unsafe {
        let factory: ID2D1Factory =
            D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None).ok()?;
        let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).ok()?;
        let props = D2D1_RENDER_TARGET_PROPERTIES {
            r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_IGNORE,
            },
            dpiX: 96.0,
            dpiY: 96.0,
            usage: D2D1_RENDER_TARGET_USAGE_NONE,
            minLevel: Default::default(),
        };
        let target = factory.CreateDCRenderTarget(&props).ok()?;
        target.SetAntialiasMode(D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
        target.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_CLEARTYPE);
        let stroke_props = D2D1_STROKE_STYLE_PROPERTIES {
            startCap: D2D1_CAP_STYLE_ROUND,
            endCap: D2D1_CAP_STYLE_ROUND,
            lineJoin: D2D1_LINE_JOIN_ROUND,
            ..Default::default()
        };
        let round_stroke = factory.CreateStrokeStyle(&stroke_props, None).ok();
        Some(Engine {
            factory,
            dwrite,
            target,
            formats: HashMap::new(),
            icons: HashMap::new(),
            brushes: RefCell::new(HashMap::new()),
            buffers: HashMap::new(),
            buffer_clock: 0,
            round_stroke,
            brand_bitmap: None,
            brand_loaded: false,
        })
    }
}

pub fn color_f(color: COLORREF, alpha: f32) -> D2D1_COLOR_F {
    let r = (color.0 & 0xFF) as f32 / 255.0;
    let g = ((color.0 >> 8) & 0xFF) as f32 / 255.0;
    let b = ((color.0 >> 16) & 0xFF) as f32 / 255.0;
    D2D1_COLOR_F { r, g, b, a: alpha }
}

pub fn rect_f(r: &RECT) -> D2D_RECT_F {
    D2D_RECT_F {
        left: r.left as f32,
        top: r.top as f32,
        right: r.right as f32,
        bottom: r.bottom as f32,
    }
}

pub fn scaled(value: i32, dpi: u32) -> i32 {
    (value * dpi as i32 + 48) / 96
}

pub fn scaled_f(value: f32, dpi: u32) -> f32 {
    value * dpi as f32 / 96.0
}

/// Render using Direct2D into a flicker-free 32-bpp DIB double buffer, then
/// `BitBlt` into `hdc` at `rect`. Coordinates inside `draw` match `rect`'s
/// original coordinate space.
static D2D_FAILURE_LOGGED: AtomicBool = AtomicBool::new(false);

thread_local! {
    /// One pending retry per HWND, so a failure on window A cannot suppress
    /// the retry for window B.
    static D2D_RETRY_PENDING: RefCell<HashSet<isize>> = RefCell::new(HashSet::new());
}

/// Ask for one more paint after a failed frame. A per-window flag prevents a
/// permanent engine failure from becoming an infinite InvalidateRect loop.
fn schedule_retry(hdc: HDC) {
    let hwnd = unsafe { WindowFromDC(hdc) };
    if hwnd.0.is_null() {
        return;
    }
    let key = hwnd.0 as isize;
    let first = D2D_RETRY_PENDING.with(|set| set.borrow_mut().insert(key));
    if first {
        unsafe {
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
    }
}

fn clear_retry(hdc: HDC) {
    let hwnd = unsafe { WindowFromDC(hdc) };
    if !hwnd.0.is_null() {
        let key = hwnd.0 as isize;
        D2D_RETRY_PENDING.with(|set| {
            set.borrow_mut().remove(&key);
        });
    }
}

pub fn paint_buffered<F: FnOnce(&mut Canvas<'_>)>(hdc: HDC, rect: RECT, draw: F) -> bool {
    let width = rect.right - rect.left;
    let height = rect.bottom - rect.top;
    if width <= 0 || height <= 0 {
        return false;
    }

    ENGINE.with(|slot| {
        let mut guard = slot.borrow_mut();
        if guard.is_none() {
            *guard = create_engine();
        }
        let Some(engine) = guard.as_mut() else {
            schedule_retry(hdc);
            return false;
        };

        unsafe {
            let key = (width, height);
            let tick = engine.buffer_clock.wrapping_add(1);
            engine.buffer_clock = tick;
            let mut evicted: Option<Buffer> = None;
            let existing = engine.buffers.get_mut(&key).map(|buffer| {
                buffer.last_used = tick;
                buffer.mem_dc
            });
            let mem_dc = match existing {
                Some(mem_dc) => mem_dc,
                None => {
                    let mem_dc = CreateCompatibleDC(Some(hdc));
                    if mem_dc.0.is_null() {
                        schedule_retry(hdc);
                        return false;
                    }
                    let bmi = BITMAPINFO {
                        bmiHeader: BITMAPINFOHEADER {
                            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                            biWidth: width,
                            biHeight: -height, // top-down DIB
                            biPlanes: 1,
                            biBitCount: 32,
                            biCompression: BI_RGB.0,
                            ..Default::default()
                        },
                        ..Default::default()
                    };
                    let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
                    let Ok(dib) =
                        CreateDIBSection(Some(mem_dc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0)
                    else {
                        let _ = DeleteDC(mem_dc);
                        schedule_retry(hdc);
                        return false;
                    };
                    let old_bmp = SelectObject(mem_dc, HGDIOBJ(dib.0));
                    if old_bmp.0.is_null() {
                        let _ = DeleteObject(HGDIOBJ(dib.0));
                        let _ = DeleteDC(mem_dc);
                        schedule_retry(hdc);
                        return false;
                    }
                    #[cfg(test)]
                    BUFFER_CREATES.with(|count| count.set(count.get() + 1));
                    if engine.buffers.len() >= 32 {
                        if let Some(evict) = engine
                            .buffers
                            .iter()
                            .min_by_key(|(_, buffer)| buffer.last_used)
                            .map(|(key, _)| *key)
                        {
                            // Keep the DC alive until after this frame's
                            // BindDC/EndDraw; the target may still reference it.
                            evicted = engine.buffers.remove(&evict);
                        }
                    }
                    engine.buffers.insert(
                        key,
                        Buffer {
                            mem_dc,
                            dib: HGDIOBJ(dib.0),
                            old_bmp,
                            last_used: tick,
                        },
                    );
                    mem_dc
                }
            };
            // The DIB is reused; erase it so a draw that does not cover every
            // pixel cannot show the previous frame.
            if !PatBlt(mem_dc, 0, 0, width, height, BLACKNESS).as_bool() {
                // A bad DC/DIB must not be reused with the previous frame.
                *guard = None;
                schedule_retry(hdc);
                return false;
            }

            let bind_rect = RECT {
                left: 0,
                top: 0,
                right: width,
                bottom: height,
            };
            let ok = if engine.target.BindDC(mem_dc, &bind_rect).is_ok() {
                engine.target.BeginDraw();
                engine
                    .target
                    .SetTransform(&Matrix3x2::translation(-rect.left as f32, -rect.top as f32));
                let mut canvas = Canvas { engine };
                draw(&mut canvas);
                engine.target.SetTransform(&Matrix3x2::identity());
                let res = engine.target.EndDraw(None, None);
                if res.is_ok() {
                    BitBlt(
                        hdc,
                        rect.left,
                        rect.top,
                        width,
                        height,
                        Some(mem_dc),
                        0,
                        0,
                        SRCCOPY,
                    )
                    .is_ok()
                } else {
                    false
                }
            } else {
                false
            };

            if !ok {
                // A lost device (D2DERR_RECREATE_TARGET) or a transient bind
                // failure: drop the engine so the next paint starts fresh.
                *guard = None;
                if !D2D_FAILURE_LOGGED.swap(true, Ordering::Relaxed) {
                    crate::logging::warn("Direct2D paint failed; retrying with a fresh engine");
                }
                schedule_retry(hdc);
            } else {
                clear_retry(hdc);
            }
            // The target is done with the previous DC; safe to release an
            // evicted buffer now.
            drop(evicted);
            ok
        }
    })
}

/// Measure single-line text using a unified `TextStyle` token.
pub fn measure_style(text: &str, style: TextStyle, dpi: u32) -> (f32, f32) {
    let (size_px, weight, mono) = style.spec(dpi);
    measure_text(text, size_px, weight, mono)
}

/// Measure single-line text width/height using DirectWrite without needing an HDC.
pub fn measure_text(text: &str, size_px: f32, weight: i32, mono: bool) -> (f32, f32) {
    if text.is_empty() {
        return (0.0, size_px * 1.25);
    }
    ENGINE.with(|slot| {
        let mut guard = slot.borrow_mut();
        if guard.is_none() {
            *guard = create_engine();
        }
        let Some(engine) = guard.as_mut() else {
            return (text.chars().count() as f32 * size_px * 0.6, size_px * 1.3);
        };
        let mut canvas = Canvas { engine };
        canvas.measure(text, size_px, weight, mono)
    })
}

pub struct Canvas<'a> {
    engine: &'a mut Engine,
}

impl<'a> Canvas<'a> {
    fn brush(&self, color: COLORREF, alpha: f32) -> Option<ID2D1SolidColorBrush> {
        let key = (color.0, alpha.to_bits());
        if let Some(brush) = self.engine.brushes.borrow().get(&key) {
            return Some(brush.clone());
        }
        let brush = unsafe {
            self.engine
                .target
                .CreateSolidColorBrush(&color_f(color, alpha), None)
                .ok()?
        };
        self.engine.brushes.borrow_mut().insert(key, brush.clone());
        Some(brush)
    }

    fn format(&mut self, key: FormatKey) -> Option<IDWriteTextFormat> {
        if let Some(fmt) = self.engine.formats.get(&key) {
            return Some(fmt.clone());
        }
        let family_name = if key.mono {
            mono_font_family()
        } else {
            ui_font_family()
        };
        let family_wide = crate::platform::wide(family_name);
        let size = (key.size_tenths as f32 / 10.0).max(8.0);
        unsafe {
            let fmt = self
                .engine
                .dwrite
                .CreateTextFormat(
                    PCWSTR(family_wide.as_ptr()),
                    None,
                    DWRITE_FONT_WEIGHT(key.weight),
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    size,
                    w!("zh-CN"),
                )
                .ok()?;
            let _ = fmt.SetTextAlignment(match key.align_h {
                AlignH::Left => DWRITE_TEXT_ALIGNMENT_LEADING,
                AlignH::Center => DWRITE_TEXT_ALIGNMENT_CENTER,
                AlignH::Right => DWRITE_TEXT_ALIGNMENT_TRAILING,
            });
            let _ = fmt.SetParagraphAlignment(match key.align_v {
                AlignV::Top => DWRITE_PARAGRAPH_ALIGNMENT_NEAR,
                AlignV::Center => DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
                AlignV::Bottom => DWRITE_PARAGRAPH_ALIGNMENT_FAR,
            });
            let _ = fmt.SetWordWrapping(if key.wrap {
                DWRITE_WORD_WRAPPING_WRAP
            } else {
                DWRITE_WORD_WRAPPING_NO_WRAP
            });
            if let Ok(ellipsis) = self.engine.dwrite.CreateEllipsisTrimmingSign(&fmt) {
                let trim = DWRITE_TRIMMING {
                    granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER,
                    delimiter: 0,
                    delimiterCount: 0,
                };
                let _ = fmt.SetTrimming(&trim, &ellipsis);
            }
            self.engine.formats.insert(key, fmt.clone());
            Some(fmt)
        }
    }

    pub fn clear(&self, color: COLORREF) {
        unsafe {
            self.engine.target.Clear(Some(&color_f(color, 1.0)));
        }
    }

    pub fn fill_rect(&self, rect: &RECT, color: COLORREF) {
        self.fill_rect_alpha(rect_f(rect), color, 1.0);
    }

    #[allow(dead_code)]
    pub fn fill_rect_f(&self, rect: D2D_RECT_F, color: COLORREF) {
        self.fill_rect_alpha(rect, color, 1.0);
    }

    pub fn fill_rect_alpha(&self, rect: D2D_RECT_F, color: COLORREF, alpha: f32) {
        let Some(brush) = self.brush(color, alpha) else {
            return;
        };
        unsafe {
            self.engine.target.FillRectangle(&rect, &brush);
        }
    }

    pub fn fill_round(&self, rect: &RECT, radius: f32, color: COLORREF) {
        self.fill_round_f(rect_f(rect), radius, color, 1.0);
    }

    pub fn fill_round_f(&self, rect: D2D_RECT_F, radius: f32, color: COLORREF, alpha: f32) {
        let Some(brush) = self.brush(color, alpha) else {
            return;
        };
        let rr = D2D1_ROUNDED_RECT {
            rect,
            radiusX: radius,
            radiusY: radius,
        };
        unsafe {
            self.engine.target.FillRoundedRectangle(&rr, &brush);
        }
    }

    pub fn stroke_round(&self, rect: &RECT, radius: f32, color: COLORREF, width: f32) {
        self.stroke_round_f(rect_f(rect), radius, color, 1.0, width);
    }

    pub fn stroke_round_f(
        &self,
        rect: D2D_RECT_F,
        radius: f32,
        color: COLORREF,
        alpha: f32,
        width: f32,
    ) {
        let Some(brush) = self.brush(color, alpha) else {
            return;
        };
        let half = width * 0.5;
        let inset = D2D_RECT_F {
            left: rect.left + half,
            top: rect.top + half,
            right: rect.right - half,
            bottom: rect.bottom - half,
        };
        if inset.right <= inset.left || inset.bottom <= inset.top {
            return;
        }
        let rr = D2D1_ROUNDED_RECT {
            rect: inset,
            radiusX: (radius - half).max(0.0),
            radiusY: (radius - half).max(0.0),
        };
        unsafe {
            self.engine
                .target
                .DrawRoundedRectangle(&rr, &brush, width, None);
        }
    }

    pub fn fill_stroke_round(
        &self,
        rect: &RECT,
        radius: f32,
        fill: COLORREF,
        stroke: COLORREF,
        stroke_width: f32,
    ) {
        self.fill_round(rect, radius, fill);
        if stroke_width > 0.0 {
            self.stroke_round(rect, radius, stroke, stroke_width);
        }
    }

    pub fn fill_gradient_round(&self, rect: &RECT, radius: f32, start: COLORREF, end: COLORREF) {
        let r = rect_f(rect);
        let stops = [
            D2D1_GRADIENT_STOP {
                position: 0.0,
                color: color_f(start, 1.0),
            },
            D2D1_GRADIENT_STOP {
                position: 1.0,
                color: color_f(end, 1.0),
            },
        ];
        unsafe {
            let Ok(collection) = self.engine.target.CreateGradientStopCollection(
                &stops,
                D2D1_GAMMA_2_2,
                D2D1_EXTEND_MODE_CLAMP,
            ) else {
                self.fill_round(rect, radius, start);
                return;
            };
            let props = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
                startPoint: pt(r.left, r.top),
                endPoint: pt(r.right, r.bottom),
            };
            let Ok(brush) = self
                .engine
                .target
                .CreateLinearGradientBrush(&props, None, &collection)
            else {
                self.fill_round(rect, radius, start);
                return;
            };
            let rr = D2D1_ROUNDED_RECT {
                rect: r,
                radiusX: radius,
                radiusY: radius,
            };
            self.engine.target.FillRoundedRectangle(&rr, &brush);
        }
    }

    /// Draw the unified QuickPhrase brand logo into `rect`, using the embedded
    /// high-resolution icon resource (cached as a Direct2D premultiplied bitmap)
    /// with a clean geometric fallback if unavailable.
    pub fn brand_logo(&mut self, rect: &RECT, dpi: u32) {
        if !self.engine.brand_loaded {
            self.engine.brand_loaded = true;
            self.engine.brand_bitmap = load_brand_bitmap(&self.engine.target);
        }
        if let Some(bmp) = &self.engine.brand_bitmap {
            let dest = rect_f(rect);
            unsafe {
                self.engine.target.DrawBitmap(
                    bmp,
                    Some(&dest),
                    1.0,
                    D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
                    None,
                );
            }
            return;
        }
        let w = (rect.right - rect.left) as f32;
        self.fill_gradient_round(
            rect,
            (w * 0.24).max(scaled_f(6.0, dpi)),
            crate::theme::rgb(0x6E, 0x63, 0xFF),
            crate::theme::rgb(0x2B, 0x1F, 0x96),
        );
        let cx = (rect.left + rect.right) as f32 * 0.5;
        let cy = (rect.top + rect.bottom) as f32 * 0.5;
        let r = w * 0.23;
        let sw = (w * 0.11).max(1.5);
        self.stroke_circle(cx, cy, r, COLORREF(0xFFFFFF), sw);
        self.line(
            cx + w * 0.04,
            cy + w * 0.04,
            cx + w * 0.25,
            cy + w * 0.25,
            COLORREF(0xFFFFFF),
            sw,
        );
    }

    pub fn fill_circle(&self, cx: f32, cy: f32, radius: f32, color: COLORREF) {
        self.fill_circle_alpha(cx, cy, radius, color, 1.0);
    }

    pub fn fill_circle_alpha(&self, cx: f32, cy: f32, radius: f32, color: COLORREF, alpha: f32) {
        let Some(brush) = self.brush(color, alpha) else {
            return;
        };
        let ellipse = D2D1_ELLIPSE {
            point: pt(cx, cy),
            radiusX: radius,
            radiusY: radius,
        };
        unsafe {
            self.engine.target.FillEllipse(&ellipse, &brush);
        }
    }

    pub fn stroke_circle(&self, cx: f32, cy: f32, radius: f32, color: COLORREF, width: f32) {
        let Some(brush) = self.brush(color, 1.0) else {
            return;
        };
        let ellipse = D2D1_ELLIPSE {
            point: pt(cx, cy),
            radiusX: radius,
            radiusY: radius,
        };
        unsafe {
            self.engine
                .target
                .DrawEllipse(&ellipse, &brush, width, None);
        }
    }

    pub fn line(&self, x1: f32, y1: f32, x2: f32, y2: f32, color: COLORREF, width: f32) {
        let Some(brush) = self.brush(color, 1.0) else {
            return;
        };
        unsafe {
            self.engine.target.DrawLine(
                pt(x1, y1),
                pt(x2, y2),
                &brush,
                width,
                self.engine.round_stroke.as_ref(),
            );
        }
    }

    pub fn measure(&mut self, text: &str, size_px: f32, weight: i32, mono: bool) -> (f32, f32) {
        if text.is_empty() {
            return (0.0, size_px * 1.25);
        }
        let key = FormatKey {
            size_tenths: (size_px * 10.0).round() as u32,
            weight,
            mono,
            align_h: AlignH::Left,
            align_v: AlignV::Top,
            wrap: false,
        };
        let Some(fmt) = self.format(key) else {
            return (text.chars().count() as f32 * size_px * 0.6, size_px * 1.25);
        };
        let utf16: Vec<u16> = text.encode_utf16().collect();
        unsafe {
            let Ok(layout) = self
                .engine
                .dwrite
                .CreateTextLayout(&utf16, &fmt, 4096.0, 256.0)
            else {
                return (text.chars().count() as f32 * size_px * 0.6, size_px * 1.25);
            };
            let mut metrics = Default::default();
            if layout.GetMetrics(&mut metrics).is_ok() {
                (metrics.widthIncludingTrailingWhitespace, metrics.height)
            } else {
                (text.chars().count() as f32 * size_px * 0.6, size_px * 1.25)
            }
        }
    }

    pub fn measure_style(&mut self, text: &str, style: TextStyle, dpi: u32) -> (f32, f32) {
        let (size_px, weight, mono) = style.spec(dpi);
        self.measure(text, size_px, weight, mono)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw_text(
        &mut self,
        text: &str,
        rect: &RECT,
        style: TextStyle,
        dpi: u32,
        color: COLORREF,
        align_h: AlignH,
        align_v: AlignV,
    ) {
        let (size_px, weight, mono) = style.spec(dpi);
        self.text_styled(
            text,
            rect_f(rect),
            size_px,
            weight,
            mono,
            false,
            color,
            align_h,
            align_v,
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw_text_wrapped(
        &mut self,
        text: &str,
        rect: &RECT,
        style: TextStyle,
        dpi: u32,
        color: COLORREF,
        align_h: AlignH,
        align_v: AlignV,
    ) {
        let (size_px, weight, mono) = style.spec(dpi);
        self.text_styled(
            text,
            rect_f(rect),
            size_px,
            weight,
            mono,
            true,
            color,
            align_h,
            align_v,
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw_text_highlighted(
        &mut self,
        text: &str,
        rect: &RECT,
        style: TextStyle,
        dpi: u32,
        normal: COLORREF,
        highlight: Option<(usize, usize)>,
        hl_bg: COLORREF,
        hl_fg: COLORREF,
        radius: f32,
    ) {
        let (size_px, weight, _) = style.spec(dpi);
        self.text_highlighted(
            text, rect, size_px, weight, normal, highlight, hl_bg, hl_fg, radius,
        );
    }

    #[allow(dead_code, clippy::too_many_arguments)]
    pub fn text(
        &mut self,
        text: &str,
        rect: &RECT,
        size_px: f32,
        weight: i32,
        color: COLORREF,
        align_h: AlignH,
        align_v: AlignV,
    ) {
        self.text_styled(
            text,
            rect_f(rect),
            size_px,
            weight,
            false,
            false,
            color,
            align_h,
            align_v,
        );
    }

    #[allow(dead_code, clippy::too_many_arguments)]
    pub fn text_wrap(
        &mut self,
        text: &str,
        rect: &RECT,
        size_px: f32,
        weight: i32,
        color: COLORREF,
        align_h: AlignH,
        align_v: AlignV,
    ) {
        self.text_styled(
            text,
            rect_f(rect),
            size_px,
            weight,
            false,
            true,
            color,
            align_h,
            align_v,
        );
    }

    #[allow(dead_code, clippy::too_many_arguments)]
    pub fn text_mono(
        &mut self,
        text: &str,
        rect: &RECT,
        size_px: f32,
        weight: i32,
        color: COLORREF,
        align_h: AlignH,
        align_v: AlignV,
    ) {
        self.text_styled(
            text,
            rect_f(rect),
            size_px,
            weight,
            true,
            false,
            color,
            align_h,
            align_v,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn text_styled(
        &mut self,
        text: &str,
        rect: D2D_RECT_F,
        size_px: f32,
        weight: i32,
        mono: bool,
        wrap: bool,
        color: COLORREF,
        align_h: AlignH,
        align_v: AlignV,
    ) {
        if text.is_empty() || rect.right <= rect.left || rect.bottom <= rect.top {
            return;
        }
        let key = FormatKey {
            size_tenths: (size_px * 10.0).round() as u32,
            weight,
            mono,
            align_h,
            align_v,
            wrap,
        };
        let Some(fmt) = self.format(key) else {
            return;
        };
        let Some(brush) = self.brush(color, 1.0) else {
            return;
        };
        let utf16: Vec<u16> = text.encode_utf16().collect();
        unsafe {
            let max_w = (rect.right - rect.left).max(1.0);
            let max_h = (rect.bottom - rect.top).max(1.0);
            let Ok(layout) = self
                .engine
                .dwrite
                .CreateTextLayout(&utf16, &fmt, max_w, max_h)
            else {
                return;
            };
            self.engine
                .target
                .PushAxisAlignedClip(&rect, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            self.engine.target.DrawTextLayout(
                pt(rect.left, rect.top),
                &layout,
                &brush,
                Default::default(),
            );
            self.engine.target.PopAxisAlignedClip();
        }
    }

    /// Draw single-line text with an optional UTF-8 byte-range highlight pill.
    #[allow(clippy::too_many_arguments)]
    pub fn text_highlighted(
        &mut self,
        text: &str,
        rect: &RECT,
        size_px: f32,
        weight: i32,
        normal: COLORREF,
        highlight: Option<(usize, usize)>,
        hl_bg: COLORREF,
        hl_fg: COLORREF,
        radius: f32,
    ) {
        let Some((start, end)) = highlight.filter(|(s, e)| {
            s < e && *e <= text.len() && text.is_char_boundary(*s) && text.is_char_boundary(*e)
        }) else {
            self.text(
                text,
                rect,
                size_px,
                weight,
                normal,
                AlignH::Left,
                AlignV::Center,
            );
            return;
        };
        let r = rect_f(rect);
        if r.right <= r.left || r.bottom <= r.top {
            return;
        }
        let key = FormatKey {
            size_tenths: (size_px * 10.0).round() as u32,
            weight,
            mono: false,
            align_h: AlignH::Left,
            align_v: AlignV::Center,
            wrap: false,
        };
        let Some(fmt) = self.format(key) else {
            return;
        };
        let Some(normal_brush) = self.brush(normal, 1.0) else {
            return;
        };
        let Some(hl_fg_brush) = self.brush(hl_fg, 1.0) else {
            return;
        };
        let utf16: Vec<u16> = text.encode_utf16().collect();
        let u16_start = text[..start].encode_utf16().count() as u32;
        let u16_len = text[start..end].encode_utf16().count() as u32;

        unsafe {
            let max_w = (r.right - r.left).max(1.0);
            let max_h = (r.bottom - r.top).max(1.0);
            let Ok(layout): Result<IDWriteTextLayout, _> = self
                .engine
                .dwrite
                .CreateTextLayout(&utf16, &fmt, max_w, max_h)
            else {
                return;
            };

            // Measure exact highlight box via DirectWrite HitTestTextRange.
            let mut hit_count = 0u32;
            let _ = layout.HitTestTextRange(u16_start, u16_len, 0.0, 0.0, None, &mut hit_count);
            let mut hits = vec![DWRITE_HIT_TEST_METRICS::default(); hit_count.max(1) as usize];
            if hit_count > 0 {
                let _ = layout.HitTestTextRange(
                    u16_start,
                    u16_len,
                    0.0,
                    0.0,
                    Some(&mut hits),
                    &mut hit_count,
                );
            }

            let range = windows::Win32::Graphics::DirectWrite::DWRITE_TEXT_RANGE {
                startPosition: u16_start,
                length: u16_len,
            };
            let _ = layout.SetDrawingEffect(&hl_fg_brush, range);

            self.engine
                .target
                .PushAxisAlignedClip(&r, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);

            for hit in hits.iter().take(hit_count as usize) {
                if hit.width > 0.0 && hit.height > 0.0 {
                    let pill = D2D_RECT_F {
                        left: (r.left + hit.left - 1.5).max(r.left),
                        top: (r.top + hit.top - 0.5).max(r.top),
                        right: (r.left + hit.left + hit.width + 1.5).min(r.right),
                        bottom: (r.top + hit.top + hit.height + 0.5).min(r.bottom),
                    };
                    self.fill_round_f(pill, radius, hl_bg, 1.0);
                }
            }

            self.engine.target.DrawTextLayout(
                pt(r.left, r.top),
                &layout,
                &normal_brush,
                Default::default(),
            );
            self.engine.target.PopAxisAlignedClip();
        }
    }

    /// Tactile `<kbd>` keycap (border + subtle bottom bevel + mono text).
    pub fn kbd(&mut self, text: &str, rect: &RECT, palette: Palette, dpi: u32) {
        let radius = scaled_f(5.0, dpi);
        let bevel = D2D_RECT_F {
            left: rect.left as f32,
            top: rect.top as f32 + scaled_f(1.0, dpi),
            right: rect.right as f32,
            bottom: rect.bottom as f32 + scaled_f(1.0, dpi),
        };
        self.fill_round_f(bevel, radius, palette.kbd_border, 0.75);
        self.fill_stroke_round(rect, radius, palette.kbd_bg, palette.kbd_border, 1.0);
        self.draw_text(
            text,
            rect,
            TextStyle::Mono,
            dpi,
            palette.secondary,
            AlignH::Center,
            AlignV::Center,
        );
    }

    /// Tactile Windows 11 / iOS pill toggle switch.
    pub fn switch(
        &self,
        rect: &RECT,
        checked: bool,
        hovered: bool,
        focused: bool,
        palette: Palette,
        dpi: u32,
    ) {
        let h = (rect.bottom - rect.top) as f32;
        let radius = h * 0.5;
        let track = if checked {
            if hovered {
                palette.primary_hover
            } else {
                palette.primary
            }
        } else {
            palette.border_strong
        };
        self.fill_round(rect, radius, track);
        if focused {
            let mut ring = *rect;
            let pad = scaled(2, dpi);
            ring.left -= pad;
            ring.top -= pad;
            ring.right += pad;
            ring.bottom += pad;
            self.stroke_round(
                &ring,
                radius + pad as f32,
                if crate::theme::high_contrast_cached() {
                    palette.heading
                } else {
                    palette.primary
                },
                scaled_f(1.5, dpi),
            );
        }
        let pad = scaled_f(3.0, dpi);
        let thumb_r = (h * 0.5 - pad).max(3.0);
        let cx = if checked {
            rect.right as f32 - pad - thumb_r
        } else {
            rect.left as f32 + pad + thumb_r
        };
        let cy = (rect.top + rect.bottom) as f32 * 0.5;
        // Subtle shadow under thumb
        self.fill_circle_alpha(cx, cy + 1.0, thumb_r, COLORREF(0x000000), 0.18);
        let thumb = if checked {
            crate::theme::on_primary(&palette)
        } else if crate::theme::high_contrast_cached() {
            palette.page
        } else {
            COLORREF(0xFFFFFF)
        };
        self.fill_circle(cx, cy, thumb_r, thumb);
    }

    /// Draw a crisp Lucide-style 24×24 vector icon centered at `(cx, cy)` within
    /// `size × size`, rendered with cached Direct2D path geometries (exact Beziers,
    /// arcs, round caps & round joins).
    pub fn icon(&mut self, icon: Icon, cx: f32, cy: f32, size: f32, color: COLORREF) {
        if size <= 1.0 {
            return;
        }
        let Some(brush) = self.brush(color, 1.0) else {
            return;
        };
        let geom = if let Some(cached) = self.engine.icons.get(&icon) {
            cached.clone()
        } else {
            let Some(built) = build_icon_geometry(&self.engine.factory, icon) else {
                return;
            };
            self.engine.icons.insert(icon, built.clone());
            built
        };

        let s = size / 24.0;
        // Exact Lucide 1.9 stroke weight in 24-unit space, clamped to >= 1.2px physical.
        let stroke_24 = 1.9_f32.max(1.2 / s);
        unsafe {
            let mut prev = Matrix3x2::default();
            self.engine.target.GetTransform(&mut prev);
            let combined = Matrix3x2 {
                M11: s,
                M12: 0.0,
                M21: 0.0,
                M22: s,
                M31: cx - 12.0 * s + prev.M31,
                M32: cy - 12.0 * s + prev.M32,
            };
            self.engine.target.SetTransform(&combined);
            self.engine.target.DrawGeometry(
                &geom,
                &brush,
                stroke_24,
                self.engine.round_stroke.as_ref(),
            );
            self.engine.target.SetTransform(&prev);
        }
    }
}

// ---------------------------------------------------------------------------
// Minimalist Lucide 24×24 SVG Path -> Direct2D ID2D1PathGeometry Builder
// ---------------------------------------------------------------------------

struct SvgCursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> SvgCursor<'a> {
    fn new(s: &'a str) -> Self {
        Self {
            bytes: s.as_bytes(),
            pos: 0,
        }
    }

    fn skip_sep(&mut self) {
        while self.pos < self.bytes.len() {
            match self.bytes[self.pos] {
                b' ' | b'\t' | b'\n' | b'\r' | b',' => self.pos += 1,
                _ => break,
            }
        }
    }

    fn peek(&mut self) -> Option<u8> {
        self.skip_sep();
        self.bytes.get(self.pos).copied()
    }

    fn has_number(&mut self) -> bool {
        matches!(self.peek(), Some(b'0'..=b'9' | b'-' | b'+' | b'.'))
    }

    fn next_cmd(&mut self) -> Option<u8> {
        let b = self.peek()?;
        if b.is_ascii_alphabetic() {
            self.pos += 1;
            Some(b)
        } else {
            None
        }
    }

    fn next_flag(&mut self) -> Option<bool> {
        let b = self.peek()?;
        match b {
            b'0' => {
                self.pos += 1;
                Some(false)
            }
            b'1' => {
                self.pos += 1;
                Some(true)
            }
            _ => None,
        }
    }

    fn next_f32(&mut self) -> Option<f32> {
        self.skip_sep();
        let start = self.pos;
        if self.pos < self.bytes.len() && matches!(self.bytes[self.pos], b'+' | b'-') {
            self.pos += 1;
        }
        let mut seen_dot = false;
        let mut seen_digit = false;
        while self.pos < self.bytes.len() {
            match self.bytes[self.pos] {
                b'0'..=b'9' => {
                    seen_digit = true;
                    self.pos += 1;
                }
                b'.' if !seen_dot => {
                    seen_dot = true;
                    self.pos += 1;
                }
                _ => break,
            }
        }
        if !seen_digit {
            self.pos = start;
            return None;
        }
        std::str::from_utf8(&self.bytes[start..self.pos])
            .ok()?
            .parse::<f32>()
            .ok()
    }
}

unsafe fn add_svg_path(sink: &ID2D1GeometrySink, d: &str) {
    let mut cur = SvgCursor::new(d);
    let mut x = 0.0_f32;
    let mut y = 0.0_f32;
    let mut sub_x = 0.0_f32;
    let mut sub_y = 0.0_f32;
    let mut figure_open = false;
    let mut cmd = 0u8;

    loop {
        if let Some(next) = cur.next_cmd() {
            cmd = next;
        } else if cmd == 0 || !cur.has_number() {
            break;
        }

        match cmd {
            b'M' | b'm' => {
                let (Some(nx), Some(ny)) = (cur.next_f32(), cur.next_f32()) else {
                    break;
                };
                if figure_open {
                    sink.EndFigure(D2D1_FIGURE_END_OPEN);
                }
                if cmd == b'm' {
                    x += nx;
                    y += ny;
                } else {
                    x = nx;
                    y = ny;
                }
                sub_x = x;
                sub_y = y;
                sink.BeginFigure(pt(x, y), D2D1_FIGURE_BEGIN_HOLLOW);
                figure_open = true;
                // Per SVG spec, extra coordinate pairs after M/m are implicit L/l.
                cmd = if cmd == b'm' { b'l' } else { b'L' };
            }
            b'L' | b'l' => {
                let (Some(nx), Some(ny)) = (cur.next_f32(), cur.next_f32()) else {
                    break;
                };
                if cmd == b'l' {
                    x += nx;
                    y += ny;
                } else {
                    x = nx;
                    y = ny;
                }
                sink.AddLine(pt(x, y));
            }
            b'H' | b'h' => {
                let Some(nx) = cur.next_f32() else {
                    break;
                };
                if cmd == b'h' {
                    x += nx;
                } else {
                    x = nx;
                }
                sink.AddLine(pt(x, y));
            }
            b'V' | b'v' => {
                let Some(ny) = cur.next_f32() else {
                    break;
                };
                if cmd == b'v' {
                    y += ny;
                } else {
                    y = ny;
                }
                sink.AddLine(pt(x, y));
            }
            b'C' | b'c' => {
                let (Some(x1), Some(y1), Some(x2), Some(y2), Some(nx), Some(ny)) = (
                    cur.next_f32(),
                    cur.next_f32(),
                    cur.next_f32(),
                    cur.next_f32(),
                    cur.next_f32(),
                    cur.next_f32(),
                ) else {
                    break;
                };
                let (p1, p2, p3) = if cmd == b'c' {
                    (pt(x + x1, y + y1), pt(x + x2, y + y2), pt(x + nx, y + ny))
                } else {
                    (pt(x1, y1), pt(x2, y2), pt(nx, ny))
                };
                x = p3.X;
                y = p3.Y;
                sink.AddBezier(&D2D1_BEZIER_SEGMENT {
                    point1: p1,
                    point2: p2,
                    point3: p3,
                });
            }
            b'A' | b'a' => {
                let (
                    Some(rx),
                    Some(ry),
                    Some(rot),
                    Some(large_arc),
                    Some(sweep),
                    Some(nx),
                    Some(ny),
                ) = (
                    cur.next_f32(),
                    cur.next_f32(),
                    cur.next_f32(),
                    cur.next_flag(),
                    cur.next_flag(),
                    cur.next_f32(),
                    cur.next_f32(),
                )
                else {
                    break;
                };
                if cmd == b'a' {
                    x += nx;
                    y += ny;
                } else {
                    x = nx;
                    y = ny;
                }
                sink.AddArc(&D2D1_ARC_SEGMENT {
                    point: pt(x, y),
                    size: D2D_SIZE_F {
                        width: rx.abs(),
                        height: ry.abs(),
                    },
                    rotationAngle: rot,
                    sweepDirection: if sweep {
                        D2D1_SWEEP_DIRECTION_CLOCKWISE
                    } else {
                        D2D1_SWEEP_DIRECTION_COUNTER_CLOCKWISE
                    },
                    arcSize: if large_arc {
                        D2D1_ARC_SIZE_LARGE
                    } else {
                        D2D1_ARC_SIZE_SMALL
                    },
                });
            }
            b'Z' | b'z' => {
                if figure_open {
                    sink.EndFigure(D2D1_FIGURE_END_CLOSED);
                    figure_open = false;
                }
                x = sub_x;
                y = sub_y;
                cmd = 0;
            }
            _ => break,
        }
    }

    if figure_open {
        sink.EndFigure(D2D1_FIGURE_END_OPEN);
    }
}

unsafe fn add_svg_circle(sink: &ID2D1GeometrySink, cx: f32, cy: f32, r: f32) {
    sink.BeginFigure(pt(cx - r, cy), D2D1_FIGURE_BEGIN_HOLLOW);
    let arc_size = D2D_SIZE_F {
        width: r,
        height: r,
    };
    sink.AddArc(&D2D1_ARC_SEGMENT {
        point: pt(cx + r, cy),
        size: arc_size,
        rotationAngle: 0.0,
        sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
        arcSize: D2D1_ARC_SIZE_SMALL,
    });
    sink.AddArc(&D2D1_ARC_SEGMENT {
        point: pt(cx - r, cy),
        size: arc_size,
        rotationAngle: 0.0,
        sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
        arcSize: D2D1_ARC_SIZE_SMALL,
    });
    sink.EndFigure(D2D1_FIGURE_END_CLOSED);
}

unsafe fn add_svg_round_rect(sink: &ID2D1GeometrySink, x: f32, y: f32, w: f32, h: f32, r: f32) {
    let r = r.min(w * 0.5).min(h * 0.5);
    let corner = D2D_SIZE_F {
        width: r,
        height: r,
    };
    sink.BeginFigure(pt(x + r, y), D2D1_FIGURE_BEGIN_HOLLOW);
    sink.AddLine(pt(x + w - r, y));
    sink.AddArc(&D2D1_ARC_SEGMENT {
        point: pt(x + w, y + r),
        size: corner,
        rotationAngle: 0.0,
        sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
        arcSize: D2D1_ARC_SIZE_SMALL,
    });
    sink.AddLine(pt(x + w, y + h - r));
    sink.AddArc(&D2D1_ARC_SEGMENT {
        point: pt(x + w - r, y + h),
        size: corner,
        rotationAngle: 0.0,
        sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
        arcSize: D2D1_ARC_SIZE_SMALL,
    });
    sink.AddLine(pt(x + r, y + h));
    sink.AddArc(&D2D1_ARC_SEGMENT {
        point: pt(x, y + h - r),
        size: corner,
        rotationAngle: 0.0,
        sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
        arcSize: D2D1_ARC_SIZE_SMALL,
    });
    sink.AddLine(pt(x, y + r));
    sink.AddArc(&D2D1_ARC_SEGMENT {
        point: pt(x + r, y),
        size: corner,
        rotationAngle: 0.0,
        sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
        arcSize: D2D1_ARC_SIZE_SMALL,
    });
    sink.EndFigure(D2D1_FIGURE_END_CLOSED);
}

fn build_icon_geometry(factory: &ID2D1Factory, icon: Icon) -> Option<ID2D1PathGeometry> {
    unsafe {
        let geom = factory.CreatePathGeometry().ok()?;
        let sink = geom.Open().ok()?;
        match icon {
            Icon::Bolt => {
                // Lucide: zap
                add_svg_path(&sink, "M13 2L3 14h9l-1 8 10-12h-9l1-8Z");
            }
            Icon::Search => {
                // Lucide: search
                add_svg_circle(&sink, 11.0, 11.0, 7.5);
                add_svg_path(&sink, "M20.5 20.5l-4.1-4.1");
            }
            Icon::Gear => {
                // Lucide: settings (matches demo)
                add_svg_path(
                    &sink,
                    "M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2Z",
                );
                add_svg_circle(&sink, 12.0, 12.0, 3.0);
            }
            Icon::Pin => {
                // Lucide: pin (matches demo)
                add_svg_path(
                    &sink,
                    "M12 17v5 M9 10.76a2 2 0 0 1-1.11 1.79l-1.78.9A2 2 0 0 0 5 15.24V16a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1v-.76a2 2 0 0 0-1.11-1.79l-1.78-.9A2 2 0 0 1 15 10.76V7a1 1 0 0 1 1-1 2 2 0 0 0 0-4H8a2 2 0 0 0 0 4 1 1 0 0 1 1 1Z",
                );
            }
            Icon::Multiline => {
                // Lucide: align-left (matches demo)
                add_svg_path(&sink, "M4 6h16 M4 12h16 M4 18h10");
            }
            Icon::Chat => {
                // Lucide: message-square (matches demo)
                add_svg_path(
                    &sink,
                    "M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2Z",
                );
            }
            Icon::Grid => {
                // Lucide: layout-grid (matches demo)
                add_svg_round_rect(&sink, 3.0, 3.0, 7.0, 7.0, 1.5);
                add_svg_round_rect(&sink, 14.0, 3.0, 7.0, 7.0, 1.5);
                add_svg_round_rect(&sink, 3.0, 14.0, 7.0, 7.0, 1.5);
                add_svg_round_rect(&sink, 14.0, 14.0, 7.0, 7.0, 1.5);
            }
            Icon::Trash => {
                // Lucide: trash-2 (matches demo)
                add_svg_path(
                    &sink,
                    "M3 6h18 M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6 M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2 M10 11v6 M14 11v6",
                );
            }
            Icon::Restore => {
                // Lucide: rotate-ccw (matches demo)
                add_svg_path(
                    &sink,
                    "M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8 M3 3v5h5",
                );
            }
            Icon::Plus => {
                // Lucide: plus (matches demo)
                add_svg_path(&sink, "M12 5v14 M5 12h14");
            }
            Icon::Pause => {
                // Lucide: pause (matches demo)
                add_svg_round_rect(&sink, 6.0, 4.0, 4.0, 16.0, 1.2);
                add_svg_round_rect(&sink, 14.0, 4.0, 4.0, 16.0, 1.2);
            }
            Icon::Play => {
                // Lucide: play
                add_svg_path(&sink, "M6 4l14 8-14 8V4Z");
            }
            Icon::Info => {
                // Lucide: info (matches demo)
                add_svg_circle(&sink, 12.0, 12.0, 9.5);
                add_svg_path(&sink, "M12 16v-4 M12 8h.01");
            }
            Icon::Exit => {
                // Lucide: log-out (matches demo)
                add_svg_path(
                    &sink,
                    "M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4 M16 17l5-5-5-5 M21 12H9",
                );
            }
            Icon::Check => {
                // Lucide: check
                add_svg_path(&sink, "M20 6L9 17l-5-5");
            }
            Icon::Warning => {
                // Lucide: triangle-alert (matches demo)
                add_svg_path(
                    &sink,
                    "M21.73 18l-8-14a2 2 0 0 0-3.48 0l-8 14A2 2 0 0 0 4 21h16a2 2 0 0 0 1.73-3Z M12 9v4 M12 17h.01",
                );
            }
            Icon::Keyboard => {
                // Lucide: keyboard (matches demo)
                add_svg_round_rect(&sink, 2.0, 4.0, 20.0, 16.0, 2.2);
                add_svg_path(
                    &sink,
                    "M6 8h.01 M10 8h.01 M14 8h.01 M18 8h.01 M8 12h.01 M12 12h.01 M16 12h.01 M7 16h10",
                );
            }
            Icon::EditPen => {
                // Lucide: pencil (matches demo)
                add_svg_path(&sink, "M17 3a2.85 2.83 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5Z");
            }
            Icon::Palette => {
                // Lucide: sun (matches demo Appearance tab)
                add_svg_circle(&sink, 12.0, 12.0, 4.0);
                add_svg_path(
                    &sink,
                    "M12 2v2 M12 20v2 M4.93 4.93l1.41 1.41 M17.66 17.66l1.41 1.41 M2 12h2 M20 12h2 M6.34 17.66l-1.41 1.41 M19.07 4.93l-1.41 1.41",
                );
            }
            Icon::Folder => {
                // Lucide: folder (matches demo)
                add_svg_path(
                    &sink,
                    "M4 20h16a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.93a2 2 0 0 1-1.66-.9l-.82-1.2A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z",
                );
            }
            Icon::ChevronDown => {
                // Lucide: chevron-down
                add_svg_path(&sink, "M6 9l6 6 6-6");
            }
            Icon::Close => {
                // Lucide: x
                add_svg_path(&sink, "M18 6L6 18 M6 6l12 12");
            }
            Icon::Lock => {
                // Lucide: lock
                add_svg_round_rect(&sink, 3.0, 11.0, 18.0, 11.0, 2.0);
                add_svg_path(&sink, "M7 11V7a5 5 0 0 1 10 0v4");
            }
            Icon::Clipboard => {
                // Lucide: clipboard
                add_svg_round_rect(&sink, 8.0, 2.0, 8.0, 4.0, 1.0);
                add_svg_path(
                    &sink,
                    "M16 4h2a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2h2",
                );
            }
        }
        sink.Close().ok()?;
        Some(geom)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        build_icon_geometry, create_engine, mono_font_family, paint_buffered, ui_font_family, Icon,
        TextStyle, BUFFER_CREATES, ENGINE,
    };
    use windows::Win32::Foundation::{COLORREF, RECT};
    use windows::Win32::Graphics::Gdi::{
        CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC,
        SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HDC, HGDIOBJ,
    };

    struct TestSurface {
        screen: HDC,
        target: HDC,
        dib: HGDIOBJ,
        old: HGDIOBJ,
    }

    impl Drop for TestSurface {
        fn drop(&mut self) {
            unsafe {
                if !self.old.0.is_null() {
                    let _ = SelectObject(self.target, self.old);
                }
                if !self.dib.0.is_null() {
                    let _ = DeleteObject(self.dib);
                }
                if !self.target.0.is_null() {
                    let _ = DeleteDC(self.target);
                }
                if !self.screen.0.is_null() {
                    ReleaseDC(None, self.screen);
                }
            }
        }
    }

    #[test]
    fn resolved_font_families_are_non_empty() {
        assert!(!ui_font_family().is_empty());
        assert!(!mono_font_family().is_empty());
    }

    #[test]
    fn type_scale_is_hierarchically_ordered() {
        assert!(TextStyle::Display.logical_size() > TextStyle::Title.logical_size());
        assert!(TextStyle::Title.logical_size() > TextStyle::Heading.logical_size());
        assert_eq!(
            TextStyle::Heading.logical_size(),
            TextStyle::Body.logical_size()
        );
        assert!(TextStyle::Body.logical_size() > TextStyle::Label.logical_size());
        assert_eq!(
            TextStyle::Label.logical_size(),
            TextStyle::Caption.logical_size()
        );
        assert!(TextStyle::Caption.logical_size() > TextStyle::Badge.logical_size());
    }

    #[test]
    fn all_lucide_icons_build_cleanly() {
        let engine = create_engine().expect("D2D engine available");
        let all = [
            Icon::Bolt,
            Icon::Search,
            Icon::Gear,
            Icon::Pin,
            Icon::Multiline,
            Icon::Chat,
            Icon::Grid,
            Icon::Trash,
            Icon::Restore,
            Icon::Plus,
            Icon::Pause,
            Icon::Play,
            Icon::Info,
            Icon::Exit,
            Icon::Check,
            Icon::Warning,
            Icon::Keyboard,
            Icon::EditPen,
            Icon::Palette,
            Icon::Folder,
            Icon::ChevronDown,
            Icon::Close,
            Icon::Lock,
            Icon::Clipboard,
        ];
        for icon in all {
            assert!(build_icon_geometry(&engine.factory, icon).is_some());
        }
    }

    #[test]
    fn buffered_paint_reuses_same_size_buffer() {
        let screen = unsafe { GetDC(None) };
        if screen.0.is_null() {
            eprintln!("skip buffered paint test: no screen DC");
            return;
        }
        let target = unsafe { CreateCompatibleDC(Some(screen)) };
        if target.0.is_null() {
            unsafe { ReleaseDC(None, screen) };
            eprintln!("skip buffered paint test: no compatible DC");
            return;
        }
        let mut surface = TestSurface {
            screen,
            target,
            dib: HGDIOBJ::default(),
            old: HGDIOBJ::default(),
        };
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: 64,
                biHeight: -32,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let dib = unsafe {
            CreateDIBSection(Some(target), &bmi, DIB_RGB_COLORS, &mut bits, None, 0)
                .expect("target dib")
        };
        surface.dib = HGDIOBJ(dib.0);
        surface.old = unsafe { SelectObject(target, HGDIOBJ(dib.0)) };

        let rect = RECT {
            left: 0,
            top: 0,
            right: 64,
            bottom: 32,
        };
        let before = BUFFER_CREATES.with(|count| count.get());
        assert!(paint_buffered(target, rect, |canvas| {
            canvas.clear(COLORREF(0xFFFFFF));
            canvas.fill_rect(&rect, COLORREF(0x102030));
        }));
        assert!(paint_buffered(target, rect, |canvas| {
            canvas.clear(COLORREF(0xFFFFFF));
        }));
        assert_eq!(BUFFER_CREATES.with(|count| count.get()), before + 1);

        let small = RECT {
            left: 0,
            top: 0,
            right: 32,
            bottom: 16,
        };
        assert!(paint_buffered(target, small, |canvas| {
            canvas.clear(COLORREF(0));
        }));
        assert_eq!(BUFFER_CREATES.with(|count| count.get()), before + 2);

        // Exercise eviction: 33 distinct sizes push the cache past its bound.
        for size in 1..=33 {
            let rect = RECT {
                left: 0,
                top: 0,
                right: size,
                bottom: size,
            };
            assert!(paint_buffered(target, rect, |canvas| {
                canvas.clear(COLORREF(0));
            }));
        }
        let cached = ENGINE.with(|slot| {
            slot.borrow()
                .as_ref()
                .map(|engine| engine.buffers.len())
                .unwrap_or(0)
        });
        assert!(cached <= 32, "cached={cached}");
    }
}
