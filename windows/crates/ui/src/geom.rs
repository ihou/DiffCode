//! Points, sizes, rectangles and colours, in device-independent pixels (DIPs). The backends scale to device pixels.

use std::ops::{Add, Sub};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub const fn new(x: f32, y: f32) -> Self {
        Point { x, y }
    }
}

impl Add for Point {
    type Output = Point;
    fn add(self, o: Point) -> Point {
        Point::new(self.x + o.x, self.y + o.y)
    }
}

impl Sub for Point {
    type Output = Point;
    fn sub(self, o: Point) -> Point {
        Point::new(self.x - o.x, self.y - o.y)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Size {
    pub w: f32,
    pub h: f32,
}

impl Size {
    pub const fn new(w: f32, h: f32) -> Self {
        Size { w, h }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const ZERO: Rect = Rect { x: 0.0, y: 0.0, w: 0.0, h: 0.0 };

    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Rect { x, y, w, h }
    }

    pub fn from_points(a: Point, b: Point) -> Self {
        let x = a.x.min(b.x);
        let y = a.y.min(b.y);
        Rect::new(x, y, (a.x - b.x).abs(), (a.y - b.y).abs())
    }

    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    pub fn origin(&self) -> Point {
        Point::new(self.x, self.y)
    }

    pub fn size(&self) -> Size {
        Size::new(self.w, self.h)
    }

    pub fn center(&self) -> Point {
        Point::new(self.x + self.w / 2.0, self.y + self.h / 2.0)
    }

    pub fn contains(&self, p: Point) -> bool {
        p.x >= self.x && p.x < self.right() && p.y >= self.y && p.y < self.bottom()
    }

    pub fn is_empty(&self) -> bool {
        self.w <= 0.0 || self.h <= 0.0
    }

    pub fn intersects(&self, o: &Rect) -> bool {
        self.x < o.right() && o.x < self.right() && self.y < o.bottom() && o.y < self.bottom()
    }

    pub fn intersect(&self, o: &Rect) -> Rect {
        let x = self.x.max(o.x);
        let y = self.y.max(o.y);
        let r = self.right().min(o.right());
        let b = self.bottom().min(o.bottom());
        Rect::new(x, y, (r - x).max(0.0), (b - y).max(0.0))
    }

    pub fn union(&self, o: &Rect) -> Rect {
        let x = self.x.min(o.x);
        let y = self.y.min(o.y);
        Rect::new(x, y, self.right().max(o.right()) - x, self.bottom().max(o.bottom()) - y)
    }

    /// Shrunk by `d` on every side (grown when negative).
    pub fn inset(&self, d: f32) -> Rect {
        self.inset_xy(d, d)
    }

    pub fn inset_xy(&self, dx: f32, dy: f32) -> Rect {
        Rect::new(self.x + dx, self.y + dy, (self.w - 2.0 * dx).max(0.0), (self.h - 2.0 * dy).max(0.0))
    }

    pub fn inset_by(&self, left: f32, top: f32, right: f32, bottom: f32) -> Rect {
        Rect::new(self.x + left, self.y + top, (self.w - left - right).max(0.0), (self.h - top - bottom).max(0.0))
    }

    pub fn offset(&self, dx: f32, dy: f32) -> Rect {
        Rect::new(self.x + dx, self.y + dy, self.w, self.h)
    }

    pub fn with_w(&self, w: f32) -> Rect {
        Rect::new(self.x, self.y, w, self.h)
    }

    pub fn with_h(&self, h: f32) -> Rect {
        Rect::new(self.x, self.y, self.w, h)
    }

    pub fn with_x(&self, x: f32) -> Rect {
        Rect::new(x, self.y, self.w, self.h)
    }

    pub fn with_y(&self, y: f32) -> Rect {
        Rect::new(self.x, y, self.w, self.h)
    }

    /// Cuts a strip of width `w` off the left; returns (strip, rest).
    pub fn split_left(&self, w: f32) -> (Rect, Rect) {
        let w = w.clamp(0.0, self.w);
        (Rect::new(self.x, self.y, w, self.h), Rect::new(self.x + w, self.y, self.w - w, self.h))
    }

    /// Cuts a strip of width `w` off the right; returns (rest, strip).
    pub fn split_right(&self, w: f32) -> (Rect, Rect) {
        let w = w.clamp(0.0, self.w);
        (Rect::new(self.x, self.y, self.w - w, self.h), Rect::new(self.right() - w, self.y, w, self.h))
    }

    /// Cuts a strip of height `h` off the top; returns (strip, rest).
    pub fn split_top(&self, h: f32) -> (Rect, Rect) {
        let h = h.clamp(0.0, self.h);
        (Rect::new(self.x, self.y, self.w, h), Rect::new(self.x, self.y + h, self.w, self.h - h))
    }

    /// Cuts a strip of height `h` off the bottom; returns (rest, strip).
    pub fn split_bottom(&self, h: f32) -> (Rect, Rect) {
        let h = h.clamp(0.0, self.h);
        (Rect::new(self.x, self.y, self.w, self.h - h), Rect::new(self.x, self.bottom() - h, self.w, h))
    }

    /// Takes a strip off the left in place and returns it.
    pub fn take_left(&mut self, w: f32) -> Rect {
        let (a, b) = self.split_left(w);
        *self = b;
        a
    }

    pub fn take_right(&mut self, w: f32) -> Rect {
        let (a, b) = self.split_right(w);
        *self = a;
        b
    }

    pub fn take_top(&mut self, h: f32) -> Rect {
        let (a, b) = self.split_top(h);
        *self = b;
        a
    }

    pub fn take_bottom(&mut self, h: f32) -> Rect {
        let (a, b) = self.split_bottom(h);
        *self = a;
        b
    }

    /// A rect of `size` centred in this one.
    pub fn centered(&self, size: Size) -> Rect {
        Rect::new(self.x + (self.w - size.w) / 2.0, self.y + (self.h - size.h) / 2.0, size.w, size.h)
    }

    /// Moved and clamped so that it lies inside `bounds` (as far as it fits).
    pub fn clamped_into(&self, bounds: &Rect) -> Rect {
        let w = self.w.min(bounds.w);
        let h = self.h.min(bounds.h);
        let x = self.x.min(bounds.right() - w).max(bounds.x);
        let y = self.y.min(bounds.bottom() - h).max(bounds.y);
        Rect::new(x, y, w, h)
    }

    /// Edges rounded to whole device pixels at `scale`.
    pub fn snapped(&self, scale: f32) -> Rect {
        let x = (self.x * scale).round() / scale;
        let y = (self.y * scale).round() / scale;
        let r = (self.right() * scale).round() / scale;
        let b = (self.bottom() * scale).round() / scale;
        Rect::new(x, y, r - x, b - y)
    }
}

/// sRGB colour with straight alpha.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const TRANSPARENT: Color = Color { r: 0.0, g: 0.0, b: 0.0, a: 0.0 };
    pub const BLACK: Color = Color { r: 0.0, g: 0.0, b: 0.0, a: 1.0 };
    pub const WHITE: Color = Color { r: 1.0, g: 1.0, b: 1.0, a: 1.0 };

    pub const fn hex(rgb: u32) -> Color {
        Color::hexa(rgb, 1.0)
    }

    pub const fn hexa(rgb: u32, a: f32) -> Color {
        Color {
            r: ((rgb >> 16) & 0xFF) as f32 / 255.0,
            g: ((rgb >> 8) & 0xFF) as f32 / 255.0,
            b: (rgb & 0xFF) as f32 / 255.0,
            a,
        }
    }

    pub fn alpha(self, a: f32) -> Color {
        Color { a: self.a * a, ..self }
    }

    pub fn with_alpha(self, a: f32) -> Color {
        Color { a, ..self }
    }

    /// `self` painted over `under` (both straight alpha); the result is opaque when `under` is.
    pub fn over(self, under: Color) -> Color {
        let a = self.a + under.a * (1.0 - self.a);
        if a <= 0.0 {
            return Color::TRANSPARENT;
        }
        let mix = |s: f32, u: f32| (s * self.a + u * under.a * (1.0 - self.a)) / a;
        Color { r: mix(self.r, under.r), g: mix(self.g, under.g), b: mix(self.b, under.b), a }
    }

    pub fn lerp(self, o: Color, t: f32) -> Color {
        Color {
            r: self.r + (o.r - self.r) * t,
            g: self.g + (o.g - self.g) * t,
            b: self.b + (o.b - self.b) * t,
            a: self.a + (o.a - self.a) * t,
        }
    }

    pub fn to_rgba8(self) -> [u8; 4] {
        let c = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        [c(self.r), c(self.g), c(self.b), c(self.a)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_and_clamps() {
        let r = Rect::new(0.0, 0.0, 100.0, 50.0);
        let (l, rest) = r.split_left(30.0);
        assert_eq!(l, Rect::new(0.0, 0.0, 30.0, 50.0));
        assert_eq!(rest, Rect::new(30.0, 0.0, 70.0, 50.0));
        let popup = Rect::new(90.0, 40.0, 30.0, 30.0).clamped_into(&r);
        assert_eq!(popup, Rect::new(70.0, 20.0, 30.0, 30.0));
        let c = Color::hexa(0xFFFFFF, 0.5).over(Color::hex(0x000000));
        assert!((c.r - 0.5).abs() < 1e-6 && c.a == 1.0);
    }
}
