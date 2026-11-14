import Foundation

/// Lines `start..<start + removed` were replaced by `inserted` new lines.
struct LineEdit: Equatable {
    var start: Int
    var removed: Int
    var inserted: Int

    var delta: Int { inserted - removed }
}

/// The text operations behind editing, on plain arrays of lines (no line terminators in them).
/// Positions are lines and UTF-16 columns, as everywhere in the canvas.
enum TextEditing {
    /// Lines longer than this (in UTF-8 bytes) are drawn cut short, so a file that has one stays read-only.
    static let maxLineBytes = 6000
    /// Above this a file is shown but not edited: every keystroke would be paid for in proportion.
    static let maxEditableBytes = 4 * 1024 * 1024

    /// The lines of text about to be inserted. "\r\n", "\r" and "\n" all end a line.
    static func lines(of text: String) -> [String] {
        var text = text
        var result: [String] = []
        text.withUTF8 { buf in
            var start = 0, i = 0
            while i < buf.count {
                let byte = buf[i]
                if byte == 0x0A || byte == 0x0D {
                    result.append(String(decoding: UnsafeBufferPointer(rebasing: buf[start..<i]), as: UTF8.self))
                    if byte == 0x0D, i + 1 < buf.count, buf[i + 1] == 0x0A { i += 1 }
                    start = i + 1
                }
                i += 1
            }
            result.append(String(decoding: UnsafeBufferPointer(rebasing: buf[start..<buf.count]), as: UTF8.self))
        }
        return result
    }

    /// `position` moved onto the text: a line that exists, a column inside it.
    static func clamp(_ position: TextPosition, in lines: [String]) -> TextPosition {
        guard !lines.isEmpty else { return TextPosition(row: 0, column: 0) }
        if position.row < 0 { return TextPosition(row: 0, column: 0) }
        if position.row >= lines.count {
            let row = lines.count - 1
            return TextPosition(row: row, column: lines[row].utf16.count)
        }
        return TextPosition(row: position.row, column: min(max(0, position.column), lines[position.row].utf16.count))
    }

    /// The text between two positions, lines joined with "\n".
    static func text(_ lines: [String], from: TextPosition, to: TextPosition) -> String {
        guard !lines.isEmpty else { return "" }
        let a = clamp(min(from, to), in: lines), b = clamp(max(from, to), in: lines)
        if a.row == b.row {
            return (lines[a.row] as NSString).substring(with: NSRange(location: a.column, length: b.column - a.column))
        }
        var parts = [(lines[a.row] as NSString).substring(from: a.column)]
        parts += lines[(a.row + 1)..<b.row]
        parts.append((lines[b.row] as NSString).substring(to: b.column))
        return parts.joined(separator: "\n")
    }

    /// Replaces the text between `from` and `to` with `text`.
    ///
    /// Returns where the inserted text ends, the lines that were rewritten (`raw`), and the smaller change
    /// that is left once identical lines at either end are taken out (`change`): typing a line break at the
    /// end of a line rewrites that line but only adds the one after it.
    @discardableResult
    static func replace(_ lines: inout [String], from: TextPosition, to: TextPosition, with text: String)
        -> (end: TextPosition, raw: LineEdit, change: LineEdit) {
        if lines.isEmpty { lines = [""] }
        let a = clamp(min(from, to), in: lines), b = clamp(max(from, to), in: lines)
        let head = (lines[a.row] as NSString).substring(to: a.column)
        let tail = (lines[b.row] as NSString).substring(from: b.column)
        var pieces = Self.lines(of: text)
        let endColumn = (pieces.count == 1 ? (head as NSString).length : 0) + (pieces[pieces.count - 1] as NSString).length
        pieces[0] = head + pieces[0]
        pieces[pieces.count - 1] += tail

        let old = Array(lines[a.row...b.row])
        lines.replaceSubrange(a.row...b.row, with: pieces)

        // Byte comparison: two spellings of "é" are equal as Strings, but not as file contents.
        let limit = min(old.count, pieces.count)
        var lead = 0
        while lead < limit, old[lead].utf8.elementsEqual(pieces[lead].utf8) { lead += 1 }
        var trail = 0
        while trail < limit - lead, old[old.count - 1 - trail].utf8.elementsEqual(pieces[pieces.count - 1 - trail].utf8) { trail += 1 }
        let raw = LineEdit(start: a.row, removed: old.count, inserted: pieces.count)
        let change = LineEdit(start: a.row + lead, removed: old.count - lead - trail, inserted: pieces.count - lead - trail)
        return (TextPosition(row: a.row + pieces.count - 1, column: endColumn), raw, change)
    }

