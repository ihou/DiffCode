//! Drives the diff/file viewer: loading documents, change navigation, in-file find, editing the file on screen
//! (writing it back, keeping its diff current, never losing typing to a file that changed underneath), conflicts,
//! and Annotate.

use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::jobs::Jobs;
use crate::canvas::{CanvasAction, CodeCanvas, InitialPosition, Mode, SearchMatch};
use crate::core::annotations::{AnnotationEntry, LineAnnotations};
use crate::core::diff::{self, Content, DiffDocument, DiffRequest, Hunk, Side, Source};
use crate::core::editor_session::{EditorSession, History, Selection, Unsaved};
use crate::core::git::ChangeKind;
use crate::core::history;
use crate::core::lang::l;
use crate::core::lf;
use crate::core::remote::{ConflictBlock, ConflictChoice};
use crate::core::text_codec;
use crate::core::text_editing::{self, TextPosition};
use crate::core::working_tree;
use crate::geom::Point;
use crate::text_field::TextField;

/// Idle time after the last keystroke before the text is written.
const SAVE_DELAY: Duration = Duration::from_millis(1000);
/// Idle time before the diff is worked out again (by git, on the exact bytes that would be written).
const DIFF_DELAY: Duration = Duration::from_millis(150);
const BLAME_DELAY: Duration = Duration::from_millis(350);
const HISTORY_LIMIT: usize = 8;

/// What the viewer asks of the app.
#[derive(Clone, Debug)]
pub enum ViewerEvent {
    Toast(String),
    /// Escape with nothing to drop: back to the list the file was picked from.
    Exit,
    /// Past the last change (or before the first): the next or previous file.
    StepFile(i32),
    /// The document on screen was replaced, cleared or edited (its changes are new), or its request changed.
    DocumentChanged,
    /// A file of this repository was written here.
    FileWritten(PathBuf),
    RevertHunk(usize),
    ToggleCommitHunk(usize),
    OpenAnnotation(AnnotationEntry),
    ContextMenu { at: Point, hunk: Option<usize> },
    FocusCanvas,
}

struct UnsavedText {
    key: String,
    name: String,
    unsaved: Unsaved,
    /// The file was left with them (and told so), rather than read again.
    announced: bool,
}

pub struct Viewer {
    pub canvas: CodeCanvas,
    pub jobs: Jobs<Viewer>,
    pub events: Vec<ViewerEvent>,
    pub request: Option<DiffRequest>,
    pub is_loading: bool,
    pub load_milliseconds: f64,
    pub conflict_count: usize,
    pub blame_milliseconds: f64,
    /// The code layout the settings ask for (side by side, or unified).
    pub unified: bool,

    pub find_visible: bool,
    pub find_field: TextField,
    pub find_count: usize,
    pub find_index: usize,
    /// Set when the find field should take the keyboard.
    pub find_focus_request: bool,

    load_generation: u64,
    pending_line: Option<usize>,
    pub land_on_last_hunk: bool,
    detached_from_focus: bool,
    armed: Option<(i32, Instant)>,
    signature: Option<String>,
    base_stamp: Option<String>,
    /// The base the document on screen was loaded against.
    loaded_stamp: Option<String>,

    // Editing.
    /// The text on screen has changes that are not in the file yet.
    pub is_dirty: bool,
    /// Why the working tree file on screen cannot be edited, when it cannot.
    pub read_only_reason: Option<String>,
    /// Moves on whenever the diff of the text being edited is recomputed.
    pub diff_revision: u64,
    save_at: Option<Instant>,
    diff_at: Option<Instant>,
    diff_generation: u64,
    /// Undo histories of files edited earlier, most recent last, so Ctrl+Z still works after coming back to one.
    histories: Vec<(String, History)>,
    /// Typing that could not be written when its file was left, kept until the file is opened again.
    unsaved_texts: Vec<UnsavedText>,

    // Annotate.
    /// Every file opened shows who last changed each line, until it is turned off.
    pub annotating: bool,
    pub is_blaming: bool,
    blame_at: Option<Instant>,
    blame_generation: u64,
    /// Width of the last column shown, kept for the next file so its text does not move when its blame arrives.
    annotation_columns: Option<usize>,
}

impl Default for Viewer {
    fn default() -> Self {
        Viewer {
            canvas: CodeCanvas::new(),
            jobs: Jobs::default(),
            events: Vec::new(),
            request: None,
            is_loading: false,
            load_milliseconds: 0.0,
            conflict_count: 0,
            blame_milliseconds: 0.0,
            unified: false,
            find_visible: false,
            find_field: TextField::new(),
            find_count: 0,
            find_index: 0,
            find_focus_request: false,
            load_generation: 0,
            pending_line: None,
            land_on_last_hunk: false,
            detached_from_focus: false,
            armed: None,
            signature: None,
            base_stamp: None,
            loaded_stamp: None,
            is_dirty: false,
            read_only_reason: None,
            diff_revision: 0,
            save_at: None,
            diff_at: None,
            diff_generation: 0,
            histories: Vec::new(),
            unsaved_texts: Vec::new(),
            annotating: false,
            is_blaming: false,
            blame_at: None,
            blame_generation: 0,
            annotation_columns: None,
        }
    }
}

fn history_key(root: &Path, path: &str) -> String {
    format!("{}\u{0}{path}", root.display())
}

fn toast(events: &mut Vec<ViewerEvent>, text: String) {
    events.push(ViewerEvent::Toast(text));
}

impl Viewer {
    pub fn document(&self) -> Option<&DiffDocument> {
        self.canvas.document()
    }

    fn mode(&self) -> Mode {
        if self.unified {
            Mode::Unified
        } else {
            Mode::SideBySide
        }
    }

    pub fn is_text_document(&self) -> bool {
        self.canvas.is_text()
    }

