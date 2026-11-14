import AppKit
import DifKit
import Sparkle

/// The direct download keeps itself up to date with Sparkle, from the feed and public key in its Info.plist.
/// A build without them (`swift run`, a local build without the release config) does not look for updates.
@MainActor final class SparkleUpdater: AppUpdater {
    private let controller: SPUStandardUpdaterController

    init?(bundle: Bundle = .main) {
        func value(_ key: String) -> String? {
            (bundle.object(forInfoDictionaryKey: key) as? String).flatMap { $0.trimmingCharacters(in: .whitespaces).isEmpty ? nil : $0 }
        }
        guard value("SUFeedURL") != nil, value("SUPublicEDKey") != nil else { return nil }
        controller = SPUStandardUpdaterController(startingUpdater: false, updaterDelegate: nil, userDriverDelegate: nil)
    }

    func start() { controller.startUpdater() }
    func checkForUpdates() { controller.checkForUpdates(nil) }
    var canCheckForUpdates: Bool { controller.updater.canCheckForUpdates }
    var automaticallyChecksForUpdates: Bool {
        get { controller.updater.automaticallyChecksForUpdates }
        set { controller.updater.automaticallyChecksForUpdates = newValue }
    }
}

MainActor.assumeIsolated {
    DiffCodeApp.run(edition: .direct, updater: SparkleUpdater())
}
