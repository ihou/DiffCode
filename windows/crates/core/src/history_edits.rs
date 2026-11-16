//! Writing history the way an IDE's Log and branch menus do: cherry-pick, revert, reset, merge, rebase, deleting and
//! renaming branches, force push, and the stash. Each is git's own command; DiffCode only remembers enough to take
//! it back.

use std::collections::HashSet;
use std::path::Path;

use crate::git::{self, parse_log, parse_numstat, ChangeKind, Commit, FileChange, EMPTY_TREE, LOG_FORMAT};
use crate::history::compare_files;
use crate::lang::l;
use crate::lf;
use crate::remote::RepoOperation;
use crate::util::natural_cmp;
use crate::working_tree::{Failure, Result};

/// How far a reset goes, as git's own modes put it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ResetMode {
    /// The commits after the target come back as staged changes.
    Soft,
    /// They come back as local changes (the default).
    Mixed,
    /// They and every local change to tracked files are thrown away.
    Hard,
    /// They are thrown away; local changes stay, and git refuses if one is in a file those commits touched.
    Keep,
}

impl ResetMode {
    pub const ALL: [ResetMode; 4] = [ResetMode::Soft, ResetMode::Mixed, ResetMode::Hard, ResetMode::Keep];

    pub fn flag(self) -> &'static str {
        match self {
            ResetMode::Soft => "--soft",
            ResetMode::Mixed => "--mixed",
            ResetMode::Hard => "--hard",
            ResetMode::Keep => "--keep",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            ResetMode::Soft => "Soft",
            ResetMode::Mixed => "Mixed",
            ResetMode::Hard => "Hard",
            ResetMode::Keep => "Keep",
        }
    }

    pub fn detail(self) -> String {
        match self {
            ResetMode::Soft => l(
                "The changes of the commits after it come back staged; local changes stay as they are.",
                "其后提交的改动回到暂存区；本地改动保持不变。",
            ),
            ResetMode::Mixed => l(
                "The changes of the commits after it come back as local changes, with yours.",
                "其后提交的改动变回本地改动，和你现有的改动放在一起。",
            ),
            ResetMode::Hard => l(
                "The commits after it and every local change to tracked files are thrown away. New files stay.",
                "其后的提交和已跟踪文件的所有本地改动都会丢弃。新文件保留。",
            ),
            ResetMode::Keep => l(
                "The commits after it are thrown away; your local changes stay. Refused if they touch the same files.",
                "其后的提交会丢弃，本地改动保留。如果两者改了同一个文件，git 会拒绝。",
            ),
        }
    }
}

/// A move of the branch (or of a detached HEAD) that DiffCode made, and how to take it back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeadMove {
    /// The branch that moved; None for a detached HEAD.
    pub branch: Option<String>,
    pub before: String,
    pub after: String,
    /// How the branch goes back: as the reset was done; `Keep` after a merge, rebase, cherry-pick or revert.
    pub mode: ResetMode,
    /// What a hard reset threw away of the local changes, kept as a stash commit that is in no list (`stash create`).
    pub snapshot: Option<String>,
}

const EDITOR: [(&str, &str); 1] = [("GIT_EDITOR", "true")];

fn run_with(dir: &Path, args: Vec<String>, env: &[(&str, &str)]) -> git::Result<Vec<u8>> {
    let mut c = git::cmd(dir, args);
    for (k, v) in env {
        c = c.env(k, *v);
    }
    c.run()
}

