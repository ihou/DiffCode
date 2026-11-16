//! Blame, file and line history, comparing with another commit, and the commit graph with its branches and tags.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::diff::{self, join_repo_path, Hunk};
use crate::git::{self, parse_commit, parse_log, parse_name_status, parse_numstat, ChangeKind, Commit, FileChange};
use crate::util::natural_cmp;

// MARK: - Blame

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BlameEntry {
    pub hash: String,
    pub author: String,
    pub email: String,
    /// Seconds since 1970.
    pub date: i64,
    pub summary: String,
}

impl BlameEntry {
    pub fn short_hash(&self) -> &str {
        &self.hash[..self.hash.len().min(7)]
    }
}

/// Who last changed each line of a file, from `git blame --porcelain`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Blame {
    pub entries: Vec<BlameEntry>,
    /// The entry of each line, in order; None for a line that is not committed yet.
    pub lines: Vec<Option<u32>>,
}

pub const UNCOMMITTED_HASH: &str = "0000000000000000000000000000000000000000";

/// `rev` None blames the file in the working tree, where lines not committed yet have no entry.
pub fn blame(path: &str, rev: Option<&str>, dir: &Path) -> git::Result<Blame> {
    let mut args = vec!["blame".to_string(), "--porcelain".into()];
    if let Some(rev) = rev {
        args.push(rev.to_string());
    }
    args.push("--".into());
    args.push(path.to_string());
    Ok(parse_blame(&git::run(dir, args)?))
}

/// Parses `git blame --porcelain`. Each line comes as a header (`<hash> <orig> <final> [<count>]`), the commit's
/// details the first time that commit appears, then the line itself after a tab.
pub fn parse_blame(data: &[u8]) -> Blame {
    let mut blame = Blame::default();
    let mut known: HashMap<&[u8], u32> = HashMap::new();
    let mut current: Option<u32> = None;
    let mut describing = false;
    let mut final_line = 0usize;
    let text = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
    let is_hex = |b: u8| b.is_ascii_digit() || (b'a'..=b'f').contains(&b);

    for line in data.split(|&b| b == b'\n') {
        if line.is_empty() {
            continue;
        }
        if line[0] == b'\t' {
            // The line itself.
            if final_line == 0 {
                continue;
            }
            if blame.lines.len() < final_line {
                blame.lines.resize(final_line, None);
            }
            blame.lines[final_line - 1] = current;
        } else if line.len() >= 41 && line[40] == b' ' && line[..40].iter().all(|&b| is_hex(b)) {
            // Header: hash, line in the original file, line in this one, [lines in the group].
            let hash = &line[..40];
            let mut fields = line[41..].split(|&b| b == b' ');
            fields.next();
            final_line = fields
                .next()
                .and_then(|f| std::str::from_utf8(f).ok())
                .and_then(|f| f.parse().ok())
                .unwrap_or(0);
            if hash == UNCOMMITTED_HASH.as_bytes() {
                current = None;
                describing = false;
            } else if let Some(&index) = known.get(hash) {
                current = Some(index);
                describing = false;
            } else {
                let index = blame.entries.len() as u32;
                known.insert(hash, index);
                blame.entries.push(BlameEntry { hash: text(hash), ..Default::default() });
                current = Some(index);
                describing = true;
            }
        } else if describing {
            let Some(index) = current else { continue };
            let entry = &mut blame.entries[index as usize];
            if let Some(rest) = line.strip_prefix(b"author ") {
                entry.author = text(rest);
            } else if let Some(rest) = line.strip_prefix(b"author-mail ") {
                let rest = rest.strip_prefix(b"<").unwrap_or(rest);
                let rest = rest.strip_suffix(b">").unwrap_or(rest);
                entry.email = text(rest);
            } else if let Some(rest) = line.strip_prefix(b"author-time ") {
                entry.date = text(rest).trim().parse().unwrap_or(0);
            } else if let Some(rest) = line.strip_prefix(b"summary ") {
                entry.summary = text(rest);
            }
        }
    }
    blame
}

// MARK: - File and line history

/// `--grep` arguments for a Log search, matched case-insensitively and literally.
pub fn grep_arguments(query: &str) -> Vec<String> {
    let q = query.trim();
    if q.is_empty() {
        Vec::new()
    } else {
        vec!["-i".into(), "--fixed-strings".into(), format!("--grep={q}")]
    }
}

