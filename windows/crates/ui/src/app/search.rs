//! Search Everywhere / Go to File / Find Action / Recent Files, the branch and stash pickers that share the list, and
//! Find in Files.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::time::{Duration, Instant};

use super::commands::commands;
use super::project::ProjectId;
use super::{App, Popup};
use crate::canvas::{CodeCanvas, InitialPosition, Mode};
use crate::core::diff::{self, Content, DiffRequest, Side, Source};
use crate::core::fuzzy;
use crate::core::git::{self, CancelToken, ChangeKind, GrepHit, GrepOptions};
use crate::core::history::{self, GitRef, RefKind};
use crate::core::history_edits::{self, StashEntry};
use crate::core::lang::l;
use crate::icons::Icon;
use crate::text_field::TextField;
use crate::widgets::Scroll;

// MARK: - Quick open

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuickOpenMode {
    All,
    Files,
    Actions,
    Recent,
    /// The branches and tags to compare the working tree with (Compare with Branch…).
    Branches,
    /// The stash list.
    Stashes,
}

#[derive(Clone, Debug)]
pub enum QuickItem {
    File { project: ProjectId, path: String, positions: Vec<usize>, kind: Option<ChangeKind> },
    Action { id: &'static str, title: String, shortcut: String, icon: Icon },
    /// A branch or tag to compare with; `positions` are the matched bytes of its short name.
    Ref { r: GitRef, positions: Vec<usize> },
    /// Whatever was typed, taken as a commit to compare with.
    Revision(String),
    /// A stash, to look at; `positions` are the matched bytes of its title.
    Stash { entry: StashEntry, positions: Vec<usize> },
    /// Stash Changes…, at the top of the stash list.
    StashChanges { count: usize },
}

pub struct QuickOpen {
    pub scope: QuickOpenMode,
    pub field: TextField,
    pub items: Vec<QuickItem>,
    pub selected: usize,
    pub is_indexing: bool,
    pub scroll: Scroll,
    /// Scroll the selected row into view at the next draw.
    pub reveal: bool,
    generation: u64,
    /// The branches and tags offered for comparing, read as the picker opens.
    refs: Option<Vec<GitRef>>,
    /// The stash list, read as it opens.
    stashes: Option<Vec<StashEntry>>,
}

impl Default for QuickOpen {
    fn default() -> Self {
        QuickOpen {
            scope: QuickOpenMode::All,
            field: TextField::new(),
            items: Vec::new(),
            selected: 0,
            is_indexing: false,
            scroll: Scroll::default(),
            reveal: false,
            generation: 0,
            refs: None,
            stashes: None,
        }
    }
}

/// Splits a trailing ":123" line number off the query.
fn parse_query(raw: &str) -> (String, Option<usize>) {
    let trimmed = raw.trim();
    if let Some(colon) = trimmed.rfind(':') {
        if let Ok(line) = trimmed[colon + 1..].parse::<usize>() {
            if line > 0 {
                return (trimmed[..colon].to_string(), Some(line));
            }
        }
    }
    (trimmed.to_string(), None)
}

impl App {
    pub fn show_quick_open(&mut self, scope: QuickOpenMode) {
        let q = &mut self.quick_open;
        q.scope = scope;
        // Like the IDE, reopening keeps the previous query selected so typing replaces it.
        if matches!(scope, QuickOpenMode::Recent | QuickOpenMode::Actions | QuickOpenMode::Branches | QuickOpenMode::Stashes) {
            q.field.set_text("");
        }
        q.field.select_all();
        if scope == QuickOpenMode::Branches {
            q.refs = None;
        }
        if scope == QuickOpenMode::Stashes {
            q.stashes = None;
        }
        q.selected = 0;
        q.scroll = Scroll::default();
        self.present(Popup::QuickOpen);
        self.focus_request = Some(super::ids::popup_field());
        self.update_quick_open();
    }

    pub fn set_quick_open_scope(&mut self, scope: QuickOpenMode) {
        if scope != self.quick_open.scope {
            self.quick_open.scope = scope;
            self.update_quick_open();
        }
    }

    /// Tab and Shift+Tab walk All → Files → Actions.
    pub fn cycle_quick_open_scope(&mut self, delta: isize) {
        let scope = self.quick_open.scope;
        if matches!(scope, QuickOpenMode::Branches | QuickOpenMode::Stashes) {
            return;
        }
        let order = [QuickOpenMode::All, QuickOpenMode::Files, QuickOpenMode::Actions];
        let index = order.iter().position(|s| *s == scope).unwrap_or(0) as isize;
        let next = order[((index + delta).rem_euclid(order.len() as isize)) as usize];
        self.set_quick_open_scope(next);
    }

