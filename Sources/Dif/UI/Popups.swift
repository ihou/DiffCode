import SwiftUI

/// Dims the window and floats the active popup near the top, Spotlight-style.
struct PopupLayer: View {
    var popup: Popup
    @Environment(AppStore.self) private var store

    var body: some View {
        switch popup {
        case .gitGraph:
            GitGraphLayer()
        case .dialog:
            if let request = store.dialog {
                ZStack {
                    Color.black.opacity(0.28)
                        .contentShape(Rectangle())
                        .onTapGesture { store.answerDialog(confirmed: false) }
                    DialogView(request: request)
                        .frame(width: UI.s(440))
                        .popupPanel()
                        .padding(24)
                }
            }
        default:
            ZStack(alignment: .top) {
                Color.black.opacity(0.22)
                    .contentShape(Rectangle())
                    .onTapGesture { store.dismissPopup() }
                Group {
                    switch popup {
                    case .quickOpen: QuickOpenView().frame(width: UI.s(660))
                    case .findInPath: FindInPathView().frame(width: UI.s(900), height: 640)
                    case .goToLine: GoToLineView().frame(width: UI.s(320))
                    case .keymap: KeymapView().frame(width: UI.s(720))
                    case .settings: SettingsView().frame(width: UI.s(460))
                    case .gitGraph, .dialog: EmptyView()
                    }
                }
                .popupPanel()
                .padding(.top, popup == .findInPath ? 56 : 90)
                .padding(.horizontal, 24)
                .padding(.bottom, 24) // so a tall popup shrinks (and scrolls) in a short window
            }
        }
    }
}

// MARK: - Quick open

struct QuickOpenView: View {
    @Environment(AppStore.self) private var store

    var body: some View {
        @Bindable var model = store.quickOpen
        VStack(spacing: 0) {
            HStack(spacing: 10) {
                Image(systemName: "magnifyingglass").font(.ui(size: 15, weight: .medium)).foregroundStyle(Color.difText2)
                SearchField(text: $model.query, placeholder: placeholder(model.scope), font: .systemFont(ofSize: 16 * UI.scale),
                            focusToken: model.focusToken,
                            onMove: { model.move($0) },
                            onSubmit: { model.activate() },
                            onCancel: { store.dismissPopup() },
                            onTab: { model.cycleScope($0 ? -1 : 1) })
                    .frame(height: UI.s(24))
                if model.isIndexing { ProgressView().controlSize(.small).scaleEffect(0.7) }
                if model.scope != .recent, model.scope != .branches, model.scope != .stashes {
                    HStack(spacing: 2) {
                        ScopeTab(title: L("All", "全部"), isSelected: model.scope == .all) { model.setScope(.all) }
                        ScopeTab(title: L("Files", "文件"), isSelected: model.scope == .files) { model.setScope(.files) }
                        ScopeTab(title: L("Actions", "操作"), isSelected: model.scope == .actions) { model.setScope(.actions) }
                    }
                }
            }
            .padding(.horizontal, 16)
            .frame(height: UI.s(52))

            Rectangle().fill(Color.difHairline).frame(height: 1)

            if model.items.isEmpty {
                Text(emptyText(model))
                    .font(.ui(size: 12.5))
                    .foregroundStyle(Color.difText3)
                    .frame(maxWidth: .infinity)
                    .frame(height: UI.s(76))
            } else {
                ScrollViewReader { proxy in
                    ScrollView {
                        LazyVStack(spacing: 0) {
                            ForEach(Array(model.items.enumerated()), id: \.element.id) { index, item in
                                QuickOpenRow(item: item, isSelected: index == model.selected)
                                    .id(index)
                                    .contentShape(Rectangle())
                                    .onTapGesture { model.activate(index) }
                            }
                        }
                        .padding(.vertical, 6)
                    }
                    .frame(maxHeight: 440)
                    .fixedSize(horizontal: false, vertical: true)
                    .onChange(of: model.selected) { _, index in proxy.scrollTo(index) }
                }
            }

            Rectangle().fill(Color.difHairline).frame(height: 1)
            HStack(spacing: 14) {
                Hint(keys: "↑↓", text: L("Navigate", "选择"))
                if model.scope == .branches {
                    Hint(keys: "↩", text: L("Compare", "比较"))
                    Spacer()
                    Text(L("Branches compare from where they forked from HEAD · or type a commit", "分支从与 HEAD 的分叉点开始比较 · 也可输入提交哈希"))
                        .font(.ui(size: 11))
                        .foregroundStyle(Color.difText3)
                        .lineLimit(1)
                } else if model.scope == .stashes {
                    Hint(keys: "↩", text: L("View", "查看"))
                    Spacer()
                    Text(L("Apply, pop or drop it from the bar over the list", "在列表上方的横条里应用、弹出或删除"))
                        .font(.ui(size: 11))
                        .foregroundStyle(Color.difText3)
                        .lineLimit(1)
                } else {
                    Hint(keys: "↩", text: L("Open", "打开"))
                    Hint(keys: "⇥", text: L("Switch tab", "切换标签"))
                    Spacer()
                    Text(L("Tip: add :42 to jump to a line", "提示：末尾加 :42 可直接跳到该行"))
                        .font(.ui(size: 11))
                        .foregroundStyle(Color.difText3)
                }
            }
            .padding(.horizontal, 14)
            .frame(height: UI.s(30))
        }
        .onChange(of: model.query) { _, _ in model.update() }
    }

