//! A file's history and its lines' history (in the Log), Annotate, and comparing the working tree with another
//! commit or branch (in the Changes tab).

use std::collections::HashSet;

use super::change_tree::{self, ChangeRow};
use super::project::{Comparison, ComparisonEnd, HistoryFilter, ProjectId, ProjectTab};
use super::{App, MenuItem, PlatformRequest, Popup};
use crate::core::annotations::AnnotationEntry;
use crate::core::diff::{join_repo_path, DiffRequest, Side, Source};
use crate::core::git::{ChangeKind, Commit, FileChange, EMPTY_TREE};
use crate::core::history;
use crate::core::lang::l;
use crate::core::lf;

/// Lines on one side of a document, 0-based and inclusive.
pub type Lines = (Side, (usize, usize));

impl App {
    // MARK: History

    /// Lists in the Log the commits that changed a file, or some of its lines, and shows the file's diff in the first.
    /// `hash`: a commit to pick instead of the newest (one clicked in the Annotate column).
    pub fn show_history(&mut self, filter: HistoryFilter, id: ProjectId, hash: Option<String>) {
        if self.selected != Some(id) {
            self.selected = Some(id);
            self.persist_projects();
        }
        let Some(p) = self.project_mut(id) else { return };
        p.history = Some(filter.clone());
        p.history_paths.clear();
        p.log_query.set_text("");
        p.browsed_path = None;
        p.tab = ProjectTab::Log;
        p.commits.clear();
        p.selected_commit_id = hash.clone();
        p.selected_commit_file_id = None;
        self.list_visible = true;
        // A commit asked for by name may be far down a long history: load enough of it in one go.
        let at_least = if hash.is_none() { 0 } else { 2000 };
        let completion = Box::new(move |app: &mut App| {
            let Some(p) = app.project(id) else { return };
            if p.history.as_ref() != Some(&filter) {
                return;
            }
            let Some(hash) = hash.filter(|h| !p.commits.iter().any(|c| &c.hash == h)) else {
                app.commit_selection_changed(id);
                return;
            };
            // Not in the file's history as listed (a commit on another branch, say): shown at the top anyway.
            let url = p.url.clone();
            app.jobs.spawn(
                {
                    let hash = hash.clone();
                    move || history::commit(&hash, &url)
                },
                move |app: &mut App, commit: Option<Commit>| {
                    let Some(p) = app.project_mut(id) else { return };
                    if p.history.as_ref() != Some(&filter) {
                        return;
                    }
                    if let Some(commit) = commit {
                        p.commits.insert(0, commit);
                        p.selected_commit_id = Some(hash);
                    }
                    app.commit_selection_changed(id);
                },
            );
        });
        self.project_mut(id).expect("project").load_log(true, at_least, Some(completion));
        self.sync_viewer();
        self.focus_list();
    }

    /// Back from a file's history to the branch the Log listed before.
    pub fn end_history(&mut self) {
        let Some(id) = self.selected else { return };
        let Some(p) = self.project_mut(id) else { return };
        if p.history.is_none() {
            return;
        }
        p.history = None;
        p.history_paths.clear();
        p.commits.clear();
        p.load_log(true, 0, Some(Box::new(move |app: &mut App| app.commit_selection_changed(id))));
    }

    /// The history of the file on screen.
    pub fn show_history_for_current_file(&mut self) {
        let Some(id) = self.selected else { return };
        let Some(request) = self.viewer_request_in(id) else { return };
        match self.history_filter(&request, None, id) {
            Some(filter) => self.show_history(filter, id, None),
            None => self.show_toast(l("Not committed yet · no history", "这个文件还没有提交过 · 没有历史")),
        }
    }

