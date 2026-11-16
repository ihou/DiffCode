//! The few edits DiffCode makes itself: rolling changes back, moving new files to the Recycle Bin, and undoing either.

use std::collections::HashSet;

use super::change_tree::{self, ChangeRow};
use super::dialog::{DialogRequest, DialogStyle};
use super::project::{ProjectId, ProjectTab};
use super::{ids, App, UndoItem};
use crate::core::diff::Source;
use crate::core::git::{ChangeKind, FileChange};
use crate::core::lang::l;
use crate::core::lf;
use crate::core::util::file_name;
use crate::core::working_tree::{self, Failure, HunkResult, Record};
use crate::icons::Icon;

pub const UNDO_LIMIT: usize = 20;

pub fn can_rollback(change: &FileChange) -> bool {
    change.kind != ChangeKind::Untracked && !change.is_submodule
}

pub fn can_delete(change: &FileChange) -> bool {
    matches!(change.kind, ChangeKind::Untracked | ChangeKind::Added)
}

/// What an operation did. `failure` is set when it stopped part of the way: the record covers the part that was done.
struct Outcome {
    record: Record,
    summary: String,
    failure: Option<Failure>,
}

impl App {
    // MARK: What a command applies to

    /// The changes the list shows, i.e. with its filter applied.
    pub fn visible_changes(&self, id: ProjectId) -> Vec<FileChange> {
        let Some(p) = self.project(id) else { return Vec::new() };
        let needle = p.filter.text.trim().to_lowercase();
        if needle.is_empty() {
            return p.changes().to_vec();
        }
        p.changes().iter().filter(|c| c.path.to_lowercase().contains(&needle)).cloned().collect()
    }

    /// Every file selected in the Changes list; a selected folder or group stands for the files under it.
    pub fn selected_changes(&self, id: ProjectId) -> Vec<FileChange> {
        let Some(p) = self.project(id) else { return Vec::new() };
        let mut ids: HashSet<String> = p.selected_change_ids.clone();
        if let Some(primary) = &p.selected_change_id {
            ids.insert(primary.clone());
        }
        change_tree::files_for(&ids, &self.visible_changes(id))
    }

    /// Drags files of a project out of the window: those of `paths` (relative, with `/`) still on disk.
    pub fn drag_out(&mut self, id: ProjectId, paths: Vec<String>) {
        let Some(p) = self.project(id) else { return };
        let files: Vec<std::path::PathBuf> =
            paths.iter().map(|path| path.split('/').fold(p.url.clone(), |at, part| at.join(part))).filter(|f| f.exists()).collect();
        if !files.is_empty() {
            self.requests.push(super::PlatformRequest::DragFiles(files));
        }
    }

    /// Dragging Local Changes rows out: the whole selection when the row is part of it, else the row's files.
    pub fn drag_changes(&mut self, row_id: &str, id: ProjectId) {
        let paths = self.edit_targets(row_id, id).into_iter().filter(|c| c.kind != ChangeKind::Deleted).map(|c| c.path).collect();
        self.drag_out(id, paths);
    }

    /// What a row's context menu acts on: the whole selection when the row is part of it, otherwise just that row.
    pub fn edit_targets(&self, row_id: &str, id: ProjectId) -> Vec<FileChange> {
        let Some(p) = self.project(id) else { return Vec::new() };
        if p.selected_change_ids.len() > 1 && p.selected_change_ids.contains(row_id) {
            return self.selected_changes(id);
        }
        change_tree::files_for(&[row_id.to_string()].into_iter().collect(), &self.visible_changes(id))
    }

