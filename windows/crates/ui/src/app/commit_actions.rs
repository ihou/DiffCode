//! Committing from the bottom of the Changes list, and taking the last commit back.

use std::collections::HashSet;

use super::project::{ProjectId, ProjectTab};
use super::{ids, App};
use crate::canvas::Inclusion;
use crate::core::commit::{self, CommitResult, PartialFile};
use crate::core::git::{self, FileChange};
use crate::core::history_edits;
use crate::core::lang::l;
use crate::core::lf;
use crate::core::remote::RepoOperation;

impl App {
    /// Ctrl+K, the IDE's Commit: puts the keyboard in the commit message. In a terminal it clears the screen instead.
    pub fn commit_shortcut(&mut self) {
        if self.terminal_has_focus() {
            self.clear_terminal();
        } else {
            self.focus_commit_message();
        }
    }

    pub fn focus_commit_message(&mut self) {
        let Some(id) = self.selected else { return };
        self.list_visible = true;
        let p = self.project_mut(id).expect("project");
        if p.tab != ProjectTab::Changes {
            p.tab = ProjectTab::Changes;
            p.browsed_path = None;
            self.sync_viewer();
        }
        let p = self.project(id).expect("project");
        if p.changes().is_empty() && !p.amend {
            self.show_toast(l("Nothing to commit", "没有可提交的改动"));
            return;
        }
        self.focus_request = Some(ids::commit_message());
    }

    /// The Amend box under the commit message: the next commit replaces the last one, and its message fills the box
    /// (what was typed there comes back when Amend goes off again, if the message was left as it was).
    pub fn set_amend(&mut self, on: bool, id: ProjectId) {
        let Some(p) = self.project_mut(id) else { return };
        if on == p.amend {
            return;
        }
        if !on {
            p.amend = false;
            p.amend_pushed_to.clear();
            if let Some((before, filled)) = p.amend_draft.take() {
                if p.commit_message.text == filled {
                    p.commit_message.set_text(&before);
                }
            }
            return;
        }
        if let Some(operation) = p.status.operation {
            let command = operation.command();
            self.show_toast(lf!("A {command} is in progress · finish it first", "正在进行 {command} · 请先完成它"));
            return;
        }
        let url = p.url.clone();
        self.jobs.spawn(
            move || {
                let head = commit::head(&url).ok()?;
                let message = git::commit_body(&head, &url);
                let pushed = history_edits::remote_branches_containing(&head, &url);
                Some((message, pushed))
            },
            move |app: &mut App, result: Option<(String, Vec<String>)>| {
                let Some((message, pushed)) = result else {
                    app.show_toast(l("There is no commit to amend yet", "还没有可以修改的提交"));
                    return;
                };
                let Some(p) = app.project_mut(id) else { return };
                if p.amend {
                    return;
                }
                p.amend = true;
                p.amend_pushed_to = pushed;
                p.amend_draft = Some((p.commit_message.text.clone(), message.clone()));
                p.commit_message.set_text(&message);
                if app.selected == Some(id) {
                    app.focus_commit_message();
                }
            },
        );
    }

    pub fn toggle_amend(&mut self) {
        let Some(id) = self.selected else { return };
        let on = !self.project(id).expect("project").amend;
        self.set_amend(on, id);
    }

