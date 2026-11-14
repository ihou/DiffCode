import AppKit

/// Which folders the Mac App Store edition may read and write.
///
/// The sandbox opens a folder to the app only when the user chose it: in the open panel, by dropping it, or by
/// opening it with the app (`dif`, Finder). A security-scoped bookmark, taken right then, opens it again on later
/// launches. Granting a folder grants everything in it, so one grant of the folder that holds all the repositories
/// (or of the home folder, which also lets git read `~/.gitconfig`) covers every project.
///
/// Outside the sandbox (the direct download) every folder is reachable and none of this does anything.
@MainActor
final class FolderAccess {
    static let shared = FolderAccess()

    private static let defaultsKey = "folderBookmarks"

    /// Granted folders by path, each with the URL whose access is open.
    private(set) var granted: [String: URL] = [:]

    var isNeeded: Bool { Sandbox.isActive }

    /// Opens every folder granted on earlier launches. Bookmarks that no longer resolve (the folder was deleted) are
    /// dropped; one whose folder moved is kept under its new path.
    func restore() {
        guard isNeeded else { return }
        var stored = storedBookmarks
        var changed = false
        for (path, data) in stored {
            var isStale = false
            guard let url = try? URL(resolvingBookmarkData: data, options: [.withSecurityScope], relativeTo: nil,
                                     bookmarkDataIsStale: &isStale),
                  url.startAccessingSecurityScopedResource() else {
                stored[path] = nil
                changed = true
                continue
            }
            let resolved = Self.key(url)
            if isStale || resolved != path {
                stored[path] = nil
                stored[resolved] = (try? Self.bookmark(for: url)) ?? data
                changed = true
            }
            granted[resolved] = url
        }
        if changed { storedBookmarks = stored }
    }

    /// Keeps the access the system just gave to `url`, for this launch and the next ones. Call it while that access
    /// is live: with what the open panel returned, or a drop, or an open event.
    @discardableResult
    func remember(_ url: URL) -> Bool {
        guard isNeeded else { return true }
        let path = Self.key(url)
        // Inside a folder already granted: nothing new to keep. (The same folder again renews its bookmark.)
        if granted[path] == nil, covers(path) { return true }
        guard let data = try? Self.bookmark(for: url) else { return covers(path) }
        var isStale = false
        // The bookmark's own URL carries the access across launches; open it now so this launch matches the next.
        if let scoped = try? URL(resolvingBookmarkData: data, options: [.withSecurityScope], relativeTo: nil,
                                 bookmarkDataIsStale: &isStale), scoped.startAccessingSecurityScopedResource() {
            granted[path]?.stopAccessingSecurityScopedResource()
            granted[path] = scoped
        } else {
            granted[path] = url
        }
        var stored = storedBookmarks
        stored[path] = data
        storedBookmarks = stored
        return true
    }

    /// Whether `path` lies in a folder the app may reach.
    func covers(_ path: String) -> Bool {
        guard isNeeded else { return true }
        let path = Self.key(URL(fileURLWithPath: path))
        return granted.keys.contains { Self.path(path, isInside: $0) }
    }

    /// Gives up a grant nothing needs any more: the folder of a project just removed, with no other project in it.
    /// A grant of a folder above the project (the folder of all the repositories, the home folder) stays.
    func release(_ path: String, keepingFor remaining: [String]) {
        let path = Self.key(URL(fileURLWithPath: path))
        guard isNeeded, let url = granted[path],
              !remaining.contains(where: { Self.path(Self.key(URL(fileURLWithPath: $0)), isInside: path) }) else { return }
        url.stopAccessingSecurityScopedResource()
        granted[path] = nil
        var stored = storedBookmarks
        stored[path] = nil
        storedBookmarks = stored
    }

    /// Asks the user for `folder` with an open panel that starts in it, so that granting it is one click. Reports
    /// whether `folder` is reachable afterwards (the user may have picked a folder above it, or another one).
    func requestAccess(to folder: URL, message: String, window: NSWindow?, completion: @escaping (Bool) -> Void) {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.allowsMultipleSelection = false
        panel.canCreateDirectories = false
        panel.directoryURL = folder
        panel.prompt = L("Grant Access", "授权访问")
        panel.message = message
        let finish: (NSApplication.ModalResponse) -> Void = { [weak self] response in
            guard let self else { return }
            if response == .OK, let url = panel.url { self.remember(url) }
            completion(self.covers(folder.path))
        }
        if let window {
            panel.beginSheetModal(for: window, completionHandler: finish)
        } else {
            finish(panel.runModal())
        }
    }

    // MARK: Storage

    private var storedBookmarks: [String: Data] {
        get { UserDefaults.standard.dictionary(forKey: Self.defaultsKey) as? [String: Data] ?? [:] }
        set { UserDefaults.standard.set(newValue, forKey: Self.defaultsKey) }
    }

    private static func bookmark(for url: URL) throws -> Data {
        try url.bookmarkData(options: [.withSecurityScope], includingResourceValuesForKeys: nil, relativeTo: nil)
    }

    /// One spelling per folder, so that a grant and a project's path compare equal (`/private/tmp` is `/tmp`).
    nonisolated static func key(_ url: URL) -> String {
        url.standardizedFileURL.resolvingSymlinksInPath().path
    }

    nonisolated static func path(_ path: String, isInside folder: String) -> Bool {
        if path == folder { return true }
        return path.hasPrefix(folder.hasSuffix("/") ? folder : folder + "/")
    }
}
