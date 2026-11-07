//! Against a real RustCloud server: `RUSTCLOUD_BIN=/path/to/rustcloud cargo test`.
//! Without the variable (and without `../rustcloud/target/debug/rustcloud`) the
//! tests say so and pass, so `cargo test` works anywhere.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use rc_core::auth::login_with_browser;
use rc_core::{Client, NodeUpdate, Tokens};
use serde_json::json;
use url::Url;

struct Server {
    url: Url,
    child: Child,
    _dir: tempfile::TempDir,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn server_binary() -> Option<PathBuf> {
    if let Some(bin) = std::env::var_os("RUSTCLOUD_BIN") {
        return Some(bin.into());
    }
    let sibling = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../rustcloud/target/debug/rustcloud");
    sibling.exists().then_some(sibling)
}

async fn start_server() -> Option<Server> {
    let Some(bin) = server_binary() else {
        eprintln!("skipped: set RUSTCLOUD_BIN to a rustcloud binary to run this test");
        return None;
    };
    let dir = tempfile::tempdir().unwrap();
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let child = Command::new(bin)
        .arg("--config")
        .arg(dir.path().join("rustcloud.toml"))
        .env("RUSTCLOUD_LISTEN", format!("127.0.0.1:{port}"))
        .env("RUSTCLOUD_DATA_DIR", dir.path().join("data"))
        .env("RUSTCLOUD_DATABASE_URL", format!("sqlite://{}?mode=rwc", dir.path().join("db.sqlite").display()))
        .env("RUSTCLOUD_LOG", "warn")
        .stdout(Stdio::null())
        .spawn()
        .expect("start rustcloud");
    let server = Server { url: Url::parse(&format!("http://127.0.0.1:{port}/")).unwrap(), child, _dir: dir };

    let http = reqwest::Client::new();
    for _ in 0..200 {
        if http.get(server.url.join("health").unwrap()).send().await.is_ok_and(|r| r.status().is_success()) {
            return Some(server);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("rustcloud did not start");
}

/// Plays the user in the browser: registers, then allows the device on the consent page.
async fn log_in(server: &Url) -> Tokens {
    let browser = reqwest::Client::builder().cookie_store(true).build().unwrap();
    let account = json!({ "username": "alice", "password": "correct-horse-1" });
    let reply = browser.post(server.join("api/v1/auth/register").unwrap()).json(&account).send().await.unwrap();
    assert!(reply.status().is_success(), "register: {}", reply.text().await.unwrap());

    let server_url = server.clone();
    login_with_browser(&Client::http(), server, "RustCloud Test", "CI", move |authorize| {
        let query: std::collections::HashMap<_, _> = authorize.query_pairs().into_owned().collect();
        tokio::spawn(async move {
            let allow = json!({
                "client_id": query["client_id"], "device_name": query["device_name"],
                "redirect_uri": query["redirect_uri"], "code_challenge": query["code_challenge"],
                "code_challenge_method": query["code_challenge_method"],
            });
            let reply = browser
                .post(server_url.join("api/v1/auth/authorize").unwrap())
                .header("X-Requested-With", "XMLHttpRequest")
                .json(&allow)
                .send()
                .await
                .unwrap();
            assert!(reply.status().is_success(), "authorize: {}", reply.text().await.unwrap());
            let code: serde_json::Value = reply.json().await.unwrap();
            let mut back = Url::parse(&query["redirect_uri"]).unwrap();
            back.query_pairs_mut()
                .append_pair("code", code["code"].as_str().unwrap())
                .append_pair("state", &query["state"]);
            let page = reqwest::get(back).await.unwrap();
            assert!(page.text().await.unwrap().contains("Готово"));
        });
        Ok(())
    })
    .await
    .expect("login")
}

#[tokio::test]
async fn login_files_and_token_renewal() {
    let Some(server) = start_server().await else { return };
    let tokens = log_in(&server.url).await;

    let saved: Arc<Mutex<Vec<Tokens>>> = Arc::default();
    let client = Client::new(Client::http(), server.url.clone(), tokens.clone()).on_tokens({
        let saved = saved.clone();
        Arc::new(move |t| saved.lock().unwrap().push(t.clone()))
    });
    assert_eq!(client.me().await.unwrap().username, "alice");

    // Folders, upload with the local mtime.
    let root = client.root().await.unwrap();
    let docs = client.create_folder(root.id, "Docs").await.unwrap();
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

    // Rename, search, trash.
    let renamed =
        client.update(node.id, &NodeUpdate { name: Some("final.txt".into()), ..Default::default() }).await.unwrap();
    assert_eq!(renamed.modified_at, replaced.modified_at, "renaming keeps the mtime");
    assert_eq!(client.search("FINAL").await.unwrap().len(), 1);
    client.delete(renamed.id).await.unwrap();
    assert!(client.children(docs.id).await.unwrap().is_empty());

    // A rejected access token (as after a server restart) is renewed once, and saved.
    let mut broken = client.tokens().await;
    broken.access_token = "not-a-token".into();
    let client = Client::new(Client::http(), server.url.clone(), broken).on_tokens({
        let saved = saved.clone();
        Arc::new(move |t| saved.lock().unwrap().push(t.clone()))
    });
    assert_eq!(client.me().await.unwrap().username, "alice");
    assert_eq!(saved.lock().unwrap().len(), 1);
    assert_ne!(saved.lock().unwrap()[0].refresh_token, tokens.refresh_token, "the refresh token rotates");

    // After logout the session is gone for good.
    client.logout().await.unwrap();
    let gone = client.me().await.unwrap_err();
    assert!(gone.needs_login(), "{gone}");
}
