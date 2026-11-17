//! The projects as a tree: a repository inside another one listed sits under it. Worked out from the paths alone,
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

/// For each path, the index of the nearest listed folder that contains it.
pub fn parents(keys: &[String]) -> Vec<Option<usize>> {
    (0..keys.len())
        .map(|i| (0..keys.len()).filter(|j| *j != i && inside(&keys[i], &keys[*j])).max_by_key(|j| keys[*j].len()))
        .collect()
}

/// The rows to show, top to bottom: each project followed by those inside it, in the list's own order. What is
/// inside a collapsed project is left out.
pub fn rows(keys: &[String], collapsed: &[String]) -> Vec<Row> {
    let parents = parents(keys);
    fn add(index: usize, depth: usize, keys: &[String], parents: &[Option<usize>], collapsed: &[String], rows: &mut Vec<Row>) {
        let children: Vec<usize> = (0..keys.len()).filter(|i| parents[*i] == Some(index)).collect();
        rows.push(Row { index, depth, has_children: !children.is_empty(), parent: parents[index] });
        if collapsed.contains(&keys[index]) {
            return;
        }
        for child in children {
            add(child, depth + 1, keys, parents, collapsed, rows);
        }
    }
    let mut rows = Vec::new();
    for root in (0..keys.len()).filter(|i| parents[*i].is_none()) {
        add(root, 0, keys, &parents, collapsed, &mut rows);
    }
    rows
}

/// Everything under a path, however deep (indices).
pub fn descendants(index: usize, keys: &[String]) -> Vec<usize> {
    (0..keys.len()).filter(|i| *i != index && inside(&keys[*i], &keys[index])).collect()
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
        assert_eq!(parents(&keys), vec![None, None, Some(0), Some(0), Some(2), None]);
        let shown: Vec<(usize, usize)> = rows(&keys, &[]).iter().map(|r| (r.index, r.depth)).collect();
        assert_eq!(shown, vec![(0, 0), (2, 1), (4, 2), (3, 1), (1, 0), (5, 0)]);
    }

    #[test]
    fn a_collapsed_project_hides_what_is_inside() {
        let keys = keys();
        let shown = |collapsed: &[&str]| -> Vec<usize> {
            let collapsed: Vec<String> = collapsed.iter().map(|p| key(Path::new(p))).collect();
            rows(&keys, &collapsed).iter().map(|r| r.index).collect()
        };
        assert_eq!(shown(&["C:\\w\\ws\\api"]), vec![0, 2, 3, 1, 5]);
        assert_eq!(shown(&["C:\\w\\ws"]), vec![0, 1, 5]);
        assert!(rows(&keys, &[key(Path::new("C:\\w\\ws"))])[0].has_children);
        assert_eq!(descendants(0, &keys), vec![2, 3, 4]);
    }
}
