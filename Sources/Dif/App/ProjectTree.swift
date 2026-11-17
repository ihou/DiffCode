import Foundation

/// The projects as a tree: a repository inside another one listed sits under it. Worked out from the paths alone,
/// so the list stays a plain list of folders and nothing has to be kept in step.
enum ProjectTree {
    struct Row: Equatable {
        /// Where the project is in the list the rows were made from.
        var index: Int
        var depth: Int
        var hasChildren: Bool
        /// The project it sits under, if any (an index like `index`).
        var parent: Int?
    }

    /// For each path, the index of the nearest listed folder that contains it.
    static func parents(_ paths: [String]) -> [Int?] {
        paths.indices.map { i in
            var best: Int?
            for j in paths.indices where j != i && paths[i].hasPrefix(paths[j] + "/") {
                if best == nil || paths[j].count > paths[best!].count { best = j }
            }
            return best
        }
    }

    /// The rows to show, top to bottom: each project followed by those inside it, in the list's own order.
    /// What is inside a collapsed project is left out.
    static func rows(paths: [String], collapsed: Set<String> = []) -> [Row] {
        let parents = parents(paths)
        var children: [Int?: [Int]] = [:]
        for (index, parent) in parents.enumerated() { children[parent, default: []].append(index) }
        var rows: [Row] = []
        func add(_ index: Int, depth: Int) {
            let inside = children[index] ?? []
            rows.append(Row(index: index, depth: depth, hasChildren: !inside.isEmpty, parent: parents[index]))
            guard !collapsed.contains(paths[index]) else { return }
            for child in inside { add(child, depth: depth + 1) }
        }
        for root in children[nil] ?? [] { add(root, depth: 0) }
        return rows
    }

    /// Everything under a path, however deep (indices).
    static func descendants(of index: Int, in paths: [String]) -> [Int] {
        paths.indices.filter { $0 != index && paths[$0].hasPrefix(paths[index] + "/") }
    }
}