    pub fn update_quick_open(&mut self) {
        self.quick_open.generation += 1;
        let generation = self.quick_open.generation;
        let scope = self.quick_open.scope;
        let (text, _) = parse_query(&self.quick_open.field.text);
        let current = self.selected;
        if scope == QuickOpenMode::Branches {
            self.update_branches(&text);
            return;
        }
        if scope == QuickOpenMode::Stashes {
            let typed = self.quick_open.field.text.trim().to_string();
            self.update_stashes(&typed);
            return;
        }
        let mut actions = Vec::new();
        if matches!(scope, QuickOpenMode::Actions | QuickOpenMode::All) {
            let needle = text.to_lowercase();
            let list = commands(self);
            let mut matched: Vec<&super::commands::Command> = list
                .iter()
                .filter(|c| if needle.is_empty() { scope == QuickOpenMode::Actions } else { c.title.to_lowercase().contains(&needle) })
                .collect();
            if needle.is_empty() {
                // What was run lately comes first.
                let recent = &self.settings.recent_actions;
                matched.sort_by_key(|c| recent.iter().position(|r| r == c.id).unwrap_or(usize::MAX));
            }
            actions = matched
                .into_iter()
                .map(|c| QuickItem::Action { id: c.id, title: c.title.clone(), shortcut: c.shortcut_label(), icon: c.icon })
                .collect();
        }
        if scope == QuickOpenMode::Actions {
            self.set_quick_items(actions, 0);
            return;
        }
        let Some(id) = current else {
            self.set_quick_items(actions, 0);
            return;
        };
        let project = self.project(id).expect("project");
        let kinds: HashMap<String, ChangeKind> = project.changes().iter().map(|c| (c.path.clone(), c.kind)).collect();
        if text.is_empty() {
            // No query: recently viewed files first, then everything with local changes.
            let mut seen = HashSet::new();
            let mut list = Vec::new();
            for entry in &self.recent {
                let Some(owner) = self.project(entry.project) else { continue };
                if !(scope == QuickOpenMode::Recent || owner.id == id) {
                    continue;
                }
                if seen.insert((owner.id, entry.path.clone())) {
                    let kind = owner.changes().iter().find(|c| c.path == entry.path).map(|c| c.kind);
                    list.push(QuickItem::File { project: owner.id, path: entry.path.clone(), positions: Vec::new(), kind });
                }
            }
            for change in project.changes() {
                if seen.insert((id, change.path.clone())) {
                    list.push(QuickItem::File { project: id, path: change.path.clone(), positions: Vec::new(), kind: Some(change.kind) });
                }
            }
            list.truncate(60);
            // Ctrl+E then Enter flips to the previous file.
            let selected = if scope == QuickOpenMode::Recent && list.len() > 1 { 1 } else { self.quick_open.selected.min(list.len().saturating_sub(1)) };
            self.set_quick_items(list, selected);
            return;
        }
        self.quick_open.is_indexing = project.cached_files().is_none();
        let project = self.project_mut(id).expect("project");
        project.with_files(move |p, files| {
            let text = text.clone();
            p.events.push(super::project::ProjectEvent::Callback(Box::new(move |app: &mut App| {
                if app.quick_open.generation != generation {
                    return;
                }
                app.jobs.spawn(
                    move || fuzzy::rank(&text, &files, 80, || false),
                    move |app: &mut App, ranked: Vec<fuzzy::Ranked>| {
                        if app.quick_open.generation != generation {
                            return;
                        }
                        app.quick_open.is_indexing = false;
                        let mut list: Vec<QuickItem> = ranked
                            .into_iter()
                            .map(|r| {
                                let kind = kinds.get(&r.path).copied();
                                QuickItem::File { project: id, path: r.path, positions: r.positions, kind }
                            })
                            .collect();
                        if app.quick_open.scope == QuickOpenMode::All {
                            list.extend(actions);
                        }
                        app.set_quick_items(list, 0);
                    },
                );
            })));
        });
    }

    fn set_quick_items(&mut self, items: Vec<QuickItem>, selected: usize) {
        self.quick_open.items = items;
        self.quick_open.selected = selected;
        self.quick_open.reveal = true;
    }

