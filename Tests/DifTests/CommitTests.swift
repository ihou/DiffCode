import XCTest
@testable import DifKit

final class CommitGraphTests: XCTestCase {
    private func commit(_ hash: String, _ parents: String...) -> Commit {
        Commit(hash: hash, parents: parents, author: "a", email: "", date: Date(), refs: [], subject: hash)
    }

    func testAStraightHistoryStaysInOneLane() {
        let rows = CommitGraph.layout([commit("c", "b"), commit("b", "a"), commit("a")])
        XCTAssertEqual(rows.map(\.column), [0, 0, 0])
        XCTAssertEqual(rows.map(\.width), [1, 1, 1])
        // The tip has nothing above it, the root nothing below.
        XCTAssertFalse(rows[0].segments.contains { $0.span == .intoNode })
        XCTAssertFalse(rows[2].segments.contains { $0.span == .outOfNode })
        XCTAssertTrue(rows[1].segments.contains { $0.span == .intoNode } && rows[1].segments.contains { $0.span == .outOfNode })
    }

    func testABranchAndItsMergeUseASecondLane() {
        //   m        merge of b2 into a2
        //   |\
        //   a2 b2
        //   |  b1
        //   | /
        //   base
        let rows = CommitGraph.layout([
            commit("m", "a2", "b2"),
            commit("a2", "base"),
            commit("b2", "b1"),
            commit("b1", "base"),
            commit("base"),
        ])
        XCTAssertEqual(rows.map(\.column), [0, 0, 1, 1, 0])
        // The merge opens a lane for its second parent.
        XCTAssertTrue(rows[0].segments.contains { $0.span == .outOfNode && $0.to == 1 })
        // While a2 is drawn, the branch passes by in lane 1.
        XCTAssertTrue(rows[1].segments.contains { $0.span == .through && $0.from == 1 })
        // b1 leads to base, which lane 0 already waits for: the line joins it instead of opening a third lane.
        XCTAssertTrue(rows[3].segments.contains { $0.span == .outOfNode && $0.from == 1 && $0.to == 0 })
        XCTAssertEqual(rows.map(\.width).max(), 2)
        // Both lines end in base's dot.
        XCTAssertEqual(rows[4].segments.filter { $0.span == .intoNode }.count, 1)
        XCTAssertEqual(rows[4].column, 0)
    }

    func testTwoTipsOfUnrelatedBranchesGetTheirOwnLanesAndColours() {
        let rows = CommitGraph.layout([commit("x2", "x1"), commit("y1"), commit("x1")])
        XCTAssertEqual(rows.map(\.column), [0, 1, 0])
        XCTAssertNotEqual(rows[0].color, rows[1].color)
        // Lane 1 is free again after y1, so the graph does not keep growing.
        XCTAssertEqual(rows[2].width, 1)
    }

    func testRefsAreParsedWithTrackingAndPeeledTags() {
        let sep = "\u{1f}", end = "\u{1e}"
        let lines = [
            ["refs/heads/main", "main", "aaa", "", "origin/main", "ahead 2, behind 1", "*", "1700000000", ""],
            ["refs/heads/old", "old", "bbb", "", "origin/old", "gone", " ", "1600000000", ""],
            ["refs/remotes/origin/HEAD", "origin", "aaa", "", "", "", " ", "1700000000", "refs/remotes/origin/main"],
            ["refs/remotes/origin/main", "origin/main", "ccc", "", "", "", " ", "1690000000", ""],
            ["refs/tags/v1", "v1", "tagobject", "ddd", "", "", " ", "1500000000", ""],
        ]
        let refs = Git.parseRefs(Data(lines.map { $0.joined(separator: sep) + end }.joined().utf8))
        XCTAssertEqual(refs.map(\.shortName), ["main", "old", "origin/main", "v1"])
        XCTAssertEqual(refs[0].kind, .local)
        XCTAssertTrue(refs[0].isHead)
        XCTAssertEqual(refs[0].ahead, 2)
        XCTAssertEqual(refs[0].behind, 1)
        XCTAssertTrue(refs[1].upstreamGone)
        XCTAssertEqual(refs[2].kind, .remote)
        XCTAssertEqual(refs[3].kind, .tag)
        XCTAssertEqual(refs[3].target, "ddd")
    }
}

final class CommitTests: XCTestCase {
    private var root: URL!
    private let fm = FileManager.default