    /// The history of the selected lines of the file on screen, or of the line with the caret.
    pub fn show_history_for_selection(&mut self) {
        let Some(id) = self.selected else { return };
        let Some(request) = self.viewer_request_in(id) else { return };
        let Some((side, range)) = self.viewer.canvas.selected_lines() else {
            self.show_history_for_current_file();
            return;
        };
        let selected: Lines = (side, (range.start, range.end.saturating_sub(1).max(range.start)));
        let not_committed = l("These lines are not committed yet · no history", "这些行还没有提交过 · 没有历史");
        if counts_in_head(&request, selected.0) {
            // The screen does not show these lines next to HEAD's, so git matches them up from the file on disk.
            if self.is_new_since_head(&request.path, id) {
                self.show_toast(not_committed);
                return;
            }
            if !self.viewer.save_edits() {
                return;
            }
            let p = self.project(id).expect("project");
            let head_path = p.changes().iter().find(|c| c.path == request.path).and_then(|c| c.old_path.clone()).unwrap_or(request.path.clone());
            let url = p.url.clone();
            let path = request.path.clone();
            let hp = head_path.clone();
            self.jobs.spawn(
                move || history::head_lines(&path, &hp, selected.1, &url),
                move |app: &mut App, lines: Option<(usize, usize)>| match lines {
                    Some((a, b)) => app.show_history(HistoryFilter { path: head_path, lines: Some((a + 1, b + 1)), rev: "HEAD".into() }, id, None),
                    None => app.show_toast(not_committed),
                },
            );
            return;
        }
        match self.history_filter(&request, Some(selected), id) {
            Some(filter) => self.show_history(filter, id, None),
            None => self.show_toast(not_committed),
        }
    }

    /// The viewer's request, when it shows a file of the project `id`.
    fn viewer_request_in(&self, id: ProjectId) -> Option<DiffRequest> {
        let url = &self.project(id)?.url;
        self.viewer.request.clone().filter(|r| &r.root == url)
    }

    /// A file HEAD does not have.
    pub fn is_new_since_head(&self, path: &str, id: ProjectId) -> bool {
        self.project(id)
            .and_then(|p| p.changes().iter().find(|c| c.path == path))
            .is_some_and(|c| matches!(c.kind, ChangeKind::Untracked | ChangeKind::Added))
    }

    /// Whether the selected lines may have a history, without asking git.
    pub fn selection_may_have_history(&self, request: &DiffRequest, lines: Lines, id: ProjectId) -> bool {
        if counts_in_head(request, lines.0) {
            !self.is_new_since_head(&request.path, id)
        } else {
            self.history_filter(request, Some(lines), id).is_some()
        }
    }

    /// The history of a file in the project, by its path in the working tree.
    pub fn show_history_of_path(&mut self, path: &str, id: ProjectId) {
        if self.is_new_since_head(path, id) {
            self.show_toast(l("Not committed yet · no history", "这个文件还没有提交过 · 没有历史"));
            return;
        }
        let Some(p) = self.project(id) else { return };
        let head_path = p.changes().iter().find(|c| c.path == path).and_then(|c| c.old_path.clone()).unwrap_or(path.to_string());
        self.show_history(HistoryFilter { path: head_path, lines: None, rev: "HEAD".into() }, id, None);
    }

    /// What to list for the history of the file a document shows, or of some of its lines (0-based, on one side).
    /// None when there is nothing committed to list, and for lines that `counts_in_head`, which need git to match up.
    pub fn history_filter(&self, request: &DiffRequest, lines: Option<Lines>, id: ProjectId) -> Option<HistoryFilter> {
        let p = self.project(id)?;
        let one_based = |(a, b): (usize, usize)| Some((a + 1, b + 1));
        match &request.source {
            Source::File | Source::Worktree { .. } => {
                let listed = p.changes().iter().any(|c| c.path == request.path);
                if self.is_new_since_head(&request.path, id) || (request.kind == Some(ChangeKind::Untracked) && !listed) {
                    return None;
                }
                let head_path = p.changes().iter().find(|c| c.path == request.path).and_then(|c| c.old_path.clone()).unwrap_or(request.path.clone());
                let Some((side, range)) = lines else {
                    // The whole file: its history up to HEAD, under the name HEAD knows it by.
                    return Some(HistoryFilter { path: head_path, lines: None, rev: "HEAD".into() });
                };
                let Source::Worktree { base } = &request.source else { return None };
                if base == EMPTY_TREE {
                    return None;
                }
                if side == Side::Left {
                    // Lines of the version compared with, where their history starts.
                    return Some(HistoryFilter {
                        path: request.old_path.clone().unwrap_or(request.path.clone()),
                        lines: one_based(range),
                        rev: base.clone(),
                    });
                }
                if !request.is_against_head() {
                    return None;
                }
                let doc = self.viewer.document()?;
                let mapped = history::old_lines_for_new(range, doc.hunks(), doc.left.len())?;
                Some(HistoryFilter { path: head_path, lines: one_based(mapped), rev: "HEAD".into() })
            }
            Source::Commit { hash, parent } => {
                let Some((side, range)) = lines else {
                    if request.kind == Some(ChangeKind::Deleted) {
                        return parent.as_ref().map(|parent| HistoryFilter {
                            path: request.old_path.clone().unwrap_or(request.path.clone()),
                            lines: None,
                            rev: parent.clone(),
                        });
                    }
                    // From HEAD while the file is still there under that name, which shows what came after too.
                    let exists = join_repo_path(&p.url, &request.path).exists();
                    return Some(HistoryFilter { path: request.path.clone(), lines: None, rev: if exists { "HEAD".into() } else { hash.clone() } });
                };
                if side == Side::Left {
                    let parent = parent.as_ref()?;
                    return Some(HistoryFilter {
                        path: request.old_path.clone().unwrap_or(request.path.clone()),
                        lines: one_based(range),
                        rev: parent.clone(),
                    });
                }
                Some(HistoryFilter { path: request.path.clone(), lines: one_based(range), rev: hash.clone() })
            }
        }
    }

