//! Drawing for the Linux shell: Cairo for shapes and images, Pango for text.
//!
//! GTK hands the draw signal a cairo surface whose device scale already matches the monitor, so everything here
//! works in DIPs and only `scale()` reports the device pixels per DIP for snapping.

use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;

use diffcode_ui::geom::{Color, Point, Rect, Size};
use diffcode_ui::paint::{
    Backend, Caret, Font, FontKind, FontMetrics, ImageSource, Overflow, Painter, PathCmd, Placement, TextLayout,
    TextRequest, VectorPath, Weight,
};
use gtk::cairo;
use gtk::gdk_pixbuf;
use gtk::glib::translate::ToGlibPtr;
use gtk::pango;
use gtk::prelude::*;

const SCALE: f64 = pango::SCALE as f64;

struct FontEntry {
    description: pango::FontDescription,
    metrics: FontMetrics,
}

pub struct LinuxBackend {
    context: pango::Context,
    fonts: RefCell<HashMap<Font, FontEntry>>,
    images: RefCell<HashMap<u64, Option<Rc<Image>>>>,
    /// The families picked in Settings for code and for terminals; None: the default monospace face.
    families: RefCell<(Option<String>, Option<String>)>,
    default_mono: String,
    mono_families: Vec<String>,
}

struct Image {
    surface: cairo::ImageSurface,
    size: Size,
}

/// A PNG the welcome screen and the window icon use, the same 256 px icon the Mac and Windows builds carry.
pub const APP_ICON_PNG: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/res/DiffCode.png"));

/// The default monospace face and every installed family whose characters share one width, as the shell reports
/// them for the font settings.
pub fn monospace_families() -> (String, Vec<String>) {
    let backend = LinuxBackend::new();
    (backend.default_mono.clone(), backend.mono_families.clone())
}

impl LinuxBackend {
    pub fn new() -> Self {
        // A tiny surface is enough for Pango to measure fonts; nothing is drawn on it.
        let surface = cairo::ImageSurface::create(cairo::Format::ARgb32, 1, 1).expect("cairo surface");
        let cr = cairo::Context::new(&surface).expect("cairo context");
        let context = pangocairo::functions::create_context(&cr);
        let mut families: Vec<String> = context.list_families().iter().map(|f| f.name().to_string()).collect();
        families.retain(|name| !name.starts_with('.') && !name.is_empty());
        // Pango's font map knows which families are monospace; it includes aliases such as "monospace".
        let mut mono: Vec<String> = context
            .list_families()
            .iter()
            .filter(|f| f.is_monospace())
            .map(|f| f.name().to_string())
            .filter(|name| {
                let lower = name.to_lowercase();
                !name.starts_with('.')
                    && !name.is_empty()
                    && !["icon", "emoji", "signwriting", "mathjax", "symbol", "awesome"]
                        .iter()
                        .any(|word| lower.contains(word))
            })
            .collect();
        mono.sort_by_key(|n| n.to_lowercase());
        mono.dedup();
        let preferred = [
            "JetBrains Mono",
            "JetBrainsMono Nerd Font",
            "JetBrainsMonoNL Nerd Font",
            "Cascadia Mono",
            "Cascadia Code",
            "Noto Sans Mono",
            "DejaVu Sans Mono",
            "Liberation Mono",
            "Hack",
        ];
        let default_mono = preferred
            .iter()
            .find(|p| mono.iter().any(|m| m.eq_ignore_ascii_case(p)))
            .map(|p| p.to_string())
            .or_else(|| mono.first().cloned())
            .unwrap_or_else(|| "monospace".to_string());
        LinuxBackend {
            context,
            fonts: RefCell::new(HashMap::new()),
            images: RefCell::new(HashMap::new()),
            families: RefCell::new((None, None)),
            default_mono,
            mono_families: mono,
        }
    }

