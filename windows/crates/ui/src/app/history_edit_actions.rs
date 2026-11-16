//! Rewriting history from the Log and the Git graph, as the IDE's menus do: cherry-pick, revert and reset; merging,
//! rebasing, renaming and deleting branches. Each is git's own command, and the toast after it offers Undo while
//! nothing else has moved.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use super::dialog::{DialogChoice, DialogItem, DialogRequest, DialogStyle};
use super::project::ProjectId;
use super::{App, AppCallback, MenuItem};
use crate::core::git::{self, Commit};
use crate::core::history::{self, GitRef, RefKind};
use crate::core::history_edits::{self, HeadMove, MergeOutcome, ResetMode, PROTECTED_BRANCHES};
use crate::core::lang::l;
use crate::core::lf;
use crate::core::remote::{self, RepoOperation};
use crate::icons::Icon;

/// How a cherry-pick, revert, merge or rebase that failed ended up, told on the worker thread.
pub enum Stop {
    /// It stopped at conflicts, and waits in the banner.
    Conflicts,
    /// It stopped with nothing to resolve: a commit brings in nothing the branch does not have.
    Empty(String),
    /// Local changes were in the way.
    Refused(String),
    Failed(String),
}

/// Tells how an operation that failed with `message` ended up. Runs git: call it off the main thread.
pub fn stop_of(message: String, git_directory: Option<&Path>, dir: &Path) -> Stop {
    if git_directory.and_then(remote::operation).is_some() {
        let clashes = git::run_string(dir, ["diff", "--name-only", "--diff-filter=U"]).unwrap_or_default();
        if clashes.trim().is_empty() {
            Stop::Empty(message)
        } else {
            Stop::Conflicts
        }
    } else if history_edits::refused_for_local_changes(&message) {
        Stop::Refused(message)
    } else {
        Stop::Failed(message)
    }
}

fn commit_items(commits: &[Commit], limit: usize, icon: Icon) -> (Vec<DialogItem>, usize) {
    let items = commits
        .iter()
        .take(limit)
        .map(|c| DialogItem { title: c.subject.clone(), detail: c.short_hash(), kind: None, icon: Some(icon) })
        .collect();
    (items, commits.len().saturating_sub(limit))
}

fn short(hash: &str) -> String {
    hash.chars().take(7).collect()
}

impl App {
    // MARK: Undo

    /// The toast after the branch moved, with Undo.
    fn announce(&mut self, text: String, undoing: HeadMove, id: ProjectId) {
        if undoing.before == undoing.after {
            self.show_toast(text);
            return;
        }
        self.show_toast_action(text, l("Undo", "撤销"), move |app| app.undo_head_move(undoing, id));
    }

    /// Puts the branch back where a cherry-pick, revert, reset, merge or rebase made here found it.
    pub fn undo_head_move(&mut self, undoing: HeadMove, id: ProjectId) {
        if !self.viewer.save_edits() {
            return;
        }
        let m = undoing.clone();
        self.run_activity(
            l("Undoing…", "正在撤销…"),
            false,
            id,
            move |dir, _| history_edits::undo_head_move(&m, dir),
            move |app, result| {
                app.after_branches_changed(id);
                match result {
                    Ok(()) => {
                        let name = undoing.branch.clone().unwrap_or("HEAD".into());
                        let before = short(&undoing.before);
                        app.show_toast(lf!("Undone · {name} is back at {before}", "已撤销 · {name} 回到了 {before}"));
                    }
                    Err(e) => app.show_failure(l("Could not undo", "无法撤销"), &e, None),
                }
            },
        );
    }

    /// Nothing half done is in the way, and what was typed is saved.
    fn ready_to_rewrite(&mut self, id: ProjectId) -> bool {
        let Some(p) = self.project(id) else { return false };
        if let Some(operation) = p.status.operation {
            let command = operation.command();
            self.show_toast(lf!("Finish or abort the {command} first", "请先完成或中止当前的 {command}"));
            return false;
        }
        if let Some(running) = p.activity.clone() {
            self.show_toast(lf!("Wait for “{running}” to finish", "请等“{running}”完成"));
            return false;
        }
        self.viewer.save_edits()
    }

    fn git_directory_of(&self, id: ProjectId) -> Option<PathBuf> {
        self.project(id).and_then(|p| p.git_directory().map(|d| d.to_path_buf()))
    }

