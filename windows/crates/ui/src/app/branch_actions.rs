//! Switching and creating branches, talking to remotes (fetch, pull, push), and finishing or abandoning a merge or
//! rebase that stopped for conflicts. Each step is git's own command, run as the user would in a terminal; nothing is
//! forced, and a refusal shows git's words.

use std::collections::HashSet;
use std::path::PathBuf;

use super::dialog::{DialogItem, DialogRequest, DialogStyle};
use super::project::{ProjectId, ProjectTab};
use super::{App, Popup};
use crate::core::git::{self, CancelToken, ChangeKind, Commit, FileChange};
use crate::core::history::{self, GitRef, RefKind};
use crate::core::history_edits::{self, PROTECTED_BRANCHES};
use crate::core::lang::l;
use crate::core::lf;
use crate::core::remote::{self, ConflictSide, PullMode};
use crate::core::util::file_name;
use crate::icons::Icon;

/// A checkout to try: run on a worker thread, again after Smart Checkout has put the local changes aside.
type Attempt = std::sync::Arc<dyn Fn(&std::path::Path) -> Result<(), String> + Send + Sync>;

fn is_cancelled(e: &git::Error) -> bool {
    matches!(e, git::Error::Cancelled)
}

fn commit_items(commits: &[Commit], limit: usize, icon: Icon) -> (Vec<DialogItem>, usize) {
    let items = commits
        .iter()
        .take(limit)
        .map(|c| DialogItem { title: c.subject.clone(), detail: c.short_hash(), kind: None, icon: Some(icon) })
        .collect();
    (items, commits.len().saturating_sub(limit))
}

/// "4\t2" from `rev-list --left-right --count` → the right number.
fn behind_count(out: &str) -> usize {
    out.split_whitespace().last().and_then(|n| n.parse().ok()).unwrap_or(0)
}

impl App {
    // MARK: Running

