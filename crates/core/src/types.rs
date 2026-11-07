//! What the server's API returns (see `docs/api` in the RustCloud repository).
//! Unknown fields are ignored, so newer servers don't break older clients.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A file or folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Node {
    pub id: Uuid,
    /// `None` for the root folder.
    pub parent_id: Option<Uuid>,
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub mime_type: Option<String>,
    /// SHA-256 of the content; `None` for folders.
    pub etag: Option<String>,
    pub created_at: DateTime<Utc>,
    /// When the server last changed it: content, name or place.
    pub updated_at: DateTime<Utc>,
    /// The mtime the uploading client gave, or the upload time. `None` from servers
    /// older than the field.
    #[serde(default)]
    pub modified_at: Option<DateTime<Utc>>,
    /// Set for what lies in the trash.
    #[serde(default)]
    pub trashed_at: Option<DateTime<Utc>>,
    /// An app (an office editor) holds the file: only it may change or delete it.
    #[serde(default)]
    pub lock: Option<FileLock>,
}

impl Node {
    /// The content time to give the local copy.
    pub fn mtime(&self) -> DateTime<Utc> {
        self.modified_at.unwrap_or(self.updated_at)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileLock {
    pub lock_id: String,
    pub app_id: String,
    pub user_id: Uuid,
    pub expires_at: DateTime<Utc>,
}

/// `GET /files/{id}`: the node and the folders above it, for breadcrumbs.
#[derive(Debug, Clone, Deserialize)]
pub struct NodeDetails {
    pub node: Node,
    pub path: Vec<Node>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct Usage {
    /// With the trash and versions.
    pub used_bytes: u64,
    pub versions_bytes: u64,
    pub quota_bytes: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct User {
    pub id: Uuid,
    pub username: String,
    pub display_name: String,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub is_admin: bool,
    #[serde(default)]
    pub quota_bytes: Option<u64>,
}
