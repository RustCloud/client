//! What a sync pair remembers between runs, in its own SQLite file:
//!
//! - `remote`: the mirror of the server's tree under the synced folder;
//! - `base`: per path, what both sides agreed on after the last run;
//! - `meta`: the feed cursor, the roots, when the tree was last walked in full.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};
use uuid::Uuid;

use crate::Result;

/// A file or folder of the server's tree, below the synced folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteNode {
    pub id: Uuid,
    pub parent: Uuid,
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    /// SHA-256 of the content; `None` for folders.
    pub etag: Option<String>,
    /// The content's mtime on the server, microseconds since 1970: downloads get it.
    pub mtime_us: i64,
}

/// What both sides agreed on for one path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaseEntry {
    pub is_dir: bool,
    pub node_id: Uuid,
    /// The server's content, `None` for folders.
    pub etag: Option<String>,
    /// The local file as it was then: a different size or mtime means it changed.
    pub size: u64,
    pub mtime_ns: i64,
}

pub struct State {
    conn: Connection,
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS remote (
    id TEXT PRIMARY KEY, parent TEXT NOT NULL, name TEXT NOT NULL,
    is_dir INTEGER NOT NULL, size INTEGER NOT NULL, etag TEXT, mtime_us INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS base (
    path TEXT PRIMARY KEY, is_dir INTEGER NOT NULL, node_id TEXT NOT NULL,
    etag TEXT, size INTEGER NOT NULL, mtime_ns INTEGER NOT NULL
);
";

impl State {
    /// Opens (or creates) the state of the pair `local_root` ↔ `remote_root`. A
    /// state kept for other roots is dropped: it describes another pair.
    pub fn open(file: &Path, local_root: &Path, remote_root: Uuid) -> Result<Self> {
        Self::init(Connection::open(file)?, local_root, remote_root)
    }

    #[cfg(test)]
    pub fn in_memory(local_root: &Path, remote_root: Uuid) -> Result<Self> {
        Self::init(Connection::open_in_memory()?, local_root, remote_root)
    }

    fn init(conn: Connection, local_root: &Path, remote_root: Uuid) -> Result<Self> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.execute_batch(SCHEMA)?;
        let state = Self { conn };
        let roots = format!("{}\n{remote_root}", local_root.display());
        if state.get("roots")?.as_deref() != Some(roots.as_str()) {
            state.conn.execute_batch("DELETE FROM meta; DELETE FROM remote; DELETE FROM base;")?;
            state.set("roots", &roots)?;
        }
        Ok(state)
    }

    pub fn get(&self, key: &str) -> Result<Option<String>> {
        Ok(self.conn.query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0)).optional()?)
    }

    pub fn set(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2) ON CONFLICT (key) DO UPDATE SET value = ?2",
            [key, value],
        )?;
        Ok(())
    }

    pub fn unset(&self, key: &str) -> Result<()> {
        self.conn.execute("DELETE FROM meta WHERE key = ?1", [key])?;
        Ok(())
    }

    pub fn remote(&self) -> Result<HashMap<Uuid, RemoteNode>> {
        let mut query = self.conn.prepare("SELECT id, parent, name, is_dir, size, etag, mtime_us FROM remote")?;
        let rows = query.query_map([], |r| {
            Ok(RemoteNode {
                id: uuid(r.get(0)?),
                parent: uuid(r.get(1)?),
                name: r.get(2)?,
                is_dir: r.get(3)?,
                size: r.get::<_, i64>(4)? as u64,
                etag: r.get(5)?,
                mtime_us: r.get(6)?,
            })
        })?;
        Ok(rows.map(|row| row.map(|n| (n.id, n))).collect::<rusqlite::Result<_>>()?)
    }

    /// Replaces the mirror and the cursor it is good for, together.
    pub fn save_remote(&mut self, nodes: &HashMap<Uuid, RemoteNode>, cursor: &str) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM remote", [])?;
        {
            let mut insert =
                tx.prepare("INSERT INTO remote (id, parent, name, is_dir, size, etag, mtime_us) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)")?;
            for n in nodes.values() {
                insert.execute(params![
                    n.id.to_string(),
                    n.parent.to_string(),
                    n.name,
                    n.is_dir,
                    n.size as i64,
                    n.etag,
                    n.mtime_us
                ])?;
            }
        }
        tx.execute(
            "INSERT INTO meta (key, value) VALUES ('cursor', ?1) ON CONFLICT (key) DO UPDATE SET value = ?1",
            [cursor],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn base(&self) -> Result<BTreeMap<String, BaseEntry>> {
        let mut query = self.conn.prepare("SELECT path, is_dir, node_id, etag, size, mtime_ns FROM base")?;
        let rows = query.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                BaseEntry {
                    is_dir: r.get(1)?,
                    node_id: uuid(r.get(2)?),
                    etag: r.get(3)?,
                    size: r.get::<_, i64>(4)? as u64,
                    mtime_ns: r.get(5)?,
                },
            ))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn put_base(&self, path: &str, e: &BaseEntry) -> Result<()> {
        self.conn.execute(
            "INSERT INTO base (path, is_dir, node_id, etag, size, mtime_ns) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT (path) DO UPDATE SET is_dir = ?2, node_id = ?3, etag = ?4, size = ?5, mtime_ns = ?6",
            params![path, e.is_dir, e.node_id.to_string(), e.etag, e.size as i64, e.mtime_ns],
        )?;
        Ok(())
    }

    pub fn remove_base(&self, path: &str) -> Result<()> {
        self.conn.execute("DELETE FROM base WHERE path = ?1", [path])?;
        Ok(())
    }

    /// `path` and everything below it.
    pub fn remove_base_tree(&self, path: &str) -> Result<()> {
        self.conn.execute(&format!("DELETE FROM base WHERE {IN_TREE}"), [path])?;
        Ok(())
    }

    /// `from` and everything below it now live at `to`.
    pub fn move_base_tree(&mut self, from: &str, to: &str) -> Result<()> {
        let tx = self.conn.transaction()?;
        let paths: Vec<String> = {
            let mut query = tx.prepare(&format!("SELECT path FROM base WHERE {IN_TREE}"))?;
            let rows = query.query_map([from], |r| r.get(0))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        for old in paths {
            let new = format!("{to}{}", &old[from.len()..]);
            tx.execute("DELETE FROM base WHERE path = ?1", [&new])?;
            tx.execute("UPDATE base SET path = ?2 WHERE path = ?1", [&old, &new])?;
        }
        tx.commit()?;
        Ok(())
    }
}

