//! A repository's files as a tree, and the rows of it that are visible.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use crate::core::fuzzy::Ranked;
use crate::core::git::FileChange;
use crate::core::util::natural_cmp;

/// One visible row of the project file tree.
#[derive(Clone, Debug, PartialEq)]
pub struct TreeRow {
    pub id: String,
    pub path: String,
    /// A file's name, or a directory's label ("app/src/main" for a chain of single-child directories).
    pub name: String,
    pub depth: usize,
    pub is_directory: bool,
    pub expanded: bool,
    pub change: Option<FileChange>,
    /// Changed files somewhere below this directory.
    pub changed_below: usize,
    /// Bytes of `path` the filter matched.
    pub positions: Vec<usize>,
}

impl TreeRow {
    pub fn file_id(path: &str) -> String {
        format!("f:{path}")
    }

    pub fn directory_id(path: &str) -> String {
        format!("d:{path}")
    }

    /// The path of the row this one hangs under ("" at the top level).
    pub fn parent_path(&self) -> &str {
        if self.path.len() > self.name.len() {
            &self.path[..self.path.len() - self.name.len() - 1]
        } else {
            ""
        }
    }
}

struct Directory {
    name: String,
    path: String,
    directories: Vec<usize>,
    files: Vec<u32>,
    sorted: bool,
}

/// Built once per file listing, off the main thread; directories are sorted the first time they are shown.
pub struct FileTreeIndex {
    pub paths: Vec<String>,
    pub set: HashSet<String>,
    dirs: RefCell<Vec<Directory>>,
    by_path: HashMap<String, usize>,
}

fn name_of(path: &str) -> &str {
    path.rsplit_once('/').map_or(path, |(_, n)| n)
}

impl FileTreeIndex {
    pub fn new(paths: Vec<String>) -> Self {
        let mut dirs = vec![Directory { name: String::new(), path: String::new(), directories: Vec::new(), files: Vec::new(), sorted: false }];
        let mut by_path: HashMap<String, usize> = HashMap::new();
        by_path.insert(String::new(), 0);
        fn ensure(path: &str, dirs: &mut Vec<Directory>, by_path: &mut HashMap<String, usize>) -> usize {
            if let Some(&i) = by_path.get(path) {
                return i;
            }
            let (parent, name) = match path.rsplit_once('/') {
                Some((p, n)) => (ensure(p, dirs, by_path), n),
                None => (0, path),
            };
            let i = dirs.len();
            dirs.push(Directory { name: name.to_string(), path: path.to_string(), directories: Vec::new(), files: Vec::new(), sorted: false });
            dirs[parent].directories.push(i);
            by_path.insert(path.to_string(), i);
            i
        }
        // `git ls-files` lists a directory's files together: remembering the last one skips most lookups.
        let mut last_path = String::new();
        let mut last = 0;
        for (index, path) in paths.iter().enumerate() {
            let dir = path.rsplit_once('/').map_or("", |(d, _)| d);
            if dir != last_path {
                last = ensure(dir, &mut dirs, &mut by_path);
                last_path = dir.to_string();
            }
            dirs[last].files.push(index as u32);
        }
        let set = paths.iter().cloned().collect();
        FileTreeIndex { paths, set, dirs: RefCell::new(dirs), by_path }
    }

    fn sort(&self, i: usize) {
        let mut dirs = self.dirs.borrow_mut();
        if dirs[i].sorted {
            return;
        }
        dirs[i].sorted = true;
        let mut subs = std::mem::take(&mut dirs[i].directories);
        subs.sort_by(|a, b| natural_cmp(&dirs[*a].name, &dirs[*b].name));
        dirs[i].directories = subs;
        let paths = &self.paths;
        dirs[i].files.sort_by(|a, b| natural_cmp(name_of(&paths[*a as usize]), name_of(&paths[*b as usize])));
    }

    pub fn has_directory(&self, path: &str) -> bool {
        self.by_path.contains_key(path)
    }
}

/// The rows visible with the `expanded` directories open.
pub fn rows(index: &FileTreeIndex, expanded: &HashSet<String>, changes: &[FileChange]) -> Vec<TreeRow> {
    let mut by_path: HashMap<&str, &FileChange> = HashMap::new();
    let mut below: HashMap<&str, usize> = HashMap::new();
    for c in changes {
        by_path.insert(&c.path, c);
        let mut p = c.path.as_str();
        while let Some((dir, _)) = p.rsplit_once('/') {
            *below.entry(dir).or_default() += 1;
            p = dir;
        }
    }
    let mut out = Vec::new();
    emit(index, 0, 0, expanded, &by_path, &below, &mut out);
    out
}

