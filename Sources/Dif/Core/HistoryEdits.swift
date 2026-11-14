import Foundation

/// How far a reset goes, as git's own modes (and the IDE's Reset dialog) put it.
enum ResetMode: String, CaseIterable, Equatable {
    /// The commits after the target come back as staged changes.
    case soft
    /// They come back as local changes (the default).
    case mixed
    /// They and every local change to tracked files are thrown away.
    case hard
    /// They are thrown away; local changes stay, and git refuses if one is in a file those commits touched.
    case keep

    var title: String {
        switch self {
        case .soft: return "Soft"
        case .mixed: return "Mixed"
        case .hard: return "Hard"
        case .keep: return "Keep"
        }
    }

    var detail: String {
        switch self {
        case .soft: return L("The changes of the commits after it come back staged; local changes stay as they are.",
                             "其后提交的改动回到暂存区；本地改动保持不变。")
        case .mixed: return L("The changes of the commits after it come back as local changes, with yours.",
                              "其后提交的改动变回本地改动，和你现有的改动放在一起。")
        case .hard: return L("The commits after it and every local change to tracked files are thrown away. New files stay.",
                             "其后的提交和已跟踪文件的所有本地改动都会丢弃。新文件保留。")
        case .keep: return L("The commits after it are thrown away; your local changes stay. Refused if they touch the same files.",
                             "其后的提交会丢弃，本地改动保留。如果两者改了同一个文件，git 会拒绝。")
        }
    }
}

/// A move of the branch (or of a detached HEAD) that DiffCode made, and how to take it back.
struct HeadMove: Equatable {
    /// The branch that moved; nil for a detached HEAD.
    var branch: String?
    var before: String
    var after: String
    /// How the branch goes back: as the reset was done; `keep` after a merge, rebase, cherry-pick or revert.
    var mode: ResetMode
    /// What a hard reset threw away of the local changes, kept as a stash commit that is in no list (`stash create`).
    var snapshot: String?
}

/// Writing history the way the IDE's Log and branch menus do: cherry-pick, revert, reset, merge, rebase, and deleting
/// and renaming branches. Each is git's own command; DiffCode only remembers enough to take it back.
extension Git {
    static func head(in dir: URL) async throws -> String {
        try await runString(["rev-parse", "--verify", "HEAD"], in: dir).trimmingCharacters(in: .whitespacesAndNewlines)
    }

    /// The checked-out branch's name; nil for a detached HEAD.
    static func currentBranch(in dir: URL) async -> String? {
        let out = (try? await runString(["symbolic-ref", "--quiet", "--short", "HEAD"], in: dir)) ?? ""
        let name = out.trimmingCharacters(in: .whitespacesAndNewlines)
        return name.isEmpty ? nil : name
    }

    /// Where HEAD is now, as a move starting there will record it.
    private static func position(in dir: URL) async throws -> (branch: String?, head: String) {
        (await currentBranch(in: dir), try await head(in: dir))
    }

    /// Whether `ancestor` is in the history of `descendant` (a commit is in its own).
    static func isAncestor(_ ancestor: String, of descendant: String, in dir: URL) async -> Bool {
        (try? await run(["merge-base", "--is-ancestor", ancestor, descendant], in: dir)) != nil
    }

    /// The remote branches that have `commit` in their history ("origin/main"); empty when it was never pushed.
    static func remoteBranches(containing commit: String, in dir: URL) async -> [String] {
        let out = (try? await runString(["branch", "-r", "--format=%(refname:short)", "--contains", commit], in: dir)) ?? ""
        return out.split(separator: "\n").map { $0.trimmingCharacters(in: .whitespaces) }
            .filter { !$0.isEmpty && !$0.hasSuffix("/HEAD") }
    }

