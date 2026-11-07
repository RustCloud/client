//! The Tauri shell: commands the Svelte UI calls, over `rc-core`.

mod session;
mod sync;

use std::path::PathBuf;
use std::time::Duration;

use rc_core::types::Node;
use rc_core::Client;
use serde::Serialize;
use tauri::{AppHandle, Manager, State};
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
    let mut slot = state.client.lock().await;
    if slot.is_none() {
        *slot = state.store.load().map(|saved| state.store.client(saved.server, saved.tokens));
    }
    let Some(client) = slot.as_ref() else { return Ok(None) };
    match account(client).await {
        Ok(account) => {
            state.pairs.start(&app, client);
            Ok(Some(account))
        }
        Err(e) if e.needs_login => {
            *slot = None;
            state.pairs.stop();
            state.store.forget();
            Ok(None)
        }
        Err(e) => Err(e),
    }
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
    Ok(account)
}

#[tauri::command]
async fn logout(state: State<'_, AppState>) -> CmdResult<()> {
    state.pairs.stop();
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

/// What is at a local path: for the form that makes a synced pair.
#[tauri::command]
fn local_folder(path: PathBuf) -> sync::LocalFolder {
    sync::local_folder(&path)
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
async fn remove_sync_pair(state: State<'_, AppState>, id: Uuid) -> CmdResult<()> {
    state.pairs.remove(id).map_err(CmdError::new)
}

#[tauri::command]
async fn sync_now(state: State<'_, AppState>, id: Uuid) -> CmdResult<()> {
    state.pairs.sync_now(id);
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
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            let pairs = sync::Pairs::load(&dir);
            app.manage(AppState { client: Mutex::new(None), store: Store::new(dir), pairs });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            session,
            login,
            logout,
            list,
            create_folder,
            local_folder,
            sync_pairs,
            add_sync_pair,
            remove_sync_pair,
            sync_now,
            confirm_deletions
        ])
        .run(tauri::generate_context!())
        .expect("error while running the RustCloud client");
}
