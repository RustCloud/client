//! The phone's RustCloud account (Android): Settings → Accounts → RustCloud,
//! through which our other apps get access tokens (`AccountManager.getAuthToken`)
//! without logging in themselves.
//!
//! The Android side is in `plugins/account`: the authenticator service and the
//! warning for apps of other developers. It asks this module for tokens over JNI
//! ([`token`]): one place renews them, with the same [`session::slot`] the app
//! uses, as two renewers of one refresh token would end the session.
//!
//! Elsewhere [`publish`] does nothing and the list of allowed apps is absent.

use serde::Serialize;
use tauri::AppHandle;

use crate::{Account, CmdError, CmdResult};

/// The account's name in Settings: `alice@cloud.example.com`, with the port if any.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
fn system_name(account: &Account) -> String {
    let host = url::Url::parse(&account.server)
        .ok()
        .and_then(|url| url.host_str().map(|host| (host.to_owned(), url.port())))
        .map(|(host, port)| port.map_or(host.clone(), |port| format!("{host}:{port}")))
        .unwrap_or_else(|| account.server.clone());
    format!("{}@{host}", account.username)
}

/// Makes the phone's account match the login: `None` removes it.
pub fn publish(#[allow(unused)] app: &AppHandle, #[allow(unused)] account: Option<&Account>) {
    #[cfg(target_os = "android")]
    {
        use tauri::Manager;
        let Some(accounts) = app.try_state::<tauri_plugin_rc_account::Accounts<tauri::Wry>>() else { return };
        let name = account.map(system_name);
        let system = account
            .zip(name.as_deref())
            .map(|(account, name)| tauri_plugin_rc_account::SystemAccount { name, server: &account.server });
        if let Err(e) = accounts.set(system) {
            tracing::warn!("the phone's account was not updated: {e}");
        }
    }
}

/// The app as the account service finds it, when its window is open.
#[cfg(target_os = "android")]
static APP: std::sync::Mutex<Option<AppHandle>> = std::sync::Mutex::new(None);

pub fn attach(#[allow(unused)] app: &AppHandle) {
    #[cfg(target_os = "android")]
    {
        *APP.lock().unwrap_or_else(|e| e.into_inner()) = Some(app.clone());
    }
}

/// An app of another developer that the user let use the account.
#[derive(Serialize)]
pub struct AllowedApp {
    package: String,
    label: String,
}

/// The apps of other developers with access to the account; `None` where there
/// is no phone account (desktops).
#[tauri::command]
pub async fn account_apps(#[allow(unused)] app: AppHandle) -> CmdResult<Option<Vec<AllowedApp>>> {
    #[cfg(target_os = "android")]
    {
        use tauri::Manager;
        let Some(accounts) = app.try_state::<tauri_plugin_rc_account::Accounts<tauri::Wry>>() else {
            return Ok(None);
        };
        let apps = accounts.allowed_apps().map_err(CmdError::new)?;
        Ok(Some(apps.into_iter().map(|a| AllowedApp { package: a.package, label: a.label }).collect()))
    }
    #[cfg(not(target_os = "android"))]
    Ok(None)
}

/// No more tokens for the app.
#[tauri::command]
pub async fn account_revoke(#[allow(unused)] app: AppHandle, #[allow(unused)] package: String) -> CmdResult<()> {
    #[cfg(target_os = "android")]
    {
        use tauri::Manager;
        if let Some(accounts) = app.try_state::<tauri_plugin_rc_account::Accounts<tauri::Wry>>() {
            accounts.revoke(&package).map_err(CmdError::new)?;
        }
        Ok(())
    }
    #[cfg(not(target_os = "android"))]
    Err(CmdError::new("Нет аккаунта телефона"))
}

