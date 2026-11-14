import Foundation
import CoreServices

/// Recursive FSEvents watcher for one repository.
///
/// FSEvents is kernel-coalesced, so an idle repo costs nothing. Events from inside `.git`
/// are filtered down to the few files that actually signal a state change (HEAD, index, refs),
/// which keeps git's own object churn from triggering refresh storms.
final class FileWatcher {
    private var stream: FSEventStreamRef?
    /// Called on the watcher's own queue, never the main thread.
    private let callback: @Sendable (_ paths: [String], _ gitStateChanged: Bool) -> Void
    private let queue = DispatchQueue(label: "dif.filewatcher", qos: .utility)
    private let root: String

    init(root: URL, callback: @escaping @Sendable (_ paths: [String], _ gitStateChanged: Bool) -> Void) {
        self.root = root.path
        self.callback = callback
        start()
    }

    deinit { stop() }

    private func start() {
        var context = FSEventStreamContext(version: 0, info: Unmanaged.passUnretained(self).toOpaque(),
                                           retain: nil, release: nil, copyDescription: nil)
        let flags = UInt32(kFSEventStreamCreateFlagFileEvents | kFSEventStreamCreateFlagNoDefer | kFSEventStreamCreateFlagUseCFTypes)
        guard let stream = FSEventStreamCreate(kCFAllocatorDefault, { _, info, count, paths, _, _ in
            guard let info else { return }
            let watcher = Unmanaged<FileWatcher>.fromOpaque(info).takeUnretainedValue()
            let list = unsafeBitCast(paths, to: NSArray.self) as? [String] ?? []
            watcher.handle(Array(list.prefix(count)))
        }, &context, [root] as CFArray, FSEventStreamEventId(kFSEventStreamEventIdSinceNow), 0.25, flags) else { return }
        self.stream = stream
        FSEventStreamSetDispatchQueue(stream, queue)
        FSEventStreamStart(stream)
    }

    func stop() {
        guard let stream else { return }
        FSEventStreamStop(stream)
        FSEventStreamInvalidate(stream)
        FSEventStreamRelease(stream)
        self.stream = nil
    }

    private func handle(_ paths: [String]) {
        var relevant: [String] = []
        var gitState = false
        for path in paths {
            if let range = path.range(of: "/.git/") {
                let inner = path[range.upperBound...]
                if inner.hasSuffix(".lock") { continue }
                let name = (path as NSString).lastPathComponent
                if ["HEAD", "index", "ORIG_HEAD", "MERGE_HEAD", "FETCH_HEAD", "packed-refs"].contains(name) || inner.hasPrefix("refs/") {
                    gitState = true
                }
                continue
            }
            if path.hasSuffix("/.git") { continue }
            relevant.append(path)
        }
        if !relevant.isEmpty || gitState { callback(relevant, gitState) }
    }
}
