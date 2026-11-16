//! Pieces every panel uses: the keyboard-driven list, section labels, key caps, file icons, the popup look.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use crate::geom::{Color, Point, Rect, Size};
use crate::icons::Icon;
use crate::input::{Event, Key, Modifiers};
use crate::paint::{Overflow, Weight};
use crate::platform::Platform;
use crate::text_field::{FieldOutcome, TextField};
use crate::ui::{Align, Id, Ui};
use crate::widgets::{scroll_area, Scroll};

// MARK: - View state

/// What the views remember between frames that the models have no reason to know: scroll positions, where a
/// Shift-extended selection started.
#[derive(Default)]
pub struct ViewState {
    lists: HashMap<Id, ListMemory>,
    scrolls: HashMap<Id, Scroll>,
    seen: HashSet<Id>,
    seen_prev: HashSet<Id>,
    /// Heights of panes split by a divider the views own (the Log's file list).
    pub heights: HashMap<Id, f32>,
    /// The image on screen, decoded.
    pub images: super::viewer_panel::ImageCache,
}

impl ViewState {
    /// Whether the view `id` is drawn this frame for the first time since it was last off screen (SwiftUI's
    /// `onAppear`).
    pub fn appeared(&mut self, id: Id) -> bool {
        self.seen.insert(id);
        !self.seen_prev.contains(&id)
    }

    pub fn end_frame(&mut self) {
        self.seen_prev = std::mem::take(&mut self.seen);
    }

    pub fn list(&mut self, id: Id) -> &mut ListMemory {
        self.lists.entry(id).or_default()
    }

    pub fn scroll(&mut self, id: Id) -> &mut Scroll {
        self.scrolls.entry(id).or_default()
    }

    /// Forgets a list's scroll position (its rows were replaced by others).
    pub fn reset_list(&mut self, id: Id) {
        self.lists.remove(&id);
    }
}

// MARK: - Keyboard-driven list

/// Which rows a click or an arrow key leaves selected in a list that allows several.
pub mod selection {
    use std::collections::HashSet;

    /// The rows from `anchor` to `target`, whichever comes first. Just `target` when the anchor is gone.
    pub fn range(ids: &[String], anchor: Option<&str>, target: &str) -> HashSet<String> {
        let Some(end) = ids.iter().position(|i| i == target) else { return HashSet::new() };
        let Some(start) = anchor.and_then(|a| ids.iter().position(|i| i == a)) else {
            return HashSet::from([target.to_string()]);
        };
        ids[start.min(end)..=start.max(end)].iter().cloned().collect()
    }

    /// Ctrl-click: adds the row, or takes it out again. Returns the selection and the row that becomes the main one.
    pub fn toggle(id: &str, selected: &HashSet<String>, ids: &[String]) -> (HashSet<String>, Option<String>) {
        let mut selected = selected.clone();
        if selected.remove(id) {
            let primary = ids.iter().rev().find(|i| selected.contains(*i)).cloned();
            return (selected, primary);
        }
        selected.insert(id.to_string());
        (selected, Some(id.to_string()))
    }
}

#[derive(Default)]
pub struct ListMemory {
    pub scroll: Scroll,
    /// Where a Shift-extended selection started.
    pub anchor: Option<String>,
    /// The main row last frame: when it moves (from here or from outside), it is scrolled into view.
    seen: Option<String>,
    rows_seen: usize,
    /// A press on a row already among several selected: picked alone on release, unless the press becomes a drag.
    pending: Option<String>,
    /// The press under way has already started a drag.
    dragged: bool,
}

pub struct ListView<'a> {
    /// The list's keyboard focus id.
    pub id: Id,
    pub rect: Rect,
    /// Row height, already scaled.
    pub row_h: f32,
    pub ids: &'a [String],
    pub selection: Option<&'a str>,
    /// Every selected row, for lists that act on several at once (Ctrl-click, Shift-click, Shift+arrows, Ctrl+A).
    pub multi: Option<&'a HashSet<String>>,
    /// Rows that can be clicked but never selected (section headers); the arrow keys step over them.
    pub selectable: Option<&'a dyn Fn(usize) -> bool>,
    /// Centre the main row, however far away it is.
    pub center: bool,
    /// Rows can be dragged out of the window (as files).
    pub draggable: bool,
}

