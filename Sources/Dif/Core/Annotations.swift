import Foundation

/// What the Annotate column shows: for each line of the text on screen, the commit that last changed it.
struct LineAnnotations: Equatable {
    struct Entry: Equatable {
        var hash: String
        var author: String
        var email: String
        var date: Date
        var summary: String
        /// "2026-11-13 Alice", cut to fit the column.
        var label: String
        /// 0 for the oldest commit in the file, 1 for the newest.
        var recency: Double

        var shortHash: String { String(hash.prefix(7)) }
    }

    var entries: [Entry]
    /// The entry of each line, in order; -1 for a line that is not committed yet.
    var lines: [Int32]
    /// Width of the column, in character cells.
    var columns: Int

    /// Longer names are cut, so one long name does not widen the column for everyone.
    static let maxAuthorColumns = 12

    init(entries: [Entry], lines: [Int32], columns: Int) {
        self.entries = entries
        self.lines = lines
        self.columns = columns
    }

    /// - Parameter lineCount: the lines the text on screen has; the blame is cut or padded to match.
    init(blame: Blame, lineCount: Int) {
        var lines = blame.lines
        if lines.count < lineCount {
            lines += repeatElement(-1, count: lineCount - lines.count)
        } else if lines.count > lineCount {
            lines.removeLast(lines.count - lineCount)
        }
        // Recency by rank rather than by date, so a file with one ancient commit still shades its recent ones apart.
        let order = blame.entries.indices.sorted { blame.entries[$0].date < blame.entries[$1].date }
        var recency = [Double](repeating: 1, count: blame.entries.count)
        if order.count > 1 {
            for (position, index) in order.enumerated() { recency[index] = Double(position) / Double(order.count - 1) }
        }
        let names = blame.entries.map { Self.cut($0.author.isEmpty ? "?" : $0.author, to: Self.maxAuthorColumns) }
        let widest = names.reduce(0) { max($0, TextBuffer.columns(of: $1)) }
        let entries = blame.entries.indices.map { index -> Entry in
            let entry = blame.entries[index]
            return Entry(hash: entry.hash, author: entry.author, email: entry.email, date: entry.date, summary: entry.summary,
                         label: Self.dateText(entry.date) + " " + names[index], recency: recency[index])
        }
        self.init(entries: entries, lines: lines, columns: 11 + max(4, widest))
    }

    /// A column of the given width with nothing in it yet: holds the text in place while the blame runs.
    static func placeholder(columns: Int, lineCount: Int) -> LineAnnotations {
        LineAnnotations(entries: [], lines: [Int32](repeating: -1, count: lineCount), columns: columns)
    }

    var isPlaceholder: Bool { entries.isEmpty }

    func index(atLine line: Int) -> Int32? {
        guard line >= 0, line < lines.count, lines[line] >= 0, Int(lines[line]) < entries.count else { return nil }
        return lines[line]
    }

    func entry(atLine line: Int) -> Entry? {
        index(atLine: line).map { entries[Int($0)] }
    }

    /// The first line of a run of lines from the same commit, where its label goes.
    func isRunStart(_ line: Int) -> Bool {
        line <= 0 || line >= lines.count || lines[line - 1] != lines[line]
    }

    /// Follows an edit of the text: lines that were rewritten are not committed any more, the rest keep their commit.
    mutating func apply(_ edit: LineEdit) {
        let start = min(max(0, edit.start), lines.count)
        let end = min(start + max(0, edit.removed), lines.count)
        lines.replaceSubrange(start..<end, with: repeatElement(-1, count: max(0, edit.inserted)))
    }

    private static let dateFormatter: DateFormatter = {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.dateFormat = "yyyy-MM-dd"
        return formatter
    }()

    static func dateText(_ date: Date) -> String { dateFormatter.string(from: date) }

    /// `text` cut to at most `limit` character cells as `TextBuffer.columns` counts them, with "…" when cut.
    static func cut(_ text: String, to limit: Int) -> String {
        guard TextBuffer.columns(of: text) > limit else { return text }
        let room = limit - TextBuffer.columns(of: "…")
        var result = ""
        var used = 0
        for character in text {
            let width = TextBuffer.columns(of: String(character))
            if used + width > room { break }
            result.append(character)
            used += width
        }
        return result + "…"
    }
}
