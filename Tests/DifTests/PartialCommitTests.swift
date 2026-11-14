import XCTest
@testable import DifKit

/// Committing part of a file, and the pieces of branch and merge handling that need no repository.
final class PartialContentTests: XCTestCase {
    private func data(_ text: String) -> Data { Data(text.utf8) }
    private func text(_ data: Data?) -> String? { data.map { String(decoding: $0, as: UTF8.self) } }
    private func removed(_ line: Int, _ text: String) -> ChangeLine { ChangeLine(removed: true, line: line, text: text) }
    private func added(_ line: Int, _ text: String) -> ChangeLine { ChangeLine(removed: false, line: line, text: text) }

    func testAChangeLeftOutKeepsHEADsLinesAndTheOthersGoIn() {
        let old = data("a\nb\nc\nd\n"), new = data("A\nb\nc\nD\n")
        let hunks = [Hunk(leftStart: 0, leftCount: 1, rightStart: 0, rightCount: 1), Hunk(leftStart: 3, leftCount: 1, rightStart: 3, rightCount: 1)]
        let result = WorkingTree.partialContent(old: old, new: new, hunks: hunks, excluded: [removed(3, "d"), added(3, "D")])
        XCTAssertEqual(text(result), "A\nb\nc\nd\n")
        XCTAssertEqual(text(WorkingTree.partialContent(old: old, new: new, hunks: hunks, excluded: [])), "A\nb\nc\nD\n")
    }

    func testAModifiedLineLeftOutKeepsItsPlaceInsideItsChange() {
        // Two lines rewritten in one change; only the second rewrite stays out.
        let old = data("x\none\ntwo\ny\n"), new = data("x\nONE\nTWO\ny\n")
        let hunks = [Hunk(leftStart: 1, leftCount: 2, rightStart: 1, rightCount: 2)]
        let result = WorkingTree.partialContent(old: old, new: new, hunks: hunks, excluded: [removed(2, "two"), added(2, "TWO")])
        XCTAssertEqual(text(result), "x\nONE\ntwo\ny\n")
    }

    func testAddedLinesLeftOutOfANewFileAndRemovedLinesKeptInADeletedOne() {
        let lines = data("n1\nn2\nn3\n")
        let added3 = [Hunk(leftStart: 0, leftCount: 0, rightStart: 0, rightCount: 3)]
        XCTAssertEqual(text(WorkingTree.partialContent(old: Data(), new: lines, hunks: added3, excluded: [added(1, "n2")])), "n1\nn3\n")
        let removed3 = [Hunk(leftStart: 0, leftCount: 3, rightStart: 0, rightCount: 0)]
        XCTAssertEqual(text(WorkingTree.partialContent(old: lines, new: Data(), hunks: removed3, excluded: [removed(1, "n2")])), "n2\n")
    }

    func testLineEndingsAndAMissingFinalLineBreakAreKept() {
        // CRLF lines are copied as they are.
        let old = data("a\r\nb\r\n"), new = data("a\r\nB\r\nc\r\n")
        let hunks = [Hunk(leftStart: 1, leftCount: 1, rightStart: 1, rightCount: 2)]
        XCTAssertEqual(text(WorkingTree.partialContent(old: old, new: new, hunks: hunks, excluded: [added(2, "c")])), "a\r\nB\r\n")
        // HEAD's last line had no line break: kept, and no longer last, it gets one.
        let short = data("a\nb"), longer = data("a\nb\nc\n")
        let tail = [Hunk(leftStart: 1, leftCount: 1, rightStart: 1, rightCount: 2)]
        XCTAssertEqual(text(WorkingTree.partialContent(old: short, new: longer, hunks: tail, excluded: [removed(1, "b"), added(1, "b")])),
                       "a\nb\nc\n")
        XCTAssertEqual(text(WorkingTree.partialContent(old: short, new: longer, hunks: tail,
                                                       excluded: [removed(1, "b"), added(1, "b"), added(2, "c")])), "a\nb")
    }

