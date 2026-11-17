//! The Mac test harness backend: draws frames with CoreGraphics and CoreText into a bitmap and writes PNGs, so the
//! interface can be checked without a Windows machine. It never ships.

mod ffi;

use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::ops::Range;
use std::path::Path;
use std::rc::Rc;

use diffcode_ui::geom::{Color, Point, Rect, Size};
use diffcode_ui::paint::{
    Backend, Caret, Font, FontKind, FontMetrics, ImageSource, Overflow, PathCmd, Placement, Painter, TextLayout,
    TextRequest, VectorPath, Weight,
};
use ffi::*;

// MARK: - Text

pub struct MacBackend {
    fonts: RefCell<HashMap<Font, Cf>>,
    images: RefCell<HashMap<u64, Option<Cf>>>,
    /// The families picked for code and for terminals.
    families: RefCell<(Option<String>, Option<String>)>,
}

/// The font of an installed family at `size`; None when no family has that name.
fn family_font(name: &str, size: f64) -> Option<Cf> {
    unsafe {
        let f = Cf(CTFontCreateWithName(cfstr(name).ptr(), size, std::ptr::null()));
        let family = Cf(CTFontCopyFamilyName(f.ptr()));
        (cf_string(family.ptr()) == name).then_some(f)
    }
}

/// The harness's stand-in for Cascadia Mono.
fn default_mono_name() -> &'static str {
    if family_font("JetBrains Mono", 12.0).is_some() {
        "JetBrains Mono"
    } else {
        "Menlo"
    }
}

/// The default monospace face and the installed monospace families, as the Windows shell reports them.
pub fn monospace_families() -> (String, Vec<String>) {
    let mut list = Vec::new();
    unsafe {
        let names = Cf(CTFontManagerCopyAvailableFontFamilyNames());
        for i in 0..CFArrayGetCount(names.ptr()) {
            let name = cf_string(CFArrayGetValueAtIndex(names.ptr(), i));
            // kCTFontTraitMonoSpace
            if !name.starts_with('.') && family_font(&name, 12.0).is_some_and(|f| CTFontGetSymbolicTraits(f.ptr()) & (1 << 10) != 0) {
                list.push(name);
            }
        }
    }
    list.sort_by_key(|n| n.to_lowercase());
    (default_mono_name().to_string(), list)
}

impl MacBackend {
    pub fn new() -> Self {
        MacBackend { fonts: RefCell::new(HashMap::new()), images: RefCell::new(HashMap::new()), families: RefCell::new((None, None)) }
    }

    fn font(&self, font: Font) -> Cf {
        if let Some(f) = self.fonts.borrow().get(&font) {
            return f.clone();
        }
        let size = font.size as f64;
        let base = unsafe {
            match font.kind {
                FontKind::Ui => Cf(CTFontCreateUIFontForLanguage(kCTFontUIFontSystem, size, std::ptr::null())),
                FontKind::Mono | FontKind::Terminal => {
                    let families = self.families.borrow();
                    let picked = if font.kind == FontKind::Mono { &families.0 } else { &families.1 };
                    picked
                        .as_deref()
                        .and_then(|name| family_font(name, size))
                        .unwrap_or_else(|| Cf(CTFontCreateWithName(cfstr(default_mono_name()).ptr(), size, std::ptr::null())))
                }
            }
        };
        let mut traits = 0u32;
        if font.weight >= Weight::Semibold {
            traits |= 1 << 1;
        }
        if font.italic {
            traits |= 1 << 0;
        }
        let f = if traits != 0 {
            let copy = unsafe { CTFontCreateCopyWithSymbolicTraits(base.ptr(), size, std::ptr::null(), traits, traits) };
            if copy.is_null() {
                base
            } else {
                Cf(copy)
            }
        } else {
            base
        };
        self.fonts.borrow_mut().insert(font, f.clone());
        f
    }

