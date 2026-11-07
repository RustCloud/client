//! Logging in through the browser: OAuth 2.0 authorization code with PKCE for
//! native apps (RFC 8252), as the server documents in `docs/api/auth.md`
//! ("Вход приложений").
//!
//! 1. [`login_with_browser`] listens on `127.0.0.1:<free port>` and gives the
//!    caller the authorize URL to open in the system browser;
//! 2. the user logs in (password, OIDC — whatever the server has) and allows the device;
//! 3. the browser comes back to `http://127.0.0.1:<port>/callback?code=…`, and the
//!    code is exchanged for [`Tokens`].
//!
//! Android can't listen reliably while the browser is in front: there the app
//! registers a private scheme (`com.rustcloud.client:/callback`), builds the URL
//! with [`AuthorizeUrl`] and gives the code it gets to [`exchange_code`].

use std::fmt;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::{DateTime, Duration, Utc};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use url::Url;

use crate::{Error, Result};

/// The pair of tokens of a device session.
///
/// **The refresh token changes on every renewal and the old one stops working.**
/// Presenting an old one later than 30 s after the renewal ends the session
/// (the server takes it for a stolen copy), so whoever stores tokens must save
/// every new pair before using it — see [`crate::Client::on_tokens`].
#[derive(Clone, Serialize, Deserialize)]
pub struct Tokens {
    pub access_token: String,
    pub access_expires_at: DateTime<Utc>,
    pub refresh_token: String,
    pub refresh_expires_at: DateTime<Utc>,
}

impl Tokens {
    /// Whether the access token is still good for `margin` from now.
    pub fn access_valid_for(&self, margin: Duration) -> bool {
        Utc::now() + margin < self.access_expires_at
    }
}

// Never print tokens: logs end up in bug reports.
impl fmt::Debug for Tokens {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Tokens")
            .field("access_expires_at", &self.access_expires_at)
            .field("refresh_expires_at", &self.refresh_expires_at)
            .finish_non_exhaustive()
    }
}

/// A PKCE pair (RFC 7636): the app keeps `verifier`, the browser carries `challenge`.
pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
}

impl Pkce {
    pub fn new() -> Self {
        let verifier = random_token(32); // 43 characters
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        Self { verifier, challenge }
    }
}

impl Default for Pkce {
    fn default() -> Self {
        Self::new()
    }
}

/// 32 random bytes → 43 URL-safe characters.
fn random_token(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::rngs::OsRng.fill_bytes(&mut buf);
    URL_SAFE_NO_PAD.encode(buf)
}

/// Everything one login attempt needs to remember.
pub struct AuthorizeUrl {
    /// What to open in the browser.
    pub url: Url,
    pub redirect_uri: String,
    pub state: String,
    pub pkce: Pkce,
}

impl AuthorizeUrl {
    /// `client_id` names the app ("RustCloud Desktop"), `device_name` the device
    /// ("Anna's laptop"); the user sees both on the consent page and in the list of sessions.
    pub fn new(server: &Url, client_id: &str, device_name: &str, redirect_uri: &str) -> Result<Self> {
        let pkce = Pkce::new();
        let state = random_token(16);
        let mut url = api_url(server, "api/v1/auth/authorize")?;
        url.query_pairs_mut()
            .append_pair("response_type", "code")
            .append_pair("client_id", client_id)
            .append_pair("device_name", device_name)
            .append_pair("redirect_uri", redirect_uri)
            .append_pair("code_challenge", &pkce.challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("state", &state);
        Ok(Self { url, redirect_uri: redirect_uri.to_owned(), state, pkce })
    }

    /// Checks what came back to the redirect URI and returns the code.
    pub fn code_from_redirect(&self, redirect: &Url) -> Result<String> {
        let param = |name: &str| redirect.query_pairs().find(|(k, _)| k == name).map(|(_, v)| v.into_owned());
        if param("state").as_deref() != Some(self.state.as_str()) {
            return Err(auth_error("invalid_state", "the answer belongs to another login attempt"));
        }
        if let Some(error) = param("error") {
            let description = param("error_description").unwrap_or_else(|| "the user did not allow the device".into());
            return Err(auth_error(&error, &description));
        }
        param("code").ok_or_else(|| auth_error("invalid_request", "no code in the answer"))
    }
}

/// The whole login on a desktop: listens on loopback, calls `open_browser` with the
/// URL to show, waits for the browser to come back and exchanges the code.
///
/// It waits as long as the user takes; wrap it in `tokio::time::timeout` and
/// drop it to cancel.
pub async fn login_with_browser(
    http: &reqwest::Client,
    server: &Url,
    client_id: &str,
    device_name: &str,
    open_browser: impl FnOnce(&Url) -> Result<()>,
) -> Result<Tokens> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let redirect_uri = format!("http://127.0.0.1:{}/callback", listener.local_addr()?.port());
    let attempt = AuthorizeUrl::new(server, client_id, device_name, &redirect_uri)?;
    open_browser(&attempt.url)?;

    let code = loop {
        let (mut stream, _) = listener.accept().await?;
        let Some(target) = read_request_target(&mut stream).await else { continue };
        // Browsers also ask for /favicon.ico and the like.
        if !target.starts_with("/callback") {
            let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
            continue;
        }
        let redirect = Url::parse(&format!("http://127.0.0.1{target}")).map_err(|e| Error::Protocol(e.to_string()))?;
        let result = attempt.code_from_redirect(&redirect);
        // Someone else's request (wrong state) must not end the wait.
        if matches!(&result, Err(e) if e.code() == Some("invalid_state")) {
            let _ =
                stream.write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
            continue;
        }
        let page = if result.is_ok() { DONE_PAGE } else { DENIED_PAGE };
        let reply = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{page}",
            page.len()
        );
        let _ = stream.write_all(reply.as_bytes()).await;
        let _ = stream.shutdown().await;
        break result?;
    };
    exchange_code(http, server, &code, &attempt.pkce.verifier, &attempt.redirect_uri).await
}

