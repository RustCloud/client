use std::future::Future;
use std::sync::{Arc, RwLock};
use std::time::Duration as StdDuration;

use chrono::Duration;
use reqwest::{RequestBuilder, Response, StatusCode};
use serde::de::DeserializeOwned;
use tokio::sync::Mutex;
use url::Url;

use crate::auth::{self, api_url, Tokens};
use crate::types::{Usage, User};
use crate::{Error, Result};

/// Called with every new pair of tokens; it must store them before returning
/// (see [`Tokens`] for why).
pub type TokensSaved = Arc<dyn Fn(&Tokens) + Send + Sync>;

/// The server's API for a logged-in device. Cheap to clone; clones share the tokens.
#[derive(Clone)]
pub struct Client {
    pub(crate) http: reqwest::Client,
    server: Url,
    tokens: Arc<Mutex<Tokens>>,
    on_tokens: Option<TokensSaved>,
}

/// CAs to trust besides the public and the system ones; see [`trust_roots`].
static EXTRA_ROOTS: RwLock<Vec<reqwest::Certificate>> = RwLock::new(Vec::new());

/// Trusts these CAs (DER) too, in HTTP clients made from now on ([`Client::http`]):
/// a self-hosted server is often signed by a home CA. On Android, where rustls
/// cannot read the phone's store, the app passes the phone's CAs here at start,
/// including the ones the user installed. Returns how many were taken; broken
/// certificates are skipped.
pub fn trust_roots<C: AsRef<[u8]>>(der: impl IntoIterator<Item = C>) -> usize {
    // reqwest parses them only when building a client, which then fails: check here.
    let usable = |certs: &[reqwest::Certificate]| {
        certs
            .iter()
            .fold(reqwest::Client::builder().tls_built_in_root_certs(false), |b, c| b.add_root_certificate(c.clone()))
            .build()
            .is_ok()
    };
    let mut found: Vec<_> =
        der.into_iter().filter_map(|der| reqwest::Certificate::from_der(der.as_ref()).ok()).collect();
    if !usable(&found) {
        found.retain(|cert| usable(std::slice::from_ref(cert)));
    }
    let taken = found.len();
    EXTRA_ROOTS.write().unwrap_or_else(|e| e.into_inner()).extend(found);
    taken
}

/// Renew the access token this long before it ends.
const RENEW_MARGIN: Duration = Duration::seconds(60);

impl Client {
    /// The HTTP client every request of the app should go through (one connection pool).
    ///
    /// It trusts the public CAs, the system's store (where the OS has one rustls
    /// can read: Linux, Windows) and the CAs given to [`trust_roots`].
    pub fn http() -> reqwest::Client {
        let builder = reqwest::Client::builder()
            .user_agent(concat!("RustCloud-Client/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(StdDuration::from_secs(15))
            // No total timeout: large files take as long as they take.
            .read_timeout(StdDuration::from_secs(120));
        let roots = EXTRA_ROOTS.read().unwrap_or_else(|e| e.into_inner());
        roots
            .iter()
            .fold(builder, |builder, root| builder.add_root_certificate(root.clone()))
            .build()
            .expect("the TLS backend is built in")
    }

    pub fn new(http: reqwest::Client, server: Url, tokens: Tokens) -> Self {
        Self { http, server, tokens: Arc::new(Mutex::new(tokens)), on_tokens: None }
    }

    /// Calls `save` with every renewed pair of tokens.
    pub fn on_tokens(mut self, save: TokensSaved) -> Self {
        self.on_tokens = Some(save);
        self
    }

    pub fn server(&self) -> &Url {
        &self.server
    }

    /// A copy of the current tokens (to store them at exit, for example).
    pub async fn tokens(&self) -> Tokens {
        self.tokens.lock().await.clone()
    }

    /// An access token good for at least a minute, renewed if needed. Only one
    /// renewal runs at a time: the lock is held through it.
    pub async fn access_token(&self) -> Result<String> {
        let mut tokens = self.tokens.lock().await;
        if !tokens.access_valid_for(RENEW_MARGIN) {
            self.renew(&mut tokens).await?;
        }
        Ok(tokens.access_token.clone())
    }

    /// Renews the tokens now, unless another task already did after `seen` was handed out.
    async fn renew_after_rejection(&self, seen: &str) -> Result<()> {
        let mut tokens = self.tokens.lock().await;
        if tokens.access_token == seen {
            self.renew(&mut tokens).await?;
        }
        Ok(())
    }

    async fn renew(&self, tokens: &mut Tokens) -> Result<()> {
        let fresh = auth::refresh(&self.http, &self.server, &tokens.refresh_token).await?;
        if let Some(save) = &self.on_tokens {
            save(&fresh);
        }
        *tokens = fresh;
        tracing::debug!("tokens renewed");
        Ok(())
    }

    pub(crate) fn url(&self, path: &str) -> Result<Url> {
        api_url(&self.server, path)
    }

    /// Sends a request built by `build` with a fresh token. If the server still
    /// says 401 (it restarted and its signing key changed, for one), renews the
    /// tokens once and builds the request again.
    pub(crate) async fn send<F, Fut>(&self, build: F) -> Result<Response>
    where
        F: Fn(String) -> Fut,
        Fut: Future<Output = Result<RequestBuilder>>,
    {
        let token = self.access_token().await?;
        let response = build(token.clone()).await?.send().await?;
        let response = if response.status() == StatusCode::UNAUTHORIZED {
            self.renew_after_rejection(&token).await?;
            let token = self.access_token().await?;
            build(token).await?.send().await?
        } else {
            response
        };
        if response.status().is_success() {
            Ok(response)
        } else {
            Err(Error::from_response(response).await)
        }
    }

    /// A simple request with an optional JSON body.
    pub(crate) async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&serde_json::Value>,
    ) -> Result<Response> {
        let url = self.url(path)?;
        self.send(|token| {
            let mut request = self.http.request(method.clone(), url.clone()).bearer_auth(token);
            if let Some(body) = body {
                request = request.json(body);
            }
            std::future::ready(Ok(request))
        })
        .await
    }

    pub(crate) async fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        Ok(self.request(reqwest::Method::GET, path, None).await?.json().await?)
    }

    /// The logged-in user.
    pub async fn me(&self) -> Result<User> {
        self.get_json("api/v1/auth/me").await
    }

    pub async fn usage(&self) -> Result<Usage> {
        self.get_json("api/v1/usage").await
    }

    /// Ends this device's session on the server. The tokens are useless afterwards.
    pub async fn logout(&self) -> Result<()> {
        self.request(reqwest::Method::POST, "api/v1/auth/logout", None).await?;
        Ok(())
    }
}