    func testALineThatNoLongerHoldsItsTextOrHunksThatDoNotFitRefuse() {
        let old = data("a\nb\n"), new = data("a\nB\n")
        let hunks = [Hunk(leftStart: 1, leftCount: 1, rightStart: 1, rightCount: 1)]
        XCTAssertNil(WorkingTree.partialContent(old: old, new: new, hunks: hunks, excluded: [added(1, "something else")]))
        XCTAssertNil(WorkingTree.partialContent(old: old, new: new, hunks: [Hunk(leftStart: 1, leftCount: 1, rightStart: 0, rightCount: 1)],
                                                excluded: []))
        // UTF-16 is refused: its line breaks are not single bytes.
        XCTAssertNil(WorkingTree.partialContent(old: Data([0xFF, 0xFE, 0x61, 0x00, 0x0A, 0x00]), new: new, hunks: hunks, excluded: []))
    }

    func testPickedLinesFollowTheirTextWhenTheFileIsEditedAbove() {
        let old = ["a", "b", "c"]
        // "top" typed above, and "C" (the excluded rewrite of "c") moved down a line.
        let new = ["top", "a", "b", "C"]
        let hunks = [Hunk(leftStart: 0, leftCount: 0, rightStart: 0, rightCount: 1), Hunk(leftStart: 2, leftCount: 1, rightStart: 3, rightCount: 1)]
        let moved = WorkingTree.relocate([removed(2, "c"), added(2, "C")], old: old, new: new, hunks: hunks)
        XCTAssertEqual(moved.lost, 0)
        XCTAssertEqual(moved.lines, [removed(2, "c"), added(3, "C")])

        // An excluded line that was rewritten is lost; a removed line put back is simply no longer a change.
        let rewritten = WorkingTree.relocate([added(2, "C")], old: old, new: ["a", "b", "D"],
                                             hunks: [Hunk(leftStart: 2, leftCount: 1, rightStart: 2, rightCount: 1)])
        XCTAssertEqual(rewritten.lost, 1)
        let putBack = WorkingTree.relocate([removed(2, "c")], old: old, new: old, hunks: [])
        XCTAssertEqual(putBack.lost, 0)
        XCTAssertTrue(putBack.lines.isEmpty)
        // HEAD changed under a removed line: it cannot be trusted.
        XCTAssertEqual(WorkingTree.relocate([removed(1, "x")], old: old, new: new, hunks: hunks).lost, 1)
    }

    func testABlankLineIsLookedForAmongTheChangesNotTheLinesAroundThem() {
        // The excluded blank line was added at index 2; a line typed at the top moves it to 3, and index 2 is now an
        // unchanged blank line, which must not be taken for it.
        let old = ["a", "", "b"]
        let new = ["top", "a", "", "", "b"]
        let hunks = [Hunk(leftStart: 0, leftCount: 0, rightStart: 0, rightCount: 1), Hunk(leftStart: 2, leftCount: 0, rightStart: 3, rightCount: 1)]
        let moved = WorkingTree.relocate([added(2, "")], old: old, new: new, hunks: hunks)
        XCTAssertEqual(moved.lines, [added(3, "")])
    }

    // MARK: Branches, checkouts and conflicts

    func testBranchNamesAreCheckedTheWayGitChecksThem() {
        XCTAssertNil(Git.branchNameProblem("feature/login-2", existing: []))
        XCTAssertNotNil(Git.branchNameProblem("main", existing: ["main"]))
        for bad in ["", " ", "-x", "a..b", "a b", "a~1", "x.lock", "x/", ".hidden", "a/.b", "@", "HEAD", "a@{1}", "a:b", "a\\b"] {
            XCTAssertNotNil(Git.branchNameProblem(bad, existing: []), bad)
        }
    }

    func testTheFilesACheckoutWouldOverwriteAreReadFromGitsMessage() {
        let message = """
        error: Your local changes to the following files would be overwritten by checkout:
        \ta.txt
        \tsrc/Main.kt
        Please commit your changes or stash them before you switch branches.
        Aborting
        """
        XCTAssertEqual(Git.overwrittenFiles(message), ["a.txt", "src/Main.kt"])
        XCTAssertEqual(Git.overwrittenFiles("fatal: invalid reference: nope"), [])
    }

    func testConflictMarkersAreFoundOnlyAtTheStartOfALine() {
        XCTAssertTrue(Git.hasConflictMarkers(data("a\n<<<<<<< HEAD\nb\n=======\nc\n>>>>>>> topic\n")))
        XCTAssertFalse(Git.hasConflictMarkers(data("let arrow = \"<<<<<<< not a marker\"\n=======\n")))
        XCTAssertFalse(Git.hasConflictMarkers(data("<<<<<<<< eight is not seven\n")))
    }