    pub fn hunk_count(&self) -> usize {
        self.document().map_or(0, |d| d.hunks().len())
    }

    pub fn focused_hunk(&self) -> Option<usize> {
        self.canvas.focused_hunk
    }

    pub fn editor(&self) -> Option<&EditorSession> {
        self.canvas.editor.as_ref()
    }

    pub fn can_edit(&self) -> bool {
        self.canvas.editor.is_some()
    }

    /// True when single changes of the document can be put back: a local change with both sides present.
    /// Not a file in conflict: its conflicts have buttons of their own, which take a whole conflict at a time.
    pub fn can_revert_hunks(&self) -> bool {
        let (Some(request), Some(doc)) = (&self.request, self.document()) else { return false };
        if self.is_loading || !request.is_against_head() || !matches!(doc.content, Content::Text) || doc.is_plain_file || doc.hunks().is_empty() {
            return false;
        }
        matches!(request.kind, Some(ChangeKind::Modified) | Some(ChangeKind::Renamed))
    }

    /// Runs what finished in the background, and what is due. Returns when it next wants to run.
    pub fn tick(&mut self) -> Option<Instant> {
        Jobs::run(self, |v| &mut v.jobs);
        let now = Instant::now();
        if self.save_at.is_some_and(|t| t <= now) {
            self.save_at = None;
            if self.canvas.has_composition() {
                self.schedule_save();
            } else {
                self.save_edits();
            }
        }
        if self.diff_at.is_some_and(|t| t <= now) {
            self.diff_at = None;
            self.rediff();
        }
        if self.blame_at.is_some_and(|t| t <= now) {
            self.blame_at = None;
            self.load_annotations(false);
        }
        [self.save_at, self.diff_at, self.blame_at].into_iter().flatten().min()
    }

    // MARK: Loading

    /// Shows `request`. A request that is already on screen is left alone unless a line is given.
    pub fn open(&mut self, request: DiffRequest, line: Option<usize>, preserve_scroll: bool) {
        if Some(&request) == self.request.as_ref() {
            if let Some(line) = line {
                if self.document().is_some() && !self.is_loading {
                    self.reveal(line);
                } else {
                    self.pending_line = Some(line);
                }
            }
            return;
        }
        let editing = self.canvas.editor.as_ref().is_some_and(|e| e.has_edits());
        if let Some(current) = self.request.clone() {
            if editing && !self.is_loading && current.root == request.root && current.path == request.path {
                // The file being edited, asked for again because the typing changed its status: it left the list, or
                // came back. The text on screen is still right, and reloading it would only make it jump.
                if let (Source::Worktree { base: a }, Source::Worktree { base: b }) = (&current.source, &request.source) {
                    if a == b && current.old_path == request.old_path && current.ignore_whitespace == request.ignore_whitespace {
                        self.request = Some(request.clone());
                        self.canvas.shows_revert_buttons = self.can_revert_hunks();
                        self.events.push(ViewerEvent::DocumentChanged);
                    }
                }
                if current.source == Source::File || request.source == Source::File || self.request.as_ref() == Some(&request) {
                    if let Some(line) = line {
                        self.reveal(line);
                    }
                    let base = self.base_stamp.clone();
                    self.reload_if_stale(base);
                    return;
                }
            }
        }
        self.load(request, line, preserve_scroll);
    }

    pub fn reload(&mut self) {
        if let Some(request) = self.request.clone() {
            self.load(request, None, true);
        }
    }

    pub fn base_stamp(&self) -> Option<&str> {
        self.base_stamp.as_deref()
    }

    pub fn set_base_stamp(&mut self, stamp: Option<String>) {
        self.base_stamp = stamp;
    }

    /// Reloads when the file on disk (or the base revision) changed since it was loaded.
    ///
    /// While the file is being edited, a new date alone is not news (saving here changes it): only a change of what
    /// the file holds is. Then unsaved typing is never thrown away: the file's new text comes in as a step that
    /// Ctrl+Z takes back.
    pub fn reload_if_stale(&mut self, base: Option<String>) {
        let Some(request) = self.request.clone() else { return };
        if self.is_loading || matches!(request.source, Source::Commit { .. }) {
            return;
        }
        let current = signature(&request, base.as_deref());
        if Some(&current) == self.signature.as_ref() {
            return;
        }
        let Some(editor) = self.canvas.editor.as_ref() else {
            self.reload();
            return;
        };
        let base_moved = base != self.loaded_stamp;
        let data = read_file(&editor.full_path());
        if let Some(data) = &data {
            if editor.matches_disk(data) {
                // The file holds what was read or written here.
                if base_moved {
                    // Committed, say: the same text against a new base. Typing is written first; the history comes along.
                    if self.save_edits() {
                        self.reload();
                    }
                } else {
                    self.signature = Some(current);
                }
                return;
            }
        }
        if !editor.is_dirty() && editor.marked().is_none() {
            self.reload();
            return;
        }
        let Some(data) = data else {
            self.signature = Some(current);
            toast(&mut self.events, l("The file was deleted · your changes are kept here, not written", "文件已被删除 · 修改还在这里，没有写入"));
            return;
        };
        self.take_disk_version(&data);
        self.signature = Some(current);
        if base_moved {
            self.reload();
        }
    }

    pub fn clear(&mut self) {
        self.save_edits();
        self.load_generation += 1;
        self.blame_generation += 1;
        self.canvas.annotation_hover = None;
        self.request = None;
        self.end_editing();
        self.is_loading = false;
        self.canvas.set_document(None, self.mode(), false);
        self.canvas.shows_revert_buttons = false;
        self.canvas.conflicts.clear();
        self.conflict_count = 0;
        self.events.push(ViewerEvent::DocumentChanged);
    }