    fn image(&self, image: &ImageSource) -> Option<Cf> {
        if let Some(i) = self.images.borrow().get(&image.id) {
            return i.clone();
        }
        let made = unsafe {
            let data = Cf(CFDataCreate(std::ptr::null(), image.bytes.as_ptr(), image.bytes.len() as CFIndex));
            let src = CGImageSourceCreateWithData(data.ptr(), std::ptr::null());
            if src.is_null() {
                None
            } else {
                let src = Cf(src);
                let img = CGImageSourceCreateImageAtIndex(src.ptr(), 0, std::ptr::null());
                (!img.is_null()).then(|| Cf(img))
            }
        };
        self.images.borrow_mut().insert(image.id, made.clone());
        made
    }
}

impl Default for MacBackend {
    fn default() -> Self {
        Self::new()
    }
}

fn cf_string(s: CFTypeRef) -> String {
    // Enough for font family names: ASCII through the description.
    extern "C" {
        fn CFStringGetCString(s: CFTypeRef, buf: *mut u8, size: CFIndex, enc: u32) -> bool;
    }
    let mut buf = [0u8; 256];
    unsafe {
        if CFStringGetCString(s, buf.as_mut_ptr(), 256, kCFStringEncodingUTF8) {
            let end = buf.iter().position(|&b| b == 0).unwrap_or(0);
            return String::from_utf8_lossy(&buf[..end]).into_owned();
        }
    }
    String::new()
}

fn cgcolor(c: Color) -> Cf {
    Cf(unsafe { CGColorCreateSRGB(c.r as f64, c.g as f64, c.b as f64, c.a as f64) })
}

/// UTF-16 index → byte offset, for every index up to and including the end.
fn utf16_map(text: &str) -> Vec<u32> {
    let mut map = Vec::with_capacity(text.len() + 1);
    for (i, ch) in text.char_indices() {
        for _ in 0..ch.len_utf16() {
            map.push(i as u32);
        }
    }
    map.push(text.len() as u32);
    map
}

fn byte_to_u16(text: &str, byte: usize) -> usize {
    let byte = byte.min(text.len());
    text[..diffcode_core::text_editing::floor_boundary(text, byte)].encode_utf16().count()
}

struct CtLine {
    line: Cf,
    top: f32,
    ascent: f32,
    height: f32,
    width: f32,
    /// UTF-16 range of the source string.
    range: Range<usize>,
}

pub struct CtLayout {
    text: String,
    lines: Vec<CtLine>,
    size: Size,
    truncated: bool,
    map: Vec<u32>,
}

impl TextLayout for CtLayout {
    fn size(&self) -> Size {
        self.size
    }

    fn baseline(&self) -> f32 {
        self.lines.first().map_or(0.0, |l| l.top + l.ascent)
    }

    fn line_count(&self) -> usize {
        self.lines.len()
    }

    fn is_truncated(&self) -> bool {
        self.truncated
    }

    fn hit_test(&self, p: Point) -> usize {
        let Some(line) = self.lines.iter().find(|l| p.y < l.top + l.height).or(self.lines.last()) else { return 0 };
        let i = unsafe { CTLineGetStringIndexForPosition(line.line.ptr(), CGPoint { x: p.x as f64, y: 0.0 }) };
        let i = if i < 0 { line.range.end } else { (i as usize).clamp(line.range.start, line.range.end) };
        // A wrapped line's end is the next line's start; stay on this line unless it is the last.
        let i = if i == line.range.end && line.range.end > line.range.start && !std::ptr::eq(line, self.lines.last().unwrap()) {
            // Before a trailing space or line break the caret stays on the line.
            i.saturating_sub(1).max(line.range.start)
        } else {
            i
        };
        self.map.get(i).copied().unwrap_or(self.text.len() as u32) as usize
    }

    fn caret(&self, byte: usize) -> Caret {
        let u = byte_to_u16(&self.text, byte);
        let line = self
            .lines
            .iter()
            .find(|l| u >= l.range.start && u < l.range.end)
            .or_else(|| self.lines.iter().rev().find(|l| u >= l.range.start))
            .or(self.lines.first());
        match line {
            Some(l) => {
                let x = unsafe { CTLineGetOffsetForStringIndex(l.line.ptr(), u as CFIndex, std::ptr::null_mut()) };
                Caret { x: x as f32, top: l.top, height: l.height }
            }
            None => Caret { x: 0.0, top: 0.0, height: self.size.h },
        }
    }