impl<'a> ListView<'a> {
    pub fn new(id: Id, rect: Rect, row_h: f32, ids: &'a [String], selection: Option<&'a str>) -> Self {
        ListView { id, rect, row_h, ids, selection, multi: None, selectable: None, center: false, draggable: false }
    }

    pub fn multi(mut self, multi: &'a HashSet<String>) -> Self {
        self.multi = Some(multi);
        self
    }

    pub fn selectable(mut self, f: &'a dyn Fn(usize) -> bool) -> Self {
        self.selectable = Some(f);
        self
    }

    pub fn center(mut self, center: bool) -> Self {
        self.center = center;
        self
    }

    pub fn draggable(mut self) -> Self {
        self.draggable = true;
        self
    }
}

/// What happened to a list this frame.
#[derive(Default)]
pub struct ListResult {
    /// The new main row (`Some(None)`: nothing selected any more).
    pub select: Option<Option<String>>,
    /// The new set of selected rows, for a list with several.
    pub multi: Option<HashSet<String>>,
    /// A plain click on a row, after it was selected (or on a row that cannot be selected).
    pub clicked: Option<usize>,
    pub double_clicked: Option<usize>,
    /// A right-click on a row: its menu, at the pointer.
    pub menu: Option<(usize, Point)>,
    /// A key the list does not handle, while it has the keyboard, with the main row. Not consumed.
    pub key: Option<(Key, Modifiers, Option<usize>)>,
    /// The last row is on screen (time to load more).
    pub at_end: bool,
    /// A row dragged far enough to take out of the window.
    pub drag: Option<usize>,
}

impl ListResult {
    /// The selection changed from here.
    pub fn changed(&self) -> bool {
        self.select.is_some() || self.multi.is_some()
    }
}

pub const LIST_PAD: f32 = 4.0;