    /// The files a keyboard command should act on, going by where the keyboard is.
    fn context_changes(&self, id: ProjectId) -> Vec<FileChange> {
        if self.focus == Some(ids::canvas()) {
            return self.viewer_change(id).into_iter().collect();
        }
        let Some(p) = self.project(id) else { return Vec::new() };
        if self.focus == Some(ids::tree()) {
            let Some(path) = p.tree_selected_id.as_deref().and_then(|s| s.strip_prefix("f:")) else { return Vec::new() };
            return p.changes().iter().filter(|c| c.path == path).cloned().collect();
        }
        // While comparing with another commit, the list holds that comparison, not what rollback and delete act on.
        if p.tab == ProjectTab::Changes && p.comparison.is_none() {
            self.selected_changes(id)
        } else {
            Vec::new()
        }
    }

    // MARK: Commands

    /// Ctrl+Alt+Z, as in the IDE: in the diff it rolls back the change under the highlight, elsewhere the selected files.
    pub fn rollback_in_context(&mut self) {
        let Some(id) = self.selected else { return };
        if self.terminal_has_focus() {
            return;
        }
        if self.focus == Some(ids::canvas()) && self.viewer.can_revert_hunks() {
            if let Some(hunk) = self.viewer.focused_hunk() {
                self.rollback_hunk(hunk);
                return;
            }
        }
        let targets: Vec<FileChange> = self.context_changes(id).into_iter().filter(can_rollback).collect();
        if targets.is_empty() {
            self.show_toast(l("Select changed files to roll back", "先选中要回滚的变更文件"));
        } else {
            self.rollback(targets, id, true);
        }
    }

    /// Delete in the Changes list or the tree.
    pub fn delete_in_context(&mut self) {
        let Some(id) = self.selected else { return };
        if self.terminal_has_focus() {
            return;
        }
        let selected = self.context_changes(id);
        let targets: Vec<FileChange> = selected.iter().filter(|c| can_delete(c)).cloned().collect();
        if targets.is_empty() {
            self.show_toast(if selected.is_empty() {
                l("Select new files to delete", "先选中要删除的新文件")
            } else {
                l("Only new files can be deleted here · Ctrl+Alt+Z rolls changes back", "这里只能删除新增的文件 · 修改请用 Ctrl+Alt+Z 回滚")
            });
        } else {
            self.delete(targets, id, true);
        }
    }

    pub fn rollback_current_hunk(&mut self) {
        match self.viewer.focused_hunk() {
            Some(hunk) if self.viewer.can_revert_hunks() => self.rollback_hunk(hunk),
            _ => self.show_toast(l("No change is highlighted in the diff", "diff 里没有选中的变更")),
        }
    }

    // MARK: Rollback

    pub fn rollback(&mut self, changes: Vec<FileChange>, id: ProjectId, confirm: bool) {
        let targets: Vec<FileChange> = changes.into_iter().filter(can_rollback).collect();
        if targets.is_empty() {
            return;
        }
        let run = {
            let targets = targets.clone();
            move |app: &mut App| {
                // Typing not yet written goes to the file first, so the backup Ctrl+Z restores from has it.
                if !app.viewer.save_edits() {
                    return;
                }
                let leaving: Vec<FileChange> = targets.iter().filter(|c| c.kind != ChangeKind::Added).cloned().collect();
                app.move_selection(&leaving, id);
                let Some(root) = app.project(id).map(|p| p.url.clone()) else { return };
                app.perform(id, l("Rollback failed", "回滚失败"), move || {
                    let (record, failure) = working_tree::rollback(&targets, &root)?;
                    let summary = if targets.len() == 1 {
                        let name = targets[0].name();
                        lf!("Rolled back {name}", "已回滚 {name}")
                    } else {
                        let n = targets.len();
                        lf!("Rolled back {n} files", "已回滚 {n} 个文件")
                    };
                    Ok(Outcome { record, summary, failure })
                });
            }
        };
        if !confirm {
            run(self);
            return;
        }
        let mut message = if targets.len() == 1 {
            l("Its local changes will be discarded. Ctrl+Z brings them back.", "本地修改会被丢弃，可以用 Ctrl+Z 撤销。")
        } else {
            l("Their local changes will be discarded. Ctrl+Z brings them back.", "这些文件的本地修改会被丢弃，可以用 Ctrl+Z 撤销。")
        };
        if targets.iter().any(|c| c.kind == ChangeKind::Added) {
            message.push(' ');
            message.push_str(&l("Newly added files stay on disk, unversioned.", "新增的文件会保留在磁盘上，变为未跟踪。"));
        }
        let title = if targets.len() == 1 {
            let name = targets[0].name();
            lf!("Roll back “{name}”?", "回滚“{name}”？")
        } else {
            let n = targets.len();
            lf!("Roll back {n} files?", "回滚 {n} 个文件？")
        };
        self.ask(
            DialogRequest::new(DialogStyle::Destructive, Icon::ArrowUndo, title, l("Roll Back", "回滚"))
                .message(message)
                .with_files(&targets, 8)
                .confirm(run),
        );
    }