/// Commits that changed `path`, newest first, following it through renames, with the path the file had in each.
/// `scope` is where the history starts: nothing for HEAD, `--all`, or a revision.
pub fn file_history(
    path: &str,
    dir: &Path,
    skip: usize,
    limit: usize,
    query: &str,
    scope: &[String],
) -> git::Result<(Vec<Commit>, HashMap<String, String>)> {
    let search = grep_arguments(query);
    let mut args: Vec<String> = vec![
        "log".into(),
        format!("--format=%x1e{}", git::LOG_FIELDS),
        "--decorate=full".into(),
        "--follow".into(),
        "--name-only".into(),
        "-z".into(),
    ];
    if search.is_empty() {
        args.push(format!("--skip={skip}"));
        args.push(format!("--max-count={limit}"));
    }
    args.extend(scope.iter().cloned());
    args.push("--".into());
    args.push(path.to_string());
    let (commits, paths) = parse_file_history(&git::cmd(dir, args).ok(&[0, 128]).run()?);
    if search.is_empty() {
        return Ok((commits, paths));
    }
    // `--follow` loses the file at a rename when `--grep` hides the commit that renamed it. So the whole history
    // is read, and git says which of its commits match, looking under every name the file had.
    let mut names: Vec<String> = paths.values().cloned().collect::<HashSet<_>>().into_iter().collect();
    if !names.iter().any(|n| n == path) {
        names.push(path.to_string());
    }
    names.sort();
    let mut args: Vec<String> = vec!["log".into(), "--format=%H".into()];
    args.extend(search);
    args.extend(scope.iter().cloned());
    args.push("--".into());
    args.extend(names);
    let out = git::cmd(dir, args).ok(&[0, 128]).run_string()?;
    let matching: HashSet<&str> = out.lines().collect();
    let commits = commits.into_iter().filter(|c| matching.contains(c.hash.as_str())).skip(skip).take(limit).collect();
    Ok((commits, paths))
}

/// Parses `git log --format=%x1e<fields> --name-only -z`: `\x1e<fields>\0\n<path>\0` per commit.
pub fn parse_file_history(data: &[u8]) -> (Vec<Commit>, HashMap<String, String>) {
    let mut commits = Vec::new();
    let mut paths = HashMap::new();
    for record in data.split(|&b| b == 0x1e).filter(|r| !r.is_empty()) {
        let nul = record.iter().position(|&b| b == 0).unwrap_or(record.len());
        let Some(commit) = parse_commit(&record[..nul]) else { continue };
        if nul < record.len() {
            let mut from = nul + 1;
            while from < record.len() && record[from] == b'\n' {
                from += 1;
            }
            let to = record[from..].iter().position(|&b| b == 0).map(|p| from + p).unwrap_or(record.len());
            if to > from {
                paths.insert(commit.hash.clone(), String::from_utf8_lossy(&record[from..to]).into_owned());
            }
        }
        commits.push(commit);
    }
    (commits, paths)
}

/// Commits that changed some lines of a file (`git log -L`), newest first.
/// `lines` are 1-based, inclusive, in the file as `rev` has it; `path` is its path there.
pub fn line_history(
    path: &str,
    lines: (usize, usize),
    rev: &str,
    dir: &Path,
    skip: usize,
    limit: usize,
    query: &str,
) -> git::Result<Vec<Commit>> {
    let mut args: Vec<String> = vec![
        "log".into(),
        format!("--format={}", git::LOG_FORMAT),
        "--decorate=full".into(),
        format!("--skip={skip}"),
        format!("--max-count={limit}"),
        "-s".into(),
        format!("-L{},{}:{}", lines.0, lines.1, path),
    ];
    args.extend(grep_arguments(query));
    // A hash or HEAD, never something that reads as an option.
    args.push(rev.to_string());
    Ok(parse_log(&git::cmd(dir, args).ok(&[0, 128]).run()?))
}