    fn family(&self, kind: FontKind) -> String {
        let families = self.families.borrow();
        let picked = match kind {
            FontKind::Ui => None,
            FontKind::Mono => families.0.as_deref(),
            FontKind::Terminal => families.1.as_deref(),
        };
        match (kind, picked) {
            (FontKind::Ui, _) => "Sans".to_string(),
            (_, Some(name)) => name.to_string(),
            _ => self.default_mono.clone(),
        }
    }

    fn entry(&self, font: Font) -> pango::FontDescription {
        if let Some(entry) = self.fonts.borrow().get(&font) {
            return entry.description.clone();
        }
        let mut description = pango::FontDescription::new();
        description.set_family(&self.family(font.kind));
        description.set_absolute_size(font.size as f64 * SCALE);
        description.set_weight(match font.weight {
            Weight::Regular => pango::Weight::Normal,
            Weight::Medium => pango::Weight::Medium,
            Weight::Semibold => pango::Weight::Semibold,
            Weight::Bold => pango::Weight::Bold,
        });
        if font.italic {
            description.set_style(pango::Style::Italic);
        }
        let m = self.context.metrics(Some(&description), None);
        let metrics = FontMetrics {
            ascent: m.ascent() as f32 / SCALE as f32,
            descent: m.descent() as f32 / SCALE as f32,
            char_width: m.approximate_digit_width() as f32 / SCALE as f32,
        };
        self.fonts.borrow_mut().insert(
            font,
            FontEntry {
                description: description.clone(),
                metrics,
            },
        );
        description
    }

    fn image(&self, image: &ImageSource) -> Option<Rc<Image>> {
        if let Some(cached) = self.images.borrow().get(&image.id) {
            return cached.clone();
        }
        let made = decode(&image.bytes).map(Rc::new);
        self.images.borrow_mut().insert(image.id, made.clone());
        made
    }
}

impl Default for LinuxBackend {
    fn default() -> Self {
        Self::new()
    }
}

/// Decodes an image with gdk-pixbuf into a premultiplied Cairo surface. Cached: images are re-drawn every frame.
fn decode(bytes: &[u8]) -> Option<Image> {
    let stream = gtk::gio::MemoryInputStream::from_bytes(&gtk::glib::Bytes::from(bytes));
    let pixbuf = gdk_pixbuf::Pixbuf::from_stream(&stream, None::<&gtk::gio::Cancellable>).ok()?;
    let width = pixbuf.width() as usize;
    let height = pixbuf.height() as usize;
    if width == 0 || height == 0 {
        return None;
    }
    let channels = pixbuf.n_channels() as usize;
    let stride = pixbuf.rowstride() as usize;
    let pixels = pixbuf.read_pixel_bytes();
    let source = &pixels[..];
    let mut data = Vec::with_capacity(width * height * 4);
    for y in 0..height {
        for x in 0..width {
            let at = y * stride + x * channels;
            let (r, g, b, a) = match channels {
                4 => (source[at], source[at + 1], source[at + 2], source[at + 3] as u32),
                3 => (source[at], source[at + 1], source[at + 2], 255),
                _ => return None,
            };
            // Cairo's ARGB32 is premultiplied and stored in native byte order.
            let word =
                (a << 24) | (((r as u32 * a) / 255) << 16) | (((g as u32 * a) / 255) << 8) | ((b as u32 * a) / 255);
            data.extend_from_slice(&word.to_ne_bytes());
        }
    }
    let surface = cairo::ImageSurface::create_for_data(
        data,
        cairo::Format::ARgb32,
        width as i32,
        height as i32,
        (width * 4) as i32,
    )
    .ok()?;
    Some(Image {
        surface,
        size: Size::new(width as f32, height as f32),
    })
}

fn pango_attr(range: Range<u32>, attr: impl Into<pango::Attribute>) -> pango::Attribute {
    let mut attr: pango::Attribute = attr.into();
    attr.set_start_index(range.start);
    attr.set_end_index(range.end);
    attr
}