    /// The picker of what to compare with: local branches first (the current one at the top), then remote ones and
    /// tags; and, once something is typed that names none of them, that text as a commit.
    fn update_branches(&mut self, text: &str) {
        let Some(url) = self.current_project().map(|p| p.url.clone()) else {
            self.set_quick_items(Vec::new(), 0);
            return;
        };
        let Some(refs) = self.quick_open.refs.clone() else {
            self.quick_open.is_indexing = true;
            self.quick_open.items.clear();
            let generation = self.quick_open.generation;
            self.jobs.spawn(
                move || history::refs(&url).unwrap_or_default(),
                move |app: &mut App, refs: Vec<GitRef>| {
                    if app.quick_open.generation != generation || app.quick_open.scope != QuickOpenMode::Branches {
                        return;
                    }
                    app.quick_open.is_indexing = false;
                    app.quick_open.refs = Some(refs);
                    app.update_quick_open();
                },
            );
            return;
        };
        let rank = |k: RefKind| match k {
            RefKind::Local => 0,
            RefKind::Remote => 1,
            RefKind::Tag => 2,
        };
        let mut sorted: Vec<(usize, GitRef)> = refs.into_iter().enumerate().collect();
        sorted.sort_by(|(ia, a), (ib, b)| b.is_head.cmp(&a.is_head).then(rank(a.kind).cmp(&rank(b.kind))).then(ia.cmp(ib)));
        let sorted: Vec<GitRef> = sorted.into_iter().map(|(_, r)| r).collect();
        let list = if text.is_empty() {
            sorted.into_iter().take(200).map(|r| QuickItem::Ref { r, positions: Vec::new() }).collect()
        } else {
            let mut by_name: HashMap<String, GitRef> = HashMap::new();
            for r in &sorted {
                by_name.entry(r.short_name.clone()).or_insert_with(|| r.clone());
            }
            let names: Vec<String> = sorted.iter().map(|r| r.short_name.clone()).collect();
            let mut list: Vec<QuickItem> = fuzzy::rank(text, &names, 80, || false)
                .into_iter()
                .filter_map(|m| by_name.get(&m.path).map(|r| QuickItem::Ref { r: r.clone(), positions: m.positions }))
                .collect();
            if !by_name.contains_key(text) && !text.starts_with('-') {
                list.push(QuickItem::Revision(text.to_string()));
            }
            list
        };
        self.set_quick_items(list, 0);
    }

    /// The stash list, newest first, under Stash Changes… while there is something to stash.
    fn update_stashes(&mut self, text: &str) {
        let Some((url, change_count)) = self.current_project().map(|p| (p.url.clone(), p.changes().len())) else {
            self.set_quick_items(Vec::new(), 0);
            return;
        };
        let Some(stashes) = self.quick_open.stashes.clone() else {
            self.quick_open.is_indexing = true;
            self.quick_open.items.clear();
            let generation = self.quick_open.generation;
            self.jobs.spawn(
                move || history_edits::stashes(&url),
                move |app: &mut App, list: Vec<StashEntry>| {
                    if app.quick_open.generation != generation || app.quick_open.scope != QuickOpenMode::Stashes {
                        return;
                    }
                    app.quick_open.is_indexing = false;
                    app.quick_open.stashes = Some(list);
                    app.update_quick_open();
                },
            );
            return;
        };
        let mut list = Vec::new();
        if text.is_empty() {
            if change_count > 0 {
                list.push(QuickItem::StashChanges { count: change_count });
            }
            list.extend(stashes.into_iter().map(|entry| QuickItem::Stash { entry, positions: Vec::new() }));
        } else {
            // Matched against the title, then where it was made ("main"), as typed.
            let titles: Vec<String> = stashes.iter().map(|s| format!("{}  {}", s.title(), s.branch().unwrap_or(""))).collect();
            let mut by_title: HashMap<&str, &StashEntry> = HashMap::new();
            for (t, s) in titles.iter().zip(&stashes) {
                by_title.entry(t.as_str()).or_insert(s);
            }
            list = fuzzy::rank(text, &titles, 80, || false)
                .into_iter()
                .filter_map(|m| by_title.get(m.path.as_str()).map(|s| QuickItem::Stash { entry: (*s).clone(), positions: m.positions }))
                .collect();
        }
        // Straight to the newest stash rather than to Stash Changes….
        let selected = if list.len() > 1 && text.is_empty() && change_count > 0 { 1 } else { 0 };
        self.set_quick_items(list, selected);
    }

