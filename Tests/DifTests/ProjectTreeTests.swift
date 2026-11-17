import XCTest
@testable import DifKit

final class ProjectTreeTests: XCTestCase {
    private let paths = ["/w/ws", "/w/other", "/w/ws/api", "/w/ws/web", "/w/ws/api/services/core", "/w/ws-2"]

    private func shown(_ collapsed: Set<String> = []) -> [String] {
        ProjectTree.rows(paths: paths, collapsed: collapsed).map { String(repeating: "  ", count: $0.depth) + paths[$0.index] }
    }

    func testARepositoryInsideAnotherSitsUnderIt() {
        XCTAssertEqual(shown(), ["/w/ws", "  /w/ws/api", "    /w/ws/api/services/core", "  /w/ws/web", "/w/other", "/w/ws-2"])
        // A name that merely starts the same is no parent, and the nearest folder is.
        XCTAssertEqual(ProjectTree.parents(paths), [nil, nil, 0, 0, 2, nil])
    }

    func testACollapsedProjectHidesWhatIsInside() {
        XCTAssertEqual(shown(["/w/ws/api"]), ["/w/ws", "  /w/ws/api", "  /w/ws/web", "/w/other", "/w/ws-2"])
        XCTAssertEqual(shown(["/w/ws"]), ["/w/ws", "/w/other", "/w/ws-2"])
        let rows = ProjectTree.rows(paths: paths, collapsed: ["/w/ws"])
        XCTAssertTrue(rows[0].hasChildren)
        XCTAssertFalse(rows[1].hasChildren)
        XCTAssertEqual(ProjectTree.descendants(of: 0, parents: ProjectTree.parents(paths)), [2, 3, 4])
        XCTAssertEqual(ProjectTree.ancestors(of: 4, parents: ProjectTree.parents(paths)), [2, 0])
    }

    /// A linked work tree sits under its repository wherever its folder is: beside it, or inside another project.
    func testAWorkTreeSitsUnderItsRepository() {
        let paths = ["/w/ws", "/w/ws/api", "/elsewhere/fix-login", "/w/ws/api/.claude/worktrees/task"]
        let owners: [Int?] = [nil, nil, 1, 0]
        XCTAssertEqual(ProjectTree.parents(paths, owners: owners), [nil, 0, 1, 0])
        // A repository's work trees come before the repositories inside it.
        XCTAssertEqual(ProjectTree.rows(paths: paths, owners: owners).map(\.index), [0, 3, 1, 2])
        XCTAssertEqual(ProjectTree.rows(paths: paths, owners: owners, collapsed: ["/w/ws/api"]).map(\.index), [0, 3, 1])
        XCTAssertEqual(ProjectTree.descendants(of: 0, parents: ProjectTree.parents(paths, owners: owners)), [1, 2, 3])
    }

    func testWorkTreesAreReadFromGit() {
        let text = "worktree /w/ws\nHEAD 4d15\nbranch refs/heads/main\n\nworktree /w/ws-fix\nHEAD 4d15\nbranch refs/heads/fix\n\n"
            + "worktree /w/gone\nHEAD 4d15\ndetached\nprunable gitdir file points to non-existent location\n\nworktree /w/bare\nbare\n"
        XCTAssertEqual(Git.parseWorktrees(text), [
            Git.Worktree(path: "/w/ws", branch: "refs/heads/main"),
            Git.Worktree(path: "/w/ws-fix", branch: "refs/heads/fix"),
            Git.Worktree(path: "/w/gone", isPrunable: true),
            Git.Worktree(path: "/w/bare", isBare: true),
        ])
    }

    func testWithoutItsParentARepositoryMovesUpALevel() {
        let rows = ProjectTree.rows(paths: ["/w/ws", "/w/ws/api/services/core"])
        XCTAssertEqual(rows.map(\.depth), [0, 1])
    }
}
