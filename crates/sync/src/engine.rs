use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::local::{self, LocalEntry, TEMP_PREFIX};
use crate::plan::{self, Action, Sides};
use crate::remote::{self, is_not_found, Remote};
use crate::state::{BaseEntry, RemoteNode, State};
use crate::{Error, Result};

/// How often the server's tree is walked in full, in case the feed missed something
/// (the server writes it just after a change, not in the same transaction).
const FULL_WALK_EVERY: chrono::Duration = chrono::Duration::hours(24);
/// Deleting more than this many items, and more than half of everything,
/// waits for [`Engine::allow_mass_delete_once`].
const MASS_DELETE: usize = 50;

/// What one run did.
#[derive(Debug, Default, Clone)]
pub struct Report {
    pub uploaded: usize,
    pub downloaded: usize,
    pub folders_created: usize,
    pub deleted_local: usize,
    pub deleted_remote: usize,
    /// Moved or renamed locally because they were on the server.
    pub moved_local: usize,
    /// Local copies set aside as `name (conflict …)`.
    pub conflicts: Vec<String>,
    /// Changed while this run worked on them: the next run takes them.
    pub postponed: Vec<String>,
    /// Failed, with the reason; the next run tries again.
    pub failed: Vec<(String, String)>,
    /// Not synced at all: links, names the server refuses.
    pub skipped: Vec<(String, &'static str)>,
}

impl Report {
    /// Whether the run changed anything on either side.
    pub fn changed_anything(&self) -> bool {
        self.uploaded
            + self.downloaded
            + self.folders_created
            + self.deleted_local
            + self.deleted_remote
            + self.moved_local
            + self.conflicts.len()
            > 0
    }
}

/// One synced pair: a local folder and a folder on the server.
pub struct Engine<R: Remote> {
    remote: R,
    local_root: PathBuf,
    remote_root: Uuid,
    state: Mutex<State>,
    running: tokio::sync::Mutex<()>,
    allow_mass_delete: AtomicBool,
}

impl<R: Remote> Engine<R> {
    /// `state_file` keeps what the pair remembers; one file per pair.
    pub fn open(remote: R, state_file: &Path, local_root: PathBuf, remote_root: Uuid) -> Result<Self> {
        let state = State::open(state_file, &local_root, remote_root)?;
        Ok(Self::new(remote, state, local_root, remote_root))
    }

    fn new(remote: R, state: State, local_root: PathBuf, remote_root: Uuid) -> Self {
        Self {
            remote,
            local_root,
            remote_root,
            state: Mutex::new(state),
            running: tokio::sync::Mutex::new(()),
            allow_mass_delete: AtomicBool::new(false),
        }
    }

    #[cfg(test)]
    pub(crate) fn in_memory(remote: R, local_root: PathBuf, remote_root: Uuid) -> Self {
        let state = State::in_memory(&local_root, remote_root).unwrap();
        Self::new(remote, state, local_root, remote_root)
    }

    pub fn local_root(&self) -> &Path {
        &self.local_root
    }

    pub fn remote_root(&self) -> Uuid {
        self.remote_root
    }

