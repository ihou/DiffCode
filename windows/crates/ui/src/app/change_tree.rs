//! The flattened, collapsible tree of the Changes list and a commit's file list.

use std::collections::{BTreeMap, HashSet};

use crate::core::git::{ChangeKind, FileChange};
use crate::core::lang::l;
use crate::core::util::natural_cmp;

#[derive(Clone, Debug, PartialEq)]
pub enum RowKind {
    Group { title: String, count: usize },
    Directory { name: String, count: usize },
    File(FileChange),
}

/// One visible row: a group header, a directory, or a file.
#[derive(Clone, Debug, PartialEq)]
pub struct ChangeRow {
    pub id: String,
    pub depth: usize,
    pub kind: RowKind,
    pub expanded: bool,
}

impl ChangeRow {
    pub fn file(&self) -> Option<&FileChange> {
        match &self.kind {
            RowKind::File(f) => Some(f),
            _ => None,
        }
    }

    pub fn file_id(path: &str) -> String {
        format!("f:{path}")
    }
}

#[derive(Default)]
struct Node {
    children: BTreeMap<String, Node>,
    files: Vec<FileChange>,
    total: usize,
}

/// `split_unversioned`: untracked files under their own "Unversioned Files" header.
pub fn rows(changes: &[FileChange], flat: bool, collapsed: &HashSet<String>, filter: &str, split_unversioned: bool) -> Vec<ChangeRow> {
    let needle = filter.trim().to_lowercase();
    let visible: Vec<&FileChange> =
        if needle.is_empty() { changes.iter().collect() } else { changes.iter().filter(|c| c.path.to_lowercase().contains(&needle)).collect() };
    if !split_unversioned {
        return build(&visible, "c", 0, flat, collapsed);
    }
    let tracked: Vec<&FileChange> = visible.iter().copied().filter(|c| c.kind != ChangeKind::Untracked).collect();
    let untracked: Vec<&FileChange> = visible.iter().copied().filter(|c| c.kind == ChangeKind::Untracked).collect();
    let mut out = Vec::new();
    for (key, title, list) in [("changes", l("Changes", "变更"), tracked), ("unversioned", l("Unversioned Files", "未跟踪文件"), untracked)] {
        if list.is_empty() {
            continue;
        }
        let id = format!("g:{key}");
        let expanded = !collapsed.contains(&id);
        out.push(ChangeRow { id, depth: 0, kind: RowKind::Group { title, count: list.len() }, expanded });
        if expanded {
            out.extend(build(&list, key, 1, flat, collapsed));
        }
    }
    out
}

fn by_name(a: &FileChange, b: &FileChange) -> std::cmp::Ordering {
    natural_cmp(a.name(), b.name())
}

fn build(changes: &[&FileChange], group: &str, depth: usize, flat: bool, collapsed: &HashSet<String>) -> Vec<ChangeRow> {
    if flat {
        let mut sorted: Vec<&FileChange> = changes.to_vec();
        sorted.sort_by(|a, b| by_name(a, b));
        return sorted
            .into_iter()
            .map(|c| ChangeRow { id: ChangeRow::file_id(&c.path), depth, kind: RowKind::File(c.clone()), expanded: true })
            .collect();
    }
    let mut root = Node::default();
    for change in changes {
        let mut node = &mut root;
        node.total += 1;
        let parts: Vec<&str> = change.path.split('/').filter(|p| !p.is_empty()).collect();
        for part in &parts[..parts.len().saturating_sub(1)] {
            node = node.children.entry(part.to_string()).or_default();
            node.total += 1;
        }
        node.files.push((*change).clone());
    }
    let mut out = Vec::new();
    emit(&root, "", group, depth, collapsed, &mut out);
    out
}