fn strings(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

pub use crate::commit::head;

/// The checked-out branch's name; None for a detached HEAD.
pub fn current_branch(dir: &Path) -> Option<String> {
    let out = git::run_string(dir, ["symbolic-ref", "--quiet", "--short", "HEAD"]).unwrap_or_default();
    let name = out.trim();
    (!name.is_empty()).then(|| name.to_string())
}

/// Where HEAD is now, as a move starting there will record it.
fn position(dir: &Path) -> Result<(Option<String>, String)> {
    Ok((current_branch(dir), head(dir)?))
}

/// Whether `ancestor` is in the history of `descendant` (a commit is in its own).
pub fn is_ancestor(ancestor: &str, descendant: &str, dir: &Path) -> bool {
    git::run(dir, ["merge-base", "--is-ancestor", ancestor, descendant]).is_ok()
}

/// The remote branches that have `commit` in their history ("origin/main"); empty when it was never pushed.
pub fn remote_branches_containing(commit: &str, dir: &Path) -> Vec<String> {
    let out = git::run_string(dir, ["branch", "-r", "--format=%(refname:short)", "--contains", commit]).unwrap_or_default();
    out.lines().map(str::trim).filter(|s| !s.is_empty() && !s.ends_with("/HEAD")).map(str::to_string).collect()
}

/// The commits `revisions` select ("a..b", or "x", "--not", "--branches"), newest first.
pub fn commits(revisions: &[&str], dir: &Path, limit: usize) -> Vec<Commit> {
    let mut args = vec!["log".to_string(), format!("--format={LOG_FORMAT}"), format!("--max-count={limit}")];
    args.extend(revisions.iter().map(|s| s.to_string()));
    args.push("--".into());
    git::run(dir, args).map(|d| parse_log(&d)).unwrap_or_default()
}

/// How many commits `revisions` select; 0 when git cannot tell.
pub fn count(revisions: &[&str], dir: &Path) -> usize {
    let mut args = strings(&["rev-list", "--count"]);
    args.extend(revisions.iter().map(|s| s.to_string()));
    args.push("--".into());
    git::run_string(dir, args).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0)
}

/// Of `hashes`, those whose changes HEAD already has as another commit (cherry-picked before, or rebased).
pub fn already_applied(hashes: &[String], dir: &Path) -> HashSet<String> {
    let mut applied = HashSet::new();
    for hash in hashes.iter().take(50) {
        // Lists just `hash`, marked "-" when HEAD has a commit with the same changes.
        let out = git::run_string(dir, ["cherry".to_string(), "HEAD".into(), hash.clone(), format!("{hash}^")])
            .unwrap_or_default();
        if out.starts_with('-') {
            applied.insert(hash.clone());
        }
    }
    applied
}

// MARK: Cherry-pick and revert

/// Applies `hashes` on top of HEAD as new commits, in the order given (oldest first).
pub fn cherry_pick(hashes: &[String], dir: &Path) -> Result<HeadMove> {
    let (branch, before) = position(dir)?;
    let mut args = strings(&["cherry-pick"]);
    args.extend(hashes.iter().cloned());
    run_with(dir, args, &EDITOR)?;
    Ok(HeadMove { branch, before, after: head(dir)?, mode: ResetMode::Keep, snapshot: None })
}

/// Adds commits that undo `hashes`, in the order given (newest first), each with git's "Revert …" message.
pub fn revert(hashes: &[String], dir: &Path) -> Result<HeadMove> {
    let (branch, before) = position(dir)?;
    let mut args = strings(&["revert", "--no-edit"]);
    args.extend(hashes.iter().cloned());
    run_with(dir, args, &EDITOR)?;
    Ok(HeadMove { branch, before, after: head(dir)?, mode: ResetMode::Keep, snapshot: None })
}

/// Leaves out the commit the operation stopped at and goes on with the rest (rebase, cherry-pick, revert).
pub fn skip_operation(operation: RepoOperation, dir: &Path) -> Result<()> {
    run_with(dir, strings(&[operation.command(), "--skip"]), &EDITOR)?;
    Ok(())
}

// MARK: Reset

/// Moves the current branch to `target`. Before a hard reset, the local changes it throws away are kept as a
/// commit in no list, so that the reset can be taken back with them.
pub fn reset(target: &str, mode: ResetMode, dir: &Path) -> Result<HeadMove> {
    let (branch, before) = position(dir)?;
    let mut snapshot = None;
    if mode == ResetMode::Hard {
        let out = git::run_string(dir, ["stash", "create"])?;
        let out = out.trim();
        snapshot = (!out.is_empty()).then(|| out.to_string());
    }
    git::run(dir, ["reset", "--quiet", mode.flag(), target])?;
    Ok(HeadMove { branch, before, after: head(dir)?, mode, snapshot })
}