    private func placeholder(_ scope: QuickOpenModel.Scope) -> String {
        switch scope {
        case .all: return L("Search files and actions", "搜索文件和操作")
        case .files: return L("Go to file — try abbreviations like \"mavm\"", "查找文件 — 支持缩写，例如 mavm")
        case .actions: return L("Find an action", "查找操作")
        case .recent: return L("Recent files", "最近的文件")
        case .branches: return L("Compare the working tree with a branch, tag or commit", "将工作区与分支、标签或提交比较")
        case .stashes: return L("Stashes", "Stash 列表")
        }
    }

    private func emptyText(_ model: QuickOpenModel) -> String {
        if model.scope == .branches {
            return model.isIndexing ? L("Reading branches…", "正在读取分支…") : L("No branches · type a commit hash", "没有分支 · 可输入提交哈希")
        }
        if model.scope == .stashes {
            if model.isIndexing { return L("Reading stashes…", "正在读取 stash…") }
            return model.query.isEmpty ? L("No stashes", "没有 stash") : L("Nothing found", "没有找到")
        }
        if model.isIndexing { return L("Indexing files…", "正在索引文件…") }
        return model.query.isEmpty ? L("Type to search", "输入以搜索") : L("Nothing found", "没有找到")
    }
}

private struct ScopeTab: View {
    var title: String
    var isSelected: Bool
    var action: () -> Void

    var body: some View {
        Button(action: action) {
            Text(title)
                .font(.ui(size: 11.5, weight: isSelected ? .semibold : .regular))
                .foregroundStyle(isSelected ? Color.difText : Color.difText2)
                .padding(.horizontal, 8)
                .padding(.vertical, 3)
                .background(RoundedRectangle(cornerRadius: 5, style: .continuous).fill(isSelected ? Color.difChip : Color.clear))
        }
        .buttonStyle(.plain)
    }
}

private struct Hint: View {
    var keys: String
    var text: String

    var body: some View {
        HStack(spacing: 4) {
            KeyCaps(shortcut: keys, size: 10)
            Text(text).font(.ui(size: 11)).foregroundStyle(Color.difText3)
        }
    }
}

private struct QuickOpenRow: View {
    var item: QuickOpenModel.Item
    var isSelected: Bool
    @Environment(AppStore.self) private var store

