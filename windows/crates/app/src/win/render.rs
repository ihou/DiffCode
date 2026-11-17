//! Drawing on Windows: DirectWrite text layouts and a Direct2D painter on the window's render target.
//!
//! Device-independent things (text formats, layouts, path geometries, decoded images) live in `DxBackend` and
//! survive a lost device; brushes and bitmaps belong to the render target and are made again with it.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use diffcode_ui::geom::{Color, Point, Rect, Size};
use diffcode_ui::paint::{
    Backend, Caret, Font, FontKind, FontMetrics, ImageSource, Overflow, PathCmd, Placement, Painter, TextLayout,
    TextRequest, VectorPath,
};
use windows::core::{Interface, BOOL, HSTRING, PCWSTR};
use windows::Win32::Foundation::{D2DERR_RECREATE_TARGET, HWND};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows_numerics::{Matrix3x2, Vector2};

fn v(p: Point) -> Vector2 {
    Vector2 { X: p.x, Y: p.y }
}

fn rf(r: Rect) -> D2D_RECT_F {
    D2D_RECT_F { left: r.x, top: r.y, right: r.right(), bottom: r.bottom() }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn range16(text: &str, r: Range<usize>) -> DWRITE_TEXT_RANGE {
    let a = u16_index(text, r.start);
    let b = u16_index(text, r.end);
    DWRITE_TEXT_RANGE { startPosition: a as u32, length: b.saturating_sub(a) as u32 }
}

fn u16_index(text: &str, byte: usize) -> usize {
    let byte = diffcode_ui::core::text_editing::floor_boundary(text, byte.min(text.len()));
    text[..byte].encode_utf16().count()
}

// MARK: - Text

/// Whether the system has a font family by this name.
fn has_family(collection: Option<&IDWriteFontCollection>, name: &str) -> bool {
    let Some(c) = collection else { return false };
    let mut index = 0;
    let mut exists = BOOL(0);
    unsafe { c.FindFamilyName(&HSTRING::from(name), &mut index, &mut exists).is_ok() && exists.as_bool() }
}

/// The monospace face DiffCode draws code and terminals in unless Settings picks another.
fn default_mono(collection: Option<&IDWriteFontCollection>) -> &'static str {
    ["Cascadia Mono", "Consolas", "Courier New"].into_iter().find(|n| has_family(collection, n)).unwrap_or("Consolas")
}

/// The default monospace face, and the installed families whose regular face is monospaced, by name, for the font
/// settings.
pub fn monospace_families() -> (String, Vec<String>) {
    unsafe {
        let Ok(dwrite) = DWriteCreateFactory::<IDWriteFactory>(DWRITE_FACTORY_TYPE_SHARED) else {
            return ("Consolas".into(), Vec::new());
        };
        let mut collection = None;
        let _ = dwrite.GetSystemFontCollection(&mut collection, true);
        let default = default_mono(collection.as_ref()).to_string();
        let Some(collection) = collection else { return (default, Vec::new()) };
        let mut names = Vec::new();
        for i in 0..collection.GetFontFamilyCount() {
            let Ok(family) = collection.GetFontFamily(i) else { continue };
            let Ok(font) = family.GetFirstMatchingFont(DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL)
            else {
                continue;
            };
            // Symbol faces (Wingdings, the icon fonts) have no letters to draw code in.
            let mono = font.cast::<IDWriteFont1>().is_ok_and(|f| f.IsMonospacedFont().as_bool());
            if !mono || font.IsSymbolFont().as_bool() {
                continue;
            }
            if let Some(name) = family.GetFamilyNames().ok().and_then(|n| english_name(&n)) {
                names.push(name);
            }
        }
        names.sort_by_key(|n| n.to_lowercase());
        names.dedup();
        (default, names)
    }
}

/// A family's English name (what Settings stores and DirectWrite finds), else the first it has.
fn english_name(names: &IDWriteLocalizedStrings) -> Option<String> {
    unsafe {
        let mut index = 0;
        let mut exists = BOOL(0);
        let _ = names.FindLocaleName(&HSTRING::from("en-us"), &mut index, &mut exists);
        if !exists.as_bool() {
            index = 0;
        }
        let len = names.GetStringLength(index).ok()? as usize;
        let mut buf = vec![0u16; len + 1];
        names.GetString(index, &mut buf).ok()?;
        Some(String::from_utf16_lossy(&buf[..len]))
    }
}

/// A decoded image, with the bytes its stream reads from.
type Decoded = (IWICFormatConverter, Arc<Vec<u8>>);