/// The history of some lines of a file in the working tree, counted in HEAD (`git log -L` needs lines of a
/// commit). None when HEAD does not have the file or none of the lines.
/// `path` is in the working tree, `head_path` its path in HEAD; `lines` are 0-based, inclusive, as on disk.
pub fn head_lines(path: &str, head_path: &str, lines: (usize, usize), dir: &Path) -> Option<(usize, usize)> {
    let new = std::fs::read(join_repo_path(dir, path)).ok()?;
    let old = git::blob("HEAD", head_path, dir)?;
    let hunks = diff::compute_hunks(Some(&old), Some(&new), false)?;
    old_lines_for_new(lines, &hunks, git::line_count_of(&old))
}

/// One commit, by any name git understands.
pub fn commit(rev: &str, dir: &Path) -> Option<Commit> {
    if rev.starts_with('-') {
        return None;
    }
    let data = git::run(
        dir,
        ["log".to_string(), "-1".into(), format!("--format={}", git::LOG_FORMAT), "--decorate=full".into(), rev.into(), "--".into()],
    )
    .ok()?;
    parse_log(&data).into_iter().next()
}

// MARK: - Comparing with another commit

/// The commit a revision names, or None when it names none.
pub fn resolve_commit(rev: &str, dir: &Path) -> Option<String> {
    let name = rev.trim();
    if name.is_empty() || name.starts_with('-') {
        return None;
    }
    let out = git::run_string(dir, ["rev-parse".to_string(), "--verify".into(), "--quiet".into(), format!("{name}^{{commit}}")])
        .ok()?;
    let hash = out.trim();
    (hash.len() == 40).then(|| hash.to_string())
}

/// Where two lines of history forked: the newest commit both contain.
pub fn merge_base(a: &str, b: &str, dir: &Path) -> Option<String> {
    if a.starts_with('-') || b.starts_with('-') {
        return None;
    }
    let out = git::run_string(dir, ["merge-base", a, b]).ok()?;
    let hash = out.trim();
    (hash.len() == 40).then(|| hash.to_string())
}

/// Everything that differs between commit `base` and the working tree: tracked files as `git diff <base>` sees
/// them (staged or not), and untracked files, which `base` cannot have. With `head`, what differs between the
/// two commits instead.
pub fn compare_files(base: &str, head: Option<&str>, dir: &Path) -> git::Result<Vec<FileChange>> {
    let mut range = vec![base.to_string()];
    if let Some(h) = head {
        range.push(h.to_string());
    }
    let args = |first: &[&str]| -> Vec<String> {
        let mut a: Vec<String> = first.iter().map(|s| s.to_string()).collect();
        a.extend(range.iter().cloned());
        a.push("--".into());
        a
    };
    let (names, stats, others) = std::thread::scope(|s| {
        let stats = s.spawn(|| git::run(dir, args(&["diff", "--numstat", "-z", "-M"])));
        let others = s.spawn(|| {
            if head.is_none() {
                git::run(dir, ["ls-files", "-z", "--others", "--exclude-standard"]).unwrap_or_default()
            } else {
                Vec::new()
            }
        });
        let names = git::run(dir, args(&["diff", "--name-status", "-z", "-M"]));
        (names, stats.join().ok().and_then(|r| r.ok()).unwrap_or_default(), others.join().unwrap_or_default())
    });
    let mut changes = parse_name_status(&names?);
    let stats = parse_numstat(&stats);
    for change in &mut changes {
        if let Some(&(a, d)) = stats.get(&change.path) {
            change.additions = Some(a);
            change.deletions = Some(d);
        }
    }
    let tracked: HashSet<String> = changes.iter().map(|c| c.path.clone()).collect();
    for path in crate::util::split_nul(&others) {
        if tracked.contains(&path) {
            continue;
        }
        let mut change = FileChange::new(path.clone(), None, ChangeKind::Untracked);
        if let Some(lines) = git::line_count(&join_repo_path(dir, &path)) {
            change.additions = Some(lines);
            change.deletions = Some(0);
        }
        changes.push(change);
    }
    changes.sort_by(|a, b| natural_cmp(&a.path, &b.path));
    Ok(changes)
}

