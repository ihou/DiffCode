//! The projects as a tree: a repository inside another one listed sits under it, and so does a linked work tree
//! under its repository, wherever its folder is. Worked out from the paths and from what git says of work trees,
//! so the list stays a plain list of folders and nothing has to be kept in step.

use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Row {
    /// Where the project is in the list the rows were made from.
    pub index: usize,
    pub depth: usize,
    pub has_children: bool,
    /// The project it sits under, if any (an index like `index`).
    pub parent: Option<usize>,
}

/// A path as compared here: one kind of slash, none at the end, and no difference of case (Windows has none).
pub fn key(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/").trim_end_matches('/').to_lowercase()
}

fn inside(path: &str, folder: &str) -> bool {
    path.len() > folder.len() + 1 && path.starts_with(folder) && path.as_bytes()[folder.len()] == b'/'
}

/// For each path, the index of the nearest listed folder that contains it; a work tree's repository (`owners`, by
/// index) comes before where its folder is.
pub fn parents(keys: &[String], owners: &[Option<usize>]) -> Vec<Option<usize>> {
    (0..keys.len())
        .map(|i| match owners.get(i).copied().flatten() {
            Some(owner) if owner != i => Some(owner),
            _ => (0..keys.len()).filter(|j| *j != i && inside(&keys[i], &keys[*j])).max_by_key(|j| keys[*j].len()),
        })
        .collect()
}

/// The rows to show, top to bottom: each project followed by its work trees and then the repositories inside it.
/// What is inside a collapsed project is left out.
pub fn rows(keys: &[String], owners: &[Option<usize>], collapsed: &[String]) -> Vec<Row> {
    let parents = parents(keys, owners);
    type Ctx<'a> = (&'a [String], &'a [Option<usize>], &'a [Option<usize>], &'a [String]);
    fn add(index: usize, depth: usize, ctx: Ctx, rows: &mut Vec<Row>) {
        let (keys, parents, owners, collapsed) = ctx;
        // A repository's own work trees first, then the repositories inside it; each kind in the list's order.
        let is_worktree = |i: &usize| owners.get(*i).copied().flatten().is_some();
        let inside: Vec<usize> = (0..keys.len()).filter(|i| parents[*i] == Some(index)).collect();
        let children: Vec<usize> =
            inside.iter().copied().filter(|i| is_worktree(i)).chain(inside.iter().copied().filter(|i| !is_worktree(i))).collect();
        rows.push(Row { index, depth, has_children: !children.is_empty(), parent: parents[index] });
        if collapsed.contains(&keys[index]) {
            return;
        }
        for child in children {
            add(child, depth + 1, ctx, rows);
        }
    }
    let mut rows = Vec::new();
    for root in (0..keys.len()).filter(|i| parents[*i].is_none()) {
        add(root, 0, (keys, &parents, owners, collapsed), &mut rows);
    }
    rows
}

/// The projects above one, nearest first.
pub fn ancestors(index: usize, parents: &[Option<usize>]) -> Vec<usize> {
    let mut found = Vec::new();
    let mut at = parents[index];
    while let Some(up) = at {
        if found.len() > parents.len() {
            break;
        }
        found.push(up);
        at = parents[up];
    }
    found
}

/// Everything under a project, however deep (indices), given each one's parent.
pub fn descendants(index: usize, parents: &[Option<usize>]) -> Vec<usize> {
    (0..parents.len()).filter(|i| ancestors(*i, parents).contains(&index)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys() -> Vec<String> {
        ["C:\\w\\ws", "C:\\w\\other", "C:\\w\\WS\\api", "C:\\w\\ws\\web", "C:\\w\\ws\\api\\services\\core", "C:\\w\\ws-2"]
            .iter()
            .map(|p| key(Path::new(p)))
            .collect()
    }

    #[test]
    fn a_repository_inside_another_sits_under_it() {
        let keys = keys();
        // A name that merely starts the same is no parent, and the nearest folder is.
        assert_eq!(parents(&keys, &[]), vec![None, None, Some(0), Some(0), Some(2), None]);
        let shown: Vec<(usize, usize)> = rows(&keys, &[], &[]).iter().map(|r| (r.index, r.depth)).collect();
        assert_eq!(shown, vec![(0, 0), (2, 1), (4, 2), (3, 1), (1, 0), (5, 0)]);
    }

    #[test]
    fn a_collapsed_project_hides_what_is_inside() {
        let keys = keys();
        let shown = |collapsed: &[&str]| -> Vec<usize> {
            let collapsed: Vec<String> = collapsed.iter().map(|p| key(Path::new(p))).collect();
            rows(&keys, &[], &collapsed).iter().map(|r| r.index).collect()
        };
        assert_eq!(shown(&["C:\\w\\ws\\api"]), vec![0, 2, 3, 1, 5]);
        assert_eq!(shown(&["C:\\w\\ws"]), vec![0, 1, 5]);
        assert!(rows(&keys, &[], &[key(Path::new("C:\\w\\ws"))])[0].has_children);
        assert_eq!(descendants(0, &parents(&keys, &[])), vec![2, 3, 4]);
        assert_eq!(ancestors(4, &parents(&keys, &[])), vec![2, 0]);
    }

    /// A linked work tree sits under its repository wherever its folder is: beside it, or inside another project.
    #[test]
    fn a_work_tree_sits_under_its_repository() {
        let keys: Vec<String> = ["C:\\w\\ws", "C:\\w\\ws\\api", "D:\\elsewhere\\fix", "C:\\w\\ws\\api\\.claude\\worktrees\\task"]
            .iter()
            .map(|p| key(Path::new(p)))
            .collect();
        let owners = [None, None, Some(1), Some(0)];
        assert_eq!(parents(&keys, &owners), vec![None, Some(0), Some(1), Some(0)]);
        let shown = |collapsed: &[String]| -> Vec<usize> { rows(&keys, &owners, collapsed).iter().map(|r| r.index).collect() };
        // A repository's work trees come before the repositories inside it.
        assert_eq!(shown(&[]), vec![0, 3, 1, 2]);
        assert_eq!(shown(&[keys[1].clone()]), vec![0, 3, 1]);
    }
}
