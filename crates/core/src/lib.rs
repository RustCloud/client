//! The core of RustCloud clients, shared by every platform and free of UI:
//!
//! - [`auth`]: logging in through the browser (OAuth 2.0 code + PKCE, RFC 8252);
//! - [`Client`]: the server's API with an access token kept fresh;
//! - [`changes`]: the change feed and its hints, for sync;
//! - [`types`]: what the API returns.

pub mod auth;
pub mod changes;
mod client;
mod error;
mod files;
#[cfg(feature = "testing")]
pub mod testing;
pub mod types;

pub use auth::Tokens;
pub use client::{Client, TokensSaved};
pub use error::{Error, Result};
pub use files::{Download, NodeUpdate};