/// A selectable list of rows of one height, drawn lazily, with arrow-key navigation. `draw` paints one row:
/// `(ui, index, rect, selected, focused)`; the selection highlight is already there.
pub fn list(
    ui: &mut Ui,
    mem: &mut ListMemory,
    view: ListView,
    mut draw: impl FnMut(&mut Ui, usize, Rect, bool, bool),
) -> ListResult {
    let t = ui.theme;
    let mut out = ListResult::default();
    let rect = view.rect;
    let n = view.ids.len();
    let pad = t.s(LIST_PAD);
    let content = n as f32 * view.row_h + pad * 2.0;
    let focused = ui.has_focus(view.id);
    let selected_index = view.selection.and_then(|s| view.ids.iter().position(|i| i == s));

    // The main row moved: bring it into view.
    let moved = view.selection.map(str::to_string) != mem.seen;
    if (moved || view.center) && view.selection.is_some() {
        if let Some(i) = selected_index {
            let top = pad + i as f32 * view.row_h;
            if view.center {
                mem.scroll.offset = top - (rect.h - view.row_h) / 2.0;
            } else {
                mem.scroll.reveal(top - pad, view.row_h + pad * 2.0, rect.h);
            }
        }
    }
    if n < mem.rows_seen && n == 0 {
        mem.scroll.offset = 0.0;
    }
    mem.rows_seen = n;
    // The main row was moved from outside: the rest of the selection goes.
    if moved {
        if let Some(multi) = view.multi {
            let keeps = match view.selection {
                Some(s) => multi.contains(s),
                None => multi.is_empty(),
            };
            if !keeps {
                out.multi = Some(view.selection.map(|s| HashSet::from([s.to_string()])).unwrap_or_default());
                mem.anchor = view.selection.map(str::to_string);
            }
        }
    }
    mem.seen = view.selection.map(str::to_string);

    // Empty space takes the keyboard too.
    let back = ui.interact(view.id, rect);
    if back.pressed || back.secondary {
        ui.focus(view.id);
    }
    scroll_area(ui, view.id.with("scroll"), rect, content, &mut mem.scroll);
    let can_select = |i: usize| view.selectable.is_none_or(|f| f(i));

    let pick = |out: &mut ListResult, mem: &mut ListMemory, id: &str| {
        if view.multi.is_some() {
            out.multi = Some(HashSet::from([id.to_string()]));
        }
        mem.anchor = Some(id.to_string());
        out.select = Some(Some(id.to_string()));
    };

    ui.clipped(rect, |ui| {
        let first = ((mem.scroll.offset - pad) / view.row_h).floor().max(0.0) as usize;
        let last = (((mem.scroll.offset + rect.h - pad) / view.row_h).ceil().max(0.0) as usize).min(n);
        if last >= n {
            out.at_end = true;
        }
        for i in first..last {
            let y = rect.y + pad + i as f32 * view.row_h - mem.scroll.offset;
            let row = Rect::new(rect.x, y, rect.w, view.row_h);
            let id = &view.ids[i];
            let selected = view.selection == Some(id.as_str()) || view.multi.is_some_and(|m| m.contains(id));
            let r = ui.interact(view.id.with(("row", id)), row);
            let highlight = row.inset_xy(t.s(6.0), 0.0);
            if selected {
                ui.round(highlight, t.s(6.0), if focused { t.selection } else { t.hover });
            } else if r.hovered && can_select(i) {
                ui.round(highlight, t.s(6.0), t.hover.alpha(0.6));
            }
            draw(ui, i, row, selected, focused);
            if r.pressed {
                mem.dragged = false;
                mem.pending = None;
            }
            // A press on one of several selected rows keeps them all, so they can be dragged out together.
            let keeps_selection =
                view.draggable && selected && !r.mods.ctrl && !r.mods.shift && view.multi.is_some_and(|m| m.len() > 1);
            if r.pressed && keeps_selection && !r.double_clicked {
                ui.focus(view.id);
                mem.pending = Some(id.clone());
            } else if r.pressed {
                ui.focus(view.id);
                if !can_select(i) {
                    out.clicked = Some(i);
                } else if let (Some(multi), true) = (view.multi, r.mods.ctrl) {
                    let mut current = multi.clone();
                    if let Some(s) = view.selection {
                        current.insert(s.to_string());
                    }
                    let (set, primary) = selection::toggle(id, &current, view.ids);
                    out.multi = Some(set);
                    mem.anchor = Some(id.clone());
                    out.select = Some(primary);
                } else if let (Some(_), true) = (view.multi, r.mods.shift) {
                    let anchor = mem.anchor.clone().or(view.selection.map(str::to_string));
                    out.multi = Some(selection::range(view.ids, anchor.as_deref(), id));
                    out.select = Some(Some(id.clone()));
                } else {
                    pick(&mut out, mem, id);
                    if r.double_clicked {
                        out.double_clicked = Some(i);
                    } else {
                        out.clicked = Some(i);
                    }
                }
            }
            if r.clicked && mem.pending.as_deref() == Some(id.as_str()) {
                mem.pending = None;
                pick(&mut out, mem, id);
                out.clicked = Some(i);
            }
            if view.draggable && r.dragging && !mem.dragged {
                let (m, p) = (ui.mouse(), ui.press_pos());
                if (m.x - p.x).abs().max((m.y - p.y).abs()) > t.s(5.0) {
                    mem.dragged = true;
                    mem.pending = None;
                    out.drag = Some(i);
                }
            }
            if r.secondary {
                ui.focus(view.id);
                out.menu = Some((i, ui.mouse()));
            }
        }
    });

    if focused && n > 0 {
        if let Some((key, mods)) = ui.key() {
            let target = match key {
                Key::Down => Some(selected_index.map_or(0, |i| i as isize + 1)),
                Key::Up => Some(selected_index.map_or(n as isize - 1, |i| i as isize - 1)),
                Key::PageDown => Some(selected_index.map_or(0, |i| i as isize) + 12),
                Key::PageUp => Some(selected_index.map_or(n as isize, |i| i as isize) - 12),
                Key::Home if !mods.ctrl => Some(0),
                Key::End if !mods.ctrl => Some(n as isize - 1),
                _ => None,
            };
            let plain = !mods.ctrl && !mods.alt;
            match target {
                Some(target) if plain => {
                    ui.consume();
                    let mut position = target.clamp(0, n as isize - 1);
                    let mut ok = true;
                    if view.selectable.is_some() {
                        // Step past rows that cannot be selected, in the direction of travel; stay put at either end.
                        let mut step = if position >= selected_index.map_or(-1, |i| i as isize) { 1 } else { -1 };
                        if target <= 0 {
                            step = 1;
                        } else if target >= n as isize - 1 {
                            step = -1;
                        }
                        while position >= 0 && position < n as isize && !can_select(position as usize) {
                            position += step;
                        }
                        ok = position >= 0 && position < n as isize;
                    }
                    if ok {
                        let id = &view.ids[position as usize];
                        if view.multi.is_some() && mods.shift {
                            let anchor = mem.anchor.clone().or(view.selection.map(str::to_string));
                            out.multi = Some(selection::range(view.ids, anchor.as_deref(), id));
                            out.select = Some(Some(id.clone()));
                        } else {
                            pick(&mut out, mem, id);
                        }
                    }
                }
                _ if key == Key::Char('a') && mods == Modifiers::CTRL && view.multi.is_some() => {
                    ui.consume();
                    out.multi = Some(view.ids.iter().enumerate().filter(|(i, _)| can_select(*i)).map(|(_, id)| id.clone()).collect());
                }
                _ => out.key = Some((key, mods, selected_index)),
            }
        }
    }
    out
}

