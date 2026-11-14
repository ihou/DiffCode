import AppKit
import DifKit

// The Mac App Store edition: sandboxed, and updated by the App Store, so nothing of Sparkle is linked in.
MainActor.assumeIsolated {
    DiffCodeApp.run(edition: .appStore)
}
