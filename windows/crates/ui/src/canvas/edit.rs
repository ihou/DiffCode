//! Typing into the right side of the canvas, when it is a file of the working tree that can be edited.
//!
//! Positions here are lines and byte columns of the right side; the canvas maps them to its rows. An input
//! method composes in place: its text sits in the line, unrecorded, until it is committed.

use std::time::Instant;

use super::{CanvasAction, CodeCanvas, Mode, TextSel};
use crate::core::diff::{DiffDocument, Hunk, Side};
use crate::core::editor_session::{EditKind, EditorSession, Outcome, Selection};
use crate::core::text_editing::{self, TextPosition};
use crate::input::{EditCommand, Event, Key, Modifiers};
use crate::text_field::Clipboard;
use crate::ui::Ui;

impl CodeCanvas {
    fn lines(&self) -> &[String] {
        self.doc.as_ref().map_or(&[], |d| d.right.lines())
    }

    fn last_row(&self) -> usize {
        self.lines().len().saturating_sub(1)
    }

    fn line_len(&self, row: usize) -> usize {
        self.lines().get(row).map_or(0, |l| l.len())
    }

    fn line(&self, row: usize) -> &str {
        self.lines().get(row).map_or("", |l| l.as_str())
    }

    fn indent_unit(&mut self) -> String {
        match (&mut self.editor, &self.doc) {
            (Some(e), Some(d)) => e.indent_unit(d).to_string(),
            _ => "    ".to_string(),
        }
    }

    // MARK: Input

    /// Handles this frame's key, text or composition while the canvas has the keyboard. Returns whether it
    /// took the event (and consumes it then).
    pub fn handle_input(&mut self, ui: &mut Ui, clipboard: &mut dyn Clipboard) -> bool {
        if ui.is_consumed() {
            return false;
        }
        let Some(event) = ui.event.clone() else { return false };
        let handled = match &event {
            Event::Key { key, mods, .. } => self.handle_key(ui, *key, *mods, clipboard),
            Event::Text(text) => self.handle_text(text),
            Event::Preedit { text, caret } => self.handle_preedit(text, *caret),
            _ => false,
        };
        if handled {
            ui.consume();
            self.blink_from = Instant::now();
        }
        handled
    }

    fn handle_key(&mut self, ui: &mut Ui, key: Key, mods: Modifiers, clipboard: &mut dyn Clipboard) -> bool {
        if self.editor.is_some() && self.edit_selection().is_some() {
            if self.editor.as_ref().is_some_and(|e| e.marked().is_some()) {
                // The input method has the keys while it composes.
                return true;
            }
            if self.handle_editing_key(ui, key, mods, clipboard) {
                return true;
            }
        }
        self.handle_viewing_key(key, mods, clipboard)
    }

    /// Keys of a viewer: scrolling, copying, leaving.
    fn handle_viewing_key(&mut self, key: Key, mods: Modifiers, clipboard: &mut dyn Clipboard) -> bool {
        let lh = self.lh();
        let page = lh.max(self.bounds.h - lh * 2.0);
        let ctrl_only = mods == Modifiers::CTRL;
        match key {
            Key::Down if mods.none() => self.scroll_by(0.0, lh * 3.0),
            Key::Up if mods.none() => self.scroll_by(0.0, -lh * 3.0),
            Key::Down if ctrl_only => self.scroll_by(0.0, lh),
            Key::Up if ctrl_only => self.scroll_by(0.0, -lh),
            Key::Left if mods.none() => self.scroll_by(-self.font().char_width * 8.0, 0.0),
            Key::Right if mods.none() => self.scroll_by(self.font().char_width * 8.0, 0.0),
            Key::PageDown if mods.none() => self.scroll_to_y(self.origin.y + page, true),
            Key::PageUp if mods.none() => self.scroll_to_y(self.origin.y - page, true),
            Key::Space if mods.none() || mods == Modifiers::SHIFT => {
                self.scroll_to_y(self.origin.y + if mods.shift { -page } else { page }, true)
            }
            Key::Home if mods.none() || ctrl_only => self.scroll_to_y(0.0, true),
            Key::End if mods.none() || ctrl_only => self.scroll_to_y(f32::MAX, true),
            Key::Escape if mods.none() => self.actions.push(CanvasAction::Exit),
            Key::Char('a') if ctrl_only => self.select_all(),
            Key::Char('c') | Key::Insert if ctrl_only => self.copy(clipboard),
            _ => return false,
        }
        if matches!(key, Key::Down | Key::Up | Key::PageDown | Key::PageUp | Key::Space | Key::Home | Key::End) {
            self.actions.push(CanvasAction::UserScrolled);
        }
        true
    }

