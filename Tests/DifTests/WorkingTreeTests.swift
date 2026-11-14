import XCTest
@testable import DifKit

final class HunkPatchTests: XCTestCase {
    private func data(_ text: String) -> Data { Data(text.utf8) }

    private func revert(_ hunk: Hunk, old: String, new: String) -> String? {
        HunkPatch.revert(hunk, old: data(old), new: data(new)).map { String(decoding: $0, as: UTF8.self) }
    }

    func testLinesKeepTheirTerminators() {
        let text = data("a\r\nb\n\nlast")
        XCTAssertEqual(HunkPatch.lineRanges(text).map { String(decoding: text.subdata(in: $0), as: UTF8.self) },
                       ["a\r\n", "b\n", "\n", "last"])
        XCTAssertEqual(HunkPatch.lineRanges(Data()), [])
        // The same count as the text shown on screen, whatever the endings.
        for sample in ["", "x", "x\n", "x\n\n", "a\r\nb\r\n", "\n"] {
            XCTAssertEqual(HunkPatch.lineRanges(data(sample)).count, TextCodec.splitLines(sample).count, sample.debugDescription)
        }
    }

    func testAModifiedHunkIsPutBackAndTheRestIsLeftAlone() {
        let old = "one\ntwo\nthree\nfour\n"
        let new = "one\nTWO\nTWO-B\nthree\nFOUR\n"
        // Hunk 0: old line 1 ("two") became new lines 1...2.
        XCTAssertEqual(revert(Hunk(leftStart: 1, leftCount: 1, rightStart: 1, rightCount: 2), old: old, new: new),
                       "one\ntwo\nthree\nFOUR\n")
        // Hunk 1: old line 3 ("four") became new line 4.
        XCTAssertEqual(revert(Hunk(leftStart: 3, leftCount: 1, rightStart: 4, rightCount: 1), old: old, new: new),
                       "one\nTWO\nTWO-B\nthree\nfour\n")
    }

    func testAddedLinesAreRemovedAndDeletedLinesComeBack() {
        XCTAssertEqual(revert(Hunk(leftStart: 1, leftCount: 0, rightStart: 1, rightCount: 2), old: "a\nd\n", new: "a\nb\nc\nd\n"), "a\nd\n")
        XCTAssertEqual(revert(Hunk(leftStart: 1, leftCount: 2, rightStart: 1, rightCount: 0), old: "a\nb\nc\nd\n", new: "a\nd\n"), "a\nb\nc\nd\n")
        // At the very start and the very end.
        XCTAssertEqual(revert(Hunk(leftStart: 0, leftCount: 1, rightStart: 0, rightCount: 0), old: "a\nb\n", new: "b\n"), "a\nb\n")
        XCTAssertEqual(revert(Hunk(leftStart: 2, leftCount: 0, rightStart: 2, rightCount: 1), old: "a\nb\n", new: "a\nb\nc\n"), "a\nb\n")
    }

    func testAMissingFinalNewlineIsRestoredExactly() {
        // git reports the last line as changed when only its newline differs, so the hunk covers it.
        XCTAssertEqual(revert(Hunk(leftStart: 1, leftCount: 1, rightStart: 1, rightCount: 2), old: "a\nb", new: "a\nb\nc\n"), "a\nb")
        XCTAssertEqual(revert(Hunk(leftStart: 1, leftCount: 2, rightStart: 1, rightCount: 1), old: "a\nb\nc\n", new: "a\nb"), "a\nb\nc\n")
    }

    func testPiecesThatStopBeingLastGetTheirNewline() {
        // Whitespace-insensitive diffs can leave a newline-less last line outside the hunk.
        XCTAssertEqual(revert(Hunk(leftStart: 2, leftCount: 1, rightStart: 2, rightCount: 0), old: "a\nb\nc\n", new: "a\nb"), "a\nb\nc\n")
        XCTAssertEqual(revert(Hunk(leftStart: 1, leftCount: 1, rightStart: 0, rightCount: 0), old: "a\nb", new: "x\n"), "b\nx\n")
        XCTAssertEqual(revert(Hunk(leftStart: 1, leftCount: 1, rightStart: 1, rightCount: 0), old: "a\r\nb\r\n", new: "a"), "a\r\nb\r\n")
    }