    /// The commits `revisions` select ("a..b", or "x", "--not", "--branches"), newest first.
    static func commits(_ revisions: String..., in dir: URL, limit: Int = 50) async -> [Commit] {
        guard let data = try? await run(["log", "--format=\(logFormat)", "--max-count=\(limit)"] + revisions + ["--"], in: dir) else { return [] }
        return parseLog(data)
    }

    /// How many commits `revisions` select; 0 when git cannot tell.
    static func count(_ revisions: String..., in dir: URL) async -> Int {
        let out = (try? await runString(["rev-list", "--count"] + revisions + ["--"], in: dir)) ?? ""
        return Int(out.trimmingCharacters(in: .whitespacesAndNewlines)) ?? 0
    }

    /// Of `hashes`, those whose changes HEAD already has as another commit (cherry-picked before, or rebased).
    static func alreadyApplied(_ hashes: [String], in dir: URL) async -> Set<String> {
        var applied = Set<String>()
        for hash in hashes.prefix(50) {
            // Lists just `hash`, marked "-" when HEAD has a commit with the same changes.
            let out = (try? await runString(["cherry", "HEAD", hash, hash + "^"], in: dir)) ?? ""
            if out.hasPrefix("-") { applied.insert(hash) }
        }
        return applied
    }

    // MARK: Cherry-pick and revert

    /// Applies `hashes` on top of HEAD as new commits, in the order given (oldest first).
    @discardableResult
    static func cherryPick(_ hashes: [String], in dir: URL) async throws -> HeadMove {
        let (branch, before) = try await position(in: dir)
        _ = try await run(["cherry-pick"] + hashes, in: dir, environment: ["GIT_EDITOR": "true"])
        return HeadMove(branch: branch, before: before, after: try await head(in: dir), mode: .keep)
    }

    /// Adds commits that undo `hashes`, in the order given (newest first), each with git's "Revert …" message.
    @discardableResult
    static func revert(_ hashes: [String], in dir: URL) async throws -> HeadMove {
        let (branch, before) = try await position(in: dir)
        _ = try await run(["revert", "--no-edit"] + hashes, in: dir, environment: ["GIT_EDITOR": "true"])
        return HeadMove(branch: branch, before: before, after: try await head(in: dir), mode: .keep)
    }

    /// Leaves out the commit the operation stopped at and goes on with the rest (rebase, cherry-pick, revert).
    static func skipOperation(_ operation: RepoOperation, in dir: URL) async throws {
        _ = try await run([operation.command, "--skip"], in: dir, environment: ["GIT_EDITOR": "true"])
    }

    // MARK: Reset

    /// Moves the current branch to `target`. Before a hard reset, the local changes it throws away are kept as a
    /// commit in no list, so that the reset can be taken back with them.
    static func reset(to target: String, mode: ResetMode, in dir: URL) async throws -> HeadMove {
        let (branch, before) = try await position(in: dir)
        var snapshot: String?
        if mode == .hard {
            let out = try await runString(["stash", "create"], in: dir).trimmingCharacters(in: .whitespacesAndNewlines)
            snapshot = out.isEmpty ? nil : out
        }
        _ = try await run(["reset", "--quiet", "--" + mode.rawValue, target], in: dir)
        return HeadMove(branch: branch, before: before, after: try await head(in: dir), mode: mode, snapshot: snapshot)
    }