    /// Keys that edit, or move the caret. Returns false for the ones that do nothing here.
    fn handle_editing_key(&mut self, ui: &mut Ui, key: Key, mods: Modifiers, clipboard: &mut dyn Clipboard) -> bool {
        let Some(sel) = self.edit_selection() else { return false };
        match key {
            // Esc drops the selection; with none, it leaves the viewer as before.
            Key::Escape if mods.none() => {
                if sel.anchor == sel.head {
                    return false;
                }
                self.set_caret(sel.head);
                return true;
            }
            // Deletes the lines the caret or the selection is on (Ctrl+Y, as the IDE's Windows keymap has it; redo
            // is Ctrl+Shift+Z).
            Key::Char('y') if mods == Modifiers::CTRL => {
                self.delete_lines(ui);
                return true;
            }
            Key::Up | Key::Down if mods == Modifiers::CTRL => {
                let lh = self.lh();
                self.scroll_by(0.0, if key == Key::Up { -lh } else { lh });
                return true;
            }
            _ => {}
        }
        let Some(command) = EditCommand::from_key(key, mods) else { return false };
        self.edit_command(ui, command, clipboard);
        true
    }

    fn edit_command(&mut self, ui: &mut Ui, command: EditCommand, clipboard: &mut dyn Clipboard) {
        match command {
            EditCommand::Left { word, select } => {
                if word {
                    self.move_caret(|c, s| c.word_before(s.head), select, None);
                } else {
                    self.move_caret(|c, s| c.character_before(s.head), select, Some(|s: Selection| s.anchor.min(s.head)));
                }
            }
            EditCommand::Right { word, select } => {
                if word {
                    self.move_caret(|c, s| c.word_after(s.head), select, None);
                } else {
                    self.move_caret(|c, s| c.character_after(s.head), select, Some(|s: Selection| s.anchor.max(s.head)));
                }
            }
            EditCommand::Up { select } => self.move_vertically(ui, -1, select),
            EditCommand::Down { select } => self.move_vertically(ui, 1, select),
            // Home goes to the start of the line's text first, as in the IDE.
            EditCommand::Home { select } => self.move_caret(|c, s| c.smart_line_start(s.head), select, None),
            EditCommand::End { select } => self.move_caret(|c, s| TextPosition::new(s.head.row, c.line_len(s.head.row)), select, None),
            EditCommand::DocStart { select } => self.move_caret(|_, _| TextPosition::new(0, 0), select, None),
            EditCommand::DocEnd { select } => self.move_caret(|c, _| c.document_end(), select, None),
            // Page Up moves the caret, not just the view.
            EditCommand::PageUp { select } => self.move_page(ui, -1, select),
            EditCommand::PageDown { select } => self.move_page(ui, 1, select),
            EditCommand::Backspace { word } => {
                if word {
                    self.delete_selection_or(|c, p| c.word_before(p));
                } else {
                    self.delete_selection_or(|c, p| c.backspace_target(p));
                }
            }
            EditCommand::Delete { word } => {
                if word {
                    self.delete_selection_or(|c, p| c.word_after(p));
                } else {
                    self.delete_selection_or(|c, p| c.character_after(p));
                }
            }
            EditCommand::SelectAll => self.select_all(),
            EditCommand::Copy => self.copy(clipboard),
            EditCommand::Cut => self.cut(ui, clipboard),
            EditCommand::Paste => self.paste(clipboard),
            EditCommand::Undo => self.undo(),
            EditCommand::Redo => self.redo(),
            EditCommand::Newline => self.insert_newline(),
            EditCommand::Tab { back: false } => self.insert_tab(),
            EditCommand::Tab { back: true } => self.indent_lines(true),
        }
    }

    fn handle_text(&mut self, text: &str) -> bool {
        if self.editor.is_none() {
            return false;
        }
        let text: String = text.chars().filter(|c| !c.is_control()).collect();
        if let Some((_, at)) = self.remove_composition() {
            self.set_caret(at);
        }
        if text.is_empty() || self.edit_selection().is_none() {
            return self.edit_selection().is_some();
        }
        self.type_text(&text);
        true
    }

