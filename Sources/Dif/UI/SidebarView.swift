import SwiftUI

/// Left column: the projects, and under them the file tree of the one that is selected.
struct SidebarView: View {
    @Environment(AppStore.self) private var store

    var body: some View {
        GeometryReader { proxy in
            VStack(spacing: 0) {
                // Clear space for the traffic lights; still draggable.
                WindowDragArea().frame(height: store.titlebarHeight)

                HStack {
                    SectionLabel(text: L("Projects", "项目"))
                    Spacer()
                    IconButton(symbol: "plus", help: L("Add Project (⌘N)", "添加项目 (⌘N)"), size: 12) { store.presentAddProject() }
                }
                .padding(.leading, 16)
                .padding(.trailing, 8)
                .padding(.bottom, 4)

                if let project = store.currentProject, !project.needsAccess, !store.gitMissing {
                    // The projects take the room they need, up to about a third; the tree gets everything else.
                    projects.frame(height: Self.projectsHeight(count: store.projectRows.count, available: proxy.size.height))
                    Rectangle().fill(Color.difHairline).frame(height: 1)
                    FileTreePanel(project: project)
                        .id(project.id)
                } else {
                    projects
                }

                Divider().opacity(0.5)
                SidebarFooter()
            }
        }
        // Opaque, like the other panels. The system's translucent sidebar material let a bright wallpaper or
        // window behind show through, and the text, coloured for a dark panel, faded into it.
        .background(Color.difSidebar)
    }

    private var projects: some View {
        ScrollView {
            LazyVStack(spacing: Self.rowSpacing) {
                let rows = store.projectRows
                // Room for the arrows only when something can be folded, so a plain list looks as it always did.
                let nested = rows.contains { $0.hasChildren || $0.depth > 0 }
                ForEach(rows, id: \.index) { row in
                    let project = store.projects[row.index]
                    ProjectRow(project: project, isSelected: project.id == store.selectedProjectID,
                               depth: row.depth, hasChildren: row.hasChildren, nested: nested,
                               parent: row.parent.map { store.projects[$0] })
                        .frame(height: Self.rowHeight)
                        .id(project.id)
                }
            }
            .padding(.horizontal, 8)
            .padding(.bottom, Self.bottomPadding)
        }
    }

    static var rowHeight: CGFloat { UI.s(40) }
    private static let rowSpacing: CGFloat = 2
    private static let bottomPadding: CGFloat = 6

    /// Every project when they fit; otherwise a cut that leaves half a row showing, so the list reads as scrollable.
    static func projectsHeight(count: Int, available: CGFloat) -> CGFloat {
        let step = rowHeight + rowSpacing
        let all = CGFloat(max(1, count)) * step - rowSpacing + bottomPadding
        let limit = max(step * 2.5, min(step * 6.5, available * 0.34))
        return all <= limit ? all : ((limit / step - 0.5).rounded(.down) + 0.5) * step
    }
}

/// The branch under a project's name. A click opens that project's branches and commit graph just below it.
private struct BranchLabel: View {
    let project: Project
    @Environment(AppStore.self) private var store
    @State private var hovering = false
    @State private var frame: CGRect = .zero

    var body: some View {
        let open = store.popup == .gitGraph && store.gitGraph.projectID == project.id
        let lit = hovering || open
        Button {
            store.toggleGitGraph(for: project, anchor: CGPoint(x: frame.minX, y: frame.maxY + 4))
        } label: {
            HStack(spacing: 3) {
                Image(systemName: "arrow.triangle.branch").font(.ui(size: 9, weight: .semibold))
                Text(project.branchLabel).font(.ui(size: 11)).lineLimit(1).truncationMode(.middle)
                if let activity = project.activity {
                    // A checkout, fetch, pull or push at work.
                    ProgressView().controlSize(.mini).scaleEffect(0.7).frame(width: UI.s(10), height: UI.s(10))
                        .help(activity)
                } else {
                    // Commits not yet pushed, and not yet pulled.
                    if project.status.ahead > 0 { Text("↑\(project.status.ahead)").font(.ui(size: 10.5, weight: .medium)).fixedSize() }
                    if project.status.behind > 0 { Text("↓\(project.status.behind)").font(.ui(size: 10.5, weight: .medium)).fixedSize() }
                }
                if let operation = project.status.operation {
                    Text(operation.title).font(.ui(size: 10.5, weight: .medium)).foregroundStyle(Color.orange).lineLimit(1).fixedSize()
                }
                // Always laid out, so nothing moves when it appears.
                Image(systemName: "chevron.down").font(.ui(size: 7, weight: .bold)).opacity(lit ? 1 : 0)
            }
            .foregroundStyle(lit ? Color.difText : Color.difText2)
            .padding(.horizontal, 4)
            .padding(.vertical, 1.5)
            .background(RoundedRectangle(cornerRadius: 4, style: .continuous).fill(lit ? Color.difText.opacity(0.1) : Color.clear))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        // The text stays in line with the project name; only the highlight reaches past it.
        .padding(.horizontal, -4)
        .onHover { hovering = $0 }
        .help(help)
        .background(GeometryReader { proxy in
            Color.clear
                .onAppear { remember(proxy.frame(in: .global)) }
                .onChange(of: proxy.frame(in: .global)) { _, value in remember(value) }
        })
    }

    private var help: String {
        let tracking = project.status.upstream.map { L("Tracking \($0)", "跟踪 \($0)") }
        let action = L("Branches and commit graph (⌥⌘B)", "分支与提交树 (⌥⌘B)")
        return [tracking, action].compactMap { $0 }.joined(separator: "\n")
    }

    private func remember(_ value: CGRect) {
        frame = value
        store.branchLabelFrames[project.id] = value
    }
}

private struct ProjectRow: View {
    let project: Project
    var isSelected: Bool
    var depth = 0
    var hasChildren = false
    /// Whether any project is inside another: then every row leaves room for the arrow.
    var nested = false
    var parent: Project?
    @Environment(AppStore.self) private var store
    @State private var hovering = false