    /// The user confirmed: the next run may delete many items.
    pub fn allow_mass_delete_once(&self) {
        self.allow_mass_delete.store(true, Ordering::SeqCst);
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// One run; runs of the same pair wait for each other.
    pub async fn sync(&self) -> Result<Report> {
        let _running = self.running.lock().await;
        let result = self.run().await;
        self.allow_mass_delete.store(false, Ordering::SeqCst);
        result
    }

    async fn run(&self) -> Result<Report> {
        if !self.local_root.is_dir() {
            return Err(Error::LocalRootMissing(self.local_root.clone()));
        }
        remote::check_root(&self.remote, self.remote_root).await?;
        let nodes = self.refresh_mirror().await?;
        let remote = remote::by_path(&nodes, self.remote_root);

        let root = self.local_root.clone();
        let scan = tokio::task::spawn_blocking(move || local::scan(&root)).await.expect("the scan panicked")?;
        let mut local = scan.entries;
        let mut base = self.state().base()?;
        let mut report = Report { skipped: scan.skipped, ..Report::default() };

        self.follow_remote_moves(&remote, &mut local, &mut base, &mut report);

        let to_hash = plan::needs_hash(&local, &remote, &base);
        let root = self.local_root.clone();
        let hashes: BTreeMap<String, String> = tokio::task::spawn_blocking(move || {
            to_hash.into_iter().filter_map(|p| local::hash(&local::join(&root, &p)).ok().map(|h| (p, h))).collect()
        })
        .await
        .expect("hashing panicked");

        let actions = plan::plan(&Sides {
            local: &local,
            unreadable: &scan.unreadable,
            remote: &remote,
            base: &base,
            hashes: &hashes,
        });

        let deleting = deleted_count(&actions, &base);
        if deleting > MASS_DELETE && deleting * 2 > base.len() && !self.allow_mass_delete.load(Ordering::SeqCst) {
            return Err(Error::TooManyDeletions { count: deleting, of: base.len() });
        }

        let mut ids: HashMap<String, Uuid> = remote.iter().map(|(p, n)| (p.clone(), n.id)).collect();
        for action in &actions {
            match self.apply(action, &remote, &mut local, &mut ids, &mut report).await {
                Ok(()) => {}
                Err(e) if e.needs_login() => return Err(e),
                Err(e) => {
                    tracing::warn!("sync of {} failed: {e}", action.path());
                    report.failed.push((action.path().to_owned(), e.to_string()));
                }
            }
        }
        Ok(report)
    }

    /// The mirror of the server's tree, brought up to date.
    async fn refresh_mirror(&self) -> Result<HashMap<Uuid, RemoteNode>> {
        let (cursor, walked_at, mut nodes) = {
            let state = self.state();
            (state.get("cursor")?, state.get("full_walk_at")?, state.remote()?)
        };
        let due = walked_at
            .and_then(|at| DateTime::parse_from_rfc3339(&at).ok())
            .is_none_or(|at| Utc::now() - at.with_timezone(&Utc) > FULL_WALK_EVERY);
        let followed = match cursor {
            Some(cursor) if !due => remote::follow_feed(&self.remote, self.remote_root, &mut nodes, cursor).await?,
            _ => None,
        };
        let cursor = match followed {
            Some(cursor) => cursor,
            None => {
                let (walked, cursor) = remote::full_walk(&self.remote, self.remote_root).await?;
                nodes = walked;
                self.state().set("full_walk_at", &Utc::now().to_rfc3339())?;
                cursor
            }
        };
        self.state().save_remote(&nodes, &cursor)?;
        Ok(nodes)
    }

    /// What the server moved or renamed is moved locally too, instead of being
    /// deleted here and downloaded again there — if the local copy is as synced.
    fn follow_remote_moves(
        &self,
        remote: &BTreeMap<String, RemoteNode>,
        local: &mut BTreeMap<String, LocalEntry>,
        base: &mut BTreeMap<String, BaseEntry>,
        report: &mut Report,
    ) {
        let where_now: HashMap<Uuid, &String> = remote.iter().map(|(p, n)| (n.id, p)).collect();
        let candidates: Vec<(String, String)> = base
            .iter()
            .filter_map(|(old, b)| {
                let new = where_now.get(&b.node_id)?;
                (*new != old && remote[*new].is_dir == b.is_dir).then(|| (old.clone(), (*new).clone()))
            })
            .collect();
        for (old, new) in candidates {
            // Moved along with a folder already, or the base changed under us.
            let Some(b) = base.get(&old).cloned() else { continue };
            if plan::local_changed(local.get(&old), Some(&b)) || local.contains_key(&new) || base.contains_key(&new) {
                continue;
            }
            let (parent, _) = local::split(&new);
            if !parent.is_empty() && !local.get(parent).is_some_and(|e| e.is_dir) {
                continue;
            }
            let (from, to) = (local::join(&self.local_root, &old), local::join(&self.local_root, &new));
            if let Err(e) = std::fs::rename(&from, &to) {
                tracing::warn!("could not move {old} to {new} as on the server: {e}");
                continue;
            }
            if let Err(e) = self.state().move_base_tree(&old, &new) {
                tracing::error!("moved {old} to {new}, but could not remember it: {e}");
            }
            move_tree(local, &old, &new);
            move_tree(base, &old, &new);
            report.moved_local += 1;
        }
    }

    fn parent_id(&self, parent: &str, ids: &HashMap<String, Uuid>) -> Result<Uuid> {
        if parent.is_empty() {
            return Ok(self.remote_root);
        }
        ids.get(parent).copied().ok_or_else(|| {
            Error::Io(std::io::Error::new(std::io::ErrorKind::NotFound, format!("no folder {parent} on the server")))
        })
    }

    async fn apply(
        &self,
        action: &Action,
        remote: &BTreeMap<String, RemoteNode>,
        local: &mut BTreeMap<String, LocalEntry>,
        ids: &mut HashMap<String, Uuid>,
        report: &mut Report,
    ) -> Result<()> {
        let at = |path: &str| local::join(&self.local_root, path);
        match action {
            Action::MkdirRemote { path } => {
                let (parent, name) = local::split(path);
                let parent_id = self.parent_id(parent, ids)?;
                let node = match self.remote.create_folder(parent_id, name).await {
                    Ok(node) => node,
                    // Someone made it meanwhile: take theirs if it is a folder.
                    Err(e) if e.code() == Some("NAME_TAKEN") => {
                        let children = self.remote.children(parent_id).await?;
                        match children.into_iter().find(|c| c.name == name && c.is_dir) {
                            Some(existing) => existing,
                            None => return Err(e.into()),
                        }
                    }
                    Err(e) => return Err(e.into()),
                };
                ids.insert(path.clone(), node.id);
                self.state().put_base(path, &dir_entry(node.id))?;
                report.folders_created += 1;
            }
            Action::MkdirLocal { path, node } => {
                std::fs::create_dir_all(at(path))?;
                local.insert(path.clone(), LocalEntry { is_dir: true, size: 0, mtime_ns: 0 });
                self.state().put_base(path, &dir_entry(*node))?;
                report.folders_created += 1;
            }
            Action::Upload { path, replace } => {
                let file = at(path);
                // What is recorded is the file as it was before the upload: if it
                // changes during the upload, the next run sees that and uploads again.
                let Some(before) = local::stat(&file)?.filter(|e| !e.is_dir) else {
                    report.postponed.push(path.clone());
                    return Ok(());
                };
                let sent = match replace {
                    Some((id, etag)) => self.remote.replace(*id, &file, etag).await,
                    None => {
                        let (parent, name) = local::split(path);
                        let parent_id = self.parent_id(parent, ids)?;
                        self.remote.upload(parent_id, name, &file).await
                    }
                };
                let node = match sent {
                    Ok(node) => node,
                    // The server's side changed meanwhile: the next run compares again.
                    Err(e) if matches!(e.code(), Some("FILE_CHANGED" | "NAME_TAKEN")) => {
                        report.postponed.push(path.clone());
                        return Ok(());
                    }
                    Err(e) => return Err(e.into()),
                };
                ids.insert(path.clone(), node.id);
                self.state().put_base(
                    path,
                    &BaseEntry {
                        is_dir: false,
                        node_id: node.id,
                        etag: node.etag,
                        size: before.size,
                        mtime_ns: before.mtime_ns,
                    },
                )?;
                report.uploaded += 1;
            }
            Action::Download { path, node, etag } => {
                let target = at(path);
                let dir = target.parent().expect("a path below the root");
                std::fs::create_dir_all(dir)?;
                let tmp = dir.join(format!("{TEMP_PREFIX}{node}.part"));
                let got = match self.remote.download(*node, &tmp).await {
                    Ok(got) => got,
                    Err(e) => {
                        let _ = std::fs::remove_file(&tmp);
                        if is_not_found(&e) {
                            report.postponed.push(path.clone());
                            return Ok(());
                        }
                        return Err(e.into());
                    }
                };
                // Changed locally since the scan: keep that, the next run decides.
                if local::stat(&target)? != local.get(path).copied() {
                    let _ = std::fs::remove_file(&tmp);
                    report.postponed.push(path.clone());
                    return Ok(());
                }
                if let Some(r) = remote.get(path) {
                    let when = DateTime::from_timestamp_micros(r.mtime_us).unwrap_or_default();
                    local::set_mtime(&tmp, when)?;
                }
                std::fs::rename(&tmp, &target)?;
                let after = local::stat(&target)?.expect("just written");
                local.insert(path.clone(), after);
                // The content that came is what is recorded, even if newer than planned.
                let etag = got.etag.unwrap_or_else(|| etag.clone());
                self.state().put_base(
                    path,
                    &BaseEntry {
                        is_dir: false,
                        node_id: *node,
                        etag: Some(etag),
                        size: after.size,
                        mtime_ns: after.mtime_ns,
                    },
                )?;
                report.downloaded += 1;
            }
            Action::DeleteRemote { path, node, .. } => {
                match self.remote.delete(*node).await {
                    Ok(()) => {}
                    Err(e) if is_not_found(&e) => {}
                    Err(e) => return Err(e.into()),
                }
                self.state().remove_base_tree(path)?;
                report.deleted_remote += 1;
            }
            Action::DeleteLocal { path, is_dir } => {
                let target = at(path);
                if *is_dir {
                    match std::fs::remove_dir(&target) {
                        Ok(()) => report.deleted_local += 1,
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                        // Not empty: something not synced is inside. Keep it.
                        Err(_) => {
                            report.postponed.push(path.clone());
                            return Ok(());
                        }
                    }
                } else {
                    // Only what is still as the scan saw it.
                    if local::stat(&target)? != local.get(path).copied() {
                        report.postponed.push(path.clone());
                        return Ok(());
                    }
                    match std::fs::remove_file(&target) {
                        Ok(()) => report.deleted_local += 1,
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                        Err(e) => return Err(e.into()),
                    }
                }
                local.remove(path);
                self.state().remove_base(path)?;
            }
            Action::Conflict { path } => {
                let target = at(path);
                let (_, name) = local::split(path);
                let mut copy = target.with_file_name(local::conflict_name(name, Utc::now()));
                let mut n = 2;
                while copy.exists() {
                    copy = target.with_file_name(format!("{} {n}", local::conflict_name(name, Utc::now())));
                    n += 1;
                }
                std::fs::rename(&target, &copy)?;
                local.remove(path);
                report.conflicts.push(path.clone());
            }
            Action::Record { path, entry } => self.state().put_base(path, entry)?,
            Action::Forget { path } => self.state().remove_base(path)?,
        }
        Ok(())
    }
}

fn dir_entry(node: Uuid) -> BaseEntry {
    BaseEntry { is_dir: true, node_id: node, etag: None, size: 0, mtime_ns: 0 }
}

/// How many synced items the actions delete (a folder counts with what is in it).
fn deleted_count(actions: &[Action], base: &BTreeMap<String, BaseEntry>) -> usize {
    actions
        .iter()
        .filter(|a| a.is_delete())
        .map(|a| {
            let path = a.path();
            let prefix = format!("{path}/");
            1 + base.range(prefix.clone()..).take_while(|(p, _)| p.starts_with(&prefix)).count()
        })
        .sum()
}

/// Moves the keys `from` and `from/…` to `to` and `to/…`.
fn move_tree<V>(map: &mut BTreeMap<String, V>, from: &str, to: &str) {
    let prefix = format!("{from}/");
    let keys: Vec<String> = map.keys().filter(|k| k.as_str() == from || k.starts_with(&prefix)).cloned().collect();
    for key in keys {
        if let Some(value) = map.remove(&key) {
            map.insert(format!("{to}{}", &key[from.len()..]), value);
        }
    }
}
