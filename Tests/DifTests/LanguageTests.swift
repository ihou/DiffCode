import XCTest
@testable import DifKit

/// The interface language picked in Settings, kept in the app's own `AppleLanguages`. The tests write to a
/// defaults domain of their own, emptied before and after each one, never to the test runner's. One fixed name,
/// so runs do not leave a file each in ~/Library/Preferences.
final class LanguageTests: XCTestCase {
    private let suite = "DifTests.language"
    private var defaults: UserDefaults!

    override func setUp() {
        super.setUp()
        defaults = UserDefaults(suiteName: suite)
        defaults.removePersistentDomain(forName: suite)
    }

    override func tearDown() {
        defaults.removePersistentDomain(forName: suite)
        super.tearDown()
    }

    private var saved: [String]? {
        defaults.persistentDomain(forName: suite)?[Settings.Language.key] as? [String]
    }

    func testEachChoiceIsSavedWhereMacOSKeepsAnAppsLanguage() {
        XCTAssertEqual(Settings.Language.stored(in: defaults, domain: suite), .system)

        Settings.Language.chinese.store(in: defaults)
        XCTAssertEqual(saved, ["zh-Hans"])
        XCTAssertEqual(Settings.Language.stored(in: defaults, domain: suite), .chinese)

        Settings.Language.english.store(in: defaults)
        XCTAssertEqual(saved, ["en"])
        XCTAssertEqual(Settings.Language.stored(in: defaults, domain: suite), .english)

        // Following the system leaves nothing of the app's own behind.
        Settings.Language.system.store(in: defaults)
        XCTAssertNil(saved)
        XCTAssertEqual(Settings.Language.stored(in: defaults, domain: suite), .system)
    }

    func testTheSystemsOwnListIsNotTakenForAChoice() {
        // A suite falls back to the system's languages, as the app's own domain does.
        XCTAssertNotNil(defaults.object(forKey: Settings.Language.key), "this Mac has no system language list to fall back to")
        XCTAssertEqual(Settings.Language.stored(in: defaults, domain: suite), .system)
    }

    func testWhatEachChoiceShows() {
        XCTAssertTrue(Settings.Language.system.isChinese(system: ["zh-Hans-CN", "en"]))
        XCTAssertFalse(Settings.Language.system.isChinese(system: ["en-US", "zh-Hans-US"]))
        XCTAssertFalse(Settings.Language.system.isChinese(system: []))
        XCTAssertTrue(Settings.Language.chinese.isChinese(system: ["en-US"]))
        XCTAssertFalse(Settings.Language.english.isChinese(system: ["zh-Hans"]))
    }

    func testALanguageSetElsewhereReadsAsWhatItShows() {
        // System Settings ▸ Language & Region ▸ Applications writes the same key, with region or script added.
        XCTAssertEqual(Settings.Language(appleLanguages: ["zh-Hans-CN"]), .chinese)
        XCTAssertEqual(Settings.Language(appleLanguages: ["zh-Hant-TW"]), .chinese)
        XCTAssertEqual(Settings.Language(appleLanguages: ["en-GB", "zh-Hans"]), .english)
        // DiffCode shows English for any other language.
        XCTAssertEqual(Settings.Language(appleLanguages: ["ja-JP"]), .english)
        XCTAssertEqual(Settings.Language(appleLanguages: []), .system)
        XCTAssertEqual(Settings.Language(appleLanguages: nil), .system)
    }

    func testTheNamesOfTheLanguagesStayInTheirOwnLanguage() {
        XCTAssertEqual(Settings.Language.english.title, "English")
        XCTAssertEqual(Settings.Language.chinese.title, "简体中文")
        XCTAssertEqual(Settings.Language.allCases, [.system, .english, .chinese])
    }
}
