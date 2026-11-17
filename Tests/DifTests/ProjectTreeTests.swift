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
        XCTAssertEqual(ProjectTree.descendants(of: 0, in: paths), [2, 3, 4])
    }

    func testWithoutItsParentARepositoryMovesUpALevel() {
        let rows = ProjectTree.rows(paths: ["/w/ws", "/w/ws/api/services/core"])
        XCTAssertEqual(rows.map(\.depth), [0, 1])
    }
}
