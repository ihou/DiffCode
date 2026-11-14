import XCTest
@testable import DifKit

/// A repository of its own for each test, with a bare one beside it standing in for the server where a test needs it.
class GitFixture: XCTestCase {
    var root: URL!
    var scratch: URL!
    let fm = FileManager.default

    override func setUp() async throws {
        scratch = fm.temporaryDirectory.appendingPathComponent("dif-history-edit-\(UUID().uuidString)")
        root = scratch.appendingPathComponent("work")
        try fm.createDirectory(at: root, withIntermediateDirectories: true)
        try git("init", "-q", "-b", "main")
        try identify(root)
        try write("a.txt", "one\ntwo\nthree\n")
        try write("b.txt", "bee\n")
        try git("add", "-A")
        try git("commit", "-q", "-m", "Initial import")
    }

    override func tearDown() async throws {
        try? fm.removeItem(at: scratch)
    }

    /// The repository's own identity, so the tests do not depend on (or use) the machine's.
    func identify(_ dir: URL) throws {
        try git("config", "user.name", "Dif Tests", in: dir)
        try git("config", "user.email", "tests@example.invalid", in: dir)
        try git("config", "commit.gpgsign", "false", in: dir)
    }

    func write(_ path: String, _ text: String, in dir: URL? = nil) throws {
        try Data(text.utf8).write(to: (dir ?? root).appendingPathComponent(path))
    }

    func read(_ path: String) throws -> String {
        try String(contentsOf: root.appendingPathComponent(path), encoding: .utf8)
    }

    func git(_ arguments: String..., in dir: URL? = nil) throws {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: Git.executable)
        process.arguments = arguments
        process.currentDirectoryURL = dir ?? root
        process.standardOutput = FileHandle.nullDevice
        process.standardError = FileHandle.nullDevice
        try process.run()
        process.waitUntilExit()
        XCTAssertEqual(process.terminationStatus, 0, "git \(arguments.joined(separator: " "))")
    }

    func output(_ arguments: String..., in dir: URL? = nil) async throws -> String {
        try await Git.runString(arguments, in: dir ?? root).trimmingCharacters(in: .whitespacesAndNewlines)
    }

    func commit(_ path: String, _ text: String, _ message: String) async throws -> String {
        try write(path, text)
        try git("add", path)
        try git("commit", "-q", "-m", message)
        return try await output("rev-parse", "HEAD")
    }

    func changes() async throws -> [String: FileChange] {
        Dictionary(uniqueKeysWithValues: try await Git.status(in: root).changes.map { ($0.path, $0) })
    }

    /// A bare repository with main pushed to it, as origin, followed by main.
    @discardableResult
    func addRemote() throws -> URL {
        let remote = scratch.appendingPathComponent("origin.git")
        try git("init", "-q", "--bare", "-b", "main", remote.path, in: scratch)
        try git("remote", "add", "origin", remote.path)
        try git("push", "-q", "-u", "origin", "main")
        return remote
    }

    /// Another clone of the remote, as a colleague's.
    func colleague(_ remote: URL) throws -> URL {
        let other = scratch.appendingPathComponent("colleague")
        try git("clone", "-q", remote.path, other.path, in: scratch)
        try identify(other)
        return other
    }
}

// MARK: - Amend

final class AmendTests: GitFixture {
    func testAmendTakesInMoreFilesAndANewMessageAndStaysOneCommit() async throws {
        let first = try await commit("a.txt", "one\ntwo\nthree\nfour\n", "Add four")
        try write("b.txt", "bee\nhive\n")
        let now = try await changes()
        let result = try await WorkingTree.commit(message: "Add four and a hive", changes: [try XCTUnwrap(now["b.txt"])], amend: true, in: root)
        XCTAssertEqual(result.amended, first)
        XCTAssertNotEqual(result.hash, first)
        let count = try await output("rev-list", "--count", "HEAD")
        XCTAssertEqual(count, "2")
        let message = try await output("log", "-1", "--format=%B")
        XCTAssertEqual(message, "Add four and a hive")
        let files = try await output("show", "--name-only", "--format=", "HEAD")
        XCTAssertEqual(Set(files.split(separator: "\n").map(String.init)), ["a.txt", "b.txt"])
        let after = try await changes()
        XCTAssertTrue(after.isEmpty, "\(after.keys)")
    }

