//! Editable text for filters, search boxes, dialogs and the commit message.

use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use diffcode_core::text_editing::{character_boundary, word_boundary};

use crate::geom::{Color, Point, Rect};
use crate::input::{EditCommand, Event, Key};
use crate::paint::{CursorIcon, Font, Overflow, TextLayout, TextRequest, TextStyle};
use crate::ui::{Id, Response, Ui};

/// Where text is copied to and pasted from.
pub trait Clipboard {
    fn get(&mut self) -> Option<String>;
    fn set(&mut self, text: &str);
}

/// What a key press did to a field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldOutcome {
    /// Not for this field.
    Ignored,
    /// Handled; the text is the same.
    Handled,
    Changed,
    /// Enter in a one-line field (Ctrl+Enter in a multi-line one).
    Submit,
    Cancel,
}

#[derive(Clone, Debug, Default)]
pub struct TextField {
    pub text: String,
    /// Byte offsets: where the caret is and where the selection started.
    pub caret: usize,
    pub anchor: usize,
    pub multiline: bool,
    scroll_x: f32,
    scroll_y: f32,
    preedit: Option<(String, Option<usize>)>,
    undo: Vec<(String, usize)>,
    redo: Vec<(String, usize)>,
    /// Since this instant the caret is shown solid (it blinks after).
    blink_from: Option<Instant>,
    layout: Option<Rc<dyn TextLayout>>,
    /// A click lands a frame later than the layout it was made on; this keeps a drag's anchor.
    dragging: bool,
}

impl std::fmt::Debug for dyn TextLayout {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TextLayout")
    }
}

impl TextField {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn multiline() -> Self {
        TextField { multiline: true, ..Default::default() }
    }

    pub fn with_text(text: &str) -> Self {
        let mut f = TextField::default();
        f.set_text(text);
        f
    }

    /// Replaces the text, the caret at its end, everything selected when `select` is set.
    pub fn set_text(&mut self, text: &str) {
        self.text = text.to_string();
        self.caret = self.text.len();
        self.anchor = self.caret;
        self.preedit = None;
        self.undo.clear();
        self.redo.clear();
    }

    pub fn select_all(&mut self) {
        self.anchor = 0;
        self.caret = self.text.len();
    }

    pub fn selection(&self) -> std::ops::Range<usize> {
        self.caret.min(self.anchor)..self.caret.max(self.anchor)
    }

    pub fn is_composing(&self) -> bool {
        self.preedit.is_some()
    }

    fn touch(&mut self) {
        self.blink_from = Some(Instant::now());
    }