    /// An input method's composition so far; empty text ends it, leaving nothing behind (a committed result
    /// arrives as text right after).
    fn handle_preedit(&mut self, text: &str, caret: Option<usize>) -> bool {
        let Some(marked) = self.editor.as_ref().map(|e| e.marked().is_some()) else { return false };
        if !marked {
            if text.is_empty() {
                return false;
            }
            // Composing over a selection replaces it, as typing would.
            if let Some(sel) = self.edit_selection() {
                if sel.anchor != sel.head {
                    self.replace_text(sel.anchor, sel.head, "", EditKind::Typing, None);
                }
            }
        }
        let Some(sel) = self.edit_selection() else { return false };
        let start = self
            .editor
            .as_ref()
            .and_then(|e| e.marked())
            .map_or(sel.anchor.min(sel.head), |(row, range)| TextPosition::new(*row, range.start));
        let owned = text.to_string();
        self.perform_edit(start.row, |e, d| vec![e.set_marked(d, &owned, start)]);
        match self.editor.as_ref().and_then(|e| e.marked().cloned()) {
            Some((row, range)) => {
                let offset = caret.unwrap_or(range.len()).min(range.len());
                let column = text_editing::floor_boundary(self.line(row), range.start + offset);
                self.set_caret(TextPosition::new(row, column));
            }
            None => self.set_caret(start),
        }
        self.reveal_pending = true;
        true
    }

    // MARK: Positions

    fn document_end(&self) -> TextPosition {
        TextPosition::new(self.last_row(), self.line_len(self.last_row()))
    }

    fn character_before(&self, p: TextPosition) -> TextPosition {
        if p.column > 0 {
            return TextPosition::new(p.row, text_editing::character_boundary(self.line(p.row), p.column, false));
        }
        if p.row > 0 {
            TextPosition::new(p.row - 1, self.line_len(p.row - 1))
        } else {
            p
        }
    }

    fn character_after(&self, p: TextPosition) -> TextPosition {
        if p.column < self.line_len(p.row) {
            return TextPosition::new(p.row, text_editing::character_boundary(self.line(p.row), p.column, true));
        }
        if p.row < self.last_row() {
            TextPosition::new(p.row + 1, 0)
        } else {
            p
        }
    }

    fn word_before(&self, p: TextPosition) -> TextPosition {
        if p.column == 0 {
            return self.character_before(p);
        }
        TextPosition::new(p.row, text_editing::word_boundary(self.line(p.row), p.column, false))
    }

    fn word_after(&self, p: TextPosition) -> TextPosition {
        if p.column >= self.line_len(p.row) {
            return self.character_after(p);
        }
        TextPosition::new(p.row, text_editing::word_boundary(self.line(p.row), p.column, true))
    }

    /// The first character of the line's text, or the very start of the line when the caret is already there.
    fn smart_line_start(&self, p: TextPosition) -> TextPosition {
        let indent = text_editing::indentation(self.line(p.row)).len();
        TextPosition::new(p.row, if p.column == indent { 0 } else { indent })
    }

    /// Where Backspace deletes back to: in indentation made of spaces, the previous indent stop.
    fn backspace_target(&mut self, p: TextPosition) -> TextPosition {
        let unit = self.indent_unit();
        if p.column == 0 || unit == "\t" {
            return self.character_before(p);
        }
        let line = self.line(p.row);
        let column = p.column.min(line.len());
        if !line.as_bytes()[..column].iter().all(|&b| b == b' ') {
            return self.character_before(p);
        }
        let width = unit.len().max(1);
        TextPosition::new(p.row, column - ((column - 1) % width + 1))
    }

    // MARK: Selection

    pub fn set_caret(&mut self, p: TextPosition) {
        self.set_selection(p, p);
    }

    /// Selects between two positions of the right side's lines.
    pub fn set_selection(&mut self, anchor: TextPosition, head: TextPosition) {
        let lines = self.lines();
        let (a, b) = (text_editing::clamp(anchor, lines), text_editing::clamp(head, lines));
        self.selection = Some(TextSel { pane: Side::Right, anchor: self.row_position(a), head: self.row_position(b) });
        self.blink_from = Instant::now();
    }

