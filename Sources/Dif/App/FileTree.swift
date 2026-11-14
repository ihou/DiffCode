import Foundation

/// One visible row of the project file tree.
struct TreeRow: Identifiable, Equatable {
    var id: String
    var path: String
    /// A file's name, or a directory's label. Chains of single-child directories share one row ("app/src/main").
    var name: String
    var depth: Int
    var isDirectory: Bool
    var isExpanded = false
    /// The file's local change, if it has one.
    var change: FileChange?
    /// Changed files somewhere below this directory.
    var changedBelow = 0
    /// Characters the filter matched, as byte offsets into `path`.
    var positions: [Int] = []

    static func fileID(_ path: String) -> String { "f:" + path }
    static func directoryID(_ path: String) -> String { "d:" + path }

    /// The path of the row this one hangs under ("" at the top level).
    var parentPath: String {
        path.count > name.count ? String(path.dropLast(name.count + 1)) : ""
    }
}

/// A repository's files arranged as a tree. Built once per file listing, off the main thread;
/// directories are sorted lazily, the first time they are shown.
final class FileTreeIndex: @unchecked Sendable {
    final class Directory {
        let name: String
        let path: String
        fileprivate(set) var directories: [Directory] = []
        /// Indices into the index's `paths`.
        fileprivate(set) var files: [Int32] = []
        fileprivate var isSorted = false

        fileprivate init(name: String, path: String) {
            self.name = name
            self.path = path
        }
    }

    let paths: [String]
    let root = Directory(name: "", path: "")
    private var byPath: [String: Directory] = [:]

    init(paths: [String]) {
        self.paths = paths
        byPath[""] = root
        // `git ls-files` lists a directory's files together, so remembering the last directory skips most lookups.
        var lastPath = ""
        var last = root
        for (index, path) in paths.enumerated() {
            let directoryPath = path.lastIndex(of: "/").map { String(path[..<$0]) } ?? ""
            if directoryPath != lastPath {
                last = ensure(directoryPath)
                lastPath = directoryPath
            }
            last.files.append(Int32(index))
        }
    }

    private func ensure(_ path: String) -> Directory {
        if let existing = byPath[path] { return existing }
        let parent: Directory
        let name: String
        if let slash = path.lastIndex(of: "/") {
            parent = ensure(String(path[..<slash]))
            name = String(path[path.index(after: slash)...])
        } else {
            parent = root
            name = path
        }
        let directory = Directory(name: name, path: path)
        parent.directories.append(directory)
        byPath[path] = directory
        return directory
    }

    func directory(at path: String) -> Directory? { byPath[path] }

    static func name(of path: String) -> Substring {
        path.lastIndex(of: "/").map { path[path.index(after: $0)...] } ?? path[...]
    }

    /// Directories first, then files, each in Finder-like order. Main thread only.
    fileprivate func sort(_ directory: Directory) {
        guard !directory.isSorted else { return }
        directory.isSorted = true
        directory.directories.sort { $0.name.localizedStandardCompare($1.name) == .orderedAscending }
        if directory.files.count > 1 {
            let keyed = directory.files.map { (index: $0, name: String(Self.name(of: paths[Int($0)]))) }
            directory.files = keyed.sorted { $0.name.localizedStandardCompare($1.name) == .orderedAscending }.map(\.index)
        }
    }
}

/// Flattens the file tree into the rows that are currently visible.
enum FileTree {
    /// - Parameter expanded: paths of the directories that are open.
    static func rows(index: FileTreeIndex, expanded: Set<String>, changes: [FileChange]) -> [TreeRow] {
        var byPath: [String: FileChange] = [:]
        var below: [String: Int] = [:]
        for change in changes {
            byPath[change.path] = change
            var path = change.path[...]
            while let slash = path.lastIndex(of: "/") {
                path = path[..<slash]
                below[String(path), default: 0] += 1
            }
        }
        var rows: [TreeRow] = []
        emit(index.root, depth: 0, index: index, expanded: expanded, changes: byPath, below: below, into: &rows)
        return rows
    }

    private static func emit(_ directory: FileTreeIndex.Directory, depth: Int, index: FileTreeIndex, expanded: Set<String>,
                             changes: [String: FileChange], below: [String: Int], into rows: inout [TreeRow]) {
        index.sort(directory)
        for child in directory.directories {
            // Fold "a" → "b" → "c" into one row when the middle directories hold nothing else.
            var node = child
            var label = child.name
            while node.files.isEmpty, node.directories.count == 1, let only = node.directories.first {
                label += "/" + only.name
                node = only
            }
            let open = expanded.contains(node.path)
            rows.append(TreeRow(id: TreeRow.directoryID(node.path), path: node.path, name: label, depth: depth, isDirectory: true,
                                isExpanded: open, changedBelow: below[node.path] ?? 0))
            if open {
                emit(node, depth: depth + 1, index: index, expanded: expanded, changes: changes, below: below, into: &rows)
            }
        }
        for file in directory.files {
            let path = index.paths[Int(file)]
            rows.append(TreeRow(id: TreeRow.fileID(path), path: path, name: String(FileTreeIndex.name(of: path)), depth: depth,
                                isDirectory: false, change: changes[path]))
        }
    }

    /// The flat list shown while a filter is typed: best fuzzy matches first.
    static func rows(matches: [Fuzzy.Ranked], changes: [FileChange]) -> [TreeRow] {
        let byPath = Dictionary(changes.map { ($0.path, $0) }, uniquingKeysWith: { first, _ in first })
        return matches.map { match in
            TreeRow(id: TreeRow.fileID(match.path), path: match.path, name: String(FileTreeIndex.name(of: match.path)), depth: 0,
                    isDirectory: false, change: byPath[match.path], positions: match.positions)
        }
    }

    /// Every directory that must be open for `path` to be visible: "a", "a/b" for "a/b/File.kt".
    static func ancestors(of path: String) -> [String] {
        var result: [String] = []
        var rest = path[...]
        while let slash = rest.lastIndex(of: "/") {
            rest = rest[..<slash]
            result.append(String(rest))
        }
        return result.reversed()
    }

    /// The directory rows above `index` that enclose it, outermost first.
    static func enclosing(_ rows: [TreeRow], at index: Int) -> [TreeRow] {
        guard index >= 0, index < rows.count else { return [] }
        var chain: [TreeRow] = []
        var depth = rows[index].depth
        var cursor = index - 1
        while depth > 0, cursor >= 0 {
            if rows[cursor].depth < depth {
                chain.append(rows[cursor])
                depth = rows[cursor].depth
            }
            cursor -= 1
        }
        return chain.reversed()
    }
}
