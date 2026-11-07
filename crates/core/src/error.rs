use reqwest::StatusCode;
use serde::Deserialize;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The server refused: `{"error": {"code": "FILE_NOT_FOUND", "message": "…"}}`.
    #[error("{message} ({code}, HTTP {status})")]
    Api { status: StatusCode, code: String, message: String },
    /// Logging in or renewing the tokens failed (OAuth `invalid_grant`, `access_denied`…).
    /// Unless `code` is `slow_down`, the user has to log in again.
    #[error("login failed: {code}: {description}")]
    Auth { code: String, description: String },
    #[error("network: {0}")]
    Http(#[from] reqwest::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("bad server address: {0}")]
    Url(#[from] url::ParseError),
    /// The server answered something this client can't understand.
    #[error("unexpected answer from the server: {0}")]
    Protocol(String),
}

impl Error {
    /// The API error code (`FILE_CHANGED`, `NAME_TAKEN`…), if the server gave one.
    pub fn code(&self) -> Option<&str> {
        match self {
            Self::Api { code, .. } | Self::Auth { code, .. } => Some(code),
            _ => None,
        }
    }

    /// Whether the session is gone and only a new login helps.
    pub fn needs_login(&self) -> bool {
        match self {
            Self::Auth { code, .. } => code != "slow_down",
            Self::Api { status, .. } => *status == StatusCode::UNAUTHORIZED,
            _ => false,
        }
    }

    /// Whether trying again later may work: network trouble, an overloaded server.
    pub fn is_transient(&self) -> bool {
        match self {
            Self::Http(e) => e.is_timeout() || e.is_connect() || e.is_request() || e.is_body(),
            Self::Api { status, .. } => status.is_server_error() || *status == StatusCode::TOO_MANY_REQUESTS,
            Self::Auth { code, .. } => code == "slow_down",
            _ => false,
        }
    }

    /// Reads a failed API response.
    pub(crate) async fn from_response(response: reqwest::Response) -> Self {
        #[derive(Deserialize)]
        struct Body {
            error: Inner,
        }
        #[derive(Deserialize)]
        struct Inner {
            code: String,
            #[serde(default)]
            message: String,
        }
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        match serde_json::from_str::<Body>(&text) {
            Ok(body) => Self::Api { status, code: body.error.code, message: body.error.message },
            Err(_) => Self::Api {
                status,
                code: format!("HTTP_{}", status.as_u16()),
                message: text.chars().take(200).collect(),
            },
        }
    }
}
