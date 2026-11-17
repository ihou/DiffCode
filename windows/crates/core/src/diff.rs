//! The diff model: both sides' text, the hunks between them, the scroll layout that keeps the two panes level,
//! word-level highlights, and loading a document for a file.

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::LazyLock;
use std::time::Instant;

use crate::git::{self, ChangeKind, EMPTY_TREE};
use crate::lang::l;
use crate::remote::ConflictBase;
use crate::syntax::{self, LanguageSpec, State, SyntaxSpan};
use crate::text_codec::{self, TextFormat};
use crate::text_editing::{self, Replacement, TextPosition};
use crate::util::byte_size_string;

// MARK: - Text buffer

/// One side of a comparison (or the whole of a plain file).
#[derive(Clone, Debug, Default)]
pub struct TextBuffer {
    lines: Vec<String>,
    pub spans: Vec<Vec<SyntaxSpan>>,
    /// The lexer state at the start of each line and after the last, for buffers that can be edited:
    /// an edit then re-colours only the lines it affects.
    pub states: Option<Vec<State>>,
    /// Widest line, in approximate monospace columns. Drives the horizontal scroll extent.
    /// Grows as lines are edited wider; never shrinks until the file is read again.
    max_columns: usize,
}

impl TextBuffer {
    pub fn new(lines: Vec<String>) -> Self {
        let mut widest = 0;
        for line in &lines {
            // cannot beat the current maximum even if all tabs
            if line.len() * 4 > widest {
                widest = widest.max(Self::columns(line));
            }
        }
        TextBuffer { lines, spans: Vec::new(), states: None, max_columns: widest }
    }

    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    pub fn max_columns(&self) -> usize {
        self.max_columns
    }

    /// Approximate monospace columns: tabs to the next stop of four, wide (CJK) characters as two.
    pub fn columns(line: &str) -> usize {
        let mut cols = 0;
        for b in line.bytes() {
            if b == b'\t' {
                cols += 4 - cols % 4;
            } else if b & 0xC0 != 0x80 {
                cols += if b >= 0xE0 { 2 } else { 1 };
            }
        }
        cols
    }

