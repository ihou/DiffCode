//! Recursive file watcher for one repository (ReadDirectoryChangesW on Windows, FSEvents on the Mac harness).
//!
//! Events are coalesced over a quarter second. Events from inside `.git` are filtered down to the few files that
//! actually signal a state change (HEAD, index, refs), which keeps git's own object churn from triggering refresh
//! storms.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use notify::{RecursiveMode, Watcher};

/// How long events are gathered before the callback hears of them.
pub const LATENCY: Duration = Duration::from_millis(250);

pub struct FileWatcher {
    // Dropping the watcher closes the channel, which ends the coalescing thread.
    _watcher: notify::RecommendedWatcher,
}

/// What one path means for the app: a file of the working tree that changed, a change of git's state, or nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Relevance {
    File,
    GitState,
    Ignored,
}

/// Sorts a changed path. Works on either kind of separator.
pub fn classify(path: &str) -> Relevance {
    let p = path.replace('\\', "/");
    if let Some(at) = p.find("/.git/") {
        let inner = &p[at + 6..];
        if inner.ends_with(".lock") {
            return Relevance::Ignored;
        }
        let name = inner.rsplit('/').next().unwrap_or(inner);
        if ["HEAD", "index", "ORIG_HEAD", "MERGE_HEAD", "FETCH_HEAD", "packed-refs"].contains(&name)
            || inner.starts_with("refs/")
        {
            return Relevance::GitState;
        }
        return Relevance::Ignored;
    }
    if p.ends_with("/.git") {
        return Relevance::Ignored;
    }
    Relevance::File
}

impl FileWatcher {
    /// Starts watching `root`. `callback` runs on the watcher's own thread with the changed working-tree paths and
    /// whether git's state (HEAD, index, refs) changed; never with nothing.
    pub fn new(root: &Path, callback: impl Fn(Vec<PathBuf>, bool) + Send + 'static) -> notify::Result<Self> {
        let (tx, rx) = mpsc::channel::<Vec<PathBuf>>();
        let mut watcher = notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
            if let Ok(event) = result {
                if !event.kind.is_access() {
                    let _ = tx.send(event.paths);
                }
            }
        })?;
        watcher.watch(root, RecursiveMode::Recursive)?;
        std::thread::Builder::new()
            .name("diffcode-watcher".into())
            .spawn(move || {
                while let Ok(first) = rx.recv() {
                    let started = Instant::now();
                    let mut paths = first;
                    loop {
                        let left = LATENCY.saturating_sub(started.elapsed());
                        if left.is_zero() {
                            break;
                        }
                        match rx.recv_timeout(left) {
                            Ok(more) => paths.extend(more),
                            Err(mpsc::RecvTimeoutError::Timeout) => break,
                            Err(mpsc::RecvTimeoutError::Disconnected) => return,
                        }
                    }
                    let mut relevant = Vec::new();
                    let mut git_state = false;
                    for path in paths {
                        match classify(&path.to_string_lossy()) {
                            Relevance::File => {
                                if !relevant.contains(&path) {
                                    relevant.push(path);
                                }
                            }
                            Relevance::GitState => git_state = true,
                            Relevance::Ignored => {}
                        }
                    }
                    if !relevant.is_empty() || git_state {
                        callback(relevant, git_state);
                    }
                }
            })
            .map_err(|e| notify::Error::generic(&e.to_string()))?;
        Ok(FileWatcher { _watcher: watcher })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_paths() {
        assert_eq!(classify(r"C:\repo\.git\index"), Relevance::GitState);
        assert_eq!(classify("/repo/.git/refs/heads/main"), Relevance::GitState);
        assert_eq!(classify("/repo/.git/index.lock"), Relevance::Ignored);
        assert_eq!(classify("/repo/.git/objects/ab/cdef"), Relevance::Ignored);
        assert_eq!(classify("/repo/.git"), Relevance::Ignored);
        assert_eq!(classify(r"C:\repo\src\main.rs"), Relevance::File);
    }

    #[test]
    fn reports_changes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let (tx, rx) = mpsc::channel();
        let _w = FileWatcher::new(&root, move |paths, git| {
            let _ = tx.send((paths, git));
        })
        .unwrap();
        std::thread::sleep(Duration::from_millis(100));
        std::fs::write(root.join("a.txt"), "x").unwrap();
        let (paths, _) = rx.recv_timeout(Duration::from_secs(5)).expect("no event");
        assert!(paths.iter().any(|p| p.ends_with("a.txt")), "{paths:?}");
    }
}