    /// Puts one change of the diff on screen back the way the left side has it. No question asked: Ctrl+Z undoes it.
    pub fn rollback_hunk(&mut self, index: usize) {
        let Some(id) = self.selected else { return };
        let Some(url) = self.project(id).map(|p| p.url.clone()) else { return };
        let Some(request) = self.viewer.request.clone().filter(|r| r.root == url) else { return };
        if !self.viewer.can_revert_hunks() || self.is_applying_edit {
            return;
        }
        let Some(document) = self.viewer.document() else { return };
        let Some(hunk) = document.hunks().get(index).cloned() else { return };
        let name = file_name(&request.path).to_string();
        if self.viewer.can_edit() {
            // In the text being edited: an edit like any other, written at once, and Ctrl+Z in the diff takes it back.
            if !self.viewer.revert_hunk_in_editor(index) {
                return;
            }
            let at_head = matches!(&request.source, Source::Worktree { base } if base == "HEAD");
            if self.viewer.text_matches_base() && request.old_path.is_none() && at_head {
                // With the last change gone the file is HEAD's again; drop anything still staged for it, so it leaves
                // the list.
                let (root, path) = (request.root.clone(), request.path.clone());
                self.jobs.spawn(
                    move || {
                        let _ = working_tree::git_paths(&["checkout", "HEAD"], &[path], &root, &[]);
                    },
                    move |app: &mut App, ()| {
                        if let Some(p) = app.project_mut(id) {
                            p.refresh();
                        }
                    },
                );
            } else if let Some(p) = self.project_mut(id) {
                p.refresh();
            }
            self.focus_viewer();
            self.show_toast_action(lf!("Rolled back a change in {name}", "已回滚 {name} 里的一处变更"), l("Undo", "撤销"), |app| {
                app.viewer.undo_in_editor()
            });
            return;
        }
        let left = document.left.lines().to_vec();
        let right = document.right.lines().to_vec();
        self.is_applying_edit = true;
        self.jobs.spawn(
            move || working_tree::revert_hunk(&request, &hunk, &left, &right),
            move |app: &mut App, result| {
                app.is_applying_edit = false;
                match result {
                    Ok(HunkResult::Reverted(record)) => {
                        app.push_undo(UndoItem { record, project: id, summary: lf!("Rolled back a change in {name}", "已回滚 {name} 里的一处变更") });
                        app.viewer.reload();
                        if let Some(p) = app.project_mut(id) {
                            p.refresh();
                        }
                        app.show_undo_toast();
                    }
                    Ok(HunkResult::Stale) => {
                        app.viewer.reload();
                        app.show_toast(l("The file changed on disk · reloaded, nothing rolled back", "文件已在磁盘上变化 · 已重新加载，未做回滚"));
                    }
                    Err(e) => app.show_failure(l("Rollback failed", "回滚失败"), &e, None),
                }
            },
        );
    }

    // MARK: Delete

