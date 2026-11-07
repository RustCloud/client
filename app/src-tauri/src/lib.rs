//! The Tauri shell: commands the Svelte UI calls, over `rc-core`.

mod session;

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
async fn session(state: State<'_, AppState>) -> CmdResult<Option<Account>> {
    let mut slot = state.client.lock().await;
    if slot.is_none() {
        *slot = state.store.load().map(|saved| state.store.client(saved.server, saved.tokens));
    }
    let Some(client) = slot.as_ref() else { return Ok(None) };
    match account(client).await {
        Ok(account) => Ok(Some(account)),
        Err(e) if e.needs_login => {
            *slot = None;
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
    *state.client.lock().await = Some(client);
    Ok(account)
}

#[tauri::command]
async fn logout(state: State<'_, AppState>) -> CmdResult<()> {
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

/// A folder's contents; the root without `folder`.
#[tauri::command]
async fn list(state: State<'_, AppState>, folder: Option<Uuid>) -> CmdResult<Listing> {
    let client = state
        .client
        .lock()
        .await
        .clone()
        .ok_or_else(|| CmdError { needs_login: true, ..CmdError::new("Не выполнен вход") })?;
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            app.manage(AppState { client: Mutex::new(None), store: Store::new(dir) });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![session, login, logout, list])
        .run(tauri::generate_context!())
        .expect("error while running the RustCloud client");
}
