//! Deciding what to do, path by path, from three states: local now (L), the
//! server now (R) and the base (B), what both agreed on after the last run.
//! Pure: no files, no network — the engine carries the actions out.
//!
//! A side changed if it differs from the base (a file: size or mtime locally,
//! ETag on the server; existence; file or folder). Then:
//!
//! | local    | server   | action                                                      |
//! |----------|----------|-------------------------------------------------------------|
//! | same     | same     | nothing                                                     |
//! | changed  | same     | make the server like local: upload, create, delete          |
//! | same     | changed  | make local like the server: download, create, delete        |
//! | deleted  | changed  | the change wins: download                                   |
//! | changed  | deleted  | the change wins: upload                                     |
//! | changed  | changed  | same content — just record it; different — conflict copy    |
//!
//! A conflict renames the local file to `name (conflict …).ext` and downloads
//! the server's: the copy is a new local file and goes up on the next run, so
//! every device sees both.
//!
//! Folders are deleted only when nothing new is inside: a folder deleted on one
//! side while the other side put something in it is created again instead.

use std::collections::{BTreeMap, BTreeSet};

use uuid::Uuid;

use crate::local::LocalEntry;
use crate::state::{BaseEntry, RemoteNode};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Create the folder on the server.
    MkdirRemote { path: String },
    /// Create the folder locally; it is the server's `node`.
    MkdirLocal { path: String, node: Uuid },
    /// Upload the local file: new, or replacing `replace` = (node, the ETag it must still have).
    Upload { path: String, replace: Option<(Uuid, String)> },
    /// Download the server's file.
    Download { path: String, node: Uuid, etag: String },
    /// Delete on the server (a folder with everything in it; it goes to the trash).
    DeleteRemote { path: String, node: Uuid, is_dir: bool },
    /// Delete locally; a folder only if it is empty by then.
    DeleteLocal { path: String, is_dir: bool },
    /// Move the local file aside as a conflict copy.
    Conflict { path: String },
    /// Both sides agree already: remember it.
    Record { path: String, entry: BaseEntry },
    /// Nothing on either side any more: forget the path.
    Forget { path: String },
}

impl Action {
    pub fn path(&self) -> &str {
        match self {
            Self::MkdirRemote { path }
            | Self::MkdirLocal { path, .. }
            | Self::Upload { path, .. }
            | Self::Download { path, .. }
            | Self::DeleteRemote { path, .. }
            | Self::DeleteLocal { path, .. }
            | Self::Conflict { path }
            | Self::Record { path, .. }
            | Self::Forget { path } => path,
        }
    }

    pub fn is_delete(&self) -> bool {
        matches!(self, Self::DeleteRemote { .. } | Self::DeleteLocal { .. })
    }
}

/// The inputs of a plan.
pub struct Sides<'a> {
    pub local: &'a BTreeMap<String, LocalEntry>,
    /// Folders that could not be read: no opinion about anything below them.
    pub unreadable: &'a [String],
    pub remote: &'a BTreeMap<String, RemoteNode>,
    pub base: &'a BTreeMap<String, BaseEntry>,
    /// Hex SHA-256 of local files, for paths where both sides changed (see
    /// [`needs_hash`]); a missing one counts as different.
    pub hashes: &'a BTreeMap<String, String>,
}

pub fn local_changed(l: Option<&LocalEntry>, b: Option<&BaseEntry>) -> bool {
    match (l, b) {
        (None, None) => false,
        (Some(l), Some(b)) => l.is_dir != b.is_dir || (!l.is_dir && (l.size != b.size || l.mtime_ns != b.mtime_ns)),
        _ => true,
    }
}

pub fn remote_changed(r: Option<&RemoteNode>, b: Option<&BaseEntry>) -> bool {
    match (r, b) {
        (None, None) => false,
        (Some(r), Some(b)) => r.is_dir != b.is_dir || r.id != b.node_id || (!r.is_dir && r.etag != b.etag),
        _ => true,
    }
}

/// Paths of local files to hash before planning: both sides have a file there
/// and both changed it.
pub fn needs_hash(
    local: &BTreeMap<String, LocalEntry>,
    remote: &BTreeMap<String, RemoteNode>,
    base: &BTreeMap<String, BaseEntry>,
) -> Vec<String> {
    local
        .iter()
        .filter(|(path, l)| {
            let (r, b) = (remote.get(*path), base.get(*path));
            !l.is_dir && r.is_some_and(|r| !r.is_dir) && local_changed(Some(l), b) && remote_changed(r, b)
        })
        .map(|(path, _)| path.clone())
        .collect()
}