    var body: some View {
        HStack(spacing: 9) {
            switch item {
            case .file(let projectID, let path, let positions, let kind):
                let name = (path as NSString).lastPathComponent
                let directory = (path as NSString).deletingLastPathComponent
                FileIcon(path: path, size: 13)
                Text(FuzzyHighlight.text(name, positions: positions, offset: path.utf8.count - name.utf8.count, base: Color.status(kind)))
                    .font(.ui(size: 13))
                    .lineLimit(1)
                    .layoutPriority(1)
                Text(FuzzyHighlight.text(directory, positions: positions, offset: 0, base: Color.difText3, size: 11.5))
                    .font(.ui(size: 11.5))
                    .lineLimit(1)
                    .truncationMode(.head)
                Spacer(minLength: 6)
                if projectID != store.selectedProjectID, let owner = store.project(projectID) {
                    Text(owner.name).font(.ui(size: 10.5)).foregroundStyle(Color.difText3)
                }
                if let kind {
                    Text(kind.letter).font(.ui(size: 10.5, weight: .bold, design: .rounded)).foregroundStyle(Color.status(kind))
                }
            case .action(let command):
                Image(systemName: command.symbol)
                    .font(.ui(size: 12))
                    .foregroundStyle(Color.difText2)
                    .frame(width: UI.s(18))
                Text(command.title).font(.ui(size: 13)).foregroundStyle(Color.difText).lineLimit(1)
                Spacer(minLength: 6)
                KeyCaps(shortcut: command.shortcut)
            case .ref(let ref, let positions):
                Image(systemName: ref.kind == .tag ? "tag" : (ref.kind == .remote ? "cloud" : "arrow.triangle.branch"))
                    .font(.ui(size: 12))
                    .foregroundStyle(Color.difText2)
                    .frame(width: UI.s(18))
                Text(FuzzyHighlight.text(ref.shortName, positions: positions, offset: 0, base: Color.difText))
                    .font(.ui(size: 13))
                    .lineLimit(1)
                    .truncationMode(.middle)
                if ref.isHead {
                    Text(L("current", "当前")).font(.ui(size: 10.5, weight: .semibold)).foregroundStyle(Color.difAdded)
                }
                Spacer(minLength: 6)
                Text(ref.date.formatted(.relative(presentation: .named)))
                    .font(.ui(size: 11))
                    .foregroundStyle(Color.difText3)
                    .lineLimit(1)
            case .revision(let text):
                Image(systemName: "number")
                    .font(.ui(size: 12))
                    .foregroundStyle(Color.difText2)
                    .frame(width: UI.s(18))
                Text(L("Compare with commit “\(text)”", "与提交“\(text)”比较")).font(.ui(size: 13)).foregroundStyle(Color.difText).lineLimit(1)
                Spacer(minLength: 6)
            case .stash(let entry, let positions):
                Image(systemName: "tray.full")
                    .font(.ui(size: 12))
                    .foregroundStyle(Color.difText2)
                    .frame(width: UI.s(18))
                Text(FuzzyHighlight.text(entry.title, positions: positions, offset: 0, base: Color.difText))
                    .font(.ui(size: 13))
                    .lineLimit(1)
                    .truncationMode(.tail)
                    .layoutPriority(1)
                Text([entry.branch.map { L("on \($0)", "在 \($0)") }, entry.untracked != nil ? L("with new files", "含新文件") : nil]
                        .compactMap { $0 }.joined(separator: " · "))
                    .font(.ui(size: 11.5))
                    .foregroundStyle(Color.difText3)
                    .lineLimit(1)
                Spacer(minLength: 6)
                Text(entry.date.formatted(.relative(presentation: .named)))
                    .font(.ui(size: 11))
                    .foregroundStyle(Color.difText3)
                    .lineLimit(1)
            case .stashChanges(let count):
                Image(systemName: "tray.and.arrow.down")
                    .font(.ui(size: 12))
                    .foregroundStyle(Color.difAccent)
                    .frame(width: UI.s(18))
                Text(L("Stash Local Changes…", "Stash 本地改动…")).font(.ui(size: 13)).foregroundStyle(Color.difText).lineLimit(1)
                Text(count == 1 ? L("1 file", "1 个文件") : L("\(count) files", "\(count) 个文件"))
                    .font(.ui(size: 11.5))
                    .foregroundStyle(Color.difText3)
                Spacer(minLength: 6)
            }
        }
        .padding(.horizontal, 12)
        .frame(height: UI.s(30))
        .background(
            RoundedRectangle(cornerRadius: 7, style: .continuous)
                .fill(isSelected ? Color.difSelection : Color.clear)
                .padding(.horizontal, 6)
        )
    }
}