    /// Puts the caret on a line of the right side, e.g. at a change F7 moved to or a line jumped to.
    pub fn place_caret(&mut self, line: usize, column: usize) {
        if self.editor.is_none() || self.lines().is_empty() {
            return;
        }
        self.commit_composition();
        if let Some(e) = self.editor.as_mut() {
            e.close_group();
        }
        self.set_caret(TextPosition::new(line, column));
        self.goal_x = None;
    }

    fn move_caret(
        &mut self,
        target: impl FnOnce(&mut Self, Selection) -> TextPosition,
        extend: bool,
        collapse: Option<fn(Selection) -> TextPosition>,
    ) {
        let Some(sel) = self.edit_selection() else { return };
        if let Some(e) = self.editor.as_mut() {
            e.close_group();
        }
        let head = match collapse {
            Some(collapse) if !extend && sel.anchor != sel.head => collapse(sel),
            _ => target(self, sel),
        };
        if extend {
            self.set_selection(sel.anchor, head);
        } else {
            self.set_caret(head);
        }
        self.goal_x = None;
        self.reveal_pending = true;
    }

    /// Up and down keep to the column the caret started in, as far as the lines passed allow.
    fn move_vertically(&mut self, ui: &mut Ui, delta: isize, extend: bool) {
        let Some(sel) = self.edit_selection() else { return };
        if let Some(e) = self.editor.as_mut() {
            e.close_group();
        }
        let head = sel.head;
        let x = self.goal_x.unwrap_or_else(|| self.x_of(ui, Side::Right, head.row, head.column));
        let row = head.row as isize + delta;
        let target = if row < 0 {
            TextPosition::new(0, 0)
        } else if row as usize > self.last_row() {
            self.document_end()
        } else {
            let row = row as usize;
            TextPosition::new(row, self.column_at(ui, Side::Right, row, x))
        };
        if extend {
            self.set_selection(sel.anchor, target);
        } else {
            self.set_caret(target);
        }
        self.goal_x = Some(x);
        self.reveal_pending = true;
    }

    fn move_page(&mut self, ui: &mut Ui, direction: isize, extend: bool) {
        let page = self.visible_line_count().saturating_sub(2).max(1);
        self.scroll_by(0.0, direction as f32 * page as f32 * self.lh());
        self.move_vertically(ui, direction * page as isize, extend);
    }

    /// The caret's line with its line break, when nothing is selected (what Ctrl+C and Ctrl+X take then).
    pub fn caret_line_text(&self) -> Option<String> {
        let sel = self.edit_selection()?;
        if sel.anchor != sel.head || sel.head.row >= self.lines().len() {
            return None;
        }
        Some(format!("{}\n", self.line(sel.head.row)))
    }

    pub fn select_all(&mut self) {
        let editable = self.editor.is_some() && (self.mode() != Mode::SideBySide || self.selection.is_none_or(|s| s.pane != Side::Left));
        if editable {
            self.commit_composition();
            let last = self.lines().len();
            if last == 0 {
                return;
            }
            self.set_selection(TextPosition::new(0, 0), TextPosition::new(last - 1, self.line_len(last - 1)));
            return;
        }
        let side = self.selection.map_or(Side::Right, |s| s.pane);
        let count = self.pane_row_count(side);
        if count == 0 {
            return;
        }
        let end = self.row_text(side, count - 1).len();
        self.selection = Some(TextSel { pane: side, anchor: TextPosition::new(0, 0), head: TextPosition::new(count - 1, end) });
    }

    // MARK: Changing the text

    /// Runs an edit of the right side. `anchor_line` (a line above the edit, or its first line) stays where it is
    /// on screen: side by side the panes are lined up through the changes, and a change growing by a line must
    /// not make the text being typed jump.
    pub fn perform_edit(&mut self, anchor_line: usize, edit: impl FnOnce(&mut EditorSession, &mut DiffDocument) -> Vec<Outcome>) {
        let lh = self.lh();
        let before = self.edit_row(anchor_line) as f32 * lh - self.resolve().right;
        let outcomes = match (self.editor.as_mut(), self.doc.as_mut()) {
            (Some(e), Some(d)) => edit(e, d),
            _ => return,
        };
        if outcomes.is_empty() {
            return;
        }
        self.text_did_change(&outcomes);
        if self.mode() != Mode::SideBySide || anchor_line >= self.lines().len() {
            return;
        }
        for _ in 0..4 {
            let after = self.edit_row(anchor_line) as f32 * lh - self.resolve().right;
            if (after - before).abs() < 0.5 {
                break;
            }
            self.scroll_by(0.0, after - before);
        }
    }