/// The actions, in the order to carry them out: first everything that creates or
/// changes (parents before children), then deletions (children before parents).
pub fn plan(sides: &Sides) -> Vec<Action> {
    let paths: BTreeSet<&String> = sides.local.keys().chain(sides.remote.keys()).chain(sides.base.keys()).collect();
    let mut by_path: BTreeMap<String, Vec<Action>> = BTreeMap::new();
    for path in paths {
        if under_any(path, sides.unreadable) {
            continue;
        }
        let actions = decide(path, sides);
        if !actions.is_empty() {
            by_path.insert(path.clone(), actions);
        }
    }
    keep_folders_with_news(&mut by_path, sides);

    let mut first = Vec::new();
    let mut last = Vec::new();
    for (_, actions) in by_path {
        if let [only] = actions.as_slice() {
            if only.is_delete() {
                last.push(only.clone());
                continue;
            }
        }
        first.extend(actions);
    }
    last.reverse();
    first.extend(last);
    first
}

fn decide(path: &str, sides: &Sides) -> Vec<Action> {
    let (l, r, b) = (sides.local.get(path), sides.remote.get(path), sides.base.get(path));
    match (local_changed(l, b), remote_changed(r, b)) {
        (false, false) => vec![],
        (true, false) => push(path, l, r),
        (false, true) => pull(path, l, r),
        (true, true) => match (l, r) {
            (None, None) => vec![Action::Forget { path: path.into() }],
            (None, Some(_)) => pull(path, l, r),
            (Some(_), None) => push(path, l, r),
            (Some(l), Some(r)) if l.is_dir && r.is_dir => vec![record(path, l, r)],
            (Some(l), Some(r)) if !l.is_dir && !r.is_dir => {
                if r.etag.is_some() && sides.hashes.get(path) == r.etag.as_ref() {
                    vec![record(path, l, r)]
                } else {
                    let mut actions = vec![Action::Conflict { path: path.into() }];
                    actions.extend(pull(path, None, Some(r)));
                    actions
                }
            }
            // A file on one side, a folder on the other.
            (Some(_), Some(r)) => {
                let mut actions = vec![Action::Conflict { path: path.into() }];
                actions.extend(pull(path, None, Some(r)));
                actions
            }
        },
    }
}

/// Make the server like local.
fn push(path: &str, l: Option<&LocalEntry>, r: Option<&RemoteNode>) -> Vec<Action> {
    let p = || path.to_owned();
    match (l, r) {
        (None, None) => vec![Action::Forget { path: p() }],
        (None, Some(r)) => vec![Action::DeleteRemote { path: p(), node: r.id, is_dir: r.is_dir }],
        (Some(l), Some(r)) if l.is_dir && r.is_dir => vec![record(path, l, r)],
        (Some(l), Some(r)) if !l.is_dir && !r.is_dir => {
            vec![Action::Upload { path: p(), replace: Some((r.id, r.etag.clone().unwrap_or_default())) }]
        }
        (Some(l), Some(r)) => {
            let mut actions = vec![Action::DeleteRemote { path: p(), node: r.id, is_dir: r.is_dir }];
            actions.extend(push(path, Some(l), None));
            actions
        }
        (Some(l), None) if l.is_dir => vec![Action::MkdirRemote { path: p() }],
        (Some(_), None) => vec![Action::Upload { path: p(), replace: None }],
    }
}

/// Make local like the server.
fn pull(path: &str, l: Option<&LocalEntry>, r: Option<&RemoteNode>) -> Vec<Action> {
    let p = || path.to_owned();
    match (l, r) {
        (None, None) => vec![Action::Forget { path: p() }],
        (Some(l), None) => vec![Action::DeleteLocal { path: p(), is_dir: l.is_dir }],
        (Some(l), Some(r)) if l.is_dir && r.is_dir => vec![record(path, l, r)],
        (Some(l), Some(r)) if l.is_dir != r.is_dir => {
            let mut actions = vec![Action::DeleteLocal { path: p(), is_dir: l.is_dir }];
            actions.extend(pull(path, None, Some(r)));
            actions
        }
        (_, Some(r)) if r.is_dir => vec![Action::MkdirLocal { path: p(), node: r.id }],
        (_, Some(r)) => vec![Action::Download { path: p(), node: r.id, etag: r.etag.clone().unwrap_or_default() }],
    }
}

