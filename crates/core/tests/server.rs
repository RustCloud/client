//! Against a real RustCloud server (see `rc_core::testing`):
//! `RC_TEST_URL=… RC_TEST_USER=… RC_TEST_PASSWORD=… cargo test -p rc-core --features testing`.
//! Each test works in a folder of its own and removes it.

use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use rc_core::changes::ChangeKind;
use rc_core::testing::TestServer;
use rc_core::{Client, NodeUpdate, Tokens};

#[tokio::test]
async fn login_files_and_token_renewal() {
    let Some(server) = TestServer::start().await else { return };
    let tokens = server.log_in("rc-core test").await;

    let saved: Arc<Mutex<Vec<Tokens>>> = Arc::default();
    let client = Client::new(Client::http(), server.url.clone(), tokens.clone()).on_tokens({
        let saved = saved.clone();
        Arc::new(move |t| saved.lock().unwrap().push(t.clone()))
    });
    assert_eq!(client.me().await.unwrap().username, server.username);

    // Folders, upload with the local mtime.
    let top = server.folder(&client, "files").await;
    let docs = client.create_folder(top.id, "Docs").await.unwrap();
    let local = tempfile::tempdir().unwrap();
    let path = local.path().join("report.txt");
    std::fs::write(&path, "first").unwrap();
    let mtime = SystemTime::UNIX_EPOCH + Duration::from_micros(1_700_000_000_123_456);
    std::fs::File::options().write(true).open(&path).unwrap().set_modified(mtime).unwrap();

    let node = client.upload(docs.id, "report.txt", &path, false).await.unwrap();
    assert_eq!(node.size, 5);
    assert_eq!(node.modified_at.unwrap().timestamp_micros(), 1_700_000_000_123_456);
    let taken = client.upload(docs.id, "report.txt", &path, false).await.unwrap_err();
    assert_eq!(taken.code(), Some("NAME_TAKEN"));
    let listed = client.children(docs.id).await.unwrap();
    assert_eq!(listed, vec![node.clone()]);

    // Replace only if unchanged.
    std::fs::write(&path, "second").unwrap();
    let replaced = client.replace(node.id, &path, node.etag.as_deref()).await.unwrap();
    assert_ne!(replaced.etag, node.etag);
    let stale = client.replace(node.id, &path, node.etag.as_deref()).await.unwrap_err();
    assert_eq!(stale.code(), Some("FILE_CHANGED"));

    // Download, then resume a partial one.
    let copy = local.path().join("copy.txt");
    let full = client.download(node.id, &copy, false).await.unwrap();
    assert_eq!(std::fs::read_to_string(&copy).unwrap(), "second");
    assert_eq!(full.etag, replaced.etag);
    std::fs::write(&copy, "sec").unwrap();
    let rest = client.download(node.id, &copy, true).await.unwrap();
    assert!(rest.resumed);
    assert_eq!(std::fs::read_to_string(&copy).unwrap(), "second");
    let again = client.download(node.id, &copy, true).await.unwrap();
    assert_eq!(again.size, 6, "a complete file stays as it is");

    // Rename (to a name nothing else has), search, trash.
    let unique = format!("final-{}.txt", top.id.simple());
    let renamed =
        client.update(node.id, &NodeUpdate { name: Some(unique.clone()), ..Default::default() }).await.unwrap();
    assert_eq!(renamed.modified_at, replaced.modified_at, "renaming keeps the mtime");
    let found = client.search(&unique.to_uppercase()).await.unwrap();
    assert_eq!(found.iter().map(|n| n.id).collect::<Vec<_>>(), [node.id]);
    client.delete(renamed.id).await.unwrap();
    assert!(client.children(docs.id).await.unwrap().is_empty());
    server.remove(&client, top.id).await;

    // A rejected access token (as after a server restart) is renewed once, and saved.
    let mut broken = client.tokens().await;
    broken.access_token = "not-a-token".into();
    let client = Client::new(Client::http(), server.url.clone(), broken).on_tokens({
        let saved = saved.clone();
        Arc::new(move |t| saved.lock().unwrap().push(t.clone()))
    });
    assert_eq!(client.me().await.unwrap().username, server.username);
    assert_eq!(saved.lock().unwrap().len(), 1);
    assert_ne!(saved.lock().unwrap()[0].refresh_token, tokens.refresh_token, "the refresh token rotates");

    // After logout the session is gone for good.
    client.logout().await.unwrap();
    let gone = client.me().await.unwrap_err();
    assert!(gone.needs_login(), "{gone}");
}

#[tokio::test]
async fn the_change_feed_and_its_hints() {
    let Some(server) = TestServer::start().await else { return };
    let client = server.client("rc-core test").await;
    let top = server.folder(&client, "feed").await;

    let start = client.changes(None).await.unwrap();
    assert!(start.changes.is_empty());
    let mut hints = client.watch_changes().await.unwrap();

    let docs = client.create_folder(top.id, "Docs").await.unwrap();
    let hint = tokio::time::timeout(Duration::from_secs(10), hints.next()).await.expect("a hint");
    assert!(hint.unwrap());

    let local = tempfile::tempdir().unwrap();
    let path = local.path().join("a.txt");
    std::fs::write(&path, "a").unwrap();
    let file = client.upload(docs.id, "a.txt", &path, false).await.unwrap();
    client.delete(file.id).await.unwrap();

    // Other activity of the account may be in the feed too: only ours counts.
    let mut cursor = start.cursor.clone();
    let mut seen = Vec::new();
    loop {
        let page = client.changes(Some(&cursor)).await.unwrap();
        seen.extend(page.changes.iter().filter(|c| c.id == docs.id || c.id == file.id).map(|c| (c.id, c.kind)));
        cursor = page.cursor;
        if !page.more {
            break;
        }
    }
    assert_eq!(
        seen,
        vec![(docs.id, ChangeKind::Created), (file.id, ChangeKind::Created), (file.id, ChangeKind::Deleted)]
    );
    let forged = client.changes(Some("not-a-cursor")).await.unwrap_err();
    assert_eq!(forged.code(), Some("INVALID_CURSOR"));

    drop(hints);
    server.remove(&client, top.id).await;
    client.logout().await.unwrap();
}
