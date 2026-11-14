import Foundation

/// One row of the commit graph: the commit, the lane its dot sits in, and the lines that pass through the row.
///
/// Each row draws only its own slice of the lines, from its top edge to its bottom edge, so rows can be
/// rendered lazily and independently and still join up into continuous branches.
struct GraphRow: Identifiable, Equatable {
    /// A line inside one row. Lanes are columns; `from` is at the top or the middle, `to` at the middle or the bottom.
    struct Segment: Equatable {
        enum Span: Equatable {
            /// From the top edge of lane `from` into the commit's dot.
            case intoNode
            /// Straight through the row in lane `from` (`to` is the same lane).
            case through
            /// From the commit's dot down to the bottom edge of lane `to`.
            case outOfNode
        }

        var span: Span
        var from: Int
        var to: Int
        var color: Int
    }

    let commit: Commit
    let column: Int
    let color: Int
    let segments: [Segment]
    /// Lanes this row touches, so the graph column can be sized to the widest row.
    let width: Int

    var id: String { commit.hash }
}

enum CommitGraph {
    /// Assigns lanes to commits listed children-before-parents (`git log --date-order` or `--topo-order`).
    ///
    /// A lane holds the hash of the commit it is waiting for. A commit takes the lane that waits for it
    /// (or a free one when nothing does: a branch tip), hands that lane on to its first parent, and opens
    /// lanes for any further parents. Lanes that waited for the same commit merge into its dot.
    /// Lanes are never compacted, so a line that passes a row stays in its column.
    static func layout(_ commits: [Commit]) -> [GraphRow] {
        var lanes: [String?] = []
        var colors: [Int] = []
        var nextColor = 0
        var rows: [GraphRow] = []
        rows.reserveCapacity(commits.count)

        func freeLane() -> Int {
            if let index = lanes.firstIndex(where: { $0 == nil }) { return index }
            lanes.append(nil)
            colors.append(0)
            return lanes.count - 1
        }

        for commit in commits {
            let before = lanes
            let beforeColors = colors
            var column: Int
            if let waiting = lanes.firstIndex(where: { $0 == commit.hash }) {
                column = waiting
            } else {
                column = freeLane()
                colors[column] = nextColor
                nextColor += 1
            }
            let color = colors[column]

            var segments: [GraphRow.Segment] = []
            for (lane, hash) in before.enumerated() {
                guard let hash else { continue }
                if hash == commit.hash {
                    segments.append(.init(span: .intoNode, from: lane, to: column, color: beforeColors[lane]))
                } else {
                    segments.append(.init(span: .through, from: lane, to: lane, color: beforeColors[lane]))
                }
            }

            // Every lane that was waiting for this commit ends here; its own lane carries on to the first parent.
            for lane in lanes.indices where lanes[lane] == commit.hash { lanes[lane] = nil }

            if let first = commit.parents.first {
                if let existing = lanes.firstIndex(where: { $0 == first }) {
                    // Another child already leads to this parent: join that line rather than drawing a second one.
                    // The bend keeps this branch's colour until it meets the other line, so it reads as this branch.
                    segments.append(.init(span: .outOfNode, from: column, to: existing, color: color))
                } else {
                    lanes[column] = first
                    colors[column] = color
                    segments.append(.init(span: .outOfNode, from: column, to: column, color: color))
                }
                for parent in commit.parents.dropFirst() {
                    if let existing = lanes.firstIndex(where: { $0 == parent }) {
                        segments.append(.init(span: .outOfNode, from: column, to: existing, color: colors[existing]))
                    } else {
                        let lane = freeLane()
                        lanes[lane] = parent
                        colors[lane] = nextColor
                        nextColor += 1
                        segments.append(.init(span: .outOfNode, from: column, to: lane, color: colors[lane]))
                    }
                }
            }

            while let last = lanes.last, last == nil {
                lanes.removeLast()
                colors.removeLast()
            }
            let width = max(before.count, lanes.count, column + 1)
            rows.append(GraphRow(commit: commit, column: column, color: color, segments: segments, width: width))
        }
        return rows
    }
}

// MARK: - Branches and tags

/// A branch or tag, as `git for-each-ref` reports it.
struct GitRef: Identifiable, Hashable {
    enum Kind: Hashable { case local, remote, tag }

    var name: String        // full: refs/heads/main
    var shortName: String   // main, origin/main, v1.0
    var kind: Kind
    /// The commit it points at (peeled, for annotated tags).
    var target: String
    var upstream: String?
    var ahead = 0
    var behind = 0
    /// The upstream branch no longer exists.
    var upstreamGone = false
    var isHead = false
    var date: Date

    var id: String { name }
}

extension Git {
    private static let refFormat = "%(refname)%1f%(refname:short)%1f%(objectname)%1f%(*objectname)%1f%(upstream:short)%1f%(upstream:track,nobracket)%1f%(HEAD)%1f%(committerdate:unix)%1f%(symref)%1e"

    static func refs(in dir: URL) async throws -> [GitRef] {
        let data = try await run(["for-each-ref", "--sort=-committerdate", "--format=\(refFormat)",
                                  "refs/heads", "refs/remotes", "refs/tags"], in: dir)
        return parseRefs(data)
    }

    static func parseRefs(_ data: Data) -> [GitRef] {
        var refs: [GitRef] = []
        for record in data.split(separator: 0x1e, omittingEmptySubsequences: true) {
            let fields = record.split(separator: 0x1f, omittingEmptySubsequences: false).map {
                String(decoding: $0, as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines)
            }
            guard fields.count >= 9, !fields[0].isEmpty else { continue }
            // origin/HEAD and the like only point at another ref.
            if !fields[8].isEmpty { continue }
            let kind: GitRef.Kind
            if fields[0].hasPrefix("refs/heads/") {
                kind = .local
            } else if fields[0].hasPrefix("refs/remotes/") {
                kind = .remote
            } else if fields[0].hasPrefix("refs/tags/") {
                kind = .tag
            } else {
                continue
            }
            var ref = GitRef(name: fields[0], shortName: fields[1], kind: kind,
                             target: fields[3].isEmpty ? fields[2] : fields[3],
                             upstream: fields[4].isEmpty ? nil : fields[4],
                             isHead: fields[6] == "*",
                             date: Date(timeIntervalSince1970: TimeInterval(fields[7]) ?? 0))
            // "ahead 2, behind 1", "ahead 3", "gone"
            for piece in fields[5].split(separator: ",") {
                let words = piece.split(separator: " ")
                if words.first == "gone" { ref.upstreamGone = true }
                if words.count == 2, let count = Int(words[1]) {
                    if words[0] == "ahead" { ref.ahead = count }
                    if words[0] == "behind" { ref.behind = count }
                }
            }
            refs.append(ref)
        }
        return refs
    }
}
