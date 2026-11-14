import Foundation

/// The App Sandbox the Mac App Store edition runs in. Outside it (the direct download) everything here is inert.
///
/// In the sandbox the app reaches a folder only once the user has chosen it, and every process it starts, git
/// included, inherits what the app may reach at that moment.
enum Sandbox {
    /// The system sets this for every sandboxed process.
    static let isActive = ProcessInfo.processInfo.environment["APP_SANDBOX_CONTAINER_ID"] != nil

    /// The user's own home folder. In the sandbox `NSHomeDirectory()` and `$HOME` are the app's container instead.
    static let realHome: String = {
        if let entry = getpwuid(getuid()), let directory = entry.pointee.pw_dir {
            return String(cString: directory)
        }
        return NSHomeDirectory()
    }()

    /// The user's global Git settings: who commits, global ignores, aliases.
    static var globalGitConfig: String { (realHome as NSString).appendingPathComponent(".gitconfig") }

    /// Whether the home folder itself can be read: outside the sandbox always, inside once the user granted it.
    static var canReadHome: Bool { !isActive || FileManager.default.isReadableFile(atPath: realHome) }

    /// Whether git can see the user's global settings (and so knows who is committing).
    static var canReadGlobalGitConfig: Bool {
        !isActive || canReadHome || FileManager.default.isReadableFile(atPath: globalGitConfig)
    }
}