/// Takes back a move of the branch made here. Refuses once HEAD has moved on, and when going back would drop
/// commits that were pushed since. Local changes made meanwhile stay (git refuses rather than overwrite them).
pub fn undo_head_move(m: &HeadMove, dir: &Path) -> Result<()> {
    match position(dir) {
        Ok((branch, now)) if branch == m.branch && now == m.after => {}
        _ => return Err(Failure::new(l("HEAD has moved since, so it was left alone.", "之后 HEAD 又变了，所以没有撤销。"))),
    }
    // Going back drops the commits `before` does not have (none, after a reset to an older commit).
    if !is_ancestor(&m.after, &m.before, dir) && !remote_branches_containing(&m.after, dir).is_empty() {
        return Err(Failure::new(l(
            "The new commits have been pushed since, so it was left alone.",
            "新的提交已经推送到远程，所以没有撤销。",
        )));
    }
    // A hard reset left a clean working tree; anything in it now is new, and `keep` keeps it.
    let mode = if m.mode == ResetMode::Hard { ResetMode::Keep } else { m.mode };
    git::run(dir, ["reset", "--quiet", mode.flag(), &m.before])?;
    if let Some(snapshot) = &m.snapshot {
        if git::run(dir, ["stash", "apply", "--quiet", snapshot]).is_err() {
            return Err(Failure::new(lf!(
                "The branch is back, but the local changes the reset threw away clash with yours. They are kept: git stash apply {snapshot}",
                "分支已恢复，但重置时丢弃的本地改动与你现在的改动冲突。它们仍然保留着：git stash apply {snapshot}"
            )));
        }
    }
    Ok(())
}

// MARK: Merge and rebase

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MergeOutcome {
    UpToDate,
    FastForward,
    Merged,
}

/// Merges `reference` into the current branch with git's own message. Stops at conflicts (an error, with the merge
/// in progress). With `autostash`, local changes in the way are put aside and back.
pub fn merge(reference: &str, autostash: bool, dir: &Path) -> Result<(MergeOutcome, HeadMove)> {
    let (branch, before) = position(dir)?;
    let mut args = strings(&["merge", "--no-edit"]);
    if autostash {
        args.push("--autostash".into());
    }
    // git's message names the branch as given: "Merge branch 'feature'", not 'refs/heads/feature'.
    args.push(shortest_name(reference, dir));
    run_with(dir, args, &[("GIT_EDITOR", "true"), ("GIT_MERGE_AUTOEDIT", "no")])?;
    let after = head(dir)?;
    let outcome = if after == before {
        MergeOutcome::UpToDate
    } else {
        let line = git::run_string(dir, ["rev-list", "--parents", "-n", "1", &after])?;
        if line.split_whitespace().count() > 2 {
            MergeOutcome::Merged
        } else {
            MergeOutcome::FastForward
        }
    };
    Ok((outcome, HeadMove { branch, before, after, mode: ResetMode::Keep, snapshot: None }))
}

/// Replays the current branch's own commits on top of `reference`. Stops at conflicts (an error, with the rebase in
/// progress). With `autostash`, local changes are put aside and back (git rebases nothing with them in place).
pub fn rebase(reference: &str, autostash: bool, dir: &Path) -> Result<HeadMove> {
    let (branch, before) = position(dir)?;
    let mut args = strings(&["rebase"]);
    if autostash {
        args.push("--autostash".into());
    }
    args.push(reference.to_string());
    run_with(dir, args, &EDITOR)?;
    Ok(HeadMove { branch, before, after: head(dir)?, mode: ResetMode::Keep, snapshot: None })
}

