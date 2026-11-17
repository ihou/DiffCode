//! What a backend draws with: the `Painter` (shapes, text, icons, images, clipping) and the `Backend` that makes
//! text layouts and reads images. Coordinates are DIPs; positions in text are UTF-8 byte offsets.

use std::any::Any;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use crate::geom::{Color, Point, Rect, Size};

// MARK: - Fonts and text

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FontKind {
    /// The interface face (Segoe UI on Windows).
    Ui,
    /// The code face: the family picked in Settings, else Cascadia Mono / Consolas on Windows.
    Mono,
    /// The terminal face: the family picked in Settings for terminals, else the default code face.
    Terminal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Weight {
    Regular,
    Medium,
    Semibold,
    Bold,
}

impl Weight {
    /// The OpenType weight value.
    pub fn value(self) -> u16 {
        match self {
            Weight::Regular => 400,
            Weight::Medium => 500,
            Weight::Semibold => 600,
            Weight::Bold => 700,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Font {
    pub kind: FontKind,
    pub size: f32,
    pub weight: Weight,
    pub italic: bool,
}

impl Font {
    pub const fn ui(size: f32) -> Font {
        Font { kind: FontKind::Ui, size, weight: Weight::Regular, italic: false }
    }

    pub const fn mono(size: f32) -> Font {
        Font { kind: FontKind::Mono, size, weight: Weight::Regular, italic: false }
    }

    pub const fn terminal(size: f32) -> Font {
        Font { kind: FontKind::Terminal, size, weight: Weight::Regular, italic: false }
    }

    pub const fn weight(self, weight: Weight) -> Font {
        Font { weight, ..self }
    }

    pub const fn italic(self) -> Font {
        Font { italic: true, ..self }
    }

    pub const fn size(self, size: f32) -> Font {
        Font { size, ..self }
    }
}

impl Eq for Font {}

impl Hash for Font {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.kind.hash(state);
        self.size.to_bits().hash(state);
        self.weight.hash(state);
        self.italic.hash(state);
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FontMetrics {
    pub ascent: f32,
    pub descent: f32,
    /// Advance of "0": the cell width of a monospace face.
    pub char_width: f32,
}

/// How text that does not fit `max_width` is treated.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Overflow {
    /// One line, as wide as it is (clipped by the caller).
    Clip,
    /// One line, cut with "…" at the end.
    Ellipsis,
    /// One line, cut with "…" in the middle (file paths).
    EllipsisMiddle,
    /// Wrapped onto as many lines as it needs.
    Wrap,
}

/// A coloured byte range of the text.
#[derive(Clone, Debug, PartialEq)]
pub struct ColorRun {
    pub range: Range<u32>,
    pub color: Color,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextStyle {
    pub font: Font,
    pub max_width: Option<f32>,
    pub overflow: Overflow,
    /// Spaces per tab stop; tab stops fall at multiples of this many "0" widths.
    pub tab_width: u8,
}

impl TextStyle {
    pub fn new(font: Font) -> Self {
        TextStyle { font, max_width: None, overflow: Overflow::Clip, tab_width: 4 }
    }
}

/// The key a layout is made and cached by.
#[derive(Clone, Debug, PartialEq)]
pub struct TextRequest {
    pub text: Arc<str>,
    pub style: TextStyle,
    pub runs: Vec<ColorRun>,
    /// Byte ranges drawn in a heavier weight (matched characters in Go to File).
    pub bold_ranges: Vec<Range<u32>>,
}

impl TextRequest {
    pub fn cache_key(&self) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.text.hash(&mut h);
        self.style.font.hash(&mut h);
        self.style.max_width.map(f32::to_bits).hash(&mut h);
        self.style.overflow.hash(&mut h);
        self.style.tab_width.hash(&mut h);
        for r in &self.runs {
            r.range.hash(&mut h);
            r.color.to_rgba8().hash(&mut h);
        }
        self.bold_ranges.hash(&mut h);
        h.finish()
    }
}

/// Where a caret sits: its x, and the top and height of its line, relative to the layout's origin.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Caret {
    pub x: f32,
    pub top: f32,
    pub height: f32,
}

/// A laid-out piece of text, made by the backend.
pub trait TextLayout: Any {
    /// Width of the widest line and the total height.
    fn size(&self) -> Size;
    /// Distance from the top to the first line's baseline.
    fn baseline(&self) -> f32;
    fn line_count(&self) -> usize;
    /// Whether an ellipsis cut the text.
    fn is_truncated(&self) -> bool;
    /// The caret position (byte offset, on a character boundary) closest to a point relative to the origin.
    fn hit_test(&self, p: Point) -> usize;
    fn caret(&self, byte: usize) -> Caret;
    /// Rectangles covering a byte range (one per line it spans), relative to the origin.
    fn range_rects(&self, range: Range<usize>) -> Vec<Rect>;
    fn as_any(&self) -> &dyn Any;
}

// MARK: - Paths

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PathCmd {
    MoveTo(Point),
    LineTo(Point),
    QuadTo(Point, Point),
    CubicTo(Point, Point, Point),
    Close,
}

/// A vector shape. `id` lets a backend cache what it builds from the shape (0: never cached).
#[derive(Clone, Debug, PartialEq)]
pub struct VectorPath {
    pub id: u64,
    pub cmds: Vec<PathCmd>,
    pub even_odd: bool,
}

impl VectorPath {
    pub fn new() -> Self {
        VectorPath { id: 0, cmds: Vec::new(), even_odd: false }
    }

    pub fn move_to(&mut self, x: f32, y: f32) -> &mut Self {
        self.cmds.push(PathCmd::MoveTo(Point::new(x, y)));
        self
    }

    pub fn line_to(&mut self, x: f32, y: f32) -> &mut Self {
        self.cmds.push(PathCmd::LineTo(Point::new(x, y)));
        self
    }

    pub fn cubic_to(&mut self, c1: Point, c2: Point, p: Point) -> &mut Self {
        self.cmds.push(PathCmd::CubicTo(c1, c2, p));
        self
    }

    pub fn close(&mut self) -> &mut Self {
        self.cmds.push(PathCmd::Close);
        self
    }

    /// Parses SVG path data (M L H V C S Q T A Z, absolute and relative).
    pub fn from_svg(id: u64, data: &str) -> Self {
        crate::svg::parse(id, data)
    }
}

impl Default for VectorPath {
    fn default() -> Self {
        Self::new()
    }
}

/// How a path is placed: `p * scale + offset`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub offset: Point,
    pub scale: f32,
}

// MARK: - Images

/// Encoded image bytes (PNG, JPEG, GIF, BMP, ICO, TIFF, WebP where the system reads it), identified for caching.
#[derive(Clone, Debug)]
pub struct ImageSource {
    pub id: u64,
    pub bytes: Arc<Vec<u8>>,
}

impl ImageSource {
    pub fn new(bytes: Arc<Vec<u8>>) -> Self {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        bytes.len().hash(&mut h);
        bytes.iter().take(4096).for_each(|b| b.hash(&mut h));
        bytes.iter().rev().take(4096).for_each(|b| b.hash(&mut h));
        ImageSource { id: h.finish(), bytes }
    }
}

// MARK: - Backend

/// Mouse cursor shapes the interface asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CursorIcon {
    #[default]
    Arrow,
    Hand,
    Text,
    ResizeHorizontal,
    ResizeVertical,
    Busy,
}

pub trait Backend {
    fn layout(&self, request: &TextRequest) -> Rc<dyn TextLayout>;
    fn metrics(&self, font: Font) -> FontMetrics;
    /// The families picked in Settings for code and for terminals (None, or one not installed: the default
    /// monospace face). Returns whether that changed what the faces are, so layouts made before are thrown away.
    fn set_families(&self, code: Option<&str>, terminal: Option<&str>) -> bool {
        let _ = (code, terminal);
        false
    }
    /// Pixel size of an image, or None when it cannot be read.
    fn image_size(&self, image: &ImageSource) -> Option<Size>;
}

pub trait Painter {
    /// Device pixels per DIP.
    fn scale(&self) -> f32;
    fn fill_rect(&mut self, r: Rect, color: Color);
    fn fill_round_rect(&mut self, r: Rect, radius: f32, color: Color);
    fn stroke_rect(&mut self, r: Rect, width: f32, color: Color);
    fn stroke_round_rect(&mut self, r: Rect, radius: f32, width: f32, color: Color);
    fn line(&mut self, a: Point, b: Point, width: f32, color: Color);
    fn dashed_line(&mut self, a: Point, b: Point, width: f32, dash: f32, color: Color);
    fn fill_path(&mut self, path: &VectorPath, at: Placement, color: Color);
    fn stroke_path(&mut self, path: &VectorPath, at: Placement, width: f32, color: Color);
    fn fill_ellipse(&mut self, center: Point, rx: f32, ry: f32, color: Color);
    fn stroke_ellipse(&mut self, center: Point, rx: f32, ry: f32, width: f32, color: Color);
    /// Text in `color` wherever its layout gave no colour of its own.
    fn text(&mut self, layout: &dyn TextLayout, origin: Point, color: Color);
    fn image(&mut self, image: &ImageSource, rect: Rect);
    /// A soft drop shadow under a rounded rectangle (popups, menus).
    fn shadow(&mut self, r: Rect, radius: f32, blur: f32, color: Color);
    fn push_clip(&mut self, r: Rect);
    fn pop_clip(&mut self);
}

// MARK: - Icons

/// Of icon images in `sizes` (pixels), the one to draw at `pixels`: the smallest at least that big, else the largest.
pub fn icon_size_for(sizes: &[u32], pixels: u32) -> Option<usize> {
    let bigger = sizes.iter().enumerate().filter(|(_, s)| **s >= pixels).min_by_key(|(_, s)| **s).map(|(i, _)| i);
    bigger.or_else(|| sizes.iter().enumerate().max_by_key(|(_, s)| **s).map(|(i, _)| i))
}

/// The PNG image in an .ico file best for `pixels` (see `icon_size_for`); None for images stored as bitmaps.
pub fn png_in_ico(ico: &[u8], pixels: u32) -> Option<&[u8]> {
    let u16_at = |i: usize| ico.get(i..i + 2).map(|b| u16::from_le_bytes([b[0], b[1]]) as usize);
    let u32_at = |i: usize| ico.get(i..i + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize);
    let count = u16_at(4)?;
    let entry = |i: usize| 6 + 16 * i;
    let sizes: Vec<u32> = (0..count).map(|i| ico.get(entry(i)).map_or(0, |w| if *w == 0 { 256 } else { *w as u32 })).collect();
    let i = icon_size_for(&sizes, pixels)?;
    let (len, offset) = (u32_at(entry(i) + 8)?, u32_at(entry(i) + 12)?);
    let png = ico.get(offset..offset + len)?;
    png.starts_with(b"\x89PNG").then_some(png)
}

#[cfg(test)]
mod icon_tests {
    use super::*;

    #[test]
    fn picks_the_icon_size() {
        let sizes = [16, 32, 48, 256];
        assert_eq!(icon_size_for(&sizes, 30), Some(1));
        assert_eq!(icon_size_for(&sizes, 48), Some(2));
        assert_eq!(icon_size_for(&sizes, 300), Some(3));
        assert_eq!(icon_size_for(&[], 16), None);
    }
}
