//! The code canvas: a diff side by side or unified, or a plain file.
//!
//! Each frame draws the rows in view: change bands, word highlights, search hits and the selection under the
//! text, line numbers in the gutters, the curved connectors between the panes, the overview ruler, and the
//! buttons on each change. Laid-out lines come from the toolkit's layout cache, so a frame that only scrolls
//! lays out just the rows that came into view.
//!
//! Side by side, the panes are not padded to equal height: each pane's offset is derived from one virtual scroll
//! position, which is what lets unchanged code stay level across the divider while the curved connectors join
//! the changes.

mod edit;

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::core::annotations::{AnnotationEntry, LineAnnotations};
use crate::core::diff::{Content, DiffDocument, HunkKind, RowKind, Side};
use crate::core::editor_session::EditorSession;
use crate::core::lang::l;
use crate::core::remote::{ConflictBlock, ConflictChoice};
use crate::core::syntax::TokenKind;
use crate::core::text_editing::{self, TextPosition};
use crate::geom::{Color, Point, Rect, Size};
use crate::icons::Icon;
use crate::input::{Event, MouseButton};
use crate::paint::{ColorRun, CursorIcon, Font, Overflow, TextLayout, TextRequest, TextStyle, VectorPath, Weight};
use crate::theme::CanvasColors;
use crate::ui::{Align, Id, Ui};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    SideBySide,
    Unified,
    Single,
}

/// A selection in the rows of one pane. Columns are byte offsets in the row's text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextSel {
    pub pane: Side,
    pub anchor: TextPosition,
    pub head: TextPosition,
}

impl TextSel {
    pub fn caret(pane: Side, at: TextPosition) -> Self {
        TextSel { pane, anchor: at, head: at }
    }

    pub fn lower(&self) -> TextPosition {
        self.anchor.min(self.head)
    }

    pub fn upper(&self) -> TextPosition {
        self.anchor.max(self.head)
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchMatch {
    pub side: Side,
    pub line: usize,
    pub range: Range<usize>,
}

/// What of a local change goes into the next commit, for the ticks on its changes.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct CommitTicks {
    /// The file is ticked in the Changes list.
    pub included: bool,
    /// Lines of HEAD's side, and of the working tree's, that stay out of the commit.
    pub removed: HashSet<usize>,
    pub added: HashSet<usize>,
}

impl CommitTicks {
    pub fn is_excluded(&self, side: Side, line: usize) -> bool {
        !self.included || if side == Side::Left { self.removed.contains(&line) } else { self.added.contains(&line) }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Inclusion {
    Off,
    On,
    Mixed,
}

/// What the canvas asks of whoever shows it.
#[derive(Clone, Debug)]
pub enum CanvasAction {
    /// Escape with nothing to drop: back to the list the file was picked from.
    Exit,
    RevertHunk(usize),
    ToggleCommitHunk(usize),
    ResolveConflict(usize, ConflictChoice),
    OpenAnnotation(AnnotationEntry),
    /// The right button went down: a context menu for what is under `at` (window coordinates).
    ContextMenu { at: Point, hunk: Option<usize> },
    /// The text was changed here (typed, pasted, undone…).
    Edited,
    /// Scrolled by hand, so keyboard navigation continues from what is on screen.
    UserScrolled,
    /// Ctrl+wheel: a step bigger or smaller.
    Zoom(i32),
}

/// The code font's metrics, rounded the way rows are laid out.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CodeFont {
    pub font: Font,
    pub line_height: f32,
    pub baseline: f32,
    pub char_width: f32,
}

impl CodeFont {
    pub fn new(ui: &Ui, font: Font) -> Self {
        let m = ui.backend.metrics(font);
        let line_height = (font.size * 1.5).round();
        let baseline = ((line_height - (m.ascent + m.descent)) / 2.0 + m.ascent).round();
        CodeFont { font, line_height, baseline, char_width: m.char_width.max(1.0) }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Drag {
    None,
    Ruler,
    Select,
    HScroll { grab: f32 },
    Revert(usize),
    Tick(usize),
    Conflict(usize, ConflictChoice),
}

#[derive(Clone, Copy, Debug)]
struct Animation {
    from: f32,
    to: f32,
    start: Instant,
}

/// Where the first view of a newly loaded document goes, once the canvas has a size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum InitialPosition {
    /// A 1-based line of the new side.
    Line(usize),
    /// The first change (or the last, stepping back from the next file); the top when there is none.
    Change { last: bool },
    /// Keep the scroll position.
    Keep,
}

#[derive(Clone, Copy, Debug)]
struct Scroll {
    /// Content y shown at the top of each pane.
    left: f32,
    right: f32,
    x: f32,
    /// The clamped virtual offset.
    y: f32,
}

const RULER: f32 = 12.0;
const CONNECTOR: f32 = 44.0;
const TEXT_INSET: f32 = 10.0;
const MAX_DISPLAY_BYTES: usize = 6000;
const REMOVED_MARK: u32 = 0xE5534B;
const HSCROLL: f32 = 8.0;

pub struct CodeCanvas {
    doc: Option<DiffDocument>,
    requested_mode: Mode,
    pub editor: Option<EditorSession>,
    /// The scroll origin: x, and the virtual y.
    origin: Point,
    animation: Option<Animation>,
    pub selection: Option<TextSel>,
    pub focused_hunk: Option<usize>,
    /// A line to tint: the target of Go to Line, say.
    pub focus_line: Option<(Side, usize)>,
    /// Every change gets a button that puts it back the way the left side has it.
    pub shows_revert_buttons: bool,
    /// Every change gets a tick for the next commit; lines that stay out are pale.
    commit_ticks: Option<CommitTicks>,
    tick_cache: Option<Vec<Inclusion>>,
    /// Conflicts git left in the right side (a file being merged), each with buttons that resolve it.
    pub conflicts: Vec<ConflictBlock>,
    /// Who last changed each line of the right side, in a column before its line numbers.
    annotations: Option<LineAnnotations>,
    pub matches: Vec<SearchMatch>,
    pub current_match: Option<usize>,
    matches_by_line: [HashMap<usize, Vec<Range<usize>>>; 2],
    hunk_rows: Vec<usize>,
    /// Where vertical caret moves try to stay.
    goal_x: Option<f32>,
    /// The last whole line copied with no selection, so pasting it goes in as a line.
    line_clipboard: Option<String>,
    /// Counts edits of the right side, so work started before one can tell it is out of date.
    pub edit_count: u64,
    pending: Option<InitialPosition>,
    pending_line_reveal: Option<usize>,
    /// The caret moved or the text changed: scroll it into view on the next frame.
    reveal_pending: bool,
    // From the last frame.
    bounds: Rect,
    font: Option<CodeFont>,
    colors: Option<CanvasColors>,
    drag: Drag,
    hovered_revert: Option<usize>,
    hovered_tick: Option<usize>,
    hovered_conflict: Option<(usize, ConflictChoice)>,
    hovered_annotation: Option<usize>,
    revert_hits: Vec<(Rect, usize)>,
    tick_hits: Vec<(Rect, usize)>,
    conflict_hits: Vec<(Rect, usize, ConflictChoice)>,
    /// The commit of the annotated line under the mouse, and where its card goes (window coordinates).
    pub annotation_hover: Option<(AnnotationEntry, Point)>,
    blink_from: Instant,
    actions: Vec<CanvasAction>,
    /// Set when an input method's composition was ended here; the shell tells the input method.
    pub ime_cancel: bool,
}

impl Default for CodeCanvas {
    fn default() -> Self {
        Self::new()
    }
}

impl CodeCanvas {
    pub fn new() -> Self {
        CodeCanvas {
            doc: None,
            requested_mode: Mode::SideBySide,
            editor: None,
            origin: Point::default(),
            animation: None,
            selection: None,
            focused_hunk: None,
            focus_line: None,
            shows_revert_buttons: false,
            commit_ticks: None,
            tick_cache: None,
            conflicts: Vec::new(),
            annotations: None,
            matches: Vec::new(),
            current_match: None,
            matches_by_line: [HashMap::new(), HashMap::new()],
            hunk_rows: Vec::new(),
            goal_x: None,
            line_clipboard: None,
            edit_count: 0,
            pending: None,
            pending_line_reveal: None,
            reveal_pending: false,
            bounds: Rect::ZERO,
            font: None,
            colors: None,
            drag: Drag::None,
            hovered_revert: None,
            hovered_tick: None,
            hovered_conflict: None,
            hovered_annotation: None,
            revert_hits: Vec::new(),
            tick_hits: Vec::new(),
            conflict_hits: Vec::new(),
            annotation_hover: None,
            blink_from: Instant::now(),
            actions: Vec::new(),
            ime_cancel: false,
        }
    }

    // MARK: Configuration

    pub fn document(&self) -> Option<&DiffDocument> {
        self.doc.as_ref()
    }

    pub fn document_mut(&mut self) -> Option<&mut DiffDocument> {
        self.doc.as_mut()
    }

    pub fn is_text(&self) -> bool {
        self.doc.as_ref().is_some_and(|d| matches!(d.content, Content::Text))
    }

    pub fn mode(&self) -> Mode {
        match &self.doc {
            Some(d) if d.is_plain_file => Mode::Single,
            Some(_) => self.requested_mode,
            None => Mode::Single,
        }
    }

    /// Replaces the document. `keep_scroll` keeps the viewport where it was (a live reload).
    pub fn set_document(&mut self, doc: Option<DiffDocument>, mode: Mode, keep_scroll: bool) {
        self.doc = doc;
        self.editor = None;
        self.annotations = None;
        self.requested_mode = mode;
        self.selection = None;
        self.focused_hunk = None;
        self.focus_line = None;
        self.matches.clear();
        self.matches_by_line = [HashMap::new(), HashMap::new()];
        self.current_match = None;
        self.animation = None;
        self.annotation_hover = None;
        self.drag = Drag::None;
        self.tick_cache = None;
        self.rebuild_hunk_rows();
        if keep_scroll {
            self.pending = Some(InitialPosition::Keep);
        } else {
            self.origin = Point::default();
        }
    }

