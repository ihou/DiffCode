//! Picking the changes, and the lines, of the file on screen that go into the next commit, as with the IDE's
//! checkboxes in the commit diff. Whatever is not picked stays in the working tree for a later commit.

use std::collections::HashSet;

use super::project::ProjectId;
use super::{App, MenuItem};
use crate::canvas::CommitTicks;
use crate::core::commit::{self, ChangeLine};
use crate::core::diff::{Content, DiffDocument, Hunk, Side};
use crate::core::git::{ChangeKind, FileChange};
use crate::core::lang::l;

impl App {
    /// The file on screen, when its changes can be picked one by one: a local change against HEAD shown as text,
    /// compared exactly (whitespace too, or a pick could take in changes that are not on screen), outside a merge or
    /// rebase.
    pub fn pickable_change(&self, id: ProjectId) -> Option<FileChange> {
        let p = self.project(id)?;
        if p.comparison.is_some() || p.status.operation.is_some() || self.viewer.is_loading {
            return None;
        }
        let request = self.viewer.request.as_ref()?;
        if request.ignore_whitespace {
            return None;
        }
        let change = self.viewer_change(id)?;
        if change.kind == ChangeKind::Conflicted || change.is_submodule {
            return None;
        }
        let document = self.viewer.document()?;
        if !matches!(document.content, Content::Text) || document.is_plain_file || document.hunks().is_empty() {
            return None;
        }
        Some(change)
    }

    /// Brings the viewer's ticks up to date with what goes into the commit. While a document loads, the one on screen
    /// keeps its ticks until the new one is in.
    pub fn update_commit_ticks(&mut self) {
        if self.viewer.is_loading {
            return;
        }
        let mut ticks = None;
        if let Some(id) = self.selected {
            if let Some(change) = self.pickable_change(id) {
                let p = self.project(id).expect("project");
                let mut value = CommitTicks { included: p.is_included(&change), ..Default::default() };
                for line in p.excluded_lines.get(&change.path).into_iter().flatten() {
                    if line.removed {
                        value.removed.insert(line.line);
                    } else {
                        value.added.insert(line.line);
                    }
                }
                ticks = Some(value);
            }
        }
        self.viewer.canvas.set_commit_ticks(ticks);
    }

    /// The document on screen changed (opened, edited, read again): the lines left out of it move with their text.
    pub fn viewer_document_changed(&mut self) {
        if let Some(id) = self.selected {
            if let Some(change) = self.pickable_change(id) {
                let document = self.viewer.document().expect("document");
                let p = self.project(id).expect("project");
                if let Some(excluded) = p.excluded_lines.get(&change.path).filter(|e| !e.is_empty()) {
                    let (moved, _) = commit::relocate(excluded, document.left.lines(), document.right.lines(), document.hunks());
                    if &moved != excluded {
                        let p = self.project_mut(id).expect("project");
                        if moved.is_empty() {
                            p.excluded_lines.remove(&change.path);
                        } else {
                            p.excluded_lines.insert(change.path.clone(), moved);
                        }
                    }
                }
            }
        }
        self.update_commit_ticks();
    }

    /// The tick on change `index` was clicked: all of the change goes in, or none of it.
    /// In a file that was not ticked, the change is the only one of the file that goes in.
    pub fn toggle_commit_hunk(&mut self, index: usize) {
        let Some(id) = self.selected else { return };
        let Some(change) = self.pickable_change(id) else { return };
        let document = self.viewer.document().expect("document");
        let Some(hunk) = document.hunks().get(index) else { return };
        let lines = change_lines(std::slice::from_ref(hunk), document);
        let p = self.project(id).expect("project");
        let mut excluded = p.excluded_lines.get(&change.path).cloned().unwrap_or_default();
        if !p.is_included(&change) {
            let all = change_lines(document.hunks(), document);
            excluded = all.difference(&lines).cloned().collect();
        } else if lines.is_subset(&excluded) {
            excluded.retain(|l| !lines.contains(l));
        } else {
            excluded.extend(lines);
        }
        let all = change_lines(document.hunks(), document);
        self.set_excluded(excluded, &change, &all, id);
    }

