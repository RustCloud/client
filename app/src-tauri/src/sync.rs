//! Synced folders of the app: which pairs there are (`sync/pairs.json` in the
//! app's data folder, the state of each in `sync/<id>.sqlite`), their background
//! runners and the status the UI shows.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use rc_core::Client;
use rc_sync::runner::{self, Runner, Status};
use rc_sync::{Engine, Error};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use tokio::task::JoinHandle;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairConfig {
    pub id: Uuid,
    /// The server the pair belongs to (`Client::server`).
    pub server: String,
    pub local: PathBuf,
    pub remote: Uuid,
    /// Where the folder was on the server when the pair was made, for showing.
    pub remote_path: String,
}

/// What the UI shows for a pair.
#[derive(Debug, Clone, Default, Serialize)]
pub struct PairStatus {
    /// `idle`, `syncing`, `error` or `paused`.
    pub state: &'static str,
    /// RFC 3339.
    pub last_sync: Option<String>,
    pub uploaded: usize,
    pub downloaded: usize,
    pub deleted: usize,
    pub conflicts: Vec<String>,
    pub failed: Vec<(String, String)>,
    pub error: Option<String>,
    /// This many of that many would be deleted: waits for a confirmation.
    pub confirm_deletions: Option<(usize, usize)>,
    pub needs_login: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct PairView {
    #[serde(flatten)]
    pub config: PairConfig,
    pub status: PairStatus,
}

struct Running {
    engine: Arc<Engine<Client>>,
    runner: Runner,
    _watcher: Option<notify::RecommendedWatcher>,
    status: Arc<Mutex<PairStatus>>,
}

pub struct Pairs {
    dir: PathBuf,
    configs: Mutex<Vec<PairConfig>>,
    running: Arc<Mutex<HashMap<Uuid, Running>>>,
    hints: Mutex<Option<JoinHandle<()>>>,
    /// Paused by the user: nothing runs until resumed (or the app restarts).
    paused: AtomicBool,
}

/// All the pairs at once, for the tray.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Overall {
    pub paused: bool,
    pub pairs: usize,
    pub syncing: usize,
    pub errors: usize,
}

impl Pairs {
    pub fn load(app_data: &Path) -> Self {
        let dir = app_data.join("sync");
        let configs = std::fs::read_to_string(dir.join("pairs.json"))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        Self {
            dir,
            configs: Mutex::new(configs),
            running: Arc::default(),
            hints: Mutex::new(None),
            paused: AtomicBool::new(false),
        }
    }

    fn save(&self, configs: &[PairConfig]) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let tmp = self.dir.join("pairs.json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(configs)?)?;
        std::fs::rename(tmp, self.dir.join("pairs.json"))
    }

    /// Starts the pairs of the logged-in server, and one listener for its hints.
    pub fn start(&self, app: &AppHandle, client: &Client) {
        if self.is_paused() {
            return;
        }
        let server = client.server().to_string();
        let configs: Vec<PairConfig> =
            self.configs.lock().unwrap().iter().filter(|c| c.server == server).cloned().collect();
        for config in configs {
            if !self.running.lock().unwrap().contains_key(&config.id) {
                self.start_one(app, client, &config);
            }
        }
        let mut hints = self.hints.lock().unwrap();
        if hints.is_none() {
            let running = self.running.clone();
            *hints = Some(runner::watch_server(client.clone(), move || {
                running.lock().unwrap().values().map(|r| r.runner.waker()).collect()
            }));
        }
    }

    fn start_one(&self, app: &AppHandle, client: &Client, config: &PairConfig) {
        let id = config.id;
        let status = Arc::new(Mutex::new(PairStatus { state: "idle", ..Default::default() }));
        let state_file = self.dir.join(format!("{id}.sqlite"));
        let engine = match std::fs::create_dir_all(&self.dir)
            .map_err(Error::from)
            .and_then(|_| Engine::open(client.clone(), &state_file, config.local.clone(), config.remote))
        {
            Ok(engine) => Arc::new(engine),
            Err(e) => {
                tracing::error!("sync pair {id}: {e}");
                return;
            }
        };
        let runner = Runner::spawn(engine.clone(), {
            let (app, status) = (app.clone(), status.clone());
            move |update| {
                let event = {
                    let mut s = status.lock().unwrap();
                    apply(&mut s, update);
                    serde_json::json!({ "id": id, "status": &*s })
                };
                let _ = app.emit("sync-status", event);
                // With the lock let go: the summary takes every pair's.
                crate::status_changed(&app);
            }
        });
        // Without a watcher (an odd file system), the timer and the hints still work.
        let watcher = runner::watch_local(&config.local, runner.waker())
            .map_err(|e| tracing::warn!("not watching {}: {e}", config.local.display()))
            .ok();
        self.running.lock().unwrap().insert(id, Running { engine, runner, _watcher: watcher, status });
    }