    /// Takes the document out (to hand it to a session that edits it, say).
    pub fn take_document(&mut self) -> Option<DiffDocument> {
        self.editor = None;
        self.doc.take()
    }

    pub fn set_mode(&mut self, mode: Mode) {
        if mode == self.requested_mode {
            return;
        }
        // The caret stays on its line; rows mean different things in the two layouts.
        let caret = self.edit_selection();
        self.requested_mode = mode;
        self.selection = None;
        self.rebuild_hunk_rows();
        if let Some(c) = caret {
            self.selection = Some(TextSel { pane: Side::Right, anchor: self.row_position(c.anchor), head: self.row_position(c.head) });
        }
        self.clamp_scroll();
    }

    pub fn set_commit_ticks(&mut self, ticks: Option<CommitTicks>) {
        if ticks != self.commit_ticks {
            self.commit_ticks = ticks;
            self.tick_cache = None;
        }
    }

    pub fn annotations(&self) -> Option<&LineAnnotations> {
        self.annotations.as_ref()
    }

    pub fn set_annotations(&mut self, annotations: Option<LineAnnotations>) {
        self.annotations = annotations;
        self.hovered_annotation = None;
        self.annotation_hover = None;
    }

    /// All occurrences on each side, and the ones Enter steps through.
    pub fn set_highlights(&mut self, by_side: [HashMap<usize, Vec<Range<usize>>>; 2], navigation: Vec<SearchMatch>, current: Option<usize>) {
        self.matches = navigation;
        self.current_match = current;
        self.matches_by_line = by_side;
    }

    pub fn set_current_match(&mut self, index: Option<usize>) {
        self.current_match = index;
    }

    /// Where the view goes once laid out: a line, the first change, or where it was.
    pub fn position_when_laid_out(&mut self, position: InitialPosition) {
        self.pending = Some(position);
    }

    fn rebuild_hunk_rows(&mut self) {
        self.tick_cache = None;
        let Some(doc) = &self.doc else {
            self.hunk_rows.clear();
            return;
        };
        let mut rows = vec![0usize; doc.hunks().len()];
        match self.mode() {
            Mode::SideBySide => {
                for s in doc.segments() {
                    if let Some(h) = s.hunk {
                        if h < rows.len() {
                            rows[h] = s.virtual_start;
                        }
                    }
                }
            }
            Mode::Unified => {
                let mut seen: i64 = -1;
                for (index, row) in doc.unified_rows().iter().enumerate() {
                    if let Some(h) = row.hunk {
                        if h as i64 > seen {
                            seen = h as i64;
                            if (h as usize) < rows.len() {
                                rows[h as usize] = index;
                            }
                        }
                    }
                }
            }
            Mode::Single => rows.clear(),
        }
        self.hunk_rows = rows;
    }

    // MARK: Geometry

    pub fn font(&self) -> CodeFont {
        self.font.unwrap_or(CodeFont { font: Font::mono(12.0), line_height: 18.0, baseline: 13.0, char_width: 7.2 })
    }

    fn lh(&self) -> f32 {
        self.font().line_height
    }

    fn bottom_padding(&self) -> f32 {
        self.lh() * 3.0
    }

    pub fn row_count(&self) -> usize {
        let Some(doc) = &self.doc else { return 0 };
        if !matches!(doc.content, Content::Text) {
            return 0;
        }
        match self.mode() {
            Mode::SideBySide => doc.virtual_line_count(),
            Mode::Unified => doc.unified_rows().len(),
            Mode::Single => doc.right.len(),
        }
    }

    pub fn hunk_row(&self, index: usize) -> usize {
        self.hunk_rows.get(index).copied().unwrap_or(0)
    }

    /// The row (in the current mode's scroll space) that shows `line` of `side`.
    pub fn row_for_line(&self, line: usize, side: Side) -> usize {
        let Some(doc) = &self.doc else { return 0 };
        match self.mode() {
            Mode::SideBySide => doc.virtual_position(line, side) as usize,
            Mode::Unified => doc.unified_row_of_line(line, side),
            Mode::Single => line,
        }
    }

    fn digits(n: usize) -> usize {
        n.max(1).to_string().len().max(3)
    }

    pub fn gutter_digits(&self) -> usize {
        self.doc.as_ref().map_or(3, |d| Self::digits(d.left.len().max(d.right.len())))
    }

    fn gutter_width(&self) -> f32 {
        if self.doc.is_none() {
            return 40.0;
        }
        (self.gutter_digits() as f32 * self.font().char_width).ceil() + 20.0
    }

    fn pane_width(&self) -> f32 {
        ((self.bounds.w - RULER - CONNECTOR) / 2.0).floor()
    }

    fn annotation_width(&self) -> f32 {
        match (&self.annotations, &self.doc) {
            (Some(a), Some(_)) => (a.columns as f32 * self.font().char_width).ceil() + 16.0,
            _ => 0.0,
        }
    }

    /// Where the Annotate column starts, relative to the canvas.
    fn annotation_x(&self) -> f32 {
        if self.mode() == Mode::SideBySide {
            self.pane_width() + CONNECTOR
        } else {
            0.0
        }
    }

    fn unified_number_width(&self) -> f32 {
        self.gutter_width() - 6.0
    }

    fn unified_gutter_width(&self) -> f32 {
        self.unified_number_width() * 2.0 + 18.0
    }

    /// The text area of a pane, relative to the canvas.
    fn text_rect(&self, pane: Side) -> Rect {
        let (w, h, g, a) = (self.bounds.w, self.bounds.h, self.gutter_width(), self.annotation_width());
        match self.mode() {
            Mode::SideBySide => {
                let half = self.pane_width();
                if pane == Side::Left {
                    Rect::new(0.0, 0.0, (half - g).max(0.0), h)
                } else {
                    let x = half + CONNECTOR + a + g;
                    Rect::new(x, 0.0, (w - RULER - x).max(0.0), h)
                }
            }
            Mode::Unified => {
                let x = a + self.unified_gutter_width();
                Rect::new(x, 0.0, (w - RULER - x).max(0.0), h)
            }
            Mode::Single => Rect::new(a + g, 0.0, (w - RULER - a - g).max(0.0), h),
        }
    }

    /// A pane including its gutter, relative to the canvas.
    fn band_rect(&self, pane: Side) -> Rect {
        let (w, h) = (self.bounds.w, self.bounds.h);
        if self.mode() != Mode::SideBySide {
            return Rect::new(0.0, 0.0, (w - RULER).max(0.0), h);
        }
        let half = self.pane_width();
        if pane == Side::Left {
            Rect::new(0.0, 0.0, half, h)
        } else {
            Rect::new(half + CONNECTOR, 0.0, (w - RULER - half - CONNECTOR).max(0.0), h)
        }
    }

    /// How far the text can scroll sideways.
    fn max_x(&self) -> f32 {
        let Some(doc) = &self.doc else { return 0.0 };
        if !matches!(doc.content, Content::Text) {
            return 0.0;
        }
        let (columns, available) = match self.mode() {
            Mode::SideBySide => {
                (doc.left.max_columns().max(doc.right.max_columns()), self.text_rect(Side::Left).w.min(self.text_rect(Side::Right).w))
            }
            Mode::Unified => (doc.left.max_columns().max(doc.right.max_columns()), self.text_rect(Side::Right).w),
            Mode::Single => (doc.right.max_columns(), self.text_rect(Side::Right).w),
        };
        let content = columns.min(4000) as f32 * self.font().char_width + TEXT_INSET + 48.0;
        (content - available).max(0.0)
    }

    fn max_y(&self) -> f32 {
        (self.row_count() as f32 * self.lh() + self.bottom_padding() - self.bounds.h).max(0.0)
    }

    fn clamp_scroll(&mut self) {
        self.origin.x = self.origin.x.clamp(0.0, self.max_x());
        self.origin.y = self.origin.y.clamp(0.0, self.max_y());
    }

    /// Resolves the scroll offset into per-pane offsets.
    ///
    /// Side by side: an anchor line slides from the top of the viewport (at the start of the file) to the
    /// bottom (at the end). Both panes are placed so the lines at the anchor's virtual position sit at the
    /// anchor, which keeps unchanged code level across the divider while each pane starts and ends flush.
    fn resolve(&self) -> Scroll {
        let h = self.bounds.h;
        let lh = self.lh();
        let pad = self.bottom_padding();
        let max_scroll = self.max_y();
        let y = self.origin.y.clamp(0.0, max_scroll);
        let x = self.origin.x.clamp(0.0, self.max_x()).round();
        let doc = match &self.doc {
            Some(d) if self.mode() == Mode::SideBySide => d,
            _ => return Scroll { left: y.round(), right: y.round(), x, y },
        };
        let k = if max_scroll > 0.0 { y / max_scroll } else { 0.0 };
        let anchor = k * (h - pad).max(0.0);
        let (pl, pr) = doc.pane_positions(((y + anchor) / lh) as f64);
        let top = |position: f64, count: usize| {
            let limit = (count as f32 * lh + pad - h).max(0.0);
            (position as f32 * lh - anchor).clamp(0.0, limit).round()
        };
        Scroll { left: top(pl, doc.left.len()), right: top(pr, doc.right.len()), x, y }
    }

    /// The buffer line a row of a pane shows.
    fn source(&self, pane: Side, row: usize) -> Option<(Side, usize)> {
        let doc = self.doc.as_ref()?;
        match self.mode() {
            Mode::SideBySide => (row < doc.buffer(pane).len()).then_some((pane, row)),
            Mode::Unified => {
                let r = doc.unified_rows().get(row)?;
                if r.kind == RowKind::Removed {
                    Some((Side::Left, r.left? as usize))
                } else {
                    Some((Side::Right, r.right? as usize))
                }
            }
            Mode::Single => (row < doc.right.len()).then_some((Side::Right, row)),
        }
    }

    fn pane_row_count(&self, pane: Side) -> usize {
        let Some(doc) = &self.doc else { return 0 };
        match self.mode() {
            Mode::SideBySide => doc.buffer(pane).len(),
            Mode::Unified => doc.unified_rows().len(),
            Mode::Single => doc.right.len(),
        }
    }