// MARK: - Text fields

/// Hands this frame's typing to `field` when it has the keyboard.
pub fn field_input(ui: &mut Ui, platform: &mut dyn Platform, id: Id, field: &mut TextField) -> FieldOutcome {
    if !ui.has_focus(id) || ui.is_consumed() {
        return FieldOutcome::Ignored;
    }
    let Some(event) = ui.event.clone() else { return FieldOutcome::Ignored };
    if !matches!(event, Event::Key { .. } | Event::Text(_) | Event::Preedit { .. }) {
        return FieldOutcome::Ignored;
    }
    let outcome = field.handle(&event, platform.clipboard());
    if outcome != FieldOutcome::Ignored {
        ui.consume();
        ui.request_repaint();
    }
    outcome
}

/// A search field: a magnifier, the text, and a clear button once there is text. Returns what typing did.
#[allow(clippy::too_many_arguments)]
pub fn search_field(
    ui: &mut Ui,
    platform: &mut dyn Platform,
    id: Id,
    rect: Rect,
    field: &mut TextField,
    placeholder: &str,
    size: f32,
) -> FieldOutcome {
    let t = ui.theme;
    let outcome = field_input(ui, platform, id, field);
    let focused = ui.has_focus(id);
    let radius = t.s(6.0);
    ui.round(rect, radius, if t.dark { Color::hex(0x1E1F22) } else { Color::WHITE });
    ui.painter.stroke_round_rect(
        rect.inset(0.5),
        radius,
        if focused { 1.5 } else { 1.0 },
        if focused { t.accent } else { t.hairline.lerp(t.text3, 0.3) },
    );
    let icon = t.s(13.0);
    ui.icon(Icon::Search, Rect::new(rect.x + t.s(7.0), rect.center().y - icon / 2.0, icon, icon), t.text3);
    let mut inner = rect.inset_by(t.s(25.0), 0.0, t.s(6.0), 0.0);
    let mut cleared = false;
    if !field.text.is_empty() {
        let b = Rect::new(rect.right() - t.s(22.0), rect.center().y - t.s(9.0), t.s(18.0), t.s(18.0));
        let r = ui.interact(id.with("clear"), b);
        ui.icon(Icon::DismissCircle, b.inset(t.s(2.0)), if r.hovered { t.text2 } else { t.text3 });
        if r.clicked {
            field.set_text("");
            ui.focus(id);
            cleared = true;
        }
        inner.w -= t.s(18.0);
    }
    let r = ui.interact(id, inner);
    if r.hovered {
        ui.set_cursor(crate::paint::CursorIcon::Text);
    }
    field.draw_text(ui, id, inner, placeholder, t.font(size), focused, &r);
    if cleared {
        FieldOutcome::Changed
    } else {
        outcome
    }
}

