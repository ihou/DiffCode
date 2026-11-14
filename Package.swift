// swift-tools-version:5.9
import PackageDescription

let package = Package(
    name: "Dif",
    platforms: [.macOS(.v14)],
    products: [
        // Downloaded from the web: updates itself with Sparkle.
        .executable(name: "Dif", targets: ["Dif"]),
        // Mac App Store: sandboxed and updated by the App Store, so no Sparkle in it at all.
        .executable(name: "DifAppStore", targets: ["DifAppStore"]),
    ],
    dependencies: [
        // The terminal emulator behind the integrated terminal.
        .package(url: "https://github.com/migueldeicaza/SwiftTerm", exact: "1.11.2"),
        // Updates for the direct download.
        .package(url: "https://github.com/sparkle-project/Sparkle", exact: "2.10.1"),
    ],
    targets: [
        // The whole app. The two editions below only say how it is updated and what the sandbox allows.
        .target(
            name: "DifKit",
            dependencies: [.product(name: "SwiftTerm", package: "SwiftTerm")],
            path: "Sources/Dif",
            // Full data-race checking. The app builds without a single warning; keep it that way. (Not the Swift 6
            // language mode: its run-time isolation checks would turn a callback that arrives on an unexpected
            // thread into a crash, where today it is at worst a race the checker should have caught here.)
            swiftSettings: [.enableUpcomingFeature("StrictConcurrency")]
        ),
        .executableTarget(
            name: "Dif",
            dependencies: ["DifKit", .product(name: "Sparkle", package: "Sparkle")],
            path: "Sources/Editions/Direct",
            // Sparkle.framework sits in Contents/Frameworks of the app bundle.
            linkerSettings: [.unsafeFlags(["-Xlinker", "-rpath", "-Xlinker", "@executable_path/../Frameworks"])]
        ),
        .executableTarget(
            name: "DifAppStore",
            dependencies: ["DifKit"],
            path: "Sources/Editions/AppStore"
        ),
        .testTarget(
            name: "DifTests",
            dependencies: ["DifKit"],
            path: "Tests/DifTests"
        ),
    ]
)