fn emit(
    index: &FileTreeIndex,
    dir: usize,
    depth: usize,
    expanded: &HashSet<String>,
    changes: &HashMap<&str, &FileChange>,
    below: &HashMap<&str, usize>,
    out: &mut Vec<TreeRow>,
) {
    index.sort(dir);
    let (subs, files) = {
        let dirs = index.dirs.borrow();
        (dirs[dir].directories.clone(), dirs[dir].files.clone())
    };
    for child in subs {
        // Fold "a" → "b" → "c" into one row when the middle directories hold nothing else.
        let (node, label, path) = {
            let dirs = index.dirs.borrow();
            let mut node = child;
            let mut label = dirs[child].name.clone();
            while dirs[node].files.is_empty() && dirs[node].directories.len() == 1 {
                node = dirs[node].directories[0];
                label = format!("{label}/{}", dirs[node].name);
            }
            (node, label, dirs[node].path.clone())
        };
        let open = expanded.contains(&path);
        out.push(TreeRow {
            id: TreeRow::directory_id(&path),
            changed_below: below.get(path.as_str()).copied().unwrap_or(0),
            path,
            name: label,
            depth,
            is_directory: true,
            expanded: open,
            change: None,
            positions: Vec::new(),
        });
        if open {
            emit(index, node, depth + 1, expanded, changes, below, out);
        }
    }
    for f in files {
        let path = &index.paths[f as usize];
        out.push(TreeRow {
            id: TreeRow::file_id(path),
            path: path.clone(),
            name: name_of(path).to_string(),
            depth,
            is_directory: false,
            expanded: false,
            change: changes.get(path.as_str()).map(|c| (*c).clone()),
            changed_below: 0,
            positions: Vec::new(),
        });
    }
}

/// The flat list shown while a filter is typed: best matches first.
pub fn match_rows(matches: &[Ranked], changes: &[FileChange]) -> Vec<TreeRow> {
    let by_path: HashMap<&str, &FileChange> = changes.iter().map(|c| (c.path.as_str(), c)).collect();
    matches
        .iter()
        .map(|m| TreeRow {
            id: TreeRow::file_id(&m.path),
            path: m.path.clone(),
            name: name_of(&m.path).to_string(),
            depth: 0,
            is_directory: false,
            expanded: false,
            change: by_path.get(m.path.as_str()).map(|c| (*c).clone()),
            changed_below: 0,
            positions: m.positions.clone(),
        })
        .collect()
}

/// Every directory that must be open for `path` to show: "a", "a/b" for "a/b/File.kt".
pub fn ancestors(path: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = path;
    while let Some((dir, _)) = rest.rsplit_once('/') {
        out.push(dir.to_string());
        rest = dir;
    }
    out.reverse();
    out
}

/// The directory rows above `index` that enclose it, outermost first.
pub fn enclosing(rows: &[TreeRow], index: usize) -> Vec<TreeRow> {
    if index >= rows.len() {
        return Vec::new();
    }
    let mut chain = Vec::new();
    let mut depth = rows[index].depth;
    let mut cursor = index as isize - 1;
    while depth > 0 && cursor >= 0 {
        let r = &rows[cursor as usize];
        if r.depth < depth {
            chain.push(r.clone());
            depth = r.depth;
        }
        cursor -= 1;
    }
    chain.reverse();
    chain
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folds_single_child_directories() {
        let index = FileTreeIndex::new(vec!["a/b/c/File.kt".into(), "a/b/c/Other.kt".into(), "README.md".into(), "z/x.txt".into()]);
        let rows = rows(&index, &HashSet::new(), &[]);
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["a/b/c", "z", "README.md"]);
        let open: HashSet<String> = ["a/b/c".to_string()].into_iter().collect();
        let rows2 = super::rows(&index, &open, &[]);
        assert_eq!(rows2.len(), 5);
        assert_eq!(rows2[1].parent_path(), "a/b/c");
        assert_eq!(ancestors("a/b/File.kt"), ["a", "a/b"]);
    }
}