    override func setUp() async throws {
        root = fm.temporaryDirectory.appendingPathComponent("dif-commit-tests-\(UUID().uuidString)")
        try fm.createDirectory(at: root.appendingPathComponent("src"), withIntermediateDirectories: true)
        try git("init", "-q", "-b", "main")
        // The repository's own identity, so the tests do not depend on (or use) the machine's.
        try git("config", "user.name", "Dif Tests")
        try git("config", "user.email", "tests@example.invalid")
        try git("config", "commit.gpgsign", "false")
        try write("src/A.kt", "class A\n")
        try write("src/B.kt", "class B\n")
        try write("old.txt", "old\n")
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

    func testOnlyTheTickedFilesAreCommittedAndOtherStagedWorkStaysStaged() async throws {
        try write("src/A.kt", "class A { val x = 1 }\n")
        try write("src/B.kt", "class B { val y = 2 }\n")
        try git("add", "src/B.kt")                 // staged, but not ticked
        try write("src/New.kt", "class New\n")     // untracked, ticked
        try write("notes.txt", "scratch\n")        // untracked, not ticked
        let now = try await changes()

        let result = try await WorkingTree.commit(message: "  Add New and change A\n\nDetails.  ",
                                                  changes: [try XCTUnwrap(now["src/A.kt"]), try XCTUnwrap(now["src/New.kt"])], in: root)
        XCTAssertFalse(result.isRoot)
        XCTAssertEqual(result.fileCount, 2)
        let files = try await output("show", "--name-only", "--format=", "HEAD")
        XCTAssertEqual(Set(files.split(separator: "\n").map(String.init)), ["src/A.kt", "src/New.kt"])
        let message = try await output("log", "-1", "--format=%B")
        XCTAssertEqual(message, "Add New and change A\n\nDetails.")

        let after = try await changes()
        XCTAssertEqual(Set(after.keys), ["src/B.kt", "notes.txt"])
        let staged = try await output("diff", "--cached", "--name-only")
        XCTAssertEqual(staged, "src/B.kt")
    }

    func testARenameIsCommittedWithItsOldPath() async throws {
        try git("mv", "old.txt", "new.txt")
        let now = try await changes()
        let rename = try XCTUnwrap(now["new.txt"])
        XCTAssertEqual(rename.kind, .renamed)
        _ = try await WorkingTree.commit(message: "Rename", changes: [rename], in: root)
        let after = try await changes()
        XCTAssertTrue(after.isEmpty, "\(after.keys)")
    }

    func testAFailedCommitLeavesNewFilesUnversioned() async throws {
        // A hook that refuses every commit, as a linter might.
        let hook = root.appendingPathComponent(".git/hooks/pre-commit")
        try Data("#!/bin/sh\necho 'lint: no'\nexit 1\n".utf8).write(to: hook)
        try fm.setAttributes([.posixPermissions: 0o755], ofItemAtPath: hook.path)
        try write("src/New.kt", "class New\n")
        let now = try await changes()
        do {
            _ = try await WorkingTree.commit(message: "Nope", changes: Array(now.values), in: root)
            XCTFail("the hook should have refused")
        } catch {
            XCTAssertTrue(error.localizedDescription.contains("lint: no"), error.localizedDescription)
        }
        let after = try await changes()
        XCTAssertEqual(after["src/New.kt"]?.kind, .untracked)
        let count = try await output("rev-list", "--count", "HEAD")
        XCTAssertEqual(count, "1")
    }

    func testUndoCommitMovesTheBranchBackAndKeepsTheChanges() async throws {
        try write("src/A.kt", "class A { val x = 1 }\n")
        let now = try await changes()
        let result = try await WorkingTree.commit(message: "Change A", changes: Array(now.values), in: root)
        try await WorkingTree.undoCommit(result, in: root)
        let count = try await output("rev-list", "--count", "HEAD")
        XCTAssertEqual(count, "1")
        let after = try await changes()
        XCTAssertEqual(after["src/A.kt"]?.kind, .modified)
        // A second undo finds HEAD elsewhere and refuses.
        do {
            try await WorkingTree.undoCommit(result, in: root)
            XCTFail("HEAD moved; undo must refuse")
        } catch {}
    }

    func testTheFirstCommitOfARepositoryCanBeMadeAndUndone() async throws {
        let fresh = fm.temporaryDirectory.appendingPathComponent("dif-commit-fresh-\(UUID().uuidString)")
        defer { try? fm.removeItem(at: fresh) }
        try fm.createDirectory(at: fresh, withIntermediateDirectories: true)
        root = fresh
        try git("init", "-q", "-b", "main")
        try git("config", "user.name", "Dif Tests")
        try git("config", "user.email", "tests@example.invalid")
        try git("config", "commit.gpgsign", "false")
        try write("one.txt", "1\n")
        try write("two.txt", "2\n")
        let now = try await changes()
        let result = try await WorkingTree.commit(message: "First", changes: [try XCTUnwrap(now["one.txt"])], in: root)
        XCTAssertTrue(result.isRoot)
        let after = try await changes()
        XCTAssertEqual(Set(after.keys), ["two.txt"])
        try await WorkingTree.undoCommit(result, in: root)
        let status = try await Git.status(in: root)
        XCTAssertTrue(status.isInitial)
    }

    func testEmptyMessageOrNoFilesIsRefusedBeforeGitRuns() async throws {
        try write("src/A.kt", "class A { val x = 1 }\n")
        let now = try await changes()
        do {
            _ = try await WorkingTree.commit(message: "  \n ", changes: Array(now.values), in: root)
            XCTFail("empty message")
        } catch {}
        do {
            _ = try await WorkingTree.commit(message: "Something", changes: [], in: root)
            XCTFail("no files")
        } catch {}
        let count = try await output("rev-list", "--count", "HEAD")
        XCTAssertEqual(count, "1")
    }
}