    /// After a cherry-pick, revert, merge or rebase failed: it stopped at conflicts (and waits in the banner), local
    /// changes were in the way, or git said why.
    /// `retry`: does it again with the local changes put aside and back (`--autostash`).
    fn report_stop(&mut self, operation: RepoOperation, stop: Stop, retry: Option<AppCallback>) {
        match stop {
            Stop::Conflicts => {
                let title = match operation {
                    RepoOperation::Merge => l("The merge stopped at conflicts", "合并在冲突处停下了"),
                    RepoOperation::Rebase => l("The rebase stopped at conflicts", "变基在冲突处停下了"),
                    RepoOperation::CherryPick => l("The cherry-pick stopped at conflicts", "拣选在冲突处停下了"),
                    RepoOperation::Revert => l("The revert stopped at conflicts", "还原在冲突处停下了"),
                };
                let note = if operation == RepoOperation::Merge {
                    l("Then commit to finish the merge, or Abort above the list to go back.", "然后提交即可完成合并；或点列表上方的“中止”回到开始之前。")
                } else {
                    l(
                        "Then Continue above the list; Skip leaves that commit out, Abort goes back to the start.",
                        "然后点列表上方的“继续”；“跳过”会略过这个提交，“中止”回到开始之前。",
                    )
                };
                self.show_conflicts_notice(title, Some(note));
            }
            Stop::Empty(message) => {
                let mut request = DialogRequest::new(DialogStyle::Caution, Icon::Warning, l("A commit brings in nothing new", "有个提交没有带来新的改动"), l("OK", "好"))
                    .notice()
                    .message(l(
                        "Its changes are already in the branch. Skip it above the list to go on, or Abort to go back to the start.",
                        "它的改动分支里已经有了。点列表上方的“跳过”继续，或“中止”回到开始之前。",
                    ));
                request.output = Some(message);
                self.ask(request);
            }
            Stop::Refused(message) => match retry {
                Some(retry) => {
                    let command = operation.command();
                    let mut request = DialogRequest::new(DialogStyle::Caution, Icon::Archive, l("Your local changes are in the way", "本地改动挡住了操作"), l("Stash and Go On", "暂存后继续"))
                        .message(lf!(
                            "Put them aside, {command}, then put them back? Where they clash, the files show up as conflicts to resolve.",
                            "先把它们放到一边，执行 {command}，再放回来？有冲突的文件会显示为冲突，待你解决。"
                        ));
                    request.output = Some(message);
                    request.on_confirm = Some(retry);
                    self.ask(request);
                }
                None => self.show_failure(
                    l("Your local changes are in the way", "本地改动挡住了操作"),
                    &message,
                    Some(l(
                        "Commit them, or put them aside with Git ▸ Stash Changes…, then try again.",
                        "请先提交它们，或用 Git ▸ Stash 改动… 暂存起来，再重试。",
                    )),
                ),
            },
            Stop::Failed(message) => {
                let title = match operation {
                    RepoOperation::Merge => l("Merge failed", "合并失败"),
                    RepoOperation::Rebase => l("Rebase failed", "变基失败"),
                    RepoOperation::CherryPick => l("Cherry-pick failed", "拣选失败"),
                    RepoOperation::Revert => l("Revert failed", "还原失败"),
                };
                self.show_failure(title, &message, None);
            }
        }
    }

    pub fn skip_operation(&mut self) {
        if let Some(id) = self.selected {
            self.skip_operation_in(id, false);
        }
    }

    /// Skip in the banner: leaves out the commit the rebase, cherry-pick or revert stopped at, and goes on.
    /// Asks first when that throws away changes (conflict resolutions).
    pub fn skip_operation_in(&mut self, id: ProjectId, confirmed: bool) {
        let Some(p) = self.project(id) else { return };
        let Some(operation) = p.status.operation.filter(|o| *o != RepoOperation::Merge) else { return };
        let has_changes = !p.status.changes.is_empty();
        if !self.viewer.save_edits() {
            return;
        }
        let command = operation.command();
        if !confirmed && has_changes {
            let request = DialogRequest::new(DialogStyle::Destructive, Icon::ArrowRight, l("Skip this commit?", "跳过这个提交？"), l("Skip", "跳过"))
                .message(lf!(
                    "Its changes are left out of the {command}, and so are the conflict resolutions made for it. The {command} goes on with the next commit.",
                    "它的改动不会进入这次 {command}，为它解决的冲突也一并丢弃。之后从下一个提交继续。"
                ))
                .confirm(move |app| app.skip_operation_in(id, true));
            self.ask(request);
            return;
        }
        let gd = self.git_directory_of(id);
        self.run_activity(
            l("Skipping…", "正在跳过…"),
            false,
            id,
            move |dir, _| history_edits::skip_operation(operation, dir).map_err(|e| stop_of(e.message, gd.as_deref(), dir)),
            move |app, result| {
                app.after_branches_changed(id);
                match result {
                    Ok(()) => app.show_toast(lf!("Skipped · the {command} is done", "已跳过 · {command} 已完成")),
                    Err(stop) => app.report_stop(operation, stop, None),
                }
            },
        );
    }

    // MARK: Commits

