//! The Git graph dropdown: the repository's branches and tags, and the commit graph of whichever is picked.
//!
//! Loaded when the dropdown opens and let go when it closes, so nothing is kept around for a dropdown that is closed.

use std::collections::HashSet;
use std::path::PathBuf;

use super::project::{ProjectId, ProjectTab};
use super::{App, MenuItem, Popup};
use crate::core::git::{self, Commit};
use crate::core::history::{self, GitRef, GraphRow, RefKind};
use crate::core::lang::l;
use crate::core::lf;
use crate::geom::Point;
use crate::icons::Icon;
use crate::text_field::TextField;
use crate::widgets::Scroll;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GraphScope {
    All,
    /// HEAD, for a detached checkout that no branch row stands for.
    Head,
    /// A full ref name.
    Ref(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum BranchRowKind {
    Section { key: &'static str, title: String, count: usize, expanded: bool },
    Scope { scope: GraphScope, title: String, icon: Icon },
    Ref(GitRef),
}

#[derive(Clone, Debug, PartialEq)]
pub struct BranchRow {
    pub id: String,
    pub kind: BranchRowKind,
}

impl BranchRow {
    pub fn is_selectable(&self) -> bool {
        !matches!(self.kind, BranchRowKind::Section { .. })
    }
}

pub const PAGE_SIZE: usize = 400;
/// The most commits the dropdown loads; the Log, with its search, is the place for older history.
pub const MAX_COMMITS: usize = 1200;
/// The graph column never grows wider than this many lanes; lines beyond it are cut off.
pub const MAX_LANES: usize = 12;

pub struct GitGraph {
    pub project: Option<ProjectId>,
    pub scope: GraphScope,
    pub refs: Vec<GitRef>,
    pub rows: Vec<GraphRow>,
    /// Lanes the graph column is sized for.
    pub lane_count: usize,
    pub has_more: bool,
    /// There is more history than `MAX_COMMITS`, and the dropdown stops there.
    pub is_capped: bool,
    pub is_loading: bool,
    pub error: Option<String>,
    pub selected_commit_id: Option<String>,
    /// Every selected commit (Ctrl+click), for comparing two. Includes `selected_commit_id`.
    pub selected_commit_ids: HashSet<String>,
    pub selected_branch_id: Option<String>,
    pub branch_filter: TextField,
    pub collapsed_sections: HashSet<&'static str>,
    /// Where the dropdown hangs from (the branch label); None for the top left corner.
    pub anchor: Option<Point>,
    pub graph_scroll: Scroll,
    pub branch_scroll: Scroll,
    /// Scroll the selected commit into view at the next draw.
    pub reveal: bool,
    root: Option<PathBuf>,
    commits: Vec<Commit>,
    generation: u64,
    refs_generation: u64,
}

impl Default for GitGraph {
    fn default() -> Self {
        GitGraph {
            project: None,
            scope: GraphScope::All,
            refs: Vec::new(),
            rows: Vec::new(),
            lane_count: 1,
            has_more: false,
            is_capped: false,
            is_loading: false,
            error: None,
            selected_commit_id: None,
            selected_commit_ids: HashSet::new(),
            selected_branch_id: Some("scope:all".into()),
            branch_filter: TextField::new(),
            collapsed_sections: ["tags"].into_iter().collect(),
            anchor: None,
            graph_scroll: Scroll::default(),
            branch_scroll: Scroll::default(),
            reveal: false,
            root: None,
            commits: Vec::new(),
            generation: 0,
            refs_generation: 0,
        }
    }
}

impl GitGraph {
    pub fn scope_title(&self) -> String {
        match &self.scope {
            GraphScope::All => l("All branches", "全部分支"),
            GraphScope::Head => "HEAD".into(),
            GraphScope::Ref(name) => self.refs.iter().find(|r| &r.name == name).map(|r| r.short_name.clone()).unwrap_or(name.clone()),
        }
    }

    pub fn branch_rows(&self) -> Vec<BranchRow> {
        let needle = self.branch_filter.text.trim().to_lowercase();
        let matching: Vec<&GitRef> =
            if needle.is_empty() { self.refs.iter().collect() } else { self.refs.iter().filter(|r| r.short_name.to_lowercase().contains(&needle)).collect() };
        let mut rows = vec![BranchRow {
            id: "scope:all".into(),
            kind: BranchRowKind::Scope { scope: GraphScope::All, title: l("All branches", "全部分支"), icon: Icon::BranchFork },
        }];
        if !self.refs.is_empty() && !self.refs.iter().any(|r| r.kind == RefKind::Local && r.is_head) {
            rows.push(BranchRow {
                id: "scope:head".into(),
                kind: BranchRowKind::Scope { scope: GraphScope::Head, title: l("HEAD (detached)", "HEAD（游离）"), icon: Icon::Circle },
            });
        }
        let sections = [("local", l("Local", "本地分支"), RefKind::Local), ("remote", l("Remote", "远程分支"), RefKind::Remote), ("tags", l("Tags", "标签"), RefKind::Tag)];
        for (key, title, kind) in sections {
            let mut list: Vec<&GitRef> = matching.iter().copied().filter(|r| r.kind == kind).collect();
            if list.is_empty() {
                continue;
            }
            // The checked-out branch first; the rest stay newest first, as for-each-ref sorted them.
            if let Some(head) = list.iter().position(|r| r.is_head) {
                let r = list.remove(head);
                list.insert(0, r);
            }
            // While filtering, every section with a match is open.
            let expanded = !needle.is_empty() || !self.collapsed_sections.contains(key);
            rows.push(BranchRow { id: format!("section:{key}"), kind: BranchRowKind::Section { key, title, count: list.len(), expanded } });
            if expanded {
                rows.extend(list.into_iter().map(|r| BranchRow { id: format!("ref:{}", r.name), kind: BranchRowKind::Ref(r.clone()) }));
            }
        }
        rows
    }

    /// The picked branch's tip, or the commit HEAD is on.
    fn initial_selection(&self) -> Option<String> {
        if let GraphScope::Ref(name) = &self.scope {
            if let Some(target) = self.refs.iter().find(|r| &r.name == name).map(|r| r.target.clone()) {
                if self.rows.iter().any(|r| r.commit.hash == target) {
                    return Some(target);
                }
            }
        }
        let head = self.rows.iter().find(|r| r.commit.refs.iter().any(|x| x == "HEAD" || x.starts_with("HEAD -> ")));
        head.or(self.rows.first()).map(|r| r.commit.hash.clone())
    }
}

impl App {
    /// Called as the dropdown opens: reloads, since commits and branches may have moved since last time.
    fn prepare_git_graph(&mut self, id: ProjectId) {
        let Some(url) = self.project(id).map(|p| p.url.clone()) else { return };
        let g = &mut self.git_graph;
        if g.project != Some(id) {
            *g = GitGraph { anchor: g.anchor, ..GitGraph::default() };
            g.project = Some(id);
            g.root = Some(url);
        }
        g.graph_scroll = Scroll::default();
        g.reveal = true;
        self.reload_git_graph();
        self.focus_request = Some(super::ids::graph());
    }

    /// Reads the branches and the graph again, keeping what is picked (after a checkout, a fetch or a push).
    pub fn reload_git_graph(&mut self) {
        self.reload_graph_refs();
        self.load_git_graph(true);
    }

    /// Lets go of everything loaded; the dropdown is closed.
    pub fn release_git_graph(&mut self) {
        let g = &mut self.git_graph;
        g.generation += 1;
        g.refs_generation += 1;
        g.commits = Vec::new();
        g.rows = Vec::new();
        g.is_loading = false;
    }

    fn reload_graph_refs(&mut self) {
        let Some(root) = self.git_graph.root.clone() else { return };
        self.git_graph.refs_generation += 1;
        let generation = self.git_graph.refs_generation;
        self.jobs.spawn(
            move || history::refs(&root).unwrap_or_default(),
            move |app: &mut App, refs: Vec<GitRef>| {
                if app.git_graph.refs_generation == generation && refs != app.git_graph.refs {
                    app.git_graph.refs = refs;
                }
            },
        );
    }

    pub fn load_git_graph(&mut self, reset: bool) {
        let Some(root) = self.git_graph.root.clone() else { return };
        let g = &mut self.git_graph;
        if g.is_loading && !reset {
            return;
        }
        g.generation += 1;
        let generation = g.generation;
        g.is_loading = true;
        let skip = if reset { 0 } else { g.commits.len() };
        let scope = g.scope.clone();
        self.jobs.spawn(
            move || match &scope {
                GraphScope::All => git::log(&root, skip, PAGE_SIZE, "", true, None),
                GraphScope::Head => git::log(&root, skip, PAGE_SIZE, "", false, None),
                GraphScope::Ref(name) => git::log(&root, skip, PAGE_SIZE, "", false, Some(name)),
            },
            move |app: &mut App, result: git::Result<Vec<Commit>>| {
                let g = &mut app.git_graph;
                if g.generation != generation {
                    return;
                }
                let (page, failure) = match result {
                    Ok(p) => (p, None),
                    Err(e) => (Vec::new(), Some(e.message())),
                };
                if reset {
                    g.commits = page.clone();
                } else {
                    let known: HashSet<String> = g.commits.iter().map(|c| c.hash.clone()).collect();
                    g.commits.extend(page.iter().filter(|c| !known.contains(&c.hash)).cloned());
                }
                g.rows = history::graph_layout(g.commits.clone());
                g.lane_count = (g.rows.iter().map(|r| r.width as usize).max().unwrap_or(1)).clamp(1, MAX_LANES);
                let full = page.len() >= PAGE_SIZE;
                g.has_more = full && g.commits.len() < MAX_COMMITS;
                g.is_capped = full && !g.has_more;
                g.error = failure;
                g.is_loading = false;
                let selected_present = g.selected_commit_id.as_ref().is_some_and(|s| g.rows.iter().any(|r| &r.commit.hash == s));
                if !selected_present {
                    g.selected_commit_id = g.initial_selection();
                    g.selected_commit_ids = g.selected_commit_id.iter().cloned().collect();
                    g.reveal = true;
                }
            },
        );
    }

    pub fn pick_branch_row(&mut self, row: &BranchRow) {
        match &row.kind {
            BranchRowKind::Section { key, .. } => {
                let c = &mut self.git_graph.collapsed_sections;
                if !c.remove(key) {
                    c.insert(key);
                }
            }
            BranchRowKind::Scope { scope, .. } => self.set_graph_scope(scope.clone(), row.id.clone()),
            BranchRowKind::Ref(r) => self.set_graph_scope(GraphScope::Ref(r.name.clone()), row.id.clone()),
        }
    }

    fn set_graph_scope(&mut self, scope: GraphScope, branch_id: String) {
        let g = &mut self.git_graph;
        g.selected_branch_id = Some(branch_id);
        if scope == g.scope {
            return;
        }
        g.scope = scope;
        g.selected_commit_id = None;
        g.selected_commit_ids.clear();
        g.graph_scroll = Scroll::default();
        self.load_git_graph(true);
    }

    /// A row's right-click menu. Picks the row, as a right-click does in the IDE.
    pub fn graph_commit_menu(&mut self, index: usize) -> Vec<MenuItem> {
        let Some(id) = self.git_graph.project else { return Vec::new() };
        let Some(row) = self.git_graph.rows.get(index) else { return Vec::new() };
        let commit = row.commit.clone();
        let list: Vec<Commit> = self.git_graph.rows.iter().map(|r| r.commit.clone()).collect();
        let mut items = Vec::new();
        let picked = self.git_graph.selected_commit_ids.clone();
        match picked.contains(&commit.hash).then(|| App::picked_pair(&picked, &list)).flatten() {
            Some((older, newer)) => {
                let (a, b) = (older.short_hash(), newer.short_hash());
                items.push(MenuItem::new(lf!("Compare {a} with {b}", "比较 {a} 与 {b}"), move |app| app.compare_commits(older, newer, id)));
                items.push(MenuItem::separator());
            }
            None => {
                self.git_graph.selected_commit_id = Some(commit.hash.clone());
                self.git_graph.selected_commit_ids = [commit.hash.clone()].into_iter().collect();
            }
        }
        let (hash, short) = (commit.hash.clone(), commit.short_hash());
        {
            let hash = hash.clone();
            items.push(MenuItem::new(l("Show in Log", "在历史中查看"), move |app| app.show_commit_in_log(hash)));
        }
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
        let commits = self.menu_commits(&commit, &self.git_graph.selected_commit_ids.clone(), &list);
        self.add_history_edit_items(&mut items, &commit, commits, &list, id);
        items.push(MenuItem::separator());
        let subject = commit.subject.clone();
        items.push(MenuItem::new(l("Copy Hash", "复制哈希"), move |app| app.copy_text(hash)));
        items.push(MenuItem::new(l("Copy Message", "复制提交信息"), move |app| app.copy_text(subject)));
        items
    }

    /// A branch's or tag's right-click menu: what changed since it, compared with the working tree.
    pub fn graph_branch_menu(&mut self, row: &BranchRow) -> Vec<MenuItem> {
        let BranchRowKind::Ref(r) = &row.kind else { return Vec::new() };
        let Some(id) = self.git_graph.project else { return Vec::new() };
        let mut items = Vec::new();
        let short = r.short_name.clone();
        if !(r.kind == RefKind::Local && r.is_head) {
            let title = if r.kind == RefKind::Tag { l("Checkout Tag…", "检出标签…") } else { l("Checkout", "检出") };
            let target = r.clone();
            items.push(MenuItem::new(title, move |app| app.checkout(target, id)));
        }
        {
            let (name, short) = (r.name.clone(), short.clone());
            items.push(MenuItem::new(lf!("New Branch from {short}…", "从 {short} 新建分支…"), move |app| app.new_branch_from(Some(name), Some(short), None, id)));
        }
        items.push(MenuItem::separator());
        if r.kind != RefKind::Tag {
            let (name, short) = (r.name.clone(), short.clone());
            items.push(MenuItem::new(lf!("Show Changes Since Forking from {short}", "查看自与 {short} 分叉以来的变更"), move |app| {
                app.compare_in(name, short, true, id)
            }));
        }
        {
            let (name, short) = (r.name.clone(), short.clone());
            items.push(MenuItem::new(l("Compare with Working Tree", "与工作区比较"), move |app| app.compare_in(name, short, false, id)));
        }
        items.push(MenuItem::separator());
        self.add_branch_edit_items(&mut items, r, id);
        items.push(MenuItem::new(l("Copy Name", "复制名称"), move |app| app.copy_text(short)));
        items
    }

    /// Ctrl+Shift+`: opens the current project's Git graph under its branch in the sidebar, or closes it.
    pub fn toggle_git_graph(&mut self) {
        if self.popup == Some(Popup::GitGraph) {
            self.dismiss_popup(true);
            return;
        }
        let Some(id) = self.selected else { return };
        // With the sidebar hidden the dropdown takes the top left corner.
        let anchor = if self.sidebar_visible { self.branch_label_anchors.get(&id).copied() } else { None };
        self.open_git_graph(id, anchor);
    }

    /// A click on a project's branch: opens that project's Git graph under it, picking the project, or closes it.
    pub fn toggle_git_graph_for(&mut self, id: ProjectId, anchor: Point) {
        if self.popup == Some(Popup::GitGraph) && self.git_graph.project == Some(id) {
            self.dismiss_popup(true);
            return;
        }
        if self.selected != Some(id) {
            self.select(id);
        }
        self.open_git_graph(id, Some(anchor));
    }

    fn open_git_graph(&mut self, id: ProjectId, anchor: Option<Point>) {
        self.git_graph.anchor = anchor;
        self.prepare_git_graph(id);
        self.present(Popup::GitGraph);
        self.focus_request = Some(super::ids::graph());
    }

    /// Shows a commit picked in the graph in the Log, with the same branches listed, and closes the graph.
    pub fn show_commit_in_log(&mut self, hash: String) {
        let Some(id) = self.selected else { return };
        let index = self.git_graph.rows.iter().position(|r| r.commit.hash == hash).unwrap_or(0);
        let scope = self.git_graph.scope.clone();
        self.dismiss_popup(false);
        let Some(p) = self.project_mut(id) else { return };
        match scope {
            GraphScope::All => {
                p.log_all_branches = true;
                p.log_ref = None;
            }
            GraphScope::Head => {
                p.log_all_branches = false;
                p.log_ref = None;
            }
            GraphScope::Ref(name) => {
                p.log_all_branches = false;
                p.log_ref = Some(name);
            }
        }
        p.log_query.set_text("");
        p.history = None;
        p.browsed_path = None;
        p.tab = ProjectTab::Log;
        p.selected_commit_id = Some(hash.clone());
        p.selected_commit_ids = [hash.clone()].into_iter().collect();
        self.list_visible = true;
        // Enough of the history in one go that the commit is in the list, however far down the graph it was.
        self.project_mut(id).expect("project").load_log(
            true,
            index + 50,
            Some(Box::new(move |app: &mut App| {
                if let Some(p) = app.project_mut(id) {
                    if p.commits.iter().any(|c| c.hash == hash) {
                        p.selected_commit_id = Some(hash);
                    }
                }
                app.commit_selection_changed(id);
            })),
        );
        self.focus_list();
    }

    /// A commit named elsewhere (Annotate) shown in the Log of the current project.
    pub fn show_commit_in_history(&mut self, hash: String) {
        self.show_commit_in_log(hash);
    }
}