/// One laid-out line: where it starts in the text, and how tall it is.
#[derive(Clone, Debug)]
struct Line {
    start: usize,
    end: usize,
    top: f32,
    height: f32,
}

pub struct LinuxLayout {
    text: String,
    layout: pango::Layout,
    lines: Vec<Line>,
    size: Size,
    /// Top padding before the first baseline, so text is centred in the line box as in the Windows build.
    pad: f32,
    /// The first line's baseline in the layout, before `pad`.
    baseline: f32,
    truncated: bool,
}

impl TextLayout for LinuxLayout {
    fn size(&self) -> Size {
        self.size
    }

    fn baseline(&self) -> f32 {
        self.baseline + self.pad
    }

    fn line_count(&self) -> usize {
        self.lines.len()
    }

    fn is_truncated(&self) -> bool {
        self.truncated
    }

    fn hit_test(&self, p: Point) -> usize {
        if self.lines.is_empty() {
            return 0;
        }
        let y = p.y - self.pad;
        let index = self
            .lines
            .iter()
            .position(|l| y < l.top + l.height)
            .unwrap_or(self.lines.len() - 1);
        let line = &self.lines[index];
        let Some(pango_line) = self.layout.line(index as i32) else {
            return line.end;
        };
        let x = (p.x.max(0.0) * SCALE as f32).round() as i32;
        let hit = pango_line.x_to_index(x);
        let mut at = (hit.index() as usize).clamp(line.start, line.end);
        // A position in the right half of a character puts the caret after it.
        if hit.trailing() > 0 && at < line.end {
            if let Some(c) = self.text[at..line.end].chars().next() {
                at += c.len_utf8();
            }
        }
        at
    }

    fn caret(&self, byte: usize) -> Caret {
        let byte = diffcode_core::text_editing::floor_boundary(&self.text, byte.min(self.text.len()));
        let mut index = self
            .lines
            .iter()
            .position(|l| byte < l.end)
            .unwrap_or(self.lines.len().saturating_sub(1));
        // At a hard line break `byte` is the newline itself: keep it at the end of the previous line. A soft wrap
        // has no newline, so the next line starts exactly at `byte` and should keep the caret there.
        if index > 0 && byte < self.lines[index].start {
            index -= 1;
        }
        let line = &self.lines[index];
        let x = self.x_of(index, byte);
        Caret {
            x,
            top: line.top + self.pad,
            height: line.height.max(1.0),
        }
    }