pub struct DxBackend {
    pub d2d: ID2D1Factory,
    dwrite: IDWriteFactory,
    wic: Option<IWICImagingFactory>,
    fallback: Option<IDWriteFontFallback>,
    ui_family: HSTRING,
    /// The monospace face used unless Settings picks another (Cascadia Mono, else Consolas).
    default_mono: HSTRING,
    /// The faces code and terminals are drawn in now, and the names Settings asked for, as last given.
    code_family: RefCell<HSTRING>,
    terminal_family: RefCell<HSTRING>,
    picked: RefCell<(Option<String>, Option<String>)>,
    formats: RefCell<HashMap<Font, IDWriteTextFormat>>,
    metrics: RefCell<HashMap<Font, FontMetrics>>,
    signs: RefCell<HashMap<Font, IDWriteInlineObject>>,
    geometries: RefCell<HashMap<u64, ID2D1PathGeometry>>,
    images: RefCell<HashMap<u64, Option<Decoded>>>,
}

impl DxBackend {
    pub fn new() -> windows::core::Result<Self> {
        unsafe {
            let d2d: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let wic: Option<IWICImagingFactory> =
                CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok();
            let mut collection = None;
            dwrite.GetSystemFontCollection(&mut collection, false)?;
            let ui_family = if has_family(collection.as_ref(), "Segoe UI Variable Text") { "Segoe UI Variable Text" } else { "Segoe UI" };
            let mono = default_mono(collection.as_ref());
            let fallback = Self::make_fallback(&dwrite).ok();
            Ok(DxBackend {
                d2d,
                dwrite,
                wic,
                fallback,
                ui_family: HSTRING::from(ui_family),
                default_mono: HSTRING::from(mono),
                code_family: RefCell::new(HSTRING::from(mono)),
                terminal_family: RefCell::new(HSTRING::from(mono)),
                picked: RefCell::new((None, None)),
                formats: RefCell::new(HashMap::new()),
                metrics: RefCell::new(HashMap::new()),
                signs: RefCell::new(HashMap::new()),
                geometries: RefCell::new(HashMap::new()),
                images: RefCell::new(HashMap::new()),
            })
        }
    }

    /// Han, kana and Hangul in Microsoft YaHei UI, so Chinese never falls into a Japanese face; then the system's.
    fn make_fallback(dwrite: &IDWriteFactory) -> windows::core::Result<IDWriteFontFallback> {
        unsafe {
            let f2: IDWriteFactory2 = dwrite.cast()?;
            let builder = f2.CreateFontFallbackBuilder()?;
            let ranges = [
                DWRITE_UNICODE_RANGE { first: 0x2E80, last: 0x2FDF },
                DWRITE_UNICODE_RANGE { first: 0x3000, last: 0x30FF },
                DWRITE_UNICODE_RANGE { first: 0x3100, last: 0x31FF },
                DWRITE_UNICODE_RANGE { first: 0x3400, last: 0x4DBF },
                DWRITE_UNICODE_RANGE { first: 0x4E00, last: 0x9FFF },
                DWRITE_UNICODE_RANGE { first: 0xF900, last: 0xFAFF },
                DWRITE_UNICODE_RANGE { first: 0xFE30, last: 0xFE4F },
                DWRITE_UNICODE_RANGE { first: 0xFF00, last: 0xFFEF },
                DWRITE_UNICODE_RANGE { first: 0x20000, last: 0x2FA1F },
            ];
            let yahei = wide("Microsoft YaHei UI");
            let names = [yahei.as_ptr()];
            builder.AddMapping(&ranges, &names, None, PCWSTR::null(), PCWSTR::null(), 1.0)?;
            builder.AddMappings(&f2.GetSystemFontFallback()?)?;
            builder.CreateFontFallback()
        }
    }

    /// The family a kind of font is drawn in now.
    fn family(&self, kind: FontKind) -> HSTRING {
        match kind {
            FontKind::Ui => self.ui_family.clone(),
            FontKind::Mono => self.code_family.borrow().clone(),
            FontKind::Terminal => self.terminal_family.borrow().clone(),
        }
    }

    fn format(&self, font: Font) -> IDWriteTextFormat {
        if let Some(f) = self.formats.borrow().get(&font) {
            return f.clone();
        }
        let family = self.family(font.kind);
        let format = unsafe {
            self.dwrite
                .CreateTextFormat(
                    &family,
                    None,
                    DWRITE_FONT_WEIGHT(font.weight.value() as i32),
                    if font.italic { DWRITE_FONT_STYLE_ITALIC } else { DWRITE_FONT_STYLE_NORMAL },
                    DWRITE_FONT_STRETCH_NORMAL,
                    font.size,
                    &HSTRING::from("en-us"),
                )
                .expect("text format")
        };
        unsafe {
            let _ = format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
        }
        self.formats.borrow_mut().insert(font, format.clone());
        format
    }

