//! The editing state of the file on screen: undo history, what has been written, and what it takes to write the
//! text back exactly. The text itself is the document's right buffer, which the canvas draws, so an edit changes
//! what is on screen with no copy in between. Methods take the document they edit.

use std::ops::Range;
use std::path::PathBuf;
use std::time::Instant;

use crate::diff::{join_repo_path, Content, DiffDocument};
use crate::text_codec::{self, EditableText, TextFormat};
use crate::text_editing::{self, LineEdit, TextPosition};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Selection {
    pub anchor: TextPosition,
    pub head: TextPosition,
}

impl Selection {
    pub fn caret(at: TextPosition) -> Self {
        Selection { anchor: at, head: at }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditKind {
    Typing,
    Deleting,
    Other,
}

/// One replacement, enough to make it again or take it back.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Change {
    from: TextPosition,
    /// End of the replaced text, before the change.
    removed_end: TextPosition,
    /// End of the new text, after the change.
    inserted_end: TextPosition,
    removed: String,
    inserted: String,
}

/// What one Ctrl+Z takes back: a run of typing, a paste, an indent of several lines…
#[derive(Clone, Debug)]
struct Group {
    changes: Vec<Change>,
    kind: EditKind,
    selection_before: Selection,
    selection_after: Selection,
    /// Whether the last line ended with a line break before and after, when the group changed that.
    final_newline: Option<(bool, bool)>,
    time: Instant,
}

/// What an edit did to the lines, for redrawing them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub raw: LineEdit,
    pub change: LineEdit,
    /// Lines whose colours were recomputed (an opened comment can recolour many).
    pub recoloured: Range<usize>,
    pub end: TextPosition,
}

/// An undo history, for a later session of the same text to take up: the file opened again, or read
/// again because the diff's base moved.
#[derive(Clone, Debug)]
pub struct History {
    undo: Vec<Group>,
    redo: Vec<Group>,
    /// The bytes the history ends with.
    pub fingerprint: Option<[u8; 32]>,
}

/// Typing that could not be written, for a later session of the same file to carry on with.
#[derive(Clone, Debug)]
pub struct Unsaved {
    lines: Vec<String>,
    format: TextFormat,
    crlf: Option<Vec<bool>>,
    undo: Vec<Group>,
    redo: Vec<Group>,
    /// What the file held when it was last read or written.
    fingerprint: Option<[u8; 32]>,
}

pub const UNDO_LIMIT: usize = 300;

#[derive(Debug)]
pub struct EditorSession {
    pub root: PathBuf,
    pub path: String,
    format: TextFormat,
    crlf: Option<Vec<bool>>,
    /// The left side's bytes, which the text is diffed against as it changes.
    pub base: Option<Vec<u8>>,
    pub ignore_whitespace: bool,
    /// What the file held when it was last read or written here.
    disk_fingerprint: Option<[u8; 32]>,
    /// Counts changes to the text, to tell whether what is on screen has been written and whether a diff is current.
    version: u64,
    saved_version: u64,
    /// Edited here at some point since it was opened.
    has_edits: bool,
    undo_stack: Vec<Group>,
    redo_stack: Vec<Group>,
    /// The last group may still take more typing.
    group_open: bool,
    /// Text an input method is composing: (row, byte range). It sits in the buffer so that it is drawn in place,
    /// but it is not part of the file: nothing is saved or diffed while it is there.
    marked: Option<(usize, Range<usize>)>,
    indent_unit: Option<String>,
    pub is_plain_file: bool,
}

impl EditorSession {
    /// A session for the document's right side, when it can be edited.
    pub fn new(document: &DiffDocument, root: PathBuf, path: String, ignore_whitespace: bool) -> Option<Self> {
        let info = document.editing.as_ref()?;
        if info.read_only_reason.is_some() || !matches!(document.content, Content::Text) || document.right.is_empty() {
            return None;
        }
        Some(EditorSession {
            root,
            path,
            format: info.format.clone(),
            crlf: info.crlf.clone(),
            base: info.base.clone(),
            ignore_whitespace,
            disk_fingerprint: info.fingerprint,
            version: 0,
            saved_version: 0,
            has_edits: false,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            group_open: false,
            marked: None,
            indent_unit: None,
            is_plain_file: document.is_plain_file,
        })
    }

    pub fn full_path(&self) -> PathBuf {
        join_repo_path(&self.root, &self.path)
    }