    func testAmendingOnlyTheMessageLeavesStagedWorkOutOfIt() async throws {
        let first = try await commit("a.txt", "one\ntwo\nthree\nfour\n", "Add fuor")
        try write("b.txt", "bee\nstaged\n")
        try git("add", "b.txt")
        let result = try await WorkingTree.commit(message: "Add four", changes: [], amend: true, in: root)
        XCTAssertEqual(result.amended, first)
        XCTAssertEqual(result.fileCount, 0)
        let message = try await output("log", "-1", "--format=%B")
        XCTAssertEqual(message, "Add four")
        let treeDiff = try await output("diff", "--name-only", first, "HEAD")
        XCTAssertEqual(treeDiff, "", "only the message changed")
        let staged = try await output("diff", "--cached", "--name-only")
        XCTAssertEqual(staged, "b.txt")
    }

    func testUndoingAnAmendBringsBackTheCommitItReplaced() async throws {
        let first = try await commit("a.txt", "one\ntwo\nthree\nfour\n", "Add four")
        try write("b.txt", "bee\nhive\n")
        let now = try await changes()
        let result = try await WorkingTree.commit(message: "Add four and more", changes: Array(now.values), amend: true, in: root)
        try await WorkingTree.undoCommit(result, in: root)
        let head = try await output("rev-parse", "HEAD")
        XCTAssertEqual(head, first)
        let message = try await output("log", "-1", "--format=%B")
        XCTAssertEqual(message, "Add four")
        // What the amend added is back, staged, and on disk as it was.
        let staged = try await output("diff", "--cached", "--name-only")
        XCTAssertEqual(staged, "b.txt")
        XCTAssertEqual(try read("b.txt"), "bee\nhive\n")
    }

    func testAPartialAmendTakesOnlyThePickedLines() async throws {
        let first = try await commit("b.txt", "bee\nb\n", "Bee")
        try write("a.txt", "zero\none\ntwo\nthree\nfour\n")
        let now = try await changes()
        let change = try XCTUnwrap(now["a.txt"])
        // Leave the added "four" out: only "zero" goes in.
        let partial = WorkingTree.PartialFile(change: change, excluded: [ChangeLine(removed: false, line: 4, text: "four")])
        let result = try await WorkingTree.commit(message: "Bee and zero", whole: [], partial: [partial], amend: true, in: root)
        XCTAssertEqual(result.amended, first)
        let committed = try await output("show", "HEAD:a.txt")
        XCTAssertEqual(committed, "zero\none\ntwo\nthree")
        XCTAssertEqual(try read("a.txt"), "zero\none\ntwo\nthree\nfour\n", "the working tree is not touched")
        let count = try await output("rev-list", "--count", "HEAD")
        XCTAssertEqual(count, "2")
        XCTAssertFalse(result.isRoot)
        let files = try await output("show", "--name-only", "--format=", "HEAD")
        XCTAssertEqual(Set(files.split(separator: "\n").map(String.init)), ["a.txt", "b.txt"], "the amended commit keeps its own change")
    }

    func testThereIsNothingToAmendBeforeTheFirstCommit() async throws {
        let fresh = scratch.appendingPathComponent("fresh")
        try fm.createDirectory(at: fresh, withIntermediateDirectories: true)
        try git("init", "-q", "-b", "main", in: fresh)
        try identify(fresh)
        try write("one.txt", "1\n", in: fresh)
        let status = try await Git.status(in: fresh)
        do {
            _ = try await WorkingTree.commit(message: "First", changes: status.changes, amend: true, in: fresh)
            XCTFail("an unborn branch has nothing to amend")
        } catch {
            XCTAssertTrue(error.localizedDescription.contains("no commit to amend") || error.localizedDescription.contains("没有可以修改"),
                          error.localizedDescription)
        }
    }
}

// MARK: - Stash

final class StashTests: GitFixture {
    func testParsesTheList() {
        let a = String(repeating: "a", count: 40), b = String(repeating: "b", count: 40)
        let c = String(repeating: "c", count: 40), u = String(repeating: "d", count: 40)
        let text = "\(a)\u{1f}\(b) \(c)\u{1f}1700000000\u{1f}stash@{0}\u{1f}On main: try the parser\u{1e}\n"
            + "\(c)\u{1f}\(b) \(c) \(u)\u{1f}1690000000\u{1f}stash@{1}\u{1f}WIP on feature/x: 1234567 Subject: with a colon\u{1e}\n"
            + "\(b)\u{1f}\(c) \(a)\u{1f}1680000000\u{1f}stash@{2}\u{1f}On (no branch): detached\u{1e}"
        let list = Git.parseStashes(Data(text.utf8))
        XCTAssertEqual(list.count, 3)
        XCTAssertEqual(list[0].hash, a)
        XCTAssertEqual(list[0].base, b)
        XCTAssertNil(list[0].untracked)
        XCTAssertEqual(list[0].title, "try the parser")
        XCTAssertEqual(list[0].branch, "main")
        XCTAssertEqual(list[0].name, "stash@{0}")
        XCTAssertEqual(list[1].untracked, u)
        XCTAssertEqual(list[1].title, "1234567 Subject: with a colon")
        XCTAssertEqual(list[1].branch, "feature/x")
        XCTAssertEqual(list[1].index, 1)
        XCTAssertNil(list[2].branch)
        XCTAssertEqual(list[2].date, Date(timeIntervalSince1970: 1680000000))
    }

