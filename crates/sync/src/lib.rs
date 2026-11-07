//! Two-way sync of a local folder with a folder on a RustCloud server.
//!
//! Each run ([`Engine::sync`]):
//!
//! 1. brings the **mirror** of the server's tree up to date from the change feed
//!    (a full walk the first time, after `CURSOR_EXPIRED` and once a day);
//! 2. **scans** the local folder (size and mtime; content hashes only when needed);
//! 3. compares, path by path, the local state, the server's and the **base** —
//!    what both sides agreed on after the last run ([`plan`]):
//!    a side that differs from the base changed; one change wins, two changes
//!    of the same content agree, two different ones make a conflict copy;
//! 4. applies the actions, recording each in the base as soon as it is done,
//!    so an interrupted run goes on where it stopped.
//!
//! Safety first: a missing local folder (an unmounted drive) or a server folder
//! that is gone stop the run instead of deleting the other side; deleting more
//! than half of everything needs a confirmation; replacing a file on the server
//! is conditional (`If-Match`), so a change made meanwhile is never overwritten.
//!
//! Folders whose name starts with a dot (`.git`, the server's `.trash` and
//! `.appdata`) are not synced — the server's change feed leaves them out too.
//! Files whose name starts with a dot are synced.
//!
//! Not yet: noticing local moves (a moved file is uploaded anew), case-insensitive
//! file systems (Windows), chunked uploads.

mod engine;
mod error;
pub mod local;
pub mod plan;
pub mod remote;
pub mod state;

#[cfg(test)]
mod fake;
#[cfg(test)]
mod tests;

pub use engine::{Engine, Report};
pub use error::{Error, Result};
pub use remote::Remote;