    pub fn spans_at(&self, line: usize) -> &[SyntaxSpan] {
        self.spans.get(line).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Replaces text; see `text_editing::replace`. Colours are the caller's to bring up to date.
    pub fn replace(&mut self, from: TextPosition, to: TextPosition, text: &str) -> Replacement {
        let count = self.lines.len();
        let result = text_editing::replace(&mut self.lines, from, to, text);
        let raw = result.raw;
        for row in raw.start..raw.start + raw.inserted {
            self.max_columns = self.max_columns.max(Self::columns(&self.lines[row]));
        }
        if count > 0 && self.spans.len() == count {
            self.spans.splice(raw.start..raw.start + raw.removed, std::iter::repeat_n(Vec::new(), raw.inserted));
        } else {
            self.spans.clear();
        }
        match &mut self.states {
            Some(states) if count > 0 && states.len() == count + 1 => {
                // The states going into the first rewritten line, and into the line after them, are kept.
                states.splice(
                    raw.start + 1..raw.start + raw.removed,
                    std::iter::repeat_n(State::default(), raw.inserted - 1),
                );
            }
            _ => self.states = None,
        }
        result
    }

    /// Re-colours after an edit, as far as the lexer state says it must. Returns the lines recoloured.
    pub fn rehighlight(&mut self, language: &LanguageSpec, first: usize, count: usize) -> Range<usize> {
        if self.spans.len() != self.lines.len() {
            self.spans.resize(self.lines.len(), Vec::new());
        }
        match &mut self.states {
            Some(states) => syntax::rehighlight(&self.lines, language, first, count, &mut self.spans, states),
            None => first..first,
        }
    }
}

// MARK: - Hunks and layout

/// What it takes to edit the right side of a document and write it back.
#[derive(Clone, Debug, Default)]
pub struct EditInfo {
    pub format: TextFormat,
    pub crlf: Option<Vec<bool>>,
    /// Fingerprint of the bytes the right side was read from.
    pub fingerprint: Option<[u8; 32]>,
    /// The left side's bytes, to diff against as the text changes; None when there is no left side.
    pub base: Option<Vec<u8>>,
    /// Why the right side stays read-only, when it does.
    pub read_only_reason: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HunkKind {
    Added,
    Deleted,
    Modified,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Hunk {
    pub left_start: usize,
    pub left_count: usize,
    pub right_start: usize,
    pub right_count: usize,
    /// Changed byte ranges per line of the hunk, for word-level highlighting.
    pub left_inline: Vec<Vec<Range<u32>>>,
    pub right_inline: Vec<Vec<Range<u32>>>,
}

impl Hunk {
    pub fn lines(left_start: usize, left_count: usize, right_start: usize, right_count: usize) -> Self {
        Hunk { left_start, left_count, right_start, right_count, left_inline: Vec::new(), right_inline: Vec::new() }
    }

    pub fn kind(&self) -> HunkKind {
        if self.left_count == 0 {
            HunkKind::Added
        } else if self.right_count == 0 {
            HunkKind::Deleted
        } else {
            HunkKind::Modified
        }
    }

    pub fn left_end(&self) -> usize {
        self.left_start + self.left_count
    }

    pub fn right_end(&self) -> usize {
        self.right_start + self.right_count
    }
}

/// A run of lines that scrolls as a unit. Unchanged runs have equal counts on both sides;
/// a hunk occupies `max(left_count, right_count)` virtual lines so the shorter side scrolls
/// proportionally slower and the two panes stay aligned without padding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Segment {
    pub virtual_start: usize,
    pub left_start: usize,
    pub left_count: usize,
    pub right_start: usize,
    pub right_count: usize,
    /// None for unchanged runs.
    pub hunk: Option<usize>,
}

impl Segment {
    pub fn span(&self) -> usize {
        self.left_count.max(self.right_count)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowKind {
    Context,
    Removed,
    Added,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnifiedRow {
    pub kind: RowKind,
    pub left: Option<u32>,
    pub right: Option<u32>,
    pub hunk: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Side {
    Left,
    Right,
}

#[derive(Clone, Debug)]
pub enum Content {
    Text,
    Image { old: Option<Vec<u8>>, new: Option<Vec<u8>> },
    Message { title: String, detail: String },
}

#[derive(Debug)]
pub struct DiffDocument {
    pub left: TextBuffer,
    pub right: TextBuffer,
    hunks: Vec<Hunk>,
    segments: Vec<Segment>,
    virtual_line_count: usize,
    pub language: &'static LanguageSpec,
    pub content: Content,
    /// True for a plain file view (no comparison).
    pub is_plain_file: bool,
    pub left_title: String,
    pub right_title: String,
    pub note: Option<String>,
    pub load_milliseconds: f64,
    /// Set when the right side is a file in the working tree: how to write it back, or why it cannot be.
    pub editing: Option<EditInfo>,
    unified_cache: std::cell::OnceCell<Vec<UnifiedRow>>,
}

impl DiffDocument {
    pub fn new(left: TextBuffer, right: TextBuffer, hunks: Vec<Hunk>, language: &'static LanguageSpec) -> Self {
        let (hunks, segments, virtual_line_count) = Self::layout(hunks, left.len(), right.len());
        DiffDocument {
            left,
            right,
            hunks,
            segments,
            virtual_line_count,
            language,
            content: Content::Text,
            is_plain_file: false,
            left_title: String::new(),
            right_title: String::new(),
            note: None,
            load_milliseconds: 0.0,
            editing: None,
            unified_cache: std::cell::OnceCell::new(),
        }
    }

    pub fn with_content(mut self, content: Content) -> Self {
        self.content = content;
        self
    }

    pub fn plain_file(mut self) -> Self {
        self.is_plain_file = true;
        self
    }

    pub fn message(title: impl Into<String>, detail: impl Into<String>) -> Self {
        Self::new(TextBuffer::default(), TextBuffer::default(), Vec::new(), &syntax::PLAIN)
            .with_content(Content::Message { title: title.into(), detail: detail.into() })
    }

    pub fn hunks(&self) -> &[Hunk] {
        &self.hunks
    }

    pub fn segments(&self) -> &[Segment] {
        &self.segments
    }

    pub fn virtual_line_count(&self) -> usize {
        self.virtual_line_count
    }

    pub fn additions(&self) -> usize {
        self.hunks.iter().map(|h| h.right_count).sum()
    }

    pub fn deletions(&self) -> usize {
        self.hunks.iter().map(|h| h.left_count).sum()
    }

    pub fn buffer(&self, side: Side) -> &TextBuffer {
        match side {
            Side::Left => &self.left,
            Side::Right => &self.right,
        }
    }

    /// New hunks for the same two sides, e.g. after the right side was edited.
    pub fn set_hunks(&mut self, hunks: Vec<Hunk>) {
        let (hunks, segments, count) = Self::layout(hunks, self.left.len(), self.right.len());
        self.hunks = hunks;
        self.segments = segments;
        self.virtual_line_count = count;
        self.unified_cache = std::cell::OnceCell::new();
    }

    fn layout(hunks: Vec<Hunk>, left_count: usize, right_count: usize) -> (Vec<Hunk>, Vec<Segment>, usize) {
        // Clamp defensively: the file may have changed between reading it and diffing it.
        let mut clean: Vec<Hunk> = Vec::with_capacity(hunks.len());
        let (mut lpos, mut rpos) = (0usize, 0usize);
        for mut h in hunks {
            h.left_start = h.left_start.max(lpos).min(left_count);
            h.right_start = h.right_start.max(rpos).min(right_count);
            h.left_count = h.left_count.min(left_count - h.left_start);
            h.right_count = h.right_count.min(right_count - h.right_start);
            if h.left_count == 0 && h.right_count == 0 {
                continue;
            }
            lpos = h.left_end();
            rpos = h.right_end();
            clean.push(h);
        }

        let mut segs = Vec::with_capacity(clean.len() * 2 + 1);
        let mut v = 0;
        lpos = 0;
        rpos = 0;
        for (index, h) in clean.iter().enumerate() {
            let gap_l = h.left_start - lpos;
            let gap_r = h.right_start - rpos;
            if gap_l > 0 || gap_r > 0 {
                segs.push(Segment {
                    virtual_start: v,
                    left_start: lpos,
                    left_count: gap_l,
                    right_start: rpos,
                    right_count: gap_r,
                    hunk: None,
                });
                v += gap_l.max(gap_r);
            }
            segs.push(Segment {
                virtual_start: v,
                left_start: h.left_start,
                left_count: h.left_count,
                right_start: h.right_start,
                right_count: h.right_count,
                hunk: Some(index),
            });
            v += h.left_count.max(h.right_count);
            lpos = h.left_end();
            rpos = h.right_end();
        }
        let tail_l = left_count - lpos;
        let tail_r = right_count - rpos;
        if tail_l > 0 || tail_r > 0 {
            segs.push(Segment {
                virtual_start: v,
                left_start: lpos,
                left_count: tail_l,
                right_start: rpos,
                right_count: tail_r,
                hunk: None,
            });
            v += tail_l.max(tail_r);
        }
        (clean, segs, v)
    }

    // MARK: Scroll mapping

    fn segment_index(&self, v: f64) -> Option<usize> {
        if self.segments.is_empty() {
            return None;
        }
        let (mut lo, mut hi) = (0, self.segments.len() - 1);
        while lo < hi {
            let mid = (lo + hi).div_ceil(2);
            if self.segments[mid].virtual_start as f64 <= v {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        Some(lo)
    }

    /// Fractional line positions on each side for a virtual line position.
    pub fn pane_positions(&self, v: f64) -> (f64, f64) {
        let Some(idx) = self.segment_index(v) else { return (0.0, 0.0) };
        let s = &self.segments[idx];
        let span = s.span() as f64;
        let t = if span > 0.0 { ((v - s.virtual_start as f64) / span).clamp(0.0, 1.0) } else { 0.0 };
        (s.left_start as f64 + t * s.left_count as f64, s.right_start as f64 + t * s.right_count as f64)
    }

    /// The virtual line position at which `line` of `side` sits.
    pub fn virtual_position(&self, line: usize, side: Side) -> f64 {
        if self.segments.is_empty() {
            return 0.0;
        }
        let start_of = |s: &Segment| if side == Side::Left { s.left_start } else { s.right_start };
        let (mut lo, mut hi) = (0, self.segments.len() - 1);
        while lo < hi {
            let mid = (lo + hi).div_ceil(2);
            if start_of(&self.segments[mid]) <= line {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        let s = &self.segments[lo];
        let start = start_of(s);
        let count = if side == Side::Left { s.left_count } else { s.right_count };
        if count == 0 {
            return s.virtual_start as f64;
        }
        let t = (line.saturating_sub(start) as f64 / count as f64).min(1.0);
        s.virtual_start as f64 + t * s.span() as f64
    }

    pub fn virtual_start_of_hunk(&self, index: usize) -> usize {
        self.segments.iter().find(|s| s.hunk == Some(index)).map(|s| s.virtual_start).unwrap_or(0)
    }

    // MARK: Unified rows

    pub fn unified_rows(&self) -> &[UnifiedRow] {
        self.unified_cache.get_or_init(|| self.build_unified_rows())
    }

    fn build_unified_rows(&self) -> Vec<UnifiedRow> {
        let mut rows = Vec::with_capacity(self.virtual_line_count + self.hunks.len() * 4);
        for s in &self.segments {
            match s.hunk {
                None => {
                    for k in 0..s.left_count.min(s.right_count) {
                        rows.push(UnifiedRow {
                            kind: RowKind::Context,
                            left: Some((s.left_start + k) as u32),
                            right: Some((s.right_start + k) as u32),
                            hunk: None,
                        });
                    }
                    // Only an empty file's one blank line has no partner on the left; it still needs a row to type on.
                    for k in s.left_count..s.left_count.max(s.right_count) {
                        rows.push(UnifiedRow {
                            kind: RowKind::Context,
                            left: None,
                            right: Some((s.right_start + k) as u32),
                            hunk: None,
                        });
                    }
                }
                Some(h) => {
                    for k in 0..s.left_count {
                        rows.push(UnifiedRow {
                            kind: RowKind::Removed,
                            left: Some((s.left_start + k) as u32),
                            right: None,
                            hunk: Some(h as u32),
                        });
                    }
                    for k in 0..s.right_count {
                        rows.push(UnifiedRow {
                            kind: RowKind::Added,
                            left: None,
                            right: Some((s.right_start + k) as u32),
                            hunk: Some(h as u32),
                        });
                    }
                }
            }
        }
        rows
    }

    /// First unified row belonging to a hunk.
    pub fn unified_row_of_hunk(&self, index: usize) -> usize {
        self.unified_rows().iter().position(|r| r.hunk == Some(index as u32)).unwrap_or(0)
    }

    /// The unified row showing `line` of `side`, or the closest row before it.
    pub fn unified_row_of_line(&self, line: usize, side: Side) -> usize {
        let rows = self.unified_rows();
        let target = line as u32;
        let value = |r: &UnifiedRow| if side == Side::Left { r.left } else { r.right };
        // Rows are sorted by line on each side; rows without a line on this side are skipped over.
        let (mut lo, mut hi) = (0isize, rows.len() as isize - 1);
        let mut best = 0usize;
        while lo <= hi {
            let mid = (lo + hi) / 2;
            let mut probe = mid;
            while probe <= hi && value(&rows[probe as usize]).is_none() {
                probe += 1;
            }
            if probe > hi {
                hi = mid - 1;
                continue;
            }
            let v = value(&rows[probe as usize]).unwrap_or(0);
            if v == target {
                return probe as usize;
            }
            if v < target {
                best = probe as usize;
                lo = probe + 1;
            } else {
                hi = mid - 1;
            }
        }
        best
    }
}

// MARK: - Hunk parsing

/// Extracts hunks from unified diff output produced with `-U0`. Also says whether git called the files binary.
pub fn parse_hunks(data: &[u8]) -> (Vec<Hunk>, bool) {
    let mut hunks = Vec::new();
    let mut binary = false;
    let n = data.len();
    let mut i = 0;
    while i < n {
        let end = data[i..].iter().position(|&b| b == b'\n').map(|p| i + p).unwrap_or(n);
        let line = &data[i..end];
        if line.len() > 4 && line.starts_with(b"@@ -") {
            let mut p = 4;
            let number = |p: &mut usize| -> usize {
                let mut value = 0usize;
                while *p < line.len() && line[*p].is_ascii_digit() {
                    value = value * 10 + (line[*p] - b'0') as usize;
                    *p += 1;
                }
                value
            };
            let a = number(&mut p);
            let mut b = 1;
            if p < line.len() && line[p] == b',' {
                p += 1;
                b = number(&mut p);
            }
            while p < line.len() && line[p] != b'+' {
                p += 1;
            }
            p += 1;
            let c = number(&mut p);
            let mut d = 1;
            if p < line.len() && line[p] == b',' {
                p += 1;
                d = number(&mut p);
            }
            // With zero lines on a side, git reports the line *after which* the change sits.
            hunks.push(Hunk::lines(
                if b == 0 { a } else { a.saturating_sub(1) },
                b,
                if d == 0 { c } else { c.saturating_sub(1) },
                d,
            ));
        } else if line.len() > 12 && line.starts_with(b"Bina") {
            binary = true;
        }
        i = end + 1;
    }
    (hunks, binary)
}

// MARK: - Word-level diff

#[derive(Clone, Copy)]
struct Token {
    line: u32,
    /// Byte offset in the line; None for the break between two lines.
    start: Option<u32>,
    length: u32,
    hash: u64,
    is_space: bool,
}

const MAX_INLINE_TOKENS: usize = 3000;
const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

fn tokenize<S: AsRef<str>>(lines: &[S]) -> Option<Vec<Token>> {
    let mut tokens = Vec::new();
    for (line_index, line) in lines.iter().enumerate() {
        let line = line.as_ref();
        if line_index > 0 {
            tokens.push(Token {
                line: (line_index - 1) as u32,
                start: None,
                length: 0,
                hash: 0x9E37_79B9_7F4A_7C15,
                is_space: true,
            });
        }
        let mut start = 0usize;
        let mut hash = FNV_OFFSET;
        let mut cls: i8 = -1; // 0 word, 1 space
        let flush = |tokens: &mut Vec<Token>, cls: &mut i8, hash: &mut u64, start: usize, end: usize| {
            if *cls >= 0 && end > start {
                tokens.push(Token {
                    line: line_index as u32,
                    start: Some(start as u32),
                    length: (end - start) as u32,
                    hash: *hash ^ (end - start) as u64,
                    is_space: *cls == 1,
                });
            }
            *cls = -1;
            *hash = FNV_OFFSET;
        };
        for (i, ch) in line.char_indices() {
            let u = ch as u32;
            let c: i8 = if ch == ' ' || ch == '\t' {
                1
            } else if ch.is_ascii_alphanumeric() || ch == '_' || (0x80..0x2E80).contains(&u) {
                0
            } else if u >= 0x10000 {
                3 // outside the BMP (emoji): starts a token that following word characters join
            } else {
                2 // punctuation and CJK: one token per character
            };
            match c {
                2 => {
                    flush(&mut tokens, &mut cls, &mut hash, start, i);
                    start = i;
                    hash = (FNV_OFFSET ^ u as u64).wrapping_mul(FNV_PRIME);
                    cls = 0;
                    flush(&mut tokens, &mut cls, &mut hash, start, i + ch.len_utf8());
                }
                3 => {
                    flush(&mut tokens, &mut cls, &mut hash, start, i);
                    start = i;
                    hash = (FNV_OFFSET ^ u as u64).wrapping_mul(FNV_PRIME);
                    cls = 0;
                }
                _ => {
                    if c != cls {
                        flush(&mut tokens, &mut cls, &mut hash, start, i);
                        start = i;
                        cls = c;
                    }
                    hash = (hash ^ u as u64).wrapping_mul(FNV_PRIME);
                }
            }
        }
        flush(&mut tokens, &mut cls, &mut hash, start, line.len());
        if tokens.len() > MAX_INLINE_TOKENS {
            return None;
        }
    }
    Some(tokens)
}

pub type InlineRanges = Vec<Vec<Range<u32>>>;

/// Per-line changed byte ranges for both sides of a modified hunk, or None when the two sides share
/// nothing worth aligning (whole-line colouring says enough).
pub fn inline_diff<S: AsRef<str>>(old: &[S], new: &[S]) -> Option<(InlineRanges, InlineRanges)> {
    let a = tokenize(old)?;
    let b = tokenize(new)?;
    if a.is_empty() || b.is_empty() {
        return None;
    }
    let ha: Vec<u64> = a.iter().map(|t| t.hash).collect();
    let hb: Vec<u64> = b.iter().map(|t| t.hash).collect();
    let (matched_a, matched_b) = myers_matches(&ha, &hb, 600)?;

    let common = a.iter().zip(&matched_a).filter(|(t, m)| **m && !t.is_space).count();
    if common == 0 {
        return None;
    }

    fn ranges(tokens: &[Token], matched: &[bool], line_count: usize) -> InlineRanges {
        let mut result: InlineRanges = vec![Vec::new(); line_count];
        // Extent of each line's visible text, to spot lines that changed from end to end.
        let mut extents: Vec<Option<Range<u32>>> = vec![None; line_count];
        let mut open: Option<(usize, u32, u32)> = None;
        let close = |open: &mut Option<(usize, u32, u32)>, result: &mut InlineRanges| {
            if let Some((line, s, e)) = open.take() {
                result[line].push(s..e);
            }
        };
        for (i, t) in tokens.iter().enumerate() {
            let Some(start) = t.start else {
                close(&mut open, &mut result); // line break
                continue;
            };
            // Whitespace never starts or ends a range (re-indented code would otherwise light up
            // as solid blocks), but a space between two changed words is bridged so they merge.
            if t.is_space {
                continue;
            }
            let line = t.line as usize;
            let end = start + t.length;
            let lower = extents[line].as_ref().map(|r| r.start).unwrap_or(start);
            extents[line] = Some(lower..end);
            if !matched[i] {
                match open {
                    Some((ol, os, _)) if ol == line => open = Some((line, os, end)),
                    _ => {
                        close(&mut open, &mut result);
                        open = Some((line, start, end));
                    }
                }
            } else {
                close(&mut open, &mut result);
            }
        }
        close(&mut open, &mut result);
        // A line that differs in its entirety gains nothing from a second, darker highlight
        // on top of its band; keeping those lines plain makes the partial edits stand out.
        for line in 0..line_count {
            if result[line].len() == 1 && Some(result[line][0].clone()) == extents[line] {
                result[line].clear();
            }
        }
        result
    }
    Some((ranges(&a, &matched_a, old.len()), ranges(&b, &matched_b, new.len())))
}

/// Myers' O(ND) diff: flags which elements of `a` and `b` belong to the longest common subsequence.
/// Returns None when more than `max_edits` edits would be needed.
pub fn myers_matches<T: PartialEq>(a: &[T], b: &[T], max_edits: usize) -> Option<(Vec<bool>, Vec<bool>)> {
    let mut matched_a = vec![false; a.len()];
    let mut matched_b = vec![false; b.len()];

    // Trim the common prefix and suffix; real edits are usually a small middle.
    let mut prefix = 0;
    while prefix < a.len() && prefix < b.len() && a[prefix] == b[prefix] {
        matched_a[prefix] = true;
        matched_b[prefix] = true;
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix < a.len() - prefix && suffix < b.len() - prefix && a[a.len() - 1 - suffix] == b[b.len() - 1 - suffix]
    {
        matched_a[a.len() - 1 - suffix] = true;
        matched_b[b.len() - 1 - suffix] = true;
        suffix += 1;
    }
    let n = (a.len() - prefix - suffix) as isize;
    let m = (b.len() - prefix - suffix) as isize;
    if n == 0 || m == 0 {
        return Some((matched_a, matched_b));
    }

    let limit = ((n + m) as usize).min(max_edits) as isize;
    let offset = limit + 1;
    let mut v = vec![0isize; (2 * limit + 3) as usize];
    let mut trace: Vec<Vec<isize>> = Vec::new();
    let mut solved_at: isize = -1;
    let at = |k: isize| (offset + k) as usize;

    'outer: for d in 0..=limit {
        trace.push(v.clone());
        let mut k = -d;
        while k <= d {
            let mut x = if k == -d || (k != d && v[at(k - 1)] < v[at(k + 1)]) { v[at(k + 1)] } else { v[at(k - 1)] + 1 };
            let mut y = x - k;
            while x < n && y < m && a[prefix + x as usize] == b[prefix + y as usize] {
                x += 1;
                y += 1;
            }
            v[at(k)] = x;
            if x >= n && y >= m {
                solved_at = d;
                break 'outer;
            }
            k += 2;
        }
    }
    if solved_at < 0 {
        return None;
    }

    let (mut x, mut y) = (n, m);
    let mut d = solved_at;
    while d >= 0 {
        let snapshot = &trace[d as usize];
        let k = x - y;
        let prev_k = if k == -d || (k != d && snapshot[at(k - 1)] < snapshot[at(k + 1)]) { k + 1 } else { k - 1 };
        let prev_x = if d == 0 { 0 } else { snapshot[at(prev_k)] };
        let prev_y = if d == 0 { 0 } else { prev_x - prev_k };
        while x > prev_x && y > prev_y {
            x -= 1;
            y -= 1;
            matched_a[prefix + x as usize] = true;
            matched_b[prefix + y as usize] = true;
        }
        x = prev_x;
        y = prev_y;
        d -= 1;
    }
    Some((matched_a, matched_b))
}

// MARK: - Loading

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// `base` (normally HEAD) against the working tree.
    Worktree { base: String },
    /// A commit against its first parent (None for a root commit).
    Commit { hash: String, parent: Option<String> },
    /// A plain file from the working tree, no comparison.
    File,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffRequest {
    pub root: PathBuf,
    pub path: String,
    pub old_path: Option<String>,
    pub kind: Option<ChangeKind>,
    pub source: Source,
    pub ignore_whitespace: bool,
}

impl DiffRequest {
    /// A local change as the Changes list has it: the working tree against HEAD, not against another commit.
    pub fn is_against_head(&self) -> bool {
        matches!(&self.source, Source::Worktree { base } if base == "HEAD" || base == EMPTY_TREE)
    }

    pub fn full_path(&self) -> PathBuf {
        join_repo_path(&self.root, &self.path)
    }
}

/// `root` joined with a repository-relative path that uses forward slashes.
pub fn join_repo_path(root: &Path, path: &str) -> PathBuf {
    let mut out = root.to_path_buf();
    for part in path.split('/').filter(|p| !p.is_empty()) {
        out.push(part);
    }
    out
}

pub const MAX_TEXT_BYTES: usize = 12 * 1024 * 1024;
pub const IMAGE_EXTENSIONS: &[&str] =
    &["png", "jpg", "jpeg", "gif", "webp", "bmp", "tiff", "tif", "heic", "ico", "icns", "pdf"];

static SCRATCH: LazyLock<PathBuf> = LazyLock::new(|| {
    let dir = std::env::temp_dir().join(format!("diffcode-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    dir
});

/// A directory of this process's own, removed when the app quits.
pub fn scratch_directory() -> &'static Path {
    &SCRATCH
}

pub fn cleanup() {
    let _ = std::fs::remove_dir_all(&*SCRATCH);
}

/// A name no other file in the scratch directory has.
pub fn unique_name() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    format!("{:x}-{:x}", t, n)
}

pub fn is_utf16(data: &[u8]) -> bool {
    data.starts_with(&[0xFF, 0xFE]) || data.starts_with(&[0xFE, 0xFF])
}

/// Loads the document for a request. Blocking: call it on a worker thread.
pub fn load(request: &DiffRequest) -> DiffDocument {
    let started = Instant::now();
    let mut doc = build(request);
    doc.load_milliseconds = started.elapsed().as_secs_f64() * 1000.0;
    doc
}

fn read_worktree(path: &Path) -> Option<Vec<u8>> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.is_dir() {
        return None;
    }
    std::fs::read(path).ok()
}

fn is_image(path: &str) -> bool {
    IMAGE_EXTENSIONS.contains(&crate::util::extension(path).as_str())
}

fn build(request: &DiffRequest) -> DiffDocument {
    let root = &request.root;
    let language = syntax::detect(&request.path);

    let old_data: Option<Vec<u8>>;
    let new_data: Option<Vec<u8>>;
    let left_title: String;
    let right_title: String;

    match &request.source {
        Source::File => {
            let full = request.full_path();
            let Some(data) = read_worktree(&full) else {
                return DiffDocument::message(l("File not found", "文件不存在"), request.path.clone());
            };
            if is_image(&request.path) {
                return DiffDocument::new(TextBuffer::default(), TextBuffer::default(), Vec::new(), &syntax::PLAIN)
                    .with_content(Content::Image { old: None, new: Some(data) })
                    .plain_file();
            }
            if text_codec::looks_binary(&data) {
                return DiffDocument::message(l("Binary file", "二进制文件"), byte_size_string(data.len() as u64));
            }
            if data.len() > MAX_TEXT_BYTES {
                return DiffDocument::message(
                    l("File too large to display", "文件过大，无法显示"),
                    byte_size_string(data.len() as u64),
                );
            }
            let (buffer, editing) = make_worktree_side(&data, language, &full, None);
            let mut doc = DiffDocument::new(TextBuffer::default(), buffer, Vec::new(), language).plain_file();
            doc.editing = Some(editing);
            return doc;
        }
        Source::Worktree { base } => {
            let has_old = !matches!(request.kind, Some(ChangeKind::Added) | Some(ChangeKind::Untracked));
            let has_new = request.kind != Some(ChangeKind::Deleted);
            let old_path = request.old_path.as_deref().unwrap_or(&request.path);
            (old_data, new_data) = std::thread::scope(|s| {
                let old = s.spawn(|| if has_old { git::blob(base, old_path, root) } else { None });
                let new = if has_new { read_worktree(&request.full_path()) } else { None };
                (old.join().unwrap_or(None), new)
            });
            // A commit compared with is named by its short hash, as in a commit's own diff.
            left_title = if base == EMPTY_TREE {
                l("Empty", "空")
            } else if base == ConflictBase::Base.rev() {
                l("Base (common ancestor)", "共同祖先")
            } else if base == ConflictBase::Incoming.rev() {
                l("Incoming", "传入的版本")
            } else if base.len() == 40 && base.bytes().all(|b| b.is_ascii_hexdigit()) {
                base[..7].to_string()
            } else {
                base.clone()
            };
            right_title = if request.kind == Some(ChangeKind::Conflicted) {
                l("Result (Working Tree)", "合并结果（工作区）")
            } else {
                l("Working Tree", "工作区")
            };
        }
        Source::Commit { hash, parent } => {
            let has_old = request.kind != Some(ChangeKind::Added) && parent.is_some();
            let has_new = request.kind != Some(ChangeKind::Deleted);
            let old_path = request.old_path.as_deref().unwrap_or(&request.path);
            (old_data, new_data) = std::thread::scope(|s| {
                let old = s.spawn(|| match (has_old, parent) {
                    (true, Some(p)) => git::blob(p, old_path, root),
                    _ => None,
                });
                let new = if has_new { git::blob(hash, &request.path, root) } else { None };
                (old.join().unwrap_or(None), new)
            });
            left_title = parent.as_ref().map(|p| p.chars().take(7).collect()).unwrap_or_else(|| l("Empty", "空"));
            right_title = hash.chars().take(7).collect();
        }
    }

    let finish = |mut doc: DiffDocument| {
        doc.left_title = left_title.clone();
        doc.right_title = right_title.clone();
        doc
    };

    if old_data.is_none() && new_data.is_none() {
        return finish(DiffDocument::message(
            l("Nothing to show", "没有可显示的内容"),
            l("The file does not exist on either side.", "文件在两侧都不存在。"),
        ));
    }
    if is_image(&request.path) {
        return finish(
            DiffDocument::new(TextBuffer::default(), TextBuffer::default(), Vec::new(), &syntax::PLAIN)
                .with_content(Content::Image { old: old_data, new: new_data }),
        );
    }
    let binary = |d: &Option<Vec<u8>>| d.as_deref().map(text_codec::looks_binary).unwrap_or(false);
    if binary(&old_data) || binary(&new_data) {
        let sizes: Vec<String> =
            [&old_data, &new_data].iter().filter_map(|d| d.as_ref().map(|d| byte_size_string(d.len() as u64))).collect();
        return finish(DiffDocument::message(l("Binary file changed", "二进制文件有变化"), sizes.join(" → ")));
    }
    let largest = old_data.as_ref().map_or(0, Vec::len).max(new_data.as_ref().map_or(0, Vec::len));
    if largest > MAX_TEXT_BYTES {
        return finish(DiffDocument::message(
            l("File too large to diff", "文件过大，无法比较"),
            byte_size_string(largest as u64),
        ));
    }

    // Decode and highlight both sides in parallel. The working tree side is read so that it can be edited.
    let worktree_path =
        if matches!(request.source, Source::Worktree { .. }) && new_data.is_some() { Some(request.full_path()) } else { None };
    let (left, (right, editing), raw_hunks) = std::thread::scope(|s| {
        let left = s.spawn(|| make_buffer(old_data.as_deref(), language));
        let hunks = s.spawn(|| compute_hunks(old_data.as_deref(), new_data.as_deref(), request.ignore_whitespace));
        let right = match (&new_data, &worktree_path) {
            (Some(data), Some(path)) => {
                let (b, e) = make_worktree_side(data, language, path, old_data.clone());
                (b, Some(e))
            }
            _ => (make_buffer(new_data.as_deref(), language), None),
        };
        (left.join().unwrap_or_default(), right, hunks.join().unwrap_or(None))
    });

    let mut hunks = if old_data.is_none() {
        // A new file: every line is added. An empty one has nothing to show, though it has a line to type on.
        if new_data.as_ref().is_none_or(|d| d.is_empty()) {
            Vec::new()
        } else {
            vec![Hunk::lines(0, 0, 0, right.len())]
        }
    } else if new_data.is_none() {
        if left.is_empty() {
            Vec::new()
        } else {
            vec![Hunk::lines(0, left.len(), 0, 0)]
        }
    } else if let Some(computed) = raw_hunks {
        computed
    } else {
        return finish(DiffDocument::message(l("Could not compute diff", "无法计算差异"), request.path.clone()));
    };
    add_inline_highlights(&mut hunks, left.lines(), right.lines());

    let note = note(&hunks, old_data.as_deref(), new_data.as_deref(), request.ignore_whitespace);
    let mut doc = DiffDocument::new(left, right, hunks, language);
    doc.editing = editing;
    doc.note = note;
    finish(doc)
}

/// Word-level highlights for the changed lines of each modified hunk (left alone above 200 lines a side).
pub fn add_inline_highlights(hunks: &mut [Hunk], left: &[String], right: &[String]) {
    for h in hunks.iter_mut() {
        if h.kind() != HunkKind::Modified
            || h.left_count > 200
            || h.right_count > 200
            || h.left_end() > left.len()
            || h.right_end() > right.len()
        {
            continue;
        }
        if let Some((old, new)) = inline_diff(&left[h.left_start..h.left_end()], &right[h.right_start..h.right_end()]) {
            h.left_inline = old;
            h.right_inline = new;
        }
    }
}

/// Said over a diff with no differences in it, when both sides exist. The lines can be the same while the bytes are
/// not: line endings git converts (`core.autocrlf`, on by default in Git for Windows), or the same text in another
/// encoding.
pub fn note(hunks: &[Hunk], old: Option<&[u8]>, new: Option<&[u8]>, ignore_whitespace: bool) -> Option<String> {
    let (Some(old), Some(new)) = (old, new) else { return None };
    if !hunks.is_empty() {
        return None;
    }
    Some(if old == new {
        l("Contents are identical", "内容完全相同")
    } else if ignore_whitespace {
        l("Only whitespace differs", "仅空白字符不同")
    } else {
        l("Only line endings or encoding differ", "仅换行符或编码不同")
    })
}

/// Whether this process may write the file (permissions, read-only attribute).
pub fn is_writable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let Ok(c) = std::ffi::CString::new(path.as_os_str().as_bytes()) else { return false };
        unsafe { libc::access(c.as_ptr(), libc::W_OK) == 0 }
    }
    #[cfg(not(unix))]
    {
        std::fs::metadata(path).map(|m| !m.permissions().readonly()).unwrap_or(false)
    }
}

/// The working tree side of a document: decoded so that it can be written back where that is possible,
/// and highlighted keeping the lexer states that editing needs. Otherwise read as any other side, with the reason.
fn make_worktree_side(
    data: &[u8],
    language: &'static LanguageSpec,
    path: &Path,
    base: Option<Vec<u8>>,
) -> (TextBuffer, EditInfo) {
    let mut info = EditInfo::default();
    let is_link = std::fs::symlink_metadata(path).map(|m| m.file_type().is_symlink()).unwrap_or(false);
    if is_link {
        info.read_only_reason = Some(l("A symbolic link · read-only", "符号链接 · 只读"));
    } else if !is_writable(path) {
        info.read_only_reason = Some(l("No permission to write it · read-only", "没有写入权限 · 只读"));
    } else if data.len() > text_editing::MAX_EDITABLE_BYTES {
        info.read_only_reason = Some(l("Larger than 4 MB · read-only", "文件超过 4 MB · 只读"));
    } else if let Some(text) = text_codec::decode_for_editing(data) {
        if text.lines.iter().any(|l| l.len() > text_editing::MAX_LINE_BYTES) {
            info.read_only_reason = Some(l("Has lines too long to edit · read-only", "有过长的行 · 只读"));
        } else {
            info.format = text.format;
            info.crlf = text.crlf;
            info.fingerprint = Some(text_codec::fingerprint(data));
            info.base = base;
            let (spans, states) = syntax::highlight_tracking(&text.lines, language);
            let mut buffer = TextBuffer::new(text.lines);
            buffer.spans = spans;
            buffer.states = Some(states);
            return (buffer, info);
        }
    } else {
        info.read_only_reason =
            Some(l("Its encoding cannot be written back exactly · read-only", "编码无法原样写回 · 只读"));
    }
    (make_buffer(Some(data), language), info)
}

fn make_buffer(data: Option<&[u8]>, language: &'static LanguageSpec) -> TextBuffer {
    let Some(data) = data else { return TextBuffer::default() };
    let mut buffer = TextBuffer::new(text_codec::split_lines(&text_codec::decode(data)));
    if data.len() <= 4 * 1024 * 1024 {
        buffer.spans = syntax::highlight(buffer.lines(), language);
    }
    buffer
}

/// Line-level hunks via `git diff --no-index` on the exact bytes being displayed, so the
/// hunks always agree with the text on screen regardless of filters, renames or repo state.
/// None when git could not compare them.
pub fn compute_hunks(old: Option<&[u8]>, new: Option<&[u8]>, ignore_whitespace: bool) -> Option<Vec<Hunk>> {
    compute_hunks_with(old, new, ignore_whitespace, &[])
}

/// `compute_hunks` with `env` added to git's environment (tests give it settings of their own).
fn compute_hunks_with(
    old: Option<&[u8]>,
    new: Option<&[u8]>,
    ignore_whitespace: bool,
    env: &[(&str, &str)],
) -> Option<Vec<Hunk>> {
    let (Some(old), Some(new)) = (old, new) else { return Some(Vec::new()) };
    if old == new {
        return Some(Vec::new());
    }
    // Git takes UTF-16 for binary (it is full of zero bytes). As UTF-8 the lines are the same lines.
    let converted;
    let (old, new) = if is_utf16(old) || is_utf16(new) {
        converted = (text_codec::decode(old).into_bytes(), text_codec::decode(new).into_bytes());
        if converted.0 == converted.1 {
            return Some(Vec::new());
        }
        (converted.0.as_slice(), converted.1.as_slice())
    } else {
        (old, new)
    };
    let scratch = scratch_directory();
    let id = unique_name();
    let a = format!("{id}.a");
    let b = format!("{id}.b");
    let (pa, pb) = (scratch.join(&a), scratch.join(&b));
    struct Cleanup(PathBuf, PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
            let _ = std::fs::remove_file(&self.1);
        }
    }
    let _cleanup = Cleanup(pa.clone(), pb.clone());
    std::fs::create_dir_all(scratch).ok()?;
    std::fs::write(&pa, old).ok()?;
    std::fs::write(&pb, new).ok()?;
    // DiffCode shows these files as text. The user's own gitattributes still apply to them: without `--text` a rule
    // such as `*.a binary` (in common templates) has git call them binary, and a textconv driver would compare
    // something else than what is shown.
    let mut args: Vec<String> = [
        "diff",
        "--no-index",
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
        "--text",
        "-U0",
        "--diff-algorithm=histogram",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    if ignore_whitespace {
        args.push("-w".into());
    }
    args.push(a);
    args.push(b);
    let mut command = git::cmd(scratch, args).ok(&[0, 1]);
    for (key, value) in env {
        command = command.env(key, *value);
    }
    let output = command.run().ok()?;
    let (hunks, binary) = parse_hunks(&output);
    // Files git did not compare line by line are not files without differences.
    (!binary).then_some(hunks)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buf(lines: &[&str]) -> TextBuffer {
        TextBuffer::new(lines.iter().map(|s| s.to_string()).collect())
    }

    #[test]
    fn parses_zero_context_hunks() {
        let out = b"diff --git a/x b/x\n@@ -3 +3 @@ x\n-a\n+b\n@@ -10,0 +11,2 @@\n+c\n+d\n@@ -20,2 +22,0 @@\n";
        let (hunks, binary) = parse_hunks(out);
        assert!(!binary);
        assert_eq!(hunks, vec![Hunk::lines(2, 1, 2, 1), Hunk::lines(10, 0, 10, 2), Hunk::lines(19, 2, 22, 0)]);
        assert!(parse_hunks(b"Binary files a and b differ\n").1);
    }

    #[test]
    fn layout_and_scroll_mapping() {
        let left = buf(&["a", "b", "c", "d"]);
        let right = buf(&["a", "x", "y", "z", "c", "d"]);
        let doc = DiffDocument::new(left, right, vec![Hunk::lines(1, 1, 1, 3)], &syntax::PLAIN);
        assert_eq!(doc.virtual_line_count(), 6);
        assert_eq!(doc.segments().len(), 3);
        assert_eq!(doc.pane_positions(2.5), (1.0 + 1.5 / 3.0, 2.5));
        assert_eq!(doc.virtual_position(2, Side::Left), 4.0);
        assert_eq!(doc.virtual_position(3, Side::Right), 3.0);
        let rows = doc.unified_rows();
        assert_eq!(rows.len(), 7);
        assert_eq!(rows[1].kind, RowKind::Removed);
        assert_eq!(doc.unified_row_of_line(4, Side::Right), 5);
        assert_eq!(doc.unified_row_of_hunk(0), 1);
    }

    #[test]
    fn layout_clamps_hunks_past_the_end() {
        let doc = DiffDocument::new(buf(&["a"]), buf(&["a", "b"]), vec![Hunk::lines(1, 0, 1, 5)], &syntax::PLAIN);
        assert_eq!(doc.hunks(), &[Hunk::lines(1, 0, 1, 1)]);
    }

    #[test]
    fn inline_ranges_are_bytes() {
        let old = ["let name = \"旧的\";"];
        let new = ["let title = \"新的\";"];
        let (a, b) = inline_diff(&old, &new).unwrap();
        let pick = |s: &str, r: &Range<u32>| s[r.start as usize..r.end as usize].to_string();
        assert_eq!(a[0].iter().map(|r| pick(old[0], r)).collect::<Vec<_>>(), vec!["name", "旧"]);
        assert_eq!(b[0].iter().map(|r| pick(new[0], r)).collect::<Vec<_>>(), vec!["title", "新"]);
    }

    #[test]
    fn whole_line_changes_have_no_inline_ranges() {
        let (a, _) = inline_diff(&["foo(1)", "same"], &["bar baz", "same"]).unwrap_or_default();
        assert!(a.iter().all(|l| l.is_empty()));
    }

    #[test]
    fn myers_finds_lcs() {
        let (a, b) = myers_matches(&[1, 2, 3, 4, 5], &[1, 3, 4, 6, 5], 100).unwrap();
        assert_eq!(a, vec![true, false, true, true, true]);
        assert_eq!(b, vec![true, true, true, false, true]);
        assert!(myers_matches(&[1, 2, 3], &[4, 5, 6], 2).is_none());
    }

    #[test]
    fn buffer_replace_keeps_colours_aligned() {
        let lines: Vec<String> = ["val a = 1", "val b = 2"].iter().map(|s| s.to_string()).collect();
        let (spans, states) = syntax::highlight_tracking(&lines, &syntax::KOTLIN);
        let mut b = TextBuffer::new(lines);
        b.spans = spans;
        b.states = Some(states);
        let r = b.replace(TextPosition::new(0, 9), TextPosition::new(0, 9), "\n/* c");
        assert_eq!(b.len(), 3);
        assert_eq!(b.spans.len(), 3);
        assert_eq!(b.states.as_ref().unwrap().len(), 4);
        let range = b.rehighlight(&syntax::KOTLIN, r.raw.start, r.raw.inserted);
        assert_eq!(range, 0..3);
        assert_eq!(b.spans_at(2)[0].kind, syntax::TokenKind::Comment);
    }

    #[test]
    fn computes_hunks_with_git() {
        if !git::is_available() {
            return;
        }
        let hunks = compute_hunks(Some(b"a\nb\nc\n"), Some(b"a\nB\nc\nd\n"), false).unwrap();
        assert_eq!(hunks, vec![Hunk::lines(1, 1, 1, 1), Hunk::lines(3, 0, 3, 1)]);
    }

    /// Issue #4: the user's own gitattributes apply to the scratch files git compares. One that called them binary
    /// left no hunks, shown as "Contents are identical".
    #[test]
    fn the_users_gitattributes_do_not_hide_changes() {
        if !git::is_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let settings = |name: &str, attributes: &str, config: &str| {
            let attributes_file = dir.path().join(format!("{name}.attributes"));
            let config_file = dir.path().join(format!("{name}.config"));
            std::fs::write(&attributes_file, attributes).unwrap();
            // Forward slashes: a backslash starts an escape in git's settings files.
            let path = attributes_file.to_string_lossy().replace('\\', "/");
            std::fs::write(&config_file, format!("[core]\n\tattributesFile = {path}\n{config}")).unwrap();
            config_file.to_string_lossy().into_owned()
        };
        let (old, new) = (&b"fdsf\n"[..], &b"fdsf1111\ndfxxs\n"[..]);
        let expected = Some(vec![Hunk::lines(0, 1, 0, 2)]);

        let binary = settings("binary", "*.a binary\n", "");
        assert_eq!(compute_hunks_with(Some(old), Some(new), false, &[("GIT_CONFIG_GLOBAL", &binary)]), expected);

        // A textconv driver that turns everything into nothing would make any two files the same.
        let textconv = settings("textconv", "* diff=nothing\n", "[diff \"nothing\"]\n\ttextconv = true\n");
        assert_eq!(compute_hunks_with(Some(old), Some(new), false, &[("GIT_CONFIG_GLOBAL", &textconv)]), expected);
    }

    #[test]
    fn different_bytes_are_not_called_identical() {
        let identical = note(&[], Some(b"a\n"), Some(b"a\n"), false);
        let line_endings = note(&[], Some(b"a\n"), Some(b"a\r\n"), false);
        assert!(identical.is_some() && line_endings.is_some());
        assert_ne!(line_endings, identical);
        assert_eq!(note(&[Hunk::lines(0, 1, 0, 1)], Some(b"a\n"), Some(b"b\n"), false), None);
    }

    #[test]
    fn loads_a_worktree_diff() {
        if !git::is_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let run = |args: &[&str]| git::cmd(root, args.iter().copied()).run().unwrap();
        run(&["init", "-q"]);
        run(&["config", "user.email", "t@example.com"]);
        run(&["config", "user.name", "t"]);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/Main.kt"), "fun main() {\n    println(1)\n}\n").unwrap();
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "init"]);
        std::fs::write(root.join("src/Main.kt"), "fun main() {\n    println(2)\n}\n").unwrap();
        let doc = load(&DiffRequest {
            root: root.to_path_buf(),
            path: "src/Main.kt".into(),
            old_path: None,
            kind: Some(ChangeKind::Modified),
            source: Source::Worktree { base: "HEAD".into() },
            ignore_whitespace: false,
        });
        assert!(matches!(doc.content, Content::Text), "{:?}", doc.content);
        assert_eq!(doc.hunks().len(), 1);
        assert_eq!(doc.hunks()[0].left_start, 1);
        assert_eq!(doc.language.name, "Kotlin");
        let editing = doc.editing.as_ref().unwrap();
        assert!(editing.read_only_reason.is_none());
        assert!(editing.base.is_some());
        assert_eq!(doc.hunks()[0].right_inline[0], vec![12..13]);
    }
}
