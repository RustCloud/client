//! The local side: scanning the folder, hashing, names.
//!
//! Paths inside a pair are relative, with `/` between names on every system.

use std::collections::BTreeMap;
use std::fs::Metadata;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};

/// Files the engine writes while downloading; never synced.
pub const TEMP_PREFIX: &str = ".rcsync-";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalEntry {
    pub is_dir: bool,
    /// 0 for folders.
    pub size: u64,
    /// 0 for folders.
    pub mtime_ns: i64,
}

impl LocalEntry {
    pub fn from_meta(meta: &Metadata) -> Self {
        if meta.is_dir() {
            Self { is_dir: true, size: 0, mtime_ns: 0 }
        } else {
            Self { is_dir: false, size: meta.len(), mtime_ns: meta.modified().map(nanos).unwrap_or(0) }
        }
    }
}

/// What a scan found.
#[derive(Debug, Default)]
pub struct Scan {
    pub entries: BTreeMap<String, LocalEntry>,
    /// Folders that could not be read: nothing below them may be taken for deleted.
    pub unreadable: Vec<String>,
    /// What is not synced, with the reason: links, names the server refuses.
    pub skipped: Vec<(String, &'static str)>,
}

/// Folders starting with a dot are not synced (see the crate docs).
pub fn is_hidden_dir(name: &str) -> bool {
    name.starts_with('.')
}

/// A name the server takes (its `validate::file_name`), and not one of ours.
pub fn is_valid_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && name.len() <= 255
        && !name.chars().any(|c| c == '/' || c == '\\' || c.is_control())
        && !name.starts_with(TEMP_PREFIX)
}

/// `root` joined with a relative `a/b/c`.
pub fn join(root: &Path, rel: &str) -> PathBuf {
    let mut path = root.to_path_buf();
    path.extend(rel.split('/').filter(|part| !part.is_empty()));
    path
}

/// The parent of a relative path (`""` for the top level) and the name.
pub fn split(rel: &str) -> (&str, &str) {
    rel.rsplit_once('/').unwrap_or(("", rel))
}

/// Walks `root`. Links are not followed: they are skipped.
pub fn scan(root: &Path) -> std::io::Result<Scan> {
    let mut scan = Scan::default();
    // The root itself must be readable: otherwise everything would look deleted.
    let mut todo = vec![(root.to_path_buf(), String::new(), std::fs::read_dir(root)?)];
    while let Some((dir, rel, entries)) = todo.pop() {
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => {
                    scan.unreadable.push(rel.clone());
                    break;
                }
            };
            let Ok(name) = entry.file_name().into_string() else {
                scan.skipped.push((format!("{rel}/{}", entry.file_name().to_string_lossy()), "name is not UTF-8"));
                continue;
            };
            let path = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
            if name.starts_with(TEMP_PREFIX) {
                continue;
            }
            let Ok(meta) = entry.metadata() else {
                scan.unreadable.push(path);
                continue;
            };
            if meta.file_type().is_symlink() {
                scan.skipped.push((path, "link"));
                continue;
            }
            if meta.is_dir() && is_hidden_dir(&name) {
                continue;
            }
            if !is_valid_name(&name) {
                scan.skipped.push((path, "name not allowed on the server"));
                continue;
            }
            if meta.is_dir() {
                match std::fs::read_dir(dir.join(&name)) {
                    Ok(inner) => todo.push((dir.join(&name), path.clone(), inner)),
                    Err(_) => {
                        scan.unreadable.push(path);
                        continue;
                    }
                }
            } else if !meta.is_file() {
                scan.skipped.push((path, "not a regular file"));
                continue;
            }
            scan.entries.insert(path, LocalEntry::from_meta(&meta));
        }
    }
    Ok(scan)
}

/// The entry at `path` now, if there is one (not following links).
pub fn stat(path: &Path) -> std::io::Result<Option<LocalEntry>> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) => Ok(Some(LocalEntry::from_meta(&meta))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Hex SHA-256 of a file, as the server's ETag.
pub fn hash(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let read = file.read(&mut buf)?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// `report.pdf` → `report (conflict 2026-11-07 153000).pdf`; `.env` →
/// `.env (conflict …)`.
pub fn conflict_name(name: &str, when: DateTime<Utc>) -> String {
    let stamp = when.format("%Y-%m-%d %H%M%S");
    match name.rfind('.') {
        Some(dot) if dot > 0 => format!("{} (conflict {stamp}){}", &name[..dot], &name[dot..]),
        _ => format!("{name} (conflict {stamp})"),
    }
}

pub fn set_mtime(path: &Path, when: DateTime<Utc>) -> std::io::Result<()> {
    let time = match u64::try_from(when.timestamp_micros()) {
        Ok(micros) => SystemTime::UNIX_EPOCH + Duration::from_micros(micros),
        Err(_) => SystemTime::UNIX_EPOCH,
    };
    std::fs::File::options().write(true).open(path)?.set_modified(time)
}

fn nanos(time: SystemTime) -> i64 {
    time.duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_nanos() as i64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    #[test]
    fn scanning_skips_what_is_not_synced() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("docs/deep")).unwrap();
        std::fs::create_dir_all(root.join(".git/objects")).unwrap();
        std::fs::write(root.join("docs/deep/a.txt"), "aaa").unwrap();
        std::fs::write(root.join(".env"), "x").unwrap();
        std::fs::write(root.join(".git/HEAD"), "x").unwrap();
        std::fs::write(root.join(".rcsync-123.part"), "x").unwrap();
        // A file name with `\` (the server refuses it); on Windows that is a separator.
        #[cfg(unix)]
        std::fs::write(root.join("back\\slash"), "x").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(root.join(".env"), root.join("link")).unwrap();

        let scan = scan(root).unwrap();
        let paths: Vec<_> = scan.entries.keys().map(String::as_str).collect();
        assert_eq!(paths, [".env", "docs", "docs/deep", "docs/deep/a.txt"]);
        assert_eq!(scan.entries["docs/deep/a.txt"].size, 3);
        assert!(scan.entries["docs"].is_dir);
        #[cfg(unix)]
        {
            let skipped: Vec<_> = scan.skipped.iter().map(|(p, _)| p.as_str()).collect();
            assert!(skipped.contains(&"link") && skipped.contains(&"back\\slash"));
        }
        assert!(scan.unreadable.is_empty());
    }

    #[test]
    fn a_missing_root_is_an_error_not_an_empty_folder() {
        assert!(scan(Path::new("/nonexistent/rc-sync-test")).is_err());
    }

    #[test]
    fn names() {
        let when = Utc.with_ymd_and_hms(2026, 11, 7, 15, 30, 0).unwrap();
        assert_eq!(conflict_name("report.pdf", when), "report (conflict 2026-11-07 153000).pdf");
        assert_eq!(conflict_name(".env", when), ".env (conflict 2026-11-07 153000)");
        assert_eq!(conflict_name("Makefile", when), "Makefile (conflict 2026-11-07 153000)");
        assert_eq!(split("a/b/c.txt"), ("a/b", "c.txt"));
        assert_eq!(split("c.txt"), ("", "c.txt"));
        assert_eq!(join(Path::new("/r"), "a/b"), Path::new("/r/a/b"));
        assert!(!is_valid_name("..") && !is_valid_name("a/b") && !is_valid_name(".rcsync-x"));
        assert!(is_valid_name(".gitignore"));
    }

    #[test]
    fn hashes_match_the_servers_etags() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f");
        std::fs::write(&path, "abc").unwrap();
        assert_eq!(hash(&path).unwrap(), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }
}
