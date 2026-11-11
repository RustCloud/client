//! The Tauri shell: commands the Svelte UI calls, over `rc-core`.

mod session;
mod sync;
#[cfg(desktop)]
mod tray;

use std::path::PathBuf;
use std::time::Duration;

use rc_core::types::Node;
use rc_core::Client;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_opener::OpenerExt;
use tokio::sync::Mutex;
use uuid::Uuid;

use session::Store;

struct AppState {
    client: Mutex<Option<Client>>,
    store: Store,
    pairs: sync::Pairs,
}

/// An error as the UI gets it.
#[derive(Debug, Serialize)]
struct CmdError {
    code: Option<String>,
    message: String,
    /// The session is gone: show the login screen.
    needs_login: bool,
}

impl From<rc_core::Error> for CmdError {
    fn from(e: rc_core::Error) -> Self {
        Self { code: e.code().map(str::to_owned), message: e.to_string(), needs_login: e.needs_login() }
    }
}

impl CmdError {
    fn new(message: impl Into<String>) -> Self {
        Self { code: None, message: message.into(), needs_login: false }
    }
}

type CmdResult<T> = Result<T, CmdError>;

#[derive(Serialize)]
struct Account {
    server: String,
    username: String,
    display_name: String,
}

async fn account(client: &Client) -> CmdResult<Account> {
    let me = client.me().await?;
    Ok(Account { server: client.server().to_string(), username: me.username, display_name: me.display_name })
}

/// The saved session, if there is one and the server still knows it.
#[tauri::command]
async fn session(app: AppHandle, state: State<'_, AppState>) -> CmdResult<Option<Account>> {
    restore(&app, &state).await
}

/// Picks up the saved session and starts its pairs: at startup, so syncing
/// does not wait for the window, and again when the window asks.
async fn restore(app: &AppHandle, state: &AppState) -> CmdResult<Option<Account>> {
    let mut slot = state.client.lock().await;
    if slot.is_none() {
        *slot = state.store.load().map(|saved| state.store.client(saved.server, saved.tokens));
    }
    let Some(client) = slot.as_ref() else { return Ok(None) };
    match account(client).await {
        Ok(account) => {
            state.pairs.start(app, client);
            status_changed(app);
            Ok(Some(account))
        }
        Err(e) if e.needs_login => {
            *slot = None;
            state.pairs.stop();
            state.store.forget();
            status_changed(app);
            Ok(None)
        }
        Err(e) => Err(e),
    }
}

/// The pairs' state changed: the tray shows it.
pub(crate) fn status_changed(#[allow(unused)] app: &AppHandle) {
    #[cfg(desktop)]
    tray::refresh(app);
}

/// Pauses or resumes all syncing; the window and the tray hear of it.
async fn set_paused(app: &AppHandle, paused: bool) {
    let state = app.state::<AppState>();
    if paused {
        state.pairs.pause();
    } else {
        let client = state.client.lock().await.clone();
        state.pairs.resume(app, client.as_ref());
    }
    let _ = app.emit("sync-paused", paused);
    status_changed(app);
}

/// Logs in through the system browser.
#[tauri::command]
async fn login(app: AppHandle, state: State<'_, AppState>, server: String) -> CmdResult<Account> {
    let server = session::server_url(&server).map_err(CmdError::new)?;
    let open = |url: &url::Url| {
        app.opener()
            .open_url(url.as_str(), None::<&str>)
            .map_err(|e| rc_core::Error::Io(std::io::Error::other(e.to_string())))
    };
    let (http, device) = (Client::http(), session::device_name());
    let login = rc_core::auth::login_with_browser(&http, &server, "RustCloud", &device, open);
    let tokens = tokio::time::timeout(Duration::from_secs(600), login)
        .await
        .map_err(|_| CmdError::new("Вход не подтверждён за 10 минут"))??;

    state.store.save(&server, &tokens).map_err(|e| CmdError::new(format!("Не удалось сохранить вход: {e}")))?;
    let client = state.store.client(server, tokens);
    let account = account(&client).await?;
    state.pairs.stop();
    state.pairs.start(&app, &client);
    *state.client.lock().await = Some(client);
    status_changed(&app);
    Ok(account)
}

#[tauri::command]
async fn logout(app: AppHandle, state: State<'_, AppState>) -> CmdResult<()> {
    state.pairs.stop();
    status_changed(&app);
    if let Some(client) = state.client.lock().await.take() {
        // The session ends on the server too; offline, it just runs out.
        if let Err(e) = client.logout().await {
            tracing::warn!("logout on the server failed: {e}");
        }
    }
    state.store.forget();
    Ok(())
}

#[derive(Serialize)]
struct Listing {
    folder: Node,
    /// Folders above, from the root.
    path: Vec<Node>,
    children: Vec<Node>,
}

async fn current(state: &AppState) -> CmdResult<Client> {
    state
        .client
        .lock()
        .await
        .clone()
        .ok_or_else(|| CmdError { needs_login: true, ..CmdError::new("Не выполнен вход") })
}

