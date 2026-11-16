//! The text operations behind editing, on plain vectors of lines (no line terminators in them).
//!
//! Positions are a line and a column; the column is a UTF-8 byte offset into the line that always sits on a
//! character boundary (the Mac version counts UTF-16 units, which is what its text system uses).

use unicode_segmentation::GraphemeCursor;

use crate::diff::Hunk;

/// Lines longer than this (in UTF-8 bytes) are drawn cut short, so a file that has one stays read-only.
pub const MAX_LINE_BYTES: usize = 6000;
/// Above this a file is shown but not edited: every keystroke would be paid for in proportion.
pub const MAX_EDITABLE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TextPosition {
    pub row: usize,
    /// UTF-8 byte offset within the row's text, on a character boundary.
    pub column: usize,
}

impl TextPosition {
    pub const fn new(row: usize, column: usize) -> Self {
        TextPosition { row, column }
    }
}

/// Lines `start..start + removed` were replaced by `inserted` new lines.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LineEdit {
    pub start: usize,
    pub removed: usize,
    pub inserted: usize,
}

impl LineEdit {
    pub fn delta(&self) -> isize {
        self.inserted as isize - self.removed as isize
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Replacement {
    /// Where the inserted text ends.
    pub end: TextPosition,
    /// The lines that were rewritten.
    pub raw: LineEdit,
    /// What is left of `raw` once identical lines at either end are taken out.
    pub change: LineEdit,
}

/// The lines of text about to be inserted. "\r\n", "\r" and "\n" all end a line.
pub fn lines_of(text: &str) -> Vec<String> {
    let buf = text.as_bytes();
    let mut result = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < buf.len() {
        let byte = buf[i];
        if byte == b'\n' || byte == b'\r' {
            result.push(text[start..i].to_string());
            if byte == b'\r' && i + 1 < buf.len() && buf[i + 1] == b'\n' {
                i += 1;
            }
            start = i + 1;
        }
        i += 1;
    }
    result.push(text[start..].to_string());
    result
}

/// The nearest character boundary at or before `column`.
pub fn floor_boundary(line: &str, column: usize) -> usize {
    let mut c = column.min(line.len());
    while c > 0 && !line.is_char_boundary(c) {
        c -= 1;
    }
    c
}

/// `position` moved onto the text: a line that exists, a column inside it on a character boundary.
pub fn clamp<S: AsRef<str>>(position: TextPosition, lines: &[S]) -> TextPosition {
    if lines.is_empty() {
        return TextPosition::default();
    }
    if position.row >= lines.len() {
        let row = lines.len() - 1;
        return TextPosition::new(row, lines[row].as_ref().len());
    }
    let line = lines[position.row].as_ref();
    TextPosition::new(position.row, floor_boundary(line, position.column))
}

/// The text between two positions, lines joined with "\n".
pub fn text<S: AsRef<str>>(lines: &[S], from: TextPosition, to: TextPosition) -> String {
    if lines.is_empty() {
        return String::new();
    }
    let a = clamp(from.min(to), lines);
    let b = clamp(from.max(to), lines);
    if a.row == b.row {
        return lines[a.row].as_ref()[a.column..b.column].to_string();
    }
    let mut out = String::new();
    out.push_str(&lines[a.row].as_ref()[a.column..]);
    for line in &lines[a.row + 1..b.row] {
        out.push('\n');
        out.push_str(line.as_ref());
    }
    out.push('\n');
    out.push_str(&lines[b.row].as_ref()[..b.column]);
    out
}

/// Replaces the text between `from` and `to` with `text`.
///
/// Returns where the inserted text ends, the lines that were rewritten (`raw`), and the smaller change
/// that is left once identical lines at either end are taken out (`change`): typing a line break at the
/// end of a line rewrites that line but only adds the one after it.
pub fn replace(lines: &mut Vec<String>, from: TextPosition, to: TextPosition, text: &str) -> Replacement {
    if lines.is_empty() {
        lines.push(String::new());
    }
    let a = clamp(from.min(to), lines);
    let b = clamp(from.max(to), lines);
    let head = lines[a.row][..a.column].to_string();
    let tail = lines[b.row][b.column..].to_string();
    let mut pieces = lines_of(text);
    let last = pieces.len() - 1;
    let end_column = if pieces.len() == 1 { head.len() } else { 0 } + pieces[last].len();
    pieces[0].insert_str(0, &head);
    pieces[last].push_str(&tail);

    let old: Vec<String> = lines.splice(a.row..=b.row, pieces.iter().cloned()).collect();

    // Byte comparison, as everywhere: two spellings of "é" are different file contents.
    let limit = old.len().min(pieces.len());
    let mut lead = 0;
    while lead < limit && old[lead] == pieces[lead] {
        lead += 1;
    }
    let mut trail = 0;
    while trail < limit - lead && old[old.len() - 1 - trail] == pieces[pieces.len() - 1 - trail] {
        trail += 1;
    }
    Replacement {
        end: TextPosition::new(a.row + pieces.len() - 1, end_column),
        raw: LineEdit { start: a.row, removed: old.len(), inserted: pieces.len() },
        change: LineEdit {
            start: a.row + lead,
            removed: old.len() - lead - trail,
            inserted: pieces.len() - lead - trail,
        },
    }
}

/// The hunks of a diff, moved to fit the right side after `change`, until the real diff comes back.
///
/// Hunks above the change stay, hunks below it move by the number of lines added or removed, and the
/// change merges with whatever hunks it touches into one, lined up against the left side where the
/// surrounding unchanged lines put it.
pub fn adjust(hunks: &[Hunk], change: LineEdit) -> Vec<Hunk> {
    if change.removed == 0 && change.inserted == 0 {
        return hunks.to_vec();
    }
    let s = change.start as isize;
    let e = (change.start + change.removed) as isize;
    let delta = change.delta();
    let mut before: Vec<Hunk> = Vec::new();
    let mut touching: Vec<Hunk> = Vec::new();
    let mut after: Vec<Hunk> = Vec::new();
    for hunk in hunks {
        let hs = hunk.right_start as isize;
        let he = (hunk.right_start + hunk.right_count) as isize;
        let overlaps = hs < e && he > s;
        let hunk_inside_change = hunk.right_count == 0 && s < hs && hs < e;
        let change_inside_hunk = change.removed == 0 && hs < s && s < he;
        let same_point = hunk.right_count == 0 && change.removed == 0 && hs == s;
        if overlaps || hunk_inside_change || change_inside_hunk || same_point {
            touching.push(Hunk::lines(hunk.left_start, hunk.left_count, hunk.right_start, hunk.right_count));
        } else if he <= s {
            before.push(hunk.clone());
        } else {
            let mut moved = hunk.clone();
            moved.right_start = (moved.right_start as isize + delta).max(0) as usize;
            after.push(moved);
        }
    }

    // The left line level with right line `r`, in the run of unchanged lines that follows `previous`.
    let level = |r: isize, previous: Option<&Hunk>| -> isize {
        match previous {
            None => r,
            Some(p) => {
                r - (p.right_start + p.right_count) as isize + (p.left_start + p.left_count) as isize
            }
        }
    };
    let right_start = s.min(touching.first().map(|h| h.right_start as isize).unwrap_or(s));
    let right_end = e.max(touching.iter().map(|h| (h.right_start + h.right_count) as isize).max().unwrap_or(e));
    let left_start = match touching.first() {
        Some(first) if first.right_start as isize <= s => first.left_start as isize,
        _ => level(s, before.last()),
    };
    let left_end = match touching.last() {
        Some(last) if (last.right_start + last.right_count) as isize >= e => (last.left_start + last.left_count) as isize,
        _ => level(e, touching.last().or(before.last())),
    };
    let merged = Hunk::lines(
        left_start.max(0) as usize,
        (left_end - left_start).max(0) as usize,
        right_start.max(0) as usize,
        (right_end - right_start + delta).max(0) as usize,
    );
    let mut result = before;
    // Back to how the left side has it: no hunk left at all.
    if merged.left_count > 0 || merged.right_count > 0 {
        result.push(merged);
    }
    result.extend(after);
    result
}

// MARK: Moving through text

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CharacterClass {
    Space,
    Word,
    Punctuation,
}

pub fn character_class(c: char) -> CharacterClass {
    match c {
        ' ' | '\t' => CharacterClass::Space,
        '0'..='9' | 'A'..='Z' | 'a'..='z' | '_' => CharacterClass::Word,
        c if c as u32 >= 0x80 => CharacterClass::Word,
        _ => CharacterClass::Punctuation,
    }
}

/// Where a word-wise move from `column` lands: past the spaces, then past one run of word or punctuation characters.
pub fn word_boundary(text: &str, column: usize, forward: bool) -> usize {
    let mut i = floor_boundary(text, column);
    if forward {
        let rest = &text[i..];
        let mut chars = rest.char_indices().peekable();
        while let Some(&(_, c)) = chars.peek() {
            if character_class(c) != CharacterClass::Space {
                break;
            }
            chars.next();
        }
        let Some(&(_, first)) = chars.peek() else { return text.len() };
        let kind = character_class(first);
        let mut end = text.len();
        for (offset, c) in chars {
            if character_class(c) != kind {
                end = i + offset;
                break;
            }
        }
        end
    } else {
        let mut chars = text[..i].char_indices().rev().peekable();
        while let Some(&(offset, c)) = chars.peek() {
            if character_class(c) != CharacterClass::Space {
                break;
            }
            i = offset;
            chars.next();
        }
        let Some(&(_, first)) = chars.peek() else { return i };
        let kind = character_class(first);
        for (offset, c) in chars {
            if character_class(c) != kind {
                break;
            }
            i = offset;
        }
        i
    }
}

/// The column one character (as the user sees characters: an emoji, a letter with its accents) away.
pub fn character_boundary(text: &str, column: usize, forward: bool) -> usize {
    let i = floor_boundary(text, column);
    let mut cursor = GraphemeCursor::new(i, text.len(), true);
    let next = if forward { cursor.next_boundary(text, 0) } else { cursor.prev_boundary(text, 0) };
    match next {
        Ok(Some(b)) => b,
        _ => {
            if forward {
                text.len()
            } else {
                0
            }
        }
    }
}

/// Leading spaces and tabs.
pub fn indentation(line: &str) -> &str {
    let end = line.bytes().position(|b| b != b' ' && b != b'\t').unwrap_or(line.len());
    &line[..end]
}

/// What one press of Tab inserts in this file: a tab where the file indents with tabs, otherwise spaces.
/// The width is the step that indentation most often increases by.
pub fn indent_unit<S: AsRef<str>>(lines: &[S]) -> String {
    let mut tabs = 0;
    let mut spaces = 0;
    let mut steps = [0usize; 9];
    let mut previous = 0usize;
    for line in lines.iter().take(4000) {
        let bytes = line.as_ref().as_bytes();
        let Some(&first) = bytes.first() else { continue };
        if first == b'\t' {
            tabs += 1;
            continue;
        }
        let width = bytes.iter().take_while(|&&b| b == b' ').count();
        // Blank lines and lines of only spaces say nothing about the indentation.
        if width == bytes.len() {
            continue;
        }
        if width > 0 {
            spaces += 1;
        }
        if width > previous && matches!(width - previous, 2 | 3 | 4 | 8) {
            steps[width - previous] += 1;
        }
        previous = width;
    }
    if tabs > spaces {
        return "\t".to_string();
    }
    // Most frequent step; on a tie the smaller step wins.
    let mut best = 4;
    let mut best_count = 0;
    for step in [2, 3, 4, 8] {
        if steps[step] > best_count {
            best = step;
            best_count = steps[step];
        }
    }
    " ".repeat(best)
}

/// Display columns up to a byte column, with tab stops every four.
pub fn visual_column(line: &str, column: usize) -> usize {
    let mut visual = 0;
    for c in line[..floor_boundary(line, column)].chars() {
        if c == '\t' {
            visual += 4 - visual % 4;
        } else {
            visual += 1;
        }
    }
    visual
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn splits_inserted_text() {
        assert_eq!(lines_of("a\r\nb\rc\nd"), v(&["a", "b", "c", "d"]));
        assert_eq!(lines_of(""), v(&[""]));
        assert_eq!(lines_of("x\n"), v(&["x", ""]));
    }

    #[test]
    fn newline_at_end_adds_one_line() {
        let mut lines = v(&["hello", "world"]);
        let r = replace(&mut lines, TextPosition::new(0, 5), TextPosition::new(0, 5), "\n");
        assert_eq!(lines, v(&["hello", "", "world"]));
        assert_eq!(r.end, TextPosition::new(1, 0));
        assert_eq!(r.raw, LineEdit { start: 0, removed: 1, inserted: 2 });
        assert_eq!(r.change, LineEdit { start: 1, removed: 0, inserted: 1 });
    }

    #[test]
    fn replace_across_lines_with_cjk() {
        let mut lines = v(&["你好世界", "abc"]);
        let r = replace(&mut lines, TextPosition::new(0, 3), TextPosition::new(1, 1), "—");
        assert_eq!(lines, v(&["你—bc"]));
        assert_eq!(r.end, TextPosition::new(0, 6));
        assert_eq!(text(&lines, TextPosition::new(0, 0), TextPosition::new(0, 6)), "你—");
    }

    #[test]
    fn clamp_lands_on_char_boundary() {
        let lines = v(&["中文"]);
        assert_eq!(clamp(TextPosition::new(0, 4), &lines), TextPosition::new(0, 3));
        assert_eq!(clamp(TextPosition::new(5, 0), &lines), TextPosition::new(0, 6));
    }

    #[test]
    fn adjust_moves_and_merges() {
        let hunks = vec![Hunk::lines(2, 1, 2, 1), Hunk::lines(10, 0, 10, 2)];
        // One line inserted at right line 5, between the hunks.
        let adjusted = adjust(&hunks, LineEdit { start: 5, removed: 0, inserted: 1 });
        assert_eq!(adjusted.len(), 3);
        assert_eq!((adjusted[1].left_start, adjusted[1].left_count), (5, 0));
        assert_eq!((adjusted[1].right_start, adjusted[1].right_count), (5, 1));
        assert_eq!(adjusted[2].right_start, 11);
        // Typing inside the first hunk keeps it one hunk.
        let inside = adjust(&hunks, LineEdit { start: 2, removed: 1, inserted: 1 });
        assert_eq!(inside[0], Hunk::lines(2, 1, 2, 1));
    }

    #[test]
    fn word_and_character_moves() {
        let line = "let  value = foo(bar);";
        assert_eq!(word_boundary(line, 0, true), 3);
        assert_eq!(word_boundary(line, 3, true), 10);
        assert_eq!(word_boundary(line, 10, false), 5);
        assert_eq!(word_boundary(line, line.len(), false), line.len() - 2);
        let emoji = "a👍🏽b";
        assert_eq!(character_boundary(emoji, 1, true), 1 + "👍🏽".len());
        assert_eq!(character_boundary(emoji, 1 + "👍🏽".len(), false), 1);
    }

    #[test]
    fn indent_units() {
        assert_eq!(indent_unit(&v(&["fun a() {", "  x", "  y", "    z"])), "  ");
        assert_eq!(indent_unit(&v(&["a", "\tb", "\tc"])), "\t");
        assert_eq!(indent_unit(&v(&["a"])), "    ");
        assert_eq!(visual_column("\tab", 2), 5);
    }
}