    fn text_did_change(&mut self, outcomes: &[Outcome]) {
        // Search hits and a "go to line" tint describe the text as it was.
        self.matches.clear();
        self.matches_by_line = [Default::default(), Default::default()];
        self.current_match = None;
        self.focus_line = None;
        let hunks = self.doc.as_ref().map_or(0, |d| d.hunks().len());
        if self.focused_hunk.is_some_and(|h| h >= hunks) {
            self.focused_hunk = None;
        }
        if let Some(a) = self.annotations.as_mut() {
            // Changed lines are not committed any more; the rest keep their commit (the next save blames again).
            for outcome in outcomes {
                a.apply(outcome.change);
            }
        }
        self.edit_count += 1;
        self.rebuild_hunk_rows();
        self.clamp_scroll();
    }

    /// New hunks for the text on screen (the real diff, back from git): the line with the caret, or the top
    /// line, stays where it is on screen while the two sides line up again around it.
    pub fn replace_hunks(&mut self, hunks: Vec<Hunk>) {
        if self.doc.is_none() {
            return;
        }
        let lh = self.lh();
        let caret = self.edit_selection();
        let anchor_line = caret.map_or(((self.resolve().right + lh / 2.0) / lh).max(0.0) as usize, |c| c.head.row);
        let before = self.edit_row(anchor_line) as f32 * lh - self.resolve().right;
        if let Some(doc) = self.doc.as_mut() {
            doc.set_hunks(hunks);
        }
        let count = self.doc.as_ref().map_or(0, |d| d.hunks().len());
        if self.focused_hunk.is_some_and(|h| h >= count) {
            self.focused_hunk = None;
        }
        self.rebuild_hunk_rows();
        if let Some(c) = caret {
            self.selection = Some(TextSel { pane: Side::Right, anchor: self.row_position(c.anchor), head: self.row_position(c.head) });
        }
        self.clamp_scroll();
        for _ in 0..4 {
            let after = self.edit_row(anchor_line) as f32 * lh - self.resolve().right;
            if (after - before).abs() < 0.5 {
                break;
            }
            self.scroll_by(0.0, after - before);
        }
    }

    /// Replaces text, records it for undo; the caret goes after the new text unless told otherwise.
    fn replace_text(&mut self, from: TextPosition, to: TextPosition, text: &str, kind: EditKind, caret: Option<TextPosition>) -> Option<Outcome> {
        let before = self.edit_selection()?;
        let mut result = None;
        self.perform_edit(from.min(to).row, |e, d| {
            let outcome = e.replace(d, from, to, text, kind, before);
            result = Some(outcome.clone());
            vec![outcome]
        });
        let result = result?;
        self.set_caret(caret.unwrap_or(result.end));
        self.goal_x = None;
        self.reveal_pending = true;
        self.actions.push(CanvasAction::Edited);
        Some(result)
    }

    /// An edit a command makes for the user (a change rolled back, a conflict resolved): one undo step, with the
    /// caret at `caret`.
    pub fn apply_command_edit(&mut self, from: TextPosition, to: TextPosition, text: &str, caret: Option<TextPosition>) {
        if self.editor.is_none() {
            return;
        }
        self.commit_composition();
        if self.edit_selection().is_none() {
            self.set_caret(from);
        }
        if let Some(e) = self.editor.as_mut() {
            e.close_group();
        }
        self.replace_text(from, to, text, EditKind::Other, caret);
    }

    /// Typed text, over the selection if there is one.
    fn type_text(&mut self, text: &str) {
        let Some(sel) = self.edit_selection() else { return };
        let (lower, upper) = (sel.anchor.min(sel.head), sel.anchor.max(sel.head));
        // A closing brace typed into a line of nothing but indentation steps out one level, as in the IDE.
        if lower == upper && text == "}" {
            let unit = self.indent_unit();
            let line = self.line(lower.row);
            let column = lower.column.min(line.len());
            let (before, after) = line.split_at(column);
            let blank = |s: &str| s.bytes().all(|b| b == b' ' || b == b'\t');
            if before.ends_with(&unit) && blank(before) && blank(after) {
                self.replace_text(TextPosition::new(lower.row, column - unit.len()), lower, text, EditKind::Typing, None);
                return;
            }
        }
        let kind = if text.contains(['\n', '\r']) { EditKind::Other } else { EditKind::Typing };
        self.replace_text(lower, upper, text, kind, None);
    }

