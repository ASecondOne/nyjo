//! Monotonic "what does the project on disk look like" revision counter.
//!
//! The server bumps the revision whenever the *parsed* project tree actually
//! changes (not on every raw filesystem event), so editor temp files, our own
//! atomic-write renames, `.git` churn and repeated saves of identical content
//! never cause false drift. The Studio plugin remembers the revision it last
//! synced against and compares it with the value the server reports in every
//! heartbeat reply, which makes drift visible within about a second.

use std::collections::VecDeque;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Component, Path};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use serde::Serialize;

use crate::model::InstanceNode;

/// How many recently changed paths are kept for display.
pub const RECENT_CHANGE_LIMIT: usize = 8;

/// Filesystem events are coalesced for this long before the tree is rescanned.
pub const WATCH_DEBOUNCE: Duration = Duration::from_millis(150);

/// Interval for the polling fallback watcher (used when inotify is unavailable).
pub const POLL_WATCH_INTERVAL: Duration = Duration::from_millis(1_000);

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RevisionSnapshot {
    /// Bumped every time the parsed project tree changes. Starts at 1 once
    /// the first scan finished; 0 means "not scanned yet".
    pub revision: u64,
    pub last_change_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scan_error: Option<String>,
    pub recent_changes: Vec<String>,
    /// "notify" (inotify & co.), "poll" (fallback) or "off".
    pub watcher: String,
}

#[derive(Debug, Default)]
struct RevisionInner {
    revision: u64,
    fingerprint: Option<u64>,
    last_change_ms: u64,
    scan_error: Option<String>,
    recent_changes: VecDeque<String>,
    pending_paths: Vec<String>,
    watcher: Option<&'static str>,
}

#[derive(Debug, Clone, Default)]
pub struct ProjectRevision {
    inner: Arc<Mutex<RevisionInner>>,
}

impl ProjectRevision {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, RevisionInner> {
        // A panic while holding this tiny lock cannot leave it inconsistent in
        // a way that matters, so recover instead of wedging the server.
        self.inner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    pub fn set_watcher_kind(&self, kind: &'static str) {
        self.lock().watcher = Some(kind);
    }

    /// Remember which project-relative paths triggered the next rescan.
    pub fn note_changed_paths<I: IntoIterator<Item = String>>(&self, paths: I) {
        let mut inner = self.lock();
        for path in paths {
            if !inner.pending_paths.contains(&path) {
                inner.pending_paths.push(path);
            }
        }
    }

    /// Record the result of a project scan and return the (possibly bumped)
    /// revision. The revision only moves when the tree content, or whether the
    /// scan fails at all, actually changes.
    pub fn observe_scan(&self, scan: &Result<InstanceNode>) -> u64 {
        let (fingerprint, scan_error) = match scan {
            Ok(tree) => (Some(fingerprint_tree(tree)), None),
            Err(error) => (None, Some(format!("{error:#}"))),
        };

        let mut inner = self.lock();
        let pending = std::mem::take(&mut inner.pending_paths);
        let changed = inner.revision == 0
            || inner.fingerprint != fingerprint
            || inner.scan_error != scan_error;
        if changed {
            inner.revision += 1;
            inner.fingerprint = fingerprint;
            inner.scan_error = scan_error;
            inner.last_change_ms = now_ms();
            for path in pending {
                inner.recent_changes.retain(|existing| existing != &path);
                inner.recent_changes.push_front(path);
            }
            inner.recent_changes.truncate(RECENT_CHANGE_LIMIT);
        }
        inner.revision
    }

    pub fn current(&self) -> u64 {
        self.lock().revision
    }