    func testOtherLinesKeepTheirBytes() {
        // A GB18030 line and a CRLF line outside the hunk come through untouched.
        var new = Data([0xD6, 0xD0, 0xCE, 0xC4, 0x0D, 0x0A])
        new.append(data("changed\nend\n"))
        var old = Data([0xD6, 0xD0, 0xCE, 0xC4, 0x0D, 0x0A])
        old.append(data("original\nend\n"))
        XCTAssertEqual(HunkPatch.revert(Hunk(leftStart: 1, leftCount: 1, rightStart: 1, rightCount: 1), old: old, new: new), old)
    }

    func testAHunkThatDoesNotFitIsRefused() {
        XCTAssertNil(revert(Hunk(leftStart: 3, leftCount: 2, rightStart: 0, rightCount: 1), old: "a\nb\n", new: "c\n"))
        XCTAssertNil(revert(Hunk(leftStart: 0, leftCount: 1, rightStart: 1, rightCount: 4), old: "a\nb\n", new: "c\n"))
        XCTAssertNil(HunkPatch.revert(Hunk(leftStart: 0, leftCount: 1, rightStart: 0, rightCount: 1),
                                      old: Data([0xFF, 0xFE, 0x61, 0x00]), new: Data([0xFF, 0xFE, 0x62, 0x00])))
    }
}

/// Rollback, delete and undo against a real repository in a temporary directory.
final class WorkingTreeTests: XCTestCase {
    private var root: URL!
    private let fm = FileManager.default
    private let cart = (1...20).map { "val line\($0) = \($0)" }.joined(separator: "\n") + "\n"

    override func setUp() async throws {
        root = fm.temporaryDirectory.appendingPathComponent("dif-edit-tests-\(UUID().uuidString)")
        try fm.createDirectory(at: root.appendingPathComponent("src"), withIntermediateDirectories: true)
        try git("init", "-q", "-b", "main")
        try write("src/Cart.kt", cart)
        try write("src/Legacy.kt", "class Legacy\n")
        try write("docs.md", "# Docs\n\nSome text that stays the same.\nMore text.\n")
        try write("run.sh", "#!/bin/sh\necho one\necho two\n")
        try fm.setAttributes([.posixPermissions: 0o755], ofItemAtPath: root.appendingPathComponent("run.sh").path)
        try write("a*.txt", "star\n")
        try write("ab.txt", "plain\n")
        try git("add", "-A")
        try git("commit", "-q", "-m", "Initial import")
    }

    override func tearDown() async throws {
        try? fm.removeItem(at: root)
    }

    private func write(_ path: String, _ text: String) throws {
        try Data(text.utf8).write(to: root.appendingPathComponent(path))
    }

    private func read(_ path: String) -> String? {
        (try? Data(contentsOf: root.appendingPathComponent(path))).map { String(decoding: $0, as: UTF8.self) }
    }

    private func exists(_ path: String) -> Bool { fm.fileExists(atPath: root.appendingPathComponent(path).path) }

