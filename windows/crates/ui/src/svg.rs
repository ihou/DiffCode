//! SVG path data → `VectorPath`. Arcs become cubic Béziers and smooth curves get their reflected control points,
//! so a backend only ever sees move, line, quad, cubic and close.

use crate::geom::Point;
use crate::paint::{PathCmd, VectorPath};

struct Lexer<'a> {
    s: &'a [u8],
    i: usize,
}

impl<'a> Lexer<'a> {
    fn skip(&mut self) {
        while self.i < self.s.len() && (self.s[self.i].is_ascii_whitespace() || self.s[self.i] == b',') {
            self.i += 1;
        }
    }

    fn command(&mut self) -> Option<u8> {
        self.skip();
        let c = *self.s.get(self.i)?;
        if c.is_ascii_alphabetic() && c != b'e' && c != b'E' {
            self.i += 1;
            Some(c)
        } else {
            None
        }
    }

    fn has_number(&mut self) -> bool {
        self.skip();
        matches!(self.s.get(self.i), Some(c) if c.is_ascii_digit() || *c == b'-' || *c == b'+' || *c == b'.')
    }

    fn number(&mut self) -> f32 {
        self.skip();
        let start = self.i;
        if matches!(self.s.get(self.i), Some(b'-') | Some(b'+')) {
            self.i += 1;
        }
        let mut seen_dot = false;
        while let Some(&c) = self.s.get(self.i) {
            if c.is_ascii_digit() {
                self.i += 1;
            } else if c == b'.' && !seen_dot {
                seen_dot = true;
                self.i += 1;
            } else if c == b'e' || c == b'E' {
                self.i += 1;
                if matches!(self.s.get(self.i), Some(b'-') | Some(b'+')) {
                    self.i += 1;
                }
            } else {
                break;
            }
        }
        std::str::from_utf8(&self.s[start..self.i]).ok().and_then(|t| t.parse().ok()).unwrap_or(0.0)
    }

    /// Arc flags may be written without separators ("a1 1 0 01 2 2").
    fn flag(&mut self) -> bool {
        self.skip();
        let c = self.s.get(self.i).copied();
        self.i += 1;
        c == Some(b'1')
    }
}

pub fn parse(id: u64, data: &str) -> VectorPath {
    let mut path = VectorPath { id, cmds: Vec::new(), even_odd: false };
    let mut lx = Lexer { s: data.as_bytes(), i: 0 };
    let mut cur = Point::default();
    let mut start = Point::default();
    let mut last_ctrl: Option<Point> = None; // for S / T
    let mut last_cmd = b' ';
    let mut cmd = match lx.command() {
        Some(c) => c,
        None => return path,
    };
    loop {
        let rel = cmd.is_ascii_lowercase();
        let base = if rel { cur } else { Point::default() };
        let pt = |lx: &mut Lexer, base: Point| {
            let x = lx.number();
            let y = lx.number();
            Point::new(base.x + x, base.y + y)
        };
        match cmd.to_ascii_uppercase() {
            b'M' => {
                cur = pt(&mut lx, base);
                start = cur;
                path.cmds.push(PathCmd::MoveTo(cur));
                // Further pairs are implicit line-tos.
                cmd = if rel { b'l' } else { b'L' };
                last_ctrl = None;
                last_cmd = b'M';
                if lx.has_number() {
                    continue;
                }
            }
            b'L' => {
                cur = pt(&mut lx, base);
                path.cmds.push(PathCmd::LineTo(cur));
                last_ctrl = None;
            }
            b'H' => {
                let x = lx.number();
                cur = Point::new(if rel { cur.x + x } else { x }, cur.y);
                path.cmds.push(PathCmd::LineTo(cur));
                last_ctrl = None;
            }
            b'V' => {
                let y = lx.number();
                cur = Point::new(cur.x, if rel { cur.y + y } else { y });
                path.cmds.push(PathCmd::LineTo(cur));
                last_ctrl = None;
            }
            b'C' => {
                let c1 = pt(&mut lx, base);
                let c2 = pt(&mut lx, base);
                let p = pt(&mut lx, base);
                path.cmds.push(PathCmd::CubicTo(c1, c2, p));
                last_ctrl = Some(c2);
                cur = p;
            }
            b'S' => {
                let c1 = match (last_ctrl, last_cmd.to_ascii_uppercase()) {
                    (Some(c), b'C') | (Some(c), b'S') => Point::new(2.0 * cur.x - c.x, 2.0 * cur.y - c.y),
                    _ => cur,
                };
                let c2 = pt(&mut lx, base);
                let p = pt(&mut lx, base);
                path.cmds.push(PathCmd::CubicTo(c1, c2, p));
                last_ctrl = Some(c2);
                cur = p;
            }
            b'Q' => {
                let c = pt(&mut lx, base);
                let p = pt(&mut lx, base);
                path.cmds.push(PathCmd::QuadTo(c, p));
                last_ctrl = Some(c);
                cur = p;
            }
            b'T' => {
                let c = match (last_ctrl, last_cmd.to_ascii_uppercase()) {
                    (Some(c), b'Q') | (Some(c), b'T') => Point::new(2.0 * cur.x - c.x, 2.0 * cur.y - c.y),
                    _ => cur,
                };
                let p = pt(&mut lx, base);
                path.cmds.push(PathCmd::QuadTo(c, p));
                last_ctrl = Some(c);
                cur = p;
            }
            b'A' => {
                let rx = lx.number();
                let ry = lx.number();
                let rotation = lx.number();
                let large = lx.flag();
                let sweep = lx.flag();
                let p = pt(&mut lx, base);
                arc_to_cubics(&mut path.cmds, cur, p, rx, ry, rotation, large, sweep);
                cur = p;
                last_ctrl = None;
            }
            b'Z' => {
                path.cmds.push(PathCmd::Close);
                cur = start;
                last_ctrl = None;
            }
            _ => break,
        }
        last_cmd = cmd;
        if !cmd.eq_ignore_ascii_case(&b'Z') && lx.has_number() {
            continue; // the same command repeats
        }
        match lx.command() {
            Some(c) => cmd = c,
            None => break,
        }
    }
    path
}