fn emit(node: &Node, path: &str, group: &str, depth: usize, collapsed: &HashSet<String>, out: &mut Vec<ChangeRow>) {
    let mut names: Vec<&String> = node.children.keys().collect();
    names.sort_by(|a, b| natural_cmp(a, b));
    for name in names {
        let mut child = &node.children[name];
        // Chains of single-child directories share one row ("app/src/main").
        let mut label = name.clone();
        let mut child_path = if path.is_empty() { name.clone() } else { format!("{path}/{name}") };
        while child.files.is_empty() && child.children.len() == 1 {
            let (only_name, only) = child.children.iter().next().unwrap();
            label = format!("{label}/{only_name}");
            child_path = format!("{child_path}/{only_name}");
            child = only;
        }
        let id = format!("d:{group}:{child_path}");
        let expanded = !collapsed.contains(&id);
        out.push(ChangeRow { id, depth, kind: RowKind::Directory { name: label, count: child.total }, expanded });
        if expanded {
            emit(child, &child_path, group, depth + 1, collapsed, out);
        }
    }
    let mut files: Vec<&FileChange> = node.files.iter().collect();
    files.sort_by(|a, b| by_name(a, b));
    for f in files {
        out.push(ChangeRow { id: ChangeRow::file_id(&f.path), depth, kind: RowKind::File(f.clone()), expanded: true });
    }
}

/// Files in display order, ignoring what is collapsed.
pub fn ordered_files(changes: &[FileChange], flat: bool, filter: &str, split_unversioned: bool) -> Vec<FileChange> {
    rows(changes, flat, &HashSet::new(), filter, split_unversioned).into_iter().filter_map(|r| r.file().cloned()).collect()
}

/// The files a set of rows stands for (file rows, and everything under folder and group rows), in the order of
/// `changes`, each once.
pub fn files_for(ids: &HashSet<String>, changes: &[FileChange]) -> Vec<FileChange> {
    let mut paths = HashSet::new();
    let mut groups = HashSet::new();
    let mut folders: Vec<(String, String)> = Vec::new();
    for id in ids {
        if let Some(p) = id.strip_prefix("f:") {
            paths.insert(p.to_string());
        } else if let Some(g) = id.strip_prefix("g:") {
            groups.insert(g.to_string());
        } else if let Some((group, dir)) = directory_id(id) {
            folders.push((group.to_string(), format!("{dir}/")));
        }
    }
    changes
        .iter()
        .filter(|c| {
            if paths.contains(&c.path) {
                return true;
            }
            // "c" is the single group of a commit's file list.
            let group = if c.kind == ChangeKind::Untracked { "unversioned" } else { "changes" };
            groups.contains(group) || folders.iter().any(|(g, prefix)| (g == group || g == "c") && c.path.starts_with(prefix.as_str()))
        })
        .cloned()
        .collect()
}

/// "d:<group>:<dir>" → (group, dir).
fn directory_id(id: &str) -> Option<(&str, &str)> {
    let rest = id.strip_prefix("d:")?;
    rest.split_once(':')
}

/// Collapsed rows that hide `path`, so it can be revealed.
pub fn ancestors(path: &str, collapsed: &HashSet<String>) -> Vec<String> {
    collapsed
        .iter()
        .filter(|id| {
            if id.starts_with("g:") {
                return true;
            }
            directory_id(id).is_some_and(|(_, dir)| path.starts_with(&format!("{dir}/")))
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fc(path: &str, kind: ChangeKind) -> FileChange {
        FileChange::new(path, None, kind)
    }

    #[test]
    fn compacts_and_groups() {
        let changes = vec![
            fc("app/src/main/A.kt", ChangeKind::Modified),
            fc("app/src/main/B.kt", ChangeKind::Added),
            fc("README.md", ChangeKind::Modified),
            fc("new.txt", ChangeKind::Untracked),
        ];
        let rows = rows(&changes, false, &HashSet::new(), "", true);
        let ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["g:changes", "d:changes:app/src/main", "f:app/src/main/A.kt", "f:app/src/main/B.kt", "f:README.md", "g:unversioned", "f:new.txt"]);
        match &rows[1].kind {
            RowKind::Directory { name, count } => {
                assert_eq!(name, "app/src/main");
                assert_eq!(*count, 2);
            }
            _ => panic!(),
        }
        let picked = files_for(&["d:changes:app/src/main".to_string()].into_iter().collect(), &changes);
        assert_eq!(picked.len(), 2);
        let hidden = ancestors("app/src/main/A.kt", &["d:changes:app/src/main".to_string()].into_iter().collect());
        assert_eq!(hidden.len(), 1);
    }
}
