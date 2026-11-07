use std::future::Future;
use std::sync::Arc;
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

/// Renew the access token this long before it ends.
const RENEW_MARGIN: Duration = Duration::seconds(60);

impl Client {
    /// The HTTP client every request of the app should go through (one connection pool).
    pub fn http() -> reqwest::Client {
        reqwest::Client::builder()
            .user_agent(concat!("RustCloud-Client/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(StdDuration::from_secs(15))
            // No total timeout: large files take as long as they take.
            .read_timeout(StdDuration::from_secs(120))
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