    fn delete_selection_or(&mut self, target: impl FnOnce(&mut Self, TextPosition) -> TextPosition) {
        let Some(sel) = self.edit_selection() else { return };
        if sel.anchor != sel.head {
            self.replace_text(sel.anchor, sel.head, "", EditKind::Other, None);
            return;
        }
        let to = target(self, sel.head);
        if to != sel.head {
            self.replace_text(sel.head, to, "", EditKind::Deleting, None);
        }
    }

    /// Enter keeps the indentation of the line, one level deeper after an opening bracket; between a pair of
    /// brackets the closing one moves to a line of its own.
    fn insert_newline(&mut self) {
        let Some(sel) = self.edit_selection() else { return };
        let mut lower = sel.anchor.min(sel.head);
        let mut upper = sel.anchor.max(sel.head);
        let unit = self.indent_unit();
        let line = self.line(lower.row).to_string();
        let column = lower.column.min(line.len());
        let before = &line[..column];
        let mut indent = text_editing::indentation(&line);
        if indent.len() > column {
            indent = before;
        }
        let indent = indent.to_string();
        let trimmed = before.trim_matches([' ', '\t']);
        // A line left with nothing but indentation is left empty.
        if trimmed.is_empty() {
            lower = TextPosition::new(lower.row, 0);
        }
        let closer = match trimmed.chars().last() {
            Some('{') => Some('}'),
            Some('[') => Some(']'),
            Some('(') => Some(')'),
            _ => None,
        };
        if let Some(closer) = closer {
            let upper_line = self.line(upper.row);
            let rest = &upper_line[upper.column.min(upper_line.len())..];
            let spaces = rest.len() - rest.trim_start_matches([' ', '\t']).len();
            if rest[spaces..].starts_with(closer) {
                upper = TextPosition::new(upper.row, upper.column + spaces);
                let text = format!("\n{indent}{unit}\n{indent}");
                let caret = TextPosition::new(lower.row + 1, indent.len() + unit.len());
                self.replace_text(lower, upper, &text, EditKind::Typing, Some(caret));
                return;
            }
            self.replace_text(lower, upper, &format!("\n{indent}{unit}"), EditKind::Typing, None);
            return;
        }
        self.replace_text(lower, upper, &format!("\n{indent}"), EditKind::Typing, None);
    }

    /// Tab indents: to the next tab stop at the caret, or every selected line by one level.
    fn insert_tab(&mut self) {
        let Some(sel) = self.edit_selection() else { return };
        if sel.anchor != sel.head {
            self.indent_lines(false);
            return;
        }
        let p = sel.head;
        let unit = self.indent_unit();
        let text = if unit == "\t" {
            unit
        } else {
            let width = unit.len().max(1);
            let visual = text_editing::visual_column(self.line(p.row), p.column);
            " ".repeat(width - visual % width)
        };
        self.replace_text(p, p, &text, EditKind::Typing, None);
    }