    /// Under its parent a repository goes by its path from there ("services/core").
    private var title: String {
        guard let parent, project.url.path.hasPrefix(parent.url.path + "/") else { return project.name }
        return String(project.url.path.dropFirst(parent.url.path.count + 1))
    }

    var body: some View {
        let collapsed = store.collapsedProjects.contains(project.url.path)
        let changes = project.changes.count + store.hiddenChanges(under: project)
        HStack(spacing: 9) {
            if nested {
                Group {
                    if hasChildren {
                        Button { store.toggleCollapsed(project) } label: {
                            Image(systemName: "chevron.right")
                                .font(.ui(size: 9, weight: .semibold))
                                .rotationEffect(.degrees(collapsed ? 0 : 90))
                                .foregroundStyle(Color.difText3)
                                .frame(maxWidth: .infinity, maxHeight: .infinity)
                                .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        .help(collapsed ? L("Show the repositories inside", "展开子仓库") : L("Hide the repositories inside", "折叠子仓库"))
                    } else {
                        Color.clear
                    }
                }
                .frame(width: UI.s(12))
                .padding(.leading, CGFloat(depth) * UI.s(14) - 3)
                .padding(.trailing, -5)
            }
            ProjectAvatar(name: project.name)
            VStack(alignment: .leading, spacing: 1) {
                Text(title)
                    .font(.ui(size: 13, weight: .medium))
                    .foregroundStyle(Color.difText)
                    .lineLimit(1)
                BranchLabel(project: project)
            }
            Spacer(minLength: 4)
            if store.terminals.hasSessions(project) {
                // A terminal is open here; orange once it rings for attention (an agent waiting on you).
                let waiting = store.terminals.needsAttention(project)
                Image(systemName: waiting ? "terminal.fill" : "terminal")
                    .font(.ui(size: 10, weight: .medium))
                    .foregroundStyle(waiting ? Color.orange : Color.difText3)
                    .help(waiting ? L("A terminal here wants attention", "这里有终端在等你") : L("Has open terminals", "有打开的终端"))
            }
            if project.errorMessage != nil {
                Image(systemName: "exclamationmark.triangle.fill")
                    .font(.ui(size: 11))
                    .foregroundStyle(.orange)
                    .help(project.errorMessage ?? "")
            } else if changes > 0 {
                Text("\(changes)")
                    .font(.ui(size: 10.5, weight: .semibold, design: .rounded))
                    .foregroundStyle(isSelected ? Color.white : Color.difText2)
                    .padding(.horizontal, 6)
                    .padding(.vertical, 1.5)
                    .background(Capsule().fill(isSelected ? Color.difAccent : Color.difChip))
            }
        }
        .padding(.horizontal, 8)
        .frame(maxHeight: .infinity)
        .background(
            RoundedRectangle(cornerRadius: 8, style: .continuous)
                .fill(isSelected ? Color.difSelection : (hovering ? Color.difHover : Color.clear))
        )
        .contentShape(Rectangle())
        .onTapGesture { store.select(project) }
        .onHover { hovering = $0 }
        .help(collapsed && changes > project.changes.count
              ? project.url.path + "\n" + L("\(changes - project.changes.count) changes in the repositories inside",
                                             "子仓库里有 \(changes - project.changes.count) 处改动")
              : project.url.path)
        .contextMenu {
            Button(L("Reveal in Finder", "在 Finder 中显示")) { NSWorkspace.shared.activateFileViewerSelecting([project.url]) }
            Button(L("Copy Path", "复制路径")) {
                NSPasteboard.general.clearContents()
                NSPasteboard.general.setString(project.url.path, forType: .string)
            }
            Button(L("Refresh", "刷新")) { store.refreshAndDiscover(project) }
            Divider()
            if hasChildren {
                Button(collapsed ? L("Expand", "展开") : L("Collapse", "折叠")) { store.toggleCollapsed(project) }
            }
            Button(L("Move Up", "上移")) { store.move(project, by: -1) }.disabled(!store.canMove(project, by: -1))
            Button(L("Move Down", "下移")) { store.move(project, by: 1) }.disabled(!store.canMove(project, by: 1))
            Divider()
            Button(L("Remove from \(appName)", "从 \(appName) 中移除"), role: .destructive) { store.remove(project) }
        }
    }
}

/// A coloured monogram so projects are recognisable at a glance.
struct ProjectAvatar: View {
    var name: String
    var size: CGFloat = 24

