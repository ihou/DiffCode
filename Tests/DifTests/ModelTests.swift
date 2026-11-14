import XCTest
@testable import DifKit

final class DiffDocumentTests: XCTestCase {
    /// Ten old lines; two of them become three, and two more are inserted further down.
    private func makeDocument() -> DiffDocument {
        let left = TextBuffer(lines: (0..<10).map { "L\($0)" })
        let right = TextBuffer(lines: (0..<13).map { "R\($0)" })
        return DiffDocument(left: left, right: right, hunks: [
            Hunk(leftStart: 2, leftCount: 2, rightStart: 2, rightCount: 3),
            Hunk(leftStart: 7, leftCount: 0, rightStart: 8, rightCount: 2),
        ], language: .detect(path: "x.txt"))
    }

    func testSegmentsCoverBothSidesWithoutPadding() {
        let doc = makeDocument()
        XCTAssertEqual(doc.segments.map(\.virtualStart), [0, 2, 5, 8, 10])
        XCTAssertEqual(doc.segments.map(\.hunk), [-1, 0, -1, 1, -1])
        XCTAssertEqual(doc.virtualLineCount, 13)
        XCTAssertEqual(doc.additions, 5)
        XCTAssertEqual(doc.deletions, 2)
    }

    func testUnchangedLinesStayLevelAcrossTheDivider() {
        let doc = makeDocument()
        // Inside unchanged runs both panes advance one line per virtual line.
        for v in [0.0, 1.0, 5.0, 6.5, 10.0, 12.0] {
            let p = doc.panePositions(forVirtual: v)
            let l = doc.virtualPosition(of: Int(p.left), side: .left)
            let r = doc.virtualPosition(of: Int(p.right), side: .right)
            XCTAssertEqual(l, r, accuracy: 0.001, "virtual \(v)")
        }
    }

    func testShorterSideScrollsSlowerInsideAHunk() {
        let doc = makeDocument()
        let start = doc.panePositions(forVirtual: 2)
        XCTAssertEqual(start.left, 2)
        XCTAssertEqual(start.right, 2)
        let middle = doc.panePositions(forVirtual: 3.5)
        XCTAssertEqual(middle.left, 3, accuracy: 0.001)   // 2 lines over a span of 3
        XCTAssertEqual(middle.right, 3.5, accuracy: 0.001)
        // A pure insertion holds the left pane still while the right one passes it.
        XCTAssertEqual(doc.panePositions(forVirtual: 8).left, 7)
        XCTAssertEqual(doc.panePositions(forVirtual: 9.9).left, 7)
        XCTAssertEqual(doc.panePositions(forVirtual: 9).right, 9, accuracy: 0.001)
    }

    func testVirtualPositionOfLines() {
        let doc = makeDocument()
        XCTAssertEqual(doc.virtualPosition(of: 4, side: .left), 5)
        XCTAssertEqual(doc.virtualPosition(of: 5, side: .right), 5)
        XCTAssertEqual(doc.virtualPosition(of: 8, side: .right), 8)
        XCTAssertEqual(doc.virtualPosition(of: 9, side: .left), 12)
        XCTAssertEqual(doc.virtualStart(ofHunk: 1), 8)
    }

    func testUnifiedRows() {
        let doc = makeDocument()
        let kinds = doc.unifiedRows.map(\.kind)
        XCTAssertEqual(kinds.count, 15)
        XCTAssertEqual(Array(kinds[0..<7]), [.context, .context, .removed, .removed, .added, .added, .added])
        XCTAssertEqual(doc.unifiedRow(ofHunk: 1), 10)
        XCTAssertEqual(doc.unifiedRow(ofLine: 4, side: .left), 7)
        XCTAssertEqual(doc.unifiedRow(ofLine: 8, side: .right), 10)
        XCTAssertEqual(doc.unifiedRow(ofLine: 3, side: .left), 3, "a removed line has a row of its own")
    }

    func testHunksOutsideTheFileAreClampedAway() {
        let buffer = TextBuffer(lines: ["a", "b"])
        let doc = DiffDocument(left: buffer, right: buffer, hunks: [
            Hunk(leftStart: 50, leftCount: 3, rightStart: 50, rightCount: 0),
        ], language: .detect(path: "x.txt"))
        XCTAssertTrue(doc.hunks.isEmpty)
        XCTAssertEqual(doc.virtualLineCount, 2)
    }

