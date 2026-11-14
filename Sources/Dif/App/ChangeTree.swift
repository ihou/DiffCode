import Foundation

/// One visible row of a change list: a group header, a directory, or a file.
struct ChangeRow: Identifiable, Equatable {
    enum Kind: Equatable {
        case group(title: String, count: Int)
        case directory(name: String, count: Int)
        case file(FileChange)
    }

    var id: String
    var depth: Int
    var kind: Kind
    var isExpanded = true

    var file: FileChange? {
        if case .file(let change) = kind { return change }
        return nil
    }

    static func fileID(_ path: String) -> String { "f:" + path }
}

/// Builds the flattened, collapsible tree shown in the Changes and commit-files lists.
enum ChangeTree {
    private final class Node {
        var children: [String: Node] = [:]
        var files: [FileChange] = []
        var total = 0
    }

    /// - Parameters:
    ///   - splitUnversioned: show untracked files under their own "Unversioned Files" header, as IntelliJ does.
    static func rows(changes: [FileChange], flat: Bool, collapsed: Set<String>, filter: String,
                     splitUnversioned: Bool) -> [ChangeRow] {
        let needle = filter.trimmingCharacters(in: .whitespaces).lowercased()
        let visible = needle.isEmpty ? changes : changes.filter { $0.path.lowercased().contains(needle) }

        guard splitUnversioned else {
            return build(visible, group: "c", depth: 0, flat: flat, collapsed: collapsed)
        }
        let tracked = visible.filter { $0.kind != .untracked }
        let untracked = visible.filter { $0.kind == .untracked }
        var rows: [ChangeRow] = []
        for (key, title, list) in [("changes", L("Changes", "变更"), tracked),
                                   ("unversioned", L("Unversioned Files", "未跟踪文件"), untracked)] where !list.isEmpty {
            let id = "g:" + key
            let expanded = !collapsed.contains(id)
            rows.append(ChangeRow(id: id, depth: 0, kind: .group(title: title, count: list.count), isExpanded: expanded))
            if expanded {
                rows += build(list, group: key, depth: 1, flat: flat, collapsed: collapsed)
            }
        }
        return rows
    }

    private static func build(_ changes: [FileChange], group: String, depth: Int, flat: Bool, collapsed: Set<String>) -> [ChangeRow] {
        if flat {
            return changes
                .sorted { $0.name.localizedStandardCompare($1.name) == .orderedAscending }
                .map { ChangeRow(id: ChangeRow.fileID($0.path), depth: depth, kind: .file($0)) }
        }
        let root = Node()
        for change in changes {
            var node = root
            node.total += 1
            let parts = change.path.split(separator: "/")
            for part in parts.dropLast() {
                let key = String(part)
                let child = node.children[key] ?? Node()
                node.children[key] = child
                child.total += 1
                node = child
            }
            node.files.append(change)
        }
        var rows: [ChangeRow] = []
        emit(root, path: "", group: group, depth: depth, collapsed: collapsed, into: &rows)
        return rows
    }

    private static func emit(_ node: Node, path: String, group: String, depth: Int, collapsed: Set<String>, into rows: inout [ChangeRow]) {
        let names = node.children.keys.sorted { $0.localizedStandardCompare($1) == .orderedAscending }
        for name in names {
            guard var child = node.children[name] else { continue }
            // Compact chains of single-child directories into one row ("app/src/main").
            var label = name
            var childPath = path.isEmpty ? name : path + "/" + name
            while child.files.isEmpty, child.children.count == 1, let (onlyName, only) = child.children.first {
                label += "/" + onlyName
                childPath += "/" + onlyName
                child = only
            }
            let id = "d:\(group):\(childPath)"
            let expanded = !collapsed.contains(id)
            rows.append(ChangeRow(id: id, depth: depth, kind: .directory(name: label, count: child.total), isExpanded: expanded))
            if expanded {
                emit(child, path: childPath, group: group, depth: depth + 1, collapsed: collapsed, into: &rows)
            }
        }
        for file in node.files.sorted(by: { $0.name.localizedStandardCompare($1.name) == .orderedAscending }) {
            rows.append(ChangeRow(id: ChangeRow.fileID(file.path), depth: depth, kind: .file(file)))
        }
    }

    /// Files in display order, ignoring collapsed state.
    static func orderedFiles(changes: [FileChange], flat: Bool, filter: String, splitUnversioned: Bool) -> [FileChange] {
        rows(changes: changes, flat: flat, collapsed: [], filter: filter, splitUnversioned: splitUnversioned).compactMap(\.file)
    }

    /// The files a set of rows stands for: file rows themselves, and everything under folder and group rows.
    /// Returned in the order of `changes`, each file once.
    static func files(for ids: Set<String>, in changes: [FileChange]) -> [FileChange] {
        var paths = Set<String>()
        var groups = Set<String>()
        var folders: [(group: String, prefix: String)] = []
        for id in ids {
            if id.hasPrefix("f:") {
                paths.insert(String(id.dropFirst(2)))
            } else if id.hasPrefix("g:") {
                groups.insert(String(id.dropFirst(2)))
            } else {
                // Directory ids look like "d:<group>:<dir>".
                let parts = id.split(separator: ":", maxSplits: 2, omittingEmptySubsequences: false)
                if parts.count == 3, parts[0] == "d" { folders.append((String(parts[1]), parts[2] + "/")) }
            }
        }
        return changes.filter { change in
            if paths.contains(change.path) { return true }
            // "c" is the single, unsplit group of a commit's file list.
            let group = change.kind == .untracked ? "unversioned" : "changes"
            if groups.contains(group) { return true }
            return folders.contains { ($0.group == group || $0.group == "c") && change.path.hasPrefix($0.prefix) }
        }
    }

    /// Collapsed-row ids that hide `path`, so it can be revealed.
    static func ancestors(of path: String, in collapsed: Set<String>) -> [String] {
        collapsed.filter { id in
            if id.hasPrefix("g:") { return true }
            // Directory ids look like "d:<group>:<dir>".
            let parts = id.split(separator: ":", maxSplits: 2, omittingEmptySubsequences: false)
            guard parts.count == 3, parts[0] == "d" else { return false }
            return path.hasPrefix(parts[2] + "/")
        }
    }
}
