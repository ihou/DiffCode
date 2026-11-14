import Foundation

/// Replaces one hunk of a file with what the base revision had there.
///
/// Works on the bytes of the two files rather than on decoded text, so the lines that are not
/// touched keep their encoding, line endings and trailing-newline state exactly.
enum HunkPatch {
    /// The byte range of every line, terminator included. Lines end at `\n`, as in `TextCodec.splitLines`.
    static func lineRanges(_ data: Data) -> [Range<Int>] {
        var ranges: [Range<Int>] = []
        data.withUnsafeBytes { (raw: UnsafeRawBufferPointer) in
            let bytes = raw.bindMemory(to: UInt8.self)
            ranges.reserveCapacity(bytes.count / 32 + 1)
            var start = 0
            for index in 0..<bytes.count where bytes[index] == 0x0A {
                ranges.append(start..<(index + 1))
                start = index + 1
            }
            if start < bytes.count { ranges.append(start..<bytes.count) }
        }
        return ranges
    }

    static func isUTF16(_ data: Data) -> Bool {
        data.starts(with: [0xFF, 0xFE]) || data.starts(with: [0xFE, 0xFF])
    }

    /// `new` with the lines of `hunk` put back the way they are in `old`.
    /// Returns nil when the hunk does not fit the two files.
    static func revert(_ hunk: Hunk, old: Data, new: Data) -> Data? {
        guard !isUTF16(old), !isUTF16(new) else { return nil }
        let oldLines = lineRanges(old), newLines = lineRanges(new)
        guard hunk.leftStart >= 0, hunk.leftCount >= 0, hunk.rightStart >= 0, hunk.rightCount >= 0,
              hunk.leftStart + hunk.leftCount <= oldLines.count,
              hunk.rightStart + hunk.rightCount <= newLines.count else { return nil }

        func bytes(_ data: Data, _ lines: [Range<Int>], _ range: Range<Int>) -> Data {
            guard !range.isEmpty else { return Data() }
            let from = lines[range.lowerBound].lowerBound, to = lines[range.upperBound - 1].upperBound
            return data.subdata(in: (data.startIndex + from)..<(data.startIndex + to))
        }
        var head = bytes(new, newLines, 0..<hunk.rightStart)
        var middle = bytes(old, oldLines, hunk.leftStart..<(hunk.leftStart + hunk.leftCount))
        let tail = bytes(new, newLines, (hunk.rightStart + hunk.rightCount)..<newLines.count)

        // Only the last line of a file may lack its newline. A piece that is no longer last gets one,
        // in the style of the file it is joined to.
        let crlf = Data([0x0D, 0x0A])
        let usesCRLF = new.range(of: crlf) != nil || (!new.contains(0x0A) && old.range(of: crlf) != nil)
        let newline: [UInt8] = usesCRLF ? [0x0D, 0x0A] : [0x0A]
        if !head.isEmpty, head.last != 0x0A, !(middle.isEmpty && tail.isEmpty) { head.append(contentsOf: newline) }
        if !middle.isEmpty, middle.last != 0x0A, !tail.isEmpty { middle.append(contentsOf: newline) }
        return head + middle + tail
    }
}

/// The few things Dif does to a working tree: roll changes back, move new files to the Trash,
/// and take either back. Everything else about the repository is read-only.
enum WorkingTree {
    struct Failure: LocalizedError {
        let message: String
        var errorDescription: String? { message }
    }

    /// Enough of a file's state to tell whether anything touched it since.
    struct Stamp: Equatable {
        var size: Int64
        var modified: Date
    }

    struct Entry {
        var path: String
        /// Where the earlier contents are kept; nil when there was no file.
        var backup: URL?
        /// `backup` is the file itself, moved to the Trash: undoing moves it back.
        var moved = false
        /// What the operation left at `path`; nil when it left nothing.
        var after: Stamp?
    }

    /// What one operation changed, kept so that it can be undone.
    struct Record {
        var root: URL
        var entries: [Entry] = []
        /// Paths to stage again when undoing: those that were staged as new or renamed files.
        var restagePresent: [String] = []
        /// The old names of renames, staged as deleted again when undoing.
        var restageAbsent: [String] = []
        /// Where this record's copies live.
        var directory: URL?

        func discard() {
            if let directory { try? FileManager.default.removeItem(at: directory) }
        }
    }

    enum HunkResult {
        case reverted(Record)
        /// The file is no longer what the diff on screen was made from; nothing was changed.
        case stale
    }

    private static let backupRoot = DiffLoader.scratchDirectory.appendingPathComponent("undo")

    static func makeBackupDirectory() throws -> URL {
        let directory = backupRoot.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        return directory
    }

    /// Does not follow symbolic links: a link is a file in its own right here.
    static func stamp(_ url: URL) -> Stamp? {
        guard let attributes = try? FileManager.default.attributesOfItem(atPath: url.path) else { return nil }
        return Stamp(size: (attributes[.size] as? NSNumber)?.int64Value ?? 0,
                     modified: attributes[.modificationDate] as? Date ?? .distantPast)
    }