    /// The commits a menu on `commit` acts on: every picked one when it is one of them, else itself; newest first.
    pub fn menu_commits(&self, commit: &Commit, picked: &HashSet<String>, list: &[Commit]) -> Vec<Commit> {
        if picked.len() > 1 && picked.contains(&commit.hash) {
            list.iter().filter(|c| picked.contains(&c.hash)).cloned().collect()
        } else {
            vec![commit.clone()]
        }
    }

    /// Applies `commits` (newest first, as the lists run) on top of HEAD, oldest first, as new commits. Merge commits,
    /// and commits whose changes HEAD already has, are left out.
    pub fn cherry_pick(&mut self, commits: Vec<Commit>, id: ProjectId) {
        if !self.ready_to_rewrite(id) {
            return;
        }
        enum Outcome {
            Nothing { all_merges: bool },
            Done { picks: Vec<Commit>, left_out: usize, moved: HeadMove },
            Stopped(Stop),
        }
        let gd = self.git_directory_of(id);
        self.run_activity(
            l("Cherry-picking…", "正在拣选…"),
            false,
            id,
            move |dir, _| {
                let mut picks = Vec::new();
                let mut left_out = 0;
                for commit in commits.iter().rev() {
                    let contained = commit.is_merge() || history_edits::is_ancestor(&commit.hash, "HEAD", dir);
                    if contained {
                        left_out += 1;
                    } else {
                        picks.push(commit.clone());
                    }
                }
                let applied = history_edits::already_applied(&picks.iter().map(|c| c.hash.clone()).collect::<Vec<_>>(), dir);
                left_out += applied.len();
                picks.retain(|c| !applied.contains(&c.hash));
                if picks.is_empty() {
                    return Outcome::Nothing { all_merges: commits.iter().all(|c| c.is_merge()) };
                }
                match history_edits::cherry_pick(&picks.iter().map(|c| c.hash.clone()).collect::<Vec<_>>(), dir) {
                    Ok(moved) => Outcome::Done { picks, left_out, moved },
                    Err(e) => Outcome::Stopped(stop_of(e.message, gd.as_deref(), dir)),
                }
            },
            move |app, outcome| match outcome {
                Outcome::Nothing { all_merges } => app.show_toast(if all_merges {
                    l("Merge commits can't be cherry-picked", "合并提交不能拣选")
                } else {
                    l("The current branch already has these changes", "当前分支已经有这些改动")
                }),
                Outcome::Done { picks, left_out, moved } => {
                    app.after_branches_changed(id);
                    let mut text = if picks.len() == 1 {
                        let h = picks[0].short_hash();
                        lf!("Cherry-picked {h}", "已拣选 {h}")
                    } else {
                        let n = picks.len();
                        lf!("Cherry-picked {n} commits", "已拣选 {n} 个提交")
                    };
                    if left_out > 0 {
                        text.push_str(&lf!(" · {left_out} left out", " · 略过 {left_out} 个"));
                    }
                    app.announce(text, moved, id);
                }
                Outcome::Stopped(stop) => {
                    app.after_branches_changed(id);
                    app.report_stop(RepoOperation::CherryPick, stop, None);
                }
            },
        );
    }

    /// Adds commits that undo `commits` (newest first), each with git's "Revert …" message. Only commits of the
    /// current branch can be, and not merges.
    pub fn revert(&mut self, commits: Vec<Commit>, id: ProjectId) {
        if !self.ready_to_rewrite(id) {
            return;
        }
        enum Outcome {
            Nothing { all_merges: bool },
            Done { picks: Vec<Commit>, left_out: usize, moved: HeadMove },
            Stopped(Stop),
        }
        let gd = self.git_directory_of(id);
        self.run_activity(
            l("Reverting…", "正在还原…"),
            false,
            id,
            move |dir, _| {
                let picks: Vec<Commit> =
                    commits.iter().filter(|c| !c.is_merge() && history_edits::is_ancestor(&c.hash, "HEAD", dir)).cloned().collect();
                if picks.is_empty() {
                    return Outcome::Nothing { all_merges: commits.iter().all(|c| c.is_merge()) };
                }
                let left_out = commits.len() - picks.len();
                match history_edits::revert(&picks.iter().map(|c| c.hash.clone()).collect::<Vec<_>>(), dir) {
                    Ok(moved) => Outcome::Done { picks, left_out, moved },
                    Err(e) => Outcome::Stopped(stop_of(e.message, gd.as_deref(), dir)),
                }
            },
            move |app, outcome| match outcome {
                Outcome::Nothing { all_merges } => app.show_toast(if all_merges {
                    l("Merge commits can't be reverted here", "合并提交不能在这里还原")
                } else {
                    l("Only commits of the current branch can be reverted", "只能还原当前分支上的提交")
                }),
                Outcome::Done { picks, left_out, moved } => {
                    app.after_branches_changed(id);
                    let mut text = if picks.len() == 1 {
                        let h = picks[0].short_hash();
                        lf!("Reverted {h}", "已还原 {h}")
                    } else {
                        let n = picks.len();
                        lf!("Reverted {n} commits", "已还原 {n} 个提交")
                    };
                    if left_out > 0 {
                        text.push_str(&lf!(" · {left_out} left out", " · 略过 {left_out} 个"));
                    }
                    app.announce(text, moved, id);
                }
                Outcome::Stopped(stop) => {
                    app.after_branches_changed(id);
                    app.report_stop(RepoOperation::Revert, stop, None);
                }
            },
        );
    }

