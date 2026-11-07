//! The core of RustCloud clients, shared by every platform and free of UI:
//!
//! - [`auth`]: logging in through the browser (OAuth 2.0 code + PKCE, RFC 8252);
//! - [`Client`]: the server's API with an access token kept fresh;
//! - [`types`]: what the API returns.
//!
//! Next: the sync engine (local state in SQLite) and the host for WASM plugins.

pub mod auth;
mod client;
mod error;
mod files;
pub mod types;

pub use auth::Tokens;
pub use client::{Client, TokensSaved};
pub use error::{Error, Result};
pub use files::{Download, NodeUpdate};