    fn sign(&self, font: Font) -> Option<IDWriteInlineObject> {
        if let Some(s) = self.signs.borrow().get(&font) {
            return Some(s.clone());
        }
        let s = unsafe { self.dwrite.CreateEllipsisTrimmingSign(&self.format(font)).ok()? };
        self.signs.borrow_mut().insert(font, s.clone());
        Some(s)
    }

    fn line_box(&self, font: Font) -> (f32, f32) {
        let m = self.metrics(font);
        let height = (m.ascent + m.descent + font.size * 0.2).ceil();
        let pad = ((height - m.ascent - m.descent) / 2.0).floor();
        (height, pad + m.ascent)
    }

    fn make(&self, text: &str, req: &TextRequest, max_width: f32, wrap: bool) -> Option<IDWriteTextLayout> {
        let style = &req.style;
        let utf16: Vec<u16> = text.encode_utf16().collect();
        unsafe {
            let layout = self.dwrite.CreateTextLayout(&utf16, &self.format(style.font), max_width, 1.0e6).ok()?;
            let _ = layout.SetWordWrapping(if wrap { DWRITE_WORD_WRAPPING_WRAP } else { DWRITE_WORD_WRAPPING_NO_WRAP });
            let m = self.metrics(style.font);
            let _ = layout.SetIncrementalTabStop((m.char_width * style.tab_width.max(1) as f32).max(1.0));
            let (height, baseline) = self.line_box(style.font);
            let _ = layout.SetLineSpacing(DWRITE_LINE_SPACING_METHOD_UNIFORM, height, baseline);
            if let (Some(fb), Ok(l2)) = (&self.fallback, layout.cast::<IDWriteTextLayout2>()) {
                let _ = l2.SetFontFallback(fb);
            }
            for r in &req.bold_ranges {
                if r.end as usize <= text.len() {
                    let _ = layout.SetFontWeight(DWRITE_FONT_WEIGHT_BOLD, range16(text, r.start as usize..r.end as usize));
                }
            }
            Some(layout)
        }
    }

    /// The path as a Direct2D geometry; kept for paths with an id (icons), made afresh for id 0 (shapes that change).
    fn geometry(&self, path: &VectorPath) -> Option<ID2D1PathGeometry> {
        if path.id != 0 {
            if let Some(g) = self.geometries.borrow().get(&path.id) {
                return Some(g.clone());
            }
        }
        let g = unsafe {
            let g = self.d2d.CreatePathGeometry().ok()?;
            let sink = g.Open().ok()?;
            sink.SetFillMode(if path.even_odd { D2D1_FILL_MODE_ALTERNATE } else { D2D1_FILL_MODE_WINDING });
            let mut open = false;
            let mut current = Point::default();
            for cmd in &path.cmds {
                match *cmd {
                    PathCmd::MoveTo(p) => {
                        if open {
                            sink.EndFigure(D2D1_FIGURE_END_OPEN);
                        }
                        sink.BeginFigure(v(p), D2D1_FIGURE_BEGIN_FILLED);
                        open = true;
                        current = p;
                    }
                    PathCmd::LineTo(p) => {
                        if !open {
                            sink.BeginFigure(v(current), D2D1_FIGURE_BEGIN_FILLED);
                            open = true;
                        }
                        sink.AddLine(v(p));
                        current = p;
                    }
                    PathCmd::QuadTo(c, p) => {
                        if !open {
                            sink.BeginFigure(v(current), D2D1_FIGURE_BEGIN_FILLED);
                            open = true;
                        }
                        sink.AddQuadraticBezier(&D2D1_QUADRATIC_BEZIER_SEGMENT { point1: v(c), point2: v(p) });
                        current = p;
                    }
                    PathCmd::CubicTo(a, b, p) => {
                        if !open {
                            sink.BeginFigure(v(current), D2D1_FIGURE_BEGIN_FILLED);
                            open = true;
                        }
                        sink.AddBezier(&D2D1_BEZIER_SEGMENT { point1: v(a), point2: v(b), point3: v(p) });
                        current = p;
                    }
                    PathCmd::Close => {
                        if open {
                            sink.EndFigure(D2D1_FIGURE_END_CLOSED);
                            open = false;
                        }
                    }
                }
            }
            if open {
                sink.EndFigure(D2D1_FIGURE_END_OPEN);
            }
            sink.Close().ok()?;
            g
        };
        if path.id != 0 {
            self.geometries.borrow_mut().insert(path.id, g.clone());
        }
        Some(g)
    }

