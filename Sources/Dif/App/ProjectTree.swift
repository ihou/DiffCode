import Foundation

/// The projects as a tree: a repository inside another one listed sits under it, and so does a linked work tree
/// under its repository, wherever its folder is. Worked out from the paths and from what git says of work trees,
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

    /// For each path, the index of the nearest listed folder that contains it; a work tree's repository
    /// (`owners`, by index) comes before where its folder is.
    static func parents(_ paths: [String], owners: [Int?] = []) -> [Int?] {
        paths.indices.map { i in
            if i < owners.count, let owner = owners[i], owner != i { return owner }
            var best: Int?
            for j in paths.indices where j != i && paths[i].hasPrefix(paths[j] + "/") {
                if best == nil || paths[j].count > paths[best!].count { best = j }
            }
            return best
        }
    }

    /// The rows to show, top to bottom: each project followed by its work trees and then the repositories inside it.
    /// What is inside a collapsed project is left out.
    static func rows(paths: [String], owners: [Int?] = [], collapsed: Set<String> = []) -> [Row] {
        let parents = parents(paths, owners: owners)
        var children: [Int?: [Int]] = [:]
        for (index, parent) in parents.enumerated() { children[parent, default: []].append(index) }
        // A repository's own work trees first, then the repositories inside it; each kind in the list's order.
        func isWorktree(_ index: Int) -> Bool { index < owners.count && owners[index] != nil }
        for (parent, inside) in children where parent != nil {
            children[parent] = inside.filter(isWorktree) + inside.filter { !isWorktree($0) }
        }
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

    /// Everything under a project, however deep (indices), given each one's parent.
    static func descendants(of index: Int, parents: [Int?]) -> [Int] {
        parents.indices.filter { candidate in
            var at = parents[candidate], steps = 0
            while let up = at, steps <= parents.count {
                if up == index { return true }
                at = parents[up]
                steps += 1
            }
            return false
        }
    }

    /// The projects above one, nearest first.
    static func ancestors(of index: Int, parents: [Int?]) -> [Int] {
        var found: [Int] = []
        var at = parents[index]
        while let up = at, found.count <= parents.count {
            found.append(up)
            at = parents[up]
        }
        return found
    }
}