    pub fn snapshot(&self) -> RevisionSnapshot {
        let inner = self.lock();
        RevisionSnapshot {
            revision: inner.revision,
            last_change_ms: inner.last_change_ms,
            scan_error: inner.scan_error.clone(),
            recent_changes: inner.recent_changes.iter().cloned().collect(),
            watcher: inner.watcher.unwrap_or("off").to_string(),
        }
    }
}

fn fingerprint_tree(tree: &InstanceNode) -> u64 {
    // serde_json output for a given tree is deterministic (properties live in
    // a BTreeMap), so hashing it is a cheap, stable content fingerprint.
    let mut hasher = DefaultHasher::new();
    match serde_json::to_vec(tree) {
        Ok(bytes) => bytes.hash(&mut hasher),
        Err(_) => format!("{tree:?}").hash(&mut hasher),
    }
    hasher.finish()
}

/// Decide whether a filesystem event path can influence the parsed tree and,
/// if so, return it relative to the project root for display.
///
/// Hidden entries (`.git`, `.nyjo-backups`, editor swap files, our own
/// `.name.nyjo-tmp-*` atomic-write temp files) are ignored, except for the two
/// dot-files Nyjo itself reads: `.nyjo` directory headers and `.nyjoignore`.
pub fn relevant_change_path(root: &Path, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(root).ok()?;
    let mut parts = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(name) => {
                let name = name.to_str()?;
                parts.push(name);
            }
            _ => return None,
        }
    }
    let (last, parents) = parts.split_last()?;
    if parents.iter().any(|part| part.starts_with('.')) {
        return None;
    }
    if last.starts_with('.') && *last != ".nyjo" && *last != ".nyjoignore" {
        return None;
    }
    if last.ends_with('~') || last.ends_with(".swp") || last.ends_with(".tmp") {
        return None;
    }
    Some(parts.join("/"))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(name: &str) -> InstanceNode {
        serde_json::from_value(serde_json::json!({
            "name": name,
            "className": "Folder",
            "path": name,
            "sourceKind": "directory",
            "sourcePath": null
        }))
        .expect("valid test node")
    }

    #[test]
    fn revision_only_moves_when_tree_changes() {
        let revision = ProjectRevision::new();
        assert_eq!(revision.current(), 0);
        assert_eq!(revision.observe_scan(&Ok(node("A"))), 1);
        assert_eq!(
            revision.observe_scan(&Ok(node("A"))),
            1,
            "same tree, same revision"
        );
        revision.note_changed_paths(["src/B.luau".to_string()]);
        assert_eq!(revision.observe_scan(&Ok(node("B"))), 2);
        let snapshot = revision.snapshot();
        assert_eq!(snapshot.recent_changes, vec!["src/B.luau".to_string()]);
        assert!(snapshot.last_change_ms > 0);
        assert_eq!(snapshot.watcher, "off");
    }

    #[test]
    fn scan_errors_bump_and_clear() {
        let revision = ProjectRevision::new();
        revision.observe_scan(&Ok(node("A")));
        let rev = revision.observe_scan(&Err(anyhow::anyhow!("bad meta.json")));
        assert_eq!(rev, 2);
        assert!(
            revision
                .snapshot()
                .scan_error
                .unwrap()
                .contains("bad meta.json")
        );
        assert_eq!(
            revision.observe_scan(&Err(anyhow::anyhow!("bad meta.json"))),
            2
        );
        assert_eq!(revision.observe_scan(&Ok(node("A"))), 3);
        assert!(revision.snapshot().scan_error.is_none());
    }

    #[test]
    fn recent_changes_are_capped_and_deduplicated() {
        let revision = ProjectRevision::new();
        for index in 0..20 {
            revision.note_changed_paths([format!("f{index}.luau"), "same.luau".to_string()]);
            revision.observe_scan(&Ok(node(&format!("N{index}"))));
        }
        let recent = revision.snapshot().recent_changes;
        assert_eq!(recent.len(), RECENT_CHANGE_LIMIT);
        assert_eq!(recent[0], "same.luau");
        assert_eq!(
            recent.iter().filter(|p| p.as_str() == "same.luau").count(),
            1
        );
    }

    #[test]
    fn event_path_filter() {
        let root = Path::new("/p");
        let check = |p: &str| relevant_change_path(root, Path::new(p));
        assert_eq!(
            check("/p/src/Main.server.luau").as_deref(),
            Some("src/Main.server.luau")
        );
        assert_eq!(check("/p/src/.nyjo").as_deref(), Some("src/.nyjo"));
        assert_eq!(check("/p/.nyjoignore").as_deref(), Some(".nyjoignore"));
        assert_eq!(check("/p/.git/index"), None);
        assert_eq!(check("/p/.nyjo-backups/studio/x.json"), None);
        assert_eq!(check("/p/src/.Main.luau.nyjo-tmp-123"), None);
        assert_eq!(check("/p/src/Main.luau~"), None);
        assert_eq!(check("/p/src/.Main.luau.swp"), None);
        assert_eq!(check("/elsewhere/x.luau"), None);
        assert_eq!(check("/p"), None);
    }
}