    private static let gradients: [(UInt32, UInt32)] = [
        (0x5B8DEF, 0x3A5FD9), (0x34C8A0, 0x1E9E84), (0xF2994A, 0xE5683B), (0xB57BEE, 0x8B55D6),
        (0xEF6F8E, 0xD9456E), (0x4FC3E8, 0x2D95D1), (0xE6B93D, 0xD9952A), (0x7ED06F, 0x4DAE4E),
    ]

    var body: some View {
        // A stable hash: `hashValue` is reseeded on every launch, which would reshuffle the colours.
        let seed = name.utf8.reduce(5381) { ($0 &* 33) &+ Int($1) }
        let pair = Self.gradients[abs(seed) % Self.gradients.count]
        RoundedRectangle(cornerRadius: size * 0.28, style: .continuous)
            .fill(LinearGradient(colors: [Color(NSColor(hex: pair.0)), Color(NSColor(hex: pair.1))],
                                 startPoint: .topLeading, endPoint: .bottomTrailing))
            .frame(width: UI.s(size), height: UI.s(size))
            .overlay(
                Text(String(name.first ?? "?").uppercased())
                    .font(.ui(size: size * 0.5, weight: .bold, design: .rounded))
                    .foregroundStyle(.white)
            )
    }
}

private struct SidebarFooter: View {
    @Environment(AppStore.self) private var store

    var body: some View {
        HStack(spacing: 2) {
            // The whole point of Dif is being light; keep the number honest and visible.
            HStack(spacing: 4) {
                Image(systemName: "bolt.fill").font(.ui(size: 9))
                Text(String(format: "%.0f MB", store.memoryMB)).font(.ui(size: 11, weight: .medium, design: .rounded))
            }
            .foregroundStyle(Color.difText3)
            .help(L("Memory used by \(appName) right now", "\(appName) 当前占用的内存"))
            Spacer()
            IconButton(symbol: "keyboard", help: L("Keymap Reference (⌘/)", "快捷键一览 (⌘/)"), size: 12) { store.present(.keymap) }
            IconButton(symbol: "gearshape", help: L("Settings (⌘,)", "设置 (⌘,)"), size: 12) { store.present(.settings) }
        }
        .padding(.leading, 14)
        .padding(.trailing, 8)
        .frame(height: UI.s(34))
    }
}

/// Shown when there are no projects yet.
struct WelcomeView: View {
    @Environment(AppStore.self) private var store

    var body: some View {
        VStack(spacing: 18) {
            // The app's own icon; its tile fills about 80% of the image (Apple's grid), so it shows at about 54 pt.
            Image(nsImage: NSApp.applicationIconImage)
                .resizable()
                .interpolation(.high)
                .frame(width: UI.s(68), height: UI.s(68))
            VStack(spacing: 6) {
                Text(L("Welcome to \(appName)", "欢迎使用 \(appName)"))
                    .font(.ui(size: 22, weight: .semibold))
                    .foregroundStyle(Color.difText)
                Text(L("A featherweight viewer for Git changes.", "一个极轻量的 Git 变更查看器。"))
                    .font(.ui(size: 13))
                    .foregroundStyle(Color.difText2)
            }
            Button {
                store.presentAddProject()
            } label: {
                HStack(spacing: 8) {
                    Image(systemName: "plus")
                    Text(L("Add a Git Repository", "添加 Git 仓库"))
                    KeyCaps(shortcut: "⌘N")
                }
                .font(.ui(size: 13, weight: .medium))
                .padding(.horizontal, 16)
                .padding(.vertical, 9)
                .background(RoundedRectangle(cornerRadius: 9, style: .continuous).fill(Color.difSelection))
                .foregroundStyle(Color.difText)
            }
            .buttonStyle(.plain)
            Text(L("…or drop folders anywhere in this window", "…或者把文件夹拖进这个窗口"))
                .font(.ui(size: 12))
                .foregroundStyle(Color.difText3)
            if FolderAccess.shared.isNeeded {
                // The sandbox asks once per folder chosen; the folder of all the repositories asks once for all of them.
                Text(L("Tip: choose the folder that holds all your repositories. \(appName) finds every one in it, and you grant access only once.",
                       "提示：直接选存放所有仓库的文件夹，\(appName) 会找出其中的每个仓库，只需授权一次。"))
                    .font(.ui(size: 12))
                    .foregroundStyle(Color.difText3)
                    .multilineTextAlignment(.center)
                    .frame(maxWidth: 380)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(WindowDragArea())
    }
}