/// The lines of the old text that `lines` of the new text stand for (0-based, inclusive): unchanged lines one for
/// one, changed lines the lines they replaced. None when all of them are new.
/// `hunks` are in order, as `git diff -U0` gives them.
pub fn old_lines_for_new(lines: (usize, usize), hunks: &[Hunk], old_count: usize) -> Option<(usize, usize)> {
    let (lo, hi) = (lines.0 as isize, lines.1 as isize);
    let mut low = isize::MAX;
    let mut high = isize::MIN;
    // The first new line after the hunks so far, and how far its old line is from it.
    let mut cursor: isize = 0;
    let mut delta: isize = 0;
    let unchanged = |end: isize, cursor: isize, delta: isize, low: &mut isize, high: &mut isize| {
        let from = lo.max(cursor);
        let to = hi.min(end);
        if from <= to {
            *low = (*low).min(from + delta);
            *high = (*high).max(to + delta);
        }
    };
    for h in hunks {
        let (ls, lc, rs, rc) = (h.left_start as isize, h.left_count as isize, h.right_start as isize, h.right_count as isize);
        unchanged(rs - 1, cursor, delta, &mut low, &mut high);
        if rc > 0 && lc > 0 && rs <= hi && rs + rc > lo {
            low = low.min(ls);
            high = high.max(ls + lc - 1);
        }
        cursor = rs + rc;
        delta = ls + lc - cursor;
    }
    unchanged(hi, cursor, delta, &mut low, &mut high);
    let low = low.max(0);
    let high = high.min(old_count as isize - 1);
    (low <= high).then_some((low as usize, high as usize))
}

// MARK: - Commit graph

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Span {
    /// From the top edge of lane `from` into the commit's dot.
    IntoNode,
    /// Straight through the row in lane `from` (`to` is the same lane).
    Through,
    /// From the commit's dot down to the bottom edge of lane `to`.
    OutOfNode,
}

/// A line inside one row. Lanes are columns; `from` is at the top or the middle, `to` at the middle or the bottom.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GraphSegment {
    pub span: Span,
    pub from: u16,
    pub to: u16,
    pub color: u32,
}

/// One row of the commit graph: the commit, the lane its dot sits in, and the lines that pass through the row.
///
/// Each row draws only its own slice of the lines, from its top edge to its bottom edge, so rows can be
/// rendered lazily and independently and still join up into continuous branches.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphRow {
    pub commit: Commit,
    pub column: u16,
    pub color: u32,
    pub segments: Vec<GraphSegment>,
    /// Lanes this row touches, so the graph column can be sized to the widest row.
    pub width: u16,
}

/// Assigns lanes to commits listed children-before-parents (`git log --date-order` or `--topo-order`).
///
/// A lane holds the hash of the commit it is waiting for. A commit takes the lane that waits for it
/// (or a free one when nothing does: a branch tip), hands that lane on to its first parent, and opens
/// lanes for any further parents. Lanes that waited for the same commit merge into its dot.
/// Lanes are never compacted, so a line that passes a row stays in its column.
pub fn graph_layout(commits: Vec<Commit>) -> Vec<GraphRow> {
    let mut lanes: Vec<Option<String>> = Vec::new();
    let mut colors: Vec<u32> = Vec::new();
    let mut next_color = 0u32;
    let mut rows = Vec::with_capacity(commits.len());

    fn free_lane(lanes: &mut Vec<Option<String>>, colors: &mut Vec<u32>) -> usize {
        if let Some(index) = lanes.iter().position(Option::is_none) {
            return index;
        }
        lanes.push(None);
        colors.push(0);
        lanes.len() - 1
    }

    for commit in commits {
        let before_count = lanes.len();
        let column = match lanes.iter().position(|l| l.as_deref() == Some(commit.hash.as_str())) {
            Some(waiting) => waiting,
            None => {
                let c = free_lane(&mut lanes, &mut colors);
                colors[c] = next_color;
                next_color += 1;
                c
            }
        };
        let color = colors[column];

        let mut segments = Vec::new();
        for (lane, hash) in lanes.iter().enumerate().take(before_count) {
            let Some(hash) = hash else { continue };
            let span = if *hash == commit.hash { Span::IntoNode } else { Span::Through };
            let to = if span == Span::IntoNode { column } else { lane };
            segments.push(GraphSegment { span, from: lane as u16, to: to as u16, color: colors[lane] });
        }

        // Every lane that was waiting for this commit ends here; its own lane carries on to the first parent.
        for lane in lanes.iter_mut() {
            if lane.as_deref() == Some(commit.hash.as_str()) {
                *lane = None;
            }
        }

        if let Some(first) = commit.parents.first() {
            if let Some(existing) = lanes.iter().position(|l| l.as_ref() == Some(first)) {
                // Another child already leads to this parent: join that line rather than drawing a second one.
                // The bend keeps this branch's colour until it meets the other line, so it reads as this branch.
                segments.push(GraphSegment { span: Span::OutOfNode, from: column as u16, to: existing as u16, color });
            } else {
                lanes[column] = Some(first.clone());
                colors[column] = color;
                segments.push(GraphSegment { span: Span::OutOfNode, from: column as u16, to: column as u16, color });
            }
            for parent in commit.parents.iter().skip(1) {
                if let Some(existing) = lanes.iter().position(|l| l.as_ref() == Some(parent)) {
                    segments.push(GraphSegment {
                        span: Span::OutOfNode,
                        from: column as u16,
                        to: existing as u16,
                        color: colors[existing],
                    });
                } else {
                    let lane = free_lane(&mut lanes, &mut colors);
                    lanes[lane] = Some(parent.clone());
                    colors[lane] = next_color;
                    next_color += 1;
                    segments.push(GraphSegment {
                        span: Span::OutOfNode,
                        from: column as u16,
                        to: lane as u16,
                        color: colors[lane],
                    });
                }
            }
        }

        while matches!(lanes.last(), Some(None)) {
            lanes.pop();
            colors.pop();
        }
        let width = before_count.max(lanes.len()).max(column + 1);
        rows.push(GraphRow { commit, column: column as u16, color, segments, width: width as u16 });
    }
    rows
}

