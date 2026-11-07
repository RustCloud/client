//! A real RustCloud server for tests (feature `testing`), one of:
//!
//! - **a running one**: `RC_TEST_URL`, `RC_TEST_USER`, `RC_TEST_PASSWORD` — an
//!   existing account. Tests work only in their own folder under
//!   `rc-client-tests/`, delete it for good and end the sessions they opened;
//! - **a fresh one** (CI): `RC_TEST_SERVER` names a rustcloud binary, started
//!   with its own data; the first account is created on first use.
//!
//! Without either, [`TestServer::start`] returns `None` and tests skip.

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use serde_json::json;
use url::Url;
use uuid::Uuid;

use crate::auth::login_with_browser;
use crate::types::Node;
use crate::{Client, Tokens};

/// Where tests put their folders on the server.
pub const TESTS_FOLDER: &str = "rc-client-tests";

pub struct TestServer {
    pub url: Url,
    pub username: String,
    password: String,
    /// A server this harness started: stopped when dropped.
    owned: Option<(Child, tempfile::TempDir)>,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        if let Some((child, _)) = &mut self.owned {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl TestServer {
    pub async fn start() -> Option<Self> {
        if let Some(url) = std::env::var_os("RC_TEST_URL") {
            let url = Url::parse(&url.to_string_lossy()).expect("RC_TEST_URL is a URL");
            let username = std::env::var("RC_TEST_USER").expect("RC_TEST_USER with RC_TEST_URL");
            let password = std::env::var("RC_TEST_PASSWORD").expect("RC_TEST_PASSWORD with RC_TEST_URL");
            return Some(Self { url, username, password, owned: None });
        }
        if let Some(bin) = std::env::var_os("RC_TEST_SERVER") {
            return Some(Self::spawn(PathBuf::from(bin)).await);
        }
        eprintln!("skipped: set RC_TEST_URL (+ RC_TEST_USER, RC_TEST_PASSWORD) or RC_TEST_SERVER to run this test");
        None
    }

    async fn spawn(bin: PathBuf) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let log_path = dir.path().join("server.log");
        let log = std::fs::File::create(&log_path).unwrap();
        let mut command = Command::new(bin);
        // The server takes every RUSTCLOUD_* variable for a setting and refuses unknown ones.
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("RUSTCLOUD_") {
                command.env_remove(key);
            }
        }
        let child = command
            .arg("--config")
            .arg(dir.path().join("rustcloud.toml"))
            .env("RUSTCLOUD_LISTEN", format!("127.0.0.1:{port}"))
            .env("RUSTCLOUD_DATA_DIR", dir.path().join("data"))
            .env("RUSTCLOUD_DATABASE_URL", format!("sqlite://{}?mode=rwc", dir.path().join("db.sqlite").display()))
            .env("RUSTCLOUD_LOG", "warn")
            .env("NO_COLOR", "1")
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .spawn()
            .expect("start rustcloud");
        let mut server = Self {
            url: Url::parse(&format!("http://127.0.0.1:{port}/")).unwrap(),
            username: "alice".into(),
            password: "correct-horse-1".into(),
            owned: Some((child, dir)),
        };

        let http = reqwest::Client::new();
        let started = Instant::now();
        // A debug build on a slow CI runner may take a while.
        while started.elapsed() < Duration::from_secs(60) {
            if http.get(server.url.join("health").unwrap()).send().await.is_ok_and(|r| r.status().is_success()) {
                return server;
            }
            if let Some(status) = server.owned.as_mut().unwrap().0.try_wait().unwrap() {
                panic!("rustcloud exited with {status}:\n{}", tail(&log_path));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("rustcloud did not answer within 60 s:\n{}", tail(&log_path));
    }

    /// Tokens of a new device: logs in with the password as the user would in
    /// the browser, allows the device, then ends that browser session.
    pub async fn log_in(&self, device: &str) -> Tokens {
        let browser = reqwest::Client::builder().cookie_store(true).build().unwrap();
        let account = json!({ "username": self.username, "password": self.password });
        let login = browser.post(self.url.join("api/v1/auth/login").unwrap()).json(&account).send().await.unwrap();
        if !login.status().is_success() {
            assert!(self.owned.is_some(), "cannot log in as {}: {}", self.username, login.text().await.unwrap());
            let reply =
                browser.post(self.url.join("api/v1/auth/register").unwrap()).json(&account).send().await.unwrap();
            assert!(reply.status().is_success(), "register: {}", reply.text().await.unwrap());
        }

        let server = self.url.clone();
        let tokens = login_with_browser(&Client::http(), &self.url, "RustCloud Test", device, {
            let browser = browser.clone();
            move |authorize| {
                let query: std::collections::HashMap<_, _> = authorize.query_pairs().into_owned().collect();
                tokio::spawn(async move {
                    let allow = json!({
                        "client_id": query["client_id"], "device_name": query["device_name"],
                        "redirect_uri": query["redirect_uri"], "code_challenge": query["code_challenge"],
                        "code_challenge_method": query["code_challenge_method"],
                    });
                    let reply = browser
                        .post(server.join("api/v1/auth/authorize").unwrap())
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
            }
        })
        .await
        .expect("login");

        let logout = browser
            .post(self.url.join("api/v1/auth/logout").unwrap())
            .header("X-Requested-With", "XMLHttpRequest")
            .send()
            .await
            .unwrap();
        assert!(logout.status().is_success(), "browser logout: {}", logout.status());
        tokens
    }

    /// A logged-in client of a new device; end it with [`Client::logout`].
    pub async fn client(&self, device: &str) -> Client {
        Client::new(Client::http(), self.url.clone(), self.log_in(device).await)
    }

    /// An empty folder of the test's own: `rc-client-tests/<name>-<random>`.
    pub async fn folder(&self, client: &Client, name: &str) -> Node {
        let root = client.root().await.unwrap();
        let tests = match client.create_folder(root.id, TESTS_FOLDER).await {
            Ok(folder) => folder,
            Err(e) if e.code() == Some("NAME_TAKEN") => {
                client.children(root.id).await.unwrap().into_iter().find(|n| n.name == TESTS_FOLDER).unwrap()
            }
            Err(e) => panic!("{e}"),
        };
        // Left by tests that failed before cleaning up.
        let stale = chrono::Utc::now() - chrono::Duration::hours(1);
        for old in client.children(tests.id).await.unwrap().into_iter().filter(|n| n.created_at < stale) {
            let _ = client.delete(old.id).await;
            let _ = client.delete(old.id).await;
        }
        let unique = Uuid::new_v4().simple().to_string();
        client.create_folder(tests.id, &format!("{name}-{}", &unique[..8])).await.unwrap()
    }

    /// Deletes a test's folder for good (to the trash, then from it).
    pub async fn remove(&self, client: &Client, folder: Uuid) {
        client.delete(folder).await.unwrap();
        client.delete(folder).await.unwrap();
    }
}

fn tail(path: &Path) -> String {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let lines: Vec<_> = text.lines().collect();
    lines[lines.len().saturating_sub(20)..].join("\n")
}
