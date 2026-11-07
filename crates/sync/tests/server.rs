//! Sync against a real RustCloud server (see `rc_core::testing`): two devices
//! of one account share a folder of the test's own.

use std::path::{Path, PathBuf};

use rc_core::testing::TestServer;
use rc_core::{Client, NodeUpdate};
use rc_sync::{Engine, Report};
use uuid::Uuid;

struct Device {
    client: Client,
    dir: tempfile::TempDir,
    engine: Engine<Client>,
}

impl Device {
    async fn new(server: &TestServer, name: &str, folder: Uuid) -> Self {
        let client = server.client(name).await;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("Sync");
        std::fs::create_dir(&root).unwrap();
        let engine = Engine::open(client.clone(), &dir.path().join("state.sqlite"), root, folder).unwrap();
        Self { client, dir, engine }
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.dir.path().join("Sync").join(rel)
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

    fn tree(&self) -> Vec<String> {
        fn walk(dir: &Path, rel: &str, out: &mut Vec<String>) {
            for e in std::fs::read_dir(dir).unwrap() {
                let e = e.unwrap();
                let name = e.file_name().into_string().unwrap();
                let p = if rel.is_empty() { name } else { format!("{rel}/{name}") };
                if e.file_type().unwrap().is_dir() {
                    out.push(format!("{p}/"));
                    walk(&e.path(), &p, out);
                } else {
                    out.push(p);
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.dir.path().join("Sync"), "", &mut out);
        out.sort();
        out
    }
}

#[tokio::test]
async fn two_devices_keep_one_folder_in_sync() {
    let Some(server) = TestServer::start().await else { return };
    let owner = server.client("rc-sync test").await;
    let folder = server.folder(&owner, "sync").await;
    let a = Device::new(&server, "rc-sync test A", folder.id).await;
    let b = Device::new(&server, "rc-sync test B", folder.id).await;

    // A puts files up, B gets them.
    a.write("notes/plan.md", "v1");
    a.write("photos/2024/a.jpg", "jpeg a");
    a.write(".gitignore", "target");
    a.write(".git/HEAD", "not synced");
    let report = a.sync().await;
    assert_eq!(report.uploaded, 3);
    let report = b.sync().await;
    assert_eq!(report.downloaded, 3);
    assert_eq!(b.tree(), [".gitignore", "notes/", "notes/plan.md", "photos/", "photos/2024/", "photos/2024/a.jpg"]);

    // Both edit the same file: the second one gets a conflict copy, which then reaches A.
    a.write("notes/plan.md", "v2 from A");
    a.sync().await;
    b.write("notes/plan.md", "v2 from B, longer");
    let report = b.sync().await;
    assert_eq!(report.conflicts, ["notes/plan.md"]);
    assert_eq!(b.read("notes/plan.md").as_deref(), Some("v2 from A"));
    b.sync().await;
    a.sync().await;
    let copy = a.tree().into_iter().find(|p| p.starts_with("notes/plan (conflict ")).expect("the copy on A");
    assert_eq!(a.read(&copy).as_deref(), Some("v2 from B, longer"));

    // A rename on the server (from the web, say) is a rename on the devices.
    let children = owner.children(folder.id).await.unwrap();
    let photos = children.iter().find(|n| n.name == "photos").unwrap();
    owner.update(photos.id, &NodeUpdate { name: Some("Pictures".into()), ..Default::default() }).await.unwrap();
    let report = b.sync().await;
    assert_eq!((report.moved_local, report.downloaded), (1, 0));
    assert_eq!(b.read("Pictures/2024/a.jpg").as_deref(), Some("jpeg a"));

    // A deletion travels too; the two devices end up the same.
    let report = a.sync().await;
    assert_eq!(report.moved_local, 1);
    std::fs::remove_dir_all(a.path("Pictures")).unwrap();
    assert_eq!(a.sync().await.deleted_remote, 1);
    assert_eq!(b.sync().await.deleted_local, 3, "a.jpg, 2024/, Pictures/");
    let mut tree_a = a.tree();
    tree_a.retain(|p| !p.starts_with(".git/")); // not synced
    assert_eq!(tree_a, b.tree());
    assert!(!b.tree().iter().any(|p| p.starts_with("Pictures")), "{:?}", b.tree());

    // A file deleted locally while it changed on the server comes back: the change wins.
    std::fs::remove_file(b.path("notes/plan.md")).unwrap();
    a.write("notes/plan.md", "v3 from A");
    a.sync().await;
    b.sync().await;
    assert_eq!(b.read("notes/plan.md").as_deref(), Some("v3 from A"));

    for device in [&a, &b] {
        device.client.logout().await.unwrap();
    }
    server.remove(&owner, folder.id).await;
    owner.logout().await.unwrap();
}