    /// Stops everything (at logout).
    pub fn stop(&self) {
        self.running.lock().unwrap().clear();
        if let Some(hints) = self.hints.lock().unwrap().take() {
            hints.abort();
        }
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }

    /// Stops every pair until [`Pairs::resume`]; a sync under way is cut short
    /// and redone later, as after a crash.
    pub fn pause(&self) {
        self.paused.store(true, Ordering::Relaxed);
        self.stop();
    }

    pub fn resume(&self, app: &AppHandle, client: Option<&Client>) {
        self.paused.store(false, Ordering::Relaxed);
        if let Some(client) = client {
            self.start(app, client);
        }
    }

    pub fn overall(&self) -> Overall {
        let running = self.running.lock().unwrap();
        let states: Vec<&str> = running.values().map(|r| r.status.lock().unwrap().state).collect();
        Overall {
            paused: self.is_paused(),
            pairs: states.len(),
            syncing: states.iter().filter(|s| **s == "syncing").count(),
            errors: states.iter().filter(|s| **s == "error").count(),
        }
    }

    pub fn list(&self, server: &str) -> Vec<PairView> {
        let paused = self.is_paused().then(|| PairStatus { state: "paused", ..Default::default() });
        let running = self.running.lock().unwrap();
        self.configs
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.server == server)
            .map(|c| PairView {
                config: c.clone(),
                status: paused.clone().unwrap_or_else(|| {
                    running.get(&c.id).map(|r| r.status.lock().unwrap().clone()).unwrap_or_default()
                }),
            })
            .collect()
    }

    pub async fn add(
        &self,
        app: &AppHandle,
        client: &Client,
        local: PathBuf,
        remote: Uuid,
    ) -> Result<PairView, String> {
        if !local.is_absolute() {
            return Err("Укажите полный путь к локальной папке".into());
        }
        // A new folder is made here; an existing one has to be a folder.
        if !local.exists() {
            std::fs::create_dir_all(&local).map_err(|e| format!("Не удалось создать {}: {e}", local.display()))?;
        }
        let local = dunce::canonicalize(&local).map_err(|e| format!("Папка {}: {e}", local.display()))?;
        if !local.is_dir() {
            return Err(format!("{} — не папка", local.display()));
        }
        let details = client.details(remote).await.map_err(|e| e.to_string())?;
        if !details.node.is_dir || details.node.trashed_at.is_some() {
            return Err("На сервере это не папка или она в корзине".into());
        }
        if details.path.iter().chain([&details.node]).any(|n| rc_sync::local::is_hidden_dir(&n.name)) {
            return Err("Скрытые папки (с точкой в начале имени) не синхронизируются".into());
        }
        let remote_path = remote_path(&details);
        let server = client.server().to_string();
        let others: Vec<PairConfig> =
            self.configs.lock().unwrap().iter().filter(|c| c.server == server).cloned().collect();
        for other in &others {
            if local.starts_with(&other.local) || other.local.starts_with(&local) {
                return Err(format!("Папка пересекается с уже синхронизируемой {}", other.local.display()));
            }
            if other.remote == remote || details.path.iter().any(|a| a.id == other.remote) {
                return Err(format!("{remote_path} уже синхронизируется (в паре с {})", other.local.display()));
            }
            // The other one inside this one.
            if let Ok(d) = client.details(other.remote).await {
                if d.path.iter().any(|a| a.id == remote) {
                    return Err(format!(
                        "Внутри {remote_path} уже синхронизируется {} (в паре с {})",
                        other.remote_path,
                        other.local.display()
                    ));
                }
            }
        }
        let config = {
            let mut configs = self.configs.lock().unwrap();
            let config = PairConfig { id: Uuid::new_v4(), server, local, remote, remote_path };
            configs.push(config.clone());
            self.save(&configs).map_err(|e| format!("Не удалось сохранить: {e}"))?;
            config
        };
        self.start(app, client);
        let state = if self.is_paused() { "paused" } else { "syncing" };
        Ok(PairView { config, status: PairStatus { state, ..Default::default() } })
    }

    /// Stops syncing a pair; the files stay on both sides.
    pub fn remove(&self, id: Uuid) -> Result<(), String> {
        self.running.lock().unwrap().remove(&id);
        let mut configs = self.configs.lock().unwrap();
        configs.retain(|c| c.id != id);
        self.save(&configs).map_err(|e| e.to_string())?;
        for ext in ["sqlite", "sqlite-wal", "sqlite-shm"] {
            let _ = std::fs::remove_file(self.dir.join(format!("{id}.{ext}")));
        }
        Ok(())
    }

    pub fn sync_all(&self) {
        self.running.lock().unwrap().values().for_each(|r| r.runner.wake());
    }

    pub fn sync_now(&self, id: Uuid) {
        if let Some(r) = self.running.lock().unwrap().get(&id) {
            r.runner.wake();
        }
    }

    /// The user agreed to the deletions the pair waits for.
    pub fn confirm_deletions(&self, id: Uuid) {
        if let Some(r) = self.running.lock().unwrap().get(&id) {
            r.engine.allow_mass_delete_once();
            r.runner.wake();
        }
    }
}