    func testStashedChangesComeBackAndTheStashStaysUntilPopped() async throws {
        try write("a.txt", "one\n2\nthree\n")
        let entryPushed = try await Git.stashPush(message: "  halfway  ", includeUntracked: false, in: root)
        let entry = try XCTUnwrap(entryPushed)
        XCTAssertEqual(entry.title, "halfway")
        XCTAssertEqual(entry.branch, "main")
        let clean = try await changes()
        XCTAssertTrue(clean.isEmpty)
        XCTAssertEqual(try read("a.txt"), "one\ntwo\nthree\n")

        let files = try await Git.stashFiles(entry, in: root)
        XCTAssertEqual(files.map(\.path), ["a.txt"])
        XCTAssertEqual(files.first?.kind, .modified)

        try await Git.stashApply(entry, drop: false, in: root)
        XCTAssertEqual(try read("a.txt"), "one\n2\nthree\n")
        let kept = await Git.stashes(in: root)
        XCTAssertEqual(kept.map(\.hash), [entry.hash])

        try git("checkout", "--", "a.txt")
        try await Git.stashApply(entry, drop: true, in: root)
        XCTAssertEqual(try read("a.txt"), "one\n2\nthree\n")
        let gone = await Git.stashes(in: root)
        XCTAssertTrue(gone.isEmpty)
    }

    func testAStashIsFoundByItsCommitAfterTheListMoved() async throws {
        try write("a.txt", "first\n")
        let olderPushed = try await Git.stashPush(message: "older", includeUntracked: false, in: root)
        let older = try XCTUnwrap(olderPushed)
        try write("b.txt", "second\n")
        let newerPushed = try await Git.stashPush(message: "newer", includeUntracked: false, in: root)
        let newer = try XCTUnwrap(newerPushed)
        XCTAssertEqual(older.index, 0, "read when it was the newest")
        // "older" is stash@{1} now; acting on it by its old name would hit "newer".
        try await Git.stashDrop(older, in: root)
        let left = await Git.stashes(in: root)
        XCTAssertEqual(left.map(\.hash), [newer.hash])
        do {
            try await Git.stashApply(older, drop: false, in: root)
            XCTFail("a dropped stash is no longer in the list")
        } catch {}
    }

    func testADroppedStashCanBeListedAgain() async throws {
        try write("a.txt", "dropped\n")
        let entryPushed = try await Git.stashPush(message: "keep me", includeUntracked: false, in: root)
        let entry = try XCTUnwrap(entryPushed)
        try await Git.stashDrop(entry, in: root)
        let empty = await Git.stashes(in: root)
        XCTAssertTrue(empty.isEmpty)
        try await Git.stashRestore(entry, in: root)
        let back = await Git.stashes(in: root)
        XCTAssertEqual(back.map(\.hash), [entry.hash])
        XCTAssertEqual(back.first?.title, "keep me")
        try await Git.stashApply(try XCTUnwrap(back.first), drop: true, in: root)
        XCTAssertEqual(try read("a.txt"), "dropped\n")
    }

    func testNewFilesGoOnlyWhenAsked() async throws {
        try write("new.txt", "fresh\nfile\n")
        let nothing = try await Git.stashPush(message: "", includeUntracked: false, in: root)
        XCTAssertNil(nothing, "new files alone stay unless included")
        XCTAssertTrue(fm.fileExists(atPath: root.appendingPathComponent("new.txt").path))

        try write("a.txt", "changed\n")
        let entryPushed = try await Git.stashPush(message: "", includeUntracked: true, in: root)
        let entry = try XCTUnwrap(entryPushed)
        XCTAssertNotNil(entry.untracked)
        XCTAssertFalse(fm.fileExists(atPath: root.appendingPathComponent("new.txt").path))
        XCTAssertTrue(entry.message.hasPrefix("WIP on main"), entry.message)
        let files = try await Git.stashFiles(entry, in: root)
        let byPath = Dictionary(uniqueKeysWithValues: files.map { ($0.path, $0) })
        XCTAssertEqual(byPath["a.txt"]?.kind, .modified)
        XCTAssertEqual(byPath["new.txt"]?.kind, .untracked)
        XCTAssertEqual(byPath["new.txt"]?.additions, 2)

        try await Git.stashApply(entry, drop: true, in: root)
        XCTAssertEqual(try read("new.txt"), "fresh\nfile\n")
    }

