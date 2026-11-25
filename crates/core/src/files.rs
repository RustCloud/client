//! Files: `docs/api/files.md` in the RustCloud repository.

use std::path::Path;
use std::time::SystemTime;

use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use reqwest::header::{CONTENT_LENGTH, ETAG, IF_MATCH, RANGE};
use reqwest::{Body, Method, StatusCode};
use serde::Serialize;
use serde_json::json;
use tokio::fs::{File, OpenOptions};
use tokio::io::AsyncWriteExt;
use tokio_util::io::ReaderStream;
use uuid::Uuid;

use crate::types::{Node, NodeDetails};
use crate::{Client, Error, Result};

/// Changes for [`Client::update`]; `None` leaves a field as it is.
#[derive(Debug, Default, Clone, Serialize)]
pub struct NodeUpdate {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified_at: Option<DateTime<Utc>>,
}

/// What [`Client::download`] wrote.
#[derive(Debug, Clone)]
pub struct Download {
    /// The ETag (SHA-256) of the content as the server sent it.
    pub etag: Option<String>,
    /// The size of the local file now.
    pub size: u64,
    /// Whether it continued a partial file rather than writing it anew.
    pub resumed: bool,
}

impl Client {
    pub async fn root(&self) -> Result<Node> {
        self.get_json("api/v1/files/root").await
    }

    pub async fn details(&self, id: Uuid) -> Result<NodeDetails> {
        self.get_json(&format!("api/v1/files/{id}")).await
    }

    /// Folders first, then files, by name.
    pub async fn children(&self, folder: Uuid) -> Result<Vec<Node>> {
        self.get_json(&format!("api/v1/files/{folder}/children")).await
    }

    /// Names containing `query`, any case; not the trash.
    pub async fn search(&self, query: &str) -> Result<Vec<Node>> {
        let mut url = self.url("api/v1/files/search")?;
        url.query_pairs_mut().append_pair("q", query);
        let response = self.send(|token| std::future::ready(Ok(self.http.get(url.clone()).bearer_auth(token)))).await?;
        Ok(response.json().await?)
    }

    pub async fn create_folder(&self, parent: Uuid, name: &str) -> Result<Node> {
        let response = self
            .request(Method::POST, &format!("api/v1/files/{parent}/folders"), Some(&json!({ "name": name })))
            .await?;
        Ok(response.json().await?)
    }

    /// Renames, moves, or sets the mtime.
    pub async fn update(&self, id: Uuid, change: &NodeUpdate) -> Result<Node> {
        let body = serde_json::to_value(change).expect("plain data");
        Ok(self.request(Method::PATCH, &format!("api/v1/files/{id}"), Some(&body)).await?.json().await?)
    }

    pub async fn copy(&self, id: Uuid, parent: Uuid, name: Option<&str>) -> Result<Node> {
        let body = json!({ "parent_id": parent, "name": name });
        Ok(self.request(Method::POST, &format!("api/v1/files/{id}/copy"), Some(&body)).await?.json().await?)
    }

    /// To the trash; from the trash — for good.
    pub async fn delete(&self, id: Uuid) -> Result<()> {
        self.request(Method::DELETE, &format!("api/v1/files/{id}"), None).await?;
        Ok(())
    }

    /// Uploads the local file `path` as `name` into `folder`, streaming it, with
    /// the local mtime and creation time. Without `overwrite`, a taken name is
    /// `409 NAME_TAKEN`.
    ///
    /// The creation time becomes the file's `born_at` (servers since API 0.2.21;
    /// older ones skip it), only for a new file: what a gallery sorts photos by.
    pub async fn upload(&self, folder: Uuid, name: &str, path: &Path, overwrite: bool) -> Result<Node> {
        let mut url = self.url(&format!("api/v1/files/{folder}/upload"))?;
        url.query_pairs_mut().append_pair("name", name);
        if overwrite {
            url.query_pairs_mut().append_pair("overwrite", "true");
        }
        let meta = tokio::fs::metadata(path).await?;
        if let Some(ctime) = birth_time(&meta).and_then(time_param) {
            url.query_pairs_mut().append_pair("ctime", &ctime);
        }
        self.put_file(Method::POST, url, path, None).await
    }

    /// Replaces the content of the file `id` with the local file `path`. With
    /// `if_match` (the ETag the client last saw), only if the file on the server
    /// didn't change since: otherwise `409 FILE_CHANGED`, and nothing is written.
    pub async fn replace(&self, id: Uuid, path: &Path, if_match: Option<&str>) -> Result<Node> {
        let url = self.url(&format!("api/v1/files/{id}/content"))?;
        self.put_file(Method::PUT, url, path, if_match).await
    }