/// Text styling goes through the attribute keys (`text[Attr.Foreground.self] = …`): the shorthand
/// `text.foregroundColor = …` forms key paths that SwiftUI has not marked Sendable, which the race checker flags.
enum Attr {
    typealias Foreground = AttributeScopes.SwiftUIAttributes.ForegroundColorAttribute
    typealias Background = AttributeScopes.SwiftUIAttributes.BackgroundColorAttribute
    typealias Font = AttributeScopes.SwiftUIAttributes.FontAttribute
}

@MainActor
enum FuzzyHighlight {
    /// Emphasises the characters the fuzzy matcher landed on. `positions` are byte offsets into the full path.
    static func text(_ text: String, positions: [Int], offset: Int, base: Color, size: CGFloat = 13) -> AttributedString {
        var result = AttributedString(text)
        result[Attr.Foreground.self] = base
        guard !positions.isEmpty else { return result }
        let utf8 = text.utf8
        for position in positions {
            let local = position - offset
            guard local >= 0, local < utf8.count else { continue }
            let start = utf8.index(utf8.startIndex, offsetBy: local)
            guard let lower = start.samePosition(in: text), lower < text.endIndex else { continue }
            let upper = text.index(after: lower)
            if let range = Range(lower..<upper, in: result) {
                result[range][Attr.Foreground.self] = Color.difAccent
                result[range][Attr.Font.self] = .ui(size: size, weight: .bold)
            }
        }
        return result
    }
}

// MARK: - Find in Files

/// Hosts the read-only preview canvas inside the Find in Files popup.
private struct PreviewHost: NSViewRepresentable {
    var attach: (CodeScrollContainer) -> Void

    func makeNSView(context: Context) -> CodeScrollContainer {
        let container = CodeScrollContainer(frame: .zero)
        attach(container)
        return container
    }

    func updateNSView(_ view: CodeScrollContainer, context: Context) {}
}

struct FindInPathView: View {
    @Environment(AppStore.self) private var store