/// A folder's contents; the root without `folder`.
#[tauri::command]
async fn list(state: State<'_, AppState>, folder: Option<Uuid>) -> CmdResult<Listing> {
    let client = current(&state).await?;
    let (folder, path) = match folder {
        Some(id) => {
            let details = client.details(id).await?;
            (details.node, details.path)
        }
        None => (client.root().await?, Vec::new()),
    };
    let children = client.children(folder.id).await?;
    Ok(Listing { folder, path, children })
}

/// A new folder on the server.
#[tauri::command]
async fn create_folder(state: State<'_, AppState>, parent: Uuid, name: String) -> CmdResult<Node> {
    let client = current(&state).await?;
    let name = name.trim();
    if name.is_empty() || name.contains(['/', '\\']) || name == "." || name == ".." {
        return Err(CmdError::new("Недопустимое имя папки"));
    }
    Ok(client.create_folder(parent, name).await?)
}

/// Subfolders of a local folder, for the picker.
#[tauri::command]
fn local_dirs(path: PathBuf) -> CmdResult<Vec<sync::LocalDir>> {
    sync::local_dirs(&path).map_err(|e| CmdError::new(sync::local_error(&path, &e)))
}

/// Where the local picker starts.
#[tauri::command]
fn local_places() -> Vec<sync::LocalDir> {
    sync::local_places()
}

/// A new local folder.
#[tauri::command]
fn create_local_dir(parent: PathBuf, name: String) -> CmdResult<sync::LocalDir> {
    sync::create_local_dir(&parent, &name).map_err(CmdError::new)
}

/// The synced folders of the logged-in server, with their status.
#[tauri::command]
async fn sync_pairs(state: State<'_, AppState>) -> CmdResult<Vec<sync::PairView>> {
    let client = current(&state).await?;
    Ok(state.pairs.list(client.server().as_str()))
}

/// Starts syncing the local folder `local` with the server's folder `remote`.
#[tauri::command]
async fn add_sync_pair(
    app: AppHandle,
    state: State<'_, AppState>,
    local: PathBuf,
    remote: Uuid,
) -> CmdResult<sync::PairView> {
    let client = current(&state).await?;
    state.pairs.add(&app, &client, local, remote).await.map_err(CmdError::new)
}

/// Stops syncing; the files stay on both sides.
#[tauri::command]
async fn remove_sync_pair(app: AppHandle, state: State<'_, AppState>, id: Uuid) -> CmdResult<()> {
    state.pairs.remove(id).map_err(CmdError::new)?;
    status_changed(&app);
    Ok(())
}

#[tauri::command]
async fn sync_now(state: State<'_, AppState>, id: Uuid) -> CmdResult<()> {
    state.pairs.sync_now(id);
    Ok(())
}

#[tauri::command]
fn sync_paused(state: State<'_, AppState>) -> bool {
    state.pairs.is_paused()
}

#[tauri::command]
async fn set_sync_paused(app: AppHandle, paused: bool) -> CmdResult<()> {
    set_paused(&app, paused).await;
    Ok(())
}

/// The user agreed to the many deletions the pair waits for.
#[tauri::command]
async fn confirm_deletions(state: State<'_, AppState>, id: Uuid) -> CmdResult<()> {
    state.pairs.confirm_deletions(id);
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default();
    // First: a second launch only brings this copy's window up.
    #[cfg(desktop)]
    let builder = builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| tray::show_window(app))).plugin(
        tauri_plugin_autostart::init(tauri_plugin_autostart::MacosLauncher::LaunchAgent, Some(vec![tray::HIDDEN_ARG])),
    );
    builder
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            let pairs = sync::Pairs::load(&dir);
            app.manage(AppState { client: Mutex::new(None), store: Store::new(dir), pairs });
            let handle = app.handle().clone();
            // Offline at login (no network yet), it tries again every minute.
            tauri::async_runtime::spawn(async move {
                while let Err(e) = restore(&handle, &handle.state::<AppState>()).await {
                    tracing::warn!("sync not started, retrying in a minute: {}", e.message);
                    tokio::time::sleep(Duration::from_secs(60)).await;
                }
            });
            // The window starts hidden on desktops (tauri.linux.conf.json,
            // tauri.windows.conf.json): shown unless started at login with a tray.
            #[cfg(desktop)]
            {
                let tray = tray::create(app.handle());
                let hidden = tray.is_some() && std::env::args().any(|a| a == tray::HIDDEN_ARG);
                if let Some(tray) = tray {
                    app.manage(tray);
                    tray::refresh(app.handle());
                }
                if !hidden {
                    tray::show_window(app.handle());
                }
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            // With a tray, closing the window only hides it: syncing goes on.
            #[cfg(desktop)]
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.app_handle().try_state::<tray::Tray>().is_some() {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
            #[cfg(mobile)]
            let _ = (window, event);
        })
        .invoke_handler(tauri::generate_handler![
            session,
            login,
            logout,
            list,
            create_folder,
            local_dirs,
            local_places,
            create_local_dir,
            sync_pairs,
            add_sync_pair,
            remove_sync_pair,
            sync_now,
            sync_paused,
            set_sync_paused,
            confirm_deletions
        ])
        .run(tauri::generate_context!())
        .expect("error while running the RustCloud client");
}