// MARK: - Small pieces

/// A section caption above a list, in small capitals.
pub fn section_label(ui: &mut Ui, x: f32, center_y: f32, w: f32, text: &str) {
    let t = ui.theme;
    let font = t.font_w(10.5, Weight::Semibold);
    let h = t.s(16.0);
    ui.label(Rect::new(x, center_y - h / 2.0, w, h), &text.to_uppercase(), font, t.text3);
}

pub fn key_caps_width(ui: &mut Ui, text: &str, size: f32) -> f32 {
    if text.is_empty() {
        return 0.0;
    }
    let t = ui.theme;
    ui.text_width(text, t.font_w(size, Weight::Medium)) + t.s(10.0)
}

/// A keyboard shortcut as a key cap. Returns its rect.
pub fn key_caps(ui: &mut Ui, x: f32, center_y: f32, text: &str, size: f32) -> Rect {
    if text.is_empty() {
        return Rect::new(x, center_y, 0.0, 0.0);
    }
    let t = ui.theme;
    let font = t.font_w(size, Weight::Medium);
    let w = key_caps_width(ui, text, size);
    let h = (font.size * 1.55).round();
    let rect = Rect::new(x, center_y - h / 2.0, w, h);
    ui.round(rect, t.s(4.0), t.chip);
    ui.label_with(rect, text, font, t.text2, Align::Center, Overflow::Clip);
    rect
}

/// "+12 −3" right-aligned at `right`; returns where it starts (or `right` when there is nothing).
pub fn stat_badge(ui: &mut Ui, right: f32, center_y: f32, additions: Option<usize>, deletions: Option<usize>, size: f32) -> f32 {
    let t = ui.theme;
    let font = t.mono(size).weight(Weight::Medium);
    let h = t.s(16.0);
    let mut x = right;
    if let Some(d) = deletions.filter(|d| *d > 0) {
        let s = format!("−{d}");
        let w = ui.text_width(&s, font);
        x -= w;
        ui.label_with(Rect::new(x, center_y - h / 2.0, w + 1.0, h), &s, font, t.removed, Align::Left, Overflow::Clip);
        x -= t.s(5.0);
    }
    if let Some(a) = additions.filter(|a| *a > 0) {
        let s = format!("+{a}");
        let w = ui.text_width(&s, font);
        x -= w;
        ui.label_with(Rect::new(x, center_y - h / 2.0, w + 1.0, h), &s, font, t.added, Align::Left, Overflow::Clip);
        x -= t.s(5.0);
    }
    if x < right {
        x + t.s(5.0)
    } else {
        right
    }
}

