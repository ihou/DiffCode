//! The stash, as the IDE's Stash Changes and Unstash Changes: put the local changes aside, look at what a stash holds,
//! bring it back (keeping it, or dropping it once applied), or drop it.

use super::dialog::{DialogOption, DialogRequest, DialogStyle};
use super::project::{Comparison, ComparisonEnd, ProjectId};
use super::App;
use crate::core::git::{self, ChangeKind};
use crate::core::history_edits::{self, StashEntry};
use crate::core::lang::l;
use crate::core::lf;
use crate::icons::Icon;

impl App {
    pub fn stash_changes(&mut self) {
        if let Some(id) = self.selected {
            self.stash_changes_in(id);
        }
    }

    /// Stash Changes…: asks for an optional message, and whether new files go too.
    pub fn stash_changes_in(&mut self, id: ProjectId) {
        let Some(p) = self.project(id) else { return };
        if let Some(operation) = p.status.operation {
            let command = operation.command();
            self.show_toast(lf!("Finish or abort the {command} first", "请先完成或中止当前的 {command}"));
            return;
        }
        if p.changes().is_empty() {
            self.show_toast(l("No local changes to stash", "没有可暂存的本地改动"));
            return;
        }
        let untracked = p.changes().iter().filter(|c| c.kind == ChangeKind::Untracked).count();
        let total = p.changes().len();
        let head = p.status.branch.clone().unwrap_or_else(|| p.status.short_head());
        let changes = p.changes().to_vec();
        if !self.viewer.save_edits() {
            return;
        }
        let mut request = DialogRequest::new(DialogStyle::Info, Icon::Archive, l("Stash local changes", "Stash 本地改动"), l("Stash", "暂存 (Stash)"))
            .message(lf!(
                "They are put aside, and the files go back to how {head} has them. Bring them back from the stash list (Git ▸ Stashes…).",
                "改动会被放到一边，文件回到 {head} 中的样子。之后可以从 Stash 列表（Git ▸ Stash 列表…）恢复。"
            ))
            .with_files(&changes, 5)
            .with_input(l("Message (optional)", "说明（可选）"), "", |_| None)
            .answer(move |app, answer| app.stash_push(answer.text, answer.option, id));
        if untracked > 0 {
            // With nothing else changed, the new files are all there is to stash.
            let title = if untracked == 1 { l("Include 1 new file", "包括 1 个新文件（未跟踪）") } else { lf!("Include {untracked} new files", "包括 {untracked} 个新文件（未跟踪）") };
            request.option = Some(DialogOption { title, is_on: untracked == total });
        }
        self.ask(request);
    }

    fn stash_push(&mut self, message: String, include_untracked: bool, id: ProjectId) {
        if !self.viewer.save_edits() {
            return;
        }
        self.run_activity(
            l("Stashing…", "正在暂存…"),
            false,
            id,
            move |dir, _| history_edits::stash_push(&message, include_untracked, dir),
            move |app, result| {
                app.after_stash_changed(id);
                match result {
                    Ok(Some(entry)) => {
                        let title = entry.title().to_string();
                        app.show_toast_action(lf!("Stashed · {title}", "已暂存 · {title}"), l("Undo", "撤销"), move |app| app.apply_stash(entry, true, id));
                    }
                    Ok(None) => app.show_toast(l("Nothing to stash · only new files changed", "没有可暂存的改动 · 只有新文件")),
                    Err(e) => app.show_failure(l("Could not stash", "无法暂存"), &e, None),
                }
            },
        );
    }

    /// The stash list, in Quick Open: Enter shows what a stash holds.
    pub fn show_stashes(&mut self) {
        if self.current_project().is_some() {
            self.show_quick_open(super::search::QuickOpenMode::Stashes);
        }
    }

    /// Lists what a stash holds in the Changes tab, against the commit it was made on. The bar over the list applies,
    /// pops or drops it.
    pub fn show_stash(&mut self, entry: StashEntry) {
        let Some(id) = self.selected else { return };
        let comparison = Comparison {
            target: entry.base.clone(),
            title: entry.name(),
            fork_point: false,
            base: entry.base.clone(),
            other: Some(ComparisonEnd { target: entry.hash.clone(), title: entry.name(), hash: entry.hash.clone() }),
            stash: Some(entry),
        };
        self.start_comparison(comparison, id);
    }

