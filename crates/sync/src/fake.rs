//! A server in memory for the engine's tests, behaving as RustCloud does where
//! the engine relies on it: ETag = SHA-256, conditional replace, `NAME_TAKEN`,
//! deleting moves into the hidden `.trash` of the user's root, the change feed
//! with cursors that can expire. Plus "another device" helpers working by path.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::time::SystemTime;

use chrono::{DateTime, Utc};
use rc_core::changes::{Change, ChangeKind, ChangePage};
use rc_core::types::{Node, NodeDetails};
use rc_core::Download;
use reqwest::StatusCode;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::remote::Remote;

struct FNode {
    id: Uuid,
    parent: Option<Uuid>,
    name: String,
    is_dir: bool,
    content: Vec<u8>,
    mtime: DateTime<Utc>,
}

struct Inner {
    nodes: HashMap<Uuid, FNode>,
    root: Uuid,
    feed: Vec<Change>,
    /// Cursors before this position are expired.
    oldest: usize,
    next_id: u128,
    /// Requests the engine made, by kind, for "it didn't download again" checks.
    calls: HashMap<&'static str, usize>,
}

pub struct FakeServer {
    inner: Mutex<Inner>,
}

fn err(status: StatusCode, code: &str) -> rc_core::Error {
    rc_core::Error::Api { status, code: code.into(), message: code.into() }
}

fn not_found() -> rc_core::Error {
    err(StatusCode::NOT_FOUND, "FILE_NOT_FOUND")
}

fn etag(content: &[u8]) -> String {
    Sha256::digest(content).iter().map(|b| format!("{b:02x}")).collect()
}

impl Inner {
    fn new_id(&mut self) -> Uuid {
        self.next_id += 1;
        Uuid::from_u128(self.next_id)
    }

    fn node(&self, id: Uuid) -> rc_core::Result<&FNode> {
        self.nodes.get(&id).ok_or_else(not_found)
    }

    fn to_node(&self, n: &FNode) -> Node {
        Node {
            id: n.id,
            parent_id: n.parent,
            name: n.name.clone(),
            is_dir: n.is_dir,
            size: n.content.len() as u64,
            mime_type: None,
            etag: (!n.is_dir).then(|| etag(&n.content)),
            created_at: n.mtime,
            updated_at: n.mtime,
            modified_at: Some(n.mtime),
            trashed_at: self.in_trash(n.id).then_some(n.mtime),
            lock: None,
        }
    }

    fn ancestors(&self, id: Uuid) -> Vec<Uuid> {
        let mut chain = Vec::new();
        let mut at = self.nodes.get(&id).and_then(|n| n.parent);
        while let Some(p) = at {
            chain.push(p);
            at = self.nodes[&p].parent;
        }
        chain.reverse();
        chain
    }

    fn in_trash(&self, id: Uuid) -> bool {
        self.ancestors(id).iter().any(|a| self.nodes[a].name == ".trash" && self.nodes[a].parent == Some(self.root))
    }

    fn child(&self, parent: Uuid, name: &str) -> Option<Uuid> {
        self.nodes.values().find(|n| n.parent == Some(parent) && n.name == name).map(|n| n.id)
    }

    fn insert(
        &mut self,
        parent: Uuid,
        name: &str,
        is_dir: bool,
        content: Vec<u8>,
        mtime: DateTime<Utc>,
    ) -> rc_core::Result<Uuid> {
        self.node(parent)?;
        if self.in_trash(parent) || self.nodes[&parent].name == ".trash" {
            return Err(err(StatusCode::BAD_REQUEST, "IN_TRASH"));
        }
        if self.child(parent, name).is_some() {
            return Err(err(StatusCode::CONFLICT, "NAME_TAKEN"));
        }
        let id = self.new_id();
        self.nodes.insert(id, FNode { id, parent: Some(parent), name: name.into(), is_dir, content, mtime });
        self.feed.push(Change { id, kind: ChangeKind::Created });
        Ok(id)
    }

    fn trash(&mut self) -> Uuid {
        match self.child(self.root, ".trash") {
            Some(id) => id,
            None => {
                let id = self.new_id();
                let root = self.root;
                self.nodes.insert(
                    id,
                    FNode {
                        id,
                        parent: Some(root),
                        name: ".trash".into(),
                        is_dir: true,
                        content: vec![],
                        mtime: Utc::now(),
                    },
                );
                id
            }
        }
    }