    func testAClashKeepsTheStash() async throws {
        try write("a.txt", "stashed\n")
        let entryPushed = try await Git.stashPush(message: "clash", includeUntracked: false, in: root)
        let entry = try XCTUnwrap(entryPushed)
        _ = try await commit("a.txt", "committed\n", "Meanwhile")
        do {
            try await Git.stashApply(entry, drop: true, in: root)
            XCTFail("the stash clashes with the new commit")
        } catch {}
        let now = try await changes()
        XCTAssertEqual(now["a.txt"]?.kind, .conflicted)
        let kept = await Git.stashes(in: root)
        XCTAssertEqual(kept.map(\.hash), [entry.hash], "git keeps a stash it could not apply cleanly")
    }
}

// MARK: - Cherry-pick, revert, reset

final class HistoryEditTests: GitFixture {
    func testCherryPickAppliesACommitOfAnotherBranchAndCanBeUndone() async throws {
        try git("switch", "-q", "-c", "feature")
        let picked = try await commit("b.txt", "bee\nfeature\n", "Feature work")
        try git("switch", "-q", "main")
        // main moves on, so the copy has another parent than the original.
        let before = try await commit("a.txt", "one\ntwo\nthree\nmain\n", "Main work")
        let move = try await Git.cherryPick([picked], in: root)
        XCTAssertEqual(move.branch, "main")
        XCTAssertEqual(move.before, before)
        let subject = try await output("log", "-1", "--format=%s")
        XCTAssertEqual(subject, "Feature work")
        XCTAssertNotEqual(move.after, picked, "a new commit, not the same one")
        let applied = await Git.alreadyApplied([picked], in: root)
        XCTAssertEqual(applied, [picked], "its changes are on main now")

        try await Git.undoHeadMove(move, in: root)
        let head = try await output("rev-parse", "HEAD")
        XCTAssertEqual(head, before)
        XCTAssertEqual(try read("b.txt"), "bee\n")
        let none = await Git.alreadyApplied([picked], in: root)
        XCTAssertTrue(none.isEmpty)
    }

    func testACherryPickThatClashesWaitsForTheUser() async throws {
        try git("switch", "-q", "-c", "feature")
        let picked = try await commit("a.txt", "one\nfeature\nthree\n", "Feature line")
        try git("switch", "-q", "main")
        _ = try await commit("a.txt", "one\nmain\nthree\n", "Main line")
        do {
            try await Git.cherryPick([picked], in: root)
            XCTFail("both changed the same line")
        } catch {}
        let found = await Git.gitDirectory(in: root)
        let directory = try XCTUnwrap(found)
        XCTAssertEqual(Git.operation(in: directory), .cherryPick)
        try await Git.skipOperation(.cherryPick, in: root)
        XCTAssertNil(Git.operation(in: directory))
        XCTAssertEqual(try read("a.txt"), "one\nmain\nthree\n")
    }

    func testRevertAddsACommitThatUndoesOne() async throws {
        let target = try await commit("a.txt", "one\ntwo\nthree\nfour\n", "Add four")
        _ = try await commit("b.txt", "bee\nlater\n", "Later")
        let move = try await Git.revert([target], in: root)
        XCTAssertEqual(try read("a.txt"), "one\ntwo\nthree\n")
        XCTAssertEqual(try read("b.txt"), "bee\nlater\n")
        let subject = try await output("log", "-1", "--format=%s")
        XCTAssertEqual(subject, "Revert \"Add four\"")
        try await Git.undoHeadMove(move, in: root)
        XCTAssertEqual(try read("a.txt"), "one\ntwo\nthree\nfour\n")
    }