/// What the authenticator answers an app with: `{"token", "server", "expiresAt"}`,
/// or `{"error": "login" | "network", "message"}`. Blocks on the network.
#[cfg(target_os = "android")]
fn token(dir: std::path::PathBuf) -> serde_json::Value {
    use serde_json::json;
    tauri::async_runtime::block_on(async move {
        let client = {
            let slot = crate::session::slot();
            let mut slot = slot.lock().await;
            if slot.is_none() {
                // The system started the process for the authenticator alone.
                let store = crate::session::Store::new(dir);
                *slot = store.load().map(|saved| store.client(saved.server, saved.tokens));
            }
            slot.clone()
        };
        let Some(client) = client else {
            return json!({"error": "login", "message": "Не выполнен вход в RustCloud"});
        };
        match client.access_token().await {
            Ok(token) => {
                let tokens = client.tokens().await;
                let mut reply = json!({"token": token, "server": client.server().as_str()});
                // Another renewal may have come in between: then the end is unknown.
                if tokens.access_token == token {
                    reply["expiresAt"] = tokens.access_expires_at.timestamp_millis().into();
                }
                reply
            }
            Err(e) if e.needs_login() => json!({"error": "login", "message": e.to_string()}),
            Err(e) => json!({"error": "network", "message": e.to_string()}),
        }
    })
}

/// The account was removed in Settings: log out as the app's "Выйти" does.
#[cfg(target_os = "android")]
fn removed(dir: std::path::PathBuf) {
    tauri::async_runtime::block_on(async move {
        let app = APP.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if let Some(app) = app {
            crate::sign_out(&app, false).await;
            use tauri::Emitter;
            let _ = app.emit("logged-out", ());
            return;
        }
        let store = crate::session::Store::new(dir);
        let client = crate::session::slot().lock().await.take();
        if let Some(client) = client.or_else(|| store.load().map(|saved| store.client(saved.server, saved.tokens))) {
            if let Err(e) = client.logout().await {
                tracing::warn!("logout on the server failed: {e}");
            }
        }
        store.forget();
    })
}

/// The functions of `com.rustcloud.account.Native` (plugins/account).
#[cfg(target_os = "android")]
mod jni_api {
    use std::panic::{catch_unwind, AssertUnwindSafe};
    use std::path::PathBuf;

    use jni::objects::{JByteArray, JClass, JObjectArray, JString};
    use jni::sys::{jint, jstring};
    use jni::JNIEnv;

    fn dir(env: &mut JNIEnv, dir: &JString) -> Option<PathBuf> {
        env.get_string(dir).ok().map(|dir| PathBuf::from(String::from(dir)))
    }

    #[no_mangle]
    pub extern "system" fn Java_com_rustcloud_account_Native_nativeToken<'l>(
        mut env: JNIEnv<'l>,
        _class: JClass<'l>,
        data_dir: JString<'l>,
    ) -> jstring {
        let reply = dir(&mut env, &data_dir)
            .and_then(|dir| catch_unwind(AssertUnwindSafe(|| super::token(dir))).ok())
            .unwrap_or_else(|| serde_json::json!({"error": "network", "message": "internal error"}));
        env.new_string(reply.to_string()).map(|s| s.into_raw()).unwrap_or(std::ptr::null_mut())
    }

    /// The phone's CAs (`TrustStore.kt`), for self-hosted servers with a home CA.
    #[no_mangle]
    pub extern "system" fn Java_com_rustcloud_account_Native_nativeTrust<'l>(
        mut env: JNIEnv<'l>,
        _class: JClass<'l>,
        certs: JObjectArray<'l>,
    ) -> jint {
        let mut der = Vec::new();
        for i in 0..env.get_array_length(&certs).unwrap_or(0) {
            let Ok(cert) = env.get_object_array_element(&certs, i) else { continue };
            if let Ok(bytes) = env.convert_byte_array(JByteArray::from(cert)) {
                der.push(bytes);
            }
        }
        catch_unwind(|| rc_core::trust_roots(der)).map_or(0, |taken| taken as jint)
    }

    #[no_mangle]
    pub extern "system" fn Java_com_rustcloud_account_Native_nativeRemoved<'l>(
        mut env: JNIEnv<'l>,
        _class: JClass<'l>,
        data_dir: JString<'l>,
    ) {
        if let Some(dir) = dir(&mut env, &data_dir) {
            let _ = catch_unwind(AssertUnwindSafe(|| super::removed(dir)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(server: &str) -> Account {
        Account { server: server.into(), username: "alice".into(), display_name: "Alice".into() }
    }

    #[test]
    fn system_names() {
        assert_eq!(system_name(&account("https://cloud.example.com/")), "alice@cloud.example.com");
        assert_eq!(system_name(&account("http://192.168.4.131:8080/")), "alice@192.168.4.131:8080");
    }
}