    /// The changed lines among those selected in the viewer, each with the line across from it; None when the
    /// selection has none. A line and the one across from it are one change: left out together, a modified line stays
    /// the way HEAD has it, rather than losing its old text without getting the new.
    pub fn selected_change_lines(&self) -> Option<HashSet<ChangeLine>> {
        let id = self.selected?;
        self.pickable_change(id)?;
        let document = self.viewer.document()?;
        if self.viewer.canvas.selection.is_none_or(|s| s.is_empty()) {
            return None;
        }
        let (left, right) = (document.left.lines(), document.right.lines());
        let mut lines = HashSet::new();
        for (side, line) in self.viewer.canvas.selected_source_lines() {
            let hunk = document.hunks().iter().find(|h| match side {
                Side::Left => (h.left_start..h.left_start + h.left_count).contains(&line),
                Side::Right => (h.right_start..h.right_start + h.right_count).contains(&line),
            });
            let Some(hunk) = hunk else { continue };
            let offset = line - if side == Side::Left { hunk.left_start } else { hunk.right_start };
            if offset < hunk.left_count && hunk.left_start + offset < left.len() {
                let at = hunk.left_start + offset;
                lines.insert(ChangeLine { removed: true, line: at, text: left[at].clone() });
            }
            if offset < hunk.right_count && hunk.right_start + offset < right.len() {
                let at = hunk.right_start + offset;
                lines.insert(ChangeLine { removed: false, line: at, text: right[at].clone() });
            }
        }
        (!lines.is_empty()).then_some(lines)
    }

    /// Puts the selected changed lines into the commit, or leaves them out of it.
    pub fn set_selected_lines(&mut self, included: bool) {
        let Some(id) = self.selected else { return };
        let Some(change) = self.pickable_change(id) else { return };
        let Some(lines) = self.selected_change_lines() else { return };
        let document = self.viewer.document().expect("document");
        let all = change_lines(document.hunks(), document);
        let p = self.project(id).expect("project");
        let mut excluded = if p.is_included(&change) { p.excluded_lines.get(&change.path).cloned().unwrap_or_default() } else { all.clone() };
        if included {
            excluded.retain(|l| !lines.contains(l));
        } else {
            excluded.extend(lines);
        }
        self.set_excluded(excluded, &change, &all, id);
    }

    /// Menu items for picking what of the file on screen goes into the commit: the selected lines, or the change
    /// under the pointer.
    pub fn add_commit_pick_items(&self, items: &mut Vec<MenuItem>, id: ProjectId) -> bool {
        let Some(change) = self.pickable_change(id) else { return false };
        let Some(document) = self.viewer.document() else { return false };
        let p = self.project(id).expect("project");
        let excluded = if p.is_included(&change) { Some(p.excluded_lines.get(&change.path).cloned().unwrap_or_default()) } else { None };
        let is_out = |line: &ChangeLine| excluded.as_ref().is_none_or(|e| e.contains(line));
        if let Some(lines) = self.selected_change_lines() {
            let mut added = false;
            if lines.iter().any(is_out) {
                items.push(MenuItem::new(l("Include Selected Lines in Commit", "提交选中的行"), |app| app.set_selected_lines(true)));
                added = true;
            }
            if lines.iter().any(|line| !is_out(line)) {
                items.push(MenuItem::new(l("Exclude Selected Lines from Commit", "不提交选中的行"), |app| app.set_selected_lines(false)));
                added = true;
            }
            return added;
        }
        let Some(index) = self.context_hunk.filter(|i| *i < document.hunks().len()) else { return false };
        let lines = change_lines(&document.hunks()[index..=index], document);
        let goes_in = p.is_included(&change) && !lines.is_subset(excluded.as_ref().unwrap_or(&HashSet::new()));
        let title = if goes_in {
            l("Exclude This Change from Commit", "不提交这一处变更")
        } else {
            l("Include This Change in Commit", "提交这一处变更")
        };
        items.push(MenuItem::new(title, move |app| app.toggle_commit_hunk(index)));
        true
    }

    /// Records what of a file stays out. With nothing of it left to go in, the file is simply unticked.
    fn set_excluded(&mut self, excluded: HashSet<ChangeLine>, change: &FileChange, all: &HashSet<ChangeLine>, id: ProjectId) {
        let Some(p) = self.project_mut(id) else { return };
        if all.is_subset(&excluded) {
            p.set_included(false, std::slice::from_ref(change));
        } else {
            if !p.is_included(change) {
                p.set_included(true, std::slice::from_ref(change));
            }
            if excluded.is_empty() {
                p.excluded_lines.remove(&change.path);
            } else {
                p.excluded_lines.insert(change.path.clone(), excluded);
            }
        }
        self.update_commit_ticks();
    }
}

/// Every changed line of `hunks`, on both sides.
fn change_lines(hunks: &[Hunk], document: &DiffDocument) -> HashSet<ChangeLine> {
    let (left, right) = (document.left.lines(), document.right.lines());
    let mut lines = HashSet::new();
    for hunk in hunks {
        for (line, text) in left.iter().enumerate().skip(hunk.left_start).take(hunk.left_count) {
            lines.insert(ChangeLine { removed: true, line, text: text.clone() });
        }
        for (line, text) in right.iter().enumerate().skip(hunk.right_start).take(hunk.right_count) {
            lines.insert(ChangeLine { removed: false, line, text: text.clone() });
        }
    }
    lines
}