    func testSoftAndMixedResetBringTheChangesBack() async throws {
        let base = try await output("rev-parse", "HEAD")
        _ = try await commit("a.txt", "one\ntwo\nthree\nfour\n", "Add four")
        let soft = try await Git.reset(to: base, mode: .soft, in: root)
        let staged = try await output("diff", "--cached", "--name-only")
        XCTAssertEqual(staged, "a.txt")
        try await Git.undoHeadMove(soft, in: root)
        let clean = try await changes()
        XCTAssertTrue(clean.isEmpty)

        let mixed = try await Git.reset(to: base, mode: .mixed, in: root)
        let unstaged = try await output("diff", "--name-only")
        XCTAssertEqual(unstaged, "a.txt")
        let nothingStaged = try await output("diff", "--cached", "--name-only")
        XCTAssertEqual(nothingStaged, "")
        try await Git.undoHeadMove(mixed, in: root)
        let head = try await output("log", "-1", "--format=%s")
        XCTAssertEqual(head, "Add four")
    }

    func testAHardResetCanBeUndoneWithTheLocalChangesItThrewAway() async throws {
        let base = try await output("rev-parse", "HEAD")
        _ = try await commit("a.txt", "one\ntwo\nthree\nfour\n", "Add four")
        try write("b.txt", "bee\nlocal\n")
        let move = try await Git.reset(to: base, mode: .hard, in: root)
        XCTAssertNotNil(move.snapshot)
        XCTAssertEqual(try read("b.txt"), "bee\n")
        XCTAssertEqual(try read("a.txt"), "one\ntwo\nthree\n")
        // Something new meanwhile, in a file the reset did not touch, stays.
        try write("new.txt", "new\n")
        try await Git.undoHeadMove(move, in: root)
        XCTAssertEqual(try read("a.txt"), "one\ntwo\nthree\nfour\n")
        XCTAssertEqual(try read("b.txt"), "bee\nlocal\n")
        XCTAssertEqual(try read("new.txt"), "new\n")
    }

    func testKeepRefusesToOverwriteLocalChanges() async throws {
        let base = try await output("rev-parse", "HEAD")
        _ = try await commit("a.txt", "one\ntwo\nthree\nfour\n", "Add four")
        try write("a.txt", "local\n")
        do {
            _ = try await Git.reset(to: base, mode: .keep, in: root)
            XCTFail("the commit and the local change are in the same file")
        } catch {}
        XCTAssertEqual(try read("a.txt"), "local\n")
        let subject = try await output("log", "-1", "--format=%s")
        XCTAssertEqual(subject, "Add four")
    }

    func testUndoRefusesOnceHeadMovedOn() async throws {
        let base = try await output("rev-parse", "HEAD")
        _ = try await commit("a.txt", "one\ntwo\nthree\nfour\n", "Add four")
        let move = try await Git.reset(to: base, mode: .mixed, in: root)
        try git("add", "a.txt")
        try git("commit", "-q", "-m", "Again")
        do {
            try await Git.undoHeadMove(move, in: root)
            XCTFail("HEAD moved since the reset")
        } catch {}
        // Nor on another branch that happens to be at the same commit.
        let other = try await Git.reset(to: base, mode: .mixed, in: root)
        try git("stash", "-q")
        try git("switch", "-q", "-c", "elsewhere")
        do {
            try await Git.undoHeadMove(other, in: root)
            XCTFail("another branch is checked out")
        } catch {}
    }

    func testUndoRefusesToDropCommitsThatWerePushedSince() async throws {
        try addRemote()
        let before = try await output("rev-parse", "HEAD")
        try git("switch", "-q", "-c", "feature")
        _ = try await commit("b.txt", "bee\nfeature\n", "Feature")
        try git("switch", "-q", "main")
        let (_, move) = try await Git.merge("refs/heads/feature", autostash: false, in: root)
        XCTAssertEqual(move.before, before)
        try git("push", "-q", "origin", "main")
        do {
            try await Git.undoHeadMove(move, in: root)
            XCTFail("the merged commits are on origin/main now")
        } catch {}
    }

    // MARK: Merge and rebase