    func testConflictBlocksWithAndWithoutTheCommonAncestor() {
        let lines = ["x", "<<<<<<< HEAD", "ours", "||||||| base", "base", "=======", "theirs", ">>>>>>> topic", "y",
                     "<<<<<<< HEAD", "o2", "=======", "t2", "t3", ">>>>>>> topic"]
        let blocks = ConflictBlock.find(in: lines)
        XCTAssertEqual(blocks, [ConflictBlock(start: 1, base: 3, middle: 5, end: 7), ConflictBlock(start: 9, base: nil, middle: 11, end: 14)])
        XCTAssertEqual(blocks[0].resolution(.ours, in: lines), ["ours"])
        XCTAssertEqual(blocks[0].resolution(.theirs, in: lines), ["theirs"])
        XCTAssertEqual(blocks[1].resolution(.both, in: lines), ["o2", "t2", "t3"])
        // An unfinished block is not a conflict.
        XCTAssertEqual(ConflictBlock.find(in: ["<<<<<<< HEAD", "a", "======="]), [])
    }
}

final class PartialCommitTests: XCTestCase {
    private var root: URL!
    private let fm = FileManager.default

    override func setUp() async throws {
        root = fm.temporaryDirectory.appendingPathComponent("dif-partial-tests-\(UUID().uuidString)")
        try fm.createDirectory(at: root, withIntermediateDirectories: true)
        try git("init", "-q", "-b", "main")
        try git("config", "user.name", "Dif Tests")
        try git("config", "user.email", "tests@example.invalid")
        try git("config", "commit.gpgsign", "false")
        try write("a.txt", (1...10).map { "line \($0)\n" }.joined())
        try write("b.txt", "b\n")
        try write("run.sh", "#!/bin/sh\necho one\necho two\n")
        try fm.setAttributes([.posixPermissions: 0o755], ofItemAtPath: root.appendingPathComponent("run.sh").path)
        try git("add", "-A")
        try git("commit", "-q", "-m", "Initial import")
    }

    override func tearDown() async throws {
        try? fm.removeItem(at: root)
    }

    private func write(_ path: String, _ text: String) throws {
        try Data(text.utf8).write(to: root.appendingPathComponent(path))
    }

