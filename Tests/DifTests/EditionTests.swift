import XCTest
@testable import DifKit

/// What differs between the direct download and the Mac App Store edition, as far as it can be checked outside
/// the sandbox (the sandbox itself is exercised with a sandboxed build of the app).
final class EditionTests: XCTestCase {
    func testGrantCoversFolderAndEverythingInIt() {
        XCTAssertTrue(FolderAccess.path("/Users/me/work", isInside: "/Users/me/work"))
        XCTAssertTrue(FolderAccess.path("/Users/me/work/app", isInside: "/Users/me/work"))
        XCTAssertTrue(FolderAccess.path("/Users/me/work/app", isInside: "/Users/me/"))
        XCTAssertTrue(FolderAccess.path("/Users/me", isInside: "/"))
        // A sibling that merely starts with the same letters is not inside.
        XCTAssertFalse(FolderAccess.path("/Users/me/workshop", isInside: "/Users/me/work"))
        XCTAssertFalse(FolderAccess.path("/Users/me", isInside: "/Users/me/work"))
    }

    func testChildProcessesDoNotInheritTheAppsMallocMode() {
        // Info.plist gives the app MallocSpaceEfficient; git and the terminal's programs must run with the normal allocator.
        setenv("MallocSpaceEfficient", "1", 1)
        defer { unsetenv("MallocSpaceEfficient") }
        XCTAssertEqual(ProcessInfo.processInfo.environment["MallocSpaceEfficient"], "1")
        XCTAssertNil(AppEnvironment.childEnvironment["MallocSpaceEfficient"])
        XCTAssertFalse(Shell.environment(directory: "/tmp").contains { $0.hasPrefix("MallocSpaceEfficient=") })
        XCTAssertNotNil(AppEnvironment.childEnvironment["PATH"], "everything else is passed on")
    }

    func testOneSpellingPerFolder() throws {
        // /tmp is a link to /private/tmp; a grant made under one name must cover a project found under the other.
        let folder = URL(fileURLWithPath: "/private/tmp").appendingPathComponent("dif-edition-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: folder) }
        let viaLink = URL(fileURLWithPath: "/tmp/" + folder.lastPathComponent)
        XCTAssertEqual(FolderAccess.key(folder), FolderAccess.key(viaLink))
        XCTAssertEqual(FolderAccess.key(URL(fileURLWithPath: folder.path + "/./sub/..")), FolderAccess.key(folder))
    }

    @MainActor
    func testOutsideTheSandboxEverythingIsReachable() {
        XCTAssertFalse(Sandbox.isActive)
        let access = FolderAccess()
        XCTAssertFalse(access.isNeeded)
        XCTAssertTrue(access.covers("/anywhere/at/all"))
        XCTAssertTrue(access.remember(URL(fileURLWithPath: "/anywhere")))
        XCTAssertTrue(access.granted.isEmpty, "nothing is bookmarked outside the sandbox")
        XCTAssertTrue(Git.isAvailable)
        XCTAssertTrue(Sandbox.canReadHome)
        XCTAssertTrue(Sandbox.canReadGlobalGitConfig)
    }

    func testGitIsLookedForWhereTheSandboxCanRunIt() {
        let candidates = Git.candidates
        // Homebrew's first (what a shell would find first), then the developer tools' own binaries.
        XCTAssertEqual(Array(candidates.prefix(2)), ["/opt/homebrew/bin/git", "/usr/local/bin/git"])
        XCTAssertTrue(candidates.contains("/Library/Developer/CommandLineTools/usr/bin/git"))
        XCTAssertTrue(candidates.contains("/Applications/Xcode.app/Contents/Developer/usr/bin/git"))
        // Never the shim, which refuses to run in the sandbox.
        XCTAssertFalse(candidates.contains("/usr/bin/git"))
        XCTAssertTrue(FileManager.default.isExecutableFile(atPath: Git.executable))
    }

    func testTheDirectDownloadHasTheTerminal() {
        XCTAssertFalse(AppEnvironment.isAppStore)
        XCTAssertTrue(AppEnvironment.hasTerminal)
    }
}

/// The bundle identifier changed from `app.dif.viewer`; the direct download brings the old settings along once.
final class LegacyDefaultsTests: XCTestCase {
    private var legacyName = ""
    private var currentName = ""

    override func setUp() {
        legacyName = "dif-test-legacy-\(UUID().uuidString)"
        currentName = "dif-test-current-\(UUID().uuidString)"
    }

    override func tearDown() {
        UserDefaults.standard.removePersistentDomain(forName: legacyName)
        UserDefaults.standard.removePersistentDomain(forName: currentName)
    }

    func testCopiesOnceWithoutOverwriting() throws {
        UserDefaults.standard.setPersistentDomain(["projects": ["/Users/me/app"], "fontSize": 14.0, "appearance": "dark"],
                                                  forName: legacyName)
        let current = try XCTUnwrap(UserDefaults(suiteName: currentName))
        current.set("light", forKey: "appearance") // already chosen in the new app: kept

        LegacyDefaults.migrateIfNeeded(into: current, domain: currentName, from: legacyName)
        XCTAssertEqual(current.stringArray(forKey: "projects"), ["/Users/me/app"])
        XCTAssertEqual(current.double(forKey: "fontSize"), 14)
        XCTAssertEqual(current.string(forKey: "appearance"), "light")
        XCTAssertTrue(current.bool(forKey: LegacyDefaults.migratedKey))

        // Once only: a project removed afterwards does not come back on the next launch.
        current.removeObject(forKey: "projects")
        LegacyDefaults.migrateIfNeeded(into: current, domain: currentName, from: legacyName)
        XCTAssertNil(current.object(forKey: "projects"))
        // And the old domain is left alone.
        XCTAssertEqual(UserDefaults.standard.persistentDomain(forName: legacyName)?["fontSize"] as? Double, 14)
    }

    func testNothingToDoWithoutABundleOrForTheOldAppItself() throws {
        UserDefaults.standard.setPersistentDomain(["projects": ["/x"]], forName: legacyName)
        let current = try XCTUnwrap(UserDefaults(suiteName: currentName))
        LegacyDefaults.migrateIfNeeded(into: current, domain: nil, from: legacyName)
        LegacyDefaults.migrateIfNeeded(into: current, domain: legacyName, from: legacyName)
        XCTAssertNil(current.object(forKey: "projects"))
        XCTAssertFalse(current.bool(forKey: LegacyDefaults.migratedKey))
    }
}