    /// Indents or outdents every line the selection touches by one level, as one undo step.
    fn indent_lines(&mut self, outdent: bool) {
        let Some(sel) = self.edit_selection() else { return };
        let (lower, upper) = (sel.anchor.min(sel.head), sel.anchor.max(sel.head));
        let mut last = upper.row;
        if upper.row > lower.row && upper.column == 0 {
            last -= 1;
        }
        let unit = self.indent_unit();
        let width = if unit == "\t" { 4 } else { unit.len().max(1) };
        let mut edits: Vec<(TextPosition, TextPosition, String)> = Vec::new();
        let mut shifts: std::collections::HashMap<usize, isize> = Default::default();
        // Bottom up, although no line comes or goes: each edit is given in the text as the ones before it left it.
        for row in (lower.row..=last).rev() {
            let line = self.line(row).as_bytes();
            if outdent {
                let remove = if line.first() == Some(&b'\t') { 1 } else { line.iter().take(width).take_while(|&&b| b == b' ').count() };
                if remove == 0 {
                    continue;
                }
                edits.push((TextPosition::new(row, 0), TextPosition::new(row, remove), String::new()));
                shifts.insert(row, -(remove as isize));
            } else {
                // Blank lines inside a block stay blank.
                if line.is_empty() && lower.row != last {
                    continue;
                }
                edits.push((TextPosition::new(row, 0), TextPosition::new(row, 0), unit.clone()));
                shifts.insert(row, unit.len() as isize);
            }
        }
        if edits.is_empty() {
            return;
        }
        let whole = sel.anchor != sel.head;
        let shifted = |p: TextPosition| -> TextPosition {
            match shifts.get(&p.row) {
                None => p,
                Some(&d) if d < 0 => TextPosition::new(p.row, (p.column as isize + d).max(0) as usize),
                // A selection of whole lines keeps starting at the start of its first line.
                Some(_) if p.column == 0 && whole => p,
                Some(&d) => TextPosition::new(p.row, p.column + d as usize),
            }
        };
        let after = Selection { anchor: shifted(sel.anchor), head: shifted(sel.head) };
        self.perform_edit(lower.row, |e, d| e.replace_many(d, &edits, sel, after));
        self.set_selection(after.anchor, after.head);
        self.goal_x = None;
        self.reveal_pending = true;
        self.actions.push(CanvasAction::Edited);
    }

    /// Deletes the lines the caret or the selection is on.
    pub fn delete_lines(&mut self, ui: &mut Ui) {
        let Some(sel) = self.edit_selection() else { return };
        let (lower, upper) = (sel.anchor.min(sel.head), sel.anchor.max(sel.head));
        let mut last = upper.row;
        if upper.row > lower.row && upper.column == 0 {
            last -= 1;
        }
        let x = self.goal_x.unwrap_or_else(|| self.x_of(ui, Side::Right, sel.head.row, sel.head.column));
        let (from, to) = if last < self.last_row() {
            (TextPosition::new(lower.row, 0), TextPosition::new(last + 1, 0))
        } else if lower.row > 0 {
            (TextPosition::new(lower.row - 1, self.line_len(lower.row - 1)), TextPosition::new(last, self.line_len(last)))
        } else {
            (TextPosition::new(0, 0), TextPosition::new(last, self.line_len(last)))
        };
        if from == to {
            return;
        }
        self.replace_text(from, to, "", EditKind::Other, None);
        // The caret keeps its column, on the line that moved up into the deleted one's place.
        let row = lower.row.min(self.last_row());
        let column = self.column_at(ui, Side::Right, row, x);
        self.set_caret(TextPosition::new(row, column));
        self.goal_x = Some(x);
        self.reveal_pending = true;
    }

    // MARK: Clipboard

    pub fn copy(&mut self, clipboard: &mut dyn Clipboard) {
        match self.selected_text() {
            Some(text) => {
                clipboard.set(&text);
                self.line_clipboard = None;
            }
            None => {
                // With nothing selected in the text being edited, copy takes the whole line, as in the IDE.
                if let Some(line) = self.caret_line_text() {
                    clipboard.set(&line);
                    self.line_clipboard = Some(line);
                }
            }
        }
    }

    pub fn cut(&mut self, ui: &mut Ui, clipboard: &mut dyn Clipboard) {
        if self.editor.is_none() {
            return;
        }
        self.commit_composition();
        let Some(sel) = self.edit_selection() else { return };
        if sel.anchor == sel.head {
            // Nothing selected: the whole line, as in the IDE.
            let Some(line) = self.caret_line_text() else { return };
            clipboard.set(&line);
            self.line_clipboard = Some(line);
            self.delete_lines(ui);
            return;
        }
        let text = text_editing::text(self.lines(), sel.anchor.min(sel.head), sel.anchor.max(sel.head));
        clipboard.set(&text);
        self.line_clipboard = None;
        self.replace_text(sel.anchor, sel.head, "", EditKind::Other, None);
    }

    pub fn paste(&mut self, clipboard: &mut dyn Clipboard) {
        if self.editor.is_none() || self.edit_selection().is_none() {
            return;
        }
        let Some(text) = clipboard.get().filter(|t| !t.is_empty()) else { return };
        let text = text.replace("\r\n", "\n");
        self.commit_composition();
        let Some(sel) = self.edit_selection() else { return };
        if sel.anchor == sel.head && self.line_clipboard.as_deref() == Some(text.as_str()) {
            // A line copied whole goes in as a line, above the caret's, and the caret stays on its text.
            let start = TextPosition::new(sel.head.row, 0);
            let added = text_editing::lines_of(&text).len() - 1;
            let caret = TextPosition::new(sel.head.row + added, sel.head.column);
            self.replace_text(start, start, &text, EditKind::Other, Some(caret));
            return;
        }
        self.replace_text(sel.anchor, sel.head, &text, EditKind::Other, None);
    }