    /// Takes back a move of the branch made here. Refuses once HEAD has moved on, and when going back would drop
    /// commits that were pushed since. Local changes made meanwhile stay (git refuses rather than overwrite them).
    static func undoHeadMove(_ move: HeadMove, in dir: URL) async throws {
        guard let now = try? await position(in: dir), now.branch == move.branch, now.head == move.after else {
            throw WorkingTree.Failure(message: L("HEAD has moved since, so it was left alone.", "之后 HEAD 又变了，所以没有撤销。"))
        }
        // Going back drops the commits `before` does not have (none, after a reset to an older commit).
        if !(await isAncestor(move.after, of: move.before, in: dir)),
           !(await remoteBranches(containing: move.after, in: dir)).isEmpty {
            throw WorkingTree.Failure(message: L("The new commits have been pushed since, so it was left alone.",
                                                 "新的提交已经推送到远程，所以没有撤销。"))
        }
        // A hard reset left a clean working tree; anything in it now is new, and `keep` keeps it.
        let mode: ResetMode = move.mode == .hard ? .keep : move.mode
        _ = try await run(["reset", "--quiet", "--" + mode.rawValue, move.before], in: dir)
        if let snapshot = move.snapshot {
            do {
                _ = try await run(["stash", "apply", "--quiet", snapshot], in: dir)
            } catch {
                throw WorkingTree.Failure(message: L("The branch is back, but the local changes the reset threw away clash with yours. They are kept: git stash apply \(snapshot)",
                                                     "分支已恢复，但重置时丢弃的本地改动与你现在的改动冲突。它们仍然保留着：git stash apply \(snapshot)"))
            }
        }
    }

    // MARK: Merge and rebase

    enum MergeOutcome: Equatable { case upToDate, fastForward, merged }

    /// Merges `ref` into the current branch with git's own message. Stops at conflicts (an error, with the merge in
    /// progress). With `autostash`, local changes in the way are put aside and back.
    static func merge(_ ref: String, autostash: Bool, in dir: URL) async throws -> (outcome: MergeOutcome, move: HeadMove) {
        let (branch, before) = try await position(in: dir)
        var args = ["merge", "--no-edit"]
        if autostash { args.append("--autostash") }
        // git's message names the branch as given: "Merge branch 'feature'", not 'refs/heads/feature'.
        let name = await shortestName(ref, in: dir)
        _ = try await run(args + [name], in: dir, environment: ["GIT_EDITOR": "true", "GIT_MERGE_AUTOEDIT": "no"])
        let after = try await head(in: dir)
        let outcome: MergeOutcome
        if after == before {
            outcome = .upToDate
        } else {
            let parents = try await runString(["rev-list", "--parents", "-n", "1", after], in: dir).split(separator: " ").count - 1
            outcome = parents > 1 ? .merged : .fastForward
        }
        return (outcome, HeadMove(branch: branch, before: before, after: after, mode: .keep))
    }

    /// Replays the current branch's own commits on top of `ref`. Stops at conflicts (an error, with the rebase in
    /// progress). With `autostash`, local changes are put aside and back (git rebases nothing with them in place).
    static func rebase(onto ref: String, autostash: Bool, in dir: URL) async throws -> HeadMove {
        let (branch, before) = try await position(in: dir)
        var args = ["rebase"]
        if autostash { args.append("--autostash") }
        _ = try await run(args + [ref], in: dir, environment: ["GIT_EDITOR": "true"])
        return HeadMove(branch: branch, before: before, after: try await head(in: dir), mode: .keep)
    }

    /// "feature" for refs/heads/feature, "origin/x" for refs/remotes/origin/x, unless git would take the short
    /// name for something else (a tag of the same name): then the full one.
    static func shortestName(_ fullName: String, in dir: URL) async -> String {
        guard let prefix = ["refs/heads/", "refs/remotes/", "refs/tags/"].first(where: fullName.hasPrefix) else { return fullName }
        let short = String(fullName.dropFirst(prefix.count))
        guard !short.isEmpty, !short.hasPrefix("-") else { return fullName }
        let resolved = (try? await runString(["rev-parse", "--symbolic-full-name", short], in: dir))?
            .trimmingCharacters(in: .whitespacesAndNewlines)
        return resolved == fullName ? short : fullName
    }

    /// git turned an operation down because of local changes (and not because of conflicts it ran into).
    static func refusedForLocalChanges(_ message: String) -> Bool {
        message.contains("would be overwritten by") || message.contains("commit your changes or stash them")
            || message.contains("Please commit or stash them") || message.contains("cannot rebase: You have unstaged changes")
            || message.contains("cannot rebase: Your index contains uncommitted changes")
    }

