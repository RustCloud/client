//! The server side: what the engine needs from it ([`Remote`], implemented by
//! [`rc_core::Client`]) and the mirror of its tree, kept up to date from the
//! change feed.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::future::Future;
use std::path::Path;

use futures_util::{StreamExt, TryStreamExt};
use rc_core::changes::ChangePage;
use rc_core::types::{Node, NodeDetails};
use rc_core::{Client, Download};
use uuid::Uuid;

use crate::local::is_hidden_dir;
use crate::state::RemoteNode;
use crate::{Error, Result};

/// The server, as far as sync needs it.
pub trait Remote: Send + Sync {
    fn changes(&self, cursor: Option<&str>) -> impl Future<Output = rc_core::Result<ChangePage>> + Send;
    fn details(&self, id: Uuid) -> impl Future<Output = rc_core::Result<NodeDetails>> + Send;
    fn children(&self, folder: Uuid) -> impl Future<Output = rc_core::Result<Vec<Node>>> + Send;
    fn create_folder(&self, parent: Uuid, name: &str) -> impl Future<Output = rc_core::Result<Node>> + Send;
    /// A new file; `NAME_TAKEN` if the name is taken.
    fn upload(&self, folder: Uuid, name: &str, path: &Path) -> impl Future<Output = rc_core::Result<Node>> + Send;
    /// New content, only if the file still has `if_match` (`FILE_CHANGED` otherwise).
    fn replace(&self, id: Uuid, path: &Path, if_match: &str) -> impl Future<Output = rc_core::Result<Node>> + Send;
    fn download(&self, id: Uuid, dest: &Path) -> impl Future<Output = rc_core::Result<Download>> + Send;
    /// To the trash.
    fn delete(&self, id: Uuid) -> impl Future<Output = rc_core::Result<()>> + Send;
}

impl Remote for Client {
    async fn changes(&self, cursor: Option<&str>) -> rc_core::Result<ChangePage> {
        Client::changes(self, cursor).await
    }
    async fn details(&self, id: Uuid) -> rc_core::Result<NodeDetails> {
        Client::details(self, id).await
    }
    async fn children(&self, folder: Uuid) -> rc_core::Result<Vec<Node>> {
        Client::children(self, folder).await
    }
    async fn create_folder(&self, parent: Uuid, name: &str) -> rc_core::Result<Node> {
        Client::create_folder(self, parent, name).await
    }
    async fn upload(&self, folder: Uuid, name: &str, path: &Path) -> rc_core::Result<Node> {
        Client::upload(self, folder, name, path, false).await
    }
    async fn replace(&self, id: Uuid, path: &Path, if_match: &str) -> rc_core::Result<Node> {
        Client::replace(self, id, path, Some(if_match)).await
    }
    async fn download(&self, id: Uuid, dest: &Path) -> rc_core::Result<Download> {
        Client::download(self, id, dest, false).await
    }
    async fn delete(&self, id: Uuid) -> rc_core::Result<()> {
        Client::delete(self, id).await
    }
}

impl<T: Remote> Remote for std::sync::Arc<T> {
    fn changes(&self, cursor: Option<&str>) -> impl Future<Output = rc_core::Result<ChangePage>> + Send {
        (**self).changes(cursor)
    }
    fn details(&self, id: Uuid) -> impl Future<Output = rc_core::Result<NodeDetails>> + Send {
        (**self).details(id)
    }
    fn children(&self, folder: Uuid) -> impl Future<Output = rc_core::Result<Vec<Node>>> + Send {
        (**self).children(folder)
    }
    fn create_folder(&self, parent: Uuid, name: &str) -> impl Future<Output = rc_core::Result<Node>> + Send {
        (**self).create_folder(parent, name)
    }
    fn upload(&self, folder: Uuid, name: &str, path: &Path) -> impl Future<Output = rc_core::Result<Node>> + Send {
        (**self).upload(folder, name, path)
    }
    fn replace(&self, id: Uuid, path: &Path, if_match: &str) -> impl Future<Output = rc_core::Result<Node>> + Send {
        (**self).replace(id, path, if_match)
    }
    fn download(&self, id: Uuid, dest: &Path) -> impl Future<Output = rc_core::Result<Download>> + Send {
        (**self).download(id, dest)
    }
    fn delete(&self, id: Uuid) -> impl Future<Output = rc_core::Result<()>> + Send {
        (**self).delete(id)
    }
}

pub(crate) fn is_not_found(e: &rc_core::Error) -> bool {
    e.code() == Some("FILE_NOT_FOUND")
}

impl RemoteNode {
    pub fn from_node(node: &Node, parent: Uuid) -> Self {
        Self {
            id: node.id,
            parent,
            name: node.name.clone(),
            is_dir: node.is_dir,
            size: node.size,
            etag: node.etag.clone(),
            mtime_us: node.mtime().timestamp_micros(),
        }
    }
}

/// The synced folder must still be there, outside the trash.
pub async fn check_root(remote: &impl Remote, root: Uuid) -> Result<()> {
    match remote.details(root).await {
        Ok(d) if d.node.is_dir && d.node.trashed_at.is_none() && !d.path.iter().any(|a| is_hidden_dir(&a.name)) => {
            Ok(())
        }
        Ok(_) => Err(Error::RemoteRootGone),
        Err(e) if is_not_found(&e) => Err(Error::RemoteRootGone),
        Err(e) => Err(e.into()),
    }
}

