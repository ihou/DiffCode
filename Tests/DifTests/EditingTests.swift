import XCTest
@testable import DifKit

final class LosslessTextTests: XCTestCase {
    private func roundTrip(_ data: Data, file: StaticString = #filePath, line: UInt = #line) -> EditableText? {
        guard let text = TextCodec.decodeForEditing(data) else { return nil }
        XCTAssertEqual(TextCodec.encode(text.lines, format: text.format, crlf: text.crlf), data, file: file, line: line)
        // The lines are the ones the viewer shows.
        if !data.isEmpty, text.format.encoding == .utf8, !text.format.bom {
            XCTAssertEqual(text.lines, TextCodec.splitLines(String(decoding: data, as: UTF8.self)), file: file, line: line)
        }
        return text
    }

    func testLineEndingsAndTheLastLineBreakAreKept() throws {
        let lf = try XCTUnwrap(roundTrip(Data("a\nb\n".utf8)))
        XCTAssertEqual(lf.lines, ["a", "b"])
        XCTAssertTrue(lf.format.finalNewline)
        XCTAssertNil(lf.crlf)

        let open = try XCTUnwrap(roundTrip(Data("a\nb".utf8)))
        XCTAssertFalse(open.format.finalNewline)

        let windows = try XCTUnwrap(roundTrip(Data("a\r\nb\r\n".utf8)))
        XCTAssertEqual(windows.format.newline, "\r\n")
        XCTAssertNil(windows.crlf)

        let mixed = try XCTUnwrap(roundTrip(Data("a\r\nb\nc\r\nd".utf8)))
        XCTAssertEqual(mixed.crlf, [true, false, true, false])
        XCTAssertEqual(mixed.format.newline, "\r\n")
    }

    func testAnEmptyFileHasOneLineToTypeOn() throws {
        let empty = try XCTUnwrap(roundTrip(Data()))
        XCTAssertEqual(empty.lines, [""])
        XCTAssertFalse(empty.format.finalNewline)
        let newline = try XCTUnwrap(roundTrip(Data("\n".utf8)))
        XCTAssertEqual(newline.lines, [""])
        XCTAssertTrue(newline.format.finalNewline)
    }

    func testByteOrderMarksAndOtherEncodingsComeBackTheSame() throws {
        let bom = try XCTUnwrap(roundTrip(Data([0xEF, 0xBB, 0xBF]) + Data("x = 1\n".utf8)))
        XCTAssertTrue(bom.format.bom)
        XCTAssertEqual(bom.lines, ["x = 1"])

        let utf16 = try XCTUnwrap(roundTrip(Data([0xFF, 0xFE]) + "中\r\nb".data(using: .utf16LittleEndian)!))
        XCTAssertEqual(utf16.format.encoding, .utf16LittleEndian)
        XCTAssertEqual(utf16.lines, ["中", "b"])

        let legacy = try XCTUnwrap("// 中文注释\nint a;\n".data(using: TextCodec.gb18030))
        let gb = try XCTUnwrap(roundTrip(legacy))
        XCTAssertEqual(gb.format.encoding, .gb18030)
        XCTAssertEqual(gb.lines, ["// 中文注释", "int a;"])
    }

    func testWhatCannotBeWrittenBackExactlyStaysReadOnly() {
        // A carriage return that is not half of "\r\n".
        XCTAssertNil(TextCodec.decodeForEditing(Data("a\rb\n".utf8)))
        XCTAssertNil(TextCodec.decodeForEditing(Data("a\r".utf8)))
        // Bytes no encoding we try accepts.
        XCTAssertNil(TextCodec.decodeForEditing(Data([0x61, 0xFF, 0xFF, 0x62])))
    }
}

final class TextEditingTests: XCTestCase {
    private func p(_ row: Int, _ column: Int) -> TextPosition { TextPosition(row: row, column: column) }

