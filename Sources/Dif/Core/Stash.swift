import Foundation

/// One entry of `git stash list`.
struct StashEntry: Identifiable, Hashable {
    /// The stash commit: the tracked files as they were. Stays the same while the list around it changes.
    var hash: String
    /// Its place in the list when read, `stash@{index}`. Stashes added or dropped since move it: act by `hash`.
    var index: Int
    /// git's line for it: "On main: message", or "WIP on main: 1a2b3c4 Subject" when none was given.
    var message: String
    var date: Date
    /// The commit it was made on (its first parent).
    var base: String
    /// The commit holding its untracked files (`stash push -u`), when it has any.
    var untracked: String?

    var id: String { hash }
    var name: String { "stash@{\(index)}" }

    /// The branch it was made on, from git's line; nil on a detached HEAD.
    var branch: String? {
        for prefix in ["WIP on ", "On "] where message.hasPrefix(prefix) {
            let rest = message.dropFirst(prefix.count)
            guard let colon = rest.range(of: ": ") else { return nil }
            let name = String(rest[..<colon.lowerBound])
            return name == "(no branch)" ? nil : name
        }
        return nil
    }

    /// What the user called it, or git's "1a2b3c4 Subject" of the commit it was made on.
    var title: String {
        guard message.hasPrefix("WIP on ") || message.hasPrefix("On "), let colon = message.range(of: ": ") else { return message }
        let rest = String(message[colon.upperBound...])
        return rest.isEmpty ? message : rest
    }
}

/// The stash: changes put aside and brought back later, as git keeps them.
extension Git {
    /// Every stash, newest first.
    static func stashes(in dir: URL) async -> [StashEntry] {
        guard let data = try? await run(["stash", "list", "--format=%H%x1f%P%x1f%ct%x1f%gd%x1f%gs%x1e"], in: dir) else { return [] }
        return parseStashes(data)
    }

    static func parseStashes(_ data: Data) -> [StashEntry] {
        var entries: [StashEntry] = []
        for record in data.split(separator: 0x1e, omittingEmptySubsequences: true) {
            let fields = record.split(separator: 0x1f, omittingEmptySubsequences: false).map {
                String(decoding: $0, as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines)
            }
            guard fields.count >= 5, fields[0].count >= 7 else { continue }
            let parents = fields[1].split(separator: " ").map(String.init)
            guard let base = parents.first else { continue }
            // "stash@{3}"; a list read otherwise counts in order.
            let selector = fields[3]
            let index = selector.hasPrefix("stash@{") && selector.hasSuffix("}")
                ? Int(selector.dropFirst(7).dropLast()) ?? entries.count : entries.count
            entries.append(StashEntry(hash: fields[0], index: index, message: fields[4],
                                      date: Date(timeIntervalSince1970: TimeInterval(fields[2]) ?? 0),
                                      base: base, untracked: parents.count > 2 ? parents[2] : nil))
        }
        return entries
    }

    /// Puts the local changes in a new stash, and the working tree goes back to HEAD. Returns the new stash; nil when
    /// there was nothing to put aside.
    /// - Parameter includeUntracked: new files go too (and leave the working tree), as with `stash push -u`.
    @discardableResult
    static func stashPush(message: String, includeUntracked: Bool, in dir: URL) async throws -> StashEntry? {
        let before = await stashes(in: dir).first?.hash
        var args = ["stash", "push", "--quiet"]
        if includeUntracked { args.append("--include-untracked") }
        let text = message.trimmingCharacters(in: .whitespacesAndNewlines)
        if !text.isEmpty { args += ["--message", text] }
        _ = try await run(args, in: dir)
        guard let top = await stashes(in: dir).first, top.hash != before else { return nil }
        return top
    }

    /// The stash as listed now, found by its commit. Fails when it has been dropped since.
    static func currentStash(_ entry: StashEntry, in dir: URL) async throws -> StashEntry {
        guard let current = await stashes(in: dir).first(where: { $0.hash == entry.hash }) else {
            throw WorkingTree.Failure(message: L("That stash is no longer in the list.", "这个 stash 已经不在列表里了。"))
        }
        return current
    }

    /// Brings a stash's changes back into the working tree. It stays in the list, unless `drop`: then it goes once
    /// its changes are in without conflicts (`stash pop`).
    static func stashApply(_ entry: StashEntry, drop: Bool, in dir: URL) async throws {
        let current = try await currentStash(entry, in: dir)
        _ = try await run(["stash", drop ? "pop" : "apply", "--quiet", current.name], in: dir)
    }

    static func stashDrop(_ entry: StashEntry, in dir: URL) async throws {
        let current = try await currentStash(entry, in: dir)
        _ = try await run(["stash", "drop", "--quiet", current.name], in: dir)
    }

    /// Lists a dropped stash again (its commit stays in the repository until git cleans up), at the top.
    static func stashRestore(_ entry: StashEntry, in dir: URL) async throws {
        _ = try await run(["stash", "store", "--quiet", "--message", entry.message, entry.hash], in: dir)
    }

    /// What a stash changed against the commit it was made on, and its untracked files, which come out as new.
    static func stashFiles(_ entry: StashEntry, in dir: URL) async throws -> [FileChange] {
        var changes = try await compareFiles(base: entry.base, head: entry.hash, in: dir)
        if let untracked = entry.untracked {
            async let names = run(["ls-tree", "-r", "-z", "--name-only", untracked], in: dir)
            async let stats = run(["diff", "--numstat", "-z", emptyTree, untracked, "--"], in: dir)
            let counts = parseNumstat((try? await stats) ?? Data())
            let tracked = Set(changes.map(\.path))
            for token in try await names.split(separator: 0, omittingEmptySubsequences: true) {
                let path = String(decoding: token, as: UTF8.self)
                guard !tracked.contains(path) else { continue }
                var change = FileChange(path: path, oldPath: nil, kind: .untracked)
                if let count = counts[path] {
                    change.additions = count.0
                    change.deletions = 0
                }
                changes.append(change)
            }
            changes.sort { $0.path.localizedStandardCompare($1.path) == .orderedAscending }
        }
        return changes
    }
}
