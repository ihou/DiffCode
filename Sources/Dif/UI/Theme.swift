import AppKit
import SwiftUI

/// The name people see: menus, window title, About, messages. The code and the `dif` command keep the old
/// short name. The bundle identifier is `com.nanshanbei.DiffCode`; builds before 2026-11-13 were
/// `app.dif.viewer`, whose settings `LegacyDefaults` copies over once.
let appName = "DiffCode"

// MARK: - Localisation

enum Localization {
    static let isChinese: Bool = Locale.preferredLanguages.first?.hasPrefix("zh") ?? false
}

/// Picks the English or Chinese string to match the system language.
func L(_ en: String, _ zh: String) -> String { Localization.isChinese ? zh : en }

// MARK: - Colours

extension NSColor {
    convenience init(hex: UInt32, alpha: CGFloat = 1) {
        self.init(srgbRed: CGFloat((hex >> 16) & 0xFF) / 255, green: CGFloat((hex >> 8) & 0xFF) / 255,
                  blue: CGFloat(hex & 0xFF) / 255, alpha: alpha)
    }

    static func dynamic(light: NSColor, dark: NSColor) -> NSColor {
        NSColor(name: nil) { appearance in
            appearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua ? dark : light
        }
    }

    static func dyn(_ light: UInt32, _ dark: UInt32, lightAlpha: CGFloat = 1, darkAlpha: CGFloat = 1) -> NSColor {
        dynamic(light: NSColor(hex: light, alpha: lightAlpha), dark: NSColor(hex: dark, alpha: darkAlpha))
    }
}

/// Chrome colours, resolved automatically for light and dark appearance.
enum Palette {
    static let window = NSColor.dyn(0xFFFFFF, 0x1E1F22)
    static let sidebar = NSColor.dyn(0xF0F1F4, 0x232428)
    static let panel = NSColor.dyn(0xF7F8FA, 0x2B2D30)
    static let elevated = NSColor.dyn(0xFFFFFF, 0x2F3136)
    static let border = NSColor.dyn(0xE1E3E8, 0x14151A)
    static let hairline = NSColor.dyn(0xEBECF0, 0x393B40)
    static let textPrimary = NSColor.dyn(0x1F2329, 0xDFE1E5)
    static let textSecondary = NSColor.dyn(0x6C707E, 0x9DA0A8)
    static let textTertiary = NSColor.dyn(0xA2A6B3, 0x868A91)
    static let accent = NSColor.dyn(0x3574F0, 0x548AF7)
    static let hover = NSColor.dyn(0x000000, 0xFFFFFF, lightAlpha: 0.05, darkAlpha: 0.06)
    static let selection = NSColor.dyn(0x3574F0, 0x548AF7, lightAlpha: 0.16, darkAlpha: 0.28)
    static let chip = NSColor.dyn(0x000000, 0xFFFFFF, lightAlpha: 0.06, darkAlpha: 0.09)
    static let added = NSColor.dyn(0x1E8A34, 0x73BD79)
    static let removed = NSColor.dyn(0xC7392F, 0xE5736B)

    static func status(_ kind: FileChange.Kind?) -> NSColor {
        switch kind {
        case .modified: return NSColor.dyn(0x1F5FD0, 0x6CA6F5)
        case .added: return added
        case .deleted: return NSColor.dyn(0x7A7E87, 0x8C9199)
        case .renamed: return NSColor.dyn(0x17858A, 0x4EC9B0)
        case .untracked: return NSColor.dyn(0xB3402E, 0xE0766B)
        case .conflicted: return NSColor.dyn(0xD12F2F, 0xF26C6C)
        case nil: return textPrimary
        }
    }
}

extension Color {
    init(_ ns: NSColor) { self.init(nsColor: ns) }
    static let difWindow = Color(Palette.window)
    static let difSidebar = Color(Palette.sidebar)
    static let difPanel = Color(Palette.panel)
    static let difElevated = Color(Palette.elevated)
    static let difBorder = Color(Palette.border)
    static let difHairline = Color(Palette.hairline)
    static let difText = Color(Palette.textPrimary)
    static let difText2 = Color(Palette.textSecondary)
    static let difText3 = Color(Palette.textTertiary)
    static let difAccent = Color(Palette.accent)
    static let difHover = Color(Palette.hover)
    static let difSelection = Color(Palette.selection)
    static let difChip = Color(Palette.chip)
    static let difAdded = Color(Palette.added)
    static let difRemoved = Color(Palette.removed)
    static func status(_ kind: FileChange.Kind?) -> Color { Color(Palette.status(kind)) }
}

// MARK: - Code canvas theme