    fn load(&mut self, request: DiffRequest, line: Option<usize>, preserve_scroll: bool) {
        // Leaving the text, or reading it again: what was typed goes to the file first.
        self.save_edits();
        self.request = Some(request.clone());
        self.pending_line = line;
        self.load_generation += 1;
        let generation = self.load_generation;
        self.is_loading = true;
        let stamp = self.base_stamp.clone();
        self.jobs.spawn(
            move || {
                let doc = diff::load(&request);
                (request, doc)
            },
            move |viewer: &mut Viewer, (request, doc): (DiffRequest, DiffDocument)| {
                if viewer.load_generation != generation || viewer.request.as_ref() != Some(&request) {
                    return;
                }
                viewer.signature = Some(signature(&request, stamp.as_deref()));
                viewer.loaded_stamp = stamp;
                viewer.apply(doc, preserve_scroll);
            },
        );
    }

    fn apply(&mut self, doc: DiffDocument, preserve_scroll: bool) {
        // Typing that went on while this was loading.
        let mut caret = None;
        if let (Some(previous), Some(request)) = (self.canvas.editor.as_ref(), self.request.clone()) {
            let same_file = previous.root == request.root && previous.path == request.path;
            if previous.is_dirty() {
                let saved = self.save_edits();
                if saved && same_file {
                    // What was just read is older than what is on screen: read it again.
                    let line = self.pending_line;
                    self.load(request, line, true);
                    return;
                }
            }
            if same_file && preserve_scroll {
                caret = self.canvas.edit_selection();
            }
        }
        self.end_editing();
        let previous_focus = self.canvas.focused_hunk;
        let hunks = doc.hunks().len();
        self.load_milliseconds = doc.load_milliseconds;
        self.is_loading = false;
        self.armed = None;
        self.canvas.set_document(Some(doc), self.mode(), preserve_scroll);
        self.canvas.shows_revert_buttons = self.can_revert_hunks();
        self.begin_editing(caret);
        self.refresh_conflicts();
        self.canvas.annotation_hover = None;
        if self.annotating {
            self.load_annotations(false);
        }
        if preserve_scroll {
            if let Some(focus) = previous_focus.filter(|f| *f < hunks) {
                self.canvas.focused_hunk = Some(focus);
            }
            if let Some(line) = self.pending_line.take() {
                self.reveal(line);
            }
        } else {
            self.detached_from_focus = false;
            match self.pending_line.take() {
                Some(line) => {
                    self.canvas.position_when_laid_out(InitialPosition::Line(line));
                    self.detached_from_focus = true;
                }
                None => self.canvas.position_when_laid_out(InitialPosition::Change { last: self.land_on_last_hunk }),
            }
            self.land_on_last_hunk = false;
        }
        if self.find_visible && !self.find_field.text.is_empty() {
            self.run_find(false);
        }
        self.events.push(ViewerEvent::DocumentChanged);
    }

    // MARK: Navigation

    /// Scrolls to a 1-based line of the new side and tints it.
    pub fn reveal(&mut self, line: usize) {
        if !self.is_text_document() {
            return;
        }
        self.canvas.reveal_line_later(line);
        self.detached_from_focus = true;
    }

    pub fn step_difference(&mut self, delta: i32) {
        let count = self.hunk_count();
        if count == 0 || !self.is_text_document() {
            self.step_file_with_confirmation(delta);
            return;
        }
        let target: i64 = match self.canvas.focused_hunk {
            Some(current) if !self.detached_from_focus => current as i64 + delta as i64,
            _ if delta > 0 => {
                // After manual scrolling, continue from what is on screen rather than the old focus.
                let top = self.canvas.top_row();
                (0..count).find(|&i| self.canvas.hunk_row(i) > top).unwrap_or(count) as i64
            }
            _ => {
                let limit = self.canvas.top_row() + self.canvas.visible_row_count() / 4;
                (0..count).rev().find(|&i| self.canvas.hunk_row(i) < limit).map_or(-1, |i| i as i64)
            }
        };
        if target < 0 || target >= count as i64 {
            self.step_file_with_confirmation(delta);
            return;
        }
        self.armed = None;
        self.focus(target as usize, true);
    }

    pub fn focus(&mut self, index: usize, animated: bool) {
        self.detached_from_focus = false;
        self.canvas.focus_hunk(index, animated);
    }

    /// At the last change the first press only hints; a second press moves to the next file.
    fn step_file_with_confirmation(&mut self, delta: i32) {
        let armed = self.armed.is_some_and(|(d, at)| d == delta && at.elapsed() < Duration::from_secs(3));
        if self.hunk_count() == 0 || armed {
            self.armed = None;
            self.land_on_last_hunk = delta < 0;
            self.events.push(ViewerEvent::StepFile(delta));
        } else {
            self.armed = Some((delta, Instant::now()));
            toast(
                &mut self.events,
                if delta > 0 {
                    l("Last change · press F7 again for the next file", "已是最后一处变更 · 再按 F7 进入下一个文件")
                } else {
                    l("First change · press Shift+F7 again for the previous file", "已是第一处变更 · 再按 Shift+F7 回到上一个文件")
                },
            );
        }
    }

    // MARK: View options

    pub fn set_unified(&mut self, unified: bool) {
        if unified == self.unified {
            return;
        }
        self.unified = unified;
        let focused = self.canvas.focused_hunk;
        self.canvas.set_mode(self.mode());
        if let Some(f) = focused.filter(|f| *f < self.hunk_count()) {
            self.canvas.scroll_to_row(self.canvas.hunk_row(f), 0.25, false);
        }
        if self.find_visible {
            self.run_find(false);
        }
    }

