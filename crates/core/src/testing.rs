//! A real RustCloud server for tests (feature `testing`): `RC_TEST_SERVER` names
//! the binary (a build of github.com/RustCloud/server); without it
//! [`TestServer::start`] returns `None` and tests skip.

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use serde_json::json;
use url::Url;

use crate::auth::login_with_browser;
use crate::{Client, Tokens};

pub const PASSWORD: &str = "correct-horse-1";

pub struct TestServer {
    pub url: Url,
    child: Child,
    _dir: tempfile::TempDir,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn server_binary() -> Option<PathBuf> {
    std::env::var_os("RC_TEST_SERVER").map(PathBuf::from)
}

impl TestServer {
    /// A fresh server with its own data; `None` (and a note) without a binary.
    pub async fn start() -> Option<Self> {
        let Some(bin) = server_binary() else {
            eprintln!("skipped: set RC_TEST_SERVER to a rustcloud binary to run this test");
            return None;
        };
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
        let mut server = Self { url: Url::parse(&format!("http://127.0.0.1:{port}/")).unwrap(), child, _dir: dir };

        let http = reqwest::Client::new();
        let started = Instant::now();
        // A debug build on a slow CI runner may take a while.
        while started.elapsed() < Duration::from_secs(60) {
            if http.get(server.url.join("health").unwrap()).send().await.is_ok_and(|r| r.status().is_success()) {
                return Some(server);
            }
            if let Some(status) = server.child.try_wait().unwrap() {
                panic!("rustcloud exited with {status}:\n{}", tail(&log_path));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("rustcloud did not answer within 60 s:\n{}", tail(&log_path));
    }

    /// Tokens of a new device of `username`: the account is created on first use
    /// (the first one is the admin), then the browser flow is played through.
    pub async fn log_in(&self, username: &str, device: &str) -> Tokens {
        let browser = reqwest::Client::builder().cookie_store(true).build().unwrap();
        let account = json!({ "username": username, "password": PASSWORD });
        let login = browser.post(self.url.join("api/v1/auth/login").unwrap()).json(&account).send().await.unwrap();
        if !login.status().is_success() {
            let reply =
                browser.post(self.url.join("api/v1/auth/register").unwrap()).json(&account).send().await.unwrap();
            assert!(reply.status().is_success(), "register: {}", reply.text().await.unwrap());
        }

        let server = self.url.clone();
        login_with_browser(&Client::http(), &self.url, "RustCloud Test", device, move |authorize| {
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
        })
        .await
        .expect("login")
    }

    /// A logged-in client for a new device of `username`.
    pub async fn client(&self, username: &str, device: &str) -> Client {
        Client::new(Client::http(), self.url.clone(), self.log_in(username, device).await)
    }
}

fn tail(path: &Path) -> String {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let lines: Vec<_> = text.lines().collect();
    lines[lines.len().saturating_sub(20)..].join("\n")
}