// MARK: - Branches and tags

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RefKind {
    Local,
    Remote,
    Tag,
}

/// A branch or tag, as `git for-each-ref` reports it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct GitRef {
    /// Full: refs/heads/main.
    pub name: String,
    /// main, origin/main, v1.0.
    pub short_name: String,
    pub kind: RefKind,
    /// The commit it points at (peeled, for annotated tags).
    pub target: String,
    pub upstream: Option<String>,
    pub ahead: usize,
    pub behind: usize,
    /// The upstream branch no longer exists.
    pub upstream_gone: bool,
    pub is_head: bool,
    /// Seconds since 1970.
    pub date: i64,
}

const REF_FORMAT: &str = "%(refname)%1f%(refname:short)%1f%(objectname)%1f%(*objectname)%1f%(upstream:short)%1f%(upstream:track,nobracket)%1f%(HEAD)%1f%(committerdate:unix)%1f%(symref)%1e";

pub fn refs(dir: &Path) -> git::Result<Vec<GitRef>> {
    let data = git::run(
        dir,
        [
            "for-each-ref".to_string(),
            "--sort=-committerdate".into(),
            format!("--format={REF_FORMAT}"),
            "refs/heads".into(),
            "refs/remotes".into(),
            "refs/tags".into(),
        ],
    )?;
    Ok(parse_refs(&data))
}