    fn range_rects(&self, range: Range<usize>) -> Vec<Rect> {
        let start = diffcode_core::text_editing::floor_boundary(&self.text, range.start.min(self.text.len()));
        let end = diffcode_core::text_editing::floor_boundary(&self.text, range.end.min(self.text.len()));
        if start >= end {
            return Vec::new();
        }
        let mut rects = Vec::new();
        for (i, line) in self.lines.iter().enumerate() {
            let a = start.max(line.start);
            let b = end.min(line.end);
            if a >= b {
                continue;
            }
            let x0 = self.x_of(i, a);
            let x1 = self.x_of(i, b);
            rects.push(Rect::new(
                x0,
                line.top + self.pad,
                (x1 - x0).max(0.0),
                line.height.max(1.0),
            ));
        }
        rects
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl LinuxLayout {
    /// The x of a byte offset on line `index`, relative to the layout's origin.
    fn x_of(&self, index: usize, byte: usize) -> f32 {
        let Some(line) = self.lines.get(index) else { return 0.0 };
        let byte = byte.clamp(line.start, line.end);
        let Some(pango_line) = self.layout.line(index as i32) else {
            return 0.0;
        };
        if byte == line.end {
            return pango_line.pixel_extents().1.width() as f32;
        }
        pango_line.index_to_x(byte as i32, false) as f32 / SCALE as f32
    }
}

impl Backend for LinuxBackend {
    fn set_families(&self, code: Option<&str>, terminal: Option<&str>) -> bool {
        let resolved = |name: Option<&str>| {
            name.filter(|name| {
                self.mono_families
                    .iter()
                    .any(|family| family.eq_ignore_ascii_case(name))
            })
            .map(String::from)
        };
        let picked = (resolved(code), resolved(terminal));
        if *self.families.borrow() == picked {
            return false;
        }
        *self.families.borrow_mut() = picked;
        self.fonts.borrow_mut().retain(|f, _| f.kind == FontKind::Ui);
        true
    }

    fn layout(&self, req: &TextRequest) -> Rc<dyn TextLayout> {
        let style = &req.style;
        let font = self.entry(style.font);
        let layout = pango::Layout::new(&self.context);
        layout.set_font_description(Some(&font));
        layout.set_text(&req.text);
        let attrs = pango::AttrList::new();
        for run in &req.runs {
            let a = run.range.start.min(req.text.len() as u32);
            let b = run.range.end.min(req.text.len() as u32);
            if a < b {
                let color = run.color;
                let attr = pango::AttrColor::new_foreground(
                    (color.r.clamp(0.0, 1.0) * 65535.0) as u16,
                    (color.g.clamp(0.0, 1.0) * 65535.0) as u16,
                    (color.b.clamp(0.0, 1.0) * 65535.0) as u16,
                );
                attrs.insert(pango_attr(a..b, attr));
            }
        }
        for range in &req.bold_ranges {
            let a = range.start.min(req.text.len() as u32);
            let b = range.end.min(req.text.len() as u32);
            if a < b {
                attrs.insert(pango_attr(a..b, pango::AttrInt::new_weight(pango::Weight::Bold)));
            }
        }
        layout.set_attributes(Some(&attrs));
        if req.text.contains('\t') {
            set_tabs(&layout, style.tab_width, self.entry(style.font));
        }
        let m = self.context.metrics(Some(&font), None);
        let ascent = m.ascent() as f32 / SCALE as f32;
        let descent = m.descent() as f32 / SCALE as f32;
        let line_height = (ascent + descent + style.font.size * 0.2).ceil();
        let pad = ((line_height - ascent - descent) / 2.0).max(0.0);
        let mut truncated = false;
        match (style.overflow, style.max_width) {
            (Overflow::Wrap, Some(width)) => {
                layout.set_width((width.max(1.0) * SCALE as f32).round() as i32);
                layout.set_wrap(pango::WrapMode::WordChar);
            }
            (Overflow::Ellipsis | Overflow::EllipsisMiddle, Some(width)) => {
                layout.set_width((width.max(1.0) * SCALE as f32).round() as i32);
                layout.set_ellipsize(if style.overflow == Overflow::Ellipsis {
                    pango::EllipsizeMode::End
                } else {
                    pango::EllipsizeMode::Middle
                });
                layout.set_single_paragraph_mode(true);
                truncated = layout.is_ellipsized();
            }
            _ => {}
        }
        // Pango puts the extra leading after each line; the Windows build centres it, so the same amount is added
        // above the first baseline through `pad`.
        let spacing = (line_height * SCALE as f32 - m.ascent() as f32 - m.descent() as f32)
            .max(0.0)
            .round() as i32;
        layout.set_spacing(spacing);
        let mut lines = Vec::new();
        let mut first_baseline = None;
        let mut iter = layout.iter();
        loop {
            let line = iter.line();
            let (top, bottom) = iter.line_yrange();
            if let Some(line) = line {
                let start = line.start_index() as usize;
                let end = start + line.length() as usize;
                lines.push(Line {
                    start,
                    end,
                    top: top as f32 / SCALE as f32,
                    height: (bottom - top).max(0) as f32 / SCALE as f32,
                });
            }
            if first_baseline.is_none() {
                first_baseline = Some(iter.baseline() as f32 / SCALE as f32);
            }
            if !iter.next_line() {
                break;
            }
        }
        if lines.is_empty() {
            lines.push(Line {
                start: 0,
                end: req.text.len(),
                top: 0.0,
                height: line_height,
            });
        }
        let width = layout.size().0 as f32 / SCALE as f32;
        let height = layout.size().1 as f32 / SCALE as f32;
        let size = Size::new(width.ceil(), height.max(line_height));
        Rc::new(LinuxLayout {
            text: req.text.to_string(),
            layout,
            lines,
            size,
            pad,
            baseline: first_baseline.unwrap_or(ascent),
            truncated,
        })
    }

    fn metrics(&self, font: Font) -> FontMetrics {
        self.entry(font);
        self.fonts
            .borrow()
            .get(&font)
            .map(|e| e.metrics)
            .unwrap_or(FontMetrics {
                ascent: font.size,
                descent: 0.0,
                char_width: 1.0,
            })
    }

    fn image_size(&self, image: &ImageSource) -> Option<Size> {
        self.image(image).map(|i| i.size)
    }
}

/// Tabs every `tab_width` cells of the monospace face, up to a few thousand pixels. Only lines with a tab pay for it.
fn set_tabs(layout: &pango::Layout, tab_width: u8, font: pango::FontDescription) {
    let surface = cairo::ImageSurface::create(cairo::Format::A8, 1, 1).expect("cairo surface");
    let cr = cairo::Context::new(&surface).expect("cairo context");
    let context = pangocairo::functions::create_context(&cr);
    let metrics = context.metrics(Some(&font), None);
    let step = (metrics.approximate_digit_width() as f32 * tab_width.max(1) as f32).max(1.0);
    let mut tabs = pango::TabArray::new(256, false);
    for i in 0..256 {
        tabs.set_tab(i, pango::TabAlign::Left, ((i as f32 + 1.0) * step) as i32);
    }
    unsafe {
        pango::ffi::pango_layout_set_tabs(layout.to_glib_none().0, tabs.to_glib_none().0);
    }
}

// MARK: - Painting

pub struct LinuxPainter<'a> {
    cr: &'a cairo::Context,
    backend: &'a LinuxBackend,
    scale: f32,
}

impl<'a> LinuxPainter<'a> {
    pub fn new(cr: &'a cairo::Context, backend: &'a LinuxBackend) -> Self {
        let scale = cr.target().device_scale().0 as f32;
        LinuxPainter {
            cr,
            backend,
            scale: if scale > 0.0 { scale } else { 1.0 },
        }
    }

