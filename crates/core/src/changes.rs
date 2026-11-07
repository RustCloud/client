//! The change feed: what changed in the user's files since a cursor
//! (`docs/api/files.md`, «Лента изменений»), and hints over server-sent events
//! that there is something new.

use std::pin::Pin;

use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use serde::Deserialize;
use uuid::Uuid;

use crate::{Client, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChangeKind {
    Created,
    /// Content or mtime.
    Changed,
    /// Moved or renamed.
    Moved,
    /// Deleted, moved to the trash or into a hidden folder.
    Deleted,
}

/// One entry: a file or folder and what happened to it. What it is now, the
/// client asks with [`Client::details`]; `FILE_NOT_FOUND` means it is gone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct Change {
    pub id: Uuid,
    pub kind: ChangeKind,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChangePage {
    /// Oldest first; one node may come more than once.
    pub changes: Vec<Change>,
    /// Where the next request starts.
    pub cursor: String,
    /// More entries wait: ask again right away.
    pub more: bool,
}

impl Client {
    /// Changes after `cursor`; without one, an empty page with the cursor of
    /// "now" to start from. An old cursor is `410 CURSOR_EXPIRED`: walk the
    /// whole tree again.
    pub async fn changes(&self, cursor: Option<&str>) -> Result<ChangePage> {
        let mut url = self.url("api/v1/files/changes")?;
        if let Some(cursor) = cursor {
            url.query_pairs_mut().append_pair("cursor", cursor);
        }
        let response = self.send(|token| std::future::ready(Ok(self.http.get(url.clone()).bearer_auth(token)))).await?;
        Ok(response.json().await?)
    }

    /// Opens the stream of hints. The server closes it after about ten
    /// minutes; open a new one then (it takes a fresh access token).
    pub async fn watch_changes(&self) -> Result<ChangeWatch> {
        let url = self.url("api/v1/files/changes/watch")?;
        let response = self
            .send(|token| {
                std::future::ready(Ok(self
                    .http
                    .get(url.clone())
                    .bearer_auth(token)
                    .header(reqwest::header::ACCEPT, "text/event-stream")))
            })
            .await?;
        Ok(ChangeWatch { body: Box::pin(response.bytes_stream()), buf: Vec::new(), event: None })
    }
}

/// Hints that the feed has something new.
pub struct ChangeWatch {
    body: Pin<Box<dyn Stream<Item = reqwest::Result<Bytes>> + Send>>,
    buf: Vec<u8>,
    event: Option<String>,
}

impl ChangeWatch {
    /// Waits for the next hint: `Ok(true)` — read the feed; `Ok(false)` — the
    /// server closed the stream, open a new one.
    pub async fn next(&mut self) -> Result<bool> {
        loop {
            while let Some(end) = self.buf.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = self.buf.drain(..=end).collect();
                let line = String::from_utf8_lossy(&line);
                let line = line.trim_end_matches(['\n', '\r']);
                if line.is_empty() {
                    // A blank line ends an event.
                    if self.event.take().as_deref() == Some("changes") {
                        return Ok(true);
                    }
                } else if let Some(name) = line.strip_prefix("event:") {
                    self.event = Some(name.trim().to_owned());
                }
                // `data:`, `id:`, `retry:` and `:` comments (keep-alives) don't matter here.
            }
            match self.body.next().await {
                Some(chunk) => self.buf.extend_from_slice(&chunk?),
                None => return Ok(false),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn watch(chunks: &[&'static str]) -> ChangeWatch {
        let items: Vec<reqwest::Result<Bytes>> = chunks.iter().map(|c| Ok(Bytes::from_static(c.as_bytes()))).collect();
        ChangeWatch { body: Box::pin(futures_util::stream::iter(items)), buf: Vec::new(), event: None }
    }

    #[tokio::test]
    async fn hints_are_read_across_chunks() {
        let mut hints =
            watch(&[":keep-alive\n\nevent: chan", "ges\ndata: new\n\n", "event: other\n\nevent:changes\r\n\r\n"]);
        assert!(hints.next().await.unwrap());
        assert!(hints.next().await.unwrap(), "\\r\\n line ends work too");
        assert!(!hints.next().await.unwrap(), "the stream ended");
    }
}