/// "feature" for refs/heads/feature, "origin/x" for refs/remotes/origin/x, unless git would take the short
/// name for something else (a tag of the same name): then the full one.
pub fn shortest_name(full_name: &str, dir: &Path) -> String {
    let Some(prefix) = ["refs/heads/", "refs/remotes/", "refs/tags/"].into_iter().find(|p| full_name.starts_with(p))
    else {
        return full_name.to_string();
    };
    let short = &full_name[prefix.len()..];
    if short.is_empty() || short.starts_with('-') {
        return full_name.to_string();
    }
    let resolved = git::run_string(dir, ["rev-parse", "--symbolic-full-name", short]).unwrap_or_default();
    if resolved.trim() == full_name {
        short.to_string()
    } else {
        full_name.to_string()
    }
}

/// git turned an operation down because of local changes (and not because of conflicts it ran into).
pub fn refused_for_local_changes(message: &str) -> bool {
    [
        "would be overwritten by",
        "commit your changes or stash them",
        "Please commit or stash them",
        "cannot rebase: You have unstaged changes",
        "cannot rebase: Your index contains uncommitted changes",
    ]
    .iter()
    .any(|m| message.contains(m))
}

// MARK: Branches

/// Deletes a local branch. Without `force`, git refuses one whose commits are on no other branch.
pub fn delete_branch(name: &str, force: bool, dir: &Path) -> Result<()> {
    git::run(dir, ["branch", if force { "-D" } else { "-d" }, name])?;
    Ok(())
}

/// git refused to delete a branch because its commits would be lost.
pub fn is_not_fully_merged(message: &str) -> bool {
    message.contains("not fully merged")
}

pub fn rename_branch(name: &str, new_name: &str, dir: &Path) -> Result<()> {
    git::run(dir, ["branch", "-m", name, new_name])?;
    Ok(())
}

/// Recreates a deleted local branch where it was, following what it followed.
pub fn restore_branch(name: &str, hash: &str, upstream: Option<&str>, dir: &Path) -> Result<()> {
    git::run(dir, ["branch", name, hash])?;
    if let Some(upstream) = upstream {
        let _ = git::run(dir, ["branch".to_string(), "--quiet".into(), format!("--set-upstream-to={upstream}"), name.into()]);
    }
    Ok(())
}

/// "origin/feature/x" → ("origin", "feature/x"), with the remote names the repository has (one may contain "/").
pub fn split_remote_branch(short_name: &str, remotes: &[String]) -> Option<(String, String)> {
    let remote = remotes.iter().filter(|r| short_name.starts_with(&format!("{r}/"))).max_by_key(|r| r.len())?;
    let branch = &short_name[remote.len() + 1..];
    (!branch.is_empty()).then(|| (remote.clone(), branch.to_string()))
}

pub fn delete_remote_branch(branch: &str, remote: &str, dir: &Path) -> Result<()> {
    git::run(dir, ["push".to_string(), "--quiet".into(), remote.into(), "--delete".into(), format!("refs/heads/{branch}")])?;
    Ok(())
}

/// Pushes a deleted remote branch back where it was.
pub fn restore_remote_branch(branch: &str, remote: &str, hash: &str, dir: &Path) -> Result<()> {
    git::run(dir, ["push".to_string(), "--quiet".into(), remote.into(), format!("{hash}:refs/heads/{branch}")])?;
    Ok(())
}

// MARK: Force push

/// Branches DiffCode never force-pushes.
pub const PROTECTED_BRANCHES: [&str; 2] = ["main", "master"];

/// The commits the upstream has and the branch does not: what a force push would take off the remote.
pub fn incoming_commits(dir: &Path, limit: usize) -> Vec<Commit> {
    commits(&["HEAD..@{upstream}"], dir, limit)
}

/// The remote and the branch on it that `branch` follows.
pub fn upstream_target(branch: &str, dir: &Path) -> Option<(String, String)> {
    let config = |key: String| git::run_string(dir, ["config".to_string(), "--get".into(), key]).unwrap_or_default().trim().to_string();
    let remote = config(format!("branch.{branch}.remote"));
    let merge = config(format!("branch.{branch}.merge"));
    if remote.is_empty() || remote == "." {
        return None;
    }
    let target = merge.strip_prefix("refs/heads/")?;
    Some((remote, target.to_string()))
}