/// `/a/b` for a folder on the server (`/` for the root).
pub fn remote_path(details: &rc_core::types::NodeDetails) -> String {
    let names: Vec<&str> =
        details.path.iter().chain([&details.node]).filter(|n| n.parent_id.is_some()).map(|n| n.name.as_str()).collect();
    format!("/{}", names.join("/"))
}

/// A local folder in the picker.
#[derive(Debug, Serialize)]
pub struct LocalDir {
    pub name: String,
    pub path: PathBuf,
}

/// A local folder's subfolders, hidden ones and links left out, by name.
pub fn local_dirs(path: &Path) -> std::io::Result<Vec<LocalDir>> {
    let mut dirs: Vec<LocalDir> = std::fs::read_dir(path)?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            (!name.starts_with('.')).then(|| LocalDir { path: e.path(), name })
        })
        .collect();
    dirs.sort_by_key(|d| d.name.to_lowercase());
    Ok(dirs)
}

/// Where the local picker starts: the home folder, then the file system roots
/// (`/`; drives on Windows; the shared storage on Android).
pub fn local_places() -> Vec<LocalDir> {
    let mut places = Vec::new();
    if cfg!(target_os = "android") {
        // Needs "access to all files" (MANAGE_EXTERNAL_STORAGE), given in the settings.
        places.push(LocalDir {
            name: "Память устройства".into(), path: PathBuf::from("/storage/emulated/0")
        });
        if let Ok(cards) = std::fs::read_dir("/storage") {
            for card in cards.filter_map(|e| e.ok()) {
                let name = card.file_name().to_string_lossy().into_owned();
                if !["emulated", "self"].contains(&name.as_str()) && card.path().is_dir() {
                    places.push(LocalDir { name: format!("Карта памяти {name}"), path: card.path() });
                }
            }
        }
        return places;
    }
    if let Some(home) = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }) {
        places.push(LocalDir { name: "Домашняя папка".into(), path: PathBuf::from(home) });
    }
    if cfg!(windows) {
        for letter in b'A'..=b'Z' {
            let drive = PathBuf::from(format!("{}:\\", letter as char));
            if drive.is_dir() {
                places.push(LocalDir { name: format!("Диск {}:", letter as char), path: drive });
            }
        }
    } else {
        places.push(LocalDir { name: "/".into(), path: PathBuf::from("/") });
    }
    places
}

/// Why a local folder cannot be read, for the user.
pub fn local_error(path: &Path, e: &std::io::Error) -> String {
    if cfg!(target_os = "android") && e.kind() == std::io::ErrorKind::PermissionDenied {
        return "Нет доступа к файлам. Разрешите его: Настройки → Приложения → RustCloud → Разрешения → \
                Файлы и медиа → «Разрешить управление всеми файлами»"
            .into();
    }
    format!("{}: {e}", path.display())
}

/// A new folder in a local folder.
pub fn create_local_dir(parent: &Path, name: &str) -> Result<LocalDir, String> {
    let name = name.trim();
    if name.is_empty() || name.contains(['/', '\\']) || name == "." || name == ".." {
        return Err("Недопустимое имя папки".into());
    }
    let path = parent.join(name);
    std::fs::create_dir(&path).map_err(|e| match e.kind() {
        std::io::ErrorKind::AlreadyExists => format!("«{name}» здесь уже есть"),
        _ => format!("Не удалось создать папку. {}", local_error(&path, &e)),
    })?;
    Ok(LocalDir { name: name.into(), path })
}

fn apply(s: &mut PairStatus, update: Status) {
    match update {
        Status::Syncing => s.state = "syncing",
        Status::Done { at, report } => {
            *s = PairStatus {
                state: "idle",
                last_sync: Some(at.to_rfc3339()),
                uploaded: report.uploaded,
                downloaded: report.downloaded,
                deleted: report.deleted_local + report.deleted_remote,
                conflicts: report.conflicts,
                failed: report.failed,
                ..Default::default()
            };
        }
        Status::Failed { error, .. } => {
            s.state = "error";
            s.needs_login = error.needs_login();
            s.confirm_deletions = match &*error {
                Error::TooManyDeletions { count, of } => Some((*count, *of)),
                _ => None,
            };
            s.error = Some(match &*error {
                Error::LocalRootMissing(path) => format!("Нет локальной папки {} — ничего не изменено", path.display()),
                Error::RemoteRootGone => "Папка на сервере удалена или в корзине — ничего не изменено".into(),
                Error::TooManyDeletions { count, of } => {
                    format!("Будет удалено {count} из {of} файлов и папок — нужно подтверждение")
                }
                other => other.to_string(),
            });
        }
    }
}
