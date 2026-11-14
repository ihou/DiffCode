import AppKit

/// What the Mac App Store edition does about the sandbox: asking for folders, and for the home folder that holds
/// the user's Git settings. In the direct download none of it comes up.
extension AppStore {
    /// Asks for a repository's folder; true once it is reachable.
    func requestAccess(to folder: URL) async -> Bool {
        await withCheckedContinuation { continuation in
            FolderAccess.shared.requestAccess(
                to: folder,
                message: L("\(appName) needs access to “\(folder.lastPathComponent)” to show its changes.",
                           "\(appName) 需要访问「\(folder.lastPathComponent)」才能显示其中的改动。"),
                window: window
            ) { granted in
                continuation.resume(returning: granted)
            }
        }
    }

    /// A project whose folder the sandbox no longer opens (the bookmark was lost, or access was reset): ask again.
    func grantAccess(to project: Project) {
        Task { @MainActor in
            if await self.requestAccess(to: project.url) {
                self.unlockGrantedProjects()
            } else {
                self.showToast(L("Still no access to \(project.name)", "仍然无法访问 \(project.name)"))
            }
        }
    }

    /// File ▸ Grant Folder Access…: one grant of a folder above the repositories covers all of them. The home folder,
    /// where the panel starts, also lets git read the user's settings (who commits, global ignores).
    func presentGrantFolderAccess() {
        FolderAccess.shared.requestAccess(
            to: URL(fileURLWithPath: Sandbox.realHome),
            message: L("Grant your home folder so \(appName) can read your Git settings (~/.gitconfig) and every repository in it. A narrower folder works too.",
                       "授权你的个人文件夹后，\(appName) 能读取 Git 设置（~/.gitconfig）和其中的所有仓库。也可以只选更小的文件夹。"),
            window: window
        ) { [weak self] _ in
            guard let self else { return }
            self.unlockGrantedProjects()
            // Settings git could not read before (who commits, global ignores) count from now on.
            self.projects.forEach { $0.refresh() }
        }
    }

    /// Projects a new grant made reachable start working.
    func unlockGrantedProjects() {
        var unlocked = false
        for project in projects where project.needsAccess && FolderAccess.shared.covers(project.url.path) {
            project.needsAccess = false
            project.errorMessage = nil
            project.startWatching()
            project.refresh()
            unlocked = true
        }
        if unlocked { syncViewer() }
    }

    /// What to add to a failed commit's message when the sandbox is why git does not know who is committing.
    func sandboxNote(for error: Error) -> String? {
        guard Sandbox.isActive, !Sandbox.canReadGlobalGitConfig else { return nil }
        let message = error.localizedDescription
        guard message.contains("Please tell me who you are") || message.contains("Author identity unknown")
                || message.contains("user.email") else { return nil }
        return L("The App Store version can read your Git settings (~/.gitconfig) only once you grant your home folder: File ▸ Grant Folder Access…",
                 "App Store 版需要你授权个人文件夹后才能读取 Git 设置（~/.gitconfig）：文件 ▸ 授权访问文件夹…")
    }
}