/// Pushes `branch` over its upstream, only if the remote branch is still at `expected`, the commit the user was
/// shown (`--force-with-lease`): commits someone pushed meanwhile are never lost.
pub fn force_push(branch: &str, expected: &str, dir: &Path) -> Result<()> {
    let Some((remote, target)) = upstream_target(branch, dir) else {
        return Err(Failure::new(lf!("{branch} follows no remote branch.", "{branch} 没有跟踪的远程分支。")));
    };
    if PROTECTED_BRANCHES.contains(&target.as_str()) || PROTECTED_BRANCHES.contains(&branch) {
        return Err(Failure::new(lf!("DiffCode never force-pushes {target}.", "DiffCode 不会强制推送 {target}。")));
    }
    git::run(
        dir,
        [
            "push".to_string(),
            "--quiet".into(),
            format!("--force-with-lease=refs/heads/{target}:{expected}"),
            remote,
            format!("refs/heads/{branch}:refs/heads/{target}"),
        ],
    )?;
    Ok(())
}

// MARK: - Stash

/// One entry of `git stash list`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct StashEntry {
    /// The stash commit: the tracked files as they were. Stays the same while the list around it changes.
    pub hash: String,
    /// Its place in the list when read, `stash@{index}`. Stashes added or dropped since move it: act by `hash`.
    pub index: usize,
    /// git's line for it: "On main: message", or "WIP on main: 1a2b3c4 Subject" when none was given.
    pub message: String,
    /// Seconds since 1970.
    pub date: i64,
    /// The commit it was made on (its first parent).
    pub base: String,
    /// The commit holding its untracked files (`stash push -u`), when it has any.
    pub untracked: Option<String>,
}

impl StashEntry {
    pub fn name(&self) -> String {
        format!("stash@{{{}}}", self.index)
    }

    /// The branch it was made on, from git's line; None on a detached HEAD.
    pub fn branch(&self) -> Option<&str> {
        for prefix in ["WIP on ", "On "] {
            if let Some(rest) = self.message.strip_prefix(prefix) {
                let colon = rest.find(": ")?;
                let name = &rest[..colon];
                return (name != "(no branch)").then_some(name);
            }
        }
        None
    }

    /// What the user called it, or git's "1a2b3c4 Subject" of the commit it was made on.
    pub fn title(&self) -> &str {
        if !(self.message.starts_with("WIP on ") || self.message.starts_with("On ")) {
            return &self.message;
        }
        match self.message.find(": ") {
            Some(colon) if colon + 2 < self.message.len() => &self.message[colon + 2..],
            _ => &self.message,
        }
    }
}

/// Every stash, newest first.
pub fn stashes(dir: &Path) -> Vec<StashEntry> {
    git::run(dir, ["stash", "list", "--format=%H%x1f%P%x1f%ct%x1f%gd%x1f%gs%x1e"])
        .map(|d| parse_stashes(&d))
        .unwrap_or_default()
}

pub fn parse_stashes(data: &[u8]) -> Vec<StashEntry> {
    let mut entries = Vec::new();
    for record in data.split(|&b| b == 0x1e).filter(|r| !r.is_empty()) {
        let fields: Vec<String> =
            record.split(|&b| b == 0x1f).map(|f| String::from_utf8_lossy(f).trim().to_string()).collect();
        if fields.len() < 5 || fields[0].len() < 7 {
            continue;
        }
        let parents: Vec<&str> = fields[1].split(' ').filter(|p| !p.is_empty()).collect();
        let Some(base) = parents.first() else { continue };
        // "stash@{3}"; a list read otherwise counts in order.
        let index = fields[3]
            .strip_prefix("stash@{")
            .and_then(|s| s.strip_suffix('}'))
            .and_then(|s| s.parse().ok())
            .unwrap_or(entries.len());
        entries.push(StashEntry {
            hash: fields[0].clone(),
            index,
            message: fields[4].clone(),
            date: fields[2].parse().unwrap_or(0),
            base: base.to_string(),
            untracked: parents.get(2).map(|s| s.to_string()),
        });
    }
    entries
}