    func testWidestLineCountsTabsAndWideCharacters() {
        XCTAssertEqual(TextBuffer(lines: ["ab", "\tx", "中文"]).maxColumns, 5)
    }
}

final class FuzzyTests: XCTestCase {
    private let paths = [
        "app/src/main/java/com/example/shop/ui/CartViewModel.kt",
        "app/src/main/java/com/example/shop/data/CartRepository.java",
        "cart/README.md",
        "docs/changelog.md",
    ]

    func testNonSubsequencesAreExcluded() {
        XCTAssertTrue(Fuzzy.rank(query: "zzz", in: paths, limit: 10).isEmpty)
        XCTAssertNil(Fuzzy.match(query: Array("xq".utf8), candidate: "CartViewModel.kt", wantPositions: false))
    }

    func testCamelHumpsFindTheFile() throws {
        let top = try XCTUnwrap(Fuzzy.rank(query: "cvm", in: paths, limit: 10).first)
        XCTAssertEqual(top.path, paths[0])
        XCTAssertEqual(top.positions.count, 3)
        let name = Array(top.path.utf8)
        XCTAssertEqual(top.positions.map { name[$0] }, Array("CVM".utf8), "the humps are what gets highlighted")
    }

    func testFileNameBeatsDirectoryName() throws {
        let ranked = Fuzzy.rank(query: "cart", in: paths, limit: 10).map(\.path)
        let inName = try XCTUnwrap(ranked.firstIndex(of: paths[1]))
        let inDirectory = try XCTUnwrap(ranked.firstIndex(of: "cart/README.md"))
        XCTAssertLessThan(inName, inDirectory)
    }

    func testQueryIsCaseAndSpaceInsensitive() {
        XCTAssertEqual(Fuzzy.rank(query: "Cart Repo", in: paths, limit: 1).first?.path, paths[1])
    }

    func testLimit() {
        XCTAssertEqual(Fuzzy.rank(query: "a", in: paths, limit: 2).count, 2)
    }
}

final class ChangeTreeTests: XCTestCase {
    private let changes = [
        FileChange(path: "app/src/main/Cart.kt", oldPath: nil, kind: .modified),
        FileChange(path: "app/src/main/ui/Screen.kt", oldPath: nil, kind: .modified),
        FileChange(path: "docs/guide.md", oldPath: nil, kind: .deleted),
        FileChange(path: "notes.txt", oldPath: nil, kind: .untracked),
    ]

    func testUnversionedFilesGetTheirOwnGroup() {
        let rows = ChangeTree.rows(changes: changes, flat: false, collapsed: [], filter: "", splitUnversioned: true)
        XCTAssertEqual(rows.first?.id, "g:changes")
        XCTAssertTrue(rows.contains { $0.id == "g:unversioned" })
        let files = rows.compactMap(\.file).map(\.path)
        XCTAssertEqual(Set(files), Set(changes.map(\.path)))
        XCTAssertEqual(files.last, "notes.txt")
    }

    func testCollapsingAGroupHidesItsRows() {
        let rows = ChangeTree.rows(changes: changes, flat: false, collapsed: ["g:changes"], filter: "", splitUnversioned: true)
        XCTAssertEqual(rows.compactMap(\.file).map(\.path), ["notes.txt"])
        XCTAssertEqual(rows.first?.isExpanded, false)
    }

    func testFilterAndFlatList() {
        let rows = ChangeTree.rows(changes: changes, flat: true, collapsed: [], filter: "  SCREEN ", splitUnversioned: true)
        XCTAssertEqual(rows.compactMap(\.file).map(\.path), ["app/src/main/ui/Screen.kt"])
        XCTAssertFalse(rows.contains { if case .directory = $0.kind { return true } else { return false } })
    }

    func testKeyboardOrderMatchesWhatIsDrawn() {
        let rows = ChangeTree.rows(changes: changes, flat: false, collapsed: [], filter: "", splitUnversioned: true)
        let ordered = ChangeTree.orderedFiles(changes: changes, flat: false, filter: "", splitUnversioned: true)
        XCTAssertEqual(ordered.map(\.path), rows.compactMap(\.file).map(\.path))
    }
}

final class ListSelectionTests: XCTestCase {
    private let ids = ["a", "b", "c", "d", "e"]