    var body: some View {
        @Bindable var model = store.finder
        VStack(spacing: 0) {
            HStack(spacing: 10) {
                Image(systemName: "text.magnifyingglass").font(.ui(size: 15, weight: .medium)).foregroundStyle(Color.difText2)
                SearchField(text: $model.query, placeholder: L("Find in files", "在文件中查找"), font: .systemFont(ofSize: 16 * UI.scale),
                            focusToken: model.focusToken,
                            onMove: { model.move($0) },
                            onSubmit: { model.activate() },
                            onCancel: { store.dismissPopup() })
                    .frame(height: UI.s(24))
                OptionToggle(label: "Aa", help: L("Match case", "区分大小写"), isOn: model.options.caseSensitive) {
                    model.options.caseSensitive.toggle()
                    model.search()
                }
                OptionToggle(label: "W", help: L("Whole words", "全词匹配"), isOn: model.options.wholeWord) {
                    model.options.wholeWord.toggle()
                    model.search()
                }
                OptionToggle(label: ".*", help: L("Regular expression", "正则表达式"), isOn: model.options.regex) {
                    model.options.regex.toggle()
                    model.search()
                }
            }
            .padding(.horizontal, 16)
            .frame(height: UI.s(52))

            HStack(spacing: 10) {
                HStack(spacing: 5) {
                    Image(systemName: "doc").font(.ui(size: 10.5)).foregroundStyle(Color.difText3)
                    TextField(L("File mask, e.g. *.kt", "文件过滤，例如 *.kt"), text: $model.options.fileMask)
                        .textFieldStyle(.plain)
                        .font(.ui(size: 12))
                        .frame(width: UI.s(170))
                        .onSubmit { model.search() }
                }
                .padding(.horizontal, 8)
                .frame(height: UI.s(24))
                .background(RoundedRectangle(cornerRadius: 6, style: .continuous).fill(Color.difChip))
                if store.projects.count > 1 {
                    HStack(spacing: 2) {
                        ScopeTab(title: L("This Project", "当前项目"), isSelected: !model.allProjects) {
                            model.allProjects = false
                            model.search()
                        }
                        ScopeTab(title: L("All Projects", "所有项目"), isSelected: model.allProjects) {
                            model.allProjects = true
                            model.search()
                        }
                    }
                }
                Spacer()
                if model.isSearching { ProgressView().controlSize(.small).scaleEffect(0.65) }
                Text(summary(model))
                    .font(.ui(size: 11.5))
                    .foregroundStyle(model.errorText == nil ? Color.difText3 : Color.difRemoved)
            }
            .padding(.horizontal, 16)
            .padding(.bottom, 10)

            Rectangle().fill(Color.difHairline).frame(height: 1)

            ScrollViewReader { proxy in
                ScrollView {
                    LazyVStack(spacing: 0) {
                        ForEach(model.hits) { hit in
                            FindHitRow(hit: hit, isSelected: hit.id == model.selected,
                                       projectName: model.allProjects ? store.project(hit.projectID)?.name : nil)
                                .id(hit.id)
                                .contentShape(Rectangle())
                                .onTapGesture {
                                    model.select(hit.id)
                                    if (NSApp.currentEvent?.clickCount ?? 1) >= 2 { model.activate() }
                                }
                        }
                    }
                    .padding(.vertical, 4)
                }
                .frame(height: 236)
                .onChange(of: model.selected) { _, index in proxy.scrollTo(index) }
            }

            Rectangle().fill(Color.difHairline).frame(height: 1)
            HStack(spacing: 6) {
                if !model.previewPath.isEmpty {
                    FileIcon(path: model.previewPath, size: 11)
                    Text(model.previewPath).font(.ui(size: 11.5)).foregroundStyle(Color.difText2).lineLimit(1).truncationMode(.head)
                }
                Spacer()
            }
            .padding(.horizontal, 12)
            .frame(height: UI.s(26))
            .background(Color.difPanel.opacity(0.6))

            ZStack {
                PreviewHost { model.attachPreview($0) }
                if model.previewDocument == nil {
                    Color.difWindow
                    Text(model.query.count < 2 ? L("Type at least two characters", "至少输入两个字符") : (model.isSearching ? "" : L("No matches", "没有匹配项")))
                        .font(.ui(size: 12.5))
                        .foregroundStyle(Color.difText3)
                }
            }

            Rectangle().fill(Color.difHairline).frame(height: 1)
            HStack(spacing: 14) {
                Hint(keys: "↑↓", text: L("Navigate", "选择"))
                Hint(keys: "↩", text: L("Open in viewer", "在查看器中打开"))
                Hint(keys: "Esc", text: L("Close", "关闭"))
                Spacer()
            }
            .padding(.horizontal, 14)
            .frame(height: UI.s(30))
        }
        .onChange(of: model.query) { _, _ in model.queryChanged() }
    }

    private func summary(_ model: FindInPathModel) -> String {
        if let error = model.errorText { return error }
        guard !model.hits.isEmpty else { return "" }
        let count = model.truncated ? "\(model.hits.count)+" : "\(model.hits.count)"
        return L("\(count) matches in \(model.fileCount) files", "\(model.fileCount) 个文件中有 \(count) 处匹配")
    }
}

private struct OptionToggle: View {
    var label: String
    var help: String
    var isOn: Bool
    var action: () -> Void