    private func git(_ arguments: String...) throws {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: Git.executable)
        process.arguments = ["-c", "user.name=Dif Tests", "-c", "user.email=tests@example.invalid", "-c", "commit.gpgsign=false"] + arguments
        process.currentDirectoryURL = root
        process.standardOutput = FileHandle.nullDevice
        process.standardError = FileHandle.nullDevice
        try process.run()
        process.waitUntilExit()
        XCTAssertEqual(process.terminationStatus, 0, "git \(arguments.joined(separator: " "))")
    }

    private func changes() async throws -> [String: FileChange] {
        Dictionary(uniqueKeysWithValues: try await Git.status(in: root).changes.map { ($0.path, $0) })
    }

    private func edited(_ line: Int...) -> String {
        (1...20).map { line.contains($0) ? "val line\($0) = CHANGED" : "val line\($0) = \($0)" }.joined(separator: "\n") + "\n"
    }

    // MARK: Rollback

    func testRollbackRestoresModifiedAndDeletedFilesStagedOrNot() async throws {
        try write("src/Cart.kt", edited(5))
        try git("add", "src/Cart.kt")
        try write("src/Cart.kt", edited(5, 9))          // staged and then edited again
        try fm.removeItem(at: root.appendingPathComponent("src/Legacy.kt"))
        try git("rm", "-q", "--cached", "docs.md")       // staged deletion, file still on disk
        try fm.removeItem(at: root.appendingPathComponent("docs.md"))
        var now = try await changes()
        XCTAssertEqual(Set(now.keys), ["src/Cart.kt", "src/Legacy.kt", "docs.md"])

        let (record, failure) = try await WorkingTree.rollback(Array(now.values), in: root)
        XCTAssertNil(failure)
        now = try await changes()
        XCTAssertTrue(now.isEmpty, "\(now.keys)")
        XCTAssertEqual(read("src/Cart.kt"), cart)
        XCTAssertEqual(read("src/Legacy.kt"), "class Legacy\n")
        XCTAssertTrue(exists("docs.md"))

        // Undo brings the edits and the deletions back.
        let undone = await WorkingTree.undo(record)
        XCTAssertEqual(undone.skipped, 0)
        XCTAssertEqual(read("src/Cart.kt"), edited(5, 9))
        XCTAssertFalse(exists("src/Legacy.kt"))
        XCTAssertFalse(exists("docs.md"))
        now = try await changes()
        XCTAssertEqual(now["src/Cart.kt"]?.kind, .modified)
        XCTAssertEqual(now["src/Legacy.kt"]?.kind, .deleted)
        XCTAssertEqual(now["docs.md"]?.kind, .deleted)
    }

    func testRollbackOfAnAddedFileOnlyUnstagesIt() async throws {
        try write("src/New.kt", "class New\n")
        try git("add", "src/New.kt")
        try write("src/New.kt", "class New // edited after staging\n")
        try write("loose.txt", "untracked\n")
        var now = try await changes()
        XCTAssertEqual(now["src/New.kt"]?.kind, .added)

        let (record, _) = try await WorkingTree.rollback(Array(now.values), in: root)
        now = try await changes()
        XCTAssertEqual(now["src/New.kt"]?.kind, .untracked)
        XCTAssertEqual(read("src/New.kt"), "class New // edited after staging\n")
        XCTAssertEqual(read("loose.txt"), "untracked\n", "untracked files are never rolled back")

        _ = await WorkingTree.undo(record)
        now = try await changes()
        XCTAssertEqual(now["src/New.kt"]?.kind, .added)
    }

    func testRollbackOfARenameBringsTheOldNameBack() async throws {
        try git("mv", "docs.md", "guide.md")
        try write("guide.md", "# Docs\n\nSome text that stays the same.\nMore text.\nA new last line.\n")
        var now = try await changes()
        XCTAssertEqual(now["guide.md"]?.kind, .renamed)
        XCTAssertEqual(now["guide.md"]?.oldPath, "docs.md")

        let (record, _) = try await WorkingTree.rollback(Array(now.values), in: root)
        now = try await changes()
        XCTAssertTrue(now.isEmpty, "\(now.keys)")
        XCTAssertTrue(exists("docs.md"))
        XCTAssertFalse(exists("guide.md"))

        let undone = await WorkingTree.undo(record)
        XCTAssertEqual(undone.skipped, 0)
        now = try await changes()
        XCTAssertEqual(now["guide.md"]?.kind, .renamed)
        XCTAssertEqual(read("guide.md")?.hasSuffix("A new last line.\n"), true)
        XCTAssertFalse(exists("docs.md"))
    }

    func testPathsAreTakenLiterally() async throws {
        // Rolling back "a*.txt" must not touch "ab.txt", which the name would match as a pattern.
        try write("a*.txt", "star, edited\n")
        try write("ab.txt", "plain, edited\n")
        let now = try await changes()
        _ = try await WorkingTree.rollback([try XCTUnwrap(now["a*.txt"])], in: root)
        XCTAssertEqual(read("a*.txt"), "star\n")
        XCTAssertEqual(read("ab.txt"), "plain, edited\n")
    }

    func testUndoLeavesAFileAloneOnceItHasChangedAgain() async throws {
        try write("src/Cart.kt", edited(5))
        try write("src/Legacy.kt", "class Legacy // edited\n")
        let now = try await changes()
        let (record, _) = try await WorkingTree.rollback(Array(now.values), in: root)
        try write("src/Cart.kt", "something written after the rollback, long enough to differ in size\n")

        let undone = await WorkingTree.undo(record)
        XCTAssertEqual(undone.skipped, 1)
        XCTAssertEqual(read("src/Cart.kt"), "something written after the rollback, long enough to differ in size\n")
        XCTAssertEqual(read("src/Legacy.kt"), "class Legacy // edited\n")
    }

    func testRollbackWithNothingToDoIsAnError() async throws {
        try write("loose.txt", "untracked\n")
        let now = try await changes()
        do {
            _ = try await WorkingTree.rollback(Array(now.values), in: root)
            XCTFail("expected an error")
        } catch {}
        XCTAssertEqual(read("loose.txt"), "untracked\n")
    }

    func testAStaleListNeverUnstagesOrTrashesACommittedFile() async throws {
        // The list said "added"; by the time the user confirms, the file has been committed and edited again.
        try write("src/New.kt", "class New\n")
        try git("add", "src/New.kt")
        let stale = try await changes()
        XCTAssertEqual(stale["src/New.kt"]?.kind, .added)
        try git("commit", "-q", "-m", "Add New")
        try write("src/New.kt", "class New // edited\n")

        // Rollback goes by what HEAD has now: the file is restored, not dropped from the index.
        _ = try await WorkingTree.rollback(Array(stale.values), in: root)
        XCTAssertEqual(read("src/New.kt"), "class New\n")
        var now = try await changes()
        XCTAssertTrue(now.isEmpty, "\(now.keys)")

        // Delete leaves it where it is.
        let (record, failed) = try await WorkingTree.trash(Array(stale.values), in: root)
        XCTAssertTrue(record.entries.isEmpty)
        XCTAssertEqual(failed, ["src/New.kt"])
        XCTAssertTrue(exists("src/New.kt"))
        now = try await changes()
        XCTAssertTrue(now.isEmpty, "\(now.keys)")
    }

    func testAFailureFromGitLeavesEverythingAsItWas() async throws {
        try write("src/Cart.kt", edited(5))
        let now = try await changes()
        // Another git process holds the index, as an agent committing in the terminal would.
        let lock = root.appendingPathComponent(".git/index.lock")
        try Data().write(to: lock)
        let (record, failure) = try await WorkingTree.rollback(Array(now.values), in: root)
        try fm.removeItem(at: lock)
        XCTAssertNotNil(failure)
        XCTAssertEqual(read("src/Cart.kt"), edited(5))
        // Undoing the part that was done (none of it) changes nothing either.
        _ = await WorkingTree.undo(record)
        XCTAssertEqual(read("src/Cart.kt"), edited(5))
    }

    // MARK: Hunks

    private func document(_ path: String) async throws -> (request: DiffRequest, document: DiffDocument) {
        let now = try await changes()
        let change = try XCTUnwrap(now[path])
        let request = DiffRequest(root: root, path: change.path, oldPath: change.oldPath, kind: change.kind, source: .worktree(base: "HEAD"))
        return (request, await DiffLoader.load(request))
    }

    private func revert(_ path: String, hunk index: Int) async throws -> WorkingTree.HunkResult {
        let (request, doc) = try await document(path)
        return try await WorkingTree.revertHunk(request: request, hunk: doc.hunks[index],
                                                expectedLeft: doc.left.lines, expectedRight: doc.right.lines)
    }

    func testRevertingOneHunkLeavesTheOthers() async throws {
        try write("src/Cart.kt", edited(5, 12) + "val extra = true\n")
        let (_, before) = try await document("src/Cart.kt")
        XCTAssertEqual(before.hunks.count, 3)

        guard case .reverted(let record) = try await revert("src/Cart.kt", hunk: 1) else { return XCTFail("stale") }
        XCTAssertEqual(read("src/Cart.kt"), edited(5) + "val extra = true\n")
        let (_, after) = try await document("src/Cart.kt")
        XCTAssertEqual(after.hunks.count, 2)

        let undone = await WorkingTree.undo(record)
        XCTAssertEqual(undone.skipped, 0)
        XCTAssertEqual(read("src/Cart.kt"), edited(5, 12) + "val extra = true\n")
    }

    func testRevertingTheLastHunkMakesTheFileCleanEvenIfItWasStaged() async throws {
        try write("src/Cart.kt", edited(5))
        try git("add", "src/Cart.kt")
        guard case .reverted = try await revert("src/Cart.kt", hunk: 0) else { return XCTFail("stale") }
        XCTAssertEqual(read("src/Cart.kt"), cart)
        let now = try await changes()
        XCTAssertNil(now["src/Cart.kt"], "nothing should be left staged")
    }

    func testRevertingAHunkKeepsTheFileExecutable() async throws {
        try write("run.sh", "#!/bin/sh\necho ONE\necho two\n")
        guard case .reverted = try await revert("run.sh", hunk: 0) else { return XCTFail("stale") }
        XCTAssertEqual(read("run.sh"), "#!/bin/sh\necho one\necho two\n")
        let mode = try fm.attributesOfItem(atPath: root.appendingPathComponent("run.sh").path)[.posixPermissions] as? Int
        XCTAssertEqual(mode, 0o755)
        let now = try await changes()
        XCTAssertNil(now["run.sh"])
    }

    func testAHunkIsNotRevertedOnceTheFileHasChangedUnderTheDiff() async throws {
        try write("src/Cart.kt", edited(5, 12))
        let (request, doc) = try await document("src/Cart.kt")
        // Something else rewrites the file after the diff was drawn: a line goes in above the hunk.
        try write("src/Cart.kt", "// inserted\n" + edited(5, 12))
        let result = try await WorkingTree.revertHunk(request: request, hunk: doc.hunks[1],
                                                      expectedLeft: doc.left.lines, expectedRight: doc.right.lines)
        guard case .stale = result else { return XCTFail("the stale diff was applied") }
        XCTAssertEqual(read("src/Cart.kt"), "// inserted\n" + edited(5, 12))
    }

    func testTwoRevertsUndoInOrder() async throws {
        try write("src/Cart.kt", edited(5, 12))
        guard case .reverted(let first) = try await revert("src/Cart.kt", hunk: 0) else { return XCTFail("stale") }
        guard case .reverted(let second) = try await revert("src/Cart.kt", hunk: 0) else { return XCTFail("stale") }
        XCTAssertEqual(read("src/Cart.kt"), cart)

        // The store carries the fresh stamp down the stack after each undo; do the same here.
        var earlier = first
        let undone = await WorkingTree.undo(second)
        XCTAssertEqual(read("src/Cart.kt"), edited(12))
        earlier.entries[0].after = undone.restored["src/Cart.kt"] ?? nil
        let again = await WorkingTree.undo(earlier)
        XCTAssertEqual(again.skipped, 0)
        XCTAssertEqual(read("src/Cart.kt"), edited(5, 12))
    }

    // MARK: Delete

    func testNewFilesGoToTheTrashAndComeBack() async throws {
        try write("scratch.txt", "untracked\n")
        try write("src/New.kt", "class New\n")
        try git("add", "src/New.kt")
        try write("src/Cart.kt", edited(5))
        var now = try await changes()

        let (record, failed) = try await WorkingTree.trash(Array(now.values), in: root)
        defer { record.entries.compactMap(\.backup).forEach { try? fm.removeItem(at: $0) } }
        XCTAssertEqual(failed, [])
        XCTAssertEqual(Set(record.entries.map(\.path)), ["scratch.txt", "src/New.kt"])
        XCTAssertFalse(exists("scratch.txt"))
        XCTAssertFalse(exists("src/New.kt"))
        XCTAssertEqual(read("src/Cart.kt"), edited(5), "a modified file is not something Delete touches")
        now = try await changes()
        XCTAssertEqual(Set(now.keys), ["src/Cart.kt"])

        let undone = await WorkingTree.undo(record)
        XCTAssertEqual(undone.skipped, 0)
        XCTAssertEqual(read("scratch.txt"), "untracked\n")
        now = try await changes()
        XCTAssertEqual(now["scratch.txt"]?.kind, .untracked)
        XCTAssertEqual(now["src/New.kt"]?.kind, .added)
    }
}