fn record(path: &str, l: &LocalEntry, r: &RemoteNode) -> Action {
    Action::Record {
        path: path.into(),
        entry: BaseEntry { is_dir: r.is_dir, node_id: r.id, etag: r.etag.clone(), size: l.size, mtime_ns: l.mtime_ns },
    }
}

/// A folder about to be deleted on one side while something comes into it from
/// that very side is created on the other instead; one deleted for good takes
/// what is inside with it (on the server, in one request).
fn keep_folders_with_news(by_path: &mut BTreeMap<String, Vec<Action>>, sides: &Sides) {
    // Deepest first, so a kept folder keeps its parents too.
    let folders: Vec<String> = by_path
        .iter()
        .filter(|(_, a)| {
            matches!(
                a.as_slice(),
                [Action::DeleteRemote { is_dir: true, .. } | Action::DeleteLocal { is_dir: true, .. }]
            )
        })
        .map(|(p, _)| p.clone())
        .rev()
        .collect();
    for folder in folders {
        let prefix = format!("{folder}/");
        let below = || by_path.range(prefix.clone()..).take_while(|(p, _)| p.starts_with(&prefix));
        match by_path[&folder][0].clone() {
            Action::DeleteRemote { node, .. } => {
                let news = below()
                    .flat_map(|(_, a)| a)
                    .any(|a| matches!(a, Action::Download { .. } | Action::MkdirLocal { .. }));
                if news {
                    by_path.insert(folder.clone(), vec![Action::MkdirLocal { path: folder.clone(), node }]);
                } else {
                    // The server deletes the whole folder: nothing below needs a request.
                    let inside: Vec<String> = below().map(|(p, _)| p.clone()).collect();
                    for path in inside {
                        by_path.remove(&path);
                    }
                }
            }
            Action::DeleteLocal { .. } => {
                let news = below()
                    .flat_map(|(_, a)| a)
                    .any(|a| matches!(a, Action::Upload { .. } | Action::MkdirRemote { .. }));
                if news {
                    by_path.insert(folder.clone(), vec![Action::MkdirRemote { path: folder.clone() }]);
                }
                // Otherwise files below go one by one and the folder after them,
                // only if empty: whatever was not synced in it stays.
            }
            _ => {}
        }
    }
    let _ = sides;
}