    func testTypingInsideALineRewritesOnlyThatLine() {
        var lines = ["let a = 1", "let b = 2"]
        let result = TextEditing.replace(&lines, from: p(0, 4), to: p(0, 5), with: "alpha")
        XCTAssertEqual(lines, ["let alpha = 1", "let b = 2"])
        XCTAssertEqual(result.end, p(0, 9))
        XCTAssertEqual(result.change, LineEdit(start: 0, removed: 1, inserted: 1))
    }

    func testALineBreakAtTheEndOfALineOnlyAddsALine() {
        var lines = ["one", "two"]
        let result = TextEditing.replace(&lines, from: p(0, 3), to: p(0, 3), with: "\n")
        XCTAssertEqual(lines, ["one", "", "two"])
        XCTAssertEqual(result.end, p(1, 0))
        XCTAssertEqual(result.raw, LineEdit(start: 0, removed: 1, inserted: 2))
        XCTAssertEqual(result.change, LineEdit(start: 1, removed: 0, inserted: 1))
    }

    func testJoiningAndReplacingAcrossLines() {
        var lines = ["one", "two", "three"]
        var result = TextEditing.replace(&lines, from: p(0, 3), to: p(1, 0), with: "")
        XCTAssertEqual(lines, ["onetwo", "three"])
        XCTAssertEqual(result.change, LineEdit(start: 0, removed: 2, inserted: 1))
        result = TextEditing.replace(&lines, from: p(0, 1), to: p(1, 2), with: "X\r\nY")
        XCTAssertEqual(lines, ["oX", "Yree"])
        XCTAssertEqual(result.end, p(1, 1))
        XCTAssertEqual(TextEditing.text(lines, from: p(0, 1), to: p(1, 1)), "X\nY")
    }

    func testHunksBelowAnAddedLineMoveDownAndTheLineBecomesAnAddition() {
        let hunks = [Hunk(leftStart: 5, leftCount: 1, rightStart: 5, rightCount: 1)]
        let adjusted = TextEditing.adjust(hunks, for: LineEdit(start: 2, removed: 0, inserted: 1))
        XCTAssertEqual(adjusted, [Hunk(leftStart: 2, leftCount: 0, rightStart: 2, rightCount: 1),
                                  Hunk(leftStart: 5, leftCount: 1, rightStart: 6, rightCount: 1)])
    }

    func testAnEditInsideAHunkStaysThatHunk() {
        let hunks = [Hunk(leftStart: 2, leftCount: 3, rightStart: 4, rightCount: 2)]
        XCTAssertEqual(TextEditing.adjust(hunks, for: LineEdit(start: 5, removed: 1, inserted: 1)),
                       [Hunk(leftStart: 2, leftCount: 3, rightStart: 4, rightCount: 2)])
        XCTAssertEqual(TextEditing.adjust(hunks, for: LineEdit(start: 5, removed: 0, inserted: 2)),
                       [Hunk(leftStart: 2, leftCount: 3, rightStart: 4, rightCount: 4)])
    }

    func testRemovingTheOnlyAddedLineLeavesNoHunk() {
        let hunks = [Hunk(leftStart: 3, leftCount: 0, rightStart: 3, rightCount: 1),
                     Hunk(leftStart: 8, leftCount: 1, rightStart: 9, rightCount: 1)]
        XCTAssertEqual(TextEditing.adjust(hunks, for: LineEdit(start: 3, removed: 1, inserted: 0)),
                       [Hunk(leftStart: 8, leftCount: 1, rightStart: 8, rightCount: 1)])
    }

    func testTypingWhereLinesWereDeletedMakesAModification() {
        let hunks = [Hunk(leftStart: 3, leftCount: 2, rightStart: 3, rightCount: 0)]
        XCTAssertEqual(TextEditing.adjust(hunks, for: LineEdit(start: 3, removed: 0, inserted: 1)),
                       [Hunk(leftStart: 3, leftCount: 2, rightStart: 3, rightCount: 1)])
        // A change in the unchanged lines after it lines up past the deleted ones.
        XCTAssertEqual(TextEditing.adjust(hunks, for: LineEdit(start: 6, removed: 1, inserted: 1)),
                       [hunks[0], Hunk(leftStart: 8, leftCount: 1, rightStart: 6, rightCount: 1)])
    }