    /// Ignore Whitespace changed: the document is read again with the new setting.
    pub fn set_ignore_whitespace(&mut self, ignore: bool) {
        let Some(mut request) = self.request.clone() else { return };
        if request.ignore_whitespace == ignore {
            return;
        }
        request.ignore_whitespace = ignore;
        self.load(request, None, true);
    }

    // MARK: Find in file

    pub fn show_find(&mut self) {
        if !self.is_text_document() {
            return;
        }
        if let Some(selected) = self.canvas.selected_text().filter(|s| !s.is_empty() && !s.contains('\n')) {
            self.find_field.set_text(&selected);
        }
        self.find_field.select_all();
        self.find_visible = true;
        self.find_focus_request = true;
        self.run_find(true);
    }

    pub fn hide_find(&mut self) {
        self.find_visible = false;
        self.find_count = 0;
        self.find_index = 0;
        self.canvas.set_highlights([HashMap::new(), HashMap::new()], Vec::new(), None);
        self.events.push(ViewerEvent::FocusCanvas);
    }

    pub fn run_find(&mut self, reveal: bool) {
        let query = self.find_field.text.clone();
        let Some(doc) = self.canvas.document().filter(|d| matches!(d.content, Content::Text) && !query.is_empty()) else {
            self.find_count = 0;
            self.find_index = 0;
            self.canvas.set_highlights([HashMap::new(), HashMap::new()], Vec::new(), None);
            return;
        };
        // Smart case: an uppercase letter in the query makes the search case-sensitive.
        let insensitive = query == query.to_lowercase();
        let mut by_side: [HashMap<usize, Vec<Range<usize>>>; 2] = [HashMap::new(), HashMap::new()];
        let mut navigation: Vec<(f64, SearchMatch)> = Vec::new();
        for side in [Side::Left, Side::Right] {
            if side == Side::Left && doc.is_plain_file {
                continue;
            }
            // The left side repeats every unchanged line; only its changed lines are navigation stops.
            let mut hunk = 0;
            let hunks = doc.hunks();
            for (index, line) in doc.buffer(side).lines().iter().enumerate() {
                if line.len() < query.len() && !insensitive {
                    continue;
                }
                let ranges = find_all(line, &query, insensitive);
                if ranges.is_empty() {
                    continue;
                }
                let stop = if side == Side::Right {
                    true
                } else {
                    while hunk < hunks.len() && hunks[hunk].left_start + hunks[hunk].left_count <= index {
                        hunk += 1;
                    }
                    hunk < hunks.len() && hunks[hunk].left_start <= index
                };
                if stop {
                    let key = doc.virtual_position(index, side) + if side == Side::Right { 0.5 } else { 0.0 };
                    for range in &ranges {
                        navigation.push((key, SearchMatch { side, line: index, range: range.clone() }));
                    }
                }
                by_side[side as usize].insert(index, ranges);
            }
        }
        navigation.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal).then(a.1.range.start.cmp(&b.1.range.start)));
        let list: Vec<SearchMatch> = navigation.into_iter().map(|(_, m)| m).collect();
        // Start from the first match at or below the top of the viewport.
        let top = self.canvas.top_row();
        let current = if list.is_empty() {
            None
        } else {
            Some(list.iter().position(|m| self.canvas.row_for_line(m.line, m.side) >= top).unwrap_or(0))
        };
        self.find_count = list.len();
        self.find_index = current.map_or(0, |c| c + 1);
        let target = current.map(|c| list[c].clone());
        self.canvas.set_highlights(by_side, list, current);
        if reveal {
            if let Some(m) = target {
                self.reveal_match(&m);
            }
        }
    }

    pub fn step_find(&mut self, delta: i32) {
        let count = self.canvas.matches.len();
        if count == 0 {
            return;
        }
        let current = self.canvas.current_match.map_or(-1, |c| c as i64);
        let next = ((current + delta as i64) % count as i64 + count as i64) % count as i64;
        let next = next as usize;
        self.canvas.set_current_match(Some(next));
        self.find_index = next + 1;
        let m = self.canvas.matches[next].clone();
        self.reveal_match(&m);
    }

    fn reveal_match(&mut self, m: &SearchMatch) {
        let row = self.canvas.row_for_line(m.line, m.side);
        let top = self.canvas.top_row();
        let visible = self.canvas.visible_row_count();
        if row < top + 2 || row + 4 > top + visible {
            self.canvas.scroll_to_row(row, 0.4, true);
        }
        self.detached_from_focus = true;
    }

    // MARK: The canvas

    /// What the canvas asked for this frame.
    pub fn handle_canvas_actions(&mut self, actions: Vec<CanvasAction>, font_size: &mut f32) -> bool {
        let mut font_changed = false;
        for action in actions {
            match action {
                CanvasAction::Exit => {
                    if self.find_visible {
                        self.hide_find();
                    } else {
                        self.events.push(ViewerEvent::Exit);
                    }
                }
                CanvasAction::RevertHunk(index) => {
                    if self.can_revert_hunks() {
                        self.canvas.focused_hunk = Some(index);
                        self.events.push(ViewerEvent::RevertHunk(index));
                    }
                }
                CanvasAction::ToggleCommitHunk(index) => self.events.push(ViewerEvent::ToggleCommitHunk(index)),
                CanvasAction::ResolveConflict(index, choice) => self.resolve_conflict(index, choice),
                CanvasAction::OpenAnnotation(entry) => {
                    self.canvas.annotation_hover = None;
                    self.events.push(ViewerEvent::OpenAnnotation(entry));
                }
                CanvasAction::ContextMenu { at, hunk } => self.events.push(ViewerEvent::ContextMenu { at, hunk }),
                CanvasAction::Edited => self.did_edit(),
                CanvasAction::UserScrolled => self.detached_from_focus = true,
                CanvasAction::Zoom(delta) => {
                    let size = (*font_size + delta as f32).clamp(9.0, 28.0);
                    if size != *font_size {
                        *font_size = size;
                        font_changed = true;
                    }
                }
            }
        }
        font_changed
    }

    // MARK: Editing: the session

    /// Starts editing the freshly loaded document if its right side can be edited, with the caret where it was and
    /// the history the same text had, if it was edited before.
    fn begin_editing(&mut self, caret: Option<Selection>) {
        let Some(doc) = self.canvas.document() else { return };
        let reason = doc.editing.as_ref().and_then(|e| e.read_only_reason.clone());
        self.read_only_reason = reason.clone();
        let Some(request) = self.request.clone() else { return };
        let key = history_key(&request.root, &request.path);
        let Some(mut session) = EditorSession::new(doc, request.root.clone(), request.path.clone(), request.ignore_whitespace) else {
            if let Some(reason) = reason {
                if self.unsaved_texts.iter().any(|u| u.key == key) {
                    toast(
                        &mut self.events,
                        lf!(
                            "Your unsaved changes to this file are still kept, but it cannot be edited now: {reason}",
                            "这个文件没保存成功的修改还暂存着，但它现在不能编辑：{reason}"
                        ),
                    );
                }
            }
            return;
        };
        if let Some(index) = self.histories.iter().rposition(|(k, _)| *k == key) {
            let (_, history) = self.histories.remove(index);
            session.adopt(history);
        }
        self.canvas.editor = Some(session);
        if let Some(c) = caret {
            self.canvas.set_selection(c.anchor, c.head);
        }
        if let Some(index) = self.unsaved_texts.iter().rposition(|u| u.key == key) {
            let entry = self.unsaved_texts.remove(index);
            self.resume(entry.unsaved, entry.announced);
        }
    }

    /// Typing that could not be written comes back (when the file is opened again, or read again), and is written as
    /// soon as it can be.
    fn resume(&mut self, unsaved: Unsaved, announce: bool) {
        let caret = self.canvas.edit_selection().unwrap_or_default();
        let mut changed_since = false;
        self.canvas.perform_edit(caret.head.row, |e, d| {
            let (outcomes, changed) = e.resume(d, unsaved, caret);
            changed_since = changed;
            outcomes
        });
        self.canvas.set_selection(caret.anchor, caret.head);
        self.did_edit();
        if changed_since {
            toast(
                &mut self.events,
                l(
                    "Changed by another program after your changes could not be saved · its version is loaded; Ctrl+Z brings yours back",
                    "修改没保存成功之后，文件被其他程序改了，已载入新版本；Ctrl+Z 找回你的修改",
                ),
            );
        } else if announce {
            toast(&mut self.events, l("Your unsaved changes are back · saving again", "没保存成功的修改已恢复，正在重新保存"));
        }
    }

    /// Lets go of the session, keeping its history for when the file is opened again.
    fn end_editing(&mut self) {
        self.save_at = None;
        self.diff_at = None;
        self.diff_generation += 1;
        if self.canvas.editor.is_some() {
            self.canvas.commit_composition();
        }
        if let Some(editor) = self.canvas.editor.take() {
            let key = history_key(&editor.root, &editor.path);
            if let Some(history) = editor.history() {
                self.histories.retain(|(k, _)| *k != key);
                self.histories.push((key, history));
                if self.histories.len() > HISTORY_LIMIT {
                    let extra = self.histories.len() - HISTORY_LIMIT;
                    self.histories.drain(..extra);
                }
            } else if let Some(unsaved) = self.canvas.document().and_then(|d| editor.unsaved(d)) {
                // Writing it failed (what went wrong was shown): it is kept, not thrown away with the session.
                let name = crate::core::util::file_name(&editor.path).to_string();
                let leaving = self.request.as_ref().is_none_or(|r| r.root != editor.root || r.path != editor.path);
                self.unsaved_texts.retain(|u| u.key != key);
                if leaving {
                    toast(
                        &mut self.events,
                        lf!(
                            "Changes to {name} could not be saved · they are kept and come back when you open it again",
                            "“{name}”的修改没能保存，已暂存；再次打开它时会恢复"
                        ),
                    );
                }
                self.unsaved_texts.push(UnsavedText { key, name, unsaved, announced: leaving });
            }
        }
        self.is_dirty = false;
    }

    /// Files whose typing could not be written and is kept here, for the question before quitting.
    pub fn unsaved_names(&self) -> Vec<String> {
        self.unsaved_texts.iter().map(|u| u.name.clone()).collect()
    }

    /// Whether the viewer is editing `path` of the repository at `root` and has changed it since it was opened.
    pub fn is_editing(&self, root: &Path, path: &str) -> bool {
        let editing = self.canvas.editor.as_ref().is_some_and(|e| e.has_edits());
        editing && self.request.as_ref().is_some_and(|r| r.root == root && r.path == path)
    }

    fn update_dirty(&mut self) {
        self.is_dirty = self.canvas.editor.as_ref().is_some_and(|e| e.is_dirty());
    }

    fn did_edit(&mut self) {
        self.update_dirty();
        self.refresh_conflicts();
        self.schedule_save();
        self.schedule_diff();
        self.events.push(ViewerEvent::DocumentChanged);
    }

    // MARK: Editing: writing

    fn schedule_save(&mut self) {
        self.save_at = Some(Instant::now() + SAVE_DELAY);
    }

    /// Ctrl+S: writes now.
    pub fn save_now(&mut self) {
        if self.canvas.editor.is_none() {
            if let Some(reason) = self.read_only_reason.clone() {
                toast(&mut self.events, reason);
            }
            return;
        }
        self.save_edits();
    }

    /// Writes the text on screen to the file, if anything in it is not written yet. Returns false when it could not
    /// be written (what happened is shown), true when it was written or there was nothing to write.
    ///
    /// The file must still hold what was last read or written here; if something else changed it, its version comes
    /// in instead, and Ctrl+Z brings back what was typed. Written in place, so the file keeps its permissions and
    /// whatever watches it sees an edit.
    pub fn save_edits(&mut self) -> bool {
        self.save_at = None;
        if self.canvas.editor.is_none() || self.request.is_none() {
            return true;
        }
        self.canvas.commit_composition();
        let (Some(editor), Some(doc)) = (self.canvas.editor.as_ref(), self.canvas.document()) else { return true };
        if !editor.is_dirty() {
            return true;
        }
        let Some(data) = editor.encoded(doc) else {
            toast(&mut self.events, l("Not saved: the text cannot be written in the file's encoding", "没有保存：内容无法用文件原来的编码写入"));
            return false;
        };
        if editor.matches_disk(&data) {
            // Typed and undone back to what the file holds.
            if let Some(e) = self.canvas.editor.as_mut() {
                e.mark_saved();
            }
            self.update_dirty();
            return true;
        }
        let path = editor.full_path();
        let Some(current) = read_file(&path) else {
            toast(&mut self.events, l("Not saved: the file is gone", "没有保存：文件已经不在了"));
            return false;
        };
        if !editor.matches_disk(&current) {
            self.take_disk_version(&current);
            return false;
        }
        if let Err(error) = working_tree::write_in_place(&path, &data) {
            toast(&mut self.events, lf!("Not saved: {error}", "保存失败：{error}"));
            return false;
        }
        if let Some(e) = self.canvas.editor.as_mut() {
            e.did_write(&data);
        }
        let request = self.request.clone().expect("request");
        // Its new date is this save's, not news to reload for.
        self.signature = Some(signature(&request, self.loaded_stamp.as_deref()));
        self.update_dirty();
        self.events.push(ViewerEvent::FileWritten(request.root.clone()));
        // Who changed what is worked out again from the file as written.
        if self.annotating {
            self.blame_at = Some(Instant::now() + BLAME_DELAY);
        }
        true
    }

    /// The file changed underneath unsaved typing: its version replaces the text, as a step Ctrl+Z takes back.
    fn take_disk_version(&mut self, data: &[u8]) {
        if self.canvas.editor.is_none() {
            return;
        }
        let text = (data.len() <= text_editing::MAX_EDITABLE_BYTES)
            .then(|| text_codec::decode_for_editing(data))
            .flatten()
            .filter(|t| !t.lines.iter().any(|l| l.len() > text_editing::MAX_LINE_BYTES));
        let Some(text) = text else {
            // It became something that cannot be edited here; show it as it is now.
            if let Some(e) = self.canvas.editor.as_mut() {
                e.mark_saved();
            }
            toast(&mut self.events, l("The file was changed by another program and can no longer be edited here", "文件被其他程序改成了无法在这里编辑的内容"));
            self.reload();
            return;
        };
        self.canvas.commit_composition();
        let caret = self.canvas.edit_selection().unwrap_or_default();
        self.canvas.perform_edit(caret.head.row, |e, d| vec![e.take_disk_version(d, &text, data, caret)]);
        self.canvas.set_selection(caret.anchor, caret.head);
        self.update_dirty();
        self.schedule_diff();
        toast(
            &mut self.events,
            l("Changed by another program · its version is loaded; Ctrl+Z brings yours back", "文件被其他程序改了，已载入新版本；Ctrl+Z 找回你的修改"),
        );
    }

    // MARK: Editing: keeping the diff current

    /// Works the diff out again after a pause in the typing. Until then the changes are moved along by the edits.
    fn schedule_diff(&mut self) {
        self.diff_at = Some(Instant::now() + DIFF_DELAY);
    }

    fn rediff(&mut self) {
        let (Some(editor), Some(doc)) = (self.canvas.editor.as_ref(), self.canvas.document()) else { return };
        if doc.is_plain_file {
            return;
        }
        if editor.marked().is_some() {
            self.schedule_diff();
            return;
        }
        let Some(data) = editor.encoded(doc) else { return };
        let version = editor.version();
        let base = editor.base.clone();
        let ignore_whitespace = editor.ignore_whitespace;
        self.diff_generation += 1;
        let generation = self.diff_generation;
        self.jobs.spawn(
            move || {
                let computed = base.as_ref().map(|b| diff::compute_hunks(Some(b), Some(&data), ignore_whitespace));
                (computed, data, base)
            },
            move |viewer: &mut Viewer, (computed, data, base): (Option<Option<Vec<Hunk>>>, Vec<u8>, Option<Vec<u8>>)| {
                if viewer.diff_generation != generation || viewer.canvas.editor.as_ref().is_none_or(|e| e.version() != version) {
                    return;
                }
                let Some(doc) = viewer.canvas.document() else { return };
                let mut hunks = match computed {
                    // A new file: every line is added; an empty one has nothing to show.
                    None if data.is_empty() => Vec::new(),
                    None => vec![Hunk::lines(0, 0, 0, doc.right.len())],
                    Some(Some(h)) => h,
                    Some(None) => return,
                };
                diff::add_inline_highlights(&mut hunks, doc.left.lines(), doc.right.lines());
                viewer.canvas.replace_hunks(hunks);
                if let Some(doc) = viewer.canvas.document_mut() {
                    doc.note = diff::note(doc.hunks(), base.as_deref(), Some(&data), ignore_whitespace);
                }
                viewer.diff_revision += 1;
                viewer.events.push(ViewerEvent::DocumentChanged);
                if viewer.find_visible && !viewer.find_field.text.is_empty() {
                    viewer.run_find(false);
                }
            },
        );
    }

    // MARK: Conflicts

    /// A file git is merging, shown against one of its versions: its conflict markers get buttons.
    pub fn shows_conflicts(&self) -> bool {
        self.request.as_ref().is_some_and(|r| r.kind == Some(ChangeKind::Conflicted) && matches!(r.source, Source::Worktree { .. }))
    }

    /// Finds the conflicts in the text on screen again (after it loaded, or was edited).
    fn refresh_conflicts(&mut self) {
        let blocks = if self.shows_conflicts() { self.document().map(|d| ConflictBlock::find(d.right.lines())).unwrap_or_default() } else { Vec::new() };
        self.conflict_count = blocks.len();
        self.canvas.conflicts = blocks;
    }

    /// Replaces conflict `index` with HEAD's lines, the incoming ones, or both: an edit of the text, which Ctrl+Z
    /// takes back.
    pub fn resolve_conflict(&mut self, index: usize, choice: ConflictChoice) {
        if self.canvas.editor.is_none() {
            let reason = self.read_only_reason.clone().unwrap_or_else(|| l("This file cannot be edited here", "这个文件无法在这里编辑"));
            toast(&mut self.events, reason);
            return;
        }
        self.canvas.commit_composition();
        let Some(doc) = self.document() else { return };
        let lines = doc.right.lines();
        let blocks = ConflictBlock::find(lines);
        let Some(block) = blocks.get(index).copied() else { return };
        let replacement = block.resolution(choice, lines);
        let end = |row: usize| TextPosition::new(row, lines[row].len());
        let (from, to, text) = if !replacement.is_empty() {
            (TextPosition::new(block.start, 0), end(block.end), replacement.join("\n"))
        } else if block.end + 1 < lines.len() {
            (TextPosition::new(block.start, 0), TextPosition::new(block.end + 1, 0), String::new())
        } else if block.start > 0 {
            (end(block.start - 1), end(block.end), String::new())
        } else {
            (TextPosition::new(0, 0), end(block.end), String::new())
        };
        self.canvas.apply_command_edit(from, to, &text, Some(from));
        self.update_dirty();
        self.schedule_diff();
        self.refresh_conflicts();
        self.save_edits();
    }

    // MARK: Rolling back a change

    /// Puts change `index` back the way the left side has it, as an edit of the text (Ctrl+Z in the editor takes it
    /// back), and writes the file. Returns false when it could not be done or written.
    pub fn revert_hunk_in_editor(&mut self, index: usize) -> bool {
        if self.canvas.editor.is_none() {
            return false;
        }
        self.canvas.commit_composition();
        let Some(doc) = self.document() else { return false };
        let Some(hunk) = doc.hunks().get(index).cloned() else { return false };
        let left = doc.left.lines()[hunk.left_start..hunk.left_start + hunk.left_count].join("\n");
        let right = doc.right.lines();
        let left_count = doc.left.len();
        let last_row = right.len() - 1;
        let end = |row: usize| TextPosition::new(row, right[row].len());
        let (from, to, text) = if hunk.right_count > 0 {
            let (first, last) = (hunk.right_start, hunk.right_start + hunk.right_count - 1);
            if hunk.left_count > 0 {
                (TextPosition::new(first, 0), end(last), left)
            } else if last < last_row {
                (TextPosition::new(first, 0), TextPosition::new(last + 1, 0), String::new())
            } else if first > 0 {
                (end(first - 1), end(last), String::new())
            } else {
                (TextPosition::new(0, 0), end(last), String::new())
            }
        } else if right.len() == 1 && right[0].is_empty() {
            // Everything was deleted: the file's one empty line becomes the old lines.
            (TextPosition::new(0, 0), TextPosition::new(0, 0), left)
        } else if hunk.right_start <= last_row {
            let at = TextPosition::new(hunk.right_start, 0);
            (at, at, format!("{left}\n"))
        } else {
            (end(last_row), end(last_row), format!("\n{left}"))
        };
        let at_end = hunk.right_start + hunk.right_count >= right.len() || hunk.left_start + hunk.left_count >= left_count;
        self.canvas.apply_command_edit(from, to, &text, Some(from));
        // At the end of the file, whether it ends with a line break is part of the change too.
        if at_end {
            if let Some(editor) = self.canvas.editor.as_mut() {
                if let Some(base) = editor.base.clone() {
                    let last = text_codec::decode_for_editing(&base).map_or(base.last() == Some(&b'\n'), |t| t.format.final_newline);
                    editor.set_final_newline(last);
                }
            }
        }
        self.update_dirty();
        self.schedule_diff();
        self.save_edits()
    }

    /// Whether the text on screen is, byte for byte, its left side (a change rolled back was the last one).
    pub fn text_matches_base(&self) -> bool {
        let (Some(editor), Some(doc)) = (self.canvas.editor.as_ref(), self.document()) else { return false };
        match (&editor.base, editor.is_dirty(), editor.encoded(doc)) {
            (Some(base), false, Some(data)) => data == *base,
            _ => false,
        }
    }

    /// The toast's Undo after a change was rolled back in the editor.
    pub fn undo_in_editor(&mut self) {
        if self.canvas.editor.is_none() {
            return;
        }
        self.canvas.undo();
        self.did_edit();
        self.save_edits();
        self.events.push(ViewerEvent::FocusCanvas);
    }

    // MARK: Annotate

    pub fn toggle_annotate(&mut self) {
        self.annotating = !self.annotating;
        if self.annotating {
            self.load_annotations(true);
        } else {
            self.blame_at = None;
            self.blame_generation += 1;
            self.is_blaming = false;
            self.annotation_columns = None;
            self.canvas.set_annotations(None);
        }
    }

    /// Whether the file on screen can be annotated: it has commits, and the side shown exists.
    pub fn can_annotate(&self) -> bool {
        self.blame_target().is_some()
    }

    /// What `git blame` reads for the document on screen: the file in the working tree (rev None) or in a commit.
    fn blame_target(&self) -> Option<(String, Option<String>)> {
        let request = self.request.as_ref()?;
        if self.is_loading || !self.is_text_document() {
            return None;
        }
        match &request.source {
            Source::File => Some((request.path.clone(), None)),
            Source::Worktree { .. } => {
                // A file never committed has nothing to tell; one added since the commit compared with may well have.
                if matches!(request.kind, Some(ChangeKind::Deleted) | Some(ChangeKind::Untracked)) {
                    return None;
                }
                if request.is_against_head() && request.kind == Some(ChangeKind::Added) {
                    return None;
                }
                Some((request.path.clone(), None))
            }
            Source::Commit { hash, .. } => {
                if request.kind == Some(ChangeKind::Deleted) {
                    return None;
                }
                Some((request.path.clone(), Some(hash.clone())))
            }
        }
    }

    /// Runs `git blame` for the document on screen and shows the result, unless the text changed meanwhile.
    /// `explicit`: the user just asked; say so when there is nothing to show.
    pub fn load_annotations(&mut self, explicit: bool) {
        self.blame_at = None;
        let Some(request) = self.request.clone() else { return };
        if !self.annotating || self.document().is_none() {
            return;
        }
        self.blame_generation += 1;
        let Some((path, rev)) = self.blame_target() else {
            self.canvas.set_annotations(None);
            self.is_blaming = false;
            if explicit && matches!(request.kind, Some(ChangeKind::Untracked) | Some(ChangeKind::Added)) {
                toast(&mut self.events, l("Not committed yet · nothing to annotate", "这个文件还没有提交过 · 没有可追溯的记录"));
            }
            return;
        };
        // Blame reads the file on disk, so what was typed goes there first.
        if rev.is_none() && self.is_dirty && !self.save_edits() {
            return;
        }
        let line_count = self.document().map_or(0, |d| d.right.len());
        if self.canvas.annotations().is_none() {
            if let Some(columns) = self.annotation_columns {
                self.canvas.set_annotations(Some(LineAnnotations::placeholder(columns, line_count)));
            }
        }
        let edits = self.canvas.edit_count;
        let generation = self.blame_generation;
        self.is_blaming = true;
        let started = Instant::now();
        let root = request.root.clone();
        self.jobs.spawn(
            move || history::blame(&path, rev.as_deref(), &root).ok(),
            move |viewer: &mut Viewer, blame| {
                if viewer.blame_generation != generation || viewer.request.as_ref() != Some(&request) {
                    return;
                }
                viewer.is_blaming = false;
                // Typed meanwhile: the annotations already follow the typing, and the next save reads the blame again.
                if viewer.canvas.edit_count != edits {
                    return;
                }
                let Some(blame) = blame else {
                    viewer.canvas.set_annotations(None);
                    if explicit {
                        toast(&mut viewer.events, l("Could not annotate this file", "无法追溯这个文件"));
                    }
                    return;
                };
                let line_count = viewer.document().map_or(0, |d| d.right.len());
                let annotations = LineAnnotations::from_blame(&blame, line_count);
                viewer.annotation_columns = Some(annotations.columns);
                viewer.canvas.set_annotations(Some(annotations));
                viewer.blame_milliseconds = started.elapsed().as_secs_f64() * 1000.0;
            },
        );
    }
}