    pub fn move_quick_selection(&mut self, delta: isize) {
        let n = self.quick_open.items.len() as isize;
        if n == 0 {
            return;
        }
        self.quick_open.selected = ((self.quick_open.selected as isize + delta).rem_euclid(n)) as usize;
        self.quick_open.reveal = true;
    }

    pub fn activate_quick_item(&mut self, index: Option<usize>) {
        let target = index.unwrap_or(self.quick_open.selected);
        let Some(item) = self.quick_open.items.get(target).cloned() else { return };
        let (_, line) = parse_query(&self.quick_open.field.text);
        match item {
            QuickItem::File { project, path, .. } => {
                if self.project(project).is_none() {
                    return;
                }
                self.dismiss_popup(false);
                self.open_file(project, path, line);
                self.focus_viewer();
            }
            QuickItem::Action { id, .. } => {
                self.dismiss_popup(true);
                self.run_command(id);
            }
            QuickItem::Ref { r, .. } => {
                // A branch is compared from where it and HEAD forked: what this side changed, not what the other did
                // since.
                let fork_point = r.kind != RefKind::Tag;
                self.compare_with(r.name.clone(), r.short_name.clone(), fork_point);
            }
            QuickItem::Revision(text) => self.compare_with(text.clone(), text, false),
            QuickItem::Stash { entry, .. } => self.show_stash(entry),
            QuickItem::StashChanges { .. } => {
                self.dismiss_popup(true);
                self.stash_changes();
            }
        }
    }
}

// MARK: - Find in Files

#[derive(Clone, Debug)]
pub struct Hit {
    pub project: ProjectId,
    pub path: String,
    pub line: usize,
    pub text: String,
    /// The match within `text`, in bytes.
    pub found: Option<Range<usize>>,
}

pub const FIND_LIMIT: usize = 3000;

pub struct FindInPath {
    pub field: TextField,
    pub mask: TextField,
    pub options: GrepOptions,
    pub all_projects: bool,
    pub hits: Vec<Hit>,
    pub file_count: usize,
    pub selected: usize,
    pub is_searching: bool,
    pub truncated: bool,
    pub error: Option<String>,
    pub preview: CodeCanvas,
    pub preview_path: String,
    pub scroll: Scroll,
    pub reveal: bool,
    /// When the search runs after typing stops.
    pub search_at: Option<Instant>,
    generation: u64,
    cancel: Option<CancelToken>,
    /// Files with results in the search under way (projects can share relative paths, hence the project).
    files: HashSet<(ProjectId, String)>,
    preview_key: Option<(ProjectId, String)>,
    preview_generation: u64,
}

impl Default for FindInPath {
    fn default() -> Self {
        let mut preview = CodeCanvas::new();
        preview.shows_revert_buttons = false;
        FindInPath {
            field: TextField::new(),
            mask: TextField::new(),
            options: GrepOptions::default(),
            all_projects: false,
            hits: Vec::new(),
            file_count: 0,
            selected: 0,
            is_searching: false,
            truncated: false,
            error: None,
            preview,
            preview_path: String::new(),
            scroll: Scroll::default(),
            reveal: false,
            search_at: None,
            generation: 0,
            cancel: None,
            files: HashSet::new(),
            preview_key: None,
            preview_generation: 0,
        }
    }
}

/// Finds the match on a result line so it can be emphasised.
pub fn matcher(pattern: &str, options: &GrepOptions) -> Option<regex_lite::Regex> {
    if pattern.is_empty() {
        return None;
    }
    let mut source = if options.regex { format!("(?:{pattern})") } else { regex_lite::escape(pattern) };
    if options.whole_word {
        source = format!(r"\b{source}\b");
    }
    if !options.case_sensitive {
        source = format!("(?i){source}");
    }
    regex_lite::Regex::new(&source).ok()
}

/// Drops indentation so matches line up in the result list, and starts long lines shortly before the match.
fn trim_leading(text: &str, found: Option<Range<usize>>) -> (String, Option<Range<usize>>) {
    let mut skip = text.bytes().take_while(|b| *b == b' ' || *b == b'\t').count();
    if let Some(f) = &found {
        if f.start < skip {
            skip = f.start;
        }
        if f.start > skip + 60 {
            skip = f.start - 40;
            while !text.is_char_boundary(skip) {
                skip -= 1;
            }
        }
    }
    if skip == 0 {
        return (text.to_string(), found);
    }
    (text[skip..].to_string(), found.map(|f| f.start - skip..f.end - skip))
}

impl App {
    pub fn show_find_in_path(&mut self) {
        // The selection in the viewer, if one line of it, seeds the query.
        let seed = self.viewer.canvas.selected_text().filter(|s| !s.contains('\n') && !s.trim().is_empty());
        let f = &mut self.find_in_path;
        if let Some(seed) = &seed {
            f.field.set_text(seed);
        }
        f.field.select_all();
        let research = seed.is_some() || (f.hits.is_empty() && !f.field.text.is_empty());
        self.present(Popup::FindInPath);
        self.focus_request = Some(super::ids::popup_field());
        if research {
            self.search_in_files();
        }
    }