/// Puts the local changes in a new stash, and the working tree goes back to HEAD. Returns the new stash; None when
/// there was nothing to put aside. With `include_untracked`, new files go too (`stash push -u`).
pub fn stash_push(message: &str, include_untracked: bool, dir: &Path) -> Result<Option<StashEntry>> {
    let before = stashes(dir).into_iter().next().map(|s| s.hash);
    let mut args = strings(&["stash", "push", "--quiet"]);
    if include_untracked {
        args.push("--include-untracked".into());
    }
    let text = message.trim();
    if !text.is_empty() {
        args.push("--message".into());
        args.push(text.to_string());
    }
    git::run(dir, args)?;
    Ok(stashes(dir).into_iter().next().filter(|top| Some(&top.hash) != before.as_ref()))
}

/// The stash as listed now, found by its commit. Fails when it has been dropped since.
pub fn current_stash(entry: &StashEntry, dir: &Path) -> Result<StashEntry> {
    stashes(dir)
        .into_iter()
        .find(|s| s.hash == entry.hash)
        .ok_or_else(|| Failure::new(l("That stash is no longer in the list.", "这个 stash 已经不在列表里了。")))
}

/// Brings a stash's changes back into the working tree. It stays in the list, unless `drop`: then it goes once
/// its changes are in without conflicts (`stash pop`).
pub fn stash_apply(entry: &StashEntry, drop: bool, dir: &Path) -> Result<()> {
    let current = current_stash(entry, dir)?;
    git::run(dir, ["stash".to_string(), if drop { "pop" } else { "apply" }.into(), "--quiet".into(), current.name()])?;
    Ok(())
}

pub fn stash_drop(entry: &StashEntry, dir: &Path) -> Result<()> {
    let current = current_stash(entry, dir)?;
    git::run(dir, ["stash".to_string(), "drop".into(), "--quiet".into(), current.name()])?;
    Ok(())
}

/// Lists a dropped stash again (its commit stays in the repository until git cleans up), at the top.
pub fn stash_restore(entry: &StashEntry, dir: &Path) -> Result<()> {
    git::run(dir, ["stash", "store", "--quiet", "--message", &entry.message, &entry.hash])?;
    Ok(())
}

