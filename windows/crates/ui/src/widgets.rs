//! Small widgets every view uses: buttons, icon buttons, checkboxes, badges, scrollbars, splitters, tooltips.

use crate::geom::{Color, Point, Rect, Size};
use crate::icons::Icon;
use crate::input::Event;
use crate::paint::{CursorIcon, Font, Overflow, Placement, TextStyle, Weight};
use crate::ui::{layer, Align, Id, Response, Ui};

impl Ui<'_> {
    pub fn icon(&mut self, icon: Icon, rect: Rect, color: Color) {
        let path = icon.path();
        let scale = rect.w.min(rect.h) / 20.0;
        let offset = Point::new(rect.x + (rect.w - 20.0 * scale) / 2.0, rect.y + (rect.h - 20.0 * scale) / 2.0);
        self.painter.fill_path(path, Placement { offset, scale }, color);
    }

    pub fn hline(&mut self, x: f32, y: f32, w: f32, color: Color) {
        let px = 1.0 / self.painter.scale();
        self.painter.fill_rect(Rect::new(x, y, w, px.max(1.0 / 1.5)), color);
    }

    pub fn vline(&mut self, x: f32, y: f32, h: f32, color: Color) {
        let px = 1.0 / self.painter.scale();
        self.painter.fill_rect(Rect::new(x, y, px.max(1.0 / 1.5), h), color);
    }

    pub fn fill(&mut self, rect: Rect, color: Color) {
        self.painter.fill_rect(rect, color);
    }

    pub fn round(&mut self, rect: Rect, radius: f32, color: Color) {
        self.painter.fill_round_rect(rect, radius, color);
    }
}

