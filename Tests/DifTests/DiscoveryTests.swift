import XCTest
@testable import DifKit

/// Which repositories a chosen folder adds (issue #5): a workspace that is a repository itself, with others in it.
final class DiscoveryTests: XCTestCase {
    private var base: URL!

    override func setUp() async throws {
        base = FileManager.default.temporaryDirectory.appendingPathComponent("dif-discovery-\(UUID().uuidString)")
            .resolvingSymlinksInPath()
        let workspace = base.appendingPathComponent("workspace")
        try FileManager.default.createDirectory(at: workspace.appendingPathComponent("docs"), withIntermediateDirectories: true)
        try git(workspace, "init", "-q", "-b", "main")
        try Data("api/\nweb/\nnode_modules/\nvendor/\n".utf8).write(to: workspace.appendingPathComponent(".gitignore"))
        try Data("# Docs\n".utf8).write(to: workspace.appendingPathComponent("docs/readme.md"))
        try git(workspace, "add", "-A")
        try git(workspace, "commit", "-q", "-m", "Workspace")
        // Ignored repositories, one not ignored below a tracked folder, one below an untracked folder.
        for path in ["api", "web", "docs/handbook", "tools/deep/thing"] { try repository(path) }
        // A repository inside a repository found, and one in a folder that one ignores.
        try Data("build/\n".utf8).write(to: workspace.appendingPathComponent("api/.gitignore"))
        for path in ["api/services/core", "api/build"] { try repository(path) }
        // Not looked for: below an ignored folder that is no repository, in a hidden folder, a linked work tree.
        for path in ["vendor/dependency", "node_modules/package", ".cache/clone"] { try repository(path) }
        try git(workspace, "worktree", "add", "-q", "linked", "-b", "linked")
    }

    override func tearDown() async throws {
        try? FileManager.default.removeItem(at: base)
    }

    private func repository(_ path: String) throws {
        let url = base.appendingPathComponent("workspace/" + path)
        try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        try git(url, "init", "-q", "-b", "main")
    }

    private func git(_ directory: URL, _ arguments: String...) throws {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: Git.executable)
        process.arguments = ["-c", "user.name=Dif Tests", "-c", "user.email=tests@example.invalid", "-c", "commit.gpgsign=false"] + arguments
        process.currentDirectoryURL = directory
        process.standardOutput = FileHandle.nullDevice
        process.standardError = FileHandle.nullDevice
        try process.run()
        process.waitUntilExit()
        XCTAssertEqual(process.terminationStatus, 0, "git \(arguments.joined(separator: " "))")
    }

    private func names(_ found: [URL]) -> [String] {
        let prefix = base.path + "/"
        return found.map { $0.resolvingSymlinksInPath().path }.map { $0.hasPrefix(prefix) ? String($0.dropFirst(prefix.count)) : $0 }
    }

    private let expected = ["workspace", "workspace/api", "workspace/api/build", "workspace/api/services/core",
                            "workspace/docs/handbook", "workspace/tools/deep/thing", "workspace/web"]

    func testARepositoryBringsTheRepositoriesInsideIt() async {
        let found = await Git.repositories(for: base.appendingPathComponent("workspace"))
        XCTAssertEqual(names(found), expected)
    }

    func testAFolderInsideTheRepositoryAddsTheSame() async {
        let found = await Git.repositories(for: base.appendingPathComponent("workspace/docs"))
        XCTAssertEqual(names(found), expected)
    }

    func testAPlainFolderLooksInsideTheRepositoriesItFinds() async {
        let found = await Git.repositories(for: base)
        XCTAssertEqual(names(found), expected)
    }

    func testARepositoryWithNothingInsideIsAddedAlone() async {
        let found = await Git.repositories(for: base.appendingPathComponent("workspace/web"))
        XCTAssertEqual(names(found), ["workspace/web"])
    }
}