    /// "Reset Current Branch to Here…": says which commits leave the branch, and asks how far the reset goes.
    pub fn reset_current_branch(&mut self, commit: Commit, id: ProjectId) {
        if !self.ready_to_rewrite(id) {
            return;
        }
        let Some(url) = self.project(id).map(|p| p.url.clone()) else { return };
        struct Plan {
            head: String,
            name: String,
            leaving: Vec<Commit>,
            gained: usize,
            unpushed: usize,
        }
        let target = commit.hash.clone();
        self.jobs.spawn(
            move || {
                let head = history_edits::head(&url).ok()?;
                let name = history_edits::current_branch(&url).unwrap_or("HEAD".into());
                let leaving = history_edits::commits(&[&format!("{target}..HEAD")], &url, 200);
                let gained = history_edits::count(&[&format!("HEAD..{target}")], &url);
                let unpushed = history_edits::count(&[&format!("{target}..HEAD"), "--not", "--remotes"], &url);
                Some(Plan { head, name, leaving, gained, unpushed })
            },
            move |app: &mut App, plan: Option<Plan>| {
                let Some(plan) = plan else { return };
                let name = plan.name.clone();
                let hash = commit.short_hash();
                if plan.head == commit.hash {
                    app.show_toast(lf!("{name} is already at {hash}", "{name} 已经在 {hash}"));
                    return;
                }
                let pushed = plan.leaving.len().saturating_sub(plan.unpushed);
                let mut lines = Vec::new();
                let n = plan.leaving.len();
                if plan.gained > 0 {
                    let g = plan.gained;
                    lines.push(if g == 1 {
                        lf!("{name} gains 1 commit it does not have.", "{name} 会多出 1 个它原本没有的提交。")
                    } else {
                        lf!("{name} gains {g} commits it does not have.", "{name} 会多出 {g} 个它原本没有的提交。")
                    });
                }
                lines.push(match n {
                    0 => lf!("No commit leaves {name}.", "不会有提交离开 {name}。"),
                    1 => lf!("1 commit leaves {name}:", "有 1 个提交将离开 {name}："),
                    _ => lf!("{n} commits leave {name}:", "有 {n} 个提交将离开 {name}："),
                });
                if pushed > 0 {
                    lines.push(lf!(
                        "{pushed} of them are already on the remote: {name} will be behind it, and only a force push could take them off there.",
                        "其中 {pushed} 个已经推送到远程：{name} 会落后于远程，只有强制推送才能把它们从远程去掉。"
                    ));
                }
                let mut request = DialogRequest::new(DialogStyle::Caution, Icon::ArrowUndo, lf!("Reset {name} to {hash}?", "将 {name} 重置到 {hash}？"), l("Reset", "重置"))
                    .message(lines.join("\n"));
                (request.items, request.hidden_count) = commit_items(&plan.leaving, 5, Icon::Subtract);
                request.choices = ResetMode::ALL
                    .iter()
                    .map(|m| DialogChoice { title: m.title().to_string(), detail: m.detail(), destructive: *m == ResetMode::Hard })
                    .collect();
                request.choice = ResetMode::ALL.iter().position(|m| *m == ResetMode::Mixed).unwrap_or(0);
                request.on_answer = Some(Box::new(move |app: &mut App, answer| app.reset_to(commit, ResetMode::ALL[answer.choice], name, id)));
                app.ask(request);
            },
        );
    }

    fn reset_to(&mut self, commit: Commit, mode: ResetMode, name: String, id: ProjectId) {
        if !self.ready_to_rewrite(id) {
            return;
        }
        let target = commit.hash.clone();
        self.run_activity(
            l("Resetting…", "正在重置…"),
            false,
            id,
            move |dir, _| history_edits::reset(&target, mode, dir),
            move |app, result| {
                app.after_branches_changed(id);
                match result {
                    Ok(moved) => {
                        let (hash, title) = (commit.short_hash(), mode.title());
                        app.announce(lf!("Reset {name} to {hash} ({title})", "已将 {name} 重置到 {hash}（{title}）"), moved, id);
                    }
                    Err(e) => {
                        let note = (mode == ResetMode::Keep).then(|| {
                            l(
                                "Keep refuses when your local changes are in files those commits changed. Pick Mixed to keep both, or stash them first.",
                                "如果本地改动所在的文件也被那些提交改过，Keep 会拒绝。可以改用 Mixed 两者都保留，或先暂存本地改动。",
                            )
                        });
                        app.show_failure(lf!("Could not reset {name}", "无法重置 {name}"), &e, note);
                    }
                }
            },
        );
    }