    // MARK: Undo

    pub fn can_undo(&self) -> bool {
        self.editor.as_ref().is_some_and(|e| e.can_undo() || e.marked().is_some())
    }

    pub fn can_redo(&self) -> bool {
        self.editor.as_ref().is_some_and(|e| e.can_redo())
    }

    pub fn undo(&mut self) {
        if self.editor.is_none() {
            return;
        }
        self.commit_composition();
        if self.editor.as_ref().is_some_and(|e| e.can_undo()) {
            self.step_history(|e, d| e.undo(d));
        }
    }

    pub fn redo(&mut self) {
        if self.editor.is_none() {
            return;
        }
        self.commit_composition();
        if self.editor.as_ref().is_some_and(|e| e.can_redo()) {
            self.step_history(|e, d| e.redo(d));
        }
    }

    fn step_history(&mut self, step: impl FnOnce(&mut EditorSession, &mut DiffDocument) -> Option<(Vec<Outcome>, Selection)>) {
        let anchor = self.edit_selection().map_or(0, |s| s.anchor.min(s.head).row);
        let mut restored = None;
        self.perform_edit(anchor, |e, d| match step(e, d) {
            Some((outcomes, selection)) => {
                restored = Some(selection);
                outcomes
            }
            None => Vec::new(),
        });
        let Some(restored) = restored else { return };
        self.set_selection(restored.anchor, restored.head);
        self.goal_x = None;
        self.reveal_pending = true;
        self.actions.push(CanvasAction::Edited);
    }

    // MARK: Composition

    /// Ends a composition, keeping what was composed as typed text (a click elsewhere, the canvas losing the
    /// keyboard, a save). The input method is told to drop its own copy.
    pub fn commit_composition(&mut self) {
        let Some((text, at)) = self.remove_composition() else { return };
        self.ime_cancel = true;
        self.set_caret(at);
        if !text.is_empty() {
            self.type_text(&text);
        }
    }

    /// Takes the composition out of the text, unrecorded. Returns what it was and where it sat.
    fn remove_composition(&mut self) -> Option<(String, TextPosition)> {
        let row = self.editor.as_ref()?.marked()?.0;
        let mut taken = None;
        self.perform_edit(row, |e, d| match e.remove_marked(d) {
            Some((text, at, outcome)) => {
                taken = Some((text, at));
                vec![outcome]
            }
            None => Vec::new(),
        });
        taken
    }

    pub fn has_composition(&self) -> bool {
        self.editor.as_ref().is_some_and(|e| e.marked().is_some())
    }

    // MARK: Showing the caret

    /// Scrolls just enough to bring the caret into view, with a little room around it.
    pub(super) fn reveal_caret(&mut self, ui: &mut Ui) {
        let Some(sel) = self.selection else { return };
        if self.editor.is_none() || (self.mode() == Mode::SideBySide && sel.pane != Side::Right) {
            return;
        }
        let lh = self.lh();
        let margin = self.font().char_width * 4.0;
        let width = self.text_rect(Side::Right).w;
        let line = self.line_position(sel.head);
        let x_text = super::TEXT_INSET + self.x_of(ui, Side::Right, line.row, line.column);
        for _ in 0..4 {
            let scroll = self.resolve();
            let y = sel.head.row as f32 * lh - scroll.right;
            let dy = if y < lh * 0.5 {
                y - lh * 0.5
            } else if y + lh * 1.5 > self.bounds.h {
                y + lh * 1.5 - self.bounds.h
            } else {
                0.0
            };
            let x = x_text - scroll.x;
            let dx = if x < margin && scroll.x > 0.0 {
                (-scroll.x).max(x - margin)
            } else if x > width - margin {
                x - (width - margin)
            } else {
                0.0
            };
            if dy.abs() < 0.5 && dx.abs() < 0.5 {
                break;
            }
            self.scroll_by(dx, dy);
        }
    }
}