    func testWordAndCharacterSteps() {
        let text = "foo.barBaz  qux 👍🏽" as NSString
        XCTAssertEqual(TextEditing.wordBoundary(in: text, from: 0, forward: true), 3)
        XCTAssertEqual(TextEditing.wordBoundary(in: text, from: 4, forward: true), 10)
        XCTAssertEqual(TextEditing.wordBoundary(in: text, from: 12, forward: false), 4)
        // An emoji with a skin tone is one character of four UTF-16 units.
        XCTAssertEqual(TextEditing.characterBoundary(in: text, from: text.length, forward: false), text.length - 4)
    }

    func testTheIndentUnitFollowsTheFile() {
        XCTAssertEqual(TextEditing.indentUnit(for: ["fun a() {", "\tb()", "\t\tc()", "}"]), "\t")
        XCTAssertEqual(TextEditing.indentUnit(for: ["a:", "  b:", "    c: 1", "  d: 2"]), "  ")
        XCTAssertEqual(TextEditing.indentUnit(for: ["x"]), "    ")
    }
}

final class HighlightingTests: XCTestCase {
    func testOpeningACommentRecoloursTheLinesAfterItUntilItCloses() {
        let language = LanguageSpec.detect(path: "a.swift")
        let buffer = TextBuffer(lines: ["let a = 1", "let b = 2", "let c = 3", "let d = 4"])
        let colours = Highlighter.highlightTracking(buffer.lines, language: language)
        buffer.spans = colours.spans
        buffer.states = colours.states

        var raw = buffer.replace(from: TextPosition(row: 0, column: 0), to: TextPosition(row: 0, column: 0), with: "/* ").raw
        var range = Highlighter.rehighlight(buffer.lines, language: language, from: raw.start, count: raw.inserted,
                                            spans: &buffer.spans, states: &buffer.states!)
        XCTAssertEqual(range, 0..<4)
        XCTAssertEqual(buffer.spans[2].map(\.kind), [.comment])

        raw = buffer.replace(from: TextPosition(row: 1, column: 9), to: TextPosition(row: 1, column: 9), with: " */").raw
        range = Highlighter.rehighlight(buffer.lines, language: language, from: raw.start, count: raw.inserted,
                                        spans: &buffer.spans, states: &buffer.states!)
        XCTAssertEqual(range, 1..<4)
        XCTAssertEqual(buffer.spans[2], Highlighter.highlight(buffer.lines, language: language)[2])
        XCTAssertEqual(buffer.spans, Highlighter.highlight(buffer.lines, language: language))

        // A change that leaves the state alone recolours just its line.
        raw = buffer.replace(from: TextPosition(row: 3, column: 4), to: TextPosition(row: 3, column: 5), with: "e").raw
        range = Highlighter.rehighlight(buffer.lines, language: language, from: raw.start, count: raw.inserted,
                                        spans: &buffer.spans, states: &buffer.states!)
        XCTAssertEqual(range, 3..<4)
    }
}

final class EditorSessionTests: XCTestCase {
    private func p(_ row: Int, _ column: Int) -> TextPosition { TextPosition(row: row, column: column) }

    private func session(_ text: String, base: String? = nil) throws -> EditorSession {
        let data = Data(text.utf8)
        let editable = try XCTUnwrap(TextCodec.decodeForEditing(data))
        let right = TextBuffer(lines: editable.lines)
        let colours = Highlighter.highlightTracking(editable.lines, language: .plain)
        right.spans = colours.spans
        right.states = colours.states
        let left = base.map { TextBuffer(lines: TextCodec.splitLines($0)) } ?? .empty
        let doc = DiffDocument(left: left, right: right, hunks: [], language: .plain, isPlainFile: base == nil)
        doc.editing = EditInfo(format: editable.format, crlf: editable.crlf, fingerprint: TextCodec.fingerprint(data),
                               base: base.map { Data($0.utf8) })
        return try XCTUnwrap(EditorSession(document: doc, root: URL(fileURLWithPath: "/tmp"), path: "a.txt", ignoreWhitespace: false))
    }