    var body: some View {
        Button(action: action) {
            Text(label)
                .font(.ui(size: 12, weight: .semibold, design: .monospaced))
                .foregroundStyle(isOn ? Color.difAccent : Color.difText2)
                .frame(width: UI.s(28), height: UI.s(24))
                .background(RoundedRectangle(cornerRadius: 6, style: .continuous).fill(isOn ? Color.difSelection : Color.difChip))
        }
        .buttonStyle(.plain)
        .help(help)
    }
}

private struct FindHitRow: View {
    var hit: FindInPathModel.Hit
    var isSelected: Bool
    var projectName: String?

    var body: some View {
        HStack(spacing: 10) {
            Text(snippet)
                .font(.code(12))
                .lineLimit(1)
                .truncationMode(.tail)
            Spacer(minLength: 8)
            if let projectName {
                Text(projectName).font(.ui(size: 10.5)).foregroundStyle(Color.difText3).lineLimit(1)
            }
            Text((hit.path as NSString).lastPathComponent)
                .font(.ui(size: 11.5))
                .foregroundStyle(Color.difText2)
                .lineLimit(1)
                .truncationMode(.middle)
                .frame(maxWidth: 240, alignment: .trailing)
            Text("\(hit.line)")
                .font(.ui(size: 11, design: .monospaced))
                .foregroundStyle(Color.difText3)
                .frame(minWidth: 34, alignment: .trailing)
        }
        .padding(.horizontal, 14)
        .frame(height: UI.s(24))
        .background(
            RoundedRectangle(cornerRadius: 6, style: .continuous)
                .fill(isSelected ? Color.difSelection : Color.clear)
                .padding(.horizontal, 6)
        )
        .help(hit.path)
    }

    private var snippet: AttributedString {
        let ns = hit.text as NSString
        guard let match = hit.match, match.upperBound <= ns.length else {
            var plain = AttributedString(String(hit.text.prefix(240)))
            plain[Attr.Foreground.self] = Color.difText
            return plain
        }
        var before = AttributedString(ns.substring(to: match.lowerBound))
        before[Attr.Foreground.self] = Color.difText2
        var middle = AttributedString(ns.substring(with: NSRange(location: match.lowerBound, length: match.count)))
        middle[Attr.Foreground.self] = Color.difText
        middle[Attr.Background.self] = Color(NSColor(hex: 0xE8B339, alpha: 0.38))
        let tailEnd = min(ns.length, match.upperBound + 200)
        var after = AttributedString(ns.substring(with: NSRange(location: match.upperBound, length: tailEnd - match.upperBound)))
        after[Attr.Foreground.self] = Color.difText2
        return before + middle + after
    }
}

// MARK: - Go to line

struct GoToLineView: View {
    @Environment(AppStore.self) private var store
    @State private var text = ""

    var body: some View {
        let total = store.viewer.document?.right.lines.count ?? 0
        HStack(spacing: 10) {
            Image(systemName: "arrow.right.to.line").font(.ui(size: 14, weight: .medium)).foregroundStyle(Color.difText2)
            SearchField(text: $text, placeholder: L("Line number (1–\(total))", "行号 (1–\(total))"), font: .systemFont(ofSize: 15 * UI.scale),
                        focusToken: 1,
                        onSubmit: {
                            let digits = text.split(separator: ":").first.map(String.init) ?? text
                            store.dismissPopup(restoreFocus: false)
                            if let line = Int(digits.trimmingCharacters(in: .whitespaces)) { store.viewer.reveal(line: line) }
                            store.viewer.focusCanvas()
                        },
                        onCancel: { store.dismissPopup() })
                .frame(height: UI.s(22))
        }
        .padding(.horizontal, 16)
        .frame(height: UI.s(48))
    }
}

// MARK: - Keymap

struct KeymapView: View {
    @Environment(AppStore.self) private var store