    fn source(&self, color: Color) {
        self.cr
            .set_source_rgba(color.r as f64, color.g as f64, color.b as f64, color.a as f64);
    }

    fn rounded(&self, r: Rect, radius: f32) {
        let radius = radius.min(r.w / 2.0).min(r.h / 2.0).max(0.0) as f64;
        let (x, y, w, h) = (r.x as f64, r.y as f64, r.w as f64, r.h as f64);
        self.cr.new_sub_path();
        self.cr
            .arc(x + w - radius, y + radius, radius, -std::f64::consts::FRAC_PI_2, 0.0);
        self.cr
            .arc(x + w - radius, y + h - radius, radius, 0.0, std::f64::consts::FRAC_PI_2);
        self.cr.arc(
            x + radius,
            y + h - radius,
            radius,
            std::f64::consts::FRAC_PI_2,
            std::f64::consts::PI,
        );
        self.cr.arc(
            x + radius,
            y + radius,
            radius,
            std::f64::consts::PI,
            1.5 * std::f64::consts::PI,
        );
        self.cr.close_path();
    }
}

impl Painter for LinuxPainter<'_> {
    fn scale(&self) -> f32 {
        self.scale
    }

    fn fill_rect(&mut self, r: Rect, color: Color) {
        self.source(color);
        self.cr.rectangle(r.x as f64, r.y as f64, r.w as f64, r.h as f64);
        self.cr.fill().ok();
    }