    // MARK: Branches

    /// Deletes a local branch. Without `force`, git refuses one whose commits are on no other branch.
    static func deleteBranch(_ name: String, force: Bool, in dir: URL) async throws {
        _ = try await run(["branch", force ? "-D" : "-d", name], in: dir)
    }

    /// git refused to delete a branch because its commits would be lost.
    static func isNotFullyMerged(_ message: String) -> Bool {
        message.contains("not fully merged")
    }

    static func renameBranch(_ name: String, to newName: String, in dir: URL) async throws {
        _ = try await run(["branch", "-m", name, newName], in: dir)
    }

    /// Recreates a deleted local branch where it was, following what it followed.
    static func restoreBranch(_ name: String, at hash: String, upstream: String?, in dir: URL) async throws {
        _ = try await run(["branch", name, hash], in: dir)
        if let upstream {
            _ = try? await run(["branch", "--quiet", "--set-upstream-to=\(upstream)", name], in: dir)
        }
    }

    /// "origin/feature/x" → ("origin", "feature/x"), with the remote names the repository has (one may contain "/").
    static func splitRemoteBranch(_ shortName: String, remotes: [String]) -> (remote: String, branch: String)? {
        let remote = remotes.filter { shortName.hasPrefix($0 + "/") }.max { $0.count < $1.count }
        guard let remote else { return nil }
        let branch = String(shortName.dropFirst(remote.count + 1))
        return branch.isEmpty ? nil : (remote, branch)
    }

    static func deleteRemoteBranch(_ branch: String, remote: String, in dir: URL) async throws {
        _ = try await run(["push", "--quiet", remote, "--delete", "refs/heads/" + branch], in: dir)
    }

    /// Pushes a deleted remote branch back where it was.
    static func restoreRemoteBranch(_ branch: String, remote: String, at hash: String, in dir: URL) async throws {
        _ = try await run(["push", "--quiet", remote, "\(hash):refs/heads/\(branch)"], in: dir)
    }

    // MARK: Force push

    /// Branches DiffCode never force-pushes, as the IDE protects them by default.
    static let protectedBranches: Set<String> = ["main", "master"]

    /// The commits the upstream has and the branch does not: what a force push would take off the remote.
    static func incomingCommits(in dir: URL, limit: Int = 50) async -> [Commit] {
        await commits("HEAD..@{upstream}", in: dir, limit: limit)
    }

    /// The remote and the branch on it that `branch` follows.
    static func upstreamTarget(of branch: String, in dir: URL) async -> (remote: String, branch: String)? {
        func config(_ key: String) async -> String {
            ((try? await runString(["config", "--get", key], in: dir)) ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        }
        let remote = await config("branch.\(branch).remote")
        let merge = await config("branch.\(branch).merge")
        guard !remote.isEmpty, remote != ".", merge.hasPrefix("refs/heads/") else { return nil }
        return (remote, String(merge.dropFirst("refs/heads/".count)))
    }

    /// Pushes `branch` over its upstream, only if the remote branch is still at `expected`, the commit the user was
    /// shown (`--force-with-lease`): commits someone pushed meanwhile are never lost.
    static func forcePush(branch: String, expected: String, in dir: URL) async throws {
        guard let target = await upstreamTarget(of: branch, in: dir) else {
            throw WorkingTree.Failure(message: L("\(branch) follows no remote branch.", "\(branch) 没有跟踪的远程分支。"))
        }
        guard !protectedBranches.contains(target.branch), !protectedBranches.contains(branch) else {
            throw WorkingTree.Failure(message: L("DiffCode never force-pushes \(target.branch).", "DiffCode 不会强制推送 \(target.branch)。"))
        }
        _ = try await run(["push", "--quiet", "--force-with-lease=refs/heads/\(target.branch):\(expected)",
                           target.remote, "refs/heads/\(branch):refs/heads/\(target.branch)"], in: dir)
    }
}