    fn snapshot(&mut self) {
        self.undo.push((self.text.clone(), self.caret));
        if self.undo.len() > 100 {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// Replaces the selection with `s`.
    pub fn insert(&mut self, s: &str) {
        let s = if self.multiline { s.replace("\r\n", "\n").replace('\r', "\n") } else { s.replace(['\r', '\n'], " ") };
        self.snapshot();
        let r = self.selection();
        self.text.replace_range(r.clone(), &s);
        self.caret = r.start + s.len();
        self.anchor = self.caret;
        self.touch();
    }

    fn delete_range(&mut self, r: std::ops::Range<usize>) -> bool {
        if r.is_empty() {
            return false;
        }
        self.snapshot();
        self.text.replace_range(r.clone(), "");
        self.caret = r.start;
        self.anchor = r.start;
        self.touch();
        true
    }

    fn move_to(&mut self, pos: usize, select: bool) {
        self.caret = pos.min(self.text.len());
        if !select {
            self.anchor = self.caret;
        }
        self.touch();
    }

    fn line_start(&self, pos: usize) -> usize {
        self.text[..pos].rfind('\n').map_or(0, |i| i + 1)
    }

    fn line_end(&self, pos: usize) -> usize {
        self.text[pos..].find('\n').map_or(self.text.len(), |i| pos + i)
    }

    /// One visual line up or down, keeping the caret's x, from the layout drawn last.
    fn vertical(&self, down: bool, lines: f32) -> Option<usize> {
        let layout = self.layout.as_ref()?;
        let c = layout.caret(self.caret);
        let y = if down { c.top + c.height * (lines - 1.0) + c.height * 1.5 } else { c.top - c.height * (lines - 1.0) - c.height * 0.5 };
        if y < 0.0 {
            return Some(0);
        }
        if y > layout.size().h {
            return Some(self.text.len());
        }
        Some(layout.hit_test(Point::new(c.x, y)).min(self.text.len()))
    }

    pub fn command(&mut self, cmd: EditCommand, clipboard: &mut dyn Clipboard) -> FieldOutcome {
        use EditCommand::*;
        let before = self.text.len();
        let changed = match cmd {
            Left { word, select } => {
                let r = self.selection();
                let to = if !select && !r.is_empty() {
                    r.start
                } else if word {
                    word_boundary(&self.text, self.caret, false)
                } else {
                    character_boundary(&self.text, self.caret, false)
                };
                self.move_to(to, select);
                false
            }
            Right { word, select } => {
                let r = self.selection();
                let to = if !select && !r.is_empty() {
                    r.end
                } else if word {
                    word_boundary(&self.text, self.caret, true)
                } else {
                    character_boundary(&self.text, self.caret, true)
                };
                self.move_to(to, select);
                false
            }
            Up { select } | PageUp { select } if self.multiline => {
                let lines = if matches!(cmd, PageUp { .. }) { 10.0 } else { 1.0 };
                let to = self.vertical(false, lines).unwrap_or(0);
                self.move_to(to, select);
                false
            }
            Down { select } | PageDown { select } if self.multiline => {
                let lines = if matches!(cmd, PageDown { .. }) { 10.0 } else { 1.0 };
                let to = self.vertical(true, lines).unwrap_or(self.text.len());
                self.move_to(to, select);
                false
            }
            Home { select } => {
                let to = if self.multiline { self.line_start(self.caret) } else { 0 };
                self.move_to(to, select);
                false
            }
            End { select } => {
                let to = if self.multiline { self.line_end(self.caret) } else { self.text.len() };
                self.move_to(to, select);
                false
            }
            DocStart { select } => {
                self.move_to(0, select);
                false
            }
            DocEnd { select } => {
                self.move_to(self.text.len(), select);
                false
            }
            Backspace { word } => {
                let r = self.selection();
                let r = if !r.is_empty() {
                    r
                } else if word {
                    word_boundary(&self.text, self.caret, false)..self.caret
                } else {
                    character_boundary(&self.text, self.caret, false)..self.caret
                };
                self.delete_range(r)
            }
            Delete { word } => {
                let r = self.selection();
                let r = if !r.is_empty() {
                    r
                } else if word {
                    self.caret..word_boundary(&self.text, self.caret, true)
                } else {
                    self.caret..character_boundary(&self.text, self.caret, true)
                };
                self.delete_range(r)
            }
            SelectAll => {
                self.select_all();
                false
            }
            Copy => {
                let r = self.selection();
                if !r.is_empty() {
                    clipboard.set(&self.text[r]);
                }
                false
            }
            Cut => {
                let r = self.selection();
                if !r.is_empty() {
                    clipboard.set(&self.text[r.clone()]);
                }
                self.delete_range(r)
            }
            Paste => match clipboard.get() {
                Some(s) if !s.is_empty() => {
                    self.insert(&s);
                    true
                }
                _ => false,
            },
            Undo => match self.undo.pop() {
                Some((text, caret)) => {
                    self.redo.push((std::mem::replace(&mut self.text, text), self.caret));
                    self.move_to(caret, false);
                    true
                }
                None => false,
            },
            Redo => match self.redo.pop() {
                Some((text, caret)) => {
                    self.undo.push((std::mem::replace(&mut self.text, text), self.caret));
                    self.move_to(caret, false);
                    true
                }
                None => false,
            },
            Newline if self.multiline => {
                self.insert("\n");
                true
            }
            Newline => return FieldOutcome::Submit,
            _ => return FieldOutcome::Ignored,
        };
        if changed || self.text.len() != before {
            FieldOutcome::Changed
        } else {
            FieldOutcome::Handled
        }
    }

    /// Takes a keyboard or text event while the field has the focus.
    pub fn handle(&mut self, event: &Event, clipboard: &mut dyn Clipboard) -> FieldOutcome {
        match event {
            Event::Key { key: Key::Escape, .. } if self.preedit.is_none() => FieldOutcome::Cancel,
            Event::Key { key: Key::Enter, mods, .. } if self.multiline && mods.ctrl => FieldOutcome::Submit,
            Event::Key { key, mods, .. } => {
                if self.preedit.is_some() {
                    return FieldOutcome::Handled;
                }
                match EditCommand::from_key(*key, *mods) {
                    Some(EditCommand::Tab { .. }) => FieldOutcome::Ignored,
                    Some(cmd) => self.command(cmd, clipboard),
                    None => FieldOutcome::Ignored,
                }
            }
            Event::Text(s) => {
                self.preedit = None;
                let s: String = s.chars().filter(|c| !c.is_control() || (*c == '\n' && self.multiline)).collect();
                if s.is_empty() {
                    return FieldOutcome::Handled;
                }
                self.insert(&s);
                FieldOutcome::Changed
            }
            Event::Preedit { text, caret } => {
                self.preedit = (!text.is_empty()).then(|| (text.clone(), *caret));
                self.touch();
                FieldOutcome::Handled
            }
            _ => FieldOutcome::Ignored,
        }
    }

    /// Draws the field in `rect` (its box included) and handles the mouse. Clicking it takes the focus.
    pub fn show(&mut self, ui: &mut Ui, id: Id, rect: Rect, placeholder: &str, font: Font) -> Response {
        let t = ui.theme;
        let focused = ui.has_focus(id);
        let r = ui.interact(id, rect);
        if r.hovered || r.dragging {
            ui.set_cursor(CursorIcon::Text);
        }
        let radius = t.s(5.0);
        let fill = if t.dark { Color::hex(0x1E1F22) } else { Color::WHITE };
        ui.round(rect, radius, fill);
        let border = if focused { t.accent } else { t.hairline.lerp(t.text3, 0.3) };
        ui.painter.stroke_round_rect(rect.inset(0.5), radius, if focused { 1.5 } else { 1.0 }, border);
        let pad = t.s(8.0);
        let inner = if self.multiline { rect.inset_xy(pad, t.s(6.0)) } else { rect.inset_xy(pad, 0.0) };
        self.draw_text(ui, id, inner, placeholder, font, focused, &r);
        r
    }

    /// Draws just the text, selection and caret in `inner` (for fields with their own box), handling the mouse.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_text(&mut self, ui: &mut Ui, id: Id, inner: Rect, placeholder: &str, font: Font, focused: bool, r: &Response) {
        let t = ui.theme;
        if r.pressed && !focused {
            ui.focus(id);
        }
        let focused = ui.has_focus(id);
        if focused {
            ui.mark_text_input(id);
        }
        // What is shown: the text with any composition in it.
        let (display, pre_range) = match &self.preedit {
            Some((p, _)) => {
                let mut s = self.text.clone();
                let at = self.caret.min(s.len());
                s.insert_str(at, p);
                (s, Some(at..at + p.len()))
            }
            None => (self.text.clone(), None),
        };
        let mut style = TextStyle::new(font);
        if self.multiline {
            style.max_width = Some(inner.w.max(1.0));
            style.overflow = Overflow::Wrap;
        }
        let layout =
            ui.layout(TextRequest { text: Arc::from(display.as_str()), style, runs: Vec::new(), bold_ranges: Vec::new() });
        if self.preedit.is_none() {
            self.layout = Some(layout.clone());
        }
        let size = layout.size();
        let caret_byte = match (&self.preedit, &pre_range) {
            (Some((_, Some(c))), Some(range)) => range.start + c,
            (Some(_), Some(range)) => range.end,
            _ => self.caret,
        };
        let caret = layout.caret(caret_byte.min(display.len()));

        // Keep the caret in view.
        if self.multiline {
            let view = inner.h;
            if caret.top < self.scroll_y {
                self.scroll_y = caret.top;
            } else if caret.top + caret.height > self.scroll_y + view {
                self.scroll_y = caret.top + caret.height - view;
            }
            self.scroll_y = self.scroll_y.clamp(0.0, (size.h - view).max(0.0));
            if let Some(d) = ui.scroll_target(id.with("scroll"), inner) {
                self.scroll_y = (self.scroll_y + d.y).clamp(0.0, (size.h - view).max(0.0));
            }
        } else {
            let view = inner.w - 2.0;
            if caret.x < self.scroll_x {
                self.scroll_x = caret.x;
            } else if caret.x > self.scroll_x + view {
                self.scroll_x = caret.x - view;
            }
            self.scroll_x = self.scroll_x.clamp(0.0, (size.w - view).max(0.0));
        }
        let origin = if self.multiline {
            Point::new(inner.x, inner.y - self.scroll_y)
        } else {
            Point::new(inner.x - self.scroll_x, inner.y + ((inner.h - size.h) / 2.0).round())
        };

        // Mouse: place the caret, drag to select, double-click a word, triple-click everything.
        if self.preedit.is_none() {
            let local = |p: Point| Point::new(p.x - origin.x, p.y - origin.y);
            if r.pressed {
                let at = layout.hit_test(local(ui.mouse())).min(self.text.len());
                if r.triple_clicked {
                    self.select_all();
                } else if r.double_clicked {
                    self.anchor = word_boundary(&self.text, character_boundary(&self.text, at, true), false).min(at);
                    self.caret = word_boundary(&self.text, self.anchor, true);
                } else {
                    self.caret = at;
                    if !r.mods.shift {
                        self.anchor = at;
                    }
                    self.dragging = true;
                }
                self.touch();
            } else if r.dragging && self.dragging {
                self.caret = layout.hit_test(local(ui.mouse())).min(self.text.len());
            } else if !r.dragging {
                self.dragging = false;
            }
        }

        ui.push_clip(inner.inset_xy(-1.0, 0.0));
        if display.is_empty() {
            ui.label(inner.with_h(if self.multiline { font.size * 1.4 } else { inner.h }), placeholder, font, t.text3);
        }
        let sel = self.selection();
        if focused && !sel.is_empty() && self.preedit.is_none() {
            for rr in layout.range_rects(sel) {
                ui.fill(rr.offset(origin.x, origin.y), t.canvas.selection);
            }
        }
        ui.painter.text(layout.as_ref(), origin, t.text);
        if let Some(range) = pre_range {
            for rr in layout.range_rects(range) {
                let y = origin.y + rr.bottom() - 1.0;
                ui.fill(Rect::new(origin.x + rr.x, y, rr.w, 1.0), t.text);
            }
        }
        if focused {
            let elapsed = self.blink_from.map_or(Duration::ZERO, |b| b.elapsed());
            let period = Duration::from_millis(530);
            let on = (elapsed.as_millis() / period.as_millis()) % 2 == 0;
            let caret_rect = Rect::new((origin.x + caret.x).round(), origin.y + caret.top, t.s(1.5).max(1.0), caret.height);
            if on {
                ui.fill(caret_rect, t.text);
            }
            ui.repaint_after(period - Duration::from_millis((elapsed.as_millis() % period.as_millis()) as u64));
            ui.st.ime_area = Some(caret_rect);
        }
        ui.pop_clip();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::Modifiers;

    struct Board(Option<String>);
    impl Clipboard for Board {
        fn get(&mut self) -> Option<String> {
            self.0.clone()
        }
        fn set(&mut self, text: &str) {
            self.0 = Some(text.to_string());
        }
    }

    fn key(k: Key, mods: Modifiers) -> Event {
        Event::Key { key: k, mods, repeat: false }
    }

    #[test]
    fn edits_like_a_windows_field() {
        let mut board = Board(None);
        let mut f = TextField::with_text("hello world");
        f.handle(&key(Key::Backspace, Modifiers::CTRL), &mut board);
        assert_eq!(f.text, "hello ");
        f.handle(&Event::Text("你好".into()), &mut board);
        assert_eq!(f.text, "hello 你好");
        f.handle(&key(Key::Left, Modifiers::SHIFT), &mut board);
        f.handle(&key(Key::Char('x'), Modifiers::CTRL), &mut board);
        assert_eq!(f.text, "hello 你");
        assert_eq!(board.0.as_deref(), Some("好"));
        f.handle(&key(Key::Char('z'), Modifiers::CTRL), &mut board);
        assert_eq!(f.text, "hello 你好");
        f.handle(&key(Key::Home, Modifiers::NONE), &mut board);
        f.handle(&key(Key::Char('v'), Modifiers::CTRL), &mut board);
        assert_eq!(f.text, "好hello 你好");
        assert_eq!(f.handle(&key(Key::Enter, Modifiers::NONE), &mut board), FieldOutcome::Submit);
        assert_eq!(f.handle(&key(Key::Escape, Modifiers::NONE), &mut board), FieldOutcome::Cancel);
    }

    #[test]
    fn composition_does_not_touch_the_text() {
        let mut board = Board(None);
        let mut f = TextField::with_text("a");
        f.handle(&Event::Preedit { text: "ni".into(), caret: Some(2) }, &mut board);
        assert_eq!(f.text, "a");
        assert!(f.is_composing());
        f.handle(&Event::Text("你".into()), &mut board);
        assert_eq!(f.text, "a你");
        assert!(!f.is_composing());
    }
}