    pub fn delete(&mut self, changes: Vec<FileChange>, id: ProjectId, confirm: bool) {
        let targets: Vec<FileChange> = changes.into_iter().filter(can_delete).collect();
        if targets.is_empty() {
            return;
        }
        let run = {
            let targets = targets.clone();
            move |app: &mut App| {
                if !app.viewer.save_edits() {
                    return;
                }
                app.move_selection(&targets, id);
                let Some(root) = app.project(id).map(|p| p.url.clone()) else { return };
                app.perform(id, l("Could not delete", "删除失败"), move || {
                    let (record, failed) = working_tree::trash(&targets, &root)?;
                    if record.entries.is_empty() {
                        if let Some(first) = failed.first() {
                            let name = file_name(first);
                            return Err(Failure::new(lf!("“{name}” could not be moved to the Recycle Bin.", "“{name}”无法移到回收站。")));
                        }
                    }
                    let moved = record.entries.len();
                    let mut summary = if moved == 1 {
                        let name = file_name(&record.entries[0].path);
                        lf!("Moved {name} to the Recycle Bin", "已把 {name} 移到回收站")
                    } else {
                        lf!("Moved {moved} files to the Recycle Bin", "已把 {moved} 个文件移到回收站")
                    };
                    if !failed.is_empty() {
                        let n = failed.len();
                        summary.push_str(&lf!(" · {n} could not be moved", " · {n} 个未能移动"));
                    }
                    Ok(Outcome { record, summary, failure: None })
                });
            }
        };
        if !confirm {
            run(self);
            return;
        }
        let title = if targets.len() == 1 {
            let name = targets[0].name();
            lf!("Move “{name}” to the Recycle Bin?", "把“{name}”移到回收站？")
        } else {
            let n = targets.len();
            lf!("Move {n} files to the Recycle Bin?", "把 {n} 个文件移到回收站？")
        };
        self.ask(
            DialogRequest::new(DialogStyle::Destructive, Icon::Delete, title, l("Move to Recycle Bin", "移到回收站"))
                .message(l(
                    "New files only. They can be taken out of the Recycle Bin, or brought back with Ctrl+Z.",
                    "只会移动新增的文件。可以从回收站里找回，也可以用 Ctrl+Z 撤销。",
                ))
                .with_files(&targets, 8)
                .confirm(run),
        );
    }

    // MARK: Undo

    pub fn can_undo_edit(&self) -> bool {
        !self.undo_stack.is_empty() && !self.is_applying_edit
    }

    /// What Ctrl+Z would take back, for the menu.
    pub fn undo_edit_title(&self) -> Option<&str> {
        self.undo_stack.last().map(|i| i.summary.as_str())
    }

    pub fn undo_edit(&mut self) {
        if self.is_applying_edit || self.undo_stack.is_empty() {
            self.show_toast(l("Nothing to undo", "没有可撤销的操作"));
            return;
        }
        // The undo puts files back only if nothing changed them since; typing on screen counts once written.
        if !self.viewer.save_edits() {
            return;
        }
        let Some(item) = self.undo_stack.pop() else { return };
        self.is_applying_edit = true;
        let record = item.record.clone();
        self.jobs.spawn(
            move || working_tree::undo(&record),
            move |app: &mut App, (restored, skipped): (Vec<(String, Option<working_tree::Stamp>)>, usize)| {
                // An earlier operation on the same file expects to find what it left behind; it is back, with a new
                // timestamp.
                for (path, stamp) in &restored {
                    let found = app
                        .undo_stack
                        .iter()
                        .rposition(|other| other.record.root == item.record.root && other.record.entries.iter().any(|e| &e.path == path));
                    if let Some(index) = found {
                        for entry in app.undo_stack[index].record.entries.iter_mut().filter(|e| &e.path == path) {
                            entry.after = *stamp;
                        }
                    }
                }
                item.record.discard();
                app.is_applying_edit = false;
                if let Some(p) = app.project_mut(item.project) {
                    p.invalidate_files();
                    p.refresh();
                }
                app.viewer.reload();
                let summary = &item.summary;
                if skipped == 0 {
                    app.show_toast(lf!("Undone · {summary}", "已撤销 · {summary}"));
                } else if restored.is_empty() {
                    app.show_toast(l("Not undone: the files have changed since", "未撤销：这些文件之后又被改过"));
                } else {
                    app.show_toast(lf!(
                        "Undone, except {skipped} files that have changed since",
                        "已撤销，但有 {skipped} 个文件之后又被改过，保持原样"
                    ));
                }
            },
        );
    }