    /// A line's commit was clicked in the Annotate column: the file's history, with that commit picked.
    pub fn open_annotation(&mut self, entry: AnnotationEntry) {
        let Some(id) = self.selected else { return };
        let Some(request) = self.viewer_request_in(id) else { return };
        let filter = self.history_filter(&request, None, id).unwrap_or(HistoryFilter { path: request.path.clone(), lines: None, rev: "HEAD".into() });
        self.show_history(filter, id, Some(entry.hash.clone()));
    }

    // MARK: Comparison

    /// The Changes tab lists everything that differs between `target` and the working tree, committed or not.
    /// `target`: a full ref name or a commit hash. `fork_point`: compare with where HEAD and `target` forked (for a
    /// branch), rather than with `target` itself.
    pub fn compare_with(&mut self, target: String, title: String, fork_point: bool) {
        let Some(id) = self.selected else { return };
        self.compare_in(target, title, fork_point, id);
    }

    pub fn compare_in(&mut self, target: String, title: String, fork_point: bool, id: ProjectId) {
        let Some(url) = self.project(id).map(|p| p.url.clone()) else { return };
        let t = target.clone();
        self.jobs.spawn(
            move || if fork_point { history::merge_base("HEAD", &t, &url) } else { history::resolve_commit(&t, &url) },
            move |app: &mut App, base: Option<String>| {
                let Some(base) = base else {
                    app.show_toast(if fork_point {
                        lf!("{title} and HEAD have no commit in common", "{title} 和 HEAD 没有共同的提交")
                    } else {
                        lf!("No commit named “{title}”", "找不到“{title}”这个提交")
                    });
                    return;
                };
                app.start_comparison(Comparison { target, title, fork_point, base, other: None, stash: None }, id);
            },
        );
    }

    /// The Changes tab lists what differs between two commits, the older one on the left (as the IDE's Compare
    /// Versions).
    pub fn compare_commits(&mut self, older: Commit, newer: Commit, id: ProjectId) {
        let comparison = Comparison {
            target: older.hash.clone(),
            title: older.short_hash(),
            fork_point: false,
            base: older.hash.clone(),
            other: Some(ComparisonEnd { target: newer.hash.clone(), title: newer.short_hash(), hash: newer.hash.clone() }),
            stash: None,
        };
        self.start_comparison(comparison, id);
    }

    /// Shows `comparison` in the project's Changes tab, from the top.
    pub fn start_comparison(&mut self, comparison: Comparison, id: ProjectId) {
        if matches!(self.popup, Some(Popup::GitGraph) | Some(Popup::QuickOpen)) {
            self.dismiss_popup(false);
        }
        if self.selected != Some(id) {
            self.select(id);
        }
        let Some(p) = self.project_mut(id) else { return };
        p.comparison = Some(comparison);
        p.compare_changes.clear();
        p.compare_selected_id = None;
        p.compare_collapsed.clear();
        p.compare_error = None;
        p.browsed_path = None;
        p.tab = ProjectTab::Changes;
        self.list_visible = true;
        self.project_mut(id).expect("project").load_comparison();
        self.focus_list();
    }