    /// The hunks of a diff, moved to fit the right side after `change`, until the real diff comes back.
    ///
    /// Hunks above the change stay, hunks below it move by the number of lines added or removed, and the
    /// change merges with whatever hunks it touches into one, lined up against the left side where the
    /// surrounding unchanged lines put it.
    static func adjust(_ hunks: [Hunk], for change: LineEdit) -> [Hunk] {
        guard change.removed > 0 || change.inserted > 0 else { return hunks }
        let s = change.start, e = change.start + change.removed, delta = change.delta
        var before: [Hunk] = [], touching: [Hunk] = [], after: [Hunk] = []
        for hunk in hunks {
            let hs = hunk.rightStart, he = hunk.rightStart + hunk.rightCount
            let overlaps = hs < e && he > s
            let hunkInsideChange = hunk.rightCount == 0 && s < hs && hs < e
            let changeInsideHunk = change.removed == 0 && hs < s && s < he
            let samePoint = hunk.rightCount == 0 && change.removed == 0 && hs == s
            if overlaps || hunkInsideChange || changeInsideHunk || samePoint {
                touching.append(hunk)
            } else if he <= s {
                before.append(hunk)
            } else {
                var moved = hunk
                moved.rightStart += delta
                after.append(moved)
            }
        }

        // The left line level with right line `r`, in the run of unchanged lines that follows `previous`.
        func level(_ r: Int, after previous: Hunk?) -> Int {
            guard let p = previous else { return r }
            return r - (p.rightStart + p.rightCount) + (p.leftStart + p.leftCount)
        }
        let rightStart = min(s, touching.first?.rightStart ?? s)
        let rightEnd = max(e, touching.map { $0.rightStart + $0.rightCount }.max() ?? e)
        let leftStart: Int
        if let first = touching.first, first.rightStart <= s {
            leftStart = first.leftStart
        } else {
            leftStart = level(s, after: before.last)
        }
        let leftEnd: Int
        if let last = touching.last, last.rightStart + last.rightCount >= e {
            leftEnd = last.leftStart + last.leftCount
        } else {
            leftEnd = level(e, after: touching.last ?? before.last)
        }
        let merged = Hunk(leftStart: leftStart, leftCount: max(0, leftEnd - leftStart),
                          rightStart: rightStart, rightCount: max(0, rightEnd - rightStart + delta))
        var result = before
        // Back to how the left side has it: no hunk left at all.
        if merged.leftCount > 0 || merged.rightCount > 0 { result.append(merged) }
        result += after
        return result
    }

    // MARK: Moving through text

    enum CharacterClass { case space, word, punctuation }

    static func characterClass(_ c: unichar) -> CharacterClass {
        if c == 32 || c == 9 { return .space }
        if (c >= 48 && c <= 57) || (c >= 65 && c <= 90) || (c >= 97 && c <= 122) || c == 95 || c >= 0x80 { return .word }
        return .punctuation
    }

    /// Where a word-wise move from `column` lands: past the spaces, then past one run of word or punctuation characters.
    static func wordBoundary(in text: NSString, from column: Int, forward: Bool) -> Int {
        let length = text.length
        var i = min(max(0, column), length)
        if forward {
            while i < length, characterClass(text.character(at: i)) == .space { i += 1 }
            guard i < length else { return i }
            let kind = characterClass(text.character(at: i))
            while i < length, characterClass(text.character(at: i)) == kind { i += 1 }
        } else {
            while i > 0, characterClass(text.character(at: i - 1)) == .space { i -= 1 }
            guard i > 0 else { return i }
            let kind = characterClass(text.character(at: i - 1))
            while i > 0, characterClass(text.character(at: i - 1)) == kind { i -= 1 }
        }
        return i
    }

    /// The column one character (as the user sees characters: an emoji, a letter with its accents) away.
    static func characterBoundary(in text: NSString, from column: Int, forward: Bool) -> Int {
        let length = text.length
        let i = min(max(0, column), length)
        if forward {
            guard i < length else { return i }
            let range = text.rangeOfComposedCharacterSequence(at: i)
            return range.location + range.length
        }
        guard i > 0 else { return 0 }
        return text.rangeOfComposedCharacterSequence(at: i - 1).location
    }

    /// Leading spaces and tabs.
    static func indentation(of line: String) -> String {
        String(line.prefix { $0 == " " || $0 == "\t" })
    }

    /// What one press of Tab inserts in this file: a tab where the file indents with tabs, otherwise spaces.
    /// The width is the step that indentation most often increases by.
    static func indentUnit(for lines: [String]) -> String {
        var tabs = 0, spaces = 0
        var steps: [Int: Int] = [:]
        var previous = 0
        for line in lines.prefix(4000) {
            guard let first = line.utf8.first else { continue }
            if first == 9 { tabs += 1; continue }
            let width = line.utf8.prefix { $0 == 32 }.count
            // Blank lines and lines of only spaces say nothing about the indentation.
            if width == line.utf8.count { continue }
            if width > 0 { spaces += 1 }
            if width > previous, [2, 3, 4, 8].contains(width - previous) { steps[width - previous, default: 0] += 1 }
            previous = width
        }
        if tabs > spaces { return "\t" }
        let step = steps.max { $0.value != $1.value ? $0.value < $1.value : $0.key > $1.key }?.key ?? 4
        return String(repeating: " ", count: step)
    }

    /// Display columns up to a UTF-16 column, with tab stops every four.
    static func visualColumn(of line: NSString, at column: Int) -> Int {
        var visual = 0
        for i in 0..<min(column, line.length) {
            if line.character(at: i) == 9 { visual += 4 - visual % 4 } else { visual += 1 }
        }
        return visual
    }
}