    fn delete(&mut self, id: Uuid) -> rc_core::Result<()> {
        self.node(id)?;
        if self.in_trash(id) {
            let mut gone = vec![id];
            gone.extend(self.nodes.keys().filter(|n| self.ancestors(**n).contains(&id)).copied());
            for g in gone {
                self.nodes.remove(&g);
            }
        } else {
            let trash = self.trash();
            let node = self.nodes.get_mut(&id).unwrap();
            node.parent = Some(trash);
            node.name = format!("{} {}", node.name, id.as_u128());
        }
        self.feed.push(Change { id, kind: ChangeKind::Deleted });
        Ok(())
    }

    /// `a/b/c` from the user's root.
    fn resolve(&self, path: &str) -> Option<Uuid> {
        path.split('/').try_fold(self.root, |at, name| self.child(at, name))
    }
}

impl FakeServer {
    pub fn new() -> Self {
        let root = Uuid::from_u128(1);
        let mut nodes = HashMap::new();
        nodes.insert(
            root,
            FNode { id: root, parent: None, name: String::new(), is_dir: true, content: vec![], mtime: Utc::now() },
        );
        Self { inner: Mutex::new(Inner { nodes, root, feed: vec![], oldest: 0, next_id: 100, calls: HashMap::new() }) }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap()
    }

    fn count(&self, call: &'static str) {
        *self.lock().calls.entry(call).or_default() += 1;
    }

    pub fn calls(&self, call: &str) -> usize {
        self.lock().calls.get(call).copied().unwrap_or(0)
    }

    pub fn id(&self, path: &str) -> Uuid {
        self.lock().resolve(path).unwrap_or_else(|| panic!("no {path} on the server"))
    }

    // "Another device", by path from the user's root.

    pub fn mkdir(&self, path: &str) -> Uuid {
        let mut s = self.lock();
        let mut at = s.root;
        for name in path.split('/') {
            at = match s.child(at, name) {
                Some(id) => id,
                None => s.insert(at, name, true, vec![], Utc::now()).unwrap(),
            };
        }
        at
    }

    /// Creates or replaces a file.
    pub fn put(&self, path: &str, content: &str) {
        let (parent, name) = path.rsplit_once('/').unwrap_or(("", path));
        let parent = if parent.is_empty() { self.lock().root } else { self.mkdir(parent) };
        let mut s = self.lock();
        match s.child(parent, name) {
            Some(id) => {
                let node = s.nodes.get_mut(&id).unwrap();
                node.content = content.as_bytes().to_vec();
                node.mtime = Utc::now();
                s.feed.push(Change { id, kind: ChangeKind::Changed });
            }
            None => {
                s.insert(parent, name, false, content.as_bytes().to_vec(), Utc::now()).unwrap();
            }
        }
    }

    /// Replaces a file's content without a feed entry, as if the change came
    /// between the engine reading the feed and acting on it.
    pub fn put_unseen(&self, path: &str, content: &str) {
        let id = self.id(path);
        let mut s = self.lock();
        let node = s.nodes.get_mut(&id).unwrap();
        node.content = content.as_bytes().to_vec();
        node.mtime = Utc::now();
    }

    pub fn read(&self, path: &str) -> Option<String> {
        let s = self.lock();
        s.resolve(path).map(|id| String::from_utf8(s.nodes[&id].content.clone()).unwrap())
    }

    pub fn remove(&self, path: &str) {
        let id = self.id(path);
        self.lock().delete(id).unwrap();
    }

    /// Moves or renames.
    pub fn rename(&self, path: &str, to: &str) {
        let id = self.id(path);
        let (parent, name) = to.rsplit_once('/').unwrap_or(("", to));
        let parent = if parent.is_empty() { self.lock().root } else { self.mkdir(parent) };
        let mut s = self.lock();
        let node = s.nodes.get_mut(&id).unwrap();
        node.parent = Some(parent);
        node.name = name.into();
        s.feed.push(Change { id, kind: ChangeKind::Moved });
    }