    /// The two commits picked together in a list, older first (lists run newest first), or None unless exactly two
    /// are.
    pub fn picked_pair(ids: &HashSet<String>, list: &[Commit]) -> Option<(Commit, Commit)> {
        if ids.len() != 2 {
            return None;
        }
        let picked: Vec<&Commit> = list.iter().filter(|c| ids.contains(&c.hash)).collect();
        if picked.len() != 2 {
            return None;
        }
        Some((picked[1].clone(), picked[0].clone()))
    }

    pub fn end_comparison(&mut self) {
        if let Some(id) = self.selected {
            self.end_comparison_in(id);
        }
    }

    /// Back to the local changes.
    pub fn end_comparison_in(&mut self, id: ProjectId) {
        let Some(p) = self.project_mut(id) else { return };
        if p.comparison.is_none() {
            return;
        }
        p.comparison = None;
        p.compare_changes.clear();
        p.compare_selected_id = None;
        p.compare_error = None;
        p.load_comparison();
        self.sync_viewer();
    }

    /// Picks a branch or tag (or types a commit) to compare the working tree with.
    pub fn show_compare_picker(&mut self) {
        if self.current_project().is_some() {
            self.show_quick_open(super::search::QuickOpenMode::Branches);
        }
    }

    pub fn compare_selection_changed(&mut self, id: ProjectId) {
        if self.selected != Some(id) {
            return;
        }
        let Some(p) = self.project(id) else { return };
        if p.tab != ProjectTab::Changes {
            return;
        }
        let Some(comparison) = p.comparison.clone() else { return };
        let Some(sel) = p.compare_selected_id.clone() else { return };
        let Some(change) = p.compare_changes.iter().find(|c| ChangeRow::file_id(&c.path) == sel).cloned() else { return };
        if p.browsed_path.as_deref() != Some(change.path.as_str()) {
            self.project_mut(id).expect("project").browsed_path = None;
        }
        self.show_compared(&change, &comparison, id);
        self.note_recent(id, &change.path);
        self.follow(&change.path, id);
    }

    fn show_compared(&mut self, change: &FileChange, comparison: &Comparison, id: ProjectId) {
        let Some(p) = self.project(id) else { return };
        let request = p.request_compared(change, comparison, self.settings.ignore_whitespace);
        if Some(&request) == self.viewer.request.as_ref() {
            self.viewer.reload_if_stale(Some(comparison.base.clone()));
        } else {
            let same = self.viewer.request.as_ref().is_some_and(|r| r.path == request.path && r.root == request.root);
            self.viewer.open(request, None, same);
        }
    }

    /// The comparison list's part of `sync_viewer`.
    pub(super) fn sync_comparison(&mut self, id: ProjectId) {
        let Some(p) = self.project(id) else { return };
        let Some(comparison) = p.comparison.clone() else { return };
        if let Some(sel) = p.compare_selected_id.clone() {
            if !sel.starts_with("f:") {
                return; // a folder or group row; leave the viewer alone
            }
            if let Some(change) = p.compare_changes.iter().find(|c| ChangeRow::file_id(&c.path) == sel).cloned() {
                self.show_compared(&change, &comparison, id);
                return;
            }
            if self.viewer.is_editing(&p.url, &sel[2..]) {
                self.viewer.reload_if_stale(Some(comparison.base.clone()));
                return;
            }
        }
        let ordered = change_tree::ordered_files(&p.compare_changes, self.settings.flat_list, &p.filter.text, true);
        if let Some(first) = ordered.first().cloned() {
            self.project_mut(id).expect("project").compare_selected_id = Some(ChangeRow::file_id(&first.path));
            self.show_compared(&first, &comparison, id);
        } else if !p.is_loading_compare {
            self.project_mut(id).expect("project").compare_selected_id = None;
            self.viewer.clear();
        }
    }

    // MARK: Menus