    pub fn format(&self) -> &TextFormat {
        &self.format
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn is_dirty(&self) -> bool {
        self.version != self.saved_version
    }

    pub fn has_edits(&self) -> bool {
        self.has_edits
    }

    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    pub fn marked(&self) -> Option<&(usize, Range<usize>)> {
        self.marked.as_ref()
    }

    /// What one press of Tab inserts in this file.
    pub fn indent_unit(&mut self, doc: &DiffDocument) -> &str {
        self.indent_unit.get_or_insert_with(|| text_editing::indent_unit(doc.right.lines()))
    }

    // MARK: Editing

    /// Replaces text and records it for undo. `selection` is what undoing restores.
    pub fn replace(
        &mut self,
        doc: &mut DiffDocument,
        from: TextPosition,
        to: TextPosition,
        text: &str,
        kind: EditKind,
        selection: Selection,
    ) -> Outcome {
        let lines = doc.right.lines();
        let a = text_editing::clamp(from.min(to), lines);
        let b = text_editing::clamp(from.max(to), lines);
        let removed = text_editing::text(lines, a, b);
        let outcome = self.apply(doc, a, b, text);
        let end = outcome.end;
        self.record(
            Change { from: a, removed_end: b, inserted_end: end, removed, inserted: text.to_string() },
            kind,
            selection,
            Selection::caret(end),
        );
        outcome
    }

    /// Several replacements as one undo step, each given in the text as it stands after the ones before it.
    pub fn replace_many(
        &mut self,
        doc: &mut DiffDocument,
        edits: &[(TextPosition, TextPosition, String)],
        selection: Selection,
        after: Selection,
    ) -> Vec<Outcome> {
        if edits.is_empty() {
            return Vec::new();
        }
        let mut changes = Vec::new();
        let mut outcomes = Vec::new();
        for (from, to, text) in edits {
            let lines = doc.right.lines();
            let a = text_editing::clamp((*from).min(*to), lines);
            let b = text_editing::clamp((*from).max(*to), lines);
            let removed = text_editing::text(lines, a, b);
            let outcome = self.apply(doc, a, b, text);
            changes.push(Change { from: a, removed_end: b, inserted_end: outcome.end, removed, inserted: text.clone() });
            outcomes.push(outcome);
        }
        self.close_group();
        self.push(Group {
            changes,
            kind: EditKind::Other,
            selection_before: selection,
            selection_after: after,
            final_newline: None,
            time: Instant::now(),
        });
        outcomes
    }

    /// Whether the file ends with a line break, as part of the last undo step (a rollback at the end of the file needs it).
    pub fn set_final_newline(&mut self, value: bool) {
        if value == self.format.final_newline {
            return;
        }
        let before = self.format.final_newline;
        self.format.final_newline = value;
        self.version += 1;
        if let Some(last) = self.undo_stack.last_mut() {
            last.final_newline = Some((before, value));
        }
    }

    /// Ends the run of typing that keystrokes are being added to: the caret moved, or something else happened.
    pub fn close_group(&mut self) {
        self.group_open = false;
    }

    pub fn undo(&mut self, doc: &mut DiffDocument) -> Option<(Vec<Outcome>, Selection)> {
        if self.marked.is_some() {
            return None;
        }
        let group = self.undo_stack.pop()?;
        self.close_group();
        let mut outcomes = Vec::new();
        for change in group.changes.iter().rev() {
            outcomes.push(self.apply(doc, change.from, change.inserted_end, &change.removed));
        }
        if let Some((before, _)) = group.final_newline {
            self.format.final_newline = before;
        }
        let selection = group.selection_before;
        self.redo_stack.push(group);
        Some((outcomes, selection))
    }

    pub fn redo(&mut self, doc: &mut DiffDocument) -> Option<(Vec<Outcome>, Selection)> {
        if self.marked.is_some() {
            return None;
        }
        let group = self.redo_stack.pop()?;
        self.close_group();
        let mut outcomes = Vec::new();
        for change in &group.changes {
            outcomes.push(self.apply(doc, change.from, change.removed_end, &change.inserted));
        }
        if let Some((_, after)) = group.final_newline {
            self.format.final_newline = after;
        }
        let selection = group.selection_after;
        self.undo_stack.push(group);
        Some((outcomes, selection))
    }

    /// Replaces the whole text with what is on disk now, as one undo step, so that Ctrl+Z brings back what was here.
    /// The text then matches the file again.
    pub fn take_disk_version(
        &mut self,
        doc: &mut DiffDocument,
        text: &EditableText,
        data: &[u8],
        selection: Selection,
    ) -> Outcome {
        let end = end_of_text(doc);
        let outcome = self.replace(doc, TextPosition::default(), end, &text.lines.join("\n"), EditKind::Other, selection);
        self.close_group();
        self.take_format(doc, text);
        self.did_write(data);
        outcome
    }

    /// The text was changed by something else, while nothing here was unsaved: it is simply the new text,
    /// and the history (which describes the old one) is dropped.
    pub fn replace_without_history(&mut self, doc: &mut DiffDocument, text: &EditableText, data: &[u8]) -> Outcome {
        let end = end_of_text(doc);
        let outcome = self.apply(doc, TextPosition::default(), end, &text.lines.join("\n"));
        self.take_format(doc, text);
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.group_open = false;
        self.did_write(data);
        outcome
    }

    fn take_format(&mut self, doc: &DiffDocument, text: &EditableText) {
        self.format = text.format.clone();
        self.crlf = text.crlf.clone().filter(|c| c.len() == doc.right.len());
    }

    // MARK: History across sessions

    /// The history, while what is on screen is exactly what the file holds.
    pub fn history(&self) -> Option<History> {
        if !self.has_edits || self.is_dirty() || self.marked.is_some() {
            return None;
        }
        Some(History { undo: self.undo_stack.clone(), redo: self.redo_stack.clone(), fingerprint: self.disk_fingerprint })
    }

    /// Takes up a history that ends with exactly this session's text. Returns whether it did.
    pub fn adopt(&mut self, history: History) -> bool {
        if self.has_edits || history.fingerprint != self.disk_fingerprint {
            return false;
        }
        self.undo_stack = history.undo;
        self.redo_stack = history.redo;
        self.has_edits = true;
        true
    }

    pub fn unsaved(&self, doc: &DiffDocument) -> Option<Unsaved> {
        if !self.is_dirty() || self.marked.is_some() {
            return None;
        }
        Some(Unsaved {
            lines: doc.right.lines().to_vec(),
            format: self.format.clone(),
            crlf: self.crlf.clone(),
            undo: self.undo_stack.clone(),
            redo: self.redo_stack.clone(),
            fingerprint: self.disk_fingerprint,
        })
    }

    /// Carries on with typing that could not be written. Over the text it was typed over, it simply comes back,
    /// unsaved, with its history. If the file has changed since, the file's text stays, and the typing is one Ctrl+Z
    /// away, as with any change underneath. Returns the outcomes and whether the file had changed.
    pub fn resume(&mut self, doc: &mut DiffDocument, unsaved: Unsaved, selection: Selection) -> (Vec<Outcome>, bool) {
        if self.has_edits || self.marked.is_some() {
            return (Vec::new(), false);
        }
        let disk_lines = doc.right.lines().join("\n");
        let disk_format = self.format.clone();
        let disk_crlf = self.crlf.clone();
        let end = end_of_text(doc);
        let mut outcomes = vec![self.apply(doc, TextPosition::default(), end, &unsaved.lines.join("\n"))];
        self.format = unsaved.format;
        self.crlf = unsaved.crlf;
        self.undo_stack = unsaved.undo;
        self.redo_stack = unsaved.redo;
        self.group_open = false;
        self.has_edits = true;
        if unsaved.fingerprint == self.disk_fingerprint {
            return (outcomes, false);
        }
        let end = end_of_text(doc);
        outcomes.push(self.replace(doc, TextPosition::default(), end, &disk_lines, EditKind::Other, selection));
        self.close_group();
        self.format = disk_format;
        self.crlf = disk_crlf;
        self.saved_version = self.version;
        (outcomes, true)
    }

    // MARK: Composition

    /// Shows `text` as the input method's composition, replacing the previous one, or starting at `position`.
    /// Not recorded, saved or diffed. An empty `text` ends the composition, leaving nothing behind.
    pub fn set_marked(&mut self, doc: &mut DiffDocument, text: &str, position: TextPosition) -> Outcome {
        // A composition is one line; an input method has no business putting line breaks in it.
        let flat = text.replace("\r\n", " ").replace(['\n', '\r'], " ");
        let (start, end) = match &self.marked {
            Some((row, range)) => (TextPosition::new(*row, range.start), TextPosition::new(*row, range.end)),
            None => {
                let p = text_editing::clamp(position, doc.right.lines());
                (p, p)
            }
        };
        let outcome = self.apply(doc, start, end, &flat);
        self.marked = (!flat.is_empty()).then(|| (start.row, start.column..start.column + flat.len()));
        outcome
    }

    /// Takes the composition out of the text again. Returns what it was and where it sat.
    pub fn remove_marked(&mut self, doc: &mut DiffDocument) -> Option<(String, TextPosition, Outcome)> {
        let (row, range) = self.marked.clone()?;
        let start = TextPosition::new(row, range.start);
        let end = TextPosition::new(row, range.end);
        let text = text_editing::text(doc.right.lines(), start, end);
        let outcome = self.apply(doc, start, end, "");
        self.marked = None;
        Some((text, start, outcome))
    }

    // MARK: Writing

    /// The file's new bytes. None while something is being composed (it is not part of the file yet),
    /// or if the text cannot be put in the file's encoding.
    pub fn encoded(&self, doc: &DiffDocument) -> Option<Vec<u8>> {
        if self.marked.is_some() {
            return None;
        }
        text_codec::encode(doc.right.lines(), &self.format, self.crlf.as_deref())
    }

    /// `data` is now what the file holds and what is on screen.
    pub fn did_write(&mut self, data: &[u8]) {
        self.disk_fingerprint = Some(text_codec::fingerprint(data));
        self.saved_version = self.version;
    }

    /// Typing was undone back to what the file holds: nothing to write after all.
    pub fn mark_saved(&mut self) {
        self.saved_version = self.version;
    }

    /// Whether `data` is what the file held when last read or written here.
    pub fn matches_disk(&self, data: &[u8]) -> bool {
        self.disk_fingerprint == Some(text_codec::fingerprint(data))
    }

    // MARK: Plumbing

    /// Changes the text without recording it, keeping everything that follows from the text in step:
    /// colours, line endings, the diff's hunks (moved until the real diff is back).
    fn apply(&mut self, doc: &mut DiffDocument, from: TextPosition, to: TextPosition, text: &str) -> Outcome {
        let result = doc.right.replace(from, to, text);
        let raw = result.raw;
        if let Some(crlf) = &mut self.crlf {
            if raw.start + raw.removed <= crlf.len() && raw.removed > 0 {
                // The last rewritten line keeps the ending it had; lines added in between get the file's usual one.
                let last = crlf[raw.start + raw.removed - 1];
                let mut added = vec![self.format.newline == "\r\n"; raw.inserted - 1];
                added.push(last);
                crlf.splice(raw.start..raw.start + raw.removed, added);
            }
        }
        let mut recoloured = raw.start..raw.start + raw.inserted;
        if doc.right.states.is_some() {
            let range = doc.right.rehighlight(doc.language, raw.start, raw.inserted);
            recoloured = recoloured.start.min(range.start)..recoloured.end.max(range.end);
        }
        if !doc.is_plain_file {
            let hunks = text_editing::adjust(doc.hunks(), result.change);
            doc.set_hunks(hunks);
        }
        self.version += 1;
        Outcome { raw, change: result.change, recoloured, end: result.end }
    }

    fn record(&mut self, change: Change, kind: EditKind, before: Selection, after: Selection) {
        let now = Instant::now();
        if self.group_open && kind != EditKind::Other {
            if let Some(last) = self.undo_stack.last_mut() {
                let joins = last.kind == kind
                    && now.duration_since(last.time).as_secs_f64() < 4.0
                    && last.changes.last().is_some_and(|previous| continues(previous, &change, kind));
                if joins {
                    last.changes.push(change);
                    last.selection_after = after;
                    last.time = now;
                    self.redo_stack.clear();
                    self.has_edits = true;
                    self.group_open = true;
                    return;
                }
            }
        }
        self.push(Group {
            changes: vec![change],
            kind,
            selection_before: before,
            selection_after: after,
            final_newline: None,
            time: now,
        });
        self.group_open = kind != EditKind::Other;
    }

    fn push(&mut self, group: Group) {
        self.undo_stack.push(group);
        if self.undo_stack.len() > UNDO_LIMIT {
            self.undo_stack.remove(0);
        }
        self.redo_stack.clear();
        self.has_edits = true;
    }
}

fn end_of_text(doc: &DiffDocument) -> TextPosition {
    let lines = doc.right.lines();
    let last = lines.len().saturating_sub(1);
    TextPosition::new(last, lines.get(last).map_or(0, String::len))
}

/// Typing goes on where the last keystroke left the caret; deleting backwards ends where the last deletion
/// began, deleting forwards begins where it began.
fn continues(previous: &Change, next: &Change, kind: EditKind) -> bool {
    match kind {
        EditKind::Typing => next.removed.is_empty() && next.from == previous.inserted_end && !next.inserted.contains('\n'),
        EditKind::Deleting => {
            next.inserted.is_empty() && (next.removed_end == previous.from || next.from == previous.from)
        }
        EditKind::Other => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::{EditInfo, Hunk, TextBuffer};
    use crate::syntax;

    fn doc(left: &[&str], right: &[&str]) -> DiffDocument {
        let l = TextBuffer::new(left.iter().map(|s| s.to_string()).collect());
        let lines: Vec<String> = right.iter().map(|s| s.to_string()).collect();
        let (spans, states) = syntax::highlight_tracking(&lines, &syntax::KOTLIN);
        let mut r = TextBuffer::new(lines);
        r.spans = spans;
        r.states = Some(states);
        let mut d = DiffDocument::new(l, r, vec![Hunk::lines(1, 1, 1, 1)], &syntax::KOTLIN);
        d.editing = Some(EditInfo { fingerprint: Some([0; 32]), ..Default::default() });
        d
    }

    fn at(row: usize, column: usize) -> TextPosition {
        TextPosition::new(row, column)
    }

    #[test]
    fn typing_groups_and_undoes() {
        let mut d = doc(&["a", "b", "c"], &["a", "B", "c"]);
        let mut s = EditorSession::new(&d, PathBuf::from("/r"), "f.kt".into(), false).unwrap();
        for (i, ch) in ["x", "y", "z"].iter().enumerate() {
            let p = at(0, 1 + i);
            s.replace(&mut d, p, p, ch, EditKind::Typing, Selection::caret(p));
        }
        assert_eq!(d.right.lines()[0], "axyz");
        assert!(s.is_dirty());
        let (_, sel) = s.undo(&mut d).unwrap();
        assert_eq!(d.right.lines()[0], "a");
        assert_eq!(sel, Selection::caret(at(0, 1)));
        assert!(!s.can_undo());
        s.redo(&mut d).unwrap();
        assert_eq!(d.right.lines()[0], "axyz");
    }

    #[test]
    fn newline_moves_hunks_and_keeps_crlf() {
        let mut d = doc(&["a", "b", "c"], &["a", "B", "c"]);
        let mut s = EditorSession::new(&d, PathBuf::from("/r"), "f.kt".into(), false).unwrap();
        s.crlf = Some(vec![true, false, true]);
        s.format.newline = "\r\n";
        s.replace(&mut d, at(0, 1), at(0, 1), "\n", EditKind::Typing, Selection::caret(at(0, 1)));
        assert_eq!(d.right.lines(), &["a", "", "B", "c"]);
        assert_eq!(s.crlf.as_deref(), Some(&[true, true, false, true][..]));
        // The inserted line merged with nothing below it; the B hunk moved down.
        assert!(d.hunks().iter().any(|h| h.right_start == 2 && h.right_count == 1));
    }

    #[test]
    fn composition_is_not_part_of_the_file() {
        let mut d = doc(&["a"], &["ab"]);
        let mut s = EditorSession::new(&d, PathBuf::from("/r"), "f.kt".into(), false).unwrap();
        s.set_marked(&mut d, "ni", at(0, 1));
        assert_eq!(d.right.lines()[0], "anib");
        assert!(s.encoded(&d).is_none());
        s.set_marked(&mut d, "你", at(0, 1));
        assert_eq!(d.right.lines()[0], "a你b");
        let (text, pos, _) = s.remove_marked(&mut d).unwrap();
        assert_eq!((text.as_str(), pos), ("你", at(0, 1)));
        assert_eq!(d.right.lines()[0], "ab");
        assert!(s.encoded(&d).is_some());
    }

    #[test]
    fn resume_over_changed_file_is_one_undo_away() {
        let mut d = doc(&["a"], &["one"]);
        let mut s = EditorSession::new(&d, PathBuf::from("/r"), "f.kt".into(), false).unwrap();
        s.replace(&mut d, at(0, 3), at(0, 3), "!", EditKind::Typing, Selection::caret(at(0, 3)));
        let unsaved = s.unsaved(&d).unwrap();

        let mut d2 = doc(&["a"], &["two"]);
        d2.editing.as_mut().unwrap().fingerprint = Some([1; 32]);
        let mut s2 = EditorSession::new(&d2, PathBuf::from("/r"), "f.kt".into(), false).unwrap();
        let (_, changed) = s2.resume(&mut d2, unsaved, Selection::default());
        assert!(changed);
        assert_eq!(d2.right.lines()[0], "two");
        assert!(!s2.is_dirty());
        s2.undo(&mut d2);
        assert_eq!(d2.right.lines()[0], "one!");
    }
}