/// Pre-resolved CGColors for the code canvas, so drawing never touches dynamic colour lookup.
struct CanvasTheme {
    let isDark: Bool
    let background: CGColor
    let text: CGColor
    let gutterText: CGColor
    let gutterTextStrong: CGColor
    let syntax: [CGColor]
    // Side-by-side follows the IntelliJ convention: blue = modified, green = inserted, grey = deleted.
    let modified: CGColor, modifiedWord: CGColor, modifiedMark: CGColor
    let added: CGColor, addedWord: CGColor, addedMark: CGColor
    let deleted: CGColor, deletedWord: CGColor, deletedMark: CGColor
    // Unified view uses the universal red/green.
    let removedLine: CGColor, removedWord: CGColor
    let selection: CGColor
    let searchMatch: CGColor
    let searchCurrent: CGColor
    let focusLine: CGColor
    let separator: CGColor
    let rulerThumb: CGColor
    let accent: CGColor

    private static func c(_ hex: UInt32, _ alpha: CGFloat = 1) -> CGColor { NSColor(hex: hex, alpha: alpha).cgColor }

    static let dark = CanvasTheme(
        isDark: true,
        background: c(0x1E1F22),
        text: c(0xBCBEC4),
        gutterText: c(0x4E525A),
        gutterTextStrong: c(0xA1A5AD),
        syntax: [
            c(0xBCBEC4), // plain
            c(0xCF8E6D), // keyword
            c(0x6AAB73), // string
            c(0x7A7E85), // comment
            c(0x2AACB8), // number
            c(0xB3AE60), // annotation
            c(0x56A8F5), // function
            c(0xC77DBB), // constant
            c(0xD5B778), // tag
            c(0xBABABA), // attribute
        ],
        modified: c(0x3574F0, 0.20), modifiedWord: c(0x3574F0, 0.40), modifiedMark: c(0x5C92F7),
        added: c(0x3FA65A, 0.20), addedWord: c(0x3FA65A, 0.42), addedMark: c(0x57B46F),
        deleted: c(0x868A93, 0.20), deletedWord: c(0x868A93, 0.40), deletedMark: c(0x8E929B),
        removedLine: c(0xE5534B, 0.17), removedWord: c(0xE5534B, 0.38),
        selection: c(0x2F65CA, 0.55),
        searchMatch: c(0xE8B339, 0.35),
        searchCurrent: c(0xF5A623, 0.85),
        focusLine: c(0xFFFFFF, 0.06),
        separator: c(0x393B40),
        rulerThumb: c(0xFFFFFF, 0.12),
        accent: c(0x548AF7)
    )

    static let light = CanvasTheme(
        isDark: false,
        background: c(0xFFFFFF),
        text: c(0x1F2329),
        gutterText: c(0xB4B8C2),
        gutterTextStrong: c(0x5A5E69),
        syntax: [
            c(0x1F2329), // plain
            c(0x0033B3), // keyword
            c(0x067D17), // string
            c(0x8C8C8C), // comment
            c(0x1750EB), // number
            c(0x9E880D), // annotation
            c(0x00627A), // function
            c(0x871094), // constant
            c(0x0033B3), // tag
            c(0x174AD4), // attribute
        ],
        modified: c(0x3574F0, 0.13), modifiedWord: c(0x3574F0, 0.28), modifiedMark: c(0x3574F0),
        added: c(0x2DA44E, 0.15), addedWord: c(0x2DA44E, 0.32), addedMark: c(0x2DA44E),
        deleted: c(0x6E7481, 0.14), deletedWord: c(0x6E7481, 0.28), deletedMark: c(0x868B97),
        removedLine: c(0xE5534B, 0.12), removedWord: c(0xE5534B, 0.28),
        selection: c(0x3574F0, 0.28),
        searchMatch: c(0xF5C542, 0.45),
        searchCurrent: c(0xF59E0B, 0.90),
        focusLine: c(0x000000, 0.045),
        separator: c(0xEBECF0),
        rulerThumb: c(0x000000, 0.10),
        accent: c(0x3574F0)
    )

    func line(_ kind: Hunk.Kind) -> CGColor {
        switch kind {
        case .modified: return modified
        case .added: return added
        case .deleted: return deleted
        }
    }

    func word(_ kind: Hunk.Kind) -> CGColor {
        switch kind {
        case .modified: return modifiedWord
        case .added: return addedWord
        case .deleted: return deletedWord
        }
    }

    func mark(_ kind: Hunk.Kind) -> CGColor {
        switch kind {
        case .modified: return modifiedMark
        case .added: return addedMark
        case .deleted: return deletedMark
        }
    }
}

/// The monospace faces code and terminals are drawn in: a family picked in Settings, else JetBrains Mono when it is
/// installed, else the system's own monospace face. A picked family that is no longer installed counts as none.
enum MonospaceFonts {
    /// What "Default" stands for in Settings.
    static var defaultName: String {
        NSFont(name: "JetBrainsMono-Regular", size: 12) != nil ? "JetBrains Mono" : "SF Mono"
    }

