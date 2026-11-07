use std::path::PathBuf;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Remote(#[from] rc_core::Error),
    #[error("sync state: {0}")]
    State(#[from] rusqlite::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// Nothing was changed: an unmounted drive must not look like "everything deleted".
    #[error("the local folder {0} is missing; nothing was changed")]
    LocalRootMissing(PathBuf),
    /// Nothing was changed: the synced folder on the server is deleted or in the trash.
    #[error("the folder on the server is gone or in the trash; nothing was changed")]
    RemoteRootGone,
    /// Nothing was changed: this run would delete `count` of `of` synced items.
    #[error("{count} of {of} synced files and folders would be deleted; confirm to go on")]
    TooManyDeletions { count: usize, of: usize },
}

impl Error {
    /// Whether the session is gone and only a new login helps.
    pub fn needs_login(&self) -> bool {
        matches!(self, Self::Remote(e) if e.needs_login())
    }
}