    private func type(_ session: EditorSession, _ text: String, at position: TextPosition) -> TextPosition {
        session.replace(from: position, to: position, with: text, kind: .typing, selection: (position, position)).end
    }

    func testTypingIsOneUndoStepAndRedoBringsItBack() throws {
        let s = try session("hello\nworld\n")
        var caret = p(0, 5)
        for ch in [",", " ", "y", "o", "u"] { caret = type(s, ch, at: caret) }
        XCTAssertEqual(s.buffer.lines, ["hello, you", "world"])
        XCTAssertTrue(s.isDirty)
        let undone = try XCTUnwrap(s.undo())
        XCTAssertEqual(s.buffer.lines, ["hello", "world"])
        XCTAssertEqual(undone.selection.head, p(0, 5))
        XCTAssertNil(s.undo())
        _ = try XCTUnwrap(s.redo())
        XCTAssertEqual(s.buffer.lines, ["hello, you", "world"])
    }

    func testMovingTheCaretStartsANewUndoStep() throws {
        let s = try session("ab\n")
        _ = type(s, "1", at: p(0, 2))
        s.closeGroup()
        _ = type(s, "2", at: p(0, 0))
        s.undo()
        XCTAssertEqual(s.buffer.lines, ["ab1"])
        s.undo()
        XCTAssertEqual(s.buffer.lines, ["ab"])
    }

    func testMixedLineEndingsAreKeptWhereTheTextWasNotTouched() throws {
        let data = Data("a\r\nb\nc\r\n".utf8)
        let editable = try XCTUnwrap(TextCodec.decodeForEditing(data))
        let right = TextBuffer(lines: editable.lines)
        let doc = DiffDocument(left: .empty, right: right, hunks: [], language: .plain, isPlainFile: true)
        doc.editing = EditInfo(format: editable.format, crlf: editable.crlf, fingerprint: TextCodec.fingerprint(data))
        let s = try XCTUnwrap(EditorSession(document: doc, root: URL(fileURLWithPath: "/tmp"), path: "x", ignoreWhitespace: false))
        // A new line after "b": the file's usual ending (\r\n) for the break typed, "b"'s own \n kept after it.
        _ = s.replace(from: p(1, 1), to: p(1, 1), with: "\nB", kind: .typing, selection: (p(1, 1), p(1, 1)))
        XCTAssertEqual(String(decoding: try XCTUnwrap(s.encoded()), as: UTF8.self), "a\r\nb\r\nB\nc\r\n")
        s.undo()
        XCTAssertEqual(try XCTUnwrap(s.encoded()), data)
    }

    func testTheDiffFollowsTheTextUntilItIsRecomputed() throws {
        let s = try session("one\ntwo\nthree\n", base: "one\ntwo\nthree\n")
        _ = s.replace(from: p(1, 3), to: p(1, 3), with: "\nnew", kind: .typing, selection: (p(1, 3), p(1, 3)))
        XCTAssertEqual(s.document.hunks, [Hunk(leftStart: 2, leftCount: 0, rightStart: 2, rightCount: 1)])
        s.undo()
        XCTAssertEqual(s.document.hunks, [])
    }

    func testCompositionIsShownButNeitherSavedNorRecorded() throws {
        let s = try session("ab\n")
        _ = s.setMarked("ni", at: p(0, 1))
        XCTAssertEqual(s.buffer.lines, ["anib"])
        XCTAssertNil(s.encoded())
        _ = s.setMarked("你", at: p(0, 1))
        XCTAssertEqual(s.buffer.lines, ["a你b"])
        let removed = try XCTUnwrap(s.removeMarked())
        XCTAssertEqual(removed.text, "你")
        XCTAssertEqual(s.buffer.lines, ["ab"])
        XCTAssertFalse(s.canUndo)
        XCTAssertEqual(s.encoded(), Data("ab\n".utf8))
    }

