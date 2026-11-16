//! The few things DiffCode does to a working tree: roll changes back, move new files to the Recycle Bin,
//! and take either back. Everything else about the repository is read-only.

use std::collections::HashSet;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::diff::{self, join_repo_path, DiffRequest, Hunk, Source};
use crate::git::{self, ChangeKind, FileChange};
use crate::lang::l;
use crate::text_codec;

/// Why an operation did nothing, in words for the user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    pub message: String,
}

impl Failure {
    pub fn new(message: impl Into<String>) -> Self {
        Failure { message: message.into() }
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Failure {}

impl From<git::Error> for Failure {
    fn from(e: git::Error) -> Self {
        Failure::new(e.message())
    }
}

impl From<std::io::Error> for Failure {
    fn from(e: std::io::Error) -> Self {
        Failure::new(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Failure>;

// MARK: - Hunk patch

/// The byte range of every line, terminator included. Lines end at `\n`, as in `text_codec::split_lines`.
pub fn line_ranges(data: &[u8]) -> Vec<Range<usize>> {
    let mut ranges = Vec::with_capacity(data.len() / 32 + 1);
    let mut start = 0;
    for (index, &b) in data.iter().enumerate() {
        if b == b'\n' {
            ranges.push(start..index + 1);
            start = index + 1;
        }
    }
    if start < data.len() {
        ranges.push(start..data.len());
    }
    ranges
}

/// "\r\n" when the file uses it (or, having no line break at all, the other side does); otherwise "\n".
fn newline_style(new: &[u8], old: &[u8]) -> &'static [u8] {
    let has_crlf = |d: &[u8]| d.windows(2).any(|w| w == b"\r\n");
    if has_crlf(new) || (!new.contains(&b'\n') && has_crlf(old)) {
        b"\r\n"
    } else {
        b"\n"
    }
}

/// `new` with the lines of `hunk` put back the way they are in `old`. Works on bytes, so the lines that are not
/// touched keep their encoding, line endings and trailing-newline state exactly. None when the hunk does not fit.
pub fn revert_hunk_bytes(hunk: &Hunk, old: &[u8], new: &[u8]) -> Option<Vec<u8>> {
    if diff::is_utf16(old) || diff::is_utf16(new) {
        return None;
    }
    let old_lines = line_ranges(old);
    let new_lines = line_ranges(new);
    if hunk.left_end() > old_lines.len() || hunk.right_end() > new_lines.len() {
        return None;
    }
    let bytes = |data: &[u8], lines: &[Range<usize>], range: Range<usize>| -> Vec<u8> {
        if range.is_empty() {
            return Vec::new();
        }
        data[lines[range.start].start..lines[range.end - 1].end].to_vec()
    };
    let mut head = bytes(new, &new_lines, 0..hunk.right_start);
    let mut middle = bytes(old, &old_lines, hunk.left_start..hunk.left_end());
    let tail = bytes(new, &new_lines, hunk.right_end()..new_lines.len());

    // Only the last line of a file may lack its newline. A piece that is no longer last gets one,
    // in the style of the file it is joined to.
    let newline = newline_style(new, old);
    if !head.is_empty() && head.last() != Some(&b'\n') && !(middle.is_empty() && tail.is_empty()) {
        head.extend_from_slice(newline);
    }
    if !middle.is_empty() && middle.last() != Some(&b'\n') && !tail.is_empty() {
        middle.extend_from_slice(newline);
    }
    head.extend(middle);
    head.extend(tail);
    Some(head)
}

// MARK: - Records

/// Enough of a file's state to tell whether anything touched it since.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stamp {
    pub size: u64,
    pub modified: SystemTime,
}

/// Does not follow symbolic links: a link is a file in its own right here.
pub fn stamp(path: &Path) -> Option<Stamp> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    Some(Stamp { size: meta.len(), modified: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH) })
}

#[derive(Clone, Debug)]
pub enum Backup {
    /// A copy of the earlier contents.
    Copy(PathBuf),
    /// The file itself, moved to the Recycle Bin: undoing moves it back. None when it went there but could not be
    /// found again, so it cannot be undone (and undo says so rather than claim it came back).
    Recycled(Option<recycle::Item>),
}

#[derive(Clone, Debug)]
pub struct Entry {
    /// Repository-relative, with forward slashes.
    pub path: String,
    /// Where the earlier contents are kept; None when there was no file.
    pub backup: Option<Backup>,
    /// What the operation left at `path`; None when it left nothing.
    pub after: Option<Stamp>,
}

/// What one operation changed, kept so that it can be undone.
#[derive(Clone, Debug, Default)]
pub struct Record {
    pub root: PathBuf,
    pub entries: Vec<Entry>,
    /// Paths to stage again when undoing: those that were staged as new or renamed files.
    pub restage_present: Vec<String>,
    /// The old names of renames, staged as deleted again when undoing.
    pub restage_absent: Vec<String>,
    /// Where this record's copies live.
    pub directory: Option<PathBuf>,
}

impl Record {
    pub fn new(root: &Path) -> Self {
        Record { root: root.to_path_buf(), ..Default::default() }
    }