/// What a stash changed against the commit it was made on, and its untracked files, which come out as new.
pub fn stash_files(entry: &StashEntry, dir: &Path) -> Result<Vec<FileChange>> {
    let mut changes = compare_files(&entry.base, Some(&entry.hash), dir)?;
    if let Some(untracked) = &entry.untracked {
        let names = git::run(dir, ["ls-tree", "-r", "-z", "--name-only", untracked])?;
        let stats = git::run(dir, ["diff", "--numstat", "-z", EMPTY_TREE, untracked, "--"]).unwrap_or_default();
        let counts = parse_numstat(&stats);
        let tracked: HashSet<String> = changes.iter().map(|c| c.path.clone()).collect();
        for path in crate::util::split_nul(&names) {
            if tracked.contains(&path) {
                continue;
            }
            let mut change = FileChange::new(path.clone(), None, ChangeKind::Untracked);
            if let Some(&(a, _)) = counts.get(&path) {
                change.additions = Some(a);
                change.deletions = Some(0);
            }
            changes.push(change);
        }
        changes.sort_by(|a, b| natural_cmp(&a.path, &b.path));
    }
    Ok(changes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::working_tree::tests::{repo, sh};

    #[test]
    fn stash_titles_and_branches() {
        let s = StashEntry {
            hash: "h".repeat(40),
            index: 0,
            message: "WIP on feature/x: 1a2b3c4 Subject".into(),
            date: 0,
            base: "b".into(),
            untracked: None,
        };
        assert_eq!(s.branch(), Some("feature/x"));
        assert_eq!(s.title(), "1a2b3c4 Subject");
        assert_eq!(s.name(), "stash@{0}");
        let parsed = parse_stashes(b"aaaaaaa\x1fbase idx untr\x1f10\x1fstash@{2}\x1fOn main: msg\x1e");
        assert_eq!(parsed[0].index, 2);
        assert_eq!(parsed[0].untracked.as_deref(), Some("untr"));
        assert_eq!(parsed[0].title(), "msg");
    }

    #[test]
    fn split_remote_names() {
        let remotes = vec!["origin".to_string(), "team/fork".to_string()];
        assert_eq!(split_remote_branch("team/fork/x/y", &remotes), Some(("team/fork".into(), "x/y".into())));
        assert_eq!(split_remote_branch("origin/", &remotes), None);
    }

    #[test]
    fn hard_reset_comes_back_with_local_changes() {
        if !git::is_available() {
            return;
        }
        let dir = repo();
        let root = dir.path();
        std::fs::write(root.join("f.txt"), "1\n").unwrap();
        sh(root, &["add", "."]);
        sh(root, &["commit", "-q", "-m", "one"]);
        std::fs::write(root.join("f.txt"), "2\n").unwrap();
        sh(root, &["commit", "-q", "-am", "two"]);
        std::fs::write(root.join("f.txt"), "local\n").unwrap();
        let m = reset("HEAD~1", ResetMode::Hard, root).unwrap();
        assert!(m.snapshot.is_some());
        assert_eq!(std::fs::read_to_string(root.join("f.txt")).unwrap(), "1\n");
        undo_head_move(&m, root).unwrap();
        assert_eq!(sh(root, &["log", "-1", "--format=%s"]).trim(), "two");
        assert_eq!(std::fs::read_to_string(root.join("f.txt")).unwrap(), "local\n");
    }

    #[test]
    fn stash_round_trip() {
        if !git::is_available() {
            return;
        }
        let dir = repo();
        let root = dir.path();
        std::fs::write(root.join("f.txt"), "1\n").unwrap();
        sh(root, &["add", "."]);
        sh(root, &["commit", "-q", "-m", "one"]);
        std::fs::write(root.join("f.txt"), "2\n").unwrap();
        std::fs::write(root.join("new.txt"), "n\n").unwrap();
        let entry = stash_push("put aside", true, root).unwrap().unwrap();
        assert_eq!(entry.title(), "put aside");
        assert!(!root.join("new.txt").exists());
        let files = stash_files(&entry, root).unwrap();
        assert_eq!(files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), vec!["f.txt", "new.txt"]);
        stash_apply(&entry, true, root).unwrap();
        assert!(stashes(root).is_empty());
        assert_eq!(std::fs::read_to_string(root.join("f.txt")).unwrap(), "2\n");
        // Dropped stashes come back by their commit.
        stash_restore(&entry, root).unwrap();
        assert_eq!(stashes(root)[0].hash, entry.hash);
    }

    #[test]
    fn merge_reports_fast_forward() {
        if !git::is_available() {
            return;
        }
        let dir = repo();
        let root = dir.path();
        std::fs::write(root.join("f.txt"), "1\n").unwrap();
        sh(root, &["add", "."]);
        sh(root, &["commit", "-q", "-m", "one"]);
        sh(root, &["checkout", "-q", "-b", "feature"]);
        std::fs::write(root.join("f.txt"), "2\n").unwrap();
        sh(root, &["commit", "-q", "-am", "two"]);
        sh(root, &["checkout", "-q", "main"]);
        let (outcome, m) = merge("refs/heads/feature", false, root).unwrap();
        assert_eq!(outcome, MergeOutcome::FastForward);
        assert_eq!(m.branch.as_deref(), Some("main"));
        undo_head_move(&m, root).unwrap();
        assert_eq!(sh(root, &["log", "-1", "--format=%s"]).trim(), "one");
    }
}