    func testOffsetsCountLineBreaksAsOneUnit() throws {
        let s = try session("ab\n\ncdé\n")
        XCTAssertEqual(s.offset(of: p(2, 1)), 5)
        XCTAssertEqual(s.position(ofOffset: 5), p(2, 1))
        XCTAssertEqual(s.position(ofOffset: 3), p(1, 0))
        XCTAssertEqual(s.totalLength, 7)
        _ = s.replace(from: p(0, 0), to: p(0, 0), with: "x\n", kind: .other, selection: (p(0, 0), p(0, 0)))
        XCTAssertEqual(s.position(ofOffset: 7), p(3, 1))
    }

    func testTakingTheDiskVersionKeepsWhatWasTypedOneUndoAway() throws {
        let s = try session("mine\n")
        _ = type(s, "!", at: p(0, 4))
        let theirs = Data("theirs\nmore\n".utf8)
        _ = s.takeDiskVersion(try XCTUnwrap(TextCodec.decodeForEditing(theirs)), data: theirs, selection: (p(0, 5), p(0, 5)))
        XCTAssertEqual(s.buffer.lines, ["theirs", "more"])
        XCTAssertFalse(s.isDirty)
        s.undo()
        XCTAssertEqual(s.buffer.lines, ["mine!"])
        XCTAssertTrue(s.isDirty)
    }

    func testAHistoryIsHandedOnOnlyToTheTextItEndsWith() throws {
        let s = try session("a\n")
        _ = type(s, "b", at: p(0, 1))
        XCTAssertNil(s.history, "unsaved typing is not a history to hand on")
        s.didWrite(try XCTUnwrap(s.encoded()))
        let history = try XCTUnwrap(s.history)

        let reopened = try session("ab\n")
        XCTAssertTrue(reopened.adopt(history))
        XCTAssertTrue(reopened.canUndo)
        reopened.undo()
        XCTAssertEqual(reopened.buffer.lines, ["a"])
        XCTAssertTrue(reopened.isDirty)

        // Someone else changed the file in between: their text, not ours, so no undoing into it.
        XCTAssertFalse(try session("ab!\n").adopt(history))
        // A session already typed in keeps its own history.
        let typedIn = try session("ab\n")
        _ = type(typedIn, "c", at: p(0, 2))
        typedIn.didWrite(try XCTUnwrap(typedIn.encoded()))
        XCTAssertFalse(typedIn.adopt(history))
    }

    func testTypingThatCouldNotBeWrittenComesBackWithItsHistory() throws {
        let s = try session("x\r\ny\nz\r\n")
        _ = type(s, "1", at: p(1, 1))
        s.didWrite(try XCTUnwrap(s.encoded()))
        s.closeGroup()
        _ = type(s, "2", at: p(1, 2))
        let unsaved = try XCTUnwrap(s.unsaved)

        let reopened = try session("x\r\ny1\nz\r\n")
        let result = reopened.resume(unsaved, selection: (p(0, 0), p(0, 0)))
        XCTAssertFalse(result.changedSince)
        XCTAssertEqual(reopened.buffer.lines, ["x", "y12", "z"])
        XCTAssertTrue(reopened.isDirty)
        // Every line keeps its own line ending.
        XCTAssertEqual(reopened.encoded(), Data("x\r\ny12\nz\r\n".utf8))
        reopened.undo()
        XCTAssertEqual(reopened.buffer.lines, ["x", "y1", "z"])
        reopened.undo()
        XCTAssertEqual(reopened.encoded(), Data("x\r\ny\nz\r\n".utf8))
    }