/// Endpoint-parameterised elliptical arc (SVG F.6) as cubic Béziers of at most 90° each.
#[allow(clippy::too_many_arguments)]
fn arc_to_cubics(out: &mut Vec<PathCmd>, p0: Point, p1: Point, rx: f32, ry: f32, rotation: f32, large: bool, sweep: bool) {
    if p0 == p1 {
        return;
    }
    let (mut rx, mut ry) = (rx.abs() as f64, ry.abs() as f64);
    if rx == 0.0 || ry == 0.0 {
        out.push(PathCmd::LineTo(p1));
        return;
    }
    let phi = (rotation as f64).to_radians();
    let (sin, cos) = phi.sin_cos();
    let (x0, y0, x1, y1) = (p0.x as f64, p0.y as f64, p1.x as f64, p1.y as f64);
    let dx = (x0 - x1) / 2.0;
    let dy = (y0 - y1) / 2.0;
    let x1p = cos * dx + sin * dy;
    let y1p = -sin * dx + cos * dy;
    let lambda = (x1p * x1p) / (rx * rx) + (y1p * y1p) / (ry * ry);
    if lambda > 1.0 {
        let s = lambda.sqrt();
        rx *= s;
        ry *= s;
    }
    let num = rx * rx * ry * ry - rx * rx * y1p * y1p - ry * ry * x1p * x1p;
    let den = rx * rx * y1p * y1p + ry * ry * x1p * x1p;
    let mut coef = if den == 0.0 { 0.0 } else { (num / den).max(0.0).sqrt() };
    if large == sweep {
        coef = -coef;
    }
    let cxp = coef * rx * y1p / ry;
    let cyp = -coef * ry * x1p / rx;
    let cx = cos * cxp - sin * cyp + (x0 + x1) / 2.0;
    let cy = sin * cxp + cos * cyp + (y0 + y1) / 2.0;
    let angle = |ux: f64, uy: f64, vx: f64, vy: f64| {
        (ux * vy - uy * vx).atan2(ux * vx + uy * vy)
    };
    let theta1 = angle(1.0, 0.0, (x1p - cxp) / rx, (y1p - cyp) / ry);
    let mut delta = angle((x1p - cxp) / rx, (y1p - cyp) / ry, (-x1p - cxp) / rx, (-y1p - cyp) / ry);
    if !sweep && delta > 0.0 {
        delta -= std::f64::consts::TAU;
    } else if sweep && delta < 0.0 {
        delta += std::f64::consts::TAU;
    }
    let segments = (delta.abs() / std::f64::consts::FRAC_PI_2).ceil().max(1.0) as usize;
    let step = delta / segments as f64;
    let k = 4.0 / 3.0 * (step / 4.0).tan();
    let point = |t: f64| {
        let (s, c) = t.sin_cos();
        (cx + rx * c * cos - ry * s * sin, cy + rx * c * sin + ry * s * cos)
    };
    let deriv = |t: f64| {
        let (s, c) = t.sin_cos();
        (-rx * s * cos - ry * c * sin, -rx * s * sin + ry * c * cos)
    };
    let mut t = theta1;
    for i in 0..segments {
        let t2 = t + step;
        let (ax, ay) = point(t);
        let (bx, by) = point(t2);
        let (dax, day) = deriv(t);
        let (dbx, dby) = deriv(t2);
        let c1 = Point::new((ax + k * dax) as f32, (ay + k * day) as f32);
        let c2 = Point::new((bx - k * dbx) as f32, (by - k * dby) as f32);
        let end = if i == segments - 1 { p1 } else { Point::new(bx as f32, by as f32) };
        out.push(PathCmd::CubicTo(c1, c2, end));
        t = t2;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_commands() {
        let p = parse(1, "M2 2h4v4H2z m10 0 l1 1 2 2");
        assert_eq!(p.cmds[0], PathCmd::MoveTo(Point::new(2.0, 2.0)));
        assert_eq!(p.cmds[1], PathCmd::LineTo(Point::new(6.0, 2.0)));
        assert_eq!(p.cmds[2], PathCmd::LineTo(Point::new(6.0, 6.0)));
        assert_eq!(p.cmds[3], PathCmd::LineTo(Point::new(2.0, 6.0)));
        assert_eq!(p.cmds[4], PathCmd::Close);
        assert_eq!(p.cmds[5], PathCmd::MoveTo(Point::new(12.0, 2.0)));
        assert_eq!(p.cmds[7], PathCmd::LineTo(Point::new(15.0, 5.0)));
    }

    #[test]
    fn arcs_end_where_they_should() {
        let p = parse(2, "M0 10a10 10 0 0 1 20 0");
        let Some(PathCmd::CubicTo(_, _, end)) = p.cmds.last() else { panic!() };
        assert_eq!(*end, Point::new(20.0, 10.0));
        assert_eq!(p.cmds.len(), 3); // a half circle: two quarter curves
        let q = parse(3, "M1.5.5a.5.5 0 01.5.5");
        assert!(matches!(q.cmds.last(), Some(PathCmd::CubicTo(..))));
    }
}