    fn range_rects(&self, range: Range<usize>) -> Vec<Rect> {
        let a = byte_to_u16(&self.text, range.start);
        let b = byte_to_u16(&self.text, range.end);
        let mut out = Vec::new();
        for l in &self.lines {
            let s = a.max(l.range.start);
            let e = b.min(l.range.end);
            if s >= e && !(a <= l.range.start && b > l.range.end) {
                continue;
            }
            let x0 = unsafe { CTLineGetOffsetForStringIndex(l.line.ptr(), s as CFIndex, std::ptr::null_mut()) } as f32;
            let x1 = unsafe { CTLineGetOffsetForStringIndex(l.line.ptr(), e as CFIndex, std::ptr::null_mut()) } as f32;
            out.push(Rect::new(x0, l.top, (x1 - x0).max(0.0), l.height));
        }
        out
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl Backend for MacBackend {
    fn set_families(&self, code: Option<&str>, terminal: Option<&str>) -> bool {
        let picked = (code.map(String::from), terminal.map(String::from));
        if *self.families.borrow() == picked {
            return false;
        }
        *self.families.borrow_mut() = picked;
        self.fonts.borrow_mut().retain(|f, _| f.kind == FontKind::Ui);
        true
    }

    fn layout(&self, req: &TextRequest) -> Rc<dyn TextLayout> {
        let text: &str = &req.text;
        let style = &req.style;
        let font = self.font(style.font);
        let metrics = self.metrics(style.font);
        unsafe {
            let s = Cf(CFAttributedStringCreateMutable(std::ptr::null(), 0));
            let cs = cfstr(text);
            let len16 = CFStringGetLength(cs.ptr());
            CFAttributedStringReplaceString(s.ptr(), CFRange { location: 0, length: 0 }, cs.ptr());
            CFAttributedStringBeginEditing(s.ptr());
            let all = CFRange { location: 0, length: len16 };
            CFAttributedStringSetAttribute(s.ptr(), all, kCTFontAttributeName, font.ptr());
            // Tabs every `tab_width` cells.
            let interval: f64 = metrics.char_width as f64 * style.tab_width as f64;
            let empty = Cf(CFArrayCreate(std::ptr::null(), std::ptr::null(), 0, &kCFTypeArrayCallBacks));
            let stops = empty.ptr();
            let settings = [
                CTParagraphStyleSetting {
                    spec: kCTParagraphStyleSpecifierDefaultTabInterval,
                    value_size: std::mem::size_of::<f64>(),
                    value: &interval as *const f64 as *const c_void,
                },
                CTParagraphStyleSetting {
                    spec: kCTParagraphStyleSpecifierTabStops,
                    value_size: std::mem::size_of::<CFTypeRef>(),
                    value: &stops as *const CFTypeRef as *const c_void,
                },
            ];
            let para = Cf(CTParagraphStyleCreate(settings.as_ptr(), settings.len()));
            CFAttributedStringSetAttribute(s.ptr(), all, kCTParagraphStyleAttributeName, para.ptr());
            // Colour: the context's, except where a run gives one.
            let mut covered = vec![false; len16 as usize];
            for run in &req.runs {
                let a = byte_to_u16(text, run.range.start as usize);
                let b = byte_to_u16(text, run.range.end as usize);
                if b > a {
                    let color = cgcolor(run.color);
                    let r = CFRange { location: a as CFIndex, length: (b - a) as CFIndex };
                    CFAttributedStringSetAttribute(s.ptr(), r, kCTForegroundColorAttributeName, color.ptr());
                    covered[a..b].iter_mut().for_each(|c| *c = true);
                }
            }
            let mut i = 0;
            while i < covered.len() {
                if covered[i] {
                    i += 1;
                    continue;
                }
                let start = i;
                while i < covered.len() && !covered[i] {
                    i += 1;
                }
                let r = CFRange { location: start as CFIndex, length: (i - start) as CFIndex };
                CFAttributedStringSetAttribute(s.ptr(), r, kCTForegroundColorFromContextAttributeName, kCFBooleanTrue);
            }
            if !req.bold_ranges.is_empty() {
                let bold = self.font(style.font.weight(Weight::Bold));
                for range in &req.bold_ranges {
                    let a = byte_to_u16(text, range.start as usize);
                    let b = byte_to_u16(text, range.end as usize);
                    if b > a {
                        let r = CFRange { location: a as CFIndex, length: (b - a) as CFIndex };
                        CFAttributedStringSetAttribute(s.ptr(), r, kCTFontAttributeName, bold.ptr());
                    }
                }
            }
            CFAttributedStringEndEditing(s.ptr());

            let line_height = (metrics.ascent + metrics.descent + (style.font.size * 0.2)).ceil();
            let mut lines = Vec::new();
            let mut truncated = false;
            let mut push = |line: Cf, top: f32, range: Range<usize>| {
                let mut a = 0.0;
                let mut d = 0.0;
                let mut l = 0.0;
                let w = CTLineGetTypographicBounds(line.ptr(), &mut a, &mut d, &mut l) as f32;
                let ascent = metrics.ascent.max(a as f32);
                let pad = ((line_height - metrics.ascent - metrics.descent) / 2.0).floor();
                lines.push(CtLine { line, top, ascent: ascent + pad, height: line_height, width: w, range });
            };
            match (style.overflow, style.max_width) {
                (Overflow::Wrap, Some(width)) => {
                    let ts = Cf(CTTypesetterCreateWithAttributedString(s.ptr()));
                    let mut start = 0isize;
                    let mut top = 0.0;
                    loop {
                        let count = CTTypesetterSuggestLineBreak(ts.ptr(), start, width as f64);
                        let count = if count <= 0 && start < len16 { 1 } else { count };
                        let line = Cf(CTTypesetterCreateLine(ts.ptr(), CFRange { location: start, length: count }));
                        push(line, top, start as usize..(start + count) as usize);
                        top += line_height;
                        start += count;
                        if start >= len16 {
                            // A final line break leaves an empty last line to put the caret on.
                            if text.ends_with('\n') {
                                let line = Cf(CTTypesetterCreateLine(ts.ptr(), CFRange { location: start, length: 0 }));
                                push(line, top, start as usize..start as usize);
                            }
                            break;
                        }
                    }
                }
                (Overflow::Ellipsis | Overflow::EllipsisMiddle, Some(width)) => {
                    let line = Cf(CTLineCreateWithAttributedString(s.ptr()));
                    let w = CTLineGetTypographicBounds(line.ptr(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut());
                    if w > width as f64 + 0.5 {
                        let token_s = Cf(CFAttributedStringCreateMutable(std::ptr::null(), 0));
                        CFAttributedStringReplaceString(token_s.ptr(), CFRange { location: 0, length: 0 }, cfstr("…").ptr());
                        CFAttributedStringSetAttribute(token_s.ptr(), CFRange { location: 0, length: 1 }, kCTFontAttributeName, font.ptr());
                        CFAttributedStringSetAttribute(token_s.ptr(), CFRange { location: 0, length: 1 }, kCTForegroundColorFromContextAttributeName, kCFBooleanTrue);
                        let token = Cf(CTLineCreateWithAttributedString(token_s.ptr()));
                        let kind = if style.overflow == Overflow::Ellipsis { kCTLineTruncationEnd } else { kCTLineTruncationMiddle };
                        let cut = CTLineCreateTruncatedLine(line.ptr(), width as f64, kind, token.ptr());
                        truncated = true;
                        push(if cut.is_null() { line } else { Cf(cut) }, 0.0, 0..len16 as usize);
                    } else {
                        push(line, 0.0, 0..len16 as usize);
                    }
                }
                _ => {
                    let line = Cf(CTLineCreateWithAttributedString(s.ptr()));
                    push(line, 0.0, 0..len16 as usize);
                }
            }
            let width = lines.iter().map(|l| l.width).fold(0.0, f32::max);
            let height = lines.last().map_or(line_height, |l| l.top + l.height);
            Rc::new(CtLayout { text: text.to_string(), lines, size: Size::new(width.ceil(), height), truncated, map: utf16_map(text) })
        }
    }

    fn metrics(&self, font: Font) -> FontMetrics {
        let f = self.font(font);
        unsafe {
            let ascent = CTFontGetAscent(f.ptr()) as f32;
            let descent = CTFontGetDescent(f.ptr()) as f32;
            let s = Cf(CFAttributedStringCreateMutable(std::ptr::null(), 0));
            CFAttributedStringReplaceString(s.ptr(), CFRange { location: 0, length: 0 }, cfstr("0").ptr());
            CFAttributedStringSetAttribute(s.ptr(), CFRange { location: 0, length: 1 }, kCTFontAttributeName, f.ptr());
            let line = Cf(CTLineCreateWithAttributedString(s.ptr()));
            let w = CTLineGetTypographicBounds(line.ptr(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut());
            FontMetrics { ascent, descent, char_width: w as f32 }
        }
    }

    fn image_size(&self, image: &ImageSource) -> Option<Size> {
        let img = self.image(image)?;
        unsafe { Some(Size::new(CGImageGetWidth(img.ptr()) as f32, CGImageGetHeight(img.ptr()) as f32)) }
    }
}

// MARK: - Painting

pub struct MacPainter<'a> {
    ctx: Cf,
    scale: f32,
    backend: &'a MacBackend,
    pixels: Vec<u8>,
    width: usize,
    height: usize,
}

impl<'a> MacPainter<'a> {
    pub fn new(backend: &'a MacBackend, size: Size, scale: f32) -> Self {
        let width = (size.w * scale).round() as usize;
        let height = (size.h * scale).round() as usize;
        let mut pixels = vec![0u8; width * height * 4];
        let ctx = unsafe {
            let space = Cf(CGColorSpaceCreateWithName(kCGColorSpaceSRGB));
            let ctx = CGBitmapContextCreate(
                pixels.as_mut_ptr() as *mut c_void,
                width,
                height,
                8,
                width * 4,
                space.ptr(),
                kCGImageAlphaPremultipliedLast | kCGBitmapByteOrderDefault,
            );
            // Top-left origin, DIPs.
            CGContextTranslateCTM(ctx, 0.0, height as f64);
            CGContextScaleCTM(ctx, scale as f64, -(scale as f64));
            CGContextSetTextMatrix(ctx, CGAffineTransform { a: 1.0, b: 0.0, c: 0.0, d: -1.0, tx: 0.0, ty: 0.0 });
            CGContextSetAllowsFontSmoothing(ctx, false);
            CGContextSetShouldSmoothFonts(ctx, false);
            Cf(ctx)
        };
        MacPainter { ctx, scale, backend, pixels, width, height }
    }

    pub fn write_png(&self, path: &Path) -> bool {
        let _ = (&self.pixels, self.width, self.height);
        unsafe {
            let image = Cf(CGBitmapContextCreateImage(self.ctx.ptr()));
            let p = path.to_string_lossy();
            let url = Cf(CFURLCreateFromFileSystemRepresentation(std::ptr::null(), p.as_ptr(), p.len() as CFIndex, 0));
            let dest = CGImageDestinationCreateWithURL(url.ptr(), cfstr("public.png").ptr(), 1, std::ptr::null());
            if dest.is_null() {
                return false;
            }
            let dest = Cf(dest);
            CGImageDestinationAddImage(dest.ptr(), image.ptr(), std::ptr::null());
            CGImageDestinationFinalize(dest.ptr())
        }
    }

    fn fill_color(&self, c: Color) {
        unsafe { CGContextSetRGBFillColor(self.ctx.ptr(), c.r as f64, c.g as f64, c.b as f64, c.a as f64) }
    }

    fn stroke_color(&self, c: Color) {
        unsafe { CGContextSetRGBStrokeColor(self.ctx.ptr(), c.r as f64, c.g as f64, c.b as f64, c.a as f64) }
    }

    fn add_path(&self, path: &VectorPath, at: Placement) {
        let ctx = self.ctx.ptr();
        let t = |p: Point| ((p.x * at.scale + at.offset.x) as f64, (p.y * at.scale + at.offset.y) as f64);
        unsafe {
            CGContextBeginPath(ctx);
            for cmd in &path.cmds {
                match *cmd {
                    PathCmd::MoveTo(p) => {
                        let (x, y) = t(p);
                        CGContextMoveToPoint(ctx, x, y)
                    }
                    PathCmd::LineTo(p) => {
                        let (x, y) = t(p);
                        CGContextAddLineToPoint(ctx, x, y)
                    }
                    PathCmd::QuadTo(c, p) => {
                        let (cx, cy) = t(c);
                        let (x, y) = t(p);
                        CGContextAddQuadCurveToPoint(ctx, cx, cy, x, y)
                    }
                    PathCmd::CubicTo(a, b, p) => {
                        let (ax, ay) = t(a);
                        let (bx, by) = t(b);
                        let (x, y) = t(p);
                        CGContextAddCurveToPoint(ctx, ax, ay, bx, by, x, y)
                    }
                    PathCmd::Close => CGContextClosePath(ctx),
                }
            }
        }
    }
}

fn cgr(r: Rect) -> CGRect {
    CGRect::new(r.x as f64, r.y as f64, r.w as f64, r.h as f64)
}

impl Painter for MacPainter<'_> {
    fn scale(&self) -> f32 {
        self.scale
    }

    fn fill_rect(&mut self, r: Rect, color: Color) {
        self.fill_color(color);
        unsafe { CGContextFillRect(self.ctx.ptr(), cgr(r)) }
    }

    fn fill_round_rect(&mut self, r: Rect, radius: f32, color: Color) {
        let radius = radius.min(r.w / 2.0).min(r.h / 2.0) as f64;
        self.fill_color(color);
        unsafe {
            let p = Cf(CGPathCreateWithRoundedRect(cgr(r), radius, radius, std::ptr::null()));
            CGContextBeginPath(self.ctx.ptr());
            CGContextAddPath(self.ctx.ptr(), p.ptr());
            CGContextFillPath(self.ctx.ptr());
        }
    }

    fn stroke_rect(&mut self, r: Rect, width: f32, color: Color) {
        self.stroke_color(color);
        unsafe { CGContextStrokeRectWithWidth(self.ctx.ptr(), cgr(r), width as f64) }
    }

    fn stroke_round_rect(&mut self, r: Rect, radius: f32, width: f32, color: Color) {
        let radius = radius.min(r.w / 2.0).min(r.h / 2.0) as f64;
        self.stroke_color(color);
        unsafe {
            let p = Cf(CGPathCreateWithRoundedRect(cgr(r), radius, radius, std::ptr::null()));
            CGContextSetLineWidth(self.ctx.ptr(), width as f64);
            CGContextBeginPath(self.ctx.ptr());
            CGContextAddPath(self.ctx.ptr(), p.ptr());
            CGContextStrokePath(self.ctx.ptr());
        }
    }

    fn line(&mut self, a: Point, b: Point, width: f32, color: Color) {
        self.stroke_color(color);
        unsafe {
            let ctx = self.ctx.ptr();
            CGContextSetLineWidth(ctx, width as f64);
            CGContextSetLineCap(ctx, kCGLineCapRound);
            CGContextBeginPath(ctx);
            CGContextMoveToPoint(ctx, a.x as f64, a.y as f64);
            CGContextAddLineToPoint(ctx, b.x as f64, b.y as f64);
            CGContextStrokePath(ctx);
        }
    }

    fn dashed_line(&mut self, a: Point, b: Point, width: f32, dash: f32, color: Color) {
        unsafe {
            CGContextSaveGState(self.ctx.ptr());
            let lengths = [dash as f64, dash as f64];
            CGContextSetLineDash(self.ctx.ptr(), 0.0, lengths.as_ptr(), 2);
        }
        self.line(a, b, width, color);
        unsafe { CGContextRestoreGState(self.ctx.ptr()) }
    }

    fn fill_path(&mut self, path: &VectorPath, at: Placement, color: Color) {
        self.fill_color(color);
        self.add_path(path, at);
        unsafe { CGContextDrawPath(self.ctx.ptr(), if path.even_odd { kCGPathEOFill } else { kCGPathFill }) }
    }

    fn stroke_path(&mut self, path: &VectorPath, at: Placement, width: f32, color: Color) {
        self.stroke_color(color);
        self.add_path(path, at);
        unsafe {
            CGContextSetLineWidth(self.ctx.ptr(), width as f64);
            CGContextStrokePath(self.ctx.ptr());
        }
    }

    fn fill_ellipse(&mut self, c: Point, rx: f32, ry: f32, color: Color) {
        self.fill_color(color);
        unsafe {
            CGContextBeginPath(self.ctx.ptr());
            CGContextAddEllipseInRect(self.ctx.ptr(), cgr(Rect::new(c.x - rx, c.y - ry, rx * 2.0, ry * 2.0)));
            CGContextFillPath(self.ctx.ptr());
        }
    }

    fn stroke_ellipse(&mut self, c: Point, rx: f32, ry: f32, width: f32, color: Color) {
        self.stroke_color(color);
        unsafe {
            CGContextSetLineWidth(self.ctx.ptr(), width as f64);
            CGContextBeginPath(self.ctx.ptr());
            CGContextAddEllipseInRect(self.ctx.ptr(), cgr(Rect::new(c.x - rx, c.y - ry, rx * 2.0, ry * 2.0)));
            CGContextStrokePath(self.ctx.ptr());
        }
    }

    fn text(&mut self, layout: &dyn TextLayout, origin: Point, color: Color) {
        let Some(l) = layout.as_any().downcast_ref::<CtLayout>() else { return };
        // Run by run: CoreText leaves a coloured run's colour in the context, and the uncoloured runs after it
        // would draw in it.
        for line in &l.lines {
            unsafe {
                let runs = CTLineGetGlyphRuns(line.line.ptr());
                for i in 0..CFArrayGetCount(runs) {
                    let run = CFArrayGetValueAtIndex(runs, i);
                    let attrs = CTRunGetAttributes(run);
                    if CFDictionaryGetValue(attrs, kCTForegroundColorAttributeName).is_null() {
                        self.fill_color(color);
                    }
                    CGContextSetTextPosition(self.ctx.ptr(), origin.x as f64, (origin.y + line.top + line.ascent) as f64);
                    CTRunDraw(run, self.ctx.ptr(), CFRange { location: 0, length: 0 });
                }
            }
        }
    }

    fn image(&mut self, image: &ImageSource, rect: Rect) {
        let Some(img) = self.backend.image(image) else { return };
        unsafe {
            CGContextSaveGState(self.ctx.ptr());
            // Images draw bottom-up; flip locally.
            CGContextTranslateCTM(self.ctx.ptr(), rect.x as f64, rect.bottom() as f64);
            CGContextScaleCTM(self.ctx.ptr(), 1.0, -1.0);
            CGContextDrawImage(self.ctx.ptr(), CGRect::new(0.0, 0.0, rect.w as f64, rect.h as f64), img.ptr());
            CGContextRestoreGState(self.ctx.ptr());
        }
    }

    fn shadow(&mut self, r: Rect, radius: f32, blur: f32, color: Color) {
        unsafe {
            CGContextSaveGState(self.ctx.ptr());
            let c = cgcolor(color);
            CGContextSetShadowWithColor(self.ctx.ptr(), CGSize { width: 0.0, height: (blur / 3.0) as f64 }, blur as f64, c.ptr());
        }
        self.fill_round_rect(r, radius, color.with_alpha(1.0));
        unsafe { CGContextRestoreGState(self.ctx.ptr()) }
    }

    fn push_clip(&mut self, r: Rect) {
        unsafe {
            CGContextSaveGState(self.ctx.ptr());
            CGContextClipToRect(self.ctx.ptr(), cgr(r));
        }
    }

    fn pop_clip(&mut self) {
        unsafe { CGContextRestoreGState(self.ctx.ptr()) }
    }
}