    fn decoded(&self, image: &ImageSource) -> Option<IWICFormatConverter> {
        if let Some(d) = self.images.borrow().get(&image.id) {
            return d.as_ref().map(|(c, _)| c.clone());
        }
        let made = (|| unsafe {
            let wic = self.wic.as_ref()?;
            let stream = wic.CreateStream().ok()?;
            // The stream reads the bytes in place; the cache keeps them alive with it.
            stream.InitializeFromMemory(&image.bytes).ok()?;
            let decoder = wic.CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnDemand).ok()?;
            let frame = decoder.GetFrame(0).ok()?;
            let converter = wic.CreateFormatConverter().ok()?;
            converter
                .Initialize(&frame, &GUID_WICPixelFormat32bppPBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeMedianCut)
                .ok()?;
            Some((converter, image.bytes.clone()))
        })();
        let converter = made.as_ref().map(|(c, _)| c.clone());
        self.images.borrow_mut().insert(image.id, made);
        converter
    }
}

pub struct DwLayout {
    layout: IDWriteTextLayout,
    /// The text as laid out (differs from the request's when cut in the middle).
    text: String,
    /// Positions do not map back to the request's text.
    shortened: bool,
    size: Size,
    baseline: f32,
    lines: usize,
    truncated: bool,
    /// UTF-16 index → byte offset, up to and including the end.
    map: Vec<u32>,
    runs: Vec<(DWRITE_TEXT_RANGE, Color)>,
    /// The render target generation the runs' brushes were set for.
    applied: Cell<u64>,
}

impl DwLayout {
    fn byte(&self, i16: usize) -> usize {
        if self.shortened {
            return 0;
        }
        self.map.get(i16).copied().unwrap_or(self.text.len() as u32) as usize
    }
}

impl TextLayout for DwLayout {
    fn size(&self) -> Size {
        self.size
    }

    fn baseline(&self) -> f32 {
        self.baseline
    }

    fn line_count(&self) -> usize {
        self.lines
    }

    fn is_truncated(&self) -> bool {
        self.truncated
    }

    fn hit_test(&self, p: Point) -> usize {
        let mut trailing = BOOL(0);
        let mut inside = BOOL(0);
        let mut m = DWRITE_HIT_TEST_METRICS::default();
        unsafe {
            if self.layout.HitTestPoint(p.x, p.y, &mut trailing, &mut inside, &mut m).is_err() {
                return 0;
            }
        }
        let mut i = m.textPosition as usize;
        if trailing.as_bool() {
            i += m.length as usize;
        }
        self.byte(i)
    }

    fn caret(&self, byte: usize) -> Caret {
        let i = if self.shortened { 0 } else { u16_index(&self.text, byte) };
        let mut x = 0.0;
        let mut y = 0.0;
        let mut m = DWRITE_HIT_TEST_METRICS::default();
        unsafe {
            let _ = self.layout.HitTestTextPosition(i as u32, false, &mut x, &mut y, &mut m);
        }
        Caret { x, top: m.top, height: if m.height > 0.0 { m.height } else { self.size.h } }
    }

