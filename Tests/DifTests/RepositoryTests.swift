import XCTest
@testable import DifKit

/// End-to-end checks against a real repository built in a temporary directory.
final class RepositoryTests: XCTestCase {
    private var root: URL!

    override func setUp() async throws {
        root = FileManager.default.temporaryDirectory.appendingPathComponent("dif-tests-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: root.appendingPathComponent("src"), withIntermediateDirectories: true)
        try git("init", "-q", "-b", "main")
        try write("src/Cart.kt", (1...20).map { "val line\($0) = \($0)" }.joined(separator: "\n") + "\n")
        try write("src/Legacy.kt", "class Legacy\n")
        try write("docs.md", "# Docs\n\nSome text that stays the same.\nMore text.\n")
        try git("add", "-A")
        try git("commit", "-q", "-m", "Initial import")

        // One of each: an edit, a deletion, a rename, an untracked file.
        try write("src/Cart.kt", (1...20).map { $0 == 5 ? "val line5 = 500" : "val line\($0) = \($0)" }.joined(separator: "\n")
                  + "\nval extra = true\n")
        try FileManager.default.removeItem(at: root.appendingPathComponent("src/Legacy.kt"))
        try git("mv", "docs.md", "guide.md")
        try write("notes with space.txt", "todo\n")
    }

    override func tearDown() async throws {
        try? FileManager.default.removeItem(at: root)
    }

    private func write(_ path: String, _ text: String) throws {
        try Data(text.utf8).write(to: root.appendingPathComponent(path))
    }

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

    func testRepositoryRootIsFoundFromASubdirectory() async {
        let found = await Git.repositoryRoot(for: root.appendingPathComponent("src"))
        XCTAssertEqual(found?.resolvingSymlinksInPath().path, root.resolvingSymlinksInPath().path)
        let outside = await Git.repositoryRoot(for: FileManager.default.temporaryDirectory)
        XCTAssertNil(outside)
    }

    func testStatusReportsEveryKindOfChangeWithLineCounts() async throws {
        let status = try await Git.status(in: root)
        XCTAssertEqual(status.branch, "main")
        let byPath = Dictionary(uniqueKeysWithValues: status.changes.map { ($0.path, $0) })
        XCTAssertEqual(byPath["src/Cart.kt"]?.kind, .modified)
        XCTAssertEqual(byPath["src/Cart.kt"]?.additions, 2)
        XCTAssertEqual(byPath["src/Cart.kt"]?.deletions, 1)
        XCTAssertEqual(byPath["src/Legacy.kt"]?.kind, .deleted)
        XCTAssertEqual(byPath["guide.md"]?.kind, .renamed)
        XCTAssertEqual(byPath["guide.md"]?.oldPath, "docs.md")
        XCTAssertEqual(byPath["notes with space.txt"]?.kind, .untracked)
        XCTAssertEqual(status.changes.count, 4)
    }

    func testWorktreeDiffHasTheExpectedHunks() async throws {
        let doc = await DiffLoader.load(DiffRequest(root: root, path: "src/Cart.kt", oldPath: nil, kind: .modified,
                                                    source: .worktree(base: "HEAD")))
        guard case .text = doc.content else { return XCTFail("expected a text diff") }
        XCTAssertEqual(doc.left.lines.count, 20)
        XCTAssertEqual(doc.right.lines.count, 21)
        XCTAssertEqual(doc.hunks.map(\.kind), [.modified, .added])
        XCTAssertEqual(doc.hunks[0].leftStart, 4)
        XCTAssertEqual(doc.hunks[0].rightInline, [[12..<15]], "only the number changed on line 5")
        XCTAssertEqual(doc.hunks[1].rightStart, 20)
        XCTAssertEqual(doc.right.lines[20], "val extra = true")
    }

    func testDeletedAndUntrackedFilesDiffAgainstNothing() async {
        let deleted = await DiffLoader.load(DiffRequest(root: root, path: "src/Legacy.kt", oldPath: nil, kind: .deleted,
                                                        source: .worktree(base: "HEAD")))
        XCTAssertEqual(deleted.left.lines, ["class Legacy"])
        XCTAssertTrue(deleted.right.lines.isEmpty)
        XCTAssertEqual(deleted.hunks.map(\.kind), [.deleted])

        let untracked = await DiffLoader.load(DiffRequest(root: root, path: "notes with space.txt", oldPath: nil, kind: .untracked,
                                                          source: .worktree(base: "HEAD")))
        XCTAssertTrue(untracked.left.lines.isEmpty)
        XCTAssertEqual(untracked.right.lines, ["todo"])
        XCTAssertEqual(untracked.hunks.map(\.kind), [.added])
    }

    func testRenameIsComparedAgainstItsOldPath() async {
        let doc = await DiffLoader.load(DiffRequest(root: root, path: "guide.md", oldPath: "docs.md", kind: .renamed,
                                                    source: .worktree(base: "HEAD")))
        XCTAssertEqual(doc.left.lines, doc.right.lines)
        XCTAssertTrue(doc.hunks.isEmpty)
    }

    func testLogAndCommitFiles() async throws {
        let commits = try await Git.log(in: root, skip: 0, limit: 10, query: "", allBranches: false)
        XCTAssertEqual(commits.map(\.subject), ["Initial import"])
        let files = try await Git.commitFiles(commits[0], in: root)
        XCTAssertEqual(Set(files.map(\.path)), ["docs.md", "src/Cart.kt", "src/Legacy.kt"])
        XCTAssertTrue(files.allSatisfy { $0.kind == .added })
    }

    func testGrepSearchesTrackedAndUntrackedFiles() async throws {
        final class Collector: @unchecked Sendable { var hits: [GrepHit] = [] }
        let collector = Collector()
        try await Git.grep("todo", options: GrepOptions(), in: root, limit: 100) { collector.hits += $0 }
        XCTAssertEqual(collector.hits.map(\.path), ["notes with space.txt"])
        XCTAssertEqual(collector.hits.first?.line, 1)
    }

    func testViewerNeverTakesTheIndexLock() async throws {
        // A stale lock makes any index-writing git command fail; read-only use must not care.
        let lock = root.appendingPathComponent(".git/index.lock")
        try Data().write(to: lock)
        defer { try? FileManager.default.removeItem(at: lock) }
        let status = try await Git.status(in: root)
        XCTAssertEqual(status.changes.count, 4)
        XCTAssertTrue(FileManager.default.fileExists(atPath: lock.path), "the lock was left exactly as found")
    }
}