    fn fill_round_rect(&mut self, r: Rect, radius: f32, color: Color) {
        self.source(color);
        self.cr.new_path();
        self.rounded(r, radius);
        self.cr.fill().ok();
    }

    fn stroke_rect(&mut self, r: Rect, width: f32, color: Color) {
        self.source(color);
        self.cr.set_line_width(width as f64);
        self.cr.rectangle(r.x as f64, r.y as f64, r.w as f64, r.h as f64);
        self.cr.stroke().ok();
    }

    fn stroke_round_rect(&mut self, r: Rect, radius: f32, width: f32, color: Color) {
        self.source(color);
        self.cr.set_line_width(width as f64);
        self.cr.new_path();
        self.rounded(r, radius);
        self.cr.stroke().ok();
    }

    fn line(&mut self, a: Point, b: Point, width: f32, color: Color) {
        self.source(color);
        self.cr.save().ok();
        self.cr.set_line_width(width as f64);
        self.cr.set_line_cap(cairo::LineCap::Round);
        self.cr.move_to(a.x as f64, a.y as f64);
        self.cr.line_to(b.x as f64, b.y as f64);
        self.cr.stroke().ok();
        self.cr.restore().ok();
    }

    fn dashed_line(&mut self, a: Point, b: Point, width: f32, dash: f32, color: Color) {
        self.cr.save().ok();
        self.cr.set_dash(&[dash as f64, dash as f64], 0.0);
        self.line(a, b, width, color);
        self.cr.restore().ok();
    }

    fn fill_path(&mut self, path: &VectorPath, at: Placement, color: Color) {
        self.source(color);
        self.add_path(path, at);
        if path.even_odd {
            self.cr.set_fill_rule(cairo::FillRule::EvenOdd);
        }
        self.cr.fill().ok();
        self.cr.set_fill_rule(cairo::FillRule::Winding);
    }

    fn stroke_path(&mut self, path: &VectorPath, at: Placement, width: f32, color: Color) {
        self.source(color);
        self.cr.set_line_width(width as f64);
        self.add_path(path, at);
        self.cr.stroke().ok();
    }

    fn fill_ellipse(&mut self, center: Point, rx: f32, ry: f32, color: Color) {
        self.source(color);
        self.cr.save().ok();
        self.cr.translate(center.x as f64, center.y as f64);
        self.cr.scale(rx.max(0.0) as f64, ry.max(0.0) as f64);
        self.cr.arc(0.0, 0.0, 1.0, 0.0, std::f64::consts::TAU);
        self.cr.restore().ok();
        self.cr.fill().ok();
    }

    fn stroke_ellipse(&mut self, center: Point, rx: f32, ry: f32, width: f32, color: Color) {
        self.source(color);
        self.cr.set_line_width(width as f64);
        self.cr.save().ok();
        self.cr.translate(center.x as f64, center.y as f64);
        self.cr.scale(rx.max(0.0) as f64, ry.max(0.0) as f64);
        self.cr.arc(0.0, 0.0, 1.0, 0.0, std::f64::consts::TAU);
        self.cr.restore().ok();
        self.cr.stroke().ok();
    }

    fn text(&mut self, layout: &dyn TextLayout, origin: Point, color: Color) {
        let Some(l) = layout.as_any().downcast_ref::<LinuxLayout>() else {
            return;
        };
        self.cr.save().ok();
        self.source(color);
        self.cr.move_to(origin.x as f64, (origin.y + l.pad) as f64);
        pangocairo::functions::show_layout(self.cr, &l.layout);
        self.cr.restore().ok();
    }