    /// Commits the ticked files with the message typed below the list.
    /// `then_push`: Commit and Push: the push dialog follows a commit that went through.
    pub fn commit_current(&mut self, then_push: bool) {
        let Some(id) = self.selected else { return };
        let p = self.project(id).expect("project");
        if p.is_committing {
            return;
        }
        if let Some(operation) = p.status.operation {
            if operation != RepoOperation::Merge {
                let command = operation.command();
                self.show_toast(lf!(
                    "A {command} is in progress · Continue it above the list",
                    "正在进行 {command} · 请用列表上方的“继续”"
                ));
                return;
            }
            if p.status.conflict_count() > 0 {
                self.show_toast(l("Resolve the conflicts first", "请先解决冲突"));
                return;
            }
            self.commit_merge(id, then_push);
            return;
        }
        let message = p.commit_message.text.clone();
        let targets = p.included_changes();
        let amend = p.amend;
        if message.trim().is_empty() {
            self.show_toast(l("Write a commit message first", "先写提交说明"));
            self.focus_commit_message();
            return;
        }
        if targets.is_empty() && !amend {
            self.show_toast(l("Tick the files to commit", "先勾选要提交的文件"));
            return;
        }
        // What was just typed is part of what gets committed.
        if !self.viewer.save_edits() {
            return;
        }
        let p = self.project(id).expect("project");
        // Files with lines left out go in part; the rest as they are.
        let partial: Vec<PartialFile> = targets
            .iter()
            .filter_map(|c| {
                p.excluded_lines.get(&c.path).filter(|e| !e.is_empty()).map(|e| PartialFile { change: c.clone(), excluded: e.clone() })
            })
            .collect();
        let whole: Vec<FileChange> = targets.iter().filter(|c| p.excluded_lines.get(&c.path).is_none_or(|e| e.is_empty())).cloned().collect();
        let url = p.url.clone();
        self.project_mut(id).expect("project").is_committing = true;
        let partial_count = partial.len();
        let committed: HashSet<String> = targets.iter().map(|c| c.path.clone()).collect();
        let text = message.clone();
        self.jobs.spawn(
            move || {
                if partial.is_empty() {
                    commit::commit(&text, &targets, amend, &url)
                } else {
                    commit::commit_partial(&text, &whole, &partial, amend, &url)
                }
            },
            move |app: &mut App, result| {
                let Some(p) = app.project_mut(id) else { return };
                p.is_committing = false;
                let result: CommitResult = match result {
                    Ok(r) => r,
                    Err(e) => {
                        p.refresh();
                        app.show_failure(l("Commit failed", "提交失败"), &e, None);
                        return;
                    }
                };
                p.last_commit = Some(result.clone());
                let draft = p.amend_draft.clone();
                if amend {
                    p.amend = false;
                    p.amend_pushed_to.clear();
                    p.amend_draft = None;
                }
                // What stays out of a file was picked against the old HEAD; what is left of it is a change like any
                // other.
                p.excluded_lines.retain(|path, _| !committed.contains(path));
                // Only if nobody started a new message while git was at work. After an amend, what was being written
                // before comes back.
                if p.commit_message.text == message {
                    let next = if amend { draft.map(|d| d.0).unwrap_or_default() } else { String::new() };
                    p.commit_message.set_text(&next);
                }
                app.after_history_changed(id);
                let n = result.file_count;
                let files = if n == 1 { l("1 file", "1 个文件") } else { lf!("{n} files", "{n} 个文件") };
                let part = if partial_count == 0 { String::new() } else { lf!(", {partial_count} in part", "（{partial_count} 个部分提交）") };
                let hash = result.short_hash().to_string();
                if let Some(amended) = &result.amended {
                    let old: String = amended.chars().take(7).collect();
                    let added = if n == 0 { String::new() } else { lf!(" with {files}{part}", "，加入 {files}{part}") };
                    app.show_toast_action(
                        lf!("Amended {old}{added} · now {hash}", "已修改 {old}{added} · 现为 {hash}"),
                        l("Undo Amend", "撤销修改"),
                        |app| app.undo_last_commit(),
                    );
                } else {
                    app.show_toast_action(
                        lf!("Committed {files}{part} · {hash}", "已提交 {files}{part} · {hash}"),
                        l("Undo Commit", "撤销提交"),
                        |app| app.undo_last_commit(),
                    );
                }
                if then_push {
                    app.push_project(id);
                }
            },
        );
    }