    // MARK: Branches

    /// Merges `r` into the current branch ("Merge into Current"), with git's own message.
    pub fn merge_ref(&mut self, r: GitRef, id: ProjectId, autostash: bool) {
        if !self.ready_to_rewrite(id) {
            return;
        }
        let Some(current) = self.project(id).and_then(|p| p.status.branch.clone()) else {
            self.show_toast(l("No branch is checked out (detached HEAD) · nothing to merge into", "当前不在任何分支上（游离 HEAD）· 无法合并"));
            return;
        };
        let gd = self.git_directory_of(id);
        let name = r.name.clone();
        let short_name = r.short_name.clone();
        self.run_activity(
            lf!("Merging {short_name}…", "正在合并 {short_name}…"),
            false,
            id,
            move |dir, _| history_edits::merge(&name, autostash, dir).map_err(|e| stop_of(e.message, gd.as_deref(), dir)),
            move |app, result| {
                app.after_branches_changed(id);
                match result {
                    Ok((MergeOutcome::UpToDate, _)) => {
                        app.show_toast(lf!("{current} already has everything in {short_name}", "{current} 已包含 {short_name} 的全部提交"))
                    }
                    Ok((MergeOutcome::FastForward, moved)) => {
                        app.announce(lf!("Merged {short_name} into {current} · fast-forward", "已将 {short_name} 合并到 {current} · 快进"), moved, id)
                    }
                    Ok((MergeOutcome::Merged, moved)) => app.announce(lf!("Merged {short_name} into {current}", "已将 {short_name} 合并到 {current}"), moved, id),
                    Err(stop) => {
                        let retry: Option<AppCallback> = if autostash { None } else { Some(Box::new(move |app: &mut App| app.merge_ref(r, id, true))) };
                        app.report_stop(RepoOperation::Merge, stop, retry);
                    }
                }
            },
        );
    }

    /// Replays the current branch's own commits on top of `r` ("Rebase Current onto Selected"). Asks first when some
    /// of them were pushed: they would have to be force-pushed.
    pub fn rebase_onto(&mut self, r: GitRef, id: ProjectId, autostash: bool, confirmed: bool) {
        if !self.ready_to_rewrite(id) {
            return;
        }
        let Some(current) = self.project(id).and_then(|p| p.status.branch.clone()) else {
            self.show_toast(l("No branch is checked out (detached HEAD) · nothing to rebase", "当前不在任何分支上（游离 HEAD）· 无法变基"));
            return;
        };
        let Some(url) = self.project(id).map(|p| p.url.clone()) else { return };
        if !confirmed {
            let name = r.name.clone();
            self.jobs.spawn(
                move || {
                    let own = history_edits::count(&[&format!("{name}..HEAD")], &url);
                    let unpushed = history_edits::count(&[&format!("{name}..HEAD"), "--not", "--remotes"], &url);
                    own.saturating_sub(unpushed)
                },
                move |app: &mut App, pushed: usize| {
                    if pushed == 0 {
                        app.rebase_onto(r, id, autostash, true);
                        return;
                    }
                    let message = if pushed == 1 {
                        lf!(
                            "A commit of {current} that would be replayed is already on the remote. Afterwards it is a new commit, and {current} can only be pushed by force.",
                            "{current} 上要重放的提交中，有 1 个已经推送到远程。变基后它会变成新的提交，{current} 只能强制推送。"
                        )
                    } else {
                        lf!(
                            "{pushed} commits of {current} that would be replayed are already on the remote. Afterwards they are new commits, and {current} can only be pushed by force.",
                            "{current} 上要重放的提交中，有 {pushed} 个已经推送到远程。变基后它们会变成新的提交，{current} 只能强制推送。"
                        )
                    };
                    let request = DialogRequest::new(DialogStyle::Caution, Icon::Warning, l("Rebase pushed commits?", "变基已推送的提交？"), l("Rebase", "变基"))
                        .message(message)
                        .confirm(move |app| app.rebase_onto(r, id, autostash, true));
                    app.ask(request);
                },
            );
            return;
        }
        let gd = self.git_directory_of(id);
        let name = r.name.clone();
        let short_name = r.short_name.clone();
        self.run_activity(
            lf!("Rebasing onto {short_name}…", "正在变基到 {short_name}…"),
            false,
            id,
            move |dir, _| history_edits::rebase(&name, autostash, dir).map_err(|e| stop_of(e.message, gd.as_deref(), dir)),
            move |app, result| {
                app.after_branches_changed(id);
                match result {
                    Ok(moved) if moved.before == moved.after => {
                        app.show_toast(lf!("{current} is already on top of {short_name}", "{current} 已经在 {short_name} 之上"))
                    }
                    Ok(moved) => app.announce(lf!("Rebased {current} onto {short_name}", "已将 {current} 变基到 {short_name}"), moved, id),
                    Err(stop) => {
                        let retry: Option<AppCallback> =
                            if autostash { None } else { Some(Box::new(move |app: &mut App| app.rebase_onto(r, id, true, true))) };
                        app.report_stop(RepoOperation::Rebase, stop, retry);
                    }
                }
            },
        );
    }