    /// Brings a stash's changes back into the working tree; with `drop`, it leaves the list once they are in (Pop).
    pub fn apply_stash(&mut self, entry: StashEntry, drop: bool, id: ProjectId) {
        let Some(p) = self.project(id) else { return };
        if let Some(operation) = p.status.operation {
            let command = operation.command();
            self.show_toast(lf!("Finish or abort the {command} first", "请先完成或中止当前的 {command}"));
            return;
        }
        if !self.viewer.save_edits() {
            return;
        }
        let e = entry.clone();
        self.run_activity(
            if drop { l("Popping the stash…", "正在弹出 stash…") } else { l("Applying the stash…", "正在应用 stash…") },
            false,
            id,
            move |dir, _| {
                history_edits::stash_apply(&e, drop, dir).map_err(|err| {
                    let clashes = git::run_string(dir, ["diff", "--name-only", "--diff-filter=U"]).unwrap_or_default();
                    (err.message, !clashes.trim().is_empty())
                })
            },
            move |app, result| {
                app.after_stash_changed(id);
                let title = entry.title().to_string();
                match result {
                    Ok(()) => {
                        app.leave_stash(&entry, id);
                        app.show_toast(if drop {
                            lf!("Popped · {title}", "已弹出 · {title}")
                        } else {
                            lf!("Applied · {title} stays in the stash list", "已应用 · {title} 仍保留在 Stash 列表中")
                        });
                    }
                    Err((_, true)) => {
                        app.leave_stash(&entry, id);
                        app.show_conflicts_notice(
                            l("The stash clashes with your files", "这个 stash 与你的文件有冲突"),
                            Some(l("The stash stays in the list until you drop it.", "这个 stash 会留在列表里，直到你删除它。")),
                        );
                    }
                    Err((message, false)) if history_edits::refused_for_local_changes(&message) => app.show_failure(
                        l("Your local changes are in the way", "本地改动挡住了 stash"),
                        &message,
                        Some(l("Commit or stash them first, then apply this one.", "请先提交或暂存它们，再应用这个 stash。")),
                    ),
                    Err((message, false)) => app.show_failure(l("Could not apply the stash", "无法应用 stash"), &message, None),
                }
            },
        );
    }

    /// Drops a stash, once confirmed. Undo in the toast lists it again.
    pub fn drop_stash(&mut self, entry: StashEntry, id: ProjectId) {
        let title = entry.title().to_string();
        let request = DialogRequest::new(DialogStyle::Destructive, Icon::Delete, lf!("Drop “{title}”?", "删除“{title}”？"), l("Drop", "删除"))
            .message(l(
                "It leaves the stash list, with its changes. Undo in the toast brings it back.",
                "它会连同改动一起从 Stash 列表中删除。可以用提示里的“撤销”恢复。",
            ))
            .confirm(move |app| {
                let e = entry.clone();
                app.run_activity(
                    l("Dropping the stash…", "正在删除 stash…"),
                    false,
                    id,
                    move |dir, _| history_edits::stash_drop(&e, dir),
                    move |app, result| match result {
                        Ok(()) => {
                            app.leave_stash(&entry, id);
                            app.show_toast_action(lf!("Dropped · {title}", "已删除 · {title}"), l("Undo", "撤销"), move |app| app.restore_stash(entry, id));
                        }
                        Err(e) => app.show_failure(l("Could not drop the stash", "无法删除 stash"), &e, None),
                    },
                );
            });
        self.ask(request);
    }

    fn restore_stash(&mut self, entry: StashEntry, id: ProjectId) {
        let title = entry.title().to_string();
        self.run_activity(
            l("Restoring the stash…", "正在恢复 stash…"),
            false,
            id,
            move |dir, _| history_edits::stash_restore(&entry, dir),
            move |app, result| match result {
                Ok(()) => app.show_toast(lf!("Restored · {title}", "已恢复 · {title}")),
                Err(e) => app.show_failure(l("Could not restore the stash", "无法恢复 stash"), &e, None),
            },
        );
    }

    /// The list stops showing `entry` (it was applied or dropped): back to the local changes.
    fn leave_stash(&mut self, entry: &StashEntry, id: ProjectId) {
        let showing = self.project(id).and_then(|p| p.comparison.as_ref()).and_then(|c| c.stash.as_ref()).is_some_and(|s| s.hash == entry.hash);
        if showing {
            self.end_comparison_in(id);
        }
    }

    /// Files came and went: the lists and the tree follow.
    fn after_stash_changed(&mut self, id: ProjectId) {
        if let Some(p) = self.project_mut(id) {
            p.invalidate_files();
            p.refresh();
        }
    }
}