/// `?1` itself and the paths below it: `?1/…` sorts from `?1/` up to (not
/// including) `?10`, as `0` follows `/`.
const IN_TREE: &str = "path = ?1 OR (path >= ?1 || '/' AND path < ?1 || '0')";

fn uuid(text: String) -> Uuid {
    Uuid::parse_str(&text).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(n: u128) -> BaseEntry {
        BaseEntry {
            is_dir: false,
            node_id: Uuid::from_u128(n),
            etag: Some(format!("e{n}")),
            size: n as u64,
            mtime_ns: 1,
        }
    }

    #[test]
    fn trees_of_paths() {
        let mut state = State::in_memory(Path::new("/l"), Uuid::from_u128(1)).unwrap();
        for (i, path) in ["a", "a/b", "a/b/c", "a-b", "a0", "ab", "b"].iter().enumerate() {
            state.put_base(path, &entry(i as u128)).unwrap();
        }
        state.move_base_tree("a", "z").unwrap();
        let paths: Vec<_> = state.base().unwrap().into_keys().collect();
        assert_eq!(paths, ["a-b", "a0", "ab", "b", "z", "z/b", "z/b/c"]);
        state.remove_base_tree("z/b").unwrap();
        let paths: Vec<_> = state.base().unwrap().into_keys().collect();
        assert_eq!(paths, ["a-b", "a0", "ab", "b", "z"]);
    }

    #[test]
    fn another_pair_starts_afresh() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("state.sqlite");
        let mut state = State::open(&file, Path::new("/l"), Uuid::from_u128(1)).unwrap();
        state.put_base("a", &entry(1)).unwrap();
        let node = RemoteNode {
            id: Uuid::from_u128(2),
            parent: Uuid::from_u128(1),
            name: "a".into(),
            is_dir: false,
            size: 1,
            etag: Some("e".into()),
            mtime_us: 5,
        };
        state.save_remote(&HashMap::from([(node.id, node.clone())]), "c1").unwrap();
        drop(state);

        let state = State::open(&file, Path::new("/l"), Uuid::from_u128(1)).unwrap();
        assert_eq!(state.base().unwrap()["a"], entry(1));
        assert_eq!(state.remote().unwrap()[&node.id], node);
        assert_eq!(state.get("cursor").unwrap().as_deref(), Some("c1"));
        drop(state);

        let state = State::open(&file, Path::new("/other"), Uuid::from_u128(1)).unwrap();
        assert!(state.base().unwrap().is_empty() && state.remote().unwrap().is_empty());
        assert_eq!(state.get("cursor").unwrap(), None);
    }
}