    /// Runs `work` as the project's one branch or remote operation, named by its branch (`title`) until it ends.
    /// `cancellable`: a network operation, which the Git graph offers to stop; `work` gets the token to honour.
    pub fn run_activity<T: Send + 'static>(
        &mut self,
        title: String,
        cancellable: bool,
        id: ProjectId,
        work: impl FnOnce(&std::path::Path, &CancelToken) -> T + Send + 'static,
        done: impl FnOnce(&mut App, T) + 'static,
    ) {
        let Some(p) = self.project_mut(id) else { return };
        if let Some(running) = &p.activity {
            let running = running.clone();
            self.show_toast(lf!("Wait for “{running}” to finish", "请等“{running}”完成"));
            return;
        }
        let token = CancelToken::new();
        p.activity = Some(title);
        p.activity_cancel = cancellable.then(|| token.clone());
        let url = p.url.clone();
        self.jobs.spawn(
            move || work(&url, &token),
            move |app: &mut App, result: T| {
                if let Some(p) = app.project_mut(id) {
                    p.activity = None;
                    p.activity_cancel = None;
                }
                done(app, result);
            },
        );
    }

    pub fn cancel_activity(&mut self, id: ProjectId) {
        if let Some(token) = self.project(id).and_then(|p| p.activity_cancel.clone()) {
            token.cancel();
        }
    }

    /// HEAD or the branches moved: the lists, the tree and an open Git graph follow.
    pub fn after_branches_changed(&mut self, id: ProjectId) {
        let Some(p) = self.project_mut(id) else { return };
        p.invalidate_files();
        p.log_stale = true;
        p.refresh();
        if p.tab == ProjectTab::Log {
            p.load_log(true, 0, None);
        }
        if self.popup == Some(Popup::GitGraph) && self.git_graph.project == Some(id) {
            self.reload_git_graph();
        }
    }

    /// What to try when git could not reach or sign in to a remote.
    pub fn remote_note(message: &str) -> Option<String> {
        if message.contains("terminal prompts disabled")
            || message.contains("could not read Username")
            || message.contains("could not read Password")
            || message.contains("Authentication failed")
        {
            return Some(l(
                "Git could not ask for a password here. Run the same command once in a terminal, so that Git Credential Manager remembers it.",
                "Git 在这里无法询问密码。请在终端里运行一次同样的命令，让 Git 凭据管理器记住它。",
            ));
        }
        if message.contains("Permission denied (publickey") || message.contains("Host key verification failed") || message.contains("passphrase") {
            return Some(l(
                "SSH could not sign in on its own. Start the OpenSSH Authentication Agent service and add your key (ssh-add), or connect once in a terminal to accept the server's key.",
                "SSH 无法自动登录。请启动 OpenSSH Authentication Agent 服务并加入密钥（ssh-add），或在终端里连接一次以接受服务器指纹。",
            ));
        }
        None
    }

    // MARK: Checkout

    /// Switches to a branch. A remote branch gets a local branch of the same name that follows it, as in the IDE; a
    /// tag is checked out without a branch.
    pub fn checkout(&mut self, r: GitRef, id: ProjectId) {
        let Some(url) = self.project(id).map(|p| p.url.clone()) else { return };
        match r.kind {
            RefKind::Local => {
                if r.is_head {
                    let name = &r.short_name;
                    self.show_toast(lf!("{name} is already checked out", "已经在 {name} 上"));
                    return;
                }
                let name = r.short_name.clone();
                let branch = name.clone();
                self.switch_to(name, id, std::sync::Arc::new(move |dir| remote::switch_branch(&branch, dir).map_err(|e| e.message())));
            }
            RefKind::Remote => {
                // "origin/feature/x" becomes "feature/x".
                let local = r.short_name.split_once('/').map(|(_, rest)| rest.to_string()).unwrap_or(r.short_name.clone());
                self.jobs.spawn(
                    move || history::refs(&url).unwrap_or_default(),
                    move |app: &mut App, refs: Vec<GitRef>| {
                        let remote_name = r.short_name.clone();
                        if let Some(existing) = refs.iter().find(|e| e.kind == RefKind::Local && e.short_name == local) {
                            if existing.upstream.as_deref() == Some(remote_name.as_str()) {
                                if existing.is_head {
                                    app.show_toast(lf!("{local} is already checked out", "已经在 {local} 上"));
                                } else {
                                    let branch = local.clone();
                                    app.switch_to(local, id, std::sync::Arc::new(move |dir| remote::switch_branch(&branch, dir).map_err(|e| e.message())));
                                }
                            } else {
                                // The name is taken by a branch that follows something else: pick another.
                                app.new_branch_from(Some(r.name.clone()), Some(remote_name.clone()), Some(remote_name), id);
                            }
                        } else {
                            let branch = local.clone();
                            app.switch_to(
                                local,
                                id,
                                std::sync::Arc::new(move |dir| remote::track_branch(&remote_name, &branch, dir).map_err(|e| e.message())),
                            );
                        }
                    },
                );
            }
            RefKind::Tag => self.checkout_revision(r.target.clone(), r.short_name.clone(), id),
        }
    }

    /// Checks out a commit without a branch, after saying what that means.
    pub fn checkout_revision(&mut self, hash: String, title: String, id: ProjectId) {
        let request = DialogRequest::new(
            DialogStyle::Caution,
            Icon::Circle,
            lf!("Check out {title} without a branch?", "不带分支检出 {title}？"),
            l("Check Out", "检出"),
        )
        .message(l(
            "HEAD will point at the commit itself (a detached HEAD), to look at or build that version. Commits made there belong to no branch: create a branch first to keep them.",
            "HEAD 将直接指向这个提交（游离 HEAD），适合查看或构建这个版本。在这里做的提交不属于任何分支：要保留它们，请先新建分支。",
        ))
        .confirm(move |app| app.switch_to(title, id, std::sync::Arc::new(move |dir| remote::detach(&hash, dir).map_err(|e| e.message()))));
        self.ask(request);
    }

    pub fn new_branch(&mut self) {
        if let Some(id) = self.selected {
            self.new_branch_from(None, None, None, id);
        }
    }

    /// Asks for a name, then creates a branch at `start` (HEAD when None) and switches to it, local changes and all.
    /// `tracking`: a remote branch the new one follows.
    pub fn new_branch_from(&mut self, start: Option<String>, start_title: Option<String>, tracking: Option<String>, id: ProjectId) {
        let Some(url) = self.project(id).map(|p| p.url.clone()) else { return };
        self.jobs.spawn(
            move || history::refs(&url).unwrap_or_default().into_iter().filter(|r| r.kind == RefKind::Local).map(|r| r.short_name).collect::<HashSet<_>>(),
            move |app: &mut App, existing: HashSet<String>| {
                let title = match &start_title {
                    Some(t) => lf!("New branch from {t}", "从 {t} 新建分支"),
                    None => l("New branch", "新建分支"),
                };
                let request = DialogRequest::new(DialogStyle::Info, Icon::Branch, title, l("Create", "创建"))
                    .message(l("It is checked out right away; your local changes come along.", "创建后立即检出，本地改动会一起带过去。"))
                    .with_input(l("Branch name", "分支名"), "", move |name| remote::branch_name_problem(name, &existing))
                    .answer(move |app, answer| {
                        let name = answer.text;
                        let branch = name.clone();
                        let attempt: Attempt = match tracking {
                            Some(tracking) => std::sync::Arc::new(move |dir| remote::track_branch(&tracking, &branch, dir).map_err(|e| e.message())),
                            None => std::sync::Arc::new(move |dir| remote::create_branch(&branch, start.as_deref(), dir).map_err(|e| e.message())),
                        };
                        app.switch_to(name, id, attempt);
                    });
                app.ask(request);
            },
        );
    }

    /// Runs a checkout. When local changes are in the way, offers to carry them over (Smart Checkout).
    fn switch_to(&mut self, title: String, id: ProjectId, attempt: Attempt) {
        let Some(p) = self.project(id) else { return };
        if let Some(operation) = p.status.operation {
            let command = operation.command();
            self.show_toast(lf!("Finish or abort the {command} first", "请先完成或中止当前的 {command}"));
            return;
        }
        if !self.viewer.save_edits() {
            return;
        }
        let run = attempt.clone();
        self.run_activity(
            lf!("Checking out {title}…", "正在检出 {title}…"),
            false,
            id,
            move |dir, _| run(dir),
            move |app, result| match result {
                Ok(()) => {
                    app.after_branches_changed(id);
                    app.show_toast(lf!("Checked out {title}", "已检出 {title}"));
                }
                Err(message) => {
                    let files = remote::overwritten_files(&message);
                    if !files.is_empty() && !message.contains("untracked working tree files") {
                        app.offer_smart_checkout(title, files, id, attempt);
                    } else {
                        app.show_failure(lf!("Could not check out {title}", "无法检出 {title}"), &message, None);
                    }
                }
            },
        );
    }

    fn offer_smart_checkout(&mut self, title: String, files: Vec<String>, id: ProjectId, attempt: Attempt) {
        let changes: Vec<FileChange> = files.iter().map(|f| FileChange::new(f.clone(), None, ChangeKind::Modified)).collect();
        let request = DialogRequest::new(
            DialogStyle::Caution,
            Icon::Branch,
            lf!("Your changes would be overwritten by {title}", "检出 {title} 会覆盖你的本地改动"),
            l("Smart Checkout", "智能检出"),
        )
        .message(lf!(
            "Smart Checkout puts your changes aside, checks out {title}, then puts them back. Where they clash, the files show up as conflicts to resolve.",
            "智能检出会先把改动放到一边，检出 {title} 后再放回来。有冲突的文件会显示为冲突，待你解决。"
        ))
        .with_files(&changes, 8)
        .confirm(move |app| app.smart_checkout(title, id, attempt));
        self.ask(request);
    }

    fn smart_checkout(&mut self, title: String, id: ProjectId, attempt: Attempt) {
        enum Step {
            Aside(String),
            Switch(String),
            Back(String),
            Done,
        }
        let label = title.clone();
        self.run_activity(
            lf!("Checking out {title}…", "正在检出 {title}…"),
            false,
            id,
            move |dir, _| {
                if let Err(e) = remote::stash(&format!("DiffCode: before checking out {label}"), dir) {
                    return Step::Aside(e.message());
                }
                if let Err(e) = attempt(dir) {
                    // Could not switch after all: the changes go back where they were.
                    let _ = remote::unstash(dir);
                    return Step::Switch(e);
                }
                match remote::unstash(dir) {
                    Ok(()) => Step::Done,
                    Err(e) => Step::Back(e.message()),
                }
            },
            move |app, step| match step {
                Step::Aside(e) => app.show_failure(l("Could not put your changes aside", "无法暂存你的改动"), &e, None),
                Step::Switch(e) => {
                    app.after_branches_changed(id);
                    app.show_failure(lf!("Could not check out {title}", "无法检出 {title}"), &e, None);
                }
                Step::Done => {
                    app.after_branches_changed(id);
                    app.show_toast(lf!("Checked out {title} · your changes came along", "已检出 {title} · 本地改动已带过来"));
                }
                Step::Back(e) => {
                    app.after_branches_changed(id);
                    let mut request = DialogRequest::new(
                        DialogStyle::Caution,
                        Icon::Warning,
                        lf!("Checked out {title} · some of your changes clash with it", "已检出 {title} · 部分本地改动与它冲突"),
                        l("OK", "好"),
                    )
                    .notice()
                    .message(l(
                        "The clashing files are marked as conflicts in the Changes list: resolve them there. Your changes also stay in git's stash (stash@{0}) until you drop it.",
                        "冲突的文件已在变更列表中标出，请在那里解决。你的改动也仍保存在 git 的 stash（stash@{0}）里，直到你删除它。",
                    ));
                    request.output = Some(e);
                    app.ask(request);
                }
            },
        );
    }

    // MARK: Remotes

    pub fn fetch(&mut self) {
        if let Some(id) = self.selected {
            self.fetch_project(id);
        }
    }

    /// Brings every remote's branches up to date.
    pub fn fetch_project(&mut self, id: ProjectId) {
        self.run_activity(
            l("Fetching…", "正在获取…"),
            true,
            id,
            |dir, token| {
                remote::fetch(dir, Some(token))?;
                let counts = git::run_string(dir, ["rev-list", "--left-right", "--count", "HEAD...@{upstream}"]).unwrap_or_default();
                Ok(behind_count(&counts))
            },
            move |app, result: git::Result<usize>| match result {
                Ok(behind) => {
                    app.after_branches_changed(id);
                    app.show_toast(if behind > 0 {
                        lf!("Fetched · ↓{behind} to pull", "已获取 · 有 {behind} 个提交可拉取")
                    } else {
                        l("Fetched · up to date", "已获取 · 已是最新")
                    });
                }
                Err(e) if is_cancelled(&e) => app.after_branches_changed(id),
                Err(e) => app.show_failure(l("Fetch failed", "获取失败"), &e, App::remote_note(&e.message())),
            },
        );
    }

    pub fn pull(&mut self) {
        if let Some(id) = self.selected {
            self.pull_project(id, PullMode::Configured, false);
        }
    }

    /// Update Project: brings the upstream's commits into the current branch.
    pub fn pull_project(&mut self, id: ProjectId, mode: PullMode, autostash: bool) {
        let Some(p) = self.project(id) else { return };
        let Some(branch) = p.status.branch.clone() else {
            self.show_toast(l("No branch is checked out (detached HEAD) · nothing to pull into", "当前不在任何分支上（游离 HEAD）· 无法拉取"));
            return;
        };
        let Some(upstream) = p.status.upstream.clone() else {
            self.show_toast(lf!("{branch} follows no remote branch · push it first", "{branch} 没有跟踪的远程分支 · 请先推送"));
            return;
        };
        if let Some(operation) = p.status.operation {
            let command = operation.command();
            self.show_toast(lf!("Finish or abort the {command} first", "请先完成或中止当前的 {command}"));
            return;
        }
        let git_directory = p.git_directory().map(|d| d.to_path_buf());
        if !self.viewer.save_edits() {
            return;
        }
        struct Pulled {
            before: Option<String>,
            after: Option<String>,
            count: usize,
            clashes: bool,
        }
        self.run_activity(
            l("Pulling…", "正在拉取…"),
            true,
            id,
            move |dir, token| {
                let before = history::resolve_commit("HEAD", dir);
                remote::pull(mode, autostash, dir, Some(token))?;
                let after = history::resolve_commit("HEAD", dir);
                let mut count = 0;
                // What came in from the upstream, not the merge commit or replayed commits of one's own.
                if let (Some(b), Some(a)) = (&before, &after) {
                    if b != a {
                        if let Ok(out) = git::run_string(dir, ["rev-list".to_string(), "--count".into(), format!("{b}..@{{upstream}}")]) {
                            count = out.trim().parse().unwrap_or(0);
                        }
                    }
                }
                let clashes = git::run_string(dir, ["diff", "--name-only", "--diff-filter=U"]).unwrap_or_default();
                Ok(Pulled { before, after, count, clashes: !clashes.trim().is_empty() })
            },
            move |app, result: git::Result<Pulled>| {
                app.after_branches_changed(id);
                match result {
                    Ok(pulled) => {
                        if pulled.clashes {
                            // The pull went through, but putting the local changes back clashed with it.
                            app.show_conflicts_notice(
                                l("Pulled · your local changes clash with what came in", "已拉取 · 本地改动与拉下来的内容冲突"),
                                Some(l("Your changes also stay in git's stash until you drop it.", "你的改动也仍保存在 git 的 stash 里，直到你删除它。")),
                            );
                        } else if pulled.before == pulled.after {
                            app.show_toast(lf!("Already up to date with {upstream}", "已是最新，与 {upstream} 一致"));
                        } else {
                            let count = pulled.count;
                            app.show_toast(if count == 1 {
                                lf!("Pulled 1 commit from {upstream}", "已从 {upstream} 拉取 1 个提交")
                            } else {
                                lf!("Pulled {count} commits from {upstream}", "已从 {upstream} 拉取 {count} 个提交")
                            });
                        }
                    }
                    Err(e) if is_cancelled(&e) => {}
                    Err(e) => {
                        let message = e.message();
                        if message.contains("divergent branches") {
                            app.ask_how_to_reconcile(branch, upstream, id, autostash);
                        } else if !autostash
                            && (message.contains("would be overwritten by merge")
                                || message.contains("cannot pull with rebase")
                                || message.contains("commit or stash them")
                                || message.contains("commit your changes or stash them"))
                        {
                            app.offer_autostash_pull(upstream, mode, id, message);
                        } else if git_directory.as_deref().and_then(remote::operation).is_some() {
                            app.show_conflicts_notice(l("The pull stopped at conflicts", "拉取在冲突处停下了"), None);
                        } else {
                            app.show_failure(l("Pull failed", "拉取失败"), &e, App::remote_note(&message));
                        }
                    }
                }
            },
        );
    }

    fn ask_how_to_reconcile(&mut self, branch: String, upstream: String, id: ProjectId, autostash: bool) {
        let mut request = DialogRequest::new(
            DialogStyle::Info,
            Icon::Merge,
            lf!("{branch} and {upstream} both have new commits", "{branch} 与 {upstream} 各有新提交"),
            l("Merge", "合并"),
        )
        .message(lf!(
            "Merge joins them with a merge commit. Rebase replays your commits on top of {upstream}.",
            "合并：用一个合并提交把两边连起来。变基：把你的提交重新接在 {upstream} 之后。"
        ))
        .confirm(move |app| app.pull_project(id, PullMode::Merge, autostash));
        request.alternate_title = Some(l("Rebase", "变基"));
        request.on_alternate = Some(Box::new(move |app| app.pull_project(id, PullMode::Rebase, autostash)));
        self.ask(request);
    }

    fn offer_autostash_pull(&mut self, upstream: String, mode: PullMode, id: ProjectId, output: String) {
        let mut request = DialogRequest::new(
            DialogStyle::Caution,
            Icon::Archive,
            l("Your local changes are in the way", "本地改动挡住了拉取"),
            l("Stash, Pull, Put Back", "暂存后拉取再恢复"),
        )
        .message(lf!(
            "Put them aside, pull from {upstream}, then put them back? Where they clash, the files show up as conflicts to resolve.",
            "先把它们放到一边，从 {upstream} 拉取，再放回来？有冲突的文件会显示为冲突，待你解决。"
        ))
        .confirm(move |app| app.pull_project(id, mode, true));
        request.output = Some(output);
        self.ask(request);
    }

    pub fn show_conflicts_notice(&mut self, title: String, note: Option<String>) {
        let mut message = l(
            "The files in conflict are marked in the Changes list. Open one to pick a side for each conflict, then mark it resolved.",
            "冲突的文件已在变更列表中标出。打开文件为每处冲突选择保留哪一边，然后标记为已解决。",
        );
        if let Some(note) = note {
            message.push_str("\n\n");
            message.push_str(&note);
        }
        self.ask(DialogRequest::new(DialogStyle::Caution, Icon::Warning, title, l("OK", "好")).notice().message(message));
    }

    pub fn push(&mut self) {
        if let Some(id) = self.selected {
            self.push_project(id);
        }
    }

    /// Ctrl+Shift+K: lists the commits a push would send, and pushes them on Enter.
    pub fn push_project(&mut self, id: ProjectId) {
        let Some(p) = self.project(id) else { return };
        let Some(branch) = p.status.branch.clone() else {
            self.show_toast(l("No branch is checked out (detached HEAD) · nothing to push", "当前不在任何分支上（游离 HEAD）· 无法推送"));
            return;
        };
        let upstream = p.status.upstream.clone();
        let url = p.url.clone();
        struct Plan {
            remotes: Vec<String>,
            remote: Option<String>,
            commits: Vec<Commit>,
            incoming: Vec<Commit>,
            protected: bool,
            expected: Option<String>,
        }
        let b = branch.clone();
        let up = upstream.clone();
        self.jobs.spawn(
            move || {
                let remotes = remote::remotes(&url);
                // With no upstream yet: the only remote, or "origin" among several.
                let remote = if up.is_some() {
                    None
                } else if remotes.len() == 1 {
                    remotes.first().cloned()
                } else if remotes.iter().any(|r| r == "origin") {
                    Some("origin".to_string())
                } else {
                    None
                };
                let mut plan = Plan { remotes, remote, commits: Vec::new(), incoming: Vec::new(), protected: true, expected: None };
                if up.is_none() && plan.remote.is_none() {
                    return plan;
                }
                plan.commits = remote::outgoing_commits(up.as_deref(), plan.remote.as_deref(), &url, 200);
                if up.is_some() {
                    // The upstream has commits the branch does not: pull them, or, when the branch was rewritten, force.
                    plan.incoming = history_edits::incoming_commits(&url, 50);
                    if !plan.incoming.is_empty() && !plan.commits.is_empty() {
                        plan.protected = history_edits::upstream_target(&b, &url).is_none_or(|(_, t)| PROTECTED_BRANCHES.contains(&t.as_str()));
                        plan.expected = history::resolve_commit("@{upstream}", &url);
                    }
                }
                plan
            },
            move |app: &mut App, plan: Plan| {
                if upstream.is_none() && plan.remote.is_none() {
                    app.show_toast(if plan.remotes.is_empty() {
                        l("This repository has no remote to push to", "这个仓库没有可推送的远程")
                    } else {
                        l("Several remotes and no upstream · push once in a terminal to pick one", "有多个远程且没有上游 · 请先在终端里推送一次以选定远程")
                    });
                    return;
                }
                let remote = plan.remote.clone();
                let target = upstream.clone().unwrap_or_else(|| format!("{}/{branch}", remote.as_deref().unwrap_or("origin")));
                if let Some(upstream) = &upstream {
                    if !plan.incoming.is_empty() {
                        if plan.commits.is_empty() {
                            app.show_toast(lf!("Nothing to push · {upstream} has commits to pull", "没有要推送的提交 · {upstream} 有可拉取的提交"));
                        } else {
                            let force_to = if plan.protected || PROTECTED_BRANCHES.contains(&branch.as_str()) { None } else { plan.expected };
                            app.offer_reconcile(branch, upstream.clone(), plan.incoming, force_to, id);
                        }
                        return;
                    }
                }
                if plan.commits.is_empty() && upstream.is_some() {
                    app.show_toast(lf!("Nothing to push · {branch} is level with {target}", "没有要推送的提交 · {branch} 与 {target} 一致"));
                    return;
                }
                let n = plan.commits.len();
                let title = if upstream.is_none() {
                    let r = remote.as_deref().unwrap_or("origin");
                    lf!("Push {branch} to {r}?", "把 {branch} 推送到 {r}？")
                } else if n == 1 {
                    lf!("Push 1 commit to {target}?", "推送 1 个提交到 {target}？")
                } else {
                    lf!("Push {n} commits to {target}?", "推送 {n} 个提交到 {target}？")
                };
                let mut request = DialogRequest::new(DialogStyle::Info, Icon::ArrowUpload, title, l("Push", "推送"));
                if upstream.is_none() {
                    request.message = if plan.commits.is_empty() {
                        lf!(
                            "{target} is created at a commit the remote already has, and {branch} follows it from now on.",
                            "将在远程已有的提交上创建 {target}，此后 {branch} 跟踪它。"
                        )
                    } else {
                        lf!("{target} does not exist yet: it is created, and {branch} follows it from now on.", "{target} 还不存在：将创建它，此后 {branch} 跟踪它。")
                    };
                }
                (request.items, request.hidden_count) = commit_items(&plan.commits, 6, Icon::Circle);
                request.on_confirm = Some(Box::new(move |app: &mut App| app.perform_push(branch, remote, target, n, id)));
                app.ask(request);
            },
        );
    }

    /// The branch and its upstream have both moved on. Pulling is the way, unless the branch was rewritten on purpose
    /// (amend, rebase, reset): then Force Push replaces the upstream's commits.
    /// `force_to`: the upstream's commit a force push may replace; None where force is not offered (main, master).
    fn offer_reconcile(&mut self, branch: String, upstream: String, incoming: Vec<Commit>, force_to: Option<String>, id: ProjectId) {
        let message = if force_to.is_none() {
            lf!(
                "{upstream} has the commits below, which {branch} does not. Pull them first, then push.",
                "{upstream} 有下面这些 {branch} 没有的提交。请先拉取，再推送。"
            )
        } else {
            lf!(
                "{upstream} has the commits below, which {branch} does not. Pull them first, then push. If {branch} was rewritten on purpose (amend, rebase, reset), Force Push replaces them on the remote instead.",
                "{upstream} 有下面这些 {branch} 没有的提交。请先拉取，再推送。如果 {branch} 是有意改写的（修改提交、变基、重置），可以用“强制推送”在远程替换掉它们。"
            )
        };
        let mut request = DialogRequest::new(
            DialogStyle::Caution,
            Icon::Branch,
            lf!("{branch} and {upstream} have both moved on", "{branch} 与 {upstream} 各自有了新提交"),
            l("Pull", "拉取"),
        )
        .message(message)
        .confirm(move |app| app.pull_project(id, PullMode::Configured, false));
        (request.items, request.hidden_count) = commit_items(&incoming, 5, Icon::Circle);
        if let Some(expected) = force_to {
            request.alternate_title = Some(l("Force Push…", "强制推送…"));
            request.on_alternate = Some(Box::new(move |app: &mut App| app.confirm_force_push(branch, upstream, incoming, expected, id)));
        }
        self.ask(request);
    }

    fn confirm_force_push(&mut self, branch: String, upstream: String, incoming: Vec<Commit>, expected: String, id: ProjectId) {
        let mut request = DialogRequest::new(
            DialogStyle::Destructive,
            Icon::Warning,
            lf!("Force push {branch} to {upstream}?", "强制推送 {branch} 到 {upstream}？"),
            l("Force Push", "强制推送"),
        )
        .message(lf!(
            "{upstream} becomes {branch} as it is here, and the commits below go off the remote, for everyone who uses it. If someone pushed to {upstream} since it was last fetched, nothing is overwritten.",
            "{upstream} 会变成本地 {branch} 的样子，下面这些提交会从远程去掉，影响所有使用它的人。如果上次获取后有人推送过 {upstream}，则不会覆盖任何内容。"
        ));
        (request.items, request.hidden_count) = commit_items(&incoming, 5, Icon::Subtract);
        request.on_confirm = Some(Box::new(move |app: &mut App| {
            let b = branch.clone();
            app.run_activity(
                l("Force pushing…", "正在强制推送…"),
                true,
                id,
                move |dir, _| history_edits::force_push(&b, &expected, dir),
                move |app, result| {
                    app.after_branches_changed(id);
                    match result {
                        Ok(()) => app.show_toast(lf!("Force-pushed {branch} to {upstream}", "已强制推送 {branch} 到 {upstream}")),
                        Err(e) => {
                            let note = if e.message.contains("stale info") {
                                Some(lf!(
                                    "{upstream} has moved on since it was last fetched, so nothing was overwritten. Fetch, look at what came in, then decide again.",
                                    "{upstream} 在上次获取后又有了新提交，所以没有覆盖任何内容。请先获取，看看新来的提交，再决定。"
                                ))
                            } else {
                                App::remote_note(&e.message)
                            };
                            app.show_failure(l("Force push failed", "强制推送失败"), &e, note);
                        }
                    }
                },
            );
        }));
        self.ask(request);
    }

    fn perform_push(&mut self, branch: String, remote: Option<String>, target: String, count: usize, id: ProjectId) {
        let b = branch.clone();
        let r = remote.clone();
        self.run_activity(
            l("Pushing…", "正在推送…"),
            true,
            id,
            move |dir, token| remote::push(&b, r.as_deref(), dir, Some(token)),
            move |app, result| match result {
                Ok(()) => {
                    app.after_branches_changed(id);
                    app.show_toast(if count == 0 {
                        lf!("Pushed {branch} to {target}", "已推送 {branch} 到 {target}")
                    } else if count == 1 {
                        lf!("Pushed 1 commit to {target}", "已推送 1 个提交到 {target}")
                    } else {
                        lf!("Pushed {count} commits to {target}", "已推送 {count} 个提交到 {target}")
                    });
                }
                Err(e) if is_cancelled(&e) => app.after_branches_changed(id),
                Err(e) => {
                    let message = e.message();
                    if remote::is_rejected_as_behind(&message) {
                        let mut request = DialogRequest::new(
                            DialogStyle::Caution,
                            Icon::ArrowDownload,
                            lf!("Push rejected: {target} has commits you don't have", "推送被拒绝：{target} 有你本地没有的提交"),
                            l("Pull", "拉取"),
                        )
                        .message(l("Pull them first, then push again.", "请先拉取，再推送。"))
                        .confirm(move |app| app.pull_project(id, PullMode::Configured, false));
                        request.output = Some(message);
                        app.ask(request);
                    } else {
                        app.show_failure(l("Push failed", "推送失败"), &e, App::remote_note(&message));
                    }
                }
            },
        );
    }

    // MARK: Merge or rebase in progress

    pub fn continue_operation(&mut self) {
        if let Some(id) = self.selected {
            self.continue_operation_in(id);
        }
    }

    /// Continue in the banner: the rebase, cherry-pick or revert goes on; a merge is finished by committing it.
    pub fn continue_operation_in(&mut self, id: ProjectId) {
        let Some(p) = self.project(id) else { return };
        let Some(operation) = p.status.operation else { return };
        if p.status.conflict_count() > 0 {
            self.show_toast(l("Resolve the conflicts first", "请先解决冲突"));
            return;
        }
        if operation == remote::RepoOperation::Merge {
            self.focus_commit_message();
            self.commit_current(false);
            return;
        }
        let git_directory: Option<PathBuf> = p.git_directory().map(|d| d.to_path_buf());
        if !self.viewer.save_edits() {
            return;
        }
        let command = operation.command();
        self.run_activity(
            lf!("Continuing the {command}…", "正在继续 {command}…"),
            false,
            id,
            move |dir, _| {
                remote::continue_operation(operation, dir).map_err(|e| {
                    let stopped = git_directory.as_deref().and_then(remote::operation).is_some()
                        && !git::run_string(dir, ["diff", "--name-only", "--diff-filter=U"]).unwrap_or_default().trim().is_empty();
                    (e, stopped)
                })
            },
            move |app, result| {
                app.after_branches_changed(id);
                match result {
                    Ok(()) => app.show_toast(l("Done", "已完成")),
                    Err((_, true)) => app.show_conflicts_notice(l("Stopped at more conflicts", "又在冲突处停下了"), None),
                    Err((e, false)) => app.show_failure(lf!("Could not continue the {command}", "无法继续 {command}"), &e, None),
                }
            },
        );
    }

    pub fn abort_operation(&mut self) {
        if let Some(id) = self.selected {
            self.abort_operation_in(id);
        }
    }

    /// Abort in the banner: back to how things were before the merge or rebase started.
    pub fn abort_operation_in(&mut self, id: ProjectId) {
        let Some(operation) = self.project(id).and_then(|p| p.status.operation) else { return };
        let command = operation.command();
        let request = DialogRequest::new(
            DialogStyle::Destructive,
            Icon::DismissCircle,
            lf!("Abort the {command}?", "中止 {command}？"),
            l("Abort", "中止"),
        )
        .message(lf!(
            "The branch goes back to where it was before the {command} started, and what it brought in is dropped, conflict resolutions included.",
            "分支会回到 {command} 开始之前的状态，它带来的改动（包括已解决的冲突）都会丢弃。"
        ))
        .confirm(move |app| {
            app.run_activity(
                l("Aborting…", "正在中止…"),
                false,
                id,
                move |dir, _| remote::abort_operation(operation, dir),
                move |app, result| {
                    app.after_branches_changed(id);
                    match result {
                        Ok(()) => app.show_toast(l("Aborted · back to where you were", "已中止 · 回到了开始之前")),
                        Err(e) => app.show_failure(lf!("Could not abort the {command}", "无法中止 {command}"), &e, None),
                    }
                },
            );
        });
        self.ask(request);
    }

    // MARK: Conflicts

    /// Tells git the file is resolved. Asks first if it still has conflict markers in it.
    pub fn mark_resolved(&mut self, path: String, id: ProjectId) {
        if !self.viewer.save_edits() {
            return;
        }
        let Some(url) = self.project(id).map(|p| p.url.clone()) else { return };
        let full = crate::core::diff::join_repo_path(&url, &path);
        let resolve = move |app: &mut App| {
            let p = path.clone();
            let root = url.clone();
            app.jobs.spawn(
                move || remote::mark_resolved(&[p], &root),
                move |app: &mut App, result| match result {
                    Ok(()) => {
                        if let Some(project) = app.project_mut(id) {
                            project.refresh();
                        }
                        let name = file_name(&path);
                        app.show_toast(lf!("Marked {name} as resolved", "已将 {name} 标记为已解决"));
                    }
                    Err(e) => app.show_failure(l("Could not mark it as resolved", "无法标记为已解决"), &e, None),
                },
            );
        };
        let has_markers = std::fs::read(&full).is_ok_and(|data| remote::has_conflict_markers(&data));
        if has_markers {
            let name = file_name(&full.to_string_lossy()).to_string();
            let request = DialogRequest::new(
                DialogStyle::Caution,
                Icon::Warning,
                lf!("{name} still has conflict markers", "{name} 里还有冲突标记"),
                l("Mark as Resolved", "标记为已解决"),
            )
            .message(l(
                "Lines starting with <<<<<<< or >>>>>>> are still in the file. Mark it as resolved anyway?",
                "文件里还有以 <<<<<<< 或 >>>>>>> 开头的行。仍然标记为已解决？",
            ))
            .confirm(resolve);
            self.ask(request);
        } else {
            resolve(self);
        }
    }

    /// Resolves a file by taking HEAD's version, or the incoming one, whole.
    pub fn take_side(&mut self, side: ConflictSide, path: String, id: ProjectId) {
        let Some(url) = self.project(id).map(|p| p.url.clone()) else { return };
        let name = file_name(&path).to_string();
        let ours = side == ConflictSide::Ours;
        let request = DialogRequest::new(
            DialogStyle::Caution,
            Icon::Merge,
            if ours { lf!("Keep HEAD’s version of {name}?", "保留 HEAD 版本的 {name}？") } else { lf!("Take the incoming version of {name}?", "采用传入版本的 {name}？") },
            if ours { l("Keep HEAD’s", "保留 HEAD 的") } else { l("Take Incoming", "采用传入的") },
        )
        .message(l(
            "The other side’s changes to this file are dropped, and so are edits made to it during the merge.",
            "另一边对这个文件的改动会被丢弃，合并过程中对它的编辑也会丢弃。",
        ))
        .confirm(move |app| {
            app.jobs.spawn(
                move || remote::take_side(side, &path, &url),
                move |app: &mut App, result| match result {
                    Ok(()) => {
                        if let Some(p) = app.project_mut(id) {
                            p.refresh();
                        }
                        app.sync_viewer();
                        app.show_toast(lf!("Resolved {name}", "已解决 {name}"));
                    }
                    Err(e) => app.show_failure(lf!("Could not resolve {name}", "无法解决 {name}"), &e, None),
                },
            );
        });
        self.ask(request);
    }
}