    /// Runs `git <arguments> -- <paths>` in as many batches as the command line needs. Paths are taken literally:
    /// a file called `*.kt` names itself, not every Kotlin file.
    /// - Parameter environment: added to git's own (`GIT_INDEX_FILE`, for an index of DiffCode's own).
    static func git(_ arguments: [String], paths: [String], in root: URL, environment: [String: String] = [:]) async throws {
        var batch: [String] = []
        var bytes = 0
        func flush() async throws {
            guard !batch.isEmpty else { return }
            _ = try await Git.run(["--literal-pathspecs"] + arguments + ["--"] + batch, in: root, environment: environment)
            batch = []
            bytes = 0
        }
        for path in paths {
            batch.append(path)
            bytes += path.utf8.count + 1
            if bytes > 60_000 { try await flush() }
        }
        try await flush()
    }

    /// Which of `paths` HEAD has. Asked just before acting, because the list on screen may be a moment old
    /// and "added" or "untracked" is only safe to act on while it is still true.
    private static func pathsInHead(_ paths: [String], in root: URL) async -> Set<String> {
        var found = Set<String>()
        var batch: [String] = []
        var bytes = 0
        func flush() async {
            guard !batch.isEmpty else { return }
            // Fails in a repository with no commits yet, where HEAD has nothing.
            if let data = try? await Git.run(["--literal-pathspecs", "ls-tree", "-r", "-z", "--name-only", "HEAD", "--"] + batch, in: root) {
                for name in data.split(separator: 0, omittingEmptySubsequences: true) {
                    found.insert(String(decoding: name, as: UTF8.self))
                }
            }
            batch = []
            bytes = 0
        }
        for path in paths {
            batch.append(path)
            bytes += path.utf8.count + 1
            if bytes > 60_000 { await flush() }
        }
        await flush()
        return found
    }

    // MARK: Rollback

    /// Puts `changes` back to what HEAD has. Files HEAD does not have (added, never committed) are taken
    /// out of the index and left on disk, as Android Studio does; untracked files and submodules are not touched.
    ///
    /// Throws when nothing could be done. If git fails part of the way, what was done is returned
    /// together with the error, so that it can still be undone.
    static func rollback(_ changes: [FileChange], in root: URL) async throws -> (record: Record, failure: Error?) {
        let fm = FileManager.default
        let targets = changes.filter { $0.kind != .untracked && !$0.isSubmodule }
        guard !targets.isEmpty else {
            throw Failure(message: L("Nothing here can be rolled back.", "这里没有可以回滚的内容。"))
        }
        let inHead = await pathsInHead(targets.flatMap { [$0.path] + ($0.oldPath.map { [$0] } ?? []) }, in: root)

        var restore: [String] = []   // brought back from HEAD, in the index and on disk
        var unstage: [String] = []   // taken out of the index, left on disk
        var remove: [String] = []    // files that go away: the new name of a rename
        var record = Record(root: root)
        for change in targets {
            if let old = change.oldPath, change.kind == .renamed, inHead.contains(old) {
                restore.append(old)
                record.restageAbsent.append(old)
                if inHead.contains(change.path) {
                    restore.append(change.path)
                } else {
                    unstage.append(change.path)
                    remove.append(change.path)
                    record.restagePresent.append(change.path)
                }
            } else if inHead.contains(change.path) {
                restore.append(change.path)
            } else {
                unstage.append(change.path)
                record.restagePresent.append(change.path)
            }
        }

        // Keep a copy of everything about to be overwritten or removed.
        let directory = try makeBackupDirectory()
        record.directory = directory
        for (index, path) in (restore + remove).enumerated() {
            let url = root.appendingPathComponent(path)
            var backup: URL?
            if stamp(url) != nil {
                let copy = directory.appendingPathComponent(String(index))
                try fm.copyItem(at: url, to: copy)
                backup = copy
            }
            record.entries.append(Entry(path: path, backup: backup))
        }

        var failure: Error?
        do {
            try await git(["checkout", "HEAD"], paths: restore, in: root)
            try await git(["rm", "--cached", "-f", "-q", "--ignore-unmatch"], paths: unstage, in: root)
            for path in remove { try fm.removeItem(at: root.appendingPathComponent(path)) }
        } catch {
            failure = error
        }
        for index in record.entries.indices {
            record.entries[index].after = stamp(root.appendingPathComponent(record.entries[index].path))
        }
        return (record, failure)
    }

    // MARK: One hunk