    func testMergeFastForwardsMergesOrFindsNothingToDo() async throws {
        try git("switch", "-q", "-c", "feature")
        _ = try await commit("b.txt", "bee\nfeature\n", "Feature")
        try git("switch", "-q", "main")
        let ff = try await Git.merge("refs/heads/feature", autostash: false, in: root)
        XCTAssertEqual(ff.outcome, .fastForward)
        let again = try await Git.merge("refs/heads/feature", autostash: false, in: root)
        XCTAssertEqual(again.outcome, .upToDate)

        try git("switch", "-q", "feature")
        _ = try await commit("b.txt", "bee\nfeature\nmore\n", "More feature")
        try git("switch", "-q", "main")
        _ = try await commit("a.txt", "one\ntwo\nthree\nmain\n", "Main")
        let merged = try await Git.merge("refs/heads/feature", autostash: false, in: root)
        XCTAssertEqual(merged.outcome, .merged)
        let subject = try await output("log", "-1", "--format=%s")
        XCTAssertEqual(subject, "Merge branch 'feature'")
        let short = await Git.shortestName("refs/heads/feature", in: root)
        XCTAssertEqual(short, "feature")
        try git("tag", "feature", "HEAD~1")
        let shadowed = await Git.shortestName("refs/heads/feature", in: root)
        XCTAssertEqual(shadowed, "refs/heads/feature", "a tag of the same name would win")
        try await Git.undoHeadMove(merged.move, in: root)
        let back = try await output("log", "-1", "--format=%s")
        XCTAssertEqual(back, "Main")
    }

    func testAMergeInTheWayOfLocalChangesIsRefusedThenGoesThroughWithThemPutAside() async throws {
        try git("switch", "-q", "-c", "feature")
        _ = try await commit("a.txt", "one\ntwo\nthree\nfeature\n", "Feature")
        try git("switch", "-q", "main")
        try write("a.txt", "local\none\ntwo\nthree\n")
        do {
            _ = try await Git.merge("refs/heads/feature", autostash: false, in: root)
            XCTFail("the local change is in a file the merge changes")
        } catch {
            XCTAssertTrue(Git.refusedForLocalChanges(error.localizedDescription), error.localizedDescription)
        }
        let result = try await Git.merge("refs/heads/feature", autostash: true, in: root)
        XCTAssertEqual(result.outcome, .fastForward)
        XCTAssertEqual(try read("a.txt"), "local\none\ntwo\nthree\nfeature\n")
    }

    func testAMergeThatClashesWaits() async throws {
        try git("switch", "-q", "-c", "feature")
        _ = try await commit("a.txt", "one\nfeature\nthree\n", "Feature")
        try git("switch", "-q", "main")
        _ = try await commit("a.txt", "one\nmain\nthree\n", "Main")
        do {
            _ = try await Git.merge("refs/heads/feature", autostash: false, in: root)
            XCTFail("both changed the same line")
        } catch {
            XCTAssertFalse(Git.refusedForLocalChanges(error.localizedDescription))
        }
        let found = await Git.gitDirectory(in: root)
        let directory = try XCTUnwrap(found)
        XCTAssertEqual(Git.operation(in: directory), .merge)
    }

    func testRebaseReplaysTheBranchOnTopAndCanBeUndone() async throws {
        try git("switch", "-q", "-c", "feature")
        _ = try await commit("b.txt", "bee\nfeature\n", "Feature")
        try git("switch", "-q", "main")
        _ = try await commit("a.txt", "one\ntwo\nthree\nmain\n", "Main")
        try git("switch", "-q", "feature")
        let move = try await Git.rebase(onto: "refs/heads/main", autostash: false, in: root)
        XCTAssertEqual(move.branch, "feature")
        let main = try await output("rev-parse", "main")
        let parent = try await output("rev-parse", "HEAD^")
        XCTAssertEqual(parent, main)
        try await Git.undoHeadMove(move, in: root)
        let head = try await output("rev-parse", "HEAD")
        XCTAssertEqual(head, move.before)
    }

    func testARebaseThatClashesWaits() async throws {
        try git("switch", "-q", "-c", "feature")
        _ = try await commit("a.txt", "one\nfeature\nthree\n", "Feature")
        try git("switch", "-q", "main")
        _ = try await commit("a.txt", "one\nmain\nthree\n", "Main")
        try git("switch", "-q", "feature")
        do {
            _ = try await Git.rebase(onto: "refs/heads/main", autostash: false, in: root)
            XCTFail("both changed the same line")
        } catch {}
        let found = await Git.gitDirectory(in: root)
        let directory = try XCTUnwrap(found)
        XCTAssertEqual(Git.operation(in: directory), .rebase)
        try await Git.abortOperation(.rebase, in: root)
        XCTAssertNil(Git.operation(in: directory))
    }

    // MARK: Branches