    /// The first hunk that ends at or after `line` on `side`.
    fn first_hunk(doc: &DiffDocument, side: Side, line: usize) -> usize {
        let hunks = doc.hunks();
        let (mut lo, mut hi) = (0, hunks.len());
        while lo < hi {
            let mid = (lo + hi) / 2;
            let h = &hunks[mid];
            let end = if side == Side::Left { h.left_start + h.left_count } else { h.right_start + h.right_count };
            if end < line {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        lo
    }

    pub fn top_row(&self) -> usize {
        (self.origin.y / self.lh()).max(0.0) as usize
    }

    pub fn visible_row_count(&self) -> usize {
        (self.bounds.h / self.lh()).max(1.0) as usize
    }

    pub fn visible_line_count(&self) -> usize {
        self.visible_row_count()
    }

    // MARK: Scrolling

    pub fn scroll_y(&self) -> f32 {
        self.origin.y
    }

    pub fn scroll_by(&mut self, dx: f32, dy: f32) {
        self.animation = None;
        self.origin.x = (self.origin.x + dx).clamp(0.0, self.max_x());
        self.origin.y = (self.origin.y + dy).clamp(0.0, self.max_y());
    }

    pub fn scroll_to_y(&mut self, target: f32, animated: bool) {
        let to = target.clamp(0.0, self.max_y());
        if !animated || (to - self.origin.y).abs() <= 1.0 {
            self.animation = None;
            self.origin.y = to;
            return;
        }
        // A short ease-out; long jumps start close by so they still feel instant.
        let from = if (to - self.origin.y).abs() > 2400.0 { to - 2400.0f32.copysign(to - self.origin.y) } else { self.origin.y };
        self.animation = Some(Animation { from, to, start: Instant::now() });
    }

    /// Scrolls so `row` sits `fraction` of the way down the viewport.
    pub fn scroll_to_row(&mut self, row: usize, fraction: f32, animated: bool) {
        self.scroll_to_y(row as f32 * self.lh() - self.bounds.h * fraction, animated);
    }

    fn step_animation(&mut self, ui: &mut Ui) {
        let Some(a) = self.animation else { return };
        let t = (a.start.elapsed().as_secs_f32() / 0.16).min(1.0);
        let eased = 1.0 - (1.0 - t).powi(3);
        self.origin.y = (a.from + (a.to - a.from) * eased).clamp(0.0, self.max_y());
        if t >= 1.0 {
            self.animation = None;
        } else {
            ui.request_repaint();
        }
    }

    // MARK: Text

    /// A row's text laid out, coloured by its syntax.
    fn line_layout(&self, ui: &mut Ui, side: Side, line: usize) -> Rc<dyn TextLayout> {
        let font = self.font();
        let colors = self.colors.as_ref();
        let mut style = TextStyle::new(font.font);
        style.tab_width = 4;
        let Some(doc) = &self.doc else { return ui.layout(TextRequest { text: Arc::from(""), style, runs: Vec::new(), bold_ranges: Vec::new() }) };
        let buffer = doc.buffer(side);
        let Some(text) = buffer.lines().get(line) else {
            return ui.layout(TextRequest { text: Arc::from(""), style, runs: Vec::new(), bold_ranges: Vec::new() });
        };
        let shown: Arc<str> = if text.len() > MAX_DISPLAY_BYTES {
            let cut = text_editing::floor_boundary(text, MAX_DISPLAY_BYTES / 2);
            Arc::from(format!("{} …", &text[..cut]))
        } else {
            Arc::from(text.as_str())
        };
        let mut runs = Vec::new();
        if let Some(c) = colors {
            for span in buffer.spans_at(line) {
                if span.kind == TokenKind::Plain {
                    continue;
                }
                let start = span.start as usize;
                let end = (start + span.length as usize).min(shown.len());
                if start < end {
                    runs.push(ColorRun { range: start as u32..end as u32, color: c.token(span.kind) });
                }
            }
        }
        ui.layout(TextRequest { text: shown, style, runs, bold_ranges: Vec::new() })
    }

    /// Distance of a column from the start of a line, as laid out.
    fn x_of(&self, ui: &mut Ui, side: Side, line: usize, column: usize) -> f32 {
        let layout = self.line_layout(ui, side, line);
        layout.caret(column).x
    }

    /// The column nearest a distance from the start of a line.
    fn column_at(&self, ui: &mut Ui, side: Side, line: usize, x: f32) -> usize {
        let layout = self.line_layout(ui, side, line);
        let len = self.doc.as_ref().and_then(|d| d.buffer(side).lines().get(line)).map_or(0, |t| t.len());
        layout.hit_test(Point::new(x, layout.baseline())).min(len)
    }

    fn line_text(&self, side: Side, line: usize) -> &str {
        self.doc.as_ref().and_then(|d| d.buffer(side).lines().get(line)).map_or("", |s| s.as_str())
    }

    fn row_text(&self, pane: Side, row: usize) -> &str {
        match self.source(pane, row) {
            Some((side, line)) => self.line_text(side, line),
            None => "",
        }
    }

    // MARK: Editing support

    /// The selection as positions of the right side's lines, when it is somewhere that can be edited.
    pub fn edit_selection(&self) -> Option<crate::core::editor_session::Selection> {
        self.editor.as_ref()?;
        let sel = self.selection?;
        if self.mode() == Mode::SideBySide && sel.pane != Side::Right {
            return None;
        }
        Some(crate::core::editor_session::Selection { anchor: self.line_position(sel.anchor), head: self.line_position(sel.head) })
    }

    /// A row of the right pane as a position in the right side's lines. In the unified view a removed row stands
    /// for the start of the next line that is still there.
    pub fn line_position(&self, p: TextPosition) -> TextPosition {
        let Some(doc) = &self.doc else { return TextPosition::new(0, 0) };
        let lines = doc.right.lines();
        if lines.is_empty() {
            return TextPosition::new(0, 0);
        }
        if self.mode() != Mode::Unified {
            return text_editing::clamp(p, lines);
        }
        let rows = doc.unified_rows();
        if rows.is_empty() {
            return TextPosition::new(0, 0);
        }
        let mut row = p.row.min(rows.len() - 1);
        if let Some(r) = rows[row].right {
            return text_editing::clamp(TextPosition::new(r as usize, p.column), lines);
        }
        while row < rows.len() && rows[row].right.is_none() {
            row += 1;
        }
        if row < rows.len() {
            return TextPosition::new(rows[row].right.unwrap() as usize, 0);
        }
        let last = lines.len() - 1;
        TextPosition::new(last, lines[last].len())
    }

    /// A position in the right side as a row of the right pane.
    pub fn row_position(&self, p: TextPosition) -> TextPosition {
        TextPosition::new(self.edit_row(p.row), p.column)
    }

    pub fn edit_row(&self, line: usize) -> usize {
        match (&self.doc, self.mode()) {
            (Some(doc), Mode::Unified) => doc.unified_row_of_line(line, Side::Right),
            _ => line,
        }
    }

    // MARK: Hunks under the mouse

    /// The change shown at `p` (relative to the canvas), if there is one there.
    fn hunk_at(&self, p: Point) -> Option<usize> {
        let doc = self.doc.as_ref()?;
        if !matches!(doc.content, Content::Text) || p.x >= self.bounds.w - RULER {
            return None;
        }
        if let Some(h) = self.revert_hit(p).or(self.tick_hit(p)) {
            return Some(h);
        }
        let side = self.pane_at(p);
        let scroll = self.resolve();
        let top = if side == Side::Left { scroll.left } else { scroll.right };
        let row = ((p.y + top) / self.lh()).floor();
        if row < 0.0 || row as usize >= self.pane_row_count(side) {
            return None;
        }
        let row = row as usize;
        match self.mode() {
            Mode::Unified => doc.unified_rows()[row].hunk.map(|h| h as usize),
            Mode::SideBySide => {
                let index = Self::first_hunk(doc, side, row);
                let hunk = doc.hunks().get(index)?;
                let (start, count) = if side == Side::Left { (hunk.left_start, hunk.left_count) } else { (hunk.right_start, hunk.right_count) };
                (row >= start && row < start + count).then_some(index)
            }
            Mode::Single => None,
        }
    }

    fn pane_at(&self, p: Point) -> Side {
        if self.mode() != Mode::SideBySide {
            return Side::Right;
        }
        if p.x < self.pane_width() + CONNECTOR / 2.0 {
            Side::Left
        } else {
            Side::Right
        }
    }

    fn revert_hit(&self, p: Point) -> Option<usize> {
        self.revert_hits.iter().find(|(r, _)| r.inset(-3.0).contains(p)).map(|(_, h)| *h)
    }

    fn tick_hit(&self, p: Point) -> Option<usize> {
        self.tick_hits.iter().find(|(r, _)| r.inset(-3.0).contains(p)).map(|(_, h)| *h)
    }

    fn conflict_hit(&self, p: Point) -> Option<(usize, ConflictChoice)> {
        self.conflict_hits.iter().find(|(r, _, _)| r.inset(-2.0).contains(p)).map(|(_, b, c)| (*b, *c))
    }

    /// The right side's line a row of the right pane shows.
    fn right_line(&self, row: usize) -> Option<usize> {
        let doc = self.doc.as_ref()?;
        match self.mode() {
            Mode::SideBySide | Mode::Single => (row < doc.right.len()).then_some(row),
            Mode::Unified => doc.unified_rows().get(row)?.right.map(|r| r as usize),
        }
    }

    /// The line whose Annotate label is under `p`, when it has a commit.
    fn annotated_line(&self, p: Point) -> Option<usize> {
        let width = self.annotation_width();
        let x0 = self.annotation_x();
        let annotations = self.annotations.as_ref()?;
        if width <= 0.0 || p.x < x0 || p.x >= x0 + width {
            return None;
        }
        let row = ((p.y + self.resolve().right) / self.lh()).floor();
        if row < 0.0 {
            return None;
        }
        let line = self.right_line(row as usize)?;
        annotations.index_at_line(line).map(|_| line)
    }

    /// The pane position under `p` (relative to the canvas).
    fn position_at(&self, ui: &mut Ui, p: Point, pane: Side) -> Option<TextPosition> {
        let count = self.pane_row_count(pane);
        if count == 0 {
            return None;
        }
        let scroll = self.resolve();
        let top = if pane == Side::Left { scroll.left } else { scroll.right };
        let raw = ((p.y + top) / self.lh()).floor();
        if raw < 0.0 {
            return Some(TextPosition::new(0, 0));
        }
        let row = (raw as usize).min(count - 1);
        let (side, line) = self.source(pane, row)?;
        if raw as usize >= count {
            return Some(TextPosition::new(row, self.line_text(side, line).len()));
        }
        let x = p.x - (self.text_rect(pane).x + TEXT_INSET - scroll.x);
        Some(TextPosition::new(row, self.column_at(ui, side, line, x)))
    }

    // MARK: Ticks

    fn tick_states(&mut self) -> Vec<Inclusion> {
        let Some(doc) = &self.doc else { return Vec::new() };
        if let Some(c) = &self.tick_cache {
            if c.len() == doc.hunks().len() {
                return c.clone();
            }
        }
        let Some(ticks) = &self.commit_ticks else { return Vec::new() };
        let count = |range: Range<usize>, lines: &HashSet<usize>| -> usize {
            if range.len() < lines.len() {
                range.filter(|l| lines.contains(l)).count()
            } else {
                lines.iter().filter(|&&l| range.contains(&l)).count()
            }
        };
        let states: Vec<Inclusion> = doc
            .hunks()
            .iter()
            .map(|h| {
                if !ticks.included {
                    return Inclusion::Off;
                }
                let left = h.left_start..h.left_start + h.left_count;
                let right = h.right_start..h.right_start + h.right_count;
                let total = left.len() + right.len();
                let out = count(left, &ticks.removed) + count(right, &ticks.added);
                if out == 0 {
                    Inclusion::On
                } else if out == total {
                    Inclusion::Off
                } else {
                    Inclusion::Mixed
                }
            })
            .collect();
        self.tick_cache = Some(states.clone());
        states
    }

    fn revert_size(&self) -> f32 {
        (self.lh() - 2.0).round().clamp(13.0, 18.0)
    }

    fn tick_size(&self) -> f32 {
        (self.lh() - 4.0).round().clamp(12.0, 15.0)
    }

    /// Lays out the buttons on each visible change: in the ribbon between the panes, or at the right edge in the
    /// unified view; with ticks, each change's tick too.
    fn layout_buttons(&mut self, ui: &mut Ui, scroll: &Scroll) {
        self.revert_hits.clear();
        self.tick_hits.clear();
        self.conflict_hits.clear();
        let mode = self.mode();
        let reverts = self.shows_revert_buttons;
        let ticks = self.commit_ticks.is_some();
        let states = if ticks { self.tick_states() } else { Vec::new() };
        let lh = self.lh();
        let h = self.bounds.h;
        if let Some(doc) = &self.doc {
            if (reverts || ticks) && matches!(mode, Mode::SideBySide | Mode::Unified) && !doc.hunks().is_empty() {
                let (size, boxs) = (self.revert_size(), self.tick_size());
                let pw = self.pane_width();
                let (x, tick_x) = if mode == Mode::SideBySide {
                    (
                        if reverts && ticks { pw + CONNECTOR - size - 4.0 } else { pw + (CONNECTOR - size) / 2.0 },
                        if reverts && ticks { pw + 4.0 } else { pw + (CONNECTOR - boxs) / 2.0 },
                    )
                } else {
                    let x = self.bounds.w - RULER - size - 10.0;
                    (x, if reverts { x - boxs - 6.0 } else { self.bounds.w - RULER - boxs - 10.0 })
                };
                let (x, tick_x) = (x.round(), tick_x.round());
                let mut index = if mode == Mode::SideBySide {
                    Self::first_hunk(doc, Side::Left, (scroll.left / lh).floor().max(0.0) as usize)
                        .min(Self::first_hunk(doc, Side::Right, (scroll.right / lh).floor().max(0.0) as usize))
                } else {
                    let top_row = (scroll.right / lh).floor().max(0.0) as usize;
                    let (mut lo, mut hi) = (0, doc.hunks().len());
                    while lo < hi {
                        let mid = (lo + hi) / 2;
                        let hk = &doc.hunks()[mid];
                        if self.hunk_row(mid) + hk.left_count + hk.right_count < top_row {
                            lo = mid + 1;
                        } else {
                            hi = mid;
                        }
                    }
                    lo
                };
                while index < doc.hunks().len() {
                    let hunk = &doc.hunks()[index];
                    let (bottom, rest);
                    if mode == Mode::SideBySide {
                        let l0 = hunk.left_start as f32 * lh - scroll.left;
                        let r0 = hunk.right_start as f32 * lh - scroll.right;
                        if l0.min(r0) > h {
                            break;
                        }
                        bottom = (l0 + hunk.left_count as f32 * lh).max(r0 + hunk.right_count as f32 * lh);
                        let left = l0 + if hunk.left_count > 0 { lh / 2.0 } else { 0.0 };
                        let right = r0 + if hunk.right_count > 0 { lh / 2.0 } else { 0.0 };
                        rest = (left + right) / 2.0 - size / 2.0;
                    } else {
                        let top = self.hunk_row(index) as f32 * lh - scroll.right;
                        if top > h {
                            break;
                        }
                        bottom = top + (hunk.left_count + hunk.right_count) as f32 * lh;
                        rest = top + (lh - size) / 2.0;
                    }
                    // A change taller than the viewport keeps its buttons in sight while any of it is.
                    let y = rest.max(3.0f32.min(bottom - size - 3.0)).round();
                    if bottom >= 0.0 && ticks && index < states.len() {
                        self.tick_hits.push((Rect::new(tick_x, (y + (size - boxs) / 2.0).round(), boxs, boxs), index));
                    }
                    if bottom >= 0.0 && reverts {
                        self.revert_hits.push((Rect::new(x, y, size, size), index));
                    }
                    index += 1;
                }
            }
            // Three buttons on each conflict's `<<<<<<<` line, at the right edge.
            if !self.conflicts.is_empty() && (mode != Mode::Single || doc.is_plain_file) {
                let font = Font::ui((self.font().font.size - 1.0).clamp(10.0, 13.0)).weight(Weight::Semibold);
                let right = self.bounds.w - RULER - 10.0;
                for (index, block) in self.conflicts.iter().enumerate() {
                    let row = if mode == Mode::Unified { doc.unified_row_of_line(block.start, Side::Right) } else { block.start };
                    let top = row as f32 * lh - scroll.right;
                    if top <= -lh || top >= h {
                        continue;
                    }
                    let mut x = right;
                    for (choice, title) in Self::conflict_choices().into_iter().rev() {
                        let text_w = ui.text_width(&title, font);
                        let pill = Size::new((text_w + 14.0).ceil(), (lh - 2.0).min(font.size * 1.3 + 4.0).ceil());
                        x -= pill.w;
                        self.conflict_hits.push((Rect::new(x.round(), (top + (lh - pill.h) / 2.0).round(), pill.w, pill.h), index, choice));
                        x -= 6.0;
                    }
                }
            }
        }
        if self.hovered_revert.is_some_and(|h| !self.revert_hits.iter().any(|(_, i)| *i == h)) {
            self.hovered_revert = None;
        }
        if self.hovered_tick.is_some_and(|h| !self.tick_hits.iter().any(|(_, i)| *i == h)) {
            self.hovered_tick = None;
        }
    }

    fn conflict_choices() -> [(ConflictChoice, String); 3] {
        [
            (ConflictChoice::Ours, l("Keep HEAD", "保留 HEAD")),
            (ConflictChoice::Theirs, l("Take Incoming", "采用传入")),
            (ConflictChoice::Both, l("Both", "两者都要")),
        ]
    }

    // MARK: The frame

    /// Draws the canvas in `rect` and handles the mouse over it. Returns what it asks of its owner.
    pub fn show(&mut self, ui: &mut Ui, id: Id, rect: Rect, focused: bool) -> Vec<CanvasAction> {
        let theme = ui.theme;
        self.colors = Some(theme.canvas.clone());
        let font = CodeFont::new(ui, theme.code_font());
        if self.font != Some(font) {
            self.font = Some(font);
        }
        let resized = self.bounds.size() != rect.size();
        self.bounds = rect;
        if resized {
            self.clamp_scroll();
        }
        if let Some(position) = self.pending.take() {
            if rect.h > 1.0 {
                self.apply_initial_position(ui, position);
            } else {
                self.pending = Some(position);
            }
        }
        self.step_animation(ui);
        if !focused && self.has_composition() {
            // Whatever an input method was composing goes in as typed, rather than being lost.
            self.commit_composition();
        }
        if std::mem::take(&mut self.reveal_pending) {
            self.reveal_caret(ui);
        }

        // The wheel; Ctrl+wheel zooms.
        if let Some(delta) = ui.scroll_target(id.with("scroll"), rect) {
            if ui.mods().ctrl {
                if delta.y.abs() > 0.5 {
                    self.actions.push(CanvasAction::Zoom(if delta.y < 0.0 { 1 } else { -1 }));
                }
            } else {
                self.scroll_by(delta.x, delta.y);
                self.actions.push(CanvasAction::UserScrolled);
                if self.hovered_annotation.is_some() {
                    self.hovered_annotation = None;
                    self.annotation_hover = None;
                }
            }
        }

        let scroll = self.resolve();
        self.layout_buttons(ui, &scroll);
        let r = ui.interact(id, rect);
        self.handle_mouse(ui, id, &r, focused);
        let scroll = self.resolve();

        let colors = theme.canvas.clone();
        ui.clipped(rect, |ui| {
            ui.fill(rect, colors.background);
            self.draw(ui, &scroll, &colors, focused);
        });
        if std::mem::take(&mut self.ime_cancel) {
            ui.st.ime_reset = true;
        }
        std::mem::take(&mut self.actions)
    }

    fn apply_initial_position(&mut self, ui: &mut Ui, position: InitialPosition) {
        self.clamp_scroll();
        match position {
            InitialPosition::Keep => {
                if let Some(line) = self.pending_line_reveal.take() {
                    self.reveal_line(ui, line);
                }
            }
            InitialPosition::Line(line) => self.reveal_line(ui, line),
            InitialPosition::Change { last } => {
                let Some(doc) = &self.doc else { return };
                if !doc.hunks().is_empty() && matches!(doc.content, Content::Text) && !doc.is_plain_file {
                    let index = if last { doc.hunks().len() - 1 } else { 0 };
                    let line = doc.hunks()[index].right_start;
                    self.focused_hunk = Some(index);
                    // Ready to type where the change is.
                    self.place_caret(line, 0);
                    let row = self.hunk_row(index);
                    if row > self.visible_row_count() / 3 {
                        self.scroll_to_row(row, 0.2, false);
                    }
                } else if self.edit_selection().is_none() {
                    self.place_caret(0, 0);
                }
            }
        }
    }

    /// Scrolls to a 1-based line of the new side and tints it.
    pub fn reveal_line(&mut self, _ui: &mut Ui, line: usize) {
        let Some(doc) = &self.doc else { return };
        if !matches!(doc.content, Content::Text) || doc.right.is_empty() {
            return;
        }
        let index = line.saturating_sub(1).min(doc.right.len() - 1);
        self.place_caret(index, 0);
        self.focus_line = Some((Side::Right, index));
        let row = self.row_for_line(index, Side::Right);
        self.scroll_to_row(row, 0.35, false);
    }

    /// Reveals a line once the canvas has a size (when it may not have one yet).
    pub fn reveal_line_later(&mut self, line: usize) {
        self.pending_line_reveal = Some(line);
        if self.pending.is_none() {
            self.pending = Some(InitialPosition::Keep);
        }
    }

    pub fn focus_hunk(&mut self, index: usize, animated: bool) {
        self.focused_hunk = Some(index);
        if let Some(line) = self.doc.as_ref().and_then(|d| d.hunks().get(index)).map(|h| h.right_start) {
            self.place_caret(line, 0);
        }
        self.scroll_to_row(self.hunk_row(index), 0.25, animated);
    }

    fn handle_mouse(&mut self, ui: &mut Ui, id: Id, r: &crate::ui::Response, focused: bool) {
        let origin = self.bounds.origin();
        let local = |p: Point| Point::new(p.x - origin.x, p.y - origin.y);
        let mouse = local(ui.mouse());

        // Hover.
        if r.hovered && self.drag == Drag::None {
            let line = self.annotated_line(mouse);
            if line != self.hovered_annotation {
                self.hovered_annotation = line;
                self.annotation_hover = line.and_then(|l| {
                    let entry = self.annotations.as_ref()?.entry_at_line(l)?.clone();
                    let row = self.edit_row(l);
                    let at = Point::new(
                        origin.x + self.annotation_x() + self.annotation_width(),
                        origin.y + row as f32 * self.lh() - self.resolve().right,
                    );
                    Some((entry, at))
                });
            }
            self.hovered_revert = self.revert_hit(mouse);
            self.hovered_tick = self.tick_hit(mouse);
            self.hovered_conflict = self.conflict_hit(mouse);
            let in_annotations = self.annotation_width() > 0.0
                && mouse.x >= self.annotation_x()
                && mouse.x < self.annotation_x() + self.annotation_width();
            let over_ruler = mouse.x >= self.bounds.w - RULER;
            let over_hscroll = self.max_x() > 0.0 && mouse.y >= self.bounds.h - HSCROLL;
            if self.hovered_revert.is_some() || self.hovered_tick.is_some() || self.hovered_conflict.is_some() || line.is_some() {
                ui.set_cursor(CursorIcon::Hand);
            } else if in_annotations || over_ruler || over_hscroll {
                ui.set_cursor(CursorIcon::Arrow);
            } else if self.is_text() {
                ui.set_cursor(CursorIcon::Text);
            }
        } else if !r.hovered && self.drag == Drag::None {
            self.hovered_revert = None;
            self.hovered_tick = None;
            self.hovered_conflict = None;
            if self.hovered_annotation.is_some() {
                self.hovered_annotation = None;
                self.annotation_hover = None;
            }
        }

        if r.secondary {
            ui.focus(id);
            let hunk = self.hunk_at(mouse);
            self.actions.push(CanvasAction::ContextMenu { at: ui.mouse(), hunk });
            ui.consume();
            return;
        }

        if r.pressed {
            ui.focus(id);
            // A click ends whatever an input method was composing.
            self.commit_composition();
            if let Some(e) = self.editor.as_mut() {
                e.close_group();
            }
            self.goal_x = None;
            self.blink_from = Instant::now();
            if mouse.x >= self.bounds.w - RULER {
                self.drag = Drag::Ruler;
                self.scroll_ruler(mouse.y);
                return;
            }
            if self.max_x() > 0.0 && mouse.y >= self.bounds.h - HSCROLL {
                let (thumb, _) = self.hscroll_thumb();
                let grab = if thumb.contains(Point::new(mouse.x, mouse.y)) { mouse.x - thumb.x } else { thumb.w / 2.0 };
                self.drag = Drag::HScroll { grab };
                self.drag_hscroll(mouse.x);
                return;
            }
            if let Some(h) = self.tick_hit(mouse) {
                self.drag = Drag::Tick(h);
                return;
            }
            if let Some(h) = self.revert_hit(mouse) {
                self.drag = Drag::Revert(h);
                return;
            }
            if let Some((b, c)) = self.conflict_hit(mouse) {
                self.drag = Drag::Conflict(b, c);
                return;
            }
            let aw = self.annotation_width();
            if aw > 0.0 && mouse.x >= self.annotation_x() && mouse.x < self.annotation_x() + aw {
                // A click on a line with a commit shows that commit; the column never selects text.
                if let Some(entry) = self.annotated_line(mouse).and_then(|l| self.annotations.as_ref()?.entry_at_line(l).cloned()) {
                    self.annotation_hover = None;
                    self.hovered_annotation = None;
                    self.actions.push(CanvasAction::OpenAnnotation(entry));
                }
                return;
            }
            let side = self.pane_at(mouse);
            let Some(pos) = self.position_at(ui, mouse, side) else {
                self.selection = None;
                return;
            };
            if r.triple_clicked {
                let len = self.row_text(side, pos.row).len();
                self.selection = Some(TextSel { pane: side, anchor: TextPosition::new(pos.row, 0), head: TextPosition::new(pos.row, len) });
            } else if r.double_clicked {
                let range = word_range(self.row_text(side, pos.row), pos.column);
                self.selection = Some(TextSel {
                    pane: side,
                    anchor: TextPosition::new(pos.row, range.start),
                    head: TextPosition::new(pos.row, range.end),
                });
            } else {
                match self.selection {
                    Some(current) if r.mods.shift && current.pane == side => {
                        self.selection = Some(TextSel { pane: side, anchor: current.anchor, head: pos })
                    }
                    _ => self.selection = Some(TextSel::caret(side, pos)),
                }
                self.drag = Drag::Select;
            }
            return;
        }

        if r.dragging {
            match self.drag {
                Drag::Ruler => self.scroll_ruler(mouse.y),
                Drag::HScroll { .. } => self.drag_hscroll(mouse.x),
                Drag::Select => {
                    if let Some(current) = self.selection {
                        if mouse.y < 0.0 {
                            self.scroll_by(0.0, mouse.y.max(-60.0));
                            ui.repaint_after(Duration::from_millis(16));
                        } else if mouse.y > self.bounds.h {
                            self.scroll_by(0.0, (mouse.y - self.bounds.h).min(60.0));
                            ui.repaint_after(Duration::from_millis(16));
                        }
                        if let Some(pos) = self.position_at(ui, mouse, current.pane) {
                            self.selection = Some(TextSel { pane: current.pane, anchor: current.anchor, head: pos });
                        }
                    }
                }
                _ => {}
            }
        }

        if let Some(Event::MouseUp { button: MouseButton::Left, pos, .. }) = &ui.event {
            if self.drag == Drag::None || ui.is_consumed() {
                return;
            }
            let at = local(*pos);
            let drag = std::mem::replace(&mut self.drag, Drag::None);
            match drag {
                Drag::Tick(h) => {
                    if self.tick_hit(at) == Some(h) {
                        self.actions.push(CanvasAction::ToggleCommitHunk(h));
                    }
                }
                Drag::Revert(h) => {
                    // Like any button: it fires only if the mouse is still on it when released.
                    if self.revert_hit(at) == Some(h) {
                        self.actions.push(CanvasAction::RevertHunk(h));
                    }
                }
                Drag::Conflict(b, c) => {
                    if self.conflict_hit(at) == Some((b, c)) {
                        self.actions.push(CanvasAction::ResolveConflict(b, c));
                    }
                }
                Drag::Select => {
                    if let Some(current) = self.selection {
                        if self.editor.is_some() && (self.mode() != Mode::SideBySide || current.pane == Side::Right) {
                            // In editable text an empty selection is the caret; in the unified view a click on a
                            // removed line lands on the line that follows it.
                            if self.mode() == Mode::Unified && current.is_empty() {
                                let caret = self.row_position(self.line_position(current.head));
                                self.selection = Some(TextSel::caret(Side::Right, caret));
                            }
                        } else if current.is_empty() && !focused {
                            self.selection = None;
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn ruler_scale(&self) -> f32 {
        let total = self.row_count() as f32 + self.bottom_padding() / self.lh();
        self.lh().min(self.bounds.h / total.max(1.0))
    }

    fn scroll_ruler(&mut self, y: f32) {
        let row = y / self.ruler_scale().max(0.0001);
        self.scroll_to_y(row * self.lh() - self.bounds.h / 2.0, false);
        self.actions.push(CanvasAction::UserScrolled);
    }

    /// The horizontal scrollbar's thumb and track, relative to the canvas.
    fn hscroll_thumb(&self) -> (Rect, Rect) {
        let track = Rect::new(0.0, self.bounds.h - HSCROLL, self.bounds.w - RULER, HSCROLL);
        let max = self.max_x();
        let view = self.text_rect(Side::Right).w.max(1.0);
        let total = view + max;
        let w = (track.w * view / total).max(24.0).min(track.w);
        let x = if max > 0.0 { (track.w - w) * (self.origin.x / max) } else { 0.0 };
        (Rect::new(x, track.y + 2.0, w, HSCROLL - 4.0), track)
    }

    fn drag_hscroll(&mut self, x: f32) {
        let Drag::HScroll { grab } = self.drag else { return };
        let (thumb, track) = self.hscroll_thumb();
        let span = (track.w - thumb.w).max(1.0);
        self.origin.x = (((x - grab) / span) * self.max_x()).clamp(0.0, self.max_x());
    }

    // MARK: Drawing

    fn draw(&mut self, ui: &mut Ui, scroll: &Scroll, c: &CanvasColors, focused: bool) {
        let Some(doc) = &self.doc else { return };
        if !matches!(doc.content, Content::Text) || self.bounds.w <= 1.0 || self.bounds.h <= 1.0 {
            return;
        }
        let mode = self.mode();
        for pane in [Side::Left, Side::Right] {
            if mode != Mode::SideBySide && pane == Side::Left {
                continue;
            }
            let top = if pane == Side::Left { scroll.left } else { scroll.right };
            self.draw_pane(ui, pane, top, scroll.x, c, focused);
        }
        if mode == Mode::SideBySide {
            self.draw_connectors(ui, scroll, c);
        }
        self.draw_ruler(ui, scroll, c);
        self.draw_buttons(ui, c);
        if self.max_x() > 0.0 {
            let (thumb, _) = self.hscroll_thumb();
            let o = self.bounds.origin();
            let hot = matches!(self.drag, Drag::HScroll { .. })
                || (ui.mouse().y >= o.y + self.bounds.h - HSCROLL * 2.0 && self.bounds.contains(ui.mouse()));
            ui.round(thumb.offset(o.x, o.y), 3.0, c.ruler_thumb.alpha(if hot { 1.6 } else { 0.8 }));
        }
    }

    fn visible_rows(&self, top: f32, count: usize) -> Option<Range<usize>> {
        if count == 0 {
            return None;
        }
        let lh = self.lh();
        let first = ((top / lh).floor().max(0.0) as usize).min(count - 1);
        let last = (((top + self.bounds.h) / lh).floor().max(first as f32) as usize).min(count - 1);
        Some(first..last + 1)
    }

    fn draw_pane(&self, ui: &mut Ui, pane: Side, top: f32, sx: f32, c: &CanvasColors, focused: bool) {
        let Some(doc) = &self.doc else { return };
        let o = self.bounds.origin();
        let band = self.band_rect(pane).offset(o.x, o.y);
        let text = self.text_rect(pane).offset(o.x, o.y);
        let lh = self.lh();
        let count = self.pane_row_count(pane);
        let Some(rows) = self.visible_rows(top, count) else { return };
        let mode = self.mode();
        let y_of = |row: usize| band.y + row as f32 * lh - top;
        let ticks = self.commit_ticks.as_ref();
        ui.push_clip(band);

        // Change bands, the marks of changes with no lines on this side, lines left out of the commit, the focus bar.
        let mut words: Vec<(Rect, Color)> = Vec::new();
        match mode {
            Mode::SideBySide => {
                let mut index = Self::first_hunk(doc, pane, rows.start);
                while index < doc.hunks().len() {
                    let hunk = &doc.hunks()[index];
                    let (start, n) = if pane == Side::Left { (hunk.left_start, hunk.left_count) } else { (hunk.right_start, hunk.right_count) };
                    if start > rows.end {
                        break;
                    }
                    let kind = hunk.kind();
                    if n == 0 {
                        ui.fill(Rect::new(band.x, y_of(start) - 1.0, band.w, 2.0), c.mark(kind));
                    } else {
                        let a = start.max(rows.start);
                        let b = (start + n).min(rows.end);
                        if a < b {
                            let r = Rect::new(band.x, y_of(a), band.w, (b - a) as f32 * lh);
                            ui.fill(r, c.line(kind));
                            if let Some(t) = ticks {
                                self.fill_excluded(ui, t, pane, a..b, y_of(a), band, c);
                            }
                            if self.focused_hunk == Some(index) {
                                let x = if pane == Side::Left { band.right() - 2.5 } else { band.x };
                                words.push((Rect::new(x, r.y, 2.5, r.h), c.mark(kind)));
                            }
                            let inline = if pane == Side::Left { &hunk.left_inline } else { &hunk.right_inline };
                            for line in a..b.min(start + inline.len()) {
                                if ticks.is_some_and(|t| t.is_excluded(pane, line)) {
                                    continue;
                                }
                                for range in &inline[line - start] {
                                    words.push((self.range_rect(ui, pane, line, range.start as usize..range.end as usize, y_of(line), text.x - sx), c.word(kind)));
                                }
                            }
                        }
                    }
                    index += 1;
                }
            }
            Mode::Unified => {
                let data = doc.unified_rows();
                let end = rows.end.min(data.len());
                let mut row = rows.start;
                while row < end {
                    let first = data[row];
                    if first.kind == RowKind::Context {
                        row += 1;
                        continue;
                    }
                    let mut next = row + 1;
                    while next < end && data[next].kind == first.kind && data[next].hunk == first.hunk {
                        next += 1;
                    }
                    let added = first.kind == RowKind::Added;
                    let r = Rect::new(band.x, y_of(row), band.w, (next - row) as f32 * lh);
                    ui.fill(r, if added { c.added } else { c.removed_line });
                    if let Some(t) = ticks {
                        let side = if added { Side::Right } else { Side::Left };
                        let first_line = if added { first.right } else { first.left }.unwrap_or(0) as usize;
                        self.fill_excluded(ui, t, side, first_line..first_line + (next - row), y_of(row), band, c);
                    }
                    if first.hunk.is_some_and(|h| Some(h as usize) == self.focused_hunk) {
                        words.push((Rect::new(band.x, r.y, 2.5, r.h), if added { c.added_mark } else { Color::hex(REMOVED_MARK) }));
                    }
                    if let Some(hunk) = first.hunk.and_then(|h| doc.hunks().get(h as usize)) {
                        for (i, item) in data.iter().enumerate().take(next).skip(row) {
                            let (side, line, start, inline) = if added {
                                let line = item.right.unwrap_or(0) as usize;
                                (Side::Right, line, hunk.right_start, &hunk.right_inline)
                            } else {
                                let line = item.left.unwrap_or(0) as usize;
                                (Side::Left, line, hunk.left_start, &hunk.left_inline)
                            };
                            let Some(offset) = line.checked_sub(start) else { continue };
                            if offset >= inline.len() || ticks.is_some_and(|t| t.is_excluded(side, line)) {
                                continue;
                            }
                            for range in &inline[offset] {
                                words.push((
                                    self.range_rect(ui, side, line, range.start as usize..range.end as usize, y_of(i), text.x - sx),
                                    if added { c.added_word } else { c.removed_word },
                                ));
                            }
                        }
                    }
                    row = next;
                }
            }
            Mode::Single => {}
        }

        // The focus line's tint.
        let focus_row = self.focus_line.and_then(|(side, line)| match mode {
            Mode::SideBySide => (side == pane).then_some(line),
            Mode::Unified => Some(doc.unified_row_of_line(line, side)),
            Mode::Single => (side == Side::Right).then_some(line),
        });
        if let Some(fr) = focus_row {
            ui.fill(Rect::new(text.x, y_of(fr), text.w, lh), c.focus_line);
        }

        // Gutter: annotations, line numbers, unified signs.
        self.draw_gutter(ui, pane, &rows, top, band, c);

        // Focus bars over the gutter edge.
        for (r, color) in words.iter().filter(|(r, _)| r.w == 2.5 && r.h >= lh - 0.5 && (r.x == band.x || r.x == band.right() - 2.5)) {
            ui.fill(*r, *color);
        }
        ui.pop_clip();

        // The text area: word highlights, search hits, the selection, the glyphs, the caret.
        ui.push_clip(text);
        for (r, color) in &words {
            if !(r.w == 2.5 && (r.x == band.x || r.x == band.right() - 2.5)) {
                ui.round(*r, 2.5, *color);
            }
        }
        if !self.matches_by_line[0].is_empty() || !self.matches_by_line[1].is_empty() {
            let active = self.current_match.and_then(|i| self.matches.get(i));
            for row in rows.clone() {
                let Some((side, line)) = self.source(pane, row) else { continue };
                let Some(ranges) = self.matches_by_line[side as usize].get(&line) else { continue };
                for range in ranges {
                    let r = self.range_rect(ui, side, line, range.clone(), y_of(row), text.x - sx);
                    ui.round(r, 2.5, c.search_match);
                }
                if let Some(m) = active.filter(|m| m.side == side && m.line == line) {
                    let r = self.range_rect(ui, side, line, m.range.clone(), y_of(row), text.x - sx);
                    ui.round(r, 2.5, c.search_current);
                }
            }
        }
        if let Some(sel) = self.selection.filter(|s| !s.is_empty() && (mode != Mode::SideBySide || s.pane == pane)) {
            let (lower, upper) = (sel.lower(), sel.upper());
            let first = lower.row.max(rows.start);
            let last = upper.row.min(rows.end.saturating_sub(1));
            for row in first..=last {
                if first > last {
                    break;
                }
                let Some((side, line)) = self.source(pane, row) else { continue };
                let layout = self.line_layout(ui, side, line);
                let x0 = if row == lower.row { layout.caret(lower.column).x } else { 0.0 };
                let x1 = if row == upper.row {
                    layout.caret(upper.column).x
                } else {
                    // One cell past the end stands for the line break.
                    layout.size().w + self.font().char_width
                };
                let x1 = x1.max(x0);
                ui.fill(Rect::new(text.x - sx + TEXT_INSET + x0, y_of(row), x1 - x0, lh), c.selection);
            }
        }
        let baseline = self.font().baseline;
        for row in rows.clone() {
            let Some((side, line)) = self.source(pane, row) else { continue };
            let layout = self.line_layout(ui, side, line);
            let origin = Point::new(text.x - sx + TEXT_INSET, y_of(row) + baseline - layout.baseline());
            if origin.x + layout.size().w >= text.x {
                ui.painter.text(layout.as_ref(), origin, c.text);
            }
        }
        // The caret, while the keyboard is here and the text can be edited; it blinks once it rests.
        if focused && pane == Side::Right {
            if let (Some(_), Some(sel)) = (&self.editor, self.selection) {
                if (mode != Mode::SideBySide || sel.pane == Side::Right) && rows.contains(&sel.head.row) {
                    if let Some((Side::Right, line)) = self.source(Side::Right, sel.head.row) {
                        let x = self.x_of(ui, Side::Right, line, sel.head.column);
                        let elapsed = self.blink_from.elapsed().as_millis() as u64;
                        let on = elapsed < 550 || ((elapsed - 550) / 530) % 2 == 1;
                        if on {
                            ui.fill(Rect::new((text.x - sx + TEXT_INSET + x - 1.0).round(), y_of(sel.head.row), 2.0, lh), c.text);
                        }
                        let wait = if elapsed < 550 { 550 - elapsed } else { 530 - (elapsed - 550) % 530 };
                        ui.repaint_after(Duration::from_millis(wait.max(1)));
                        // Where the input method puts its windows.
                        ui.st.ime_area = Some(Rect::new(text.x - sx + TEXT_INSET + x, y_of(sel.head.row), 2.0, lh));
                    }
                }
            }
            // What an input method is still composing is underlined in place.
            if let Some((row_line, range)) = self.editor.as_ref().and_then(|e| e.marked().cloned()) {
                let row = self.edit_row(row_line);
                if rows.contains(&row) {
                    let r = self.range_rect(ui, Side::Right, row_line, range, y_of(row), text.x - sx);
                    ui.fill(Rect::new(r.x, r.bottom() - 1.5, r.w, 1.5), c.text);
                }
            }
        }
        ui.pop_clip();
    }

    /// A rectangle over `range` of a line shown at `y`, with the text starting at `x0` (inset not included).
    fn range_rect(&self, ui: &mut Ui, side: Side, line: usize, range: Range<usize>, y: f32, x0: f32) -> Rect {
        let layout = self.line_layout(ui, side, line);
        let a = layout.caret(range.start).x;
        let b = layout.caret(range.end).x;
        Rect::new(x0 + TEXT_INSET + a, y + 1.0, (b - a).max(1.0), self.lh() - 2.0)
    }

    /// Pales the lines of `lines` (shown from `y`) that stay out of the commit.
    #[allow(clippy::too_many_arguments)]
    fn fill_excluded(&self, ui: &mut Ui, t: &CommitTicks, side: Side, lines: Range<usize>, y: f32, band: Rect, c: &CanvasColors) {
        let lh = self.lh();
        let color = c.background.with_alpha(0.62);
        let start_line = lines.start;
        let mut add = |run: Range<usize>| {
            ui.fill(Rect::new(band.x, y + (run.start - start_line) as f32 * lh, band.w, run.len() as f32 * lh), color);
        };
        if !t.included {
            add(lines);
            return;
        }
        let out = if side == Side::Left { &t.removed } else { &t.added };
        if out.is_empty() {
            return;
        }
        let mut start = None;
        for line in lines.clone() {
            if out.contains(&line) {
                start.get_or_insert(line);
            } else if let Some(s) = start.take() {
                add(s..line);
            }
        }
        if let Some(s) = start {
            add(s..lines.end);
        }
    }

    fn draw_gutter(&self, ui: &mut Ui, pane: Side, rows: &Range<usize>, top: f32, band: Rect, c: &CanvasColors) {
        let Some(doc) = &self.doc else { return };
        let lh = self.lh();
        let font = self.font();
        let mode = self.mode();
        let gutter_x = match mode {
            Mode::SideBySide if pane == Side::Left => band.x + self.pane_width() - self.gutter_width(),
            _ => band.x,
        };
        let shift = if pane == Side::Right { self.annotation_width() } else { 0.0 };
        let number = |ui: &mut Ui, n: usize, right_edge: f32, y: f32| {
            let layout = ui.text_layout(&n.to_string(), TextStyle::new(font.font));
            let origin = Point::new(right_edge - layout.size().w, y + font.baseline - layout.baseline());
            ui.painter.text(layout.as_ref(), origin, c.gutter_text);
        };
        // Annotate: a bar by every line with a commit, darker the newer, and date and author on each run's first line.
        if shift > 0.0 {
            if let Some(a) = &self.annotations {
                for row in rows.clone() {
                    let Some(line) = self.right_line(row) else { continue };
                    let Some(index) = a.index_at_line(line) else { continue };
                    let entry = &a.entries[index as usize];
                    let y = band.y + row as f32 * lh - top;
                    let first = a.is_run_start(line);
                    let off = if first { 1.0 } else { 0.0 };
                    ui.fill(Rect::new(gutter_x + 4.0, y + off, 3.0, lh - off), c.accent.with_alpha(0.18 + 0.72 * entry.recency as f32));
                    if first {
                        let color = if entry.recency >= 0.75 { c.gutter_text_strong } else { c.gutter_text };
                        let layout = ui.text_layout(&entry.label, TextStyle::new(font.font));
                        ui.painter.text(layout.as_ref(), Point::new(gutter_x + 12.0, y + font.baseline - layout.baseline()), color);
                    }
                }
            }
        }
        if mode == Mode::Unified {
            let nw = self.unified_number_width();
            let data = doc.unified_rows();
            for row in rows.clone() {
                let r = data[row];
                let y = band.y + row as f32 * lh - top;
                if let Some(left) = r.left {
                    number(ui, left as usize + 1, gutter_x + shift + nw - 4.0, y);
                }
                if let Some(right) = r.right {
                    number(ui, right as usize + 1, gutter_x + shift + nw * 2.0 - 4.0, y);
                }
                if r.kind != RowKind::Context {
                    let added = r.kind == RowKind::Added;
                    let layout = ui.text_layout(if added { "+" } else { "−" }, TextStyle::new(font.font));
                    let color = if added { c.added_mark } else { Color::hex(REMOVED_MARK) };
                    ui.painter.text(layout.as_ref(), Point::new(gutter_x + shift + nw * 2.0 + 4.0, y + font.baseline - layout.baseline()), color);
                }
            }
        } else {
            // Line numbers hug the divider on both sides.
            let right_edge = gutter_x + shift + self.gutter_width() - 10.0;
            for row in rows.clone() {
                number(ui, row + 1, right_edge, band.y + row as f32 * lh - top);
            }
        }
    }

    /// The curved ribbons joining each change across the divider.
    fn draw_connectors(&self, ui: &mut Ui, scroll: &Scroll, c: &CanvasColors) {
        let Some(doc) = &self.doc else { return };
        let lh = self.lh();
        let h = self.bounds.h;
        let x0 = self.bounds.x + self.pane_width();
        let y0 = self.bounds.y;
        let x1 = CONNECTOR;
        let bend = CONNECTOR * 0.5;
        let first_left = (scroll.left / lh).floor().max(0.0) as usize;
        let first_right = (scroll.right / lh).floor().max(0.0) as usize;
        let mut index = Self::first_hunk(doc, Side::Left, first_left).min(Self::first_hunk(doc, Side::Right, first_right));
        let mut fills: [VectorPath; 3] = [VectorPath::new(), VectorPath::new(), VectorPath::new()];
        let mut outline: Option<(VectorPath, Color)> = None;
        let kinds = [HunkKind::Modified, HunkKind::Added, HunkKind::Deleted];
        let p = |x: f32, y: f32| Point::new(x0 + x, y0 + y);
        while index < doc.hunks().len() {
            let hunk = &doc.hunks()[index];
            let mut l0 = hunk.left_start as f32 * lh - scroll.left;
            let mut l1 = l0 + hunk.left_count as f32 * lh;
            let mut r0 = hunk.right_start as f32 * lh - scroll.right;
            let mut r1 = r0 + hunk.right_count as f32 * lh;
            if l0.min(r0) > h {
                break;
            }
            if l1.max(r1) >= 0.0 {
                if hunk.left_count == 0 {
                    l0 -= 1.0;
                    l1 += 1.0;
                }
                if hunk.right_count == 0 {
                    r0 -= 1.0;
                    r1 += 1.0;
                }
                // Ends far outside the viewport are pulled in; the curve looks the same where it shows.
                l0 = l0.max(-h);
                r0 = r0.max(-h);
                l1 = l1.min(2.0 * h);
                r1 = r1.min(2.0 * h);
                let k = kinds.iter().position(|k| *k == hunk.kind()).unwrap_or(0);
                let path = &mut fills[k];
                let start = p(0.0, l0);
                path.move_to(start.x, start.y);
                path.cubic_to(p(bend, l0), p(x1 - bend, r0), p(x1, r0));
                let e = p(x1, r1);
                path.line_to(e.x, e.y);
                path.cubic_to(p(x1 - bend, r1), p(bend, l1), p(0.0, l1));
                path.close();
                if self.focused_hunk == Some(index) {
                    // Only the two curved edges: a closed outline reads as a box on one-line changes.
                    let mut edges = VectorPath::new();
                    edges.move_to(start.x, start.y);
                    edges.cubic_to(p(bend, l0), p(x1 - bend, r0), p(x1, r0));
                    edges.move_to(e.x, e.y);
                    edges.cubic_to(p(x1 - bend, r1), p(bend, l1), p(0.0, l1));
                    outline = Some((edges, c.mark(hunk.kind())));
                }
            }
            index += 1;
        }
        let at = crate::paint::Placement { offset: Point::default(), scale: 1.0 };
        ui.push_clip(Rect::new(x0, y0, CONNECTOR, h));
        for (k, path) in fills.iter().enumerate() {
            if !path.cmds.is_empty() {
                ui.painter.fill_path(path, at, c.line(kinds[k]));
            }
        }
        if let Some((edges, color)) = outline {
            ui.painter.stroke_path(&edges, at, 1.0, color);
        }
        ui.pop_clip();
    }

    /// Every change in the file at a glance, and where the viewport is.
    fn draw_ruler(&self, ui: &mut Ui, scroll: &Scroll, c: &CanvasColors) {
        let total = self.row_count();
        if total == 0 {
            return;
        }
        let lh = self.lh();
        let h = self.bounds.h;
        let scale = self.ruler_scale();
        let x = self.bounds.right() - RULER;
        let y0 = self.bounds.y;
        if total as f32 * lh + self.bottom_padding() > h {
            let thumb = Rect::new(x + 1.0, y0 + scroll.y / lh * scale, RULER - 2.0, (h / lh * scale).max(12.0));
            ui.round(thumb, 3.0, c.ruler_thumb);
        }
        let Some(doc) = &self.doc else { return };
        let unified = self.mode() == Mode::Unified;
        for (index, hunk) in doc.hunks().iter().enumerate() {
            let Some(row) = self.hunk_rows.get(index) else { break };
            let span = if unified { hunk.left_count + hunk.right_count } else { hunk.left_count.max(hunk.right_count) };
            ui.fill(Rect::new(x + 3.0, y0 + *row as f32 * scale, RULER - 6.0, (span as f32 * scale).max(2.0)), c.mark(hunk.kind()));
        }
    }

    fn draw_buttons(&mut self, ui: &mut Ui, c: &CanvasColors) {
        let o = self.bounds.origin();
        let side_by_side = self.mode() == Mode::SideBySide;
        let states = if self.commit_ticks.is_some() { self.tick_states() } else { Vec::new() };
        for (rect, index) in self.tick_hits.clone() {
            let r = rect.offset(o.x, o.y);
            let hot = self.hovered_tick == Some(index);
            let state = states.get(index).copied().unwrap_or(Inclusion::Off);
            draw_tick(ui, r, state, hot, c);
        }
        for (rect, index) in self.revert_hits.clone() {
            let r = rect.offset(o.x, o.y);
            let hot = self.hovered_revert == Some(index);
            if hot {
                ui.round(r, 4.0, c.accent);
            } else if !side_by_side {
                // Over code the button needs a ground of its own; in the ribbon it does not.
                ui.round(r, 4.0, c.background);
                ui.painter.stroke_round_rect(r.inset(0.5), 4.0, 1.0, c.separator);
            }
            let s = (r.w * 0.72).round();
            ui.icon(Icon::ArrowUndo, r.centered(Size::new(s, s)), if hot { Color::WHITE } else { c.gutter_text_strong });
        }
        let font = Font::ui((self.font().font.size - 1.0).clamp(10.0, 13.0)).weight(Weight::Semibold);
        for (rect, block, choice) in self.conflict_hits.clone() {
            let r = rect.offset(o.x, o.y);
            let hot = self.hovered_conflict == Some((block, choice));
            let prominent = choice != ConflictChoice::Both;
            let radius = r.h / 2.0;
            ui.round(r, radius, if hot { c.accent } else { c.background });
            if !hot {
                ui.painter.stroke_round_rect(r.inset(0.5), radius, 1.0, if prominent { c.accent.with_alpha(0.7) } else { c.separator });
            }
            let title = Self::conflict_choices().into_iter().find(|(ch, _)| *ch == choice).map(|(_, t)| t).unwrap_or_default();
            let ink = if hot {
                Color::WHITE
            } else if prominent {
                c.accent
            } else {
                c.gutter_text_strong
            };
            ui.label_with(r, &title, font, ink, Align::Center, Overflow::Clip);
        }
    }

    // MARK: Text out

    pub fn selected_text(&self) -> Option<String> {
        let sel = self.selection.filter(|s| !s.is_empty())?;
        let (lower, upper) = (sel.lower(), sel.upper());
        let mut parts = Vec::new();
        for row in lower.row..=upper.row {
            let text = self.row_text(sel.pane, row);
            let start = if row == lower.row { text_editing::floor_boundary(text, lower.column.min(text.len())) } else { 0 };
            let end = if row == upper.row { text_editing::floor_boundary(text, upper.column.min(text.len())) } else { text.len() };
            parts.push(text[start..end.max(start)].to_string());
        }
        Some(parts.join("\n"))
    }

    /// The 1-based line of the selection's start (or the top visible line) on the new side.
    pub fn current_line_number(&self) -> usize {
        if let Some(sel) = self.selection {
            if let Some((_, line)) = self.source(sel.pane, sel.lower().row) {
                return line + 1;
            }
        }
        let scroll = self.resolve();
        let row = ((scroll.right + self.bounds.h * 0.3) / self.lh()).max(0.0) as usize;
        let count = self.pane_row_count(Side::Right);
        self.source(Side::Right, row.min(count.saturating_sub(1))).map_or(1, |(_, l)| l + 1)
    }

    /// The lines the selection covers on one side (0-based), or the caret's line.
    pub fn selected_lines(&self) -> Option<(Side, Range<usize>)> {
        let doc = self.doc.as_ref()?;
        if !matches!(doc.content, Content::Text) {
            return None;
        }
        let sel = self.selection?;
        let mut upper = sel.upper().row;
        // A selection that ends at the very start of a line does not take that line.
        if upper > sel.lower().row && sel.upper().column == 0 {
            upper -= 1;
        }
        match self.mode() {
            Mode::SideBySide | Mode::Single => {
                let side = if self.mode() == Mode::Single { Side::Right } else { sel.pane };
                let count = doc.buffer(side).len();
                if count == 0 {
                    return None;
                }
                Some((side, sel.lower().row.min(count - 1)..upper.min(count - 1) + 1))
            }
            Mode::Unified => {
                let rows = doc.unified_rows();
                if rows.is_empty() {
                    return None;
                }
                let mut right: Option<(usize, usize)> = None;
                let mut left: Option<(usize, usize)> = None;
                for r in &rows[sel.lower().row.min(rows.len() - 1)..=upper.min(rows.len() - 1)] {
                    if let Some(line) = r.right {
                        let line = line as usize;
                        right = Some(right.map_or((line, line), |(a, b)| (a.min(line), b.max(line))));
                    } else if let Some(line) = r.left {
                        let line = line as usize;
                        left = Some(left.map_or((line, line), |(a, b)| (a.min(line), b.max(line))));
                    }
                }
                if let Some((a, b)) = right {
                    return Some((Side::Right, a..b + 1));
                }
                left.map(|(a, b)| (Side::Left, a..b + 1))
            }
        }
    }

    /// Every line the selection covers, each with its side, or the caret's line.
    pub fn selected_source_lines(&self) -> Vec<(Side, usize)> {
        let Some(sel) = self.selection else { return Vec::new() };
        let mut upper = sel.upper().row;
        if upper > sel.lower().row && sel.upper().column == 0 {
            upper -= 1;
        }
        (sel.lower().row..=upper).filter_map(|r| self.source(sel.pane, r)).collect()
    }

    /// The 1-based line numbers the selection covers, when it touches real lines.
    pub fn selected_line_range(&self) -> Option<(usize, usize)> {
        let sel = self.selection.filter(|s| !s.is_empty())?;
        let mut first = None;
        let mut last = None;
        for row in sel.lower().row..=sel.upper().row {
            if let Some((_, line)) = self.source(sel.pane, row) {
                first.get_or_insert(line + 1);
                last = Some(line + 1);
            }
        }
        Some((first?, last?))
    }

    /// The change the mouse is over, for a context menu built from code.
    pub fn hunk_at_window_point(&self, p: Point) -> Option<usize> {
        self.hunk_at(Point::new(p.x - self.bounds.x, p.y - self.bounds.y))
    }

    pub fn bounds(&self) -> Rect {
        self.bounds
    }
}

/// The word around a column, or the one character there.
fn word_range(text: &str, column: usize) -> Range<usize> {
    if text.is_empty() {
        return 0..0;
    }
    let is_word = |c: char| c.is_alphanumeric() || c == '_' || !c.is_ascii();
    let pivot = text_editing::floor_boundary(text, column.min(text.len().saturating_sub(1)));
    let ch = text[pivot..].chars().next().unwrap_or(' ');
    if !is_word(ch) {
        return pivot..pivot + ch.len_utf8();
    }
    let mut start = pivot;
    for (i, c) in text[..pivot].char_indices().rev() {
        if !is_word(c) {
            break;
        }
        start = i;
    }
    let mut end = pivot + ch.len_utf8();
    for (i, c) in text[end..].char_indices() {
        if !is_word(c) {
            break;
        }
        end = pivot + ch.len_utf8() + i + c.len_utf8();
    }
    start..end
}

/// The commit tick, drawn like the ones in the Changes list.
fn draw_tick(ui: &mut Ui, r: Rect, state: Inclusion, hot: bool, c: &CanvasColors) {
    let b = r.inset(0.75);
    if state == Inclusion::Off {
        ui.round(b, 3.5, c.background);
        ui.painter.stroke_round_rect(b, 3.5, 1.3, if hot { c.accent } else { c.gutter_text_strong });
        return;
    }
    ui.round(b, 3.5, if hot { c.accent.with_alpha(0.82) } else { c.accent });
    let s = r.w;
    let w = (s * 0.13).max(1.6);
    let p = |x: f32, y: f32| Point::new(r.x + s * x, r.y + s * y);
    if state == Inclusion::On {
        ui.painter.line(p(0.28, 0.49), p(0.44, 0.65), w, Color::WHITE);
        ui.painter.line(p(0.44, 0.65), p(0.73, 0.34), w, Color::WHITE);
    } else {
        ui.painter.line(p(0.3, 0.5), p(0.7, 0.5), w, Color::WHITE);
    }
}

#[cfg(test)]
mod tests {
    use super::word_range;

    #[test]
    fn words() {
        assert_eq!(word_range("let foo_bar = 1;", 6), 4..11);
        assert_eq!(word_range("let foo_bar = 1;", 12), 12..13);
        assert_eq!(word_range("变量名 = 1", 3), 0..9);
        assert_eq!(word_range("", 0), 0..0);
    }
}
