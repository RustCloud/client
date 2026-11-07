//! The engine against the in-memory server: two devices, one folder.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::fake::FakeServer;
use crate::{Engine, Error, Report};

struct Device {
    dir: tempfile::TempDir,
    engine: Engine<Arc<FakeServer>>,
}

impl Device {
    fn new(server: &Arc<FakeServer>) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("Sync");
        std::fs::create_dir(&root).unwrap();
        let engine = Engine::in_memory(server.clone(), root, server.id("Sync"));
        Self { dir, engine }
    }

    fn root(&self) -> PathBuf {
        self.dir.path().join("Sync")
    }

    fn path(&self, rel: &str) -> PathBuf {
        crate::local::join(&self.root(), rel)
    }

    fn write(&self, rel: &str, content: &str) {
        let path = self.path(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    fn read(&self, rel: &str) -> Option<String> {
        std::fs::read_to_string(self.path(rel)).ok()
    }

    async fn sync(&self) -> Report {
        let report = self.engine.sync().await.unwrap();
        assert!(report.failed.is_empty(), "{:?}", report.failed);
        report
    }

    /// Everything below the root, folders marked with a `/`, as `FakeServer::tree`.
    fn tree(&self) -> Vec<String> {
        fn walk(dir: &Path, rel: &str, out: &mut Vec<String>) {
            for e in std::fs::read_dir(dir).unwrap() {
                let e = e.unwrap();
                let name = e.file_name().into_string().unwrap();
                let p = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
                if e.file_type().unwrap().is_dir() {
                    out.push(format!("{p}/"));
                    walk(&e.path(), &p, out);
                } else {
                    out.push(p);
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.root(), "", &mut out);
        out.sort();
        out
    }
}

fn setup() -> (Arc<FakeServer>, Device, Device) {
    let server = Arc::new(FakeServer::new());
    server.mkdir("Sync");
    let (a, b) = (Device::new(&server), Device::new(&server));
    (server, a, b)
}

#[tokio::test]
async fn the_first_run_merges_and_the_second_has_nothing_to_do() {
    let (server, a, _) = setup();
    server.put("Sync/docs/remote.txt", "from the server");
    a.write("docs/local.txt", "from the device");
    a.write("top.txt", "top");

    let report = a.sync().await;
    assert_eq!((report.uploaded, report.downloaded), (2, 1));
    assert_eq!(server.tree("Sync"), a.tree());
    assert_eq!(a.tree(), ["docs/", "docs/local.txt", "docs/remote.txt", "top.txt"]);

    let again = a.sync().await;
    assert!(!again.changed_anything(), "{again:?}");
}

#[tokio::test]
async fn changes_travel_between_two_devices() {
    let (server, a, b) = setup();
    a.write("notes/plan.md", "v1");
    a.write("old.txt", "old");
    a.sync().await;
    b.sync().await;
    assert_eq!(b.read("notes/plan.md").as_deref(), Some("v1"));

    // Edit, add, delete on one device…
    b.write("notes/plan.md", "v2 from b");
    b.write("notes/new.md", "new");
    std::fs::remove_file(b.path("old.txt")).unwrap();
    let report = b.sync().await;
    assert_eq!((report.uploaded, report.deleted_remote), (2, 1));

    // …arrives on the other.
    let report = a.sync().await;
    assert_eq!((report.downloaded, report.deleted_local), (2, 1));
    assert_eq!(a.read("notes/plan.md").as_deref(), Some("v2 from b"));
    assert_eq!(a.tree(), ["notes/", "notes/new.md", "notes/plan.md"]);
    assert_eq!(server.read("Sync/old.txt"), None);
}

#[tokio::test]
async fn the_downloaded_file_keeps_the_servers_mtime() {
    let (server, a, b) = setup();
    a.write("photo.jpg", "jpeg");
    let old = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_600_000_000);
    std::fs::File::options().write(true).open(a.path("photo.jpg")).unwrap().set_modified(old).unwrap();
    a.sync().await;
    b.sync().await;
    let mtime = std::fs::metadata(b.path("photo.jpg")).unwrap().modified().unwrap();
    assert_eq!(mtime, old);
    assert!(server.read("Sync/photo.jpg").is_some());
}

#[tokio::test]
async fn two_different_edits_make_a_conflict_copy_that_reaches_everyone() {
    let (server, a, b) = setup();
    a.write("report.txt", "base");
    a.sync().await;
    b.sync().await;

    a.write("report.txt", "a's version");
    a.sync().await;
    b.write("report.txt", "b's longer version");
    let report = b.sync().await;
    assert_eq!(report.conflicts, ["report.txt"]);
    assert_eq!(b.read("report.txt").as_deref(), Some("a's version"), "the server's version wins the name");
    let copy = b.tree().into_iter().find(|p| p.starts_with("report (conflict ")).expect("a conflict copy");
    assert_eq!(b.read(&copy).as_deref(), Some("b's longer version"));

    // The copy goes up on the next run and comes down to the other device.
    b.sync().await;
    a.sync().await;
    assert_eq!(a.read(&copy).as_deref(), Some("b's longer version"));
    assert_eq!(server.tree("Sync"), a.tree());
}

#[tokio::test]
async fn the_same_edit_on_both_sides_is_not_a_conflict() {
    let (server, a, _) = setup();
    server.put("Sync/same.txt", "identical");
    a.write("same.txt", "identical");
    let report = a.sync().await;
    assert!(report.conflicts.is_empty());
    assert_eq!((report.uploaded, report.downloaded), (0, 0));
    assert!(!a.sync().await.changed_anything());
}

#[tokio::test]
async fn a_rename_on_the_server_is_a_rename_here_not_a_download() {
    let (server, a, _) = setup();
    server.put("Sync/Photos/2024/a.jpg", "a");
    server.put("Sync/Photos/2024/b.jpg", "b");
    a.sync().await;
    let downloads = server.calls("download");

    server.rename("Sync/Photos/2024", "Sync/Photos/Summer 2024");
    let report = a.sync().await;
    assert_eq!(report.moved_local, 1);
    assert_eq!(server.calls("download"), downloads, "nothing downloaded again");
    assert_eq!(a.tree(), ["Photos/", "Photos/Summer 2024/", "Photos/Summer 2024/a.jpg", "Photos/Summer 2024/b.jpg"]);
    assert!(!a.sync().await.changed_anything());
}

#[tokio::test]
async fn a_folder_deleted_here_goes_to_the_trash_there_with_one_request() {
    let (server, a, _) = setup();
    for i in 0..5 {
        a.write(&format!("old/{i}.txt"), "x");
    }
    a.write("keep.txt", "k");
    a.sync().await;
    let deletes = server.calls("delete");
    std::fs::remove_dir_all(a.path("old")).unwrap();
    let report = a.sync().await;
    assert_eq!(report.deleted_remote, 1);
    assert_eq!(server.calls("delete"), deletes + 1);
    assert_eq!(server.tree("Sync"), ["keep.txt"]);
}

#[tokio::test]
async fn a_folder_deleted_on_one_side_while_the_other_added_to_it_is_kept() {
    let (server, a, _) = setup();
    a.write("shared/old.txt", "o");
    a.sync().await;
    server.remove("Sync/shared");
    a.write("shared/new.txt", "n");
    a.sync().await;
    assert_eq!(server.tree("Sync"), ["shared/", "shared/new.txt"]);
    assert_eq!(a.tree(), ["shared/", "shared/new.txt"]);
}

#[tokio::test]
async fn hidden_folders_stay_where_they_are() {
    let (server, a, _) = setup();
    a.write(".git/HEAD", "ref");
    a.write(".gitignore", "target");
    server.put("Sync/.cache/x", "x");
    a.sync().await;
    assert_eq!(server.tree("Sync"), [".cache/", ".cache/x", ".gitignore"]);
    assert_eq!(a.tree(), [".git/", ".git/HEAD", ".gitignore"]);
}

#[tokio::test]
async fn a_missing_local_folder_stops_everything() {
    let (server, a, _) = setup();
    a.write("precious.txt", "p");
    a.sync().await;
    std::fs::remove_dir_all(a.root()).unwrap(); // as if the drive were unmounted
    let err = a.engine.sync().await.unwrap_err();
    assert!(matches!(err, Error::LocalRootMissing(_)), "{err}");
    assert_eq!(server.read("Sync/precious.txt").as_deref(), Some("p"));
}

#[tokio::test]
async fn a_server_folder_in_the_trash_stops_everything() {
    let (server, a, _) = setup();
    a.write("precious.txt", "p");
    a.sync().await;
    server.remove("Sync");
    let err = a.engine.sync().await.unwrap_err();
    assert!(matches!(err, Error::RemoteRootGone), "{err}");
    assert_eq!(a.read("precious.txt").as_deref(), Some("p"));
}

#[tokio::test]
async fn deleting_most_of_everything_waits_for_a_confirmation() {
    let (server, a, _) = setup();
    for i in 0..60 {
        a.write(&format!("{i:02}.txt"), "x");
    }
    a.sync().await;
    for i in 0..60 {
        server.remove(&format!("Sync/{i:02}.txt"));
    }
    let err = a.engine.sync().await.unwrap_err();
    assert!(matches!(err, Error::TooManyDeletions { count: 60, of: 60 }), "{err}");
    assert_eq!(a.tree().len(), 60, "nothing deleted yet");

    a.engine.allow_mass_delete_once();
    let report = a.sync().await;
    assert_eq!(report.deleted_local, 60);
    assert!(a.tree().is_empty());
}

#[tokio::test]
async fn an_expired_cursor_means_a_full_walk() {
    let (server, a, _) = setup();
    a.write("a.txt", "a");
    a.sync().await;
    server.put("Sync/b.txt", "b");
    server.expire_feed();
    let report = a.sync().await;
    assert_eq!(report.downloaded, 1);
    assert_eq!(a.tree(), ["a.txt", "b.txt"]);
}

#[tokio::test]
async fn a_file_changed_on_the_server_meanwhile_is_not_overwritten() {
    let (server, a, _) = setup();
    a.write("doc.txt", "v1");
    a.sync().await;
    // Once more, so the feed entry of our own upload is behind the cursor.
    assert!(!a.sync().await.changed_anything());
    a.write("doc.txt", "v2 local");
    // The server changes after the engine looked but before it uploads: simulated
    // by a change the feed doesn't show, so the engine's mirror is stale.
    server.put_unseen("Sync/doc.txt", "v2 server");
    let report = a.sync().await;
    assert_eq!(report.postponed, ["doc.txt"], "If-Match refused the upload: {report:?}");
    assert_eq!(server.read("Sync/doc.txt").as_deref(), Some("v2 server"));

    // Once the feed shows it, the next run makes a conflict copy, losing nothing.
    server.put("Sync/doc.txt", "v3 server");
    let report = a.sync().await;
    assert_eq!(report.conflicts, ["doc.txt"]);
    assert_eq!(a.read("doc.txt").as_deref(), Some("v3 server"));
    let copy = a.tree().into_iter().find(|p| p.starts_with("doc (conflict ")).unwrap();
    assert_eq!(a.read(&copy).as_deref(), Some("v2 local"));
}

#[tokio::test]
async fn the_feed_in_small_pages_and_a_burst_of_changes() {
    let (server, a, _) = setup();
    a.sync().await;
    for i in 0..20 {
        server.put(&format!("Sync/burst/{i}.txt"), &i.to_string());
    }
    server.rename("Sync/burst/0.txt", "Sync/burst/zero.txt");
    server.remove("Sync/burst/1.txt");
    let report = a.sync().await;
    assert_eq!(report.downloaded, 19);
    assert_eq!(server.tree("Sync"), a.tree());
}