    private func git(_ arguments: String...) throws {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: Git.executable)
        process.arguments = arguments
        process.currentDirectoryURL = root
        process.standardOutput = FileHandle.nullDevice
        process.standardError = FileHandle.nullDevice
        try process.run()
        process.waitUntilExit()
        XCTAssertEqual(process.terminationStatus, 0, "git \(arguments.joined(separator: " "))")
    }

    private func output(_ arguments: String...) async throws -> String {
        try await Git.runString(arguments, in: root).trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private func changes() async throws -> [String: FileChange] {
        Dictionary(uniqueKeysWithValues: try await Git.status(in: root).changes.map { ($0.path, $0) })
    }

    func testPartOfAFileGoesInTheRestStaysAndOtherStagedWorkIsLeftAlone() async throws {
        try write("a.txt", (1...10).map { $0 == 2 ? "two\n" : $0 == 9 ? "nine\n" : "line \($0)\n" }.joined())
        try write("new.txt", "n1\nn2\n")
        try write("b.txt", "b staged\n")
        try git("add", "b.txt")                    // staged, not ticked
        let now = try await changes()
        let a = try XCTUnwrap(now["a.txt"]), new = try XCTUnwrap(now["new.txt"])

        let result = try await WorkingTree.commit(
            message: "Part of a", whole: [new],
            partial: [WorkingTree.PartialFile(change: a, excluded: [ChangeLine(removed: true, line: 8, text: "line 9"),
                                                                    ChangeLine(removed: false, line: 8, text: "nine")])],
            in: root)
        XCTAssertEqual(result.fileCount, 2)
        let committedA = try await output("show", "HEAD:a.txt")
        XCTAssertEqual(committedA, (1...10).map { $0 == 2 ? "two" : "line \($0)" }.joined(separator: "\n"))
        let committedNew = try await output("show", "HEAD:new.txt")
        XCTAssertEqual(committedNew, "n1\nn2")
        // The working tree is untouched: the line left out is still a change, and b.txt is still staged, alone.
        let disk = try String(contentsOf: root.appendingPathComponent("a.txt"), encoding: .utf8)
        XCTAssertTrue(disk.contains("nine"))
        let unstaged = try await output("diff", "--name-only")
        XCTAssertEqual(unstaged, "a.txt")
        let staged = try await output("diff", "--cached", "--name-only")
        XCTAssertEqual(staged, "b.txt")
        let files = try await output("show", "--name-only", "--format=", "HEAD")
        XCTAssertEqual(Set(files.split(separator: "\n").map(String.init)), ["a.txt", "new.txt"])
    }

    func testAPartialFileKeepsItsExecutableBit() async throws {
        try write("run.sh", "#!/bin/sh\necho ONE\necho TWO\n")
        let now = try await changes()
        let file = try XCTUnwrap(now["run.sh"])
        _ = try await WorkingTree.commit(message: "One", whole: [],
                                         partial: [WorkingTree.PartialFile(change: file, excluded: [
                                             ChangeLine(removed: true, line: 2, text: "echo two"),
                                             ChangeLine(removed: false, line: 2, text: "echo TWO"),
                                         ])], in: root)
        let tree = try await output("ls-tree", "HEAD", "run.sh")
        XCTAssertTrue(tree.hasPrefix("100755 "), tree)
        let committed = try await output("show", "HEAD:run.sh")
        XCTAssertEqual(committed, "#!/bin/sh\necho ONE\necho two")
    }

    func testLinesPickedBeforeTheFileChangedAgainCommitNothing() async throws {
        try write("a.txt", (1...10).map { $0 == 5 ? "five\n" : "line \($0)\n" }.joined())
        let now = try await changes()
        let file = try XCTUnwrap(now["a.txt"])
        // Picked as "five", then rewritten on disk.
        try write("a.txt", (1...10).map { $0 == 5 ? "FIVE!\n" : "line \($0)\n" }.joined())
        do {
            _ = try await WorkingTree.commit(message: "Stale", whole: [],
                                             partial: [WorkingTree.PartialFile(change: file, excluded: [ChangeLine(removed: false, line: 4, text: "five")])],
                                             in: root)
            XCTFail("the picked line is gone; the commit must refuse")
        } catch {}
        let count = try await output("rev-list", "--count", "HEAD")
        XCTAssertEqual(count, "1")
    }

    func testTheFirstCommitCanTakePartOfANewFile() async throws {
        let fresh = fm.temporaryDirectory.appendingPathComponent("dif-partial-fresh-\(UUID().uuidString)")
        defer { try? fm.removeItem(at: fresh) }
        try fm.createDirectory(at: fresh, withIntermediateDirectories: true)
        root = fresh
        try git("init", "-q", "-b", "main")
        try git("config", "user.name", "Dif Tests")
        try git("config", "user.email", "tests@example.invalid")
        try git("config", "commit.gpgsign", "false")
        try write("one.txt", "keep\ndrop\nkeep too\n")
        let now = try await changes()
        let file = try XCTUnwrap(now["one.txt"])
        let result = try await WorkingTree.commit(message: "First", whole: [],
                                                  partial: [WorkingTree.PartialFile(change: file, excluded: [ChangeLine(removed: false, line: 1, text: "drop")])],
                                                  in: root)
        XCTAssertTrue(result.isRoot)
        let committed = try await output("show", "HEAD:one.txt")
        XCTAssertEqual(committed, "keep\nkeep too")
        let after = try await changes()
        XCTAssertEqual(after["one.txt"]?.kind, .modified)
    }

    func testTwoCommitsCanBeComparedWithEachOther() async throws {
        let first = try await output("rev-parse", "HEAD")
        try write("b.txt", "b2\n")
        try write("c.txt", "c\n")
        try git("add", "-A")
        try git("commit", "-q", "-m", "Second")
        try write("b.txt", "b3, not committed\n")
        let second = try await output("rev-parse", "HEAD")
        let files = try await Git.compareFiles(base: first, head: second, in: root)
        XCTAssertEqual(Set(files.map(\.path)), ["b.txt", "c.txt"])
        // Against the working tree the uncommitted edit counts too, as do untracked files.
        try write("d.txt", "d\n")
        let worktree = try await Git.compareFiles(base: second, in: root)
        XCTAssertEqual(Set(worktree.map(\.path)), ["b.txt", "d.txt"])
    }
}
