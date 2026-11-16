//! What the Annotate column shows: for each line of the text on screen, the commit that last changed it.

use crate::diff::TextBuffer;
use crate::history::Blame;
use crate::text_editing::LineEdit;
use crate::util::date_text;

#[derive(Clone, Debug, PartialEq)]
pub struct AnnotationEntry {
    pub hash: String,
    pub author: String,
    pub email: String,
    /// Seconds since 1970.
    pub date: i64,
    pub summary: String,
    /// "2026-11-13 Alice", cut to fit the column.
    pub label: String,
    /// 0 for the oldest commit in the file, 1 for the newest.
    pub recency: f64,
}

impl AnnotationEntry {
    pub fn short_hash(&self) -> &str {
        &self.hash[..self.hash.len().min(7)]
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct LineAnnotations {
    pub entries: Vec<AnnotationEntry>,
    /// The entry of each line, in order; None for a line that is not committed yet.
    pub lines: Vec<Option<u32>>,
    /// Width of the column, in character cells.
    pub columns: usize,
}

/// Longer names are cut, so one long name does not widen the column for everyone.
pub const MAX_AUTHOR_COLUMNS: usize = 12;

impl LineAnnotations {
    /// `line_count`: the lines the text on screen has; the blame is cut or padded to match.
    pub fn from_blame(blame: &Blame, line_count: usize) -> Self {
        let mut lines = blame.lines.clone();
        lines.resize(line_count, None);
        // Recency by rank rather than by date, so a file with one ancient commit still shades its recent ones apart.
        let mut order: Vec<usize> = (0..blame.entries.len()).collect();
        order.sort_by_key(|&i| blame.entries[i].date);
        let mut recency = vec![1.0; blame.entries.len()];
        if order.len() > 1 {
            for (position, &index) in order.iter().enumerate() {
                recency[index] = position as f64 / (order.len() - 1) as f64;
            }
        }
        let names: Vec<String> = blame
            .entries
            .iter()
            .map(|e| cut(if e.author.is_empty() { "?" } else { &e.author }, MAX_AUTHOR_COLUMNS))
            .collect();
        let widest = names.iter().map(|n| TextBuffer::columns(n)).max().unwrap_or(0);
        let entries = blame
            .entries
            .iter()
            .enumerate()
            .map(|(i, e)| AnnotationEntry {
                hash: e.hash.clone(),
                author: e.author.clone(),
                email: e.email.clone(),
                date: e.date,
                summary: e.summary.clone(),
                label: format!("{} {}", date_text(e.date), names[i]),
                recency: recency[i],
            })
            .collect();
        LineAnnotations { entries, lines, columns: 11 + widest.max(4) }
    }

    /// A column of the given width with nothing in it yet: holds the text in place while the blame runs.
    pub fn placeholder(columns: usize, line_count: usize) -> Self {
        LineAnnotations { entries: Vec::new(), lines: vec![None; line_count], columns }
    }

    pub fn is_placeholder(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn index_at_line(&self, line: usize) -> Option<u32> {
        let index = (*self.lines.get(line)?)?;
        ((index as usize) < self.entries.len()).then_some(index)
    }

    pub fn entry_at_line(&self, line: usize) -> Option<&AnnotationEntry> {
        self.index_at_line(line).map(|i| &self.entries[i as usize])
    }

    /// The first line of a run of lines from the same commit, where its label goes.
    pub fn is_run_start(&self, line: usize) -> bool {
        line == 0 || line >= self.lines.len() || self.lines[line - 1] != self.lines[line]
    }

    /// Follows an edit of the text: lines that were rewritten are not committed any more, the rest keep their commit.
    pub fn apply(&mut self, edit: LineEdit) {
        let start = edit.start.min(self.lines.len());
        let end = (start + edit.removed).min(self.lines.len());
        self.lines.splice(start..end, std::iter::repeat_n(None, edit.inserted));
    }
}

/// `text` cut to at most `limit` character cells as `TextBuffer::columns` counts them, with "…" when cut.
pub fn cut(text: &str, limit: usize) -> String {
    if TextBuffer::columns(text) <= limit {
        return text.to_string();
    }
    let room = limit.saturating_sub(TextBuffer::columns("…"));
    let mut result = String::new();
    let mut used = 0;
    let mut buf = [0u8; 4];
    for c in text.chars() {
        let width = TextBuffer::columns(c.encode_utf8(&mut buf));
        if used + width > room {
            break;
        }
        result.push(c);
        used += width;
    }
    result.push('…');
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::BlameEntry;

    #[test]
    fn builds_from_blame() {
        let blame = Blame {
            entries: vec![
                BlameEntry { hash: "a".repeat(40), author: "Alexandria Longname".into(), date: 200, ..Default::default() },
                BlameEntry { hash: "b".repeat(40), author: "Bo".into(), date: 100, ..Default::default() },
            ],
            lines: vec![Some(0), Some(0), Some(1)],
        };
        let a = LineAnnotations::from_blame(&blame, 4);
        assert_eq!(a.lines.len(), 4);
        assert_eq!(a.entries[0].recency, 1.0);
        assert_eq!(a.entries[1].recency, 0.0);
        assert!(a.entries[0].label.ends_with("Alexandria…"), "{}", a.entries[0].label);
        assert_eq!(a.columns, 11 + 12);
        assert!(a.is_run_start(0) && !a.is_run_start(1) && a.is_run_start(2));
        assert_eq!(a.entry_at_line(3), None);
    }

    #[test]
    fn edits_uncommit_lines() {
        let mut a = LineAnnotations { entries: Vec::new(), lines: vec![Some(0), Some(1), Some(2)], columns: 15 };
        a.apply(LineEdit { start: 1, removed: 1, inserted: 2 });
        assert_eq!(a.lines, vec![Some(0), None, None, Some(2)]);
        assert_eq!(cut("中文名字很长", 6), "中文…");
    }
}