    func testTypingThatCouldNotBeWrittenStaysOneUndoAwayFromAFileChangedSince() throws {
        let s = try session("mine\n")
        _ = type(s, "!", at: p(0, 4))
        let unsaved = try XCTUnwrap(s.unsaved)
        XCTAssertNil(try session("mine\n").unsaved, "nothing typed, nothing to keep")

        let reopened = try session("theirs\n")
        XCTAssertTrue(reopened.resume(unsaved, selection: (p(0, 0), p(0, 0))).changedSince)
        XCTAssertEqual(reopened.buffer.lines, ["theirs"])
        XCTAssertFalse(reopened.isDirty)
        XCTAssertEqual(reopened.encoded(), Data("theirs\n".utf8))
        reopened.undo()
        XCTAssertEqual(reopened.buffer.lines, ["mine!"])
        XCTAssertTrue(reopened.isDirty)
        reopened.undo()
        XCTAssertEqual(reopened.buffer.lines, ["mine"])
    }
}

final class WorkingTreeDiffTests: XCTestCase {
    func testUTF16TextGetsTheSameHunksAsUTF8() async throws {
        func utf16(_ text: String) -> Data { Data([0xFF, 0xFE]) + text.data(using: .utf16LittleEndian)! }
        let hunks = await DiffLoader.computeHunks(old: utf16("a\nb\nc\n"), new: utf16("a\nB\nc\nd\n"), ignoreWhitespace: false)
        XCTAssertEqual(hunks, [Hunk(leftStart: 1, leftCount: 1, rightStart: 1, rightCount: 1),
                               Hunk(leftStart: 3, leftCount: 0, rightStart: 3, rightCount: 1)])
        let same = await DiffLoader.computeHunks(old: utf16("x\n"), new: Data("x\n".utf8), ignoreWhitespace: false)
        XCTAssertEqual(same, [], "the same lines in another encoding are not a change of lines")
    }

    /// Issue #4: the user's own gitattributes apply to the scratch files git compares. One that called them binary
    /// left no hunks, shown as "Contents are identical".
    func testTheUsersGitattributesDoNotHideChanges() async throws {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("dif-attributes-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: dir) }
        func settings(_ name: String, attributes: String, config: String = "") throws -> [String: String] {
            let attributesFile = dir.appendingPathComponent("\(name).attributes"), configFile = dir.appendingPathComponent("\(name).config")
            try Data(attributes.utf8).write(to: attributesFile)
            try Data("[core]\n\tattributesFile = \(attributesFile.path)\n\(config)".utf8).write(to: configFile)
            return ["GIT_CONFIG_GLOBAL": configFile.path]
        }
        let old = Data("fdsf\n".utf8), new = Data("fdsf1111\ndfxxs\n".utf8)
        let expected = [Hunk(leftStart: 0, leftCount: 1, rightStart: 0, rightCount: 2)]

        let binary = try settings("binary", attributes: "*.a binary\n")
        let hunks = await DiffLoader.computeHunks(old: old, new: new, ignoreWhitespace: false, environment: binary)
        XCTAssertEqual(hunks, expected)

        // A textconv driver that turns everything into nothing would make any two files the same.
        let textconv = try settings("textconv", attributes: "* diff=nothing\n", config: "[diff \"nothing\"]\n\ttextconv = true\n")
        let converted = await DiffLoader.computeHunks(old: old, new: new, ignoreWhitespace: false, environment: textconv)
        XCTAssertEqual(converted, expected)
    }

    func testDifferentBytesAreNotCalledIdentical() {
        let identical = DiffLoader.note(hunks: [], old: Data("a\n".utf8), new: Data("a\n".utf8), ignoreWhitespace: false)
        let lineEndings = DiffLoader.note(hunks: [], old: Data("a\n".utf8), new: Data("a\r\n".utf8), ignoreWhitespace: false)
        XCTAssertNotNil(identical)
        XCTAssertNotNil(lineEndings)
        XCTAssertNotEqual(lineEndings, identical)
        XCTAssertNil(DiffLoader.note(hunks: [Hunk(leftStart: 0, leftCount: 1, rightStart: 0, rightCount: 1)],
                                     old: Data("a\n".utf8), new: Data("b\n".utf8), ignoreWhitespace: false))
    }
}