    /// Puts one hunk of a working-tree file back to the base revision's lines.
    ///
    /// `expectedLeft` and `expectedRight` are the two sides of the diff as shown. If either file reads
    /// differently now, nothing is written and `.stale` is returned, so a click never lands on lines
    /// other than the ones that were on screen.
    static func revertHunk(request: DiffRequest, hunk: Hunk, expectedLeft: [String], expectedRight: [String]) async throws -> HunkResult {
        guard case .worktree(let base) = request.source else {
            throw Failure(message: L("Only local changes can be rolled back.", "只有本地变更可以回滚。"))
        }
        let fm = FileManager.default
        let root = request.root
        let url = root.appendingPathComponent(request.path)
        guard let attributes = try? fm.attributesOfItem(atPath: url.path),
              attributes[.type] as? FileAttributeType == .typeRegular,
              let new = try? Data(contentsOf: url),
              let old = await Git.blob(rev: base, path: request.oldPath ?? request.path, in: root) else { return .stale }
        if HunkPatch.isUTF16(old) || HunkPatch.isUTF16(new) {
            throw Failure(message: L("UTF-16 files can only be rolled back as a whole.", "UTF-16 文件只能整个回滚。"))
        }
        guard TextCodec.splitLines(TextCodec.decode(old)) == expectedLeft,
              TextCodec.splitLines(TextCodec.decode(new)) == expectedRight,
              HunkPatch.lineRanges(old).count == expectedLeft.count,
              HunkPatch.lineRanges(new).count == expectedRight.count,
              let result = HunkPatch.revert(hunk, old: old, new: new), result != new else { return .stale }

        let directory = try makeBackupDirectory()
        let copy = directory.appendingPathComponent("0")
        try fm.copyItem(at: url, to: copy)
        var record = Record(root: root, entries: [Entry(path: request.path, backup: copy)], directory: directory)

        // Written in place: the file keeps its permissions, and whatever is watching it sees an edit, not a replacement.
        let handle = try FileHandle(forWritingTo: url)
        try handle.truncate(atOffset: 0)
        try handle.write(contentsOf: result)
        try handle.close()

        // With the last hunk gone the file is HEAD's again; drop anything still staged for it, so it leaves the list.
        if result == old, request.oldPath == nil, base == "HEAD" {
            try? await git(["checkout", "HEAD"], paths: [request.path], in: root)
        }
        record.entries[0].after = stamp(url)
        return .reverted(record)
    }

    // MARK: Delete

    /// Moves new files (untracked, or added and never committed) to the Trash.
    /// Returns what was moved and the paths that could not be.
    static func trash(_ changes: [FileChange], in root: URL) async throws -> (record: Record, failed: [String]) {
        let fm = FileManager.default
        let candidates = changes.filter { $0.kind == .untracked || $0.kind == .added }
        // A file that has been committed in the meantime is no longer a new file; it stays.
        let inHead = await pathsInHead(candidates.map(\.path), in: root)
        let targets = candidates.filter { !inHead.contains($0.path) }
        var failed = candidates.filter { inHead.contains($0.path) }.map(\.path)
        var record = Record(root: root)
        let staged = targets.filter { $0.kind == .added }.map(\.path)
        try await git(["rm", "--cached", "-f", "-q", "--ignore-unmatch"], paths: staged, in: root)
        record.restagePresent = staged

        for change in targets {
            let url = root.appendingPathComponent(change.path)
            guard stamp(url) != nil else { continue }
            do {
                var resulting: NSURL?
                try fm.trashItem(at: url, resultingItemURL: &resulting)
                record.entries.append(Entry(path: change.path, backup: resulting as URL?, moved: true))
            } catch {
                failed.append(change.path)
            }
        }
        return (record, failed)
    }

    // MARK: Undo

    /// Takes an operation back. A file that has changed since the operation is left as it is and counted as skipped.
    /// Returns the new stamp of every restored path (nil where the file is gone again).
    static func undo(_ record: Record) async -> (restored: [String: Stamp?], skipped: Int) {
        let fm = FileManager.default
        var restored: [String: Stamp?] = [:]
        var skipped = 0
        for entry in record.entries {
            let url = record.root.appendingPathComponent(entry.path)
            guard stamp(url) == entry.after else {
                skipped += 1
                continue
            }
            do {
                if entry.moved {
                    guard let backup = entry.backup, stamp(backup) != nil else {
                        skipped += 1
                        continue
                    }
                    try fm.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
                    try fm.moveItem(at: backup, to: url)
                } else {
                    if entry.after != nil { try fm.removeItem(at: url) }
                    if let backup = entry.backup {
                        try fm.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
                        try fm.copyItem(at: backup, to: url)
                        // A copy keeps the old modification time; build tools need to see that the file changed.
                        try? fm.setAttributes([.modificationDate: Date()], ofItemAtPath: url.path)
                    }
                }
                restored[entry.path] = .some(stamp(url))
            } catch {
                skipped += 1
            }
        }
        let present = record.restagePresent.filter { stamp(record.root.appendingPathComponent($0)) != nil }
        let absent = record.restageAbsent.filter { stamp(record.root.appendingPathComponent($0)) == nil }
        try? await git(["add", "-A"], paths: present + absent, in: record.root)
        return (restored, skipped)
    }
}