    func testABranchWithCommitsOfItsOwnIsOnlyDeletedByForceAndComesBack() async throws {
        try addRemote()
        try git("switch", "-q", "-c", "feature")
        _ = try await commit("b.txt", "bee\nfeature\n", "Feature")
        try git("push", "-q", "-u", "origin", "feature")
        _ = try await commit("b.txt", "bee\nfeature\nunpushed\n", "Unpushed")
        let unpushedTip = try await output("rev-parse", "HEAD")
        try git("switch", "-q", "main")
        do {
            try await Git.deleteBranch("feature", force: false, in: root)
            XCTFail("its last commit is on no other branch")
        } catch {
            XCTAssertTrue(Git.isNotFullyMerged(error.localizedDescription), error.localizedDescription)
        }
        let own = await Git.commits(unpushedTip, "--not", "--exclude=feature", "--branches", "--remotes", in: root)
        XCTAssertEqual(own.map(\.subject), ["Unpushed"], "origin/feature has the first one")
        try await Git.deleteBranch("feature", force: true, in: root)
        let gone = try await output("branch", "--list", "feature")
        XCTAssertEqual(gone, "")
        try await Git.restoreBranch("feature", at: unpushedTip, upstream: "origin/feature", in: root)
        let back = try await output("rev-parse", "feature")
        XCTAssertEqual(back, unpushedTip)
        let upstream = try await output("rev-parse", "--abbrev-ref", "feature@{upstream}")
        XCTAssertEqual(upstream, "origin/feature")
    }

    func testRenamingABranchKeepsWhatItFollows() async throws {
        try addRemote()
        try git("switch", "-q", "-c", "feature")
        try git("push", "-q", "-u", "origin", "feature")
        try await Git.renameBranch("feature", to: "feature-2", in: root)
        let current = await Git.currentBranch(in: root)
        XCTAssertEqual(current, "feature-2")
        let upstream = try await output("rev-parse", "--abbrev-ref", "feature-2@{upstream}")
        XCTAssertEqual(upstream, "origin/feature")
    }

    func testARemoteBranchNameSplitsAtTheLongestRemote() {
        XCTAssertTrue(Git.splitRemoteBranch("origin/feature/x", remotes: ["origin"]).map { $0 == ("origin", "feature/x") } ?? false)
        XCTAssertTrue(Git.splitRemoteBranch("up/stream/main", remotes: ["up", "up/stream"]).map { $0 == ("up/stream", "main") } ?? false)
        XCTAssertNil(Git.splitRemoteBranch("fork/main", remotes: ["origin"]))
        XCTAssertNil(Git.splitRemoteBranch("origin/", remotes: ["origin"]))
    }

    func testARemoteBranchCanBeDeletedAndPushedBack() async throws {
        let remote = try addRemote()
        try git("switch", "-q", "-c", "feature")
        let tip = try await commit("b.txt", "bee\nfeature\n", "Feature")
        try git("push", "-q", "-u", "origin", "feature")
        try await Git.deleteRemoteBranch("feature", remote: "origin", in: root)
        let after = try await output("branch", "--list", "feature", in: remote)
        XCTAssertEqual(after, "")
        try await Git.restoreRemoteBranch("feature", remote: "origin", at: tip, in: root)
        let back = try await output("rev-parse", "feature", in: remote)
        XCTAssertEqual(back, tip)
    }

    // MARK: Force push

    func testForcePushReplacesTheRemoteOnlyWhereItWasExpected() async throws {
        let remote = try addRemote()
        try git("switch", "-q", "-c", "feature")
        let old = try await commit("b.txt", "bee\nfeature\n", "Feature")
        try git("push", "-q", "-u", "origin", "feature")
        // Amend the pushed commit: the branch and its upstream now each have one the other has not.
        try git("commit", "-q", "--amend", "-m", "Feature, reworded")
        let incoming = await Git.incomingCommits(in: root)
        XCTAssertEqual(incoming.map(\.hash), [old])
        let target = await Git.upstreamTarget(of: "feature", in: root)
        XCTAssertEqual(target?.remote, "origin")
        XCTAssertEqual(target?.branch, "feature")

        // A colleague pushes meanwhile; what this clone last saw of origin/feature is out of date.
        let other = try colleague(remote)
        try git("switch", "-q", "feature", in: other)
        try write("c.txt", "theirs\n", in: other)
        try git("add", "c.txt", in: other)
        try git("commit", "-q", "-m", "Colleague", in: other)
        try git("push", "-q", "origin", "feature", in: other)
        do {
            try await Git.forcePush(branch: "feature", expected: old, in: root)
            XCTFail("the remote moved since it was expected at \(old)")
        } catch {
            XCTAssertTrue(error.localizedDescription.contains("stale info"), error.localizedDescription)
        }
        let theirs = try await output("log", "-1", "--format=%s", "feature", in: remote)
        XCTAssertEqual(theirs, "Colleague", "nothing was overwritten")

        // Once fetched (and looked at), what is there now is what the force push expects, and it goes through.
        try git("fetch", "-q", "origin")
        let current = try await output("rev-parse", "origin/feature")
        try await Git.forcePush(branch: "feature", expected: current, in: root)
        let replaced = try await output("rev-parse", "feature", in: remote)
        let local = try await output("rev-parse", "feature")
        XCTAssertEqual(replaced, local)
    }