/// Everything below `folder`, hidden folders left out.
async fn walk(remote: &impl Remote, folder: Uuid, into: &mut HashMap<Uuid, RemoteNode>) -> Result<()> {
    let mut todo = vec![folder];
    while let Some(dir) = todo.pop() {
        let children = match remote.children(dir).await {
            Ok(children) => children,
            // Deleted while we walk: the feed will tell.
            Err(e) if is_not_found(&e) && dir != folder => continue,
            Err(e) => return Err(e.into()),
        };
        for child in children {
            if child.is_dir && is_hidden_dir(&child.name) {
                continue;
            }
            if child.is_dir {
                todo.push(child.id);
            }
            into.insert(child.id, RemoteNode::from_node(&child, dir));
        }
    }
    Ok(())
}

/// The whole tree anew, with the cursor taken before the walk: whatever
/// changes during it is in the feed after that cursor.
pub async fn full_walk(remote: &impl Remote, root: Uuid) -> Result<(HashMap<Uuid, RemoteNode>, String)> {
    let cursor = remote.changes(None).await?.cursor;
    let mut nodes = HashMap::new();
    walk(remote, root, &mut nodes).await?;
    Ok((nodes, cursor))
}

/// Applies the feed after `cursor` to `nodes`; returns the new cursor, or
/// `None` if the cursor expired (walk the tree in full instead).
pub async fn follow_feed(
    remote: &impl Remote,
    root: Uuid,
    nodes: &mut HashMap<Uuid, RemoteNode>,
    mut cursor: String,
) -> Result<Option<String>> {
    loop {
        let page = match remote.changes(Some(&cursor)).await {
            Ok(page) => page,
            Err(e) if matches!(e.code(), Some("CURSOR_EXPIRED" | "INVALID_CURSOR")) => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        // Each node once, in the order of its last change; what matters is how it is now.
        let mut seen = HashSet::new();
        let mut ids: Vec<Uuid> = page.changes.iter().rev().map(|c| c.id).filter(|id| seen.insert(*id)).collect();
        ids.reverse();
        ids.retain(|id| *id != root);

        let found: Vec<(Uuid, Option<NodeDetails>)> = futures_util::stream::iter(ids)
            .map(|id| async move {
                match remote.details(id).await {
                    Ok(d) => Ok((id, Some(d))),
                    Err(e) if is_not_found(&e) => Ok((id, None)),
                    Err(e) => Err(Error::from(e)),
                }
            })
            .buffered(8)
            .try_collect()
            .await?;

        for (id, details) in found {
            match details.and_then(|d| inside(&d, root).map(|parent| (d, parent))) {
                Some((d, parent)) => {
                    remove_below(nodes, id);
                    nodes.insert(id, RemoteNode::from_node(&d.node, parent));
                    if d.node.is_dir {
                        walk(remote, id, nodes).await?;
                    }
                }
                None => {
                    remove_below(nodes, id);
                    nodes.remove(&id);
                }
            }
        }
        cursor = page.cursor;
        if !page.more {
            return Ok(Some(cursor));
        }
    }
}

/// The parent of a node, if it is synced: below the root, not in the trash and
/// not in (or itself) a hidden folder.
fn inside(d: &NodeDetails, root: Uuid) -> Option<Uuid> {
    if d.node.trashed_at.is_some() || (d.node.is_dir && is_hidden_dir(&d.node.name)) {
        return None;
    }
    let at = d.path.iter().position(|a| a.id == root)?;
    if d.path[at + 1..].iter().any(|a| is_hidden_dir(&a.name)) {
        return None;
    }
    d.node.parent_id
}

/// Drops everything below `id` (not `id` itself).
fn remove_below(nodes: &mut HashMap<Uuid, RemoteNode>, id: Uuid) {
    let mut gone = HashSet::from([id]);
    loop {
        let more: Vec<Uuid> =
            nodes.values().filter(|n| gone.contains(&n.parent) && !gone.contains(&n.id)).map(|n| n.id).collect();
        if more.is_empty() {
            break;
        }
        gone.extend(more);
    }
    gone.remove(&id);
    nodes.retain(|nid, _| !gone.contains(nid));
}

/// The mirror by relative path. Nodes whose chain does not reach the root (the
/// mirror is inconsistent) are left out; the daily full walk mends that.
pub fn by_path(nodes: &HashMap<Uuid, RemoteNode>, root: Uuid) -> BTreeMap<String, RemoteNode> {
    let mut paths: HashMap<Uuid, Option<String>> = HashMap::new();
    fn path_of(
        id: Uuid,
        root: Uuid,
        nodes: &HashMap<Uuid, RemoteNode>,
        paths: &mut HashMap<Uuid, Option<String>>,
        depth: usize,
    ) -> Option<String> {
        if let Some(known) = paths.get(&id) {
            return known.clone();
        }
        let node = nodes.get(&id)?;
        let path = if node.parent == root {
            Some(node.name.clone())
        } else if depth > 256 {
            None
        } else {
            path_of(node.parent, root, nodes, paths, depth + 1).map(|parent| format!("{parent}/{}", node.name))
        };
        paths.insert(id, path.clone());
        path
    }
    nodes.keys().filter_map(|id| path_of(*id, root, nodes, &mut paths, 0).map(|p| (p, nodes[id].clone()))).collect()
}