    static func font(family: String?, size: CGFloat) -> NSFont {
        if let family, let font = regular(family, size: size) { return font }
        return NSFont(name: "JetBrainsMono-Regular", size: size)
            ?? NSFont(name: "JetBrains Mono", size: size)
            ?? NSFont.monospacedSystemFont(ofSize: size, weight: .regular)
    }

    /// The regular face of an installed family; nil when no family has that name.
    static func regular(_ family: String, size: CGFloat) -> NSFont? {
        guard let font = NSFont(descriptor: NSFontDescriptor(fontAttributes: [.family: family]), size: size),
              font.familyName == family else { return nil }
        return font
    }

    /// The installed families code can be drawn in, by name: those whose letters, digits and punctuation all have
    /// one width (the canvas puts characters in columns). Tens of milliseconds, so not on the main thread.
    static func installed() -> [String] {
        let families = CTFontManagerCopyAvailableFontFamilyNames() as? [String] ?? []
        return families
            .filter { !$0.hasPrefix(".") && (regular($0, size: 12).map(isMonospaced) ?? false) }
            .sorted { $0.localizedStandardCompare($1) == .orderedAscending }
    }

    /// Measured rather than read from the font's flags: some faces flagged monospace have Latin letters of
    /// different widths, and patched faces (Nerd Fonts) may lose the flag although their letters keep one width.
    static func isMonospaced(_ font: NSFont) -> Bool {
        let characters = Array("iMW0.l_m ".utf16)
        var glyphs = [CGGlyph](repeating: 0, count: characters.count)
        guard CTFontGetGlyphsForCharacters(font, characters, &glyphs, characters.count) else { return false }
        var advances = [CGSize](repeating: .zero, count: glyphs.count)
        CTFontGetAdvancesForGlyphs(font, .horizontal, glyphs, &advances, glyphs.count)
        let width = advances[0].width
        return width > 0 && advances.allSatisfy { abs($0.width - width) < 0.01 }
    }
}

/// Font and derived metrics for code rendering.
struct CodeFont: Equatable {
    let font: NSFont
    let size: CGFloat
    let lineHeight: CGFloat
    let baseline: CGFloat
    let charWidth: CGFloat

    /// `family`: a family picked in Settings, or nil for the default face.
    init(size: CGFloat, family: String? = nil) {
        self.size = size
        font = MonospaceFonts.font(family: family, size: size)
        lineHeight = (size * 1.5).rounded()
        let ascent = CTFontGetAscent(font), descent = CTFontGetDescent(font)
        baseline = ((lineHeight - (ascent + descent)) / 2 + ascent).rounded()
        charWidth = ("0" as NSString).size(withAttributes: [.font: font]).width
    }

    /// The code font Settings asks for.
    @MainActor
    static func current(_ settings: Settings = .shared) -> CodeFont {
        CodeFont(size: settings.fontSize, family: settings.codeFontFamily)
    }

    static func == (a: CodeFont, b: CodeFont) -> Bool { a.size == b.size && a.font.fontName == b.font.fontName }
}

/// Interface metrics. Sizes in the views are written for the default 13 pt interface font and
/// scaled from there, so one setting resizes every list, header and popup together.
///
/// The scale is read from the observable settings, so any view body that asks for a font or a
/// metric is re-evaluated automatically when the setting changes.
@MainActor
enum UI {
    static let baseFontSize: CGFloat = 13

    static var scale: CGFloat { CGFloat(Settings.shared.uiFontSize) / baseFontSize }

    /// A length that should grow with the interface font (row heights, icon boxes), in whole points.
    static func s(_ value: CGFloat) -> CGFloat { (value * scale).rounded() }
}

extension Font {
    /// Interface text. `size` is the size at the default interface font size.
    @MainActor
    static func ui(size: CGFloat, weight: Font.Weight = .regular, design: Font.Design = .default) -> Font {
        .system(size: size * UI.scale, weight: weight, design: design)
    }

    /// The UI's monospace face, matching the code canvas.
    @MainActor
    static func code(_ size: CGFloat, weight: Font.Weight = .regular) -> Font {
        let scaled = size * UI.scale
        if let family = Settings.shared.codeFontFamily, let font = MonospaceFonts.regular(family, size: scaled) {
            return .custom(font.fontName, size: scaled).weight(weight)
        }
        if NSFont(name: "JetBrainsMono-Regular", size: scaled) != nil {
            return .custom("JetBrains Mono", size: scaled).weight(weight)
        }
        return .system(size: scaled, weight: weight, design: .monospaced)
    }
}