    var body: some View {
        let commands = store.commands
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                Text(L("Keymap", "快捷键")).font(.ui(size: 15, weight: .semibold)).foregroundStyle(Color.difText)
                Text(L("Aligned with Android Studio's macOS keymap", "与 Android Studio 的 macOS 键位保持一致"))
                    .font(.ui(size: 11.5))
                    .foregroundStyle(Color.difText3)
                Spacer()
                KeyCaps(shortcut: "Esc")
            }
            .padding(.horizontal, 20)
            .frame(height: UI.s(48))
            Rectangle().fill(Color.difHairline).frame(height: 1)
            ScrollView {
                HStack(alignment: .top, spacing: 28) {
                    column(commands, groups: AppEnvironment.hasTerminal ? [.navigate, .terminal] : [.navigate])
                    VStack(alignment: .leading, spacing: 18) {
                        column(commands, groups: [.view])
                        column(commands, groups: [.git, .file])
                        extras
                    }
                }
                .padding(20)
            }
            .frame(maxHeight: 620)
        }
    }

    private func column(_ commands: [AppCommand], groups: [AppCommand.Group]) -> some View {
        VStack(alignment: .leading, spacing: 18) {
            ForEach(groups, id: \.self) { group in
                VStack(alignment: .leading, spacing: 7) {
                    SectionLabel(text: group.title)
                    ForEach(commands.filter { $0.group == group && !$0.shortcut.isEmpty }) { command in
                        shortcutRow(command.title.replacingOccurrences(of: "…", with: ""), command.shortcut)
                    }
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var extras: some View {
        VStack(alignment: .leading, spacing: 7) {
            SectionLabel(text: L("In the viewer", "查看器内"))
            shortcutRow(L("Next / Previous Change", "下一处 / 上一处变更"), "⌃⌥⇧↓ / ⌃⌥⇧↑")
            shortcutRow(L("Find in File", "在文件中查找"), "⌘F")
            shortcutRow(L("Next / Previous Occurrence", "下一个 / 上一个匹配"), "⌘G / ⇧⌘G")
            shortcutRow(L("Top / Bottom of File", "文件开头 / 末尾"), "⌘↑ / ⌘↓")
            shortcutRow(L("Back to the File List", "回到文件列表"), "Esc")
            shortcutRow(L("Zoom Text", "缩放文字"), L("Pinch", "双指捏合"))
            if AppEnvironment.hasTerminal {
                shortcutRow(L("Close Terminal (when focused)", "关闭终端（聚焦时）"), "⌘W")
                shortcutRow(L("Next / Previous Terminal (when focused)", "下一个 / 上一个终端（聚焦时）"), "⇧⌘] / ⇧⌘[")
            }
        }
    }

    private func shortcutRow(_ title: String, _ shortcut: String) -> some View {
        HStack {
            Text(title).font(.ui(size: 12.5)).foregroundStyle(Color.difText2).lineLimit(1)
            Spacer(minLength: 12)
            KeyCaps(shortcut: shortcut)
        }
    }
}

// MARK: - Settings

struct SettingsView: View {
    @Environment(AppStore.self) private var store
    /// Installed monospace families for the font pickers, read when Settings opens.
    @State private var families: [String] = []

    var body: some View {
        @Bindable var settings = store.settings
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                Text(L("Settings", "设置")).font(.ui(size: 15, weight: .semibold)).foregroundStyle(Color.difText)
                Spacer()
                KeyCaps(shortcut: "Esc")
            }
            .padding(.horizontal, 20)
            .frame(height: UI.s(48))
            Rectangle().fill(Color.difHairline).frame(height: 1)

            VStack(alignment: .leading, spacing: 16) {
                row(L("Appearance", "外观")) {
                    Picker("", selection: $settings.appearance) {
                        ForEach(Settings.Appearance.allCases, id: \.self) { Text($0.title).tag($0) }
                    }
                    .pickerStyle(.segmented)
                    .labelsHidden()
                    .frame(width: UI.s(240))
                }
                VStack(alignment: .trailing, spacing: 8) {
                    row(L("Language", "语言")) {
                        Picker("", selection: Binding(get: { settings.language }, set: { settings.chooseLanguage($0) })) {
                            ForEach(Settings.Language.allCases, id: \.self) { Text($0.title).tag($0) }
                        }
                        .pickerStyle(.segmented)
                        .labelsHidden()
                        .frame(width: UI.s(240))
                    }
                    if settings.languageNeedsRestart {
                        HStack(spacing: 8) {
                            Text(L("Shows after \(appName) restarts", "重启 \(appName) 后生效"))
                                .font(.ui(size: 11.5))
                                .foregroundStyle(Color.difText3)
                            BannerButton(title: L("Restart Now", "立即重启"), prominent: true) { store.restart() }
                        }
                    }
                }
                row(L("Code font", "代码字体")) {
                    fontPicker(settings.codeFontFamily) { store.chooseCodeFont($0) }
                }
                row(L("Code font size", "代码字号")) {
                    stepper(settings.fontSize, down: "⌃⇧,", up: "⌃⇧.") { store.viewer.changeFontSize(by: $0) }
                }
                row(L("Interface font size", "界面字号")) {
                    stepper(settings.uiFontSize, down: L("Smaller", "减小"), up: L("Larger", "增大")) { store.changeInterfaceFontSize(by: $0) }
                }
                if AppEnvironment.hasTerminal {
                    row(L("Terminal font", "终端字体")) {
                        fontPicker(settings.terminalFontFamily) { store.chooseTerminalFont($0) }
                    }
                    row(L("Terminal font size", "终端字号")) {
                        stepper(settings.terminalFontSize, down: L("Smaller", "减小"), up: L("Larger", "增大")) { store.changeTerminalFontSize(by: $0) }
                    }
                }
                row(L("Default view", "默认视图")) {
                    Picker("", selection: $settings.unified) {
                        Text(L("Side-by-side", "并排")).tag(false)
                        Text(L("Unified", "合并")).tag(true)
                    }
                    .pickerStyle(.segmented)
                    .labelsHidden()
                    .frame(width: UI.s(240))
                }
            }
            .padding(20)
        }
        .onAppear { settings.reloadLanguage() }
        .task {
            // Fonts installed since the last time show too.
            families = await Task.detached(priority: .userInitiated) { MonospaceFonts.installed() }.value
        }
        .onChange(of: settings.appearance) { _, _ in
            settings.save()
            settings.applyAppearance()
        }
        .onChange(of: settings.unified) { _, _ in
            settings.save()
            store.viewer.container?.setMode(store.viewer.canvasMode)
        }
    }

    private func row<Content: View>(_ title: String, @ViewBuilder content: () -> Content) -> some View {
        HStack {
            Text(title).font(.ui(size: 13)).foregroundStyle(Color.difText2)
            Spacer()
            content()
        }
    }

    /// "Default" and the installed monospace families. A picked family that has since been uninstalled stays in the
    /// list, marked, so the picker never shows a blank; the code falls back to the default face meanwhile.
    private func fontPicker(_ selection: String?, choose: @escaping (String?) -> Void) -> some View {
        let missing = selection.flatMap { families.isEmpty || families.contains($0) ? nil : $0 }
        let choices = families.isEmpty ? selection.map { [$0] } ?? [] : families + (missing.map { [$0] } ?? [])
        return Picker("", selection: Binding(get: { selection }, set: { choose($0) })) {
            Text(L("Default (\(MonospaceFonts.defaultName))", "默认（\(MonospaceFonts.defaultName)）")).tag(String?.none)
            Divider()
            ForEach(choices, id: \.self) { family in
                Text(family == missing ? family + L(" (not installed)", "（未安装）") : family).tag(String?.some(family))
            }
        }
        .pickerStyle(.menu)
        .labelsHidden()
        .frame(width: UI.s(240))
    }

    private func stepper(_ value: Double, down: String, up: String, change: @escaping (Double) -> Void) -> some View {
        HStack(spacing: 8) {
            IconButton(symbol: "minus", help: down) { change(-1) }
            Text("\(Int(value)) pt").font(.ui(size: 12.5, design: .rounded)).frame(width: UI.s(44))
            IconButton(symbol: "plus", help: up) { change(1) }
        }
    }
}