/// What tells whether the file (or the base) changed since it was read: its date and size, and the base.
fn signature(request: &DiffRequest, base: Option<&str>) -> String {
    let path = diff::join_repo_path(&request.root, &request.path);
    let (modified, size) = match std::fs::metadata(&path) {
        Ok(m) => (m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos()), m.len() as i64),
        Err(_) => (0, -1),
    };
    format!("{modified}|{size}|{}", base.unwrap_or(""))
}

fn read_file(path: &Path) -> Option<Vec<u8>> {
    if path.is_dir() {
        return None;
    }
    std::fs::read(path).ok()
}

/// Every occurrence of `query` in `line`, as byte ranges; case-insensitive compares character by character.
pub fn find_all(line: &str, query: &str, insensitive: bool) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    if query.is_empty() {
        return out;
    }
    if !insensitive {
        let mut from = 0;
        while let Some(at) = line[from..].find(query) {
            let start = from + at;
            out.push(start..start + query.len());
            from = start + query.len().max(1);
            if from > line.len() {
                break;
            }
        }
        return out;
    }
    let needle: Vec<char> = query.chars().flat_map(|c| c.to_lowercase()).collect();
    let mut start = 0;
    while start < line.len() {
        // Match lowercased characters of the line against the lowercased query.
        let mut matched = 0;
        let mut end = start;
        let mut chars = line[start..].char_indices();
        'outer: for (offset, c) in chars.by_ref() {
            for lc in c.to_lowercase() {
                if matched < needle.len() && needle[matched] == lc {
                    matched += 1;
                } else {
                    matched = usize::MAX;
                    break 'outer;
                }
            }
            if matched == needle.len() {
                end = start + offset + c.len_utf8();
                break;
            }
        }
        if matched == needle.len() {
            out.push(start..end);
            start = end;
        } else {
            start += line[start..].chars().next().map_or(1, |c| c.len_utf8());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::find_all;

    #[test]
    fn smart_case_find() {
        assert_eq!(find_all("Foo foo FOO", "foo", true), vec![0..3, 4..7, 8..11]);
        assert_eq!(find_all("Foo foo FOO", "Foo", false), vec![0..3]);
        assert_eq!(find_all("中文 中文", "中文", true), vec![0..6, 7..13]);
        assert_eq!(find_all("aaa", "aa", false), vec![0..2]);
        assert!(find_all("abc", "", true).is_empty());
    }
}