    func testShiftSelectsEverythingBetweenTheAnchorAndTheTarget() {
        XCTAssertEqual(ListSelection.range(in: ids, from: "b", to: "d"), ["b", "c", "d"])
        XCTAssertEqual(ListSelection.range(in: ids, from: "d", to: "a"), ["a", "b", "c", "d"])
        XCTAssertEqual(ListSelection.range(in: ids, from: "c", to: "c"), ["c"])
        // No anchor, or one that has left the list: just the row that was clicked.
        XCTAssertEqual(ListSelection.range(in: ids, from: nil, to: "d"), ["d"])
        XCTAssertEqual(ListSelection.range(in: ids, from: "gone", to: "d"), ["d"])
        XCTAssertEqual(ListSelection.range(in: ids, from: "a", to: "gone"), [])
    }

    func testCommandClickAddsARowAndTakesItOutAgain() {
        let added = ListSelection.toggle("d", in: ["a"], ids: ids)
        XCTAssertEqual(added.selected, ["a", "d"])
        XCTAssertEqual(added.primary, "d")
        // Taking the main row out hands that role to the lowest row still selected.
        let removed = ListSelection.toggle("d", in: ["a", "c", "d"], ids: ids)
        XCTAssertEqual(removed.selected, ["a", "c"])
        XCTAssertEqual(removed.primary, "c")
        let emptied = ListSelection.toggle("a", in: ["a"], ids: ids)
        XCTAssertEqual(emptied.selected, [])
        XCTAssertNil(emptied.primary)
    }
}

final class ChangeSelectionTests: XCTestCase {
    private let changes = [
        FileChange(path: "app/src/Cart.kt", oldPath: nil, kind: .modified),
        FileChange(path: "app/src/ui/Screen.kt", oldPath: nil, kind: .added),
        FileChange(path: "app/src/ui/Draft.kt", oldPath: nil, kind: .untracked),
        FileChange(path: "app/srcs/Other.kt", oldPath: nil, kind: .deleted),
        FileChange(path: "notes.txt", oldPath: nil, kind: .untracked),
    ]

    private func paths(_ ids: Set<String>) -> [String] { ChangeTree.files(for: ids, in: changes).map(\.path) }

    func testAFileRowStandsForItself() {
        XCTAssertEqual(paths([ChangeRow.fileID("notes.txt")]), ["notes.txt"])
        XCTAssertEqual(paths([ChangeRow.fileID("gone.txt")]), [])
        XCTAssertEqual(paths([]), [])
    }

    func testAGroupRowStandsForEveryFileOfItsKind() {
        XCTAssertEqual(paths(["g:changes"]), ["app/src/Cart.kt", "app/src/ui/Screen.kt", "app/srcs/Other.kt"])
        XCTAssertEqual(paths(["g:unversioned"]), ["app/src/ui/Draft.kt", "notes.txt"])
    }

    func testAFolderRowStandsForTheFilesBelowItInItsOwnGroup() {
        // "app/src" must not take "app/srcs/…" with it, nor the untracked file that sits in the same folder.
        XCTAssertEqual(paths(["d:changes:app/src"]), ["app/src/Cart.kt", "app/src/ui/Screen.kt"])
        XCTAssertEqual(paths(["d:unversioned:app/src/ui"]), ["app/src/ui/Draft.kt"])
        XCTAssertEqual(paths(["d:changes:app/src/ui", ChangeRow.fileID("notes.txt")]), ["app/src/ui/Screen.kt", "notes.txt"])
    }

    func testEachFileIsListedOnceHoweverManyRowsCoverIt() {
        XCTAssertEqual(paths(["g:changes", "d:changes:app/src", ChangeRow.fileID("app/src/Cart.kt")]),
                       ["app/src/Cart.kt", "app/src/ui/Screen.kt", "app/srcs/Other.kt"])
    }

    func testOnlyTrackedChangesRollBackAndOnlyNewFilesAreDeleted() {
        XCTAssertEqual(changes.filter(AppStore.canRollback).map(\.path), ["app/src/Cart.kt", "app/src/ui/Screen.kt", "app/srcs/Other.kt"])
        XCTAssertEqual(changes.filter(AppStore.canDelete).map(\.path), ["app/src/ui/Screen.kt", "app/src/ui/Draft.kt", "notes.txt"])
        var submodule = FileChange(path: "vendor/lib", oldPath: nil, kind: .modified)
        submodule.isSubmodule = true
        XCTAssertFalse(AppStore.canRollback(submodule))
    }
}