    func testMainIsNeverForcePushed() async throws {
        try addRemote()
        let pushed = try await output("rev-parse", "HEAD")
        try git("commit", "-q", "--amend", "-m", "Rewritten")
        do {
            try await Git.forcePush(branch: "main", expected: pushed, in: root)
            XCTFail("main is protected")
        } catch {}
    }

    func testCountsAndListsOfCommits() async throws {
        let base = try await output("rev-parse", "HEAD")
        _ = try await commit("a.txt", "1\n", "One")
        _ = try await commit("a.txt", "2\n", "Two")
        let count = await Git.count("\(base)..HEAD", in: root)
        XCTAssertEqual(count, 2)
        let listed = await Git.commits("\(base)..HEAD", in: root)
        XCTAssertEqual(listed.map(\.subject), ["Two", "One"])
        let none = await Git.count("no-such-ref..HEAD", in: root)
        XCTAssertEqual(none, 0)
    }
}

final class MenuCommitsTests: XCTestCase {
    private func commit(_ hash: String, _ parents: [String]) -> Commit {
        Commit(hash: hash, parents: parents, author: "", email: "", date: Date(), refs: [], subject: hash)
    }

    @MainActor
    func testTheCommitsHeadHasAreFoundThroughTheirParents() {
        // d is HEAD, a merge of c (on the branch) and x (merged in); y is on another branch.
        let list = [commit("y", ["b"]), commit("d", ["c", "x"]), commit("x", ["a"]), commit("c", ["b"]), commit("b", ["a"]), commit("a", [])]
        XCTAssertEqual(AppStore.commitsInHead(list, head: "d"), ["d", "c", "x", "b", "a"])
        XCTAssertEqual(AppStore.commitsInHead(list, head: "y"), ["y", "b", "a"])
        XCTAssertNil(AppStore.commitsInHead(list, head: "elsewhere"), "HEAD not in the list: nothing can be told")
        XCTAssertNil(AppStore.commitsInHead(list, head: nil))
    }
}

final class AcknowledgementsTests: XCTestCase {
    func testEachEditionCreditsWhatItHasInside() {
        let saved = AppEnvironment.edition
        defer { AppEnvironment.edition = saved }
        AppEnvironment.edition = .direct
        XCTAssertEqual(Acknowledgements.included.map(\.name), ["SwiftTerm", "Sparkle"])
        AppEnvironment.edition = .appStore
        XCTAssertEqual(Acknowledgements.included.map(\.name), ["SwiftTerm"])
    }

    /// `DIF_BUILT_APP=/path/DiffCode.app swift test --filter Acknowledgements` checks a bundle the build script made.
    @MainActor
    func testABuiltAppCarriesTheLicensesItCredits() throws {
        guard let path = ProcessInfo.processInfo.environment["DIF_BUILT_APP"] else { throw XCTSkip("no built app given") }
        let bundle = try XCTUnwrap(Bundle(url: URL(fileURLWithPath: path)))
        for component in Acknowledgements.components {
            let text = try XCTUnwrap(Acknowledgements.licenseText(component, in: bundle), component.name)
            XCTAssertTrue(text.contains("Permission is hereby granted"), component.name)
        }
        let credits = Acknowledgements.credits(in: bundle).string
        XCTAssertTrue(credits.contains("SwiftTerm") && credits.contains("Miguel de Icaza"), credits.prefix(200).description)
        XCTAssertTrue(credits.contains("Sparkle") && credits.contains("Andy Matuschak"))
    }

    func testTheLicensesAreInThePackages() throws {
        // The build copies these into the app; they must exist where Scripts/build-app.sh looks.
        let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        for path in [".build/checkouts/SwiftTerm/LICENSE", ".build/artifacts/sparkle/Sparkle/LICENSE"] {
            let text = try String(contentsOf: root.appendingPathComponent(path), encoding: .utf8)
            XCTAssertTrue(text.contains("Permission is hereby granted"), path)
        }
    }
}