enum Glyph {
    Letter(&'static str),
    Icon(Icon),
}

fn file_glyph(path: &str) -> (Glyph, u32) {
    let ext = path.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    use Glyph::*;
    match ext.as_str() {
        "kt" | "kts" => (Letter("K"), 0x8B6CF6),
        "java" => (Letter("J"), 0xE8833A),
        "swift" => (Letter("S"), 0xF05138),
        "xml" => (Icon(self::Icon::Code), 0xE0914A),
        "html" | "htm" => (Icon(self::Icon::Code), 0xE0714A),
        "json" => (Letter("{}"), 0xD9A520),
        "yaml" | "yml" => (Icon(self::Icon::TextBulletListTree), 0xC9566B),
        "gradle" => (Icon(self::Icon::Settings), 0x3BA99C),
        "properties" | "toml" | "ini" | "cfg" | "conf" => (Icon(self::Icon::Settings), 0x8C93A1),
        "md" | "txt" => (Icon(self::Icon::DocumentText), if ext == "md" { 0x4C9FE8 } else { 0x8C93A1 }),
        "js" | "jsx" | "mjs" | "cjs" => (Letter("J"), 0xE2C12E),
        "ts" | "tsx" => (Letter("T"), 0x3D8FD9),
        "py" => (Letter("P"), 0x4B8BBE),
        "go" => (Letter("G"), 0x2AB3CF),
        "rs" => (Letter("R"), 0xD9773A),
        "c" | "cpp" | "cc" | "cxx" => (Letter("C"), 0x5A8FD6),
        "h" | "hpp" => (Letter("H"), 0x8D6FD1),
        "cs" => (Letter("C"), 0x9B6FD6),
        "m" | "mm" => (Letter("M"), 0x5A8FD6),
        "dart" => (Letter("D"), 0x2CB7F6),
        "sh" | "bash" | "zsh" | "ps1" | "bat" | "cmd" => (Icon(self::Icon::WindowConsole), 0x6FBF73),
        "css" => (Letter("#"), 0x4C9FE8),
        "scss" => (Letter("#"), 0xCF649A),
        "sql" => (Icon(self::Icon::Cube), 0xD9A520),
        "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp" | "ico" => (Icon(self::Icon::Image), 0xB46BD6),
        "svg" => (Icon(self::Icon::Image), 0xE0914A),
        "pdf" => (Icon(self::Icon::DocumentText), 0xE5534B),
        "lock" => (Icon(self::Icon::LockClosed), 0x8C93A1),
        "proto" => (Letter("P"), 0x3BA99C),
        "rb" => (Letter("R"), 0xCC342D),
        "php" => (Letter("P"), 0x7377AD),
        _ => (Icon(self::Icon::Document), 0x8C93A1),
    }
}

/// A file-type glyph, tinted by language family, centred in `rect`.
pub fn file_icon(ui: &mut Ui, rect: Rect, path: &str, size: f32) {
    let t = ui.theme;
    let (glyph, color) = file_glyph(path);
    let color = Color::hex(color);
    let s = t.s(size);
    let b = rect.centered(Size::new(s, s));
    match glyph {
        Glyph::Letter(text) => {
            let square = b.inset(t.s(0.5));
            ui.round(square, t.s(3.0), color);
            let font = t.font_w(if text.len() > 1 { size * 0.55 } else { size * 0.68 }, Weight::Bold);
            ui.label_with(square, text, font, Color::WHITE, Align::Center, Overflow::Clip);
        }
        Glyph::Icon(icon) => ui.icon(icon, b.inset(-t.s(1.0)), color),
    }
}

/// The folder glyph of the trees.
pub fn folder_icon(ui: &mut Ui, rect: Rect, open: bool, size: f32) {
    let t = ui.theme;
    let s = t.s(size + 2.0);
    let color = if t.dark { Color::hex(0x8FA8C8) } else { Color::hex(0x6A8BB5) };
    ui.icon(if open { Icon::FolderOpen } else { Icon::Folder }, rect.centered(Size::new(s, s)), color);
}

/// A coloured monogram so projects are recognisable at a glance.
pub fn avatar(ui: &mut Ui, rect: Rect, name: &str) {
    const PAIRS: [(u32, u32); 8] = [
        (0x5B8DEF, 0x3A5FD9),
        (0x34C8A0, 0x1E9E84),
        (0xF2994A, 0xE5683B),
        (0xB57BEE, 0x8B55D6),
        (0xEF6F8E, 0xD9456E),
        (0x4FC3E8, 0x2D95D1),
        (0xE6B93D, 0xD9952A),
        (0x7ED06F, 0x4DAE4E),
    ];
    // The same stable hash as the Mac app, so a project wears the same colour on both.
    let seed = name.bytes().fold(5381i64, |h, b| h.wrapping_mul(33).wrapping_add(b as i64));
    let (a, b) = PAIRS[(seed.unsigned_abs() % PAIRS.len() as u64) as usize];
    let radius = rect.w * 0.28;
    let (ca, cb) = (Color::hex(a), Color::hex(b));
    ui.round(rect, radius, ca.lerp(cb, 0.5));
    // A lighter top left and a darker bottom right, for the gradient's direction.
    ui.round(rect.inset_by(0.0, 0.0, rect.w * 0.45, rect.h * 0.45), radius, ca.alpha(0.55));
    let letter: String = name.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_else(|| "?".into());
    let font = crate::paint::Font::ui(rect.h * 0.5).weight(Weight::Bold);
    ui.label_with(rect, &letter, font, Color::WHITE, Align::Center, Overflow::Clip);
}

/// An indeterminate progress spinner: dots fading round a circle.
pub fn spinner(ui: &mut Ui, center: Point, radius: f32, color: Color) {
    const DOTS: usize = 8;
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    let tick = (START.get_or_init(std::time::Instant::now).elapsed().as_millis() / 90) as u64 % DOTS as u64;
    let dot = (radius * 0.22).max(1.0);
    for i in 0..DOTS {
        let angle = i as f32 / DOTS as f32 * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
        let age = (tick as usize + DOTS - i) % DOTS;
        let alpha = 1.0 - age as f32 / DOTS as f32 * 0.85;
        let p = Point::new(center.x + angle.cos() * (radius - dot), center.y + angle.sin() * (radius - dot));
        ui.painter.fill_ellipse(p, dot, dot, color.alpha(alpha));
    }
    ui.repaint_after(Duration::from_millis(90));
}

/// The floating-panel look shared by every popup: a shadow, the raised surface and a hairline edge.
pub fn popup_panel(ui: &mut Ui, rect: Rect, radius: f32) {
    let t = ui.theme;
    ui.painter.shadow(rect, radius, t.s(24.0), Color::BLACK.alpha(if t.dark { 0.45 } else { 0.22 }));
    ui.round(rect, radius, t.elevated);
    ui.painter.stroke_round_rect(rect.inset(0.5), radius, 1.0, t.hairline);
}

/// A row-height-centred line of secondary text, e.g. "No changes".
pub fn placeholder(ui: &mut Ui, rect: Rect, text: &str) {
    let t = ui.theme;
    let mut style = crate::paint::TextStyle::new(t.font(12.5));
    style.max_width = Some((rect.w - t.s(32.0)).max(10.0));
    style.overflow = Overflow::Wrap;
    let layout = ui.text_layout(text, style);
    ui.draw_layout(&layout, rect, t.text3, Align::Center);
}

/// The ellipsis-free width of `text`, capped.
pub fn width(ui: &mut Ui, text: &str, font: crate::paint::Font) -> f32 {
    ui.text_width(text, font).ceil()
}

/// Text with the characters a fuzzy match landed on in the accent colour and bold. `positions` are byte offsets
/// into the full string `text` was cut from, `offset` bytes before it.
#[allow(clippy::too_many_arguments)]
pub fn fuzzy_label(
    ui: &mut Ui,
    rect: Rect,
    text: &str,
    positions: &[usize],
    offset: usize,
    base: Color,
    font: crate::paint::Font,
    overflow: Overflow,
) -> std::rc::Rc<dyn crate::paint::TextLayout> {
    let t = ui.theme;
    let mut runs = Vec::new();
    let mut bold = Vec::new();
    for &p in positions {
        let Some(local) = p.checked_sub(offset) else { continue };
        if local >= text.len() || !text.is_char_boundary(local) {
            continue;
        }
        let Some(c) = text[local..].chars().next() else { continue };
        let range = local as u32..(local + c.len_utf8()) as u32;
        runs.push(crate::paint::ColorRun { range: range.clone(), color: t.accent });
        bold.push(range);
    }
    let mut style = crate::paint::TextStyle::new(font);
    style.max_width = Some(rect.w.max(1.0));
    style.overflow = overflow;
    let layout = ui.layout(crate::paint::TextRequest { text: std::sync::Arc::from(text), style, runs, bold_ranges: bold });
    ui.draw_layout(&layout, rect, base, Align::Left);
    layout
}

/// A list with nothing in it: a glyph, a title, a line of detail.
pub fn empty_state(ui: &mut Ui, rect: Rect, icon: Icon, title: &str, detail: &str) {
    let t = ui.theme;
    let glyph = t.s(28.0);
    let title_h = t.s(20.0);
    let mut style = crate::paint::TextStyle::new(t.font(11.5));
    style.max_width = Some((rect.w - t.s(48.0)).max(40.0));
    style.overflow = Overflow::Wrap;
    let detail_layout = (!detail.is_empty()).then(|| ui.text_layout(detail, style));
    let detail_h = detail_layout.as_ref().map_or(0.0, |l| l.size().h + t.s(8.0));
    let total = glyph + t.s(8.0) + title_h + detail_h;
    let top = rect.y + ((rect.h - total) / 2.0).max(t.s(12.0));
    ui.icon(icon, Rect::new(rect.center().x - glyph / 2.0, top, glyph, glyph), t.text3);
    let y = top + glyph + t.s(8.0);
    ui.label_with(Rect::new(rect.x + t.s(12.0), y, rect.w - t.s(24.0), title_h), title, t.font_w(13.0, Weight::Medium), t.text2, Align::Center, Overflow::Ellipsis);
    if let Some(layout) = detail_layout {
        let s = layout.size();
        ui.draw_layout(&layout, Rect::new(rect.center().x - s.w / 2.0, y + title_h + t.s(8.0), s.w, s.h), t.text3, Align::Left);
    }
}

/// The filter box over a list: a funnel, the text, and a clear button once there is text.
pub fn filter_field(
    ui: &mut Ui,
    platform: &mut dyn Platform,
    id: Id,
    rect: Rect,
    field: &mut TextField,
    placeholder: &str,
) -> FieldOutcome {
    let t = ui.theme;
    let outcome = field_input(ui, platform, id, field);
    let focused = ui.has_focus(id);
    let radius = t.s(7.0);
    ui.round(rect, radius, t.chip);
    if focused {
        ui.painter.stroke_round_rect(rect.inset(0.5), radius, 1.5, t.accent.alpha(0.7));
    }
    let icon = t.s(13.0);
    ui.icon(Icon::Filter, Rect::new(rect.x + t.s(8.0), rect.center().y - icon / 2.0, icon, icon), t.text3);
    let mut inner = rect.inset_by(t.s(26.0), 0.0, t.s(8.0), 0.0);
    let mut cleared = false;
    if !field.text.is_empty() {
        let b = Rect::new(rect.right() - t.s(22.0), rect.center().y - t.s(8.0), t.s(16.0), t.s(16.0));
        let r = ui.interact(id.with("clear"), b);
        ui.icon(Icon::DismissCircle, b, if r.hovered { t.text2 } else { t.text3 });
        ui.tooltip(&r, &crate::core::lang::l("Clear", "清除"));
        if r.clicked {
            field.set_text("");
            cleared = true;
        }
        inner.w -= t.s(18.0);
    }
    let r = ui.interact(id, inner);
    if r.hovered {
        ui.set_cursor(crate::paint::CursorIcon::Text);
    }
    field.draw_text(ui, id, inner, placeholder, t.font(12.0), focused, &r);
    if cleared {
        FieldOutcome::Changed
    } else {
        outcome
    }
}

/// Speed search, as in the IDE's trees: typing while a list has the keyboard edits its filter without leaving the
/// list. Returns true when the filter changed.
pub fn speed_search(ui: &mut Ui, focused: bool, field: &mut TextField) -> bool {
    if !focused || ui.is_consumed() {
        return false;
    }
    match ui.event.clone() {
        Some(Event::Text(s)) => {
            let ok = !s.is_empty() && s.chars().all(|c| c.is_alphanumeric() || "._-/".contains(c));
            if ok {
                let mut text = field.text.clone();
                text.push_str(&s);
                field.set_text(&text);
                ui.consume();
            }
            ok
        }
        Some(Event::Key { key: Key::Backspace, mods, .. }) if mods.none() && !field.text.is_empty() => {
            let mut text = field.text.clone();
            text.pop();
            field.set_text(&text);
            ui.consume();
            true
        }
        Some(Event::Key { key: Key::Escape, mods, .. }) if mods.none() && !field.text.is_empty() => {
            field.set_text("");
            ui.consume();
            true
        }
        _ => false,
    }
}