    fn range_rects(&self, range: Range<usize>) -> Vec<Rect> {
        if self.shortened || range.start >= range.end {
            return Vec::new();
        }
        let r = range16(&self.text, range);
        let mut count = 0u32;
        unsafe {
            let _ = self.layout.HitTestTextRange(r.startPosition, r.length, 0.0, 0.0, None, &mut count);
            if count == 0 {
                return Vec::new();
            }
            let mut out = vec![DWRITE_HIT_TEST_METRICS::default(); count as usize];
            if self.layout.HitTestTextRange(r.startPosition, r.length, 0.0, 0.0, Some(&mut out), &mut count).is_err() {
                return Vec::new();
            }
            out.truncate(count as usize);
            out.iter().map(|m| Rect::new(m.left, m.top, m.width, m.height)).collect()
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl Backend for DxBackend {
    fn set_families(&self, code: Option<&str>, terminal: Option<&str>) -> bool {
        {
            let picked = self.picked.borrow();
            if picked.0.as_deref() == code && picked.1.as_deref() == terminal {
                return false;
            }
        }
        *self.picked.borrow_mut() = (code.map(String::from), terminal.map(String::from));
        // Looked up afresh, so a font installed since DiffCode started counts.
        let collection = unsafe {
            let mut c = None;
            self.dwrite.GetSystemFontCollection(&mut c, true).ok().and(c)
        };
        let resolve = |name: Option<&str>| match name {
            Some(n) if has_family(collection.as_ref(), n) => HSTRING::from(n),
            _ => self.default_mono.clone(),
        };
        let (code, terminal) = (resolve(code), resolve(terminal));
        if *self.code_family.borrow() == code && *self.terminal_family.borrow() == terminal {
            return false;
        }
        *self.code_family.borrow_mut() = code;
        *self.terminal_family.borrow_mut() = terminal;
        self.formats.borrow_mut().retain(|f, _| f.kind == FontKind::Ui);
        self.metrics.borrow_mut().retain(|f, _| f.kind == FontKind::Ui);
        self.signs.borrow_mut().retain(|f, _| f.kind == FontKind::Ui);
        true
    }

    fn layout(&self, req: &TextRequest) -> Rc<dyn TextLayout> {
        let style = &req.style;
        let original: &str = &req.text;
        let (wrap, max) = match (style.overflow, style.max_width) {
            (Overflow::Wrap, Some(w)) => (true, w.max(1.0)),
            _ => (false, 1.0e6),
        };
        let mut text = original.to_string();
        let mut shortened = false;
        let mut truncated = false;
        let mut layout = self.make(&text, req, max, wrap).expect("text layout");
        let width = |l: &IDWriteTextLayout| unsafe {
            let mut m = DWRITE_TEXT_METRICS::default();
            let _ = l.GetMetrics(&mut m);
            m.widthIncludingTrailingWhitespace
        };
        if let (Overflow::Ellipsis | Overflow::EllipsisMiddle, Some(limit)) = (style.overflow, style.max_width) {
            let full = width(&layout);
            if full > limit + 0.5 {
                truncated = true;
                if style.overflow == Overflow::Ellipsis {
                    unsafe {
                        let _ = layout.SetMaxWidth(limit.max(1.0));
                        let trimming = DWRITE_TRIMMING { granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER, delimiter: 0, delimiterCount: 0 };
                        let _ = layout.SetTrimming(&trimming, self.sign(style.font).as_ref());
                    }
                } else {
                    // DirectWrite only trims at the end: keep as much of both ends as fits around "…".
                    let ell = {
                        let probe = self.make("…", req, 1.0e6, false).expect("text layout");
                        width(&probe)
                    };
                    let budget = (limit - ell).max(0.0);
                    let x_at = |byte: usize| unsafe {
                        let mut x = 0.0;
                        let mut y = 0.0;
                        let mut m = DWRITE_HIT_TEST_METRICS::default();
                        let _ = layout.HitTestTextPosition(u16_index(original, byte) as u32, false, &mut x, &mut y, &mut m);
                        x
                    };
                    let bounds: Vec<usize> = original.char_indices().map(|(i, _)| i).chain(std::iter::once(original.len())).collect();
                    let xs: Vec<f32> = bounds.iter().map(|&b| x_at(b)).collect();
                    let total = *xs.last().unwrap_or(&full);
                    // Grow the head and tail alternately while they fit.
                    let (mut head, mut tail) = (0usize, bounds.len() - 1);
                    loop {
                        let grow_head = head < tail && xs[head + 1] + (total - xs[tail]) <= budget;
                        let grow_tail = tail > head && xs[head] + (total - xs[tail - 1]) <= budget;
                        let head_w = xs[head];
                        let tail_w = total - xs[tail];
                        if grow_head && (head_w <= tail_w || !grow_tail) {
                            head += 1;
                        } else if grow_tail {
                            tail -= 1;
                        } else {
                            break;
                        }
                        if head >= tail {
                            break;
                        }
                    }
                    text = format!("{}…{}", &original[..bounds[head]], &original[bounds[tail]..]);
                    shortened = true;
                    layout = self.make(&text, req, 1.0e6, false).expect("text layout");
                }
            }
        }
        let mut runs = Vec::new();
        if !shortened {
            for run in &req.runs {
                if run.range.start < run.range.end && run.range.end as usize <= text.len() {
                    runs.push((range16(&text, run.range.start as usize..run.range.end as usize), run.color));
                }
            }
        }
        let (size, lines, baseline) = unsafe {
            let mut m = DWRITE_TEXT_METRICS::default();
            let _ = layout.GetMetrics(&mut m);
            let mut count = 0u32;
            let _ = layout.GetLineMetrics(None, &mut count);
            let mut lm = vec![DWRITE_LINE_METRICS::default(); count.max(1) as usize];
            let _ = layout.GetLineMetrics(Some(&mut lm), &mut count);
            let baseline = lm.first().map_or(0.0, |l| l.baseline);
            let w = if truncated && style.overflow == Overflow::Ellipsis {
                m.widthIncludingTrailingWhitespace.min(style.max_width.unwrap_or(f32::MAX))
            } else {
                m.widthIncludingTrailingWhitespace
            };
            (Size::new(w.ceil(), m.height), count as usize, baseline)
        };
        let mut map = Vec::with_capacity(text.len() + 1);
        for (i, ch) in text.char_indices() {
            for _ in 0..ch.len_utf16() {
                map.push(i as u32);
            }
        }
        map.push(text.len() as u32);
        Rc::new(DwLayout { layout, text, shortened, size, baseline, lines, truncated, map, runs, applied: Cell::new(u64::MAX) })
    }

    fn metrics(&self, font: Font) -> FontMetrics {
        if let Some(m) = self.metrics.borrow().get(&font) {
            return *m;
        }
        let m = unsafe {
            let format = self.format(font);
            let mut ascent = font.size * 0.92;
            let mut descent = font.size * 0.23;
            if let Ok(collection) = format.GetFontCollection() {
                let family = self.family(font.kind);
                let mut index = 0;
                let mut exists = BOOL(0);
                if collection.FindFamilyName(&family, &mut index, &mut exists).is_ok() && exists.as_bool() {
                    if let Ok(f) = collection.GetFontFamily(index).and_then(|fam| {
                        fam.GetFirstMatchingFont(DWRITE_FONT_WEIGHT(font.weight.value() as i32), DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL)
                    }) {
                        let mut fm = DWRITE_FONT_METRICS::default();
                        f.GetMetrics(&mut fm);
                        let em = fm.designUnitsPerEm.max(1) as f32;
                        ascent = fm.ascent as f32 / em * font.size;
                        descent = fm.descent as f32 / em * font.size;
                    }
                }
            }
            let zero: Vec<u16> = "0".encode_utf16().collect();
            let char_width = self
                .dwrite
                .CreateTextLayout(&zero, &format, 1.0e6, 1.0e6)
                .ok()
                .map(|l| {
                    let mut m = DWRITE_TEXT_METRICS::default();
                    let _ = l.GetMetrics(&mut m);
                    m.widthIncludingTrailingWhitespace
                })
                .unwrap_or(font.size * 0.6);
            FontMetrics { ascent, descent, char_width }
        };
        self.metrics.borrow_mut().insert(font, m);
        m
    }

    fn image_size(&self, image: &ImageSource) -> Option<Size> {
        let c = self.decoded(image)?;
        let (mut w, mut h) = (0u32, 0u32);
        unsafe { c.GetSize(&mut w, &mut h).ok()? };
        Some(Size::new(w as f32, h as f32))
    }
}

// MARK: - Painting

/// Draws on a window through a Direct2D render target, made (again) as needed.
pub struct DxPainter {
    backend: Rc<DxBackend>,
    hwnd: HWND,
    target: Option<ID2D1HwndRenderTarget>,
    generation: u64,
    brushes: HashMap<u32, ID2D1SolidColorBrush>,
    bitmaps: HashMap<u64, ID2D1Bitmap>,
    round: Option<ID2D1StrokeStyle>,
    dashes: HashMap<u32, ID2D1StrokeStyle>,
    scale: f32,
    drawing: bool,
}

impl DxPainter {
    pub fn new(backend: Rc<DxBackend>, hwnd: HWND) -> Self {
        let round = unsafe {
            let props = D2D1_STROKE_STYLE_PROPERTIES {
                startCap: D2D1_CAP_STYLE_ROUND,
                endCap: D2D1_CAP_STYLE_ROUND,
                dashCap: D2D1_CAP_STYLE_FLAT,
                lineJoin: D2D1_LINE_JOIN_ROUND,
                miterLimit: 10.0,
                dashStyle: D2D1_DASH_STYLE_SOLID,
                dashOffset: 0.0,
            };
            backend.d2d.CreateStrokeStyle(&props, None).ok()
        };
        DxPainter {
            backend,
            hwnd,
            target: None,
            generation: 0,
            brushes: HashMap::new(),
            bitmaps: HashMap::new(),
            round,
            dashes: HashMap::new(),
            scale: 1.0,
            drawing: false,
        }
    }

    /// Starts a frame of `pixels` at `dpi`. False when there is nothing to draw on.
    pub fn begin(&mut self, pixels: (u32, u32), dpi: f32, clear: Color) -> bool {
        self.scale = dpi / 96.0;
        unsafe {
            if self.target.is_none() {
                let props = D2D1_RENDER_TARGET_PROPERTIES {
                    r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
                    pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_IGNORE },
                    dpiX: dpi,
                    dpiY: dpi,
                    usage: D2D1_RENDER_TARGET_USAGE_NONE,
                    minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
                };
                let hwnd_props = D2D1_HWND_RENDER_TARGET_PROPERTIES {
                    hwnd: self.hwnd,
                    pixelSize: D2D_SIZE_U { width: pixels.0.max(1), height: pixels.1.max(1) },
                    presentOptions: D2D1_PRESENT_OPTIONS_IMMEDIATELY,
                };
                match self.backend.d2d.CreateHwndRenderTarget(&props, &hwnd_props) {
                    Ok(t) => self.target = Some(t),
                    Err(_) => return false,
                }
            }
            let t = self.target.as_ref().unwrap();
            let size = t.GetPixelSize();
            if size.width != pixels.0.max(1) || size.height != pixels.1.max(1) {
                let _ = t.Resize(&D2D_SIZE_U { width: pixels.0.max(1), height: pixels.1.max(1) });
            }
            t.SetDpi(dpi, dpi);
            t.BeginDraw();
            t.SetTransform(&Matrix3x2::identity());
            t.Clear(Some(&D2D1_COLOR_F { r: clear.r, g: clear.g, b: clear.b, a: 1.0 }));
        }
        self.drawing = true;
        true
    }

    pub fn end(&mut self) {
        if !self.drawing {
            return;
        }
        self.drawing = false;
        if let Some(t) = &self.target {
            let r = unsafe { t.EndDraw(None, None) };
            if let Err(e) = r {
                if e.code() == D2DERR_RECREATE_TARGET {
                    self.target = None;
                    self.generation += 1;
                    self.brushes.clear();
                    self.bitmaps.clear();
                }
            }
        }
    }

    /// Lets go of the render target (the window is going away).
    pub fn release(&mut self) {
        self.target = None;
        self.brushes.clear();
        self.bitmaps.clear();
        self.generation += 1;
    }

    fn brush(&mut self, c: Color) -> Option<ID2D1SolidColorBrush> {
        let key = c.to_rgba8();
        let key = u32::from_be_bytes(key);
        if let Some(b) = self.brushes.get(&key) {
            return Some(b.clone());
        }
        let t = self.target.as_ref()?;
        let b = unsafe { t.CreateSolidColorBrush(&D2D1_COLOR_F { r: c.r, g: c.g, b: c.b, a: c.a }, None).ok()? };
        if self.brushes.len() > 512 {
            self.brushes.clear();
        }
        self.brushes.insert(key, b.clone());
        Some(b)
    }

    fn rt(&self) -> Option<&ID2D1HwndRenderTarget> {
        if self.drawing {
            self.target.as_ref()
        } else {
            None
        }
    }

    fn dash(&mut self, dash: f32) -> Option<ID2D1StrokeStyle> {
        let key = (dash * 100.0) as u32;
        if let Some(s) = self.dashes.get(&key) {
            return Some(s.clone());
        }
        let s = unsafe {
            let props = D2D1_STROKE_STYLE_PROPERTIES {
                startCap: D2D1_CAP_STYLE_FLAT,
                endCap: D2D1_CAP_STYLE_FLAT,
                dashCap: D2D1_CAP_STYLE_FLAT,
                lineJoin: D2D1_LINE_JOIN_MITER,
                miterLimit: 10.0,
                dashStyle: D2D1_DASH_STYLE_CUSTOM,
                dashOffset: 0.0,
            };
            // Dash lengths are in stroke widths; the caller's are in DIPs at width 1.
            self.backend.d2d.CreateStrokeStyle(&props, Some(&[dash, dash])).ok()?
        };
        self.dashes.insert(key, s.clone());
        Some(s)
    }

    fn rounded(r: Rect, radius: f32) -> D2D1_ROUNDED_RECT {
        let radius = radius.min(r.w / 2.0).min(r.h / 2.0).max(0.0);
        D2D1_ROUNDED_RECT { rect: rf(r), radiusX: radius, radiusY: radius }
    }

    fn with_transform(&self, at: Placement, f: impl FnOnce(&ID2D1HwndRenderTarget)) {
        let Some(t) = self.rt() else { return };
        unsafe {
            t.SetTransform(&Matrix3x2 { M11: at.scale, M12: 0.0, M21: 0.0, M22: at.scale, M31: at.offset.x, M32: at.offset.y });
            f(t);
            t.SetTransform(&Matrix3x2::identity());
        }
    }
}

impl Painter for DxPainter {
    fn scale(&self) -> f32 {
        self.scale
    }

    fn fill_rect(&mut self, r: Rect, color: Color) {
        let Some(b) = self.brush(color) else { return };
        if let Some(t) = self.rt() {
            unsafe { t.FillRectangle(&rf(r), &b) }
        }
    }

    fn fill_round_rect(&mut self, r: Rect, radius: f32, color: Color) {
        let Some(b) = self.brush(color) else { return };
        if let Some(t) = self.rt() {
            unsafe { t.FillRoundedRectangle(&Self::rounded(r, radius), &b) }
        }
    }

    fn stroke_rect(&mut self, r: Rect, width: f32, color: Color) {
        let Some(b) = self.brush(color) else { return };
        if let Some(t) = self.rt() {
            unsafe { t.DrawRectangle(&rf(r), &b, width, None::<&ID2D1StrokeStyle>) }
        }
    }

    fn stroke_round_rect(&mut self, r: Rect, radius: f32, width: f32, color: Color) {
        let Some(b) = self.brush(color) else { return };
        if let Some(t) = self.rt() {
            unsafe { t.DrawRoundedRectangle(&Self::rounded(r, radius), &b, width, None::<&ID2D1StrokeStyle>) }
        }
    }

    fn line(&mut self, a: Point, b: Point, width: f32, color: Color) {
        let Some(br) = self.brush(color) else { return };
        if let Some(t) = self.rt() {
            unsafe { t.DrawLine(v(a), v(b), &br, width, self.round.as_ref()) }
        }
    }

    fn dashed_line(&mut self, a: Point, b: Point, width: f32, dash: f32, color: Color) {
        let Some(br) = self.brush(color) else { return };
        let style = self.dash(dash / width.max(0.01));
        if let Some(t) = self.rt() {
            unsafe { t.DrawLine(v(a), v(b), &br, width, style.as_ref()) }
        }
    }

    fn fill_path(&mut self, path: &VectorPath, at: Placement, color: Color) {
        let Some(b) = self.brush(color) else { return };
        let Some(g) = self.backend.geometry(path) else { return };
        self.with_transform(at, |t| unsafe { t.FillGeometry(&g, &b, None::<&ID2D1Brush>) });
    }

    fn stroke_path(&mut self, path: &VectorPath, at: Placement, width: f32, color: Color) {
        let Some(b) = self.brush(color) else { return };
        let Some(g) = self.backend.geometry(path) else { return };
        let round = self.round.clone();
        let w = width / at.scale.max(0.001);
        self.with_transform(at, |t| unsafe { t.DrawGeometry(&g, &b, w, round.as_ref()) });
    }

    fn fill_ellipse(&mut self, c: Point, rx: f32, ry: f32, color: Color) {
        let Some(b) = self.brush(color) else { return };
        if let Some(t) = self.rt() {
            unsafe { t.FillEllipse(&D2D1_ELLIPSE { point: v(c), radiusX: rx, radiusY: ry }, &b) }
        }
    }

    fn stroke_ellipse(&mut self, c: Point, rx: f32, ry: f32, width: f32, color: Color) {
        let Some(b) = self.brush(color) else { return };
        if let Some(t) = self.rt() {
            unsafe { t.DrawEllipse(&D2D1_ELLIPSE { point: v(c), radiusX: rx, radiusY: ry }, &b, width, None::<&ID2D1StrokeStyle>) }
        }
    }

    fn text(&mut self, layout: &dyn TextLayout, origin: Point, color: Color) {
        let Some(l) = layout.as_any().downcast_ref::<DwLayout>() else { return };
        let Some(b) = self.brush(color) else { return };
        if l.applied.get() != self.generation {
            for (range, c) in &l.runs {
                if let Some(rb) = self.brush(*c) {
                    unsafe {
                        let _ = l.layout.SetDrawingEffect(&rb, *range);
                    }
                }
            }
            l.applied.set(self.generation);
        }
        if let Some(t) = self.rt() {
            unsafe { t.DrawTextLayout(v(origin), &l.layout, &b, D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT) }
        }
    }

    fn image(&mut self, image: &ImageSource, rect: Rect) {
        let bitmap = match self.bitmaps.get(&image.id) {
            Some(b) => b.clone(),
            None => {
                let Some(c) = self.backend.decoded(image) else { return };
                let Some(t) = self.target.as_ref() else { return };
                let Ok(b) = (unsafe { t.CreateBitmapFromWicBitmap(&c, None) }) else { return };
                if self.bitmaps.len() > 16 {
                    self.bitmaps.clear();
                }
                self.bitmaps.insert(image.id, b.clone());
                b
            }
        };
        if let Some(t) = self.rt() {
            unsafe { t.DrawBitmap(&bitmap, Some(&rf(rect)), 1.0, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None) }
        }
    }

    fn shadow(&mut self, r: Rect, radius: f32, blur: f32, color: Color) {
        // A soft edge from a few widening, fading rounded rects, a little below the shape.
        let steps = 6;
        let base = r.offset(0.0, blur / 4.0);
        for i in (1..=steps).rev() {
            let t = i as f32 / steps as f32;
            let grow = blur * 0.5 * t;
            let alpha = color.a * (1.0 - t).powf(1.5) * 0.45 + color.a * 0.02;
            self.fill_round_rect(base.inset(-grow), radius + grow, color.with_alpha(alpha));
        }
    }

    fn push_clip(&mut self, r: Rect) {
        if let Some(t) = self.rt() {
            unsafe { t.PushAxisAlignedClip(&rf(r), D2D1_ANTIALIAS_MODE_ALIASED) }
        }
    }

    fn pop_clip(&mut self) {
        if let Some(t) = self.rt() {
            unsafe { t.PopAxisAlignedClip() }
        }
    }
}