pub fn parse_refs(data: &[u8]) -> Vec<GitRef> {
    let mut refs = Vec::new();
    for record in data.split(|&b| b == 0x1e).filter(|r| !r.is_empty()) {
        let fields: Vec<String> =
            record.split(|&b| b == 0x1f).map(|f| String::from_utf8_lossy(f).trim().to_string()).collect();
        if fields.len() < 9 || fields[0].is_empty() {
            continue;
        }
        // origin/HEAD and the like only point at another ref.
        if !fields[8].is_empty() {
            continue;
        }
        let kind = if fields[0].starts_with("refs/heads/") {
            RefKind::Local
        } else if fields[0].starts_with("refs/remotes/") {
            RefKind::Remote
        } else if fields[0].starts_with("refs/tags/") {
            RefKind::Tag
        } else {
            continue;
        };
        let mut r = GitRef {
            name: fields[0].clone(),
            short_name: fields[1].clone(),
            kind,
            target: if fields[3].is_empty() { fields[2].clone() } else { fields[3].clone() },
            upstream: (!fields[4].is_empty()).then(|| fields[4].clone()),
            ahead: 0,
            behind: 0,
            upstream_gone: false,
            is_head: fields[6] == "*",
            date: fields[7].parse().unwrap_or(0),
        };
        // "ahead 2, behind 1", "ahead 3", "gone"
        for piece in fields[5].split(',') {
            let words: Vec<&str> = piece.split_whitespace().collect();
            if words.first() == Some(&"gone") {
                r.upstream_gone = true;
            }
            if words.len() == 2 {
                if let Ok(count) = words[1].parse() {
                    match words[0] {
                        "ahead" => r.ahead = count,
                        "behind" => r.behind = count,
                        _ => {}
                    }
                }
            }
        }
        refs.push(r);
    }
    refs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(hash: &str, parents: &[&str]) -> Commit {
        Commit {
            hash: hash.into(),
            parents: parents.iter().map(|p| p.to_string()).collect(),
            author: String::new(),
            email: String::new(),
            date: 0,
            refs: Vec::new(),
            subject: String::new(),
            body: String::new(),
        }
    }

    #[test]
    fn parses_porcelain_blame() {
        let h1 = "a".repeat(40);
        let data = format!(
            "{h1} 1 1 2\nauthor Ann\nauthor-mail <ann@x>\nauthor-time 100\nsummary First\nfilename f\n\tline one\n\
             {h1} 2 2\n\tline two\n{UNCOMMITTED_HASH} 3 3 1\nauthor Not Committed Yet\n\tline three\n"
        );
        let b = parse_blame(data.as_bytes());
        assert_eq!(b.entries.len(), 1);
        assert_eq!(b.entries[0].author, "Ann");
        assert_eq!(b.entries[0].email, "ann@x");
        assert_eq!(b.entries[0].date, 100);
        assert_eq!(b.lines, vec![Some(0), Some(0), None]);
    }

    #[test]
    fn graph_lanes_for_a_merge() {
        // M merges B into A; both come from R.
        let rows = graph_layout(vec![c("M", &["A", "B"]), c("A", &["R"]), c("B", &["R"]), c("R", &[])]);
        assert_eq!(rows[0].column, 0);
        assert_eq!(rows[1].column, 0);
        assert_eq!(rows[2].column, 1);
        assert_eq!(rows[3].column, 0);
        // B's line joins A's lane on the way to R.
        assert!(rows[2].segments.iter().any(|s| s.span == Span::OutOfNode && s.from == 1 && s.to == 0));
        assert_eq!(rows[0].width, 2);
    }

    #[test]
    fn parses_refs() {
        let data = b"refs/heads/main\x1fmain\x1fabc\x1f\x1forigin/main\x1fahead 2, behind 1\x1f*\x1f100\x1f\x1e\n\
refs/remotes/origin/HEAD\x1forigin\x1fabc\x1f\x1f\x1f\x1f \x1f100\x1frefs/remotes/origin/main\x1e\n\
refs/tags/v1\x1fv1\x1ftagobj\x1fpeeled\x1f\x1f\x1f \x1f50\x1f\x1e";
        let refs = parse_refs(data);
        assert_eq!(refs.len(), 2);
        assert_eq!((refs[0].ahead, refs[0].behind, refs[0].is_head), (2, 1, true));
        assert_eq!(refs[1].kind, RefKind::Tag);
        assert_eq!(refs[1].target, "peeled");
    }

    #[test]
    fn maps_new_lines_to_old() {
        // old: a b c d ; new: a X c d e  (b → X, e added)
        let hunks = vec![Hunk::lines(1, 1, 1, 1), Hunk::lines(4, 0, 4, 1)];
        assert_eq!(old_lines_for_new((0, 2), &hunks, 4), Some((0, 2)));
        assert_eq!(old_lines_for_new((4, 4), &hunks, 4), None);
        assert_eq!(old_lines_for_new((3, 4), &hunks, 4), Some((3, 3)));
    }

    #[test]
    fn parses_file_history_paths() {
        let h = "b".repeat(40);
        let data = format!("\x1e{h}\x1f\x1fA\x1fa@x\x1f1\x1f\x1fRename\0\nnew/name.kt\0");
        let (commits, paths) = parse_file_history(data.as_bytes());
        assert_eq!(commits.len(), 1);
        assert_eq!(paths.get(&h).map(String::as_str), Some("new/name.kt"));
    }
}