    /// The query or an option changed: search again once typing pauses.
    pub fn find_in_path_changed(&mut self) {
        self.find_in_path.search_at = Some(Instant::now() + Duration::from_millis(120));
    }

    pub fn search_in_files(&mut self) {
        let f = &mut self.find_in_path;
        f.search_at = None;
        if let Some(c) = f.cancel.take() {
            c.cancel();
        }
        f.generation += 1;
        let token = f.generation;
        let pattern = f.field.text.clone();
        f.hits.clear();
        f.files.clear();
        f.file_count = 0;
        f.selected = 0;
        f.truncated = false;
        f.error = None;
        f.reveal = true;
        let long_enough = pattern.chars().count() >= 2 || (pattern.chars().count() == 1 && !pattern.is_ascii());
        if !long_enough {
            f.is_searching = false;
            self.set_find_preview(None);
            return;
        }
        f.options.file_mask = f.mask.text.trim().to_string();
        if f.options.regex && regex_lite::Regex::new(&pattern).is_err() {
            f.error = Some(l("Invalid regular expression", "正则表达式无效"));
            f.is_searching = false;
            return;
        }
        let targets: Vec<(ProjectId, std::path::PathBuf)> = if f.all_projects {
            self.projects.iter().map(|p| (p.id, p.url.clone())).collect()
        } else {
            self.current_project().map(|p| (p.id, p.url.clone())).into_iter().collect()
        };
        let options = self.find_in_path.options.clone();
        let cancel = CancelToken::new();
        self.find_in_path.cancel = Some(cancel.clone());
        self.find_in_path.is_searching = true;
        let poster = self.jobs.poster();
        self.jobs.spawn(
            move || {
                let highlighter = matcher(&pattern, &options);
                let mut total = 0;
                for (id, url) in targets {
                    if cancel.is_cancelled() || total >= FIND_LIMIT {
                        break;
                    }
                    let remaining = FIND_LIMIT - total;
                    // git grep streams results; each batch goes to the main thread and is shown at once.
                    let _ = git::grep(&pattern, &options, &url, remaining, Some(&cancel), |batch: Vec<GrepHit>| {
                        total += batch.len();
                        let converted: Vec<Hit> = batch
                            .into_iter()
                            .map(|h| {
                                let found = highlighter.as_ref().and_then(|r| r.find(&h.text)).map(|m| m.range()).filter(|r| !r.is_empty());
                                let (text, found) = trim_leading(&h.text, found);
                                Hit { project: id, path: h.path, line: h.line, text, found }
                            })
                            .collect();
                        poster.post(move |app: &mut App| app.append_hits(token, converted));
                    });
                }
            },
            move |app: &mut App, ()| {
                let f = &mut app.find_in_path;
                if f.generation != token {
                    return;
                }
                f.is_searching = false;
                f.truncated = f.hits.len() >= FIND_LIMIT;
                if f.hits.is_empty() {
                    app.set_find_preview(None);
                }
            },
        );
    }

    /// Adds a batch of results as it arrives.
    fn append_hits(&mut self, token: u64, batch: Vec<Hit>) {
        let f = &mut self.find_in_path;
        if f.generation != token {
            return;
        }
        let first = f.hits.is_empty();
        for hit in batch {
            f.files.insert((hit.project, hit.path.clone()));
            f.hits.push(hit);
        }
        f.file_count = f.files.len();
        if first && !f.hits.is_empty() {
            self.load_find_preview();
        }
    }

