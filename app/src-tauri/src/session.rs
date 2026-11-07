//! The saved login: server address and tokens in `session.json` in the app's
//! data folder, readable only by the user.
//!
//! TODO: the system keychain (Secret Service, Windows Credential Manager,
//! Android Keystore) instead of a file.

use std::path::PathBuf;
use std::sync::Arc;

use rc_core::{Client, Tokens};
use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Serialize, Deserialize)]
pub struct Saved {
    pub server: Url,
    pub tokens: Tokens,
}

#[derive(Clone)]
pub struct Store {
    file: PathBuf,
}

impl Store {
    pub fn new(dir: PathBuf) -> Self {
        Self { file: dir.join("session.json") }
    }

    pub fn load(&self) -> Option<Saved> {
        let text = std::fs::read_to_string(&self.file).ok()?;
        serde_json::from_str(&text).map_err(|e| tracing::warn!("session.json is unreadable: {e}")).ok()
    }

    /// Writes a new file and renames it over the old one: a crash midway must not
    /// lose the refresh token, which the server has already replaced.
    pub fn save(&self, server: &Url, tokens: &Tokens) -> std::io::Result<()> {
        let dir = self.file.parent().expect("a file in a folder");
        std::fs::create_dir_all(dir)?;
        let text = serde_json::to_vec(&Saved { server: server.clone(), tokens: tokens.clone() })?;
        let tmp = self.file.with_extension("json.tmp");
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        std::io::Write::write_all(&mut options.open(&tmp)?, &text)?;
        std::fs::rename(&tmp, &self.file)
    }

    pub fn forget(&self) {
        let _ = std::fs::remove_file(&self.file);
    }

    /// A client that saves every renewed pair of tokens here.
    pub fn client(&self, server: Url, tokens: Tokens) -> Client {
        let store = self.clone();
        let saved_server = server.clone();
        Client::new(Client::http(), server, tokens).on_tokens(Arc::new(move |tokens| {
            if let Err(e) = store.save(&saved_server, tokens) {
                tracing::error!("could not save the renewed tokens: {e}");
            }
        }))
    }
}

/// What the user typed → the server's address: `cloud.example.com` means https.
pub fn server_url(input: &str) -> Result<Url, String> {
    let input = input.trim();
    if input.is_empty() {
        return Err("Укажите адрес сервера".into());
    }
    let with_scheme = if input.contains("://") { input.to_owned() } else { format!("https://{input}") };
    let url = Url::parse(&with_scheme).map_err(|e| format!("Неверный адрес сервера: {e}"))?;
    if !matches!(url.scheme(), "http" | "https") || url.host().is_none() {
        return Err("Адрес сервера должен начинаться с https:// или http://".into());
    }
    Ok(url)
}

/// The name the server shows in the list of sessions.
pub fn device_name() -> String {
    let host = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .or_else(|| std::fs::read_to_string("/etc/hostname").ok())
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty());
    let os = std::env::consts::OS;
    match host {
        Some(host) => format!("{host} ({os})"),
        None => format!("RustCloud ({os})"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_addresses() {
        assert_eq!(server_url("cloud.example.com").unwrap().as_str(), "https://cloud.example.com/");
        assert_eq!(server_url(" http://10.0.0.5:8080 ").unwrap().as_str(), "http://10.0.0.5:8080/");
        assert!(server_url("").is_err());
        assert!(server_url("ftp://example.com").is_err());
    }

    #[test]
    fn the_session_survives_a_restart() {
        let dir = std::env::temp_dir().join(format!("rc-session-{}", std::process::id()));
        let store = Store::new(dir.clone());
        let tokens = Tokens {
            access_token: "a".into(),
            access_expires_at: chrono_now(),
            refresh_token: "r".into(),
            refresh_expires_at: chrono_now(),
        };
        let server = server_url("cloud.example.com").unwrap();
        store.save(&server, &tokens).unwrap();
        let loaded = store.load().unwrap();
        assert_eq!((loaded.server, loaded.tokens.refresh_token), (server, "r".to_owned()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join("session.json")).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        store.forget();
        assert!(store.load().is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    fn chrono_now() -> chrono::DateTime<chrono::Utc> {
        chrono::Utc::now()
    }
}