    pub fn rename_branch(&mut self, r: GitRef, id: ProjectId) {
        if r.kind != RefKind::Local {
            return;
        }
        let Some(url) = self.project(id).map(|p| p.url.clone()) else { return };
        let full = r.name.clone();
        self.jobs.spawn(
            move || {
                history::refs(&url)
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|x| x.kind == RefKind::Local && x.name != full)
                    .map(|x| x.short_name)
                    .collect::<HashSet<_>>()
            },
            move |app: &mut App, existing: HashSet<String>| {
                let name = r.short_name.clone();
                let message = match &r.upstream {
                    Some(u) => lf!("It goes on following {u}; the remote branch keeps its name.", "它仍然跟踪 {u}；远程分支的名称不变。"),
                    None => String::new(),
                };
                let old = name.clone();
                let request = DialogRequest::new(DialogStyle::Info, Icon::Edit, lf!("Rename {name}", "重命名 {name}"), l("Rename", "重命名"))
                    .message(message)
                    .with_input(l("Branch name", "分支名"), &name, move |typed| {
                        if typed == old {
                            None
                        } else {
                            remote::branch_name_problem(typed, &existing)
                        }
                    })
                    .answer(move |app, answer| {
                        if answer.text != name {
                            app.rename_branch_to(name, answer.text, id, true);
                        }
                    });
                app.ask(request);
            },
        );
    }

    fn rename_branch_to(&mut self, name: String, new_name: String, id: ProjectId, undoable: bool) {
        let (a, b) = (name.clone(), new_name.clone());
        self.run_activity(
            lf!("Renaming {name}…", "正在重命名 {name}…"),
            false,
            id,
            move |dir, _| history_edits::rename_branch(&a, &b, dir),
            move |app, result| {
                app.after_branches_changed(id);
                match result {
                    Ok(()) => {
                        let text = lf!("Renamed {name} to {new_name}", "已将 {name} 重命名为 {new_name}");
                        if undoable {
                            app.show_toast_action(text, l("Undo", "撤销"), move |app| app.rename_branch_to(new_name, name, id, false));
                        } else {
                            app.show_toast(text);
                        }
                    }
                    Err(e) => app.show_failure(lf!("Could not rename {name}", "无法重命名 {name}"), &e, None),
                }
            },
        );
    }

    /// Deletes a local branch; Undo in the toast brings it back. One whose commits are in no other branch is only
    /// deleted once confirmed.
    pub fn delete_branch(&mut self, r: GitRef, id: ProjectId, force: bool) {
        if r.kind != RefKind::Local {
            return;
        }
        let name = r.short_name.clone();
        if r.is_head {
            self.show_toast(lf!("{name} is checked out · check out another branch first", "{name} 正处于检出状态 · 请先检出别的分支"));
            return;
        }
        enum Outcome {
            Deleted,
            Unmerged(Vec<Commit>),
            Failed(String),
        }
        let (n, target) = (name.clone(), r.target.clone());
        self.run_activity(
            lf!("Deleting {name}…", "正在删除 {name}…"),
            false,
            id,
            move |dir, _| match history_edits::delete_branch(&n, force, dir) {
                Ok(()) => Outcome::Deleted,
                Err(e) if !force && history_edits::is_not_fully_merged(&e.message) => {
                    // Its own commits: on it, and on no other branch.
                    let exclude = format!("--exclude={n}");
                    Outcome::Unmerged(history_edits::commits(&[&target, "--not", &exclude, "--branches", "--remotes"], dir, 200))
                }
                Err(e) => Outcome::Failed(e.message),
            },
            move |app, outcome| match outcome {
                Outcome::Deleted => {
                    app.after_branches_changed(id);
                    app.show_toast_action(lf!("Deleted {name}", "已删除 {name}"), l("Undo", "撤销"), move |app| app.restore_branch(r, id));
                }
                Outcome::Failed(message) => app.show_failure(lf!("Could not delete {name}", "无法删除 {name}"), &message, None),
                Outcome::Unmerged(own) => {
                    let message = if own.is_empty() {
                        let into = r.upstream.clone();
                        match into {
                            Some(u) => lf!("Its commits are not in {u} yet. Delete it anyway?", "它的提交还不在 {u} 里。仍然删除？"),
                            None => l("Its commits are not in the current branch yet. Delete it anyway?", "它的提交还不在当前分支里。仍然删除？"),
                        }
                    } else {
                        l(
                            "These commits are on no other branch. Delete it anyway? Undo in the toast brings it back.",
                            "这些提交不在任何其他分支上。仍然删除？可以用提示里的“撤销”恢复。",
                        )
                    };
                    let mut request =
                        DialogRequest::new(DialogStyle::Destructive, Icon::Delete, lf!("{name} is not fully merged", "{name} 还没有完全合并"), l("Delete Anyway", "仍然删除"))
                            .message(message);
                    (request.items, request.hidden_count) = commit_items(&own, 5, Icon::Circle);
                    request.on_confirm = Some(Box::new(move |app: &mut App| app.delete_branch(r, id, true)));
                    app.ask(request);
                }
            },
        );
    }

    fn restore_branch(&mut self, r: GitRef, id: ProjectId) {
        let name = r.short_name.clone();
        let (n, target, upstream) = (name.clone(), r.target.clone(), r.upstream.clone());
        self.run_activity(
            lf!("Restoring {name}…", "正在恢复 {name}…"),
            false,
            id,
            move |dir, _| history_edits::restore_branch(&n, &target, upstream.as_deref(), dir),
            move |app, result| {
                app.after_branches_changed(id);
                match result {
                    Ok(()) => app.show_toast(lf!("Restored {name}", "已恢复 {name}")),
                    Err(e) => app.show_failure(lf!("Could not restore {name}", "无法恢复 {name}"), &e, None),
                }
            },
        );
    }

    /// Deletes a branch on its remote, for everyone, once confirmed. Undo in the toast pushes it back.
    pub fn delete_remote_branch(&mut self, r: GitRef, id: ProjectId) {
        if r.kind != RefKind::Remote {
            return;
        }
        let Some(url) = self.project(id).map(|p| p.url.clone()) else { return };
        self.jobs.spawn(
            move || remote::remotes(&url),
            move |app: &mut App, remotes: Vec<String>| {
                let Some((remote_name, branch)) = history_edits::split_remote_branch(&r.short_name, &remotes) else { return };
                let short_name = r.short_name.clone();
                if PROTECTED_BRANCHES.contains(&branch.as_str()) {
                    app.show_toast(lf!("DiffCode doesn't delete {short_name}", "DiffCode 不会删除 {short_name}"));
                    return;
                }
                let target = r.target.clone();
                let request = DialogRequest::new(
                    DialogStyle::Destructive,
                    Icon::Delete,
                    lf!("Delete {branch} from {remote_name}?", "从 {remote_name} 删除 {branch}？"),
                    l("Delete", "删除"),
                )
                .message(l(
                    "The branch goes away on the server, for everyone who uses it. Local branches stay.",
                    "服务器上的这个分支会被删除，所有使用它的人都会受影响。本地分支保留。",
                ))
                .confirm(move |app| {
                    let (b, rm) = (branch.clone(), remote_name.clone());
                    app.run_activity(
                        lf!("Deleting {short_name}…", "正在删除 {short_name}…"),
                        true,
                        id,
                        move |dir, _| history_edits::delete_remote_branch(&b, &rm, dir),
                        move |app, result| {
                            app.after_branches_changed(id);
                            match result {
                                Ok(()) => app.show_toast_action(lf!("Deleted {short_name}", "已删除 {short_name}"), l("Undo", "撤销"), move |app| {
                                    app.restore_remote_branch(branch, remote_name, target, id)
                                }),
                                Err(e) => {
                                    let note = App::remote_note(&e.message);
                                    app.show_failure(lf!("Could not delete {short_name}", "无法删除 {short_name}"), &e, note);
                                }
                            }
                        },
                    );
                });
                app.ask(request);
            },
        );
    }

    fn restore_remote_branch(&mut self, branch: String, remote_name: String, hash: String, id: ProjectId) {
        let (b, r) = (branch.clone(), remote_name.clone());
        self.run_activity(
            lf!("Pushing {branch} back…", "正在推回 {branch}…"),
            true,
            id,
            move |dir, _| history_edits::restore_remote_branch(&b, &r, &hash, dir),
            move |app, result| {
                app.after_branches_changed(id);
                match result {
                    Ok(()) => app.show_toast(lf!("Restored {remote_name}/{branch}", "已恢复 {remote_name}/{branch}")),
                    Err(e) => {
                        let note = App::remote_note(&e.message);
                        app.show_failure(lf!("Could not restore {remote_name}/{branch}", "无法恢复 {remote_name}/{branch}"), &e, note);
                    }
                }
            },
        );
    }

    // MARK: Menus

    /// Which of `list` HEAD has: HEAD and its ancestors among them, found through their parents. None when HEAD is
    /// not in the list, and so nothing can be told.
    pub fn commits_in_head(list: &[Commit], head: Option<&str>) -> Option<HashSet<String>> {
        let head = head?;
        let mut by_hash: HashMap<&str, &Commit> = HashMap::new();
        for c in list {
            by_hash.entry(c.hash.as_str()).or_insert(c);
        }
        by_hash.get(head)?;
        let mut seen = HashSet::new();
        let mut stack = vec![head.to_string()];
        while let Some(hash) = stack.pop() {
            let Some(commit) = by_hash.get(hash.as_str()) else { continue };
            if seen.insert(hash) {
                stack.extend(commit.parents.iter().cloned());
            }
        }
        Some(seen)
    }

    /// Cherry-pick, Revert and Reset for a commit's right-click menu (the Log's and the Git graph's).
    /// `commits`: what the menu acts on (`menu_commits`), newest first; reset takes the clicked one.
    /// `list`: the commits on screen, to tell which the current branch has (`commits_in_head`).
    pub fn add_history_edit_items(&self, items: &mut Vec<MenuItem>, commit: &Commit, commits: Vec<Commit>, list: &[Commit], id: ProjectId) {
        let Some(p) = self.project(id) else { return };
        let several = commits.len() > 1;
        let head = p.status.head_oid.clone();
        let in_head = App::commits_in_head(list, head.as_deref());
        // Picking brings in what the branch does not have; reverting takes out what it has.
        let pickable: Vec<Commit> = commits.iter().filter(|c| !c.is_merge() && in_head.as_ref().is_none_or(|s| !s.contains(&c.hash))).cloned().collect();
        let revertible: Vec<Commit> = commits.iter().filter(|c| !c.is_merge() && in_head.as_ref().is_none_or(|s| s.contains(&c.hash))).cloned().collect();
        let (np, nr) = (pickable.len(), revertible.len());
        let pick_title = if several { lf!("Cherry-Pick {np} Commits", "拣选 {np} 个提交 (Cherry-Pick)") } else { l("Cherry-Pick", "拣选到当前分支 (Cherry-Pick)") };
        let revert_title = if several { lf!("Revert {nr} Commits", "还原 {nr} 个提交 (Revert)") } else { l("Revert Commit", "还原提交 (Revert)") };
        items.push(MenuItem::new(pick_title, move |app| app.cherry_pick(pickable, id)).disabled(np == 0));
        items.push(MenuItem::new(revert_title, move |app| app.revert(revertible, id)).disabled(nr == 0));
        let name = p.status.branch.clone().unwrap_or("HEAD".into());
        let is_head = head.as_deref() == Some(commit.hash.as_str());
        let c = commit.clone();
        items.push(MenuItem::new(lf!("Reset {name} to Here…", "将 {name} 重置到这里…"), move |app| app.reset_current_branch(c, id)).disabled(several || is_head));
    }

    /// Merge, Rebase, Rename and Delete for a branch's right-click menu in the Git graph.
    pub fn add_branch_edit_items(&self, items: &mut Vec<MenuItem>, r: &GitRef, id: ProjectId) {
        let Some(p) = self.project(id) else { return };
        let short_name = r.short_name.clone();
        if let Some(current) = p.status.branch.clone() {
            if r.kind != RefKind::Tag && !(r.kind == RefKind::Local && r.is_head) {
                let (a, b) = (r.clone(), r.clone());
                items.push(MenuItem::new(lf!("Merge {short_name} into {current}", "将 {short_name} 合并到 {current}"), move |app| app.merge_ref(a, id, false)));
                items.push(MenuItem::new(lf!("Rebase {current} onto {short_name}", "将 {current} 变基到 {short_name}"), move |app| {
                    app.rebase_onto(b, id, false, false)
                }));
                items.push(MenuItem::separator());
            }
        }
        match r.kind {
            RefKind::Local => {
                let (a, b) = (r.clone(), r.clone());
                items.push(MenuItem::new(l("Rename…", "重命名…"), move |app| app.rename_branch(a, id)));
                items.push(MenuItem::new(l("Delete", "删除"), move |app| app.delete_branch(b, id, false)).disabled(r.is_head));
                items.push(MenuItem::separator());
            }
            RefKind::Remote => {
                let (remote_name, branch) = r.short_name.split_once('/').map(|(a, b)| (a.to_string(), b.to_string())).unwrap_or(("origin".into(), String::new()));
                let blocked = PROTECTED_BRANCHES.contains(&branch.as_str()) || branch == "HEAD";
                let a = r.clone();
                items.push(MenuItem::new(lf!("Delete from {remote_name}…", "从 {remote_name} 删除…"), move |app| app.delete_remote_branch(a, id)).disabled(blocked));
                items.push(MenuItem::separator());
            }
            RefKind::Tag => {}
        }
    }
}