const DONE_PAGE: &str = "<!doctype html><meta charset=utf-8><title>RustCloud</title>\
<p style=\"font:16px sans-serif;text-align:center;margin-top:20vh\">Готово — вернитесь в приложение. Вкладку можно закрыть.</p>";
const DENIED_PAGE: &str = "<!doctype html><meta charset=utf-8><title>RustCloud</title>\
<p style=\"font:16px sans-serif;text-align:center;margin-top:20vh\">Вход отменён. Вкладку можно закрыть.</p>";

/// The target of the first request line (`GET /callback?… HTTP/1.1`).
async fn read_request_target(stream: &mut tokio::net::TcpStream) -> Option<String> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
        let read =
            tokio::time::timeout(std::time::Duration::from_secs(10), stream.read(&mut chunk)).await.ok()?.ok()?;
        if read == 0 || buf.len() > 16 * 1024 {
            return None;
        }
        buf.extend_from_slice(&chunk[..read]);
    }
    let head = String::from_utf8_lossy(&buf);
    let mut parts = head.lines().next()?.split(' ');
    (parts.next()? == "GET").then(|| parts.next().map(str::to_owned))?
}

/// Step 3: the code for a pair of tokens.
pub async fn exchange_code(
    http: &reqwest::Client,
    server: &Url,
    code: &str,
    verifier: &str,
    redirect_uri: &str,
) -> Result<Tokens> {
    let form = [
        ("grant_type", "authorization_code"),
        ("code", code),
        ("code_verifier", verifier),
        ("redirect_uri", redirect_uri),
    ];
    token_request(http, server, &form).await
}

/// A new pair for the refresh token; the old refresh token stops working.
pub async fn refresh(http: &reqwest::Client, server: &Url, refresh_token: &str) -> Result<Tokens> {
    token_request(http, server, &[("grant_type", "refresh_token"), ("refresh_token", refresh_token)]).await
}

async fn token_request(http: &reqwest::Client, server: &Url, form: &[(&str, &str)]) -> Result<Tokens> {
    #[derive(Deserialize)]
    struct Reply {
        access_token: String,
        expires_in: i64,
        refresh_token: String,
        refresh_expires_at: DateTime<Utc>,
    }
    #[derive(Deserialize)]
    struct OauthError {
        error: String,
        #[serde(default)]
        error_description: String,
    }

    let asked_at = Utc::now();
    let response = http.post(api_url(server, "api/v1/auth/token")?).form(form).send().await?;
    let status = response.status();
    let text = response.text().await?;
    if !status.is_success() {
        return Err(match serde_json::from_str::<OauthError>(&text) {
            Ok(e) => auth_error(&e.error, &e.error_description),
            Err(_) => Error::Api { status, code: format!("HTTP_{}", status.as_u16()), message: text },
        });
    }
    let reply: Reply = serde_json::from_str(&text).map_err(|e| Error::Protocol(format!("token reply: {e}")))?;
    Ok(Tokens {
        access_token: reply.access_token,
        // Counted from when we asked, so a slow answer doesn't make it look fresher.
        access_expires_at: asked_at + Duration::seconds(reply.expires_in),
        refresh_token: reply.refresh_token,
        refresh_expires_at: reply.refresh_expires_at,
    })
}

fn auth_error(code: &str, description: &str) -> Error {
    Error::Auth { code: code.to_owned(), description: description.to_owned() }
}

/// `path` under the server's address, which may itself sit under a path
/// (`https://example.com/cloud/`).
pub(crate) fn api_url(server: &Url, path: &str) -> Result<Url> {
    let mut base = server.clone();
    if !base.path().ends_with('/') {
        base.set_path(&format!("{}/", base.path()));
    }
    Ok(base.join(path)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_follows_rfc_7636() {
        let pkce = Pkce::new();
        assert_eq!(pkce.verifier.len(), 43);
        assert_eq!(pkce.challenge, URL_SAFE_NO_PAD.encode(Sha256::digest(pkce.verifier.as_bytes())));
        // The example of RFC 7636, appendix B.
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(b"dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"));
        assert_eq!(challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    #[test]
    fn api_urls_keep_the_servers_path() {
        let server = Url::parse("https://example.com/cloud").unwrap();
        assert_eq!(
            api_url(&server, "api/v1/files/root").unwrap().as_str(),
            "https://example.com/cloud/api/v1/files/root"
        );
        let server = Url::parse("http://127.0.0.1:8080").unwrap();
        assert_eq!(api_url(&server, "api/v1/auth/token").unwrap().as_str(), "http://127.0.0.1:8080/api/v1/auth/token");
    }

    #[test]
    fn the_redirect_is_checked() {
        let server = Url::parse("http://localhost:8080").unwrap();
        let attempt = AuthorizeUrl::new(&server, "Test", "Laptop", "http://127.0.0.1:1/callback").unwrap();
        let back = |q: &str| Url::parse(&format!("http://127.0.0.1:1/callback?{q}")).unwrap();

        let ok = attempt.code_from_redirect(&back(&format!("code=abc&state={}", attempt.state)));
        assert_eq!(ok.unwrap(), "abc");
        let other = attempt.code_from_redirect(&back("code=abc&state=forged")).unwrap_err();
        assert_eq!(other.code(), Some("invalid_state"));
        let denied = attempt.code_from_redirect(&back(&format!("error=access_denied&state={}", attempt.state)));
        let denied = denied.unwrap_err();
        assert_eq!(denied.code(), Some("access_denied"));
        assert!(denied.needs_login());
    }
}