fn under_any(path: &str, folders: &[String]) -> bool {
    folders
        .iter()
        .any(|f| f.is_empty() || path == f || path.strip_prefix(f.as_str()).is_some_and(|rest| rest.starts_with('/')))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOT: Uuid = Uuid::from_u128(1);

    #[derive(Default)]
    struct World {
        local: BTreeMap<String, LocalEntry>,
        remote: BTreeMap<String, RemoteNode>,
        base: BTreeMap<String, BaseEntry>,
        hashes: BTreeMap<String, String>,
        unreadable: Vec<String>,
    }

    fn id(path: &str) -> Uuid {
        Uuid::from_u128(path.bytes().fold(7u128, |h, b| h.wrapping_mul(31).wrapping_add(b as u128)))
    }

    impl World {
        /// A file on both sides and in the base, all agreeing.
        fn synced(mut self, path: &str, content: &str) -> Self {
            self.local.insert(path.into(), file(content));
            self.remote.insert(path.into(), rfile(path, content));
            self.base.insert(path.into(), bfile(path, content));
            self
        }

        fn synced_dir(mut self, path: &str) -> Self {
            self.local.insert(path.into(), LocalEntry { is_dir: true, size: 0, mtime_ns: 0 });
            self.remote.insert(
                path.into(),
                RemoteNode {
                    id: id(path),
                    parent: ROOT,
                    name: path.into(),
                    is_dir: true,
                    size: 0,
                    etag: None,
                    mtime_us: 0,
                },
            );
            self.base
                .insert(path.into(), BaseEntry { is_dir: true, node_id: id(path), etag: None, size: 0, mtime_ns: 0 });
            self
        }

        fn plan(&self) -> Vec<Action> {
            plan(&Sides {
                local: &self.local,
                unreadable: &self.unreadable,
                remote: &self.remote,
                base: &self.base,
                hashes: &self.hashes,
            })
        }
    }

    /// Content stands for size and mtime locally and for the ETag remotely.
    fn file(content: &str) -> LocalEntry {
        LocalEntry { is_dir: false, size: content.len() as u64, mtime_ns: content.len() as i64 * 1000 + 1 }
    }

    fn rfile(path: &str, content: &str) -> RemoteNode {
        RemoteNode {
            id: id(path),
            parent: ROOT,
            name: path.into(),
            is_dir: false,
            size: content.len() as u64,
            etag: Some(format!("etag-{content}")),
            mtime_us: 0,
        }
    }

    fn bfile(path: &str, content: &str) -> BaseEntry {
        let l = file(content);
        BaseEntry {
            is_dir: false,
            node_id: id(path),
            etag: Some(format!("etag-{content}")),
            size: l.size,
            mtime_ns: l.mtime_ns,
        }
    }

    #[test]
    fn nothing_changed_nothing_to_do() {
        let w = World::default().synced("a.txt", "a").synced_dir("d").synced("d/b.txt", "b");
        assert_eq!(w.plan(), vec![]);
    }

    #[test]
    fn one_sided_changes_go_across() {
        let mut w = World::default().synced("edited.txt", "a").synced("theirs.txt", "a").synced("gone.txt", "x");
        w.local.insert("edited.txt".into(), file("aa"));
        w.local.insert("new.txt".into(), file("n"));
        w.remote.insert("theirs.txt".into(), rfile("theirs.txt", "bb"));
        w.remote.insert("fresh.txt".into(), rfile("fresh.txt", "f"));
        w.local.remove("gone.txt");
        assert_eq!(
            w.plan(),
            vec![
                Action::Upload { path: "edited.txt".into(), replace: Some((id("edited.txt"), "etag-a".into())) },
                Action::Download { path: "fresh.txt".into(), node: id("fresh.txt"), etag: "etag-f".into() },
                Action::Upload { path: "new.txt".into(), replace: None },
                Action::Download { path: "theirs.txt".into(), node: id("theirs.txt"), etag: "etag-bb".into() },
                Action::DeleteRemote { path: "gone.txt".into(), node: id("gone.txt"), is_dir: false },
            ]
        );
    }

    #[test]
    fn two_changes_with_the_same_content_agree() {
        let mut w = World::default();
        w.local.insert("a.txt".into(), file("same"));
        w.remote.insert("a.txt".into(), rfile("a.txt", "same"));
        assert_eq!(needs_hash(&w.local, &w.remote, &w.base), vec!["a.txt".to_owned()]);
        w.hashes.insert("a.txt".into(), "etag-same".into());
        assert_eq!(w.plan(), vec![Action::Record { path: "a.txt".into(), entry: bfile("a.txt", "same") }]);
    }

    #[test]
    fn two_different_changes_make_a_conflict_copy() {
        let mut w = World::default().synced("a.txt", "a");
        w.local.insert("a.txt".into(), file("mine"));
        w.remote.insert("a.txt".into(), rfile("a.txt", "theirs"));
        w.hashes.insert("a.txt".into(), "etag-mine".into());
        assert_eq!(
            w.plan(),
            vec![
                Action::Conflict { path: "a.txt".into() },
                Action::Download { path: "a.txt".into(), node: id("a.txt"), etag: "etag-theirs".into() },
            ]
        );
    }

    #[test]
    fn a_change_wins_over_a_deletion() {
        let mut w = World::default().synced("kept-here.txt", "a").synced("kept-there.txt", "a");
        w.local.insert("kept-here.txt".into(), file("changed"));
        w.remote.remove("kept-here.txt");
        w.local.remove("kept-there.txt");
        w.remote.insert("kept-there.txt".into(), rfile("kept-there.txt", "changed"));
        assert_eq!(
            w.plan(),
            vec![
                Action::Upload { path: "kept-here.txt".into(), replace: None },
                Action::Download {
                    path: "kept-there.txt".into(),
                    node: id("kept-there.txt"),
                    etag: "etag-changed".into()
                },
            ]
        );
    }

    #[test]
    fn deleted_on_both_sides_is_forgotten() {
        let mut w = World::default().synced("a.txt", "a");
        w.local.clear();
        w.remote.clear();
        assert_eq!(w.plan(), vec![Action::Forget { path: "a.txt".into() }]);
    }

    #[test]
    fn a_folder_deleted_on_the_server_goes_with_one_request_and_children_first_locally() {
        // Deleted locally: one request for the folder on the server.
        let mut w = World::default().synced_dir("d").synced("d/a.txt", "a").synced("d/b.txt", "b");
        w.local.clear();
        assert_eq!(w.plan(), vec![Action::DeleteRemote { path: "d".into(), node: id("d"), is_dir: true }]);

        // Deleted on the server: files first, then the folder.
        let mut w = World::default().synced_dir("d").synced("d/a.txt", "a");
        w.remote.clear();
        assert_eq!(
            w.plan(),
            vec![
                Action::DeleteLocal { path: "d/a.txt".into(), is_dir: false },
                Action::DeleteLocal { path: "d".into(), is_dir: true },
            ]
        );
    }

    #[test]
    fn a_folder_with_news_is_not_deleted() {
        // Deleted locally, but a new file arrived in it on the server.
        let mut w = World::default().synced_dir("d").synced_dir("d/e").synced("d/e/old.txt", "o");
        w.local.clear();
        w.remote.insert("d/e/new.txt".into(), rfile("d/e/new.txt", "n"));
        let plan = w.plan();
        assert_eq!(
            plan,
            vec![
                Action::MkdirLocal { path: "d".into(), node: id("d") },
                Action::MkdirLocal { path: "d/e".into(), node: id("d/e") },
                Action::Download { path: "d/e/new.txt".into(), node: id("d/e/new.txt"), etag: "etag-n".into() },
                Action::DeleteRemote { path: "d/e/old.txt".into(), node: id("d/e/old.txt"), is_dir: false },
            ]
        );

        // Deleted on the server, but a new file was put in it locally.
        let mut w = World::default().synced_dir("d").synced("d/old.txt", "o");
        w.remote.clear();
        w.local.insert("d/new.txt".into(), file("n"));
        assert_eq!(
            w.plan(),
            vec![
                Action::MkdirRemote { path: "d".into() },
                Action::Upload { path: "d/new.txt".into(), replace: None },
                Action::DeleteLocal { path: "d/old.txt".into(), is_dir: false },
            ]
        );
    }

    #[test]
    fn a_file_against_a_folder() {
        let mut w = World::default();
        w.local.insert("x".into(), file("f"));
        w.remote.insert(
            "x".into(),
            RemoteNode { id: id("x"), parent: ROOT, name: "x".into(), is_dir: true, size: 0, etag: None, mtime_us: 0 },
        );
        assert_eq!(
            w.plan(),
            vec![Action::Conflict { path: "x".into() }, Action::MkdirLocal { path: "x".into(), node: id("x") }]
        );
    }

    #[test]
    fn nothing_below_an_unreadable_folder_is_touched() {
        let mut w = World::default().synced_dir("locked").synced("locked/a.txt", "a").synced("b.txt", "b");
        w.local.retain(|p, _| p == "locked" || p == "b.txt");
        w.unreadable.push("locked".into());
        w.remote.remove("b.txt");
        assert_eq!(w.plan(), vec![Action::DeleteLocal { path: "b.txt".into(), is_dir: false }]);
    }

    #[test]
    fn first_run_merges_both_sides() {
        let mut w = World::default();
        w.local.insert("d".into(), LocalEntry { is_dir: true, size: 0, mtime_ns: 0 });
        w.remote.insert(
            "d".into(),
            RemoteNode { id: id("d"), parent: ROOT, name: "d".into(), is_dir: true, size: 0, etag: None, mtime_us: 0 },
        );
        w.local.insert("d/mine.txt".into(), file("m"));
        w.remote.insert("d/theirs.txt".into(), rfile("d/theirs.txt", "t"));
        let plan = w.plan();
        assert!(matches!(&plan[0], Action::Record { path, .. } if path == "d"));
        assert_eq!(plan[1], Action::Upload { path: "d/mine.txt".into(), replace: None });
        assert!(matches!(&plan[2], Action::Download { path, .. } if path == "d/theirs.txt"));
    }
}