    /// The commit that finishes a merge: the merge's staged result plus the ticked files, with the message in the box
    /// (git's prepared one, unless the user rewrote it).
    fn commit_merge(&mut self, id: ProjectId, then_push: bool) {
        let p = self.project(id).expect("project");
        let message = p.commit_message.text.clone();
        if message.trim().is_empty() {
            self.show_toast(l("Write a commit message first", "先写提交说明"));
            self.focus_commit_message();
            return;
        }
        if !self.viewer.save_edits() {
            return;
        }
        let p = self.project(id).expect("project");
        let targets = p.included_changes();
        let url = p.url.clone();
        self.project_mut(id).expect("project").is_committing = true;
        let text = message.clone();
        self.jobs.spawn(
            move || commit::commit_merge(&text, &targets, &url),
            move |app: &mut App, result| {
                let Some(p) = app.project_mut(id) else { return };
                p.is_committing = false;
                match result {
                    Ok(result) => {
                        // Undoing a merge commit would not bring the merge back, only its changes: not offered.
                        p.last_commit = None;
                        // Lines picked before the merge were picked against the old HEAD.
                        p.excluded_lines.clear();
                        if p.commit_message.text == message {
                            p.commit_message.set_text("");
                        }
                        app.after_history_changed(id);
                        let hash = result.short_hash().to_string();
                        app.show_toast(lf!("Merge committed · {hash}", "合并已提交 · {hash}"));
                        if then_push {
                            app.push_project(id);
                        }
                    }
                    Err(e) => {
                        p.refresh();
                        app.show_failure(l("Commit failed", "提交失败"), &e, None);
                    }
                }
            },
        );
    }

    pub fn can_undo_commit(&self) -> bool {
        self.current_project().is_some_and(|p| p.last_commit.is_some())
    }

    /// Moves the branch back past the commit made here; its changes come back, staged, and so does its message.
    pub fn undo_last_commit(&mut self) {
        let Some(id) = self.selected else { return };
        let p = self.project(id).expect("project");
        let Some(result) = p.last_commit.clone() else {
            self.show_toast(l("No commit made here to undo", "没有可撤销的提交"));
            return;
        };
        let url = p.url.clone();
        let r = result.clone();
        self.jobs.spawn(
            move || commit::undo_commit(&r, &url),
            move |app: &mut App, outcome| {
                let Some(p) = app.project_mut(id) else { return };
                p.last_commit = None;
                match outcome {
                    Ok(()) => {
                        if p.commit_message.text.trim().is_empty() {
                            p.commit_message.set_text(&result.message);
                        }
                        app.after_history_changed(id);
                        if let Some(amended) = &result.amended {
                            let old: String = amended.chars().take(7).collect();
                            app.show_toast(lf!(
                                "Amend undone · {old} is back; what it added is staged",
                                "已撤销修改 · {old} 已恢复，新加入的改动在暂存区"
                            ));
                        } else {
                            app.show_toast(l("Commit undone · the changes are back in the list", "已撤销提交 · 改动已放回列表"));
                        }
                    }
                    Err(e) => app.show_failure(l("Could not undo the commit", "无法撤销提交"), &e, None),
                }
            },
        );
    }

    pub fn after_history_changed(&mut self, id: ProjectId) {
        let Some(p) = self.project_mut(id) else { return };
        p.invalidate_files();
        p.log_stale = true;
        p.refresh();
        if p.tab == ProjectTab::Log {
            p.load_log(true, 0, None);
        }
    }

    /// Space in the Changes list: ticks or unticks the selected files, all the same way.
    pub fn toggle_included_selection(&mut self, id: ProjectId) {
        let targets = self.selected_changes(id);
        if targets.is_empty() {
            return;
        }
        let Some(p) = self.project_mut(id) else { return };
        let all_on = targets.iter().all(|c| p.inclusion(c) == Inclusion::On);
        p.set_included(!all_on, &targets);
    }
}