/// A square button with an icon; `on` draws it pressed in (a toggle that is on).
pub fn icon_button(ui: &mut Ui, id: Id, rect: Rect, icon: Icon, tip: &str, enabled: bool, on: bool) -> Response {
    let mut r = ui.interact(id, rect);
    let t = ui.theme;
    let radius = t.s(5.0);
    if on {
        ui.round(rect, radius, t.accent.alpha(if t.dark { 0.28 } else { 0.16 }));
    } else if enabled && (r.dragging || r.hovered) {
        ui.round(rect, radius, if r.dragging { t.pressed } else { t.hover });
    }
    let color = if !enabled {
        t.text3.alpha(0.6)
    } else if on {
        t.accent
    } else {
        t.text2
    };
    let size = t.s(16.0);
    ui.icon(icon, rect.centered(Size::new(size, size)), color);
    if enabled && r.hovered {
        ui.set_cursor(CursorIcon::Arrow);
    }
    ui.tooltip(&r, tip);
    if !enabled {
        r.clicked = false;
        r.pressed = false;
    }
    r
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonKind {
    /// Filled with the accent colour: the action a dialog is for.
    Primary,
    /// Outlined: the other choices.
    Secondary,
    /// Red: an action that throws something away.
    Danger,
    /// Text only.
    Plain,
}

/// Width a text button needs for its label.
pub fn button_width(ui: &mut Ui, label: &str) -> f32 {
    let font = ui.theme.font_w(12.5, Weight::Medium);
    ui.text_width(label, font) + ui.theme.s(24.0)
}

pub fn button(ui: &mut Ui, id: Id, rect: Rect, label: &str, kind: ButtonKind, enabled: bool) -> Response {
    let mut r = ui.interact(id, rect);
    let t = ui.theme;
    let radius = t.s(5.0);
    let font = t.font_w(12.5, Weight::Medium);
    let (fill, text, border) = match kind {
        ButtonKind::Primary => (t.accent, t.on_accent, None),
        ButtonKind::Danger => (Color::hex(if t.dark { 0xC94F4F } else { 0xD13B3B }), Color::WHITE, None),
        ButtonKind::Secondary => (t.elevated, t.text, Some(t.hairline.lerp(t.text3, 0.35))),
        ButtonKind::Plain => (Color::TRANSPARENT, t.accent, None),
    };
    let fill = if !enabled {
        fill.alpha(0.5)
    } else if r.dragging {
        t.pressed.over(fill)
    } else if r.hovered {
        t.hover.over(fill)
    } else {
        fill
    };
    if fill.a > 0.0 {
        ui.round(rect, radius, fill);
    }
    if let Some(b) = border {
        ui.painter.stroke_round_rect(rect.inset(0.5), radius, 1.0, b);
    }
    let color = if enabled { text } else { text.alpha(0.6) };
    ui.label_with(rect.inset_xy(t.s(6.0), 0.0), label, font, color, Align::Center, Overflow::Ellipsis);
    if !enabled {
        r.clicked = false;
        r.pressed = false;
    }
    r
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Check {
    Off,
    On,
    /// Some of what it stands for is ticked.
    Mixed,
}

/// Draws a checkbox in `rect` (its box is centred) and reports clicks on the whole rect.
pub fn checkbox(ui: &mut Ui, id: Id, rect: Rect, state: Check, enabled: bool) -> Response {
    let r = ui.interact(id, rect);
    draw_checkbox(ui, rect, state, enabled, r.hovered);
    r
}

pub fn draw_checkbox(ui: &mut Ui, rect: Rect, state: Check, enabled: bool, hovered: bool) {
    let t = ui.theme;
    let size = t.s(14.0);
    let b = rect.centered(Size::new(size, size)).snapped(ui.painter.scale());
    let radius = t.s(3.0);
    let alpha = if enabled { 1.0 } else { 0.5 };
    match state {
        Check::Off => {
            ui.round(b, radius, if t.dark { Color::hex(0x2B2D30) } else { Color::WHITE }.alpha(alpha));
            let border = if hovered { t.accent } else { t.text3 };
            ui.painter.stroke_round_rect(b.inset(0.5), radius, 1.0, border.alpha(alpha));
        }
        Check::On | Check::Mixed => {
            ui.round(b, radius, t.accent.alpha(alpha));
            if state == Check::On {
                ui.icon(Icon::Checkmark, b.inset(t.s(1.0)), Color::WHITE.alpha(alpha));
            } else {
                let bar = Rect::new(b.x + size * 0.25, b.center().y - t.s(1.0), size * 0.5, t.s(2.0).max(1.5));
                ui.round(bar, 1.0, Color::WHITE.alpha(alpha));
            }
        }
    }
}

/// A small rounded count, as next to a project or a tab.
pub fn badge(ui: &mut Ui, right: f32, center_y: f32, text: &str, fill: Color, color: Color) -> Rect {
    let t = ui.theme;
    let font = t.font_w(11.0, Weight::Semibold);
    let w = ui.text_width(text, font).max(t.s(8.0)) + t.s(10.0);
    let h = t.s(17.0);
    let rect = Rect::new(right - w, center_y - h / 2.0, w, h);
    ui.round(rect, h / 2.0, fill);
    ui.label_with(rect, text, font, color, Align::Center, Overflow::Clip);
    rect
}

/// A rounded tag (a branch name on a commit, a key in a hint).
pub fn chip(ui: &mut Ui, x: f32, center_y: f32, text: &str, font: Font, fill: Color, color: Color) -> Rect {
    let t = ui.theme;
    let w = ui.text_width(text, font) + t.s(10.0);
    let h = (font.size * 1.45).round();
    let rect = Rect::new(x, center_y - h / 2.0, w, h);
    ui.round(rect, t.s(4.0), fill);
    ui.label_with(rect, text, font, color, Align::Center, Overflow::Clip);
    rect
}

// MARK: Scrolling

/// The scroll position of one area, in DIPs from the top.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Scroll {
    pub offset: f32,
    /// Where the thumb was grabbed, while it is being dragged.
    grab: Option<f32>,
}

impl Scroll {
    pub fn clamp(&mut self, content: f32, view: f32) {
        self.offset = self.offset.clamp(0.0, (content - view).max(0.0));
    }

    /// Scrolls the least distance that brings `top..top + height` into a view of `view` height.
    pub fn reveal(&mut self, top: f32, height: f32, view: f32) {
        if top < self.offset {
            self.offset = top;
        } else if top + height > self.offset + view {
            self.offset = top + height - view;
        }
    }
}

/// Handles the wheel over `rect` and draws a scrollbar when the content is taller than the view.
/// Returns whether the offset changed.
pub fn scroll_area(ui: &mut Ui, id: Id, rect: Rect, content: f32, scroll: &mut Scroll) -> bool {
    let before = scroll.offset;
    if let Some(delta) = ui.scroll_target(id, rect) {
        scroll.offset += delta.y;
    }
    scroll.clamp(content, rect.h);
    vscrollbar(ui, id.with("bar"), rect, content, scroll);
    scroll.offset != before
}

/// A thin overlay scrollbar along the right edge of `rect`.
pub fn vscrollbar(ui: &mut Ui, id: Id, rect: Rect, content: f32, scroll: &mut Scroll) {
    if content <= rect.h + 0.5 {
        return;
    }
    let t = ui.theme;
    let width = t.s(10.0);
    let track = Rect::new(rect.right() - width, rect.y, width, rect.h);
    let r = ui.interact(id, track);
    let thumb_h = (rect.h * rect.h / content).max(t.s(24.0)).min(rect.h);
    let max_offset = content - rect.h;
    let travel = rect.h - thumb_h;
    let thumb_y = rect.y + if max_offset > 0.0 { scroll.offset / max_offset * travel } else { 0.0 };
    let thumb = Rect::new(track.x + t.s(2.0), thumb_y, width - t.s(4.0), thumb_h);
    if r.pressed {
        let y = ui.mouse().y;
        if y >= thumb.y && y < thumb.bottom() {
            scroll.grab = Some(y - thumb.y);
        } else {
            // A click on the track jumps there.
            scroll.grab = Some(thumb_h / 2.0);
        }
    }
    if r.dragging {
        if let Some(grab) = scroll.grab {
            let y = ui.mouse().y - grab - rect.y;
            scroll.offset = if travel > 0.0 { (y / travel).clamp(0.0, 1.0) * max_offset } else { 0.0 };
        }
    } else {
        scroll.grab = None;
    }
    let strong = r.hovered || r.dragging;
    let color = if t.dark { Color::WHITE } else { Color::BLACK }.alpha(if strong { 0.32 } else { 0.18 });
    ui.round(thumb, thumb.w / 2.0, color);
}

// MARK: Splitters

/// A draggable edge between two panes. `value` is the size it controls; `sign` is +1 when dragging right/down grows
/// it. Returns true when a drag ended (time to save the setting).
#[allow(clippy::too_many_arguments)]
pub fn splitter(ui: &mut Ui, id: Id, rect: Rect, vertical_bar: bool, value: &mut f32, min: f32, max: f32, sign: f32) -> bool {
    splitter_clicks(ui, id, rect, vertical_bar, value, min, max, sign).0
}

/// `splitter`, and whether the edge was double-clicked this frame (where that puts something back).
#[allow(clippy::too_many_arguments)]
pub fn splitter_clicks(ui: &mut Ui, id: Id, rect: Rect, vertical_bar: bool, value: &mut f32, min: f32, max: f32, sign: f32) -> (bool, bool) {
    let r = ui.interact(id, rect);
    if r.double_clicked {
        return (false, true);
    }
    let cursor = if vertical_bar { CursorIcon::ResizeHorizontal } else { CursorIcon::ResizeVertical };
    if r.hovered || r.dragging {
        ui.set_cursor(cursor);
    }
    if r.pressed {
        ui.st.drag_value = Some((id, *value));
    }
    if r.dragging {
        let m = ui.mouse();
        let p = ui.press_pos();
        let start = match ui.st.drag_value {
            Some((i, v)) if i == id => v,
            _ => *value,
        };
        let d = if vertical_bar { m.x - p.x } else { m.y - p.y };
        *value = (start + sign * d).clamp(min, max.max(min)).round();
    }
    (matches!(ui.event, Some(Event::MouseUp { .. })) && ui.st.is_active(id), false)
}

// MARK: Tooltips

pub(crate) fn draw_tooltip(ui: &mut Ui, text: &str, anchor: Rect) {
    let t = ui.theme;
    ui.with_layer(layer::TOOLTIP, |ui| {
        let font = t.font(12.0);
        let mut style = TextStyle::new(font);
        style.max_width = Some(t.s(360.0));
        style.overflow = Overflow::Wrap;
        let layout = ui.text_layout(text, style);
        let pad = Size::new(t.s(8.0), t.s(5.0));
        let size = layout.size();
        let w = size.w + pad.w * 2.0;
        let h = size.h + pad.h * 2.0;
        let mut rect = Rect::new(anchor.x + (anchor.w - w) / 2.0, anchor.bottom() + t.s(6.0), w, h);
        if rect.bottom() > ui.size.h - 4.0 {
            rect.y = anchor.y - h - t.s(6.0);
        }
        let rect = rect.clamped_into(&ui.bounds().inset(4.0)).snapped(ui.painter.scale());
        ui.painter.shadow(rect, t.s(5.0), t.s(8.0), t.shadow);
        ui.round(rect, t.s(5.0), t.elevated);
        ui.painter.stroke_round_rect(rect.inset(0.5), t.s(5.0), 1.0, t.hairline);
        ui.painter.text(layout.as_ref(), Point::new(rect.x + pad.w, rect.y + pad.h), t.text);
    });
}