    async fn put_file(&self, method: Method, mut url: url::Url, path: &Path, if_match: Option<&str>) -> Result<Node> {
        let meta = tokio::fs::metadata(path).await?;
        if let Some(mtime) = meta.modified().ok().and_then(time_param) {
            url.query_pairs_mut().append_pair("mtime", &mtime);
        }
        let size = meta.len();
        let response = self
            .send(|token| {
                let (method, url) = (method.clone(), url.clone());
                async move {
                    // Opened anew for each attempt: a body stream can't be replayed.
                    let file = File::open(path).await?;
                    let mut request = self
                        .http
                        .request(method, url)
                        .bearer_auth(token)
                        .header(CONTENT_LENGTH, size)
                        .body(Body::wrap_stream(ReaderStream::new(file)));
                    if let Some(etag) = if_match {
                        request = request.header(IF_MATCH, format!("\"{etag}\""));
                    }
                    Ok(request)
                }
            })
            .await?;
        Ok(response.json().await?)
    }

    /// Downloads the file `id` into `dest`. With `resume`, an existing `dest` is
    /// taken for the beginning of the file and only the rest is asked for; if the
    /// server sends the whole file instead, it is written anew.
    ///
    /// The caller checks [`Download::etag`] against what it expected: the file
    /// may have changed between two parts.
    pub async fn download(&self, id: Uuid, dest: &Path, resume: bool) -> Result<Download> {
        let have = if resume { tokio::fs::metadata(dest).await.map(|m| m.len()).unwrap_or(0) } else { 0 };
        let url = self.url(&format!("api/v1/files/{id}/content"))?;
        let response = self
            .send(|token| {
                let mut request = self.http.get(url.clone()).bearer_auth(token);
                if have > 0 {
                    request = request.header(RANGE, format!("bytes={have}-"));
                }
                std::future::ready(Ok(request))
            })
            .await;
        // Asking for the bytes after the end: the local file is the whole of it already.
        if let Err(Error::Api { status: StatusCode::RANGE_NOT_SATISFIABLE, .. }) = &response {
            let details = self.details(id).await?;
            if details.node.size == have {
                return Ok(Download { etag: details.node.etag, size: have, resumed: true });
            }
            return Box::pin(self.download(id, dest, false)).await;
        }
        let response = response?;
        let resumed = response.status() == StatusCode::PARTIAL_CONTENT;
        let etag = response
            .headers()
            .get(ETAG)
            .and_then(|v| v.to_str().ok())
            .map(|v| v.trim_start_matches("W/").trim_matches('"').to_owned());

        let mut file =
            if resumed { OpenOptions::new().append(true).open(dest).await? } else { File::create(dest).await? };
        let mut size = if resumed { have } else { 0 };
        let mut body = response.bytes_stream();
        while let Some(chunk) = body.next().await {
            let chunk = chunk?;
            file.write_all(&chunk).await?;
            size += chunk.len() as u64;
        }
        file.flush().await?;
        file.sync_all().await?;
        Ok(Download { etag, size, resumed })
    }
}

/// When the file was created on this device. A copied file is younger than its
/// content (a photo copied from a camera today, taken in March): then the mtime,
/// which copies keep, is nearer the truth. Without a birth time (Linux before
/// statx, Android) — the mtime too.
fn birth_time(meta: &std::fs::Metadata) -> Option<SystemTime> {
    match (meta.created().ok(), meta.modified().ok()) {
        (Some(created), Some(modified)) => Some(created.min(modified)),
        (created, modified) => created.or(modified),
    }
}

/// Unix seconds with microseconds, as the server takes `mtime` and `ctime`.
/// `None` for what the server refuses with a 400 — before 1970 or more than a
/// day ahead (a clock gone wrong): better upload without the time than fail.
fn time_param(time: SystemTime) -> Option<String> {
    if time > SystemTime::now() + std::time::Duration::from_secs(12 * 3600) {
        return None;
    }
    let since = time.duration_since(SystemTime::UNIX_EPOCH).ok()?;
    Some(format!("{}.{:06}", since.as_secs(), since.subsec_micros()))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn times_keep_microseconds() {
        let time = SystemTime::UNIX_EPOCH + Duration::new(1_730_985_600, 123_456_789);
        assert_eq!(time_param(time).unwrap(), "1730985600.123456");
        assert_eq!(time_param(SystemTime::UNIX_EPOCH - Duration::from_secs(1)), None);
        assert_eq!(time_param(SystemTime::now() + Duration::from_secs(2 * 86400)), None, "a clock gone wrong");
    }
}