    fn image(&mut self, image: &ImageSource, rect: Rect) {
        let Some(img) = self.backend.image(image) else { return };
        let (w, h) = (img.size.w.max(1.0) as f64, img.size.h.max(1.0) as f64);
        self.cr.save().ok();
        self.cr.translate(rect.x as f64, rect.y as f64);
        self.cr.scale(rect.w as f64 / w, rect.h as f64 / h);
        self.cr.set_source_surface(&img.surface, 0.0, 0.0).ok();
        self.cr.paint().ok();
        self.cr.restore().ok();
    }

    fn shadow(&mut self, r: Rect, radius: f32, blur: f32, color: Color) {
        // Cairo has no shadow primitive: a handful of translucent rings under the box reads the same at this size.
        let layers = 8;
        for i in (1..=layers).rev() {
            let grow = blur * i as f32 / layers as f32;
            let alpha = color.a * 0.05;
            self.fill_round_rect(
                Rect::new(r.x - grow, r.y - grow + blur / 3.0, r.w + grow * 2.0, r.h + grow * 2.0),
                radius + grow,
                Color {
                    r: color.r,
                    g: color.g,
                    b: color.b,
                    a: alpha,
                },
            );
        }
    }

    fn push_clip(&mut self, r: Rect) {
        self.cr.save().ok();
        self.cr.rectangle(r.x as f64, r.y as f64, r.w as f64, r.h as f64);
        self.cr.clip();
    }

    fn pop_clip(&mut self) {
        self.cr.restore().ok();
    }
}

impl LinuxPainter<'_> {
    fn add_path(&self, path: &VectorPath, at: Placement) {
        let t = |p: Point| {
            (
                (p.x * at.scale + at.offset.x) as f64,
                (p.y * at.scale + at.offset.y) as f64,
            )
        };
        self.cr.new_path();
        for cmd in &path.cmds {
            match *cmd {
                PathCmd::MoveTo(p) => {
                    let (x, y) = t(p);
                    self.cr.move_to(x, y);
                }
                PathCmd::LineTo(p) => {
                    let (x, y) = t(p);
                    self.cr.line_to(x, y);
                }
                PathCmd::QuadTo(c, p) => {
                    // Cairo has no quadratic curves; the equivalent cubic control points.
                    let (x0, y0) = self.cr.current_point().unwrap_or((0.0, 0.0));
                    let (cx, cy) = t(c);
                    let (x, y) = t(p);
                    let c1 = (x0 + 2.0 / 3.0 * (cx - x0), y0 + 2.0 / 3.0 * (cy - y0));
                    let c2 = (x + 2.0 / 3.0 * (cx - x), y + 2.0 / 3.0 * (cy - y));
                    self.cr.curve_to(c1.0, c1.1, c2.0, c2.1, x, y);
                }
                PathCmd::CubicTo(a, b, p) => {
                    let (ax, ay) = t(a);
                    let (bx, by) = t(b);
                    let (x, y) = t(p);
                    self.cr.curve_to(ax, ay, bx, by, x, y);
                }
                PathCmd::Close => self.cr.close_path(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use diffcode_ui::paint::TextStyle;
    use std::sync::Arc;

    fn layout(text: &str, style: TextStyle) -> Rc<dyn TextLayout> {
        LinuxBackend::new().layout(&TextRequest {
            text: Arc::from(text),
            style,
            runs: Vec::new(),
            bold_ranges: Vec::new(),
        })
    }

    #[test]
    fn caret_stays_before_a_newline() {
        let layout = layout("ab\ncd", TextStyle::new(Font::mono(13.0)));
        assert_eq!(layout.caret(1).top, layout.caret(2).top);
        assert!(layout.caret(3).top > layout.caret(2).top);
    }

    #[test]
    fn ellipsis_is_reported() {
        let mut style = TextStyle::new(Font::mono(13.0));
        style.max_width = Some(12.0);
        style.overflow = Overflow::Ellipsis;
        let layout = layout("a filename that does not fit", style);
        assert!(layout.is_truncated());
    }
}