    /// Ctrl+Z outside the editor: the last rollback or delete.
    pub fn undo_in_context(&mut self) {
        if self.terminal_has_focus() {
            return;
        }
        self.undo_edit();
    }

    fn push_undo(&mut self, item: UndoItem) {
        self.undo_stack.push(item);
        while self.undo_stack.len() > UNDO_LIMIT {
            self.undo_stack.remove(0).record.discard();
        }
    }

    fn show_undo_toast(&mut self) {
        let Some(item) = self.undo_stack.last() else { return };
        let summary = item.summary.clone();
        self.show_toast_action(summary, l("Undo", "撤销"), |app| app.undo_edit());
    }

    // MARK: Plumbing

    /// Runs one operation at a time, then refreshes the project and offers to undo.
    fn perform(&mut self, id: ProjectId, failure: String, work: impl FnOnce() -> working_tree::Result<Outcome> + Send + 'static) {
        if self.is_applying_edit {
            return;
        }
        self.is_applying_edit = true;
        self.jobs.spawn(work, move |app: &mut App, result| {
            app.is_applying_edit = false;
            match result {
                Ok(outcome) => {
                    app.push_undo(UndoItem { record: outcome.record, project: id, summary: outcome.summary });
                    if let Some(p) = app.project_mut(id) {
                        p.invalidate_files();
                        p.refresh();
                    }
                    if let Some(error) = outcome.failure {
                        app.show_failure(
                            failure,
                            &error,
                            Some(l("Part of it was done. Ctrl+Z takes that part back.", "有一部分已经完成，按 Ctrl+Z 可以撤销这一部分。")),
                        );
                    } else {
                        app.show_undo_toast();
                    }
                }
                Err(error) => {
                    if let Some(p) = app.project_mut(id) {
                        p.refresh();
                    }
                    app.show_failure(failure, &error, None);
                }
            }
        });
    }

    /// Before files leave the list, move its highlight to the nearest file that stays.
    fn move_selection(&mut self, leaving: &[FileChange], id: ProjectId) {
        let gone: HashSet<String> = leaving.iter().map(|c| ChangeRow::file_id(&c.path)).collect();
        if gone.is_empty() {
            return;
        }
        let flat = self.settings.flat_list;
        let Some(p) = self.project(id) else { return };
        let ordered: Vec<String> =
            change_tree::ordered_files(p.changes(), flat, &p.filter.text, true).iter().map(|c| ChangeRow::file_id(&c.path)).collect();
        let next = (|| {
            let current = p.selected_change_id.as_ref()?;
            if !(gone.contains(current) || !current.starts_with("f:")) {
                return None;
            }
            let last = ordered.iter().rposition(|i| gone.contains(i))?;
            let next = ordered[last + 1..].iter().find(|i| !gone.contains(*i)).or_else(|| ordered[..last].iter().rev().find(|i| !gone.contains(*i)));
            if p.browsed_path.is_some() {
                return None;
            }
            next.cloned()
        })();
        let (selected, tab) = (self.selected, p.tab);
        let p = self.project_mut(id).expect("project");
        if let Some(next) = next {
            p.selected_change_id = Some(next);
            p.selected_change_ids = p.selected_change_id.iter().cloned().collect();
            if selected == Some(id) && tab == ProjectTab::Changes {
                self.change_selection_changed(id);
            }
        } else {
            p.selected_change_ids = p.selected_change_id.iter().cloned().collect();
        }
    }
}