    /// A commit's right-click menu in the Log.
    pub fn log_commit_menu(&mut self, commit: &Commit, id: ProjectId) -> Vec<MenuItem> {
        let Some(p) = self.project(id) else { return Vec::new() };
        let mut items = Vec::new();
        if p.selected_commit_ids.contains(&commit.hash) {
            if let Some((older, newer)) = App::picked_pair(&p.selected_commit_ids, &p.commits) {
                let (a, b) = (older.short_hash(), newer.short_hash());
                items.push(MenuItem::new(lf!("Compare {a} with {b}", "比较 {a} 与 {b}"), move |app| app.compare_commits(older, newer, id)));
                items.push(MenuItem::separator());
            }
        }
        let (hash, short) = (commit.hash.clone(), commit.short_hash());
        {
            let (hash, short) = (hash.clone(), short.clone());
            items.push(MenuItem::new(l("Compare with Working Tree", "与工作区比较"), move |app| app.compare_in(hash, short, false, id)));
        }
        items.push(MenuItem::separator());
        {
            let (hash, short) = (hash.clone(), short.clone());
            items.push(MenuItem::new(l("New Branch Here…", "在此新建分支…"), move |app| app.new_branch_from(Some(hash), Some(short), None, id)));
        }
        {
            let (hash, short) = (hash.clone(), short.clone());
            items.push(MenuItem::new(l("Checkout Revision…", "检出此版本…"), move |app| app.checkout_revision(hash, short, id)));
        }
        items.push(MenuItem::separator());
        let picked = self.menu_commits(commit, &p.selected_commit_ids, &p.commits);
        let list = p.commits.clone();
        self.add_history_edit_items(&mut items, commit, picked, &list, id);
        items.push(MenuItem::separator());
        let subject = commit.subject.clone();
        items.push(MenuItem::new(l("Copy Hash", "复制哈希"), move |app| app.copy_text(hash)));
        items.push(MenuItem::new(l("Copy Message", "复制提交信息"), move |app| app.copy_text(subject)));
        items
    }

    /// A file's right-click menu in a commit's file list.
    pub fn commit_file_menu(&mut self, change: &FileChange, commit: &Commit, id: ProjectId) -> Vec<MenuItem> {
        let Some(p) = self.project(id) else { return Vec::new() };
        let request = p.request_in_commit(change, commit, self.settings.ignore_whitespace);
        let filter = self.history_filter(&request, None, id);
        let enabled = filter.is_some();
        let path = change.path.clone();
        vec![
            MenuItem::new(l("Show History", "查看历史"), move |app| {
                if let Some(filter) = filter {
                    app.show_history(filter, id, None);
                }
            })
            .disabled(!enabled),
            MenuItem::separator(),
            MenuItem::new(l("Copy Path", "复制路径"), move |app| app.copy_text(path)),
        ]
    }

    pub fn copy_text(&mut self, text: String) {
        self.requests.push(PlatformRequest::Copy(text));
        self.show_toast(l("Copied", "已复制"));
    }

    /// History, Annotate and comparison items for the viewer's right-click menu.
    pub fn add_history_items(&mut self, items: &mut Vec<MenuItem>, id: ProjectId) {
        let Some(request) = self.viewer_request_in(id) else { return };
        let annotating = self.viewer.annotating;
        items.push(
            MenuItem::new(l("Annotate with Git Blame", "逐行追溯 (Blame)"), |app| app.viewer.toggle_annotate())
                .checked(annotating)
                .disabled(!(annotating || self.viewer.can_annotate())),
        );
        let has_history = self.history_filter(&request, None, id).is_some();
        items.push(MenuItem::new(l("Show History", "查看历史"), |app| app.show_history_for_current_file()).disabled(!has_history));
        if let Some((side, range)) = self.viewer.canvas.selected_lines() {
            if self.viewer.canvas.selected_text().is_some() {
                let lines: Lines = (side, (range.start, range.end.saturating_sub(1).max(range.start)));
                let may = self.selection_may_have_history(&request, lines, id);
                items.push(MenuItem::new(l("Show History for Selection", "查看所选内容的历史"), |app| app.show_history_for_selection()).disabled(!may));
            }
        }
        items.push(MenuItem::separator());
    }
}

/// Lines of the working tree that the screen does not show next to HEAD's: in a plain file, or in the working tree
/// compared with another commit. Their history is counted from HEAD all the same, so that it lists the commits after
/// that one too.
pub fn counts_in_head(request: &DiffRequest, side: Side) -> bool {
    match request.source {
        Source::File => true,
        Source::Worktree { .. } => side == Side::Right && !request.is_against_head(),
        Source::Commit { .. } => false,
    }
}