    pub fn move_find_selection(&mut self, delta: isize) {
        let f = &mut self.find_in_path;
        if f.hits.is_empty() {
            return;
        }
        f.selected = (f.selected as isize + delta).clamp(0, f.hits.len() as isize - 1) as usize;
        f.reveal = true;
        self.load_find_preview();
    }

    pub fn select_find_hit(&mut self, index: usize) {
        if index < self.find_in_path.hits.len() {
            self.find_in_path.selected = index;
            self.load_find_preview();
        }
    }

    pub fn activate_find_hit(&mut self) {
        let f = &self.find_in_path;
        let Some(hit) = f.hits.get(f.selected).cloned() else { return };
        if self.project(hit.project).is_none() {
            return;
        }
        self.dismiss_popup(false);
        self.open_file(hit.project, hit.path, Some(hit.line));
        self.focus_viewer();
    }

    fn set_find_preview(&mut self, doc: Option<diff::DiffDocument>) {
        let f = &mut self.find_in_path;
        if doc.is_none() {
            f.preview_path.clear();
            f.preview_key = None;
        }
        f.preview.set_document(doc, Mode::Single, false);
    }

    fn position_find_preview(&mut self) {
        let f = &mut self.find_in_path;
        let Some(hit) = f.hits.get(f.selected) else { return };
        let Some(doc) = f.preview.document() else { return };
        if !matches!(doc.content, Content::Text) {
            return;
        }
        let count = doc.right.len();
        if count == 0 {
            return;
        }
        let line = hit.line.saturating_sub(1).min(count - 1);
        // Every occurrence near the hit is emphasised, with the same matcher as the result list.
        let mut ranges: HashMap<usize, Vec<Range<usize>>> = HashMap::new();
        if let Some(regex) = matcher(&f.field.text, &f.options) {
            let lines = doc.right.lines();
            for (index, text) in lines.iter().enumerate().take((line + 200).min(count - 1) + 1).skip(line.saturating_sub(200)) {
                if let Some(m) = regex.find(text) {
                    ranges.insert(index, vec![m.range()]);
                }
            }
        }
        f.preview.focus_line = Some((Side::Right, line));
        f.preview.set_highlights([HashMap::new(), ranges], Vec::new(), None);
        f.preview.position_when_laid_out(InitialPosition::Line(line + 1));
    }

    pub fn load_find_preview(&mut self) {
        let f = &self.find_in_path;
        let Some(hit) = f.hits.get(f.selected).cloned() else { return };
        let Some(project) = self.project(hit.project) else { return };
        let prefix = if self.projects.len() > 1 && f.all_projects { format!("{} › ", project.name) } else { String::new() };
        let key = (hit.project, hit.path.clone());
        let url = project.url.clone();
        let f = &mut self.find_in_path;
        f.preview_path = format!("{prefix}{}", hit.path);
        if f.preview_key.as_ref() == Some(&key) && f.preview.document().is_some() {
            self.position_find_preview();
            return;
        }
        f.preview_generation += 1;
        let generation = f.preview_generation;
        let request = DiffRequest { root: url, path: hit.path, old_path: None, kind: None, source: Source::File, ignore_whitespace: false };
        self.jobs.spawn(
            move || diff::load(&request),
            move |app: &mut App, doc: diff::DiffDocument| {
                if app.find_in_path.preview_generation != generation {
                    return;
                }
                app.find_in_path.preview_key = Some(key);
                app.find_in_path.preview.set_document(Some(doc), Mode::Single, false);
                app.position_find_preview();
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_line_suffix() {
        assert_eq!(parse_query("Main.kt:42"), ("Main.kt".to_string(), Some(42)));
        assert_eq!(parse_query(" a:b "), ("a:b".to_string(), None));
        assert_eq!(parse_query("x:0"), ("x:0".to_string(), None));
    }

    #[test]
    fn marks_and_trims() {
        let options = GrepOptions { case_sensitive: false, whole_word: true, ..Default::default() };
        let r = matcher("foo", &options).unwrap();
        assert_eq!(r.find("    let Foo = foobar").map(|m| m.range()), Some(8..11));
        let (text, found) = trim_leading("    let Foo", Some(8..11));
        assert_eq!(text, "let Foo");
        assert_eq!(found, Some(4..7));
        let plain = matcher("a.b", &GrepOptions { case_sensitive: true, ..Default::default() }).unwrap();
        assert!(plain.find("axb").is_none());
    }
}
