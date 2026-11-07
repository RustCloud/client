//! Running sync in the background: a loop per pair that syncs when told to
//! ([`Runner::wake`]), when the local folder changes ([`watch_local`]), when
//! the server hints at news ([`watch_server`]) and every few minutes anyway.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use notify::{RecursiveMode, Watcher};
use rc_core::Client;
use tokio::sync::Notify;
use tokio::task::JoinHandle;

use crate::local::TEMP_PREFIX;
use crate::{Engine, Error, Remote, Report};

/// Without any hint, a run every this long.
pub const EVERY: Duration = Duration::from_secs(5 * 60);
/// Changes closer together than this make one run.
const SETTLE: Duration = Duration::from_millis(1500);

/// Where a pair is, for the UI.
#[derive(Debug, Clone)]
pub enum Status {
    Syncing,
    Done { at: DateTime<Utc>, report: Report },
    Failed { at: DateTime<Utc>, error: Arc<Error> },
}

/// The loop of one pair; stops when dropped.
pub struct Runner {
    wake: Arc<Notify>,
    task: JoinHandle<()>,
}

impl Drop for Runner {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Runner {
    pub fn spawn<R: Remote + 'static>(
        engine: Arc<Engine<R>>,
        on_status: impl Fn(Status) + Send + Sync + 'static,
    ) -> Self {
        let wake = Arc::new(Notify::new());
        let task = tokio::spawn({
            let wake = wake.clone();
            async move {
                loop {
                    on_status(Status::Syncing);
                    let status = match engine.sync().await {
                        Ok(report) => Status::Done { at: Utc::now(), report },
                        Err(error) => Status::Failed { at: Utc::now(), error: Arc::new(error) },
                    };
                    let failed = matches!(status, Status::Failed { .. });
                    on_status(status);
                    // After a failure, not sooner than in a minute (unless asked).
                    let wait = if failed { Duration::from_secs(60) } else { EVERY };
                    let _ = tokio::time::timeout(wait, wake.notified()).await;
                    tokio::time::sleep(SETTLE).await;
                }
            }
        });
        Self { wake, task }
    }

    /// Sync soon.
    pub fn wake(&self) {
        self.wake.notify_one();
    }

    /// What wakes this runner, for [`watch_local`] and [`watch_server`].
    pub fn waker(&self) -> Arc<Notify> {
        self.wake.clone()
    }
}

/// Wakes `wake` when something changes below `root` (our own temporary files
/// aside). Keep the watcher: dropping it stops watching.
pub fn watch_local(root: &Path, wake: Arc<Notify>) -> notify::Result<notify::RecommendedWatcher> {
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        let Ok(event) = event else { return };
        let ours = event
            .paths
            .iter()
            .all(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with(TEMP_PREFIX)));
        if !ours && !matches!(event.kind, notify::EventKind::Access(_)) {
            wake.notify_one();
        }
    })?;
    watcher.watch(root, RecursiveMode::Recursive)?;
    Ok(watcher)
}

/// Listens to the server's hints and wakes every runner in `wakes()` on each;
/// reconnects as long as the task lives. One per account is enough.
pub fn watch_server(client: Client, wakes: impl Fn() -> Vec<Arc<Notify>> + Send + 'static) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut backoff = Duration::from_secs(1);
        loop {
            match client.watch_changes().await {
                Ok(mut hints) => {
                    backoff = Duration::from_secs(1);
                    loop {
                        match hints.next().await {
                            Ok(true) => wakes().iter().for_each(|w| w.notify_one()),
                            Ok(false) => break, // the server closed it: open a new one
                            Err(e) => {
                                tracing::debug!("hints stopped: {e}");
                                break;
                            }
                        }
                    }
                }
                Err(e) if e.needs_login() => {
                    tracing::warn!("server hints: logged out");
                    return;
                }
                Err(e) => {
                    tracing::debug!("server hints unavailable: {e}");
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(Duration::from_secs(300));
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::fake::FakeServer;

    #[tokio::test]
    async fn a_local_change_wakes_the_runner() {
        let server = Arc::new(FakeServer::new());
        server.mkdir("Sync");
        let dir = tempfile::tempdir().unwrap();
        let engine = Arc::new(Engine::in_memory(server.clone(), dir.path().to_path_buf(), server.id("Sync")));
        let runs = Arc::new(Mutex::new(0));
        let runner = Runner::spawn(engine, {
            let runs = runs.clone();
            move |status| {
                if let Status::Done { .. } = status {
                    *runs.lock().unwrap() += 1;
                }
            }
        });
        let _watcher = watch_local(dir.path(), runner.waker()).unwrap();

        // The first run starts at once.
        wait_for(|| *runs.lock().unwrap() >= 1).await;
        std::fs::write(dir.path().join("new.txt"), "n").unwrap();
        wait_for(|| server.read("Sync/new.txt").is_some()).await;
    }

    async fn wait_for(done: impl Fn() -> bool) {
        for _ in 0..100 {
            if done() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("timed out");
    }
}
