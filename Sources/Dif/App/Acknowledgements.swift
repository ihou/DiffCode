import AppKit

/// The open-source software built into DiffCode. Their MIT licenses ask for the copyright and permission notice to go
/// with every copy, so the About panel lists each one with its license in full.
///
/// The texts are copied into the app at build time (`Scripts/build-app.sh`, `Contents/Resources/Licenses`) from the
/// very versions the package resolved, so they cannot drift from what is linked in. A bare build (`swift run`) has no
/// bundle and shows the names and links only.
enum Acknowledgements {
    struct Component {
        var name: String
        var url: String
        /// `Contents/Resources/Licenses/<file>.txt`.
        var file: String
        /// Sparkle is only in the direct download.
        var directOnly = false
    }

    static let components = [
        Component(name: "SwiftTerm", url: "https://github.com/migueldeicaza/SwiftTerm", file: "SwiftTerm"),
        Component(name: "Sparkle", url: "https://sparkle-project.org", file: "Sparkle", directOnly: true),
    ]

    /// The components this edition has in it.
    static var included: [Component] {
        components.filter { !$0.directOnly || AppEnvironment.edition == .direct }
    }

    /// A component's license as the build put it in the app, or nil without one (a bare build).
    static func licenseText(_ component: Component, in bundle: Bundle = .main) -> String? {
        guard let url = bundle.resourceURL?.appendingPathComponent("Licenses/\(component.file).txt") else { return nil }
        return try? String(contentsOf: url, encoding: .utf8)
    }

    /// The About panel's credits: what DiffCode is built with, then each license.
    @MainActor static func credits(in bundle: Bundle = .main) -> NSAttributedString {
        let text = NSMutableAttributedString()
        let body = NSFont.systemFont(ofSize: 11)
        let bold = NSFont.systemFont(ofSize: 11, weight: .semibold)
        let mono = NSFont.monospacedSystemFont(ofSize: 9.5, weight: .regular)
        let centered = NSMutableParagraphStyle()
        centered.alignment = .center
        let left = NSMutableParagraphStyle()
        left.alignment = .left
        left.paragraphSpacingBefore = 2

        func add(_ string: String, _ font: NSFont, _ color: NSColor = .labelColor, _ style: NSParagraphStyle = centered,
                 link: String? = nil) {
            var attributes: [NSAttributedString.Key: Any] = [.font: font, .foregroundColor: color, .paragraphStyle: style]
            if let link, let url = URL(string: link) { attributes[.link] = url }
            text.append(NSAttributedString(string: string, attributes: attributes))
        }

        add(L("Built with open-source software:", "DiffCode 使用了以下开源软件：") + "\n", body, .secondaryLabelColor)
        for component in included {
            add(component.name, bold)
            add("  ", body)
            add(component.url, body, .linkColor, link: component.url)
            add("\n", body)
        }
        for component in included {
            guard let license = licenseText(component, in: bundle) else { continue }
            add("\n" + component.name + "\n", bold, .labelColor, left)
            add(license.trimmingCharacters(in: .whitespacesAndNewlines) + "\n", mono, .secondaryLabelColor, left)
        }
        return text
    }
}