    /// Everything below `path` (relative to it), folders marked with a `/`.
    pub fn tree(&self, path: &str) -> Vec<String> {
        let s = self.lock();
        let top = s.resolve(path).unwrap();
        let mut out = Vec::new();
        let mut todo = vec![(top, String::new())];
        while let Some((dir, rel)) = todo.pop() {
            for n in s.nodes.values().filter(|n| n.parent == Some(dir)) {
                let p = if rel.is_empty() { n.name.clone() } else { format!("{rel}/{}", n.name) };
                if n.is_dir {
                    out.push(format!("{p}/"));
                    todo.push((n.id, p));
                } else {
                    out.push(p);
                }
            }
        }
        out.sort();
        out
    }

    /// Older cursors get `410 CURSOR_EXPIRED`.
    pub fn expire_feed(&self) {
        let mut s = self.lock();
        s.oldest = s.feed.len();
    }
}

impl Remote for FakeServer {
    async fn changes(&self, cursor: Option<&str>) -> rc_core::Result<ChangePage> {
        self.count("changes");
        let s = self.lock();
        let Some(cursor) = cursor else {
            return Ok(ChangePage { changes: vec![], cursor: format!("c{}", s.feed.len()), more: false });
        };
        let from: usize = cursor
            .strip_prefix('c')
            .and_then(|n| n.parse().ok())
            .ok_or_else(|| err(StatusCode::BAD_REQUEST, "INVALID_CURSOR"))?;
        if from < s.oldest {
            return Err(err(StatusCode::GONE, "CURSOR_EXPIRED"));
        }
        let to = s.feed.len().min(from + 3); // small pages, so `more` is exercised
        Ok(ChangePage { changes: s.feed[from..to].to_vec(), cursor: format!("c{to}"), more: to < s.feed.len() })
    }

    async fn details(&self, id: Uuid) -> rc_core::Result<NodeDetails> {
        self.count("details");
        let s = self.lock();
        let node = s.to_node(s.node(id)?);
        let path = s.ancestors(id).iter().map(|a| s.to_node(&s.nodes[a])).collect();
        Ok(NodeDetails { node, path })
    }

    async fn children(&self, folder: Uuid) -> rc_core::Result<Vec<Node>> {
        self.count("children");
        let s = self.lock();
        s.node(folder)?;
        let mut children: Vec<Node> =
            s.nodes.values().filter(|n| n.parent == Some(folder)).map(|n| s.to_node(n)).collect();
        children.sort_by_key(|n| (!n.is_dir, n.name.to_lowercase()));
        Ok(children)
    }

    async fn create_folder(&self, parent: Uuid, name: &str) -> rc_core::Result<Node> {
        self.count("create_folder");
        let mut s = self.lock();
        let id = s.insert(parent, name, true, vec![], Utc::now())?;
        Ok(s.to_node(&s.nodes[&id]))
    }

    async fn upload(&self, folder: Uuid, name: &str, path: &Path) -> rc_core::Result<Node> {
        self.count("upload");
        let content = std::fs::read(path)?;
        let mtime = file_mtime(path);
        let mut s = self.lock();
        let id = s.insert(folder, name, false, content, mtime)?;
        Ok(s.to_node(&s.nodes[&id]))
    }

    async fn replace(&self, id: Uuid, path: &Path, if_match: &str) -> rc_core::Result<Node> {
        self.count("replace");
        let content = std::fs::read(path)?;
        let mtime = file_mtime(path);
        let mut s = self.lock();
        if etag(&s.node(id)?.content) != if_match {
            return Err(err(StatusCode::CONFLICT, "FILE_CHANGED"));
        }
        let node = s.nodes.get_mut(&id).unwrap();
        node.content = content;
        node.mtime = mtime;
        s.feed.push(Change { id, kind: ChangeKind::Changed });
        Ok(s.to_node(&s.nodes[&id]))
    }

    async fn download(&self, id: Uuid, dest: &Path) -> rc_core::Result<Download> {
        self.count("download");
        let content = self.lock().node(id)?.content.clone();
        std::fs::write(dest, &content)?;
        Ok(Download { etag: Some(etag(&content)), size: content.len() as u64, resumed: false })
    }

    async fn delete(&self, id: Uuid) -> rc_core::Result<()> {
        self.count("delete");
        self.lock().delete(id)
    }
}

fn file_mtime(path: &Path) -> DateTime<Utc> {
    let time = std::fs::metadata(path).and_then(|m| m.modified()).unwrap_or(SystemTime::now());
    DateTime::<Utc>::from(time)
}