    pub fn discard(&self) {
        if let Some(dir) = &self.directory {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

#[derive(Debug)]
pub enum HunkResult {
    Reverted(Record),
    /// The file is no longer what the diff on screen was made from; nothing was changed.
    Stale,
}

pub fn make_backup_directory() -> Result<PathBuf> {
    let dir = diff::scratch_directory().join("undo").join(diff::unique_name());
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Runs `git <arguments> -- <paths>` in as many batches as the command line needs. Paths are taken literally:
/// a file called `*.kt` names itself, not every Kotlin file. `env` is added to git's own (`GIT_INDEX_FILE`).
pub fn git_paths(arguments: &[&str], paths: &[String], root: &Path, env: &[(&str, &str)]) -> Result<()> {
    // Windows limits a command line to 32767 UTF-16 units; stay well under it.
    const LIMIT: usize = if cfg!(windows) { 24_000 } else { 60_000 };
    let mut batch: Vec<&str> = Vec::new();
    let mut bytes = 0;
    let flush = |batch: &mut Vec<&str>, bytes: &mut usize| -> Result<()> {
        if batch.is_empty() {
            return Ok(());
        }
        let mut args: Vec<String> = vec!["--literal-pathspecs".into()];
        args.extend(arguments.iter().map(|s| s.to_string()));
        args.push("--".into());
        args.extend(batch.iter().map(|s| s.to_string()));
        let mut c = git::cmd(root, args);
        for (k, v) in env {
            c = c.env(k, *v);
        }
        c.run()?;
        batch.clear();
        *bytes = 0;
        Ok(())
    };
    for path in paths {
        batch.push(path);
        bytes += path.len() + 3;
        if bytes > LIMIT {
            flush(&mut batch, &mut bytes)?;
        }
    }
    flush(&mut batch, &mut bytes)
}

/// Which of `paths` HEAD has. Asked just before acting, because the list on screen may be a moment old
/// and "added" or "untracked" is only safe to act on while it is still true.
fn paths_in_head(paths: &[String], root: &Path) -> HashSet<String> {
    let mut found = HashSet::new();
    for chunk in paths.chunks(400) {
        let mut args: Vec<String> =
            ["--literal-pathspecs", "ls-tree", "-r", "-z", "--name-only", "HEAD", "--"].iter().map(|s| s.to_string()).collect();
        args.extend(chunk.iter().cloned());
        // Fails in a repository with no commits yet, where HEAD has nothing.
        if let Ok(data) = git::run(root, args) {
            found.extend(crate::util::split_nul(&data));
        }
    }
    found
}

fn copy_into(source: &Path, directory: &Path, name: &str) -> Result<PathBuf> {
    let copy = directory.join(name);
    copy_file(source, &copy)?;
    Ok(copy)
}

/// Copies a file, or a symbolic link as a link.
fn copy_file(source: &Path, target: &Path) -> std::io::Result<()> {
    let meta = std::fs::symlink_metadata(source)?;
    if meta.file_type().is_symlink() {
        #[cfg(unix)]
        std::os::unix::fs::symlink(std::fs::read_link(source)?, target)?;
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(std::fs::read_link(source)?, target)?;
    } else {
        std::fs::copy(source, target)?;
    }
    Ok(())
}

// MARK: - Rollback

/// Puts `changes` back to what HEAD has. Files HEAD does not have (added, never committed) are taken
/// out of the index and left on disk; untracked files and submodules are not touched.
///
/// Fails when nothing could be done. If git fails part of the way, what was done is returned
/// together with the error, so that it can still be undone.
pub fn rollback(changes: &[FileChange], root: &Path) -> Result<(Record, Option<Failure>)> {
    let targets: Vec<&FileChange> =
        changes.iter().filter(|c| c.kind != ChangeKind::Untracked && !c.is_submodule).collect();
    if targets.is_empty() {
        return Err(Failure::new(l("Nothing here can be rolled back.", "这里没有可以回滚的内容。")));
    }
    let mut asked: Vec<String> = Vec::new();
    for c in &targets {
        asked.push(c.path.clone());
        if let Some(old) = &c.old_path {
            asked.push(old.clone());
        }
    }
    let in_head = paths_in_head(&asked, root);

    let mut restore: Vec<String> = Vec::new(); // brought back from HEAD, in the index and on disk
    let mut unstage: Vec<String> = Vec::new(); // taken out of the index, left on disk
    let mut remove: Vec<String> = Vec::new(); // files that go away: the new name of a rename
    let mut record = Record::new(root);
    for change in targets {
        match &change.old_path {
            Some(old) if change.kind == ChangeKind::Renamed && in_head.contains(old) => {
                restore.push(old.clone());
                record.restage_absent.push(old.clone());
                if in_head.contains(&change.path) {
                    restore.push(change.path.clone());
                } else {
                    unstage.push(change.path.clone());
                    remove.push(change.path.clone());
                    record.restage_present.push(change.path.clone());
                }
            }
            _ if in_head.contains(&change.path) => restore.push(change.path.clone()),
            _ => {
                unstage.push(change.path.clone());
                record.restage_present.push(change.path.clone());
            }
        }
    }

    // Keep a copy of everything about to be overwritten or removed.
    let directory = make_backup_directory()?;
    record.directory = Some(directory.clone());
    for (index, path) in restore.iter().chain(remove.iter()).enumerate() {
        let full = join_repo_path(root, path);
        let backup = if stamp(&full).is_some() {
            Some(Backup::Copy(copy_into(&full, &directory, &index.to_string())?))
        } else {
            None
        };
        record.entries.push(Entry { path: path.clone(), backup, after: None });
    }

    let failure = (|| -> Result<()> {
        git_paths(&["checkout", "HEAD"], &restore, root, &[])?;
        git_paths(&["rm", "--cached", "-f", "-q", "--ignore-unmatch"], &unstage, root, &[])?;
        for path in &remove {
            std::fs::remove_file(join_repo_path(root, path))?;
        }
        Ok(())
    })()
    .err();
    for entry in &mut record.entries {
        entry.after = stamp(&join_repo_path(root, &entry.path));
    }
    Ok((record, failure))
}

// MARK: - One hunk

/// Puts one hunk of a working-tree file back to the base revision's lines.
///
/// `expected_left` and `expected_right` are the two sides of the diff as shown. If either file reads
/// differently now, nothing is written and `Stale` is returned, so a click never lands on lines
/// other than the ones that were on screen.
pub fn revert_hunk(
    request: &DiffRequest,
    hunk: &Hunk,
    expected_left: &[String],
    expected_right: &[String],
) -> Result<HunkResult> {
    let Source::Worktree { base } = &request.source else {
        return Err(Failure::new(l("Only local changes can be rolled back.", "只有本地变更可以回滚。")));
    };
    let root = &request.root;
    let path = request.full_path();
    let is_file = std::fs::symlink_metadata(&path).map(|m| m.file_type().is_file()).unwrap_or(false);
    if !is_file {
        return Ok(HunkResult::Stale);
    }
    let Ok(new) = std::fs::read(&path) else { return Ok(HunkResult::Stale) };
    let Some(old) = git::blob(base, request.old_path.as_deref().unwrap_or(&request.path), root) else {
        return Ok(HunkResult::Stale);
    };
    if diff::is_utf16(&old) || diff::is_utf16(&new) {
        return Err(Failure::new(l("UTF-16 files can only be rolled back as a whole.", "UTF-16 文件只能整个回滚。")));
    }
    let lines = |d: &[u8]| text_codec::split_lines(&text_codec::decode(d));
    if lines(&old) != expected_left
        || lines(&new) != expected_right
        || line_ranges(&old).len() != expected_left.len()
        || line_ranges(&new).len() != expected_right.len()
    {
        return Ok(HunkResult::Stale);
    }
    let Some(result) = revert_hunk_bytes(hunk, &old, &new) else { return Ok(HunkResult::Stale) };
    if result == new {
        return Ok(HunkResult::Stale);
    }

    let directory = make_backup_directory()?;
    let copy = copy_into(&path, &directory, "0")?;
    let mut record = Record::new(root);
    record.entries.push(Entry { path: request.path.clone(), backup: Some(Backup::Copy(copy)), after: None });
    record.directory = Some(directory);

    // Written in place: the file keeps its permissions, and whatever is watching it sees an edit, not a replacement.
    write_in_place(&path, &result)?;

    // With the last hunk gone the file is HEAD's again; drop anything still staged for it, so it leaves the list.
    if result == old && request.old_path.is_none() && base == "HEAD" {
        let _ = git_paths(&["checkout", "HEAD"], std::slice::from_ref(&request.path), root, &[]);
    }
    record.entries[0].after = stamp(&path);
    Ok(HunkResult::Reverted(record))
}

/// Overwrites a file's contents without replacing the file: new bytes over the old, then cut to length, so at no
/// moment is the file empty.
pub fn write_in_place(path: &Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new().write(true).open(path)?;
    file.write_all(data)?;
    file.set_len(data.len() as u64)?;
    file.sync_data().ok();
    Ok(())
}

// MARK: - Delete

/// Moves new files (untracked, or added and never committed) to the Recycle Bin.
/// Returns what was moved and the paths that could not be.
pub fn trash(changes: &[FileChange], root: &Path) -> Result<(Record, Vec<String>)> {
    let candidates: Vec<&FileChange> =
        changes.iter().filter(|c| matches!(c.kind, ChangeKind::Untracked | ChangeKind::Added)).collect();
    // A file that has been committed in the meantime is no longer a new file; it stays.
    let in_head = paths_in_head(&candidates.iter().map(|c| c.path.clone()).collect::<Vec<_>>(), root);
    let targets: Vec<&FileChange> = candidates.iter().copied().filter(|c| !in_head.contains(&c.path)).collect();
    let mut failed: Vec<String> = candidates.iter().filter(|c| in_head.contains(&c.path)).map(|c| c.path.clone()).collect();
    let mut record = Record::new(root);
    let staged: Vec<String> = targets.iter().filter(|c| c.kind == ChangeKind::Added).map(|c| c.path.clone()).collect();
    git_paths(&["rm", "--cached", "-f", "-q", "--ignore-unmatch"], &staged, root, &[])?;
    record.restage_present = staged;

    let existing: Vec<(String, PathBuf)> = targets
        .iter()
        .map(|c| (c.path.clone(), join_repo_path(root, &c.path)))
        .filter(|(_, full)| stamp(full).is_some())
        .collect();
    let full_paths: Vec<PathBuf> = existing.iter().map(|(_, f)| f.clone()).collect();
    let moved = recycle::delete(&full_paths);
    for ((path, _), item) in existing.into_iter().zip(moved) {
        match item {
            Ok(item) => record.entries.push(Entry { path, backup: Some(Backup::Recycled(item)), after: None }),
            Err(_) => failed.push(path),
        }
    }
    Ok((record, failed))
}

// MARK: - Undo

/// Takes an operation back. A file that has changed since the operation is left as it is and counted as skipped.
/// Returns the new stamp of every restored path (None where the file is gone again) and the number skipped.
pub fn undo(record: &Record) -> (Vec<(String, Option<Stamp>)>, usize) {
    let mut restored = Vec::new();
    let mut skipped = 0;
    for entry in &record.entries {
        let path = join_repo_path(&record.root, &entry.path);
        if stamp(&path) != entry.after {
            skipped += 1;
            continue;
        }
        let outcome = (|| -> std::io::Result<bool> {
            match &entry.backup {
                Some(Backup::Recycled(None)) => Ok(false),
                Some(Backup::Recycled(Some(item))) => {
                    if let Some(parent) = path.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    Ok(recycle::restore(item, &path) && stamp(&path).is_some())
                }
                backup => {
                    if entry.after.is_some() {
                        std::fs::remove_file(&path)?;
                    }
                    if let Some(Backup::Copy(copy)) = backup {
                        if let Some(parent) = path.parent() {
                            std::fs::create_dir_all(parent)?;
                        }
                        copy_file(copy, &path)?;
                        // A copy keeps the old modification time; build tools need to see that the file changed.
                        if let Ok(f) = std::fs::OpenOptions::new().write(true).open(&path) {
                            let _ = f.set_modified(SystemTime::now());
                        }
                    }
                    Ok(true)
                }
            }
        })();
        match outcome {
            Ok(true) => restored.push((entry.path.clone(), stamp(&path))),
            _ => skipped += 1,
        }
    }
    let present: Vec<String> =
        record.restage_present.iter().filter(|p| stamp(&join_repo_path(&record.root, p)).is_some()).cloned().collect();
    let absent: Vec<String> =
        record.restage_absent.iter().filter(|p| stamp(&join_repo_path(&record.root, p)).is_none()).cloned().collect();
    let all: Vec<String> = present.into_iter().chain(absent).collect();
    let _ = git_paths(&["add", "-A"], &all, &record.root, &[]);
    (restored, skipped)
}

/// The system's Recycle Bin. On Windows the real one, with restore; elsewhere (the development harness) a folder
/// in the scratch directory standing in for it.
pub mod recycle {
    use std::path::{Path, PathBuf};

    #[cfg(windows)]
    #[derive(Clone, Debug)]
    pub struct Item(trash::TrashItem);

    #[cfg(not(windows))]
    #[derive(Clone, Debug)]
    pub struct Item(PathBuf);

    /// Moves each path to the Recycle Bin. Per path: the item to restore it from (None when it was recycled but
    /// cannot be found there again, so it cannot be undone), or an error when it was not moved.
    #[cfg(windows)]
    pub fn delete(paths: &[PathBuf]) -> Vec<Result<Option<Item>, String>> {
        let key = |p: &Path| {
            let s = p.to_string_lossy().replace('/', "\\");
            s.strip_prefix(r"\\?\").unwrap_or(&s).to_lowercase()
        };
        // The Recycle Bin remembers the full long path; the one given may be relative to a short (8.3) name such as
        // C:\Users\RUNNER~1, so it is resolved while the file is still there.
        let wanted: Vec<String> = paths.iter().map(|p| key(&std::fs::canonicalize(p).unwrap_or_else(|_| p.clone()))).collect();
        let results: Vec<Result<(), String>> =
            paths.iter().map(|p| trash::delete(p).map_err(|e| e.to_string())).collect();
        if results.iter().all(|r| r.is_err()) {
            return results.into_iter().map(|r| r.map(|_| None)).collect();
        }
        let listed = trash::os_limited::list().unwrap_or_default();
        wanted
            .into_iter()
            .zip(results)
            .map(|(wanted, r)| {
                r.map(|_| {
                    listed
                        .iter()
                        .filter(|item| key(&item.original_path()) == wanted)
                        .max_by_key(|item| item.time_deleted)
                        .cloned()
                        .map(Item)
                })
            })
            .collect()
    }

    #[cfg(windows)]
    pub fn restore(item: &Item, _to: &Path) -> bool {
        trash::os_limited::restore_all([item.0.clone()]).is_ok()
    }

    #[cfg(not(windows))]
    pub fn delete(paths: &[PathBuf]) -> Vec<Result<Option<Item>, String>> {
        let bin = crate::diff::scratch_directory().join("recycle");
        paths
            .iter()
            .map(|path| {
                let place = bin.join(crate::diff::unique_name());
                std::fs::create_dir_all(&place).map_err(|e| e.to_string())?;
                let target = place.join(path.file_name().unwrap_or_default());
                std::fs::rename(path, &target).map_err(|e| e.to_string())?;
                Ok(Some(Item(target)))
            })
            .collect()
    }

    #[cfg(not(windows))]
    pub fn restore(item: &Item, to: &Path) -> bool {
        std::fs::rename(&item.0, to).is_ok()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        sh(root, &["init", "-q", "-b", "main"]);
        sh(root, &["config", "user.email", "t@example.com"]);
        sh(root, &["config", "user.name", "t"]);
        sh(root, &["config", "commit.gpgsign", "false"]);
        // These fixtures assert exact bytes, independent of the user's Windows Git settings.
        sh(root, &["config", "core.autocrlf", "false"]);
        dir
    }

    pub fn sh(root: &Path, args: &[&str]) -> String {
        git::cmd(root, args.iter().copied()).run_string().unwrap()
    }

    #[test]
    fn reverts_one_hunk_by_bytes() {
        let old = b"a\r\nb\r\nc\r\n";
        let new = b"a\r\nB\r\nc\r\nd";
        let back = revert_hunk_bytes(&Hunk::lines(1, 1, 1, 1), old, new).unwrap();
        assert_eq!(back, b"a\r\nb\r\nc\r\nd");
        // Removing the last line that lacked a newline: the piece before it keeps its own.
        let back = revert_hunk_bytes(&Hunk::lines(3, 0, 3, 1), old, new).unwrap();
        assert_eq!(back, b"a\r\nB\r\nc\r\n");
        assert_eq!(line_ranges(b"x\ny"), vec![0..2, 2..3]);
    }

    #[test]
    fn rollback_and_undo() {
        if !git::is_available() {
            return;
        }
        let dir = repo();
        let root = dir.path();
        std::fs::write(root.join("a.txt"), "one\n").unwrap();
        sh(root, &["add", "."]);
        sh(root, &["commit", "-q", "-m", "init"]);
        std::fs::write(root.join("a.txt"), "two\n").unwrap();
        std::fs::write(root.join("new.txt"), "fresh\n").unwrap();
        sh(root, &["add", "new.txt"]);
        let status = git::status(root, None).unwrap();
        let (record, failure) = rollback(&status.changes, root).unwrap();
        assert!(failure.is_none());
        assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), "one\n");
        assert!(root.join("new.txt").exists(), "added files stay on disk");
        let after = git::status(root, None).unwrap();
        assert_eq!(after.changes.len(), 1);
        assert_eq!(after.changes[0].kind, ChangeKind::Untracked);

        let (restored, skipped) = undo(&record);
        assert_eq!(skipped, 0);
        assert_eq!(restored.len(), 1);
        assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), "two\n");
        let again = git::status(root, None).unwrap();
        assert!(again.changes.iter().any(|c| c.path == "new.txt" && c.kind == ChangeKind::Added));
        record.discard();
    }

    #[test]
    fn revert_one_hunk_in_the_working_tree() {
        if !git::is_available() {
            return;
        }
        let dir = repo();
        let root = dir.path();
        std::fs::write(root.join("f.txt"), "1\n2\n3\n4\n5\n").unwrap();
        sh(root, &["add", "."]);
        sh(root, &["commit", "-q", "-m", "init"]);
        std::fs::write(root.join("f.txt"), "1\nTWO\n3\n4\nFIVE\n").unwrap();
        let request = DiffRequest {
            root: root.to_path_buf(),
            path: "f.txt".into(),
            old_path: None,
            kind: Some(ChangeKind::Modified),
            source: Source::Worktree { base: "HEAD".into() },
            ignore_whitespace: false,
        };
        let doc = diff::load(&request);
        assert_eq!(doc.hunks().len(), 2);
        let result =
            revert_hunk(&request, &doc.hunks()[0], doc.left.lines(), doc.right.lines()).unwrap();
        let HunkResult::Reverted(record) = result else { panic!("stale") };
        assert_eq!(std::fs::read_to_string(root.join("f.txt")).unwrap(), "1\n2\n3\n4\nFIVE\n");
        // The same click again, on what is now an out-of-date diff, changes nothing.
        let stale = revert_hunk(&request, &doc.hunks()[0], doc.left.lines(), doc.right.lines()).unwrap();
        assert!(matches!(stale, HunkResult::Stale));
        undo(&record);
        assert_eq!(std::fs::read_to_string(root.join("f.txt")).unwrap(), "1\nTWO\n3\n4\nFIVE\n");
    }

    #[test]
    fn trash_and_restore() {
        if !git::is_available() {
            return;
        }
        let dir = repo();
        let root = dir.path();
        std::fs::write(root.join("keep.txt"), "k\n").unwrap();
        sh(root, &["add", "."]);
        sh(root, &["commit", "-q", "-m", "init"]);
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("sub/new.txt"), "n\n").unwrap();
        let status = git::status(root, None).unwrap();
        let (record, failed) = trash(&status.changes, root).unwrap();
        assert!(failed.is_empty());
        assert!(!root.join("sub/new.txt").exists());
        assert!(matches!(record.entries[0].backup, Some(Backup::Recycled(Some(_)))), "lost in the Recycle Bin: {:?}", record.entries);
        let (_, skipped) = undo(&record);
        assert_eq!(skipped, 0);
        assert_eq!(std::fs::read_to_string(root.join("sub/new.txt")).unwrap(), "n\n");
    }

    #[test]
    fn undo_skips_what_the_recycle_bin_lost() {
        let dir = tempfile::tempdir().unwrap();
        let mut record = Record::new(dir.path());
        record.entries.push(Entry { path: "gone.txt".into(), backup: Some(Backup::Recycled(None)), after: None });
        let (restored, skipped) = undo(&record);
        assert!(restored.is_empty());
        assert_eq!(skipped, 1);
    }
}
