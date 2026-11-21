//! The phone's RustCloud account: Settings → Accounts → RustCloud, through
//! which our other apps get access tokens without logging in themselves.
//!
//! The Android side (`android/`) is the account authenticator: a service the
//! system calls when an app asks for a token. Tokens themselves come from the
//! app's Rust code over JNI (see the app's `account.rs`): one place renews them.
//! This crate only lets the app add, update and remove the account, and manage
//! which apps of other developers the user let in. Elsewhere it does nothing.

use serde::{Deserialize, Serialize};
use tauri::plugin::{Builder, TauriPlugin};
use tauri::Runtime;

/// The logged-in account as the system shows it.
pub struct SystemAccount<'a> {
    /// `alice@cloud.example.com`.
    pub name: &'a str,
    pub server: &'a str,
}

/// An app of another developer that the user let in.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AllowedApp {
    pub package: String,
    /// Its name in the launcher; the package if it is gone.
    pub label: String,
}

pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("rc-account")
        .setup(|_app, _api| {
            #[cfg(target_os = "android")]
            {
                use tauri::Manager;
                let handle = _api.register_android_plugin("com.rustcloud.account", "AccountPlugin")?;
                _app.manage(android::Accounts(handle));
            }
            Ok(())
        })
        .build()
}

#[cfg(target_os = "android")]
pub use android::Accounts;

#[cfg(target_os = "android")]
mod android {
    use super::*;
    use tauri::plugin::PluginHandle;

    /// The plugin's Android side. Its calls wait for the main thread: never call
    /// them from it.
    pub struct Accounts<R: Runtime>(pub(crate) PluginHandle<R>);

    #[derive(Serialize)]
    struct SetAccount<'a> {
        name: Option<&'a str>,
        server: Option<&'a str>,
    }

    #[derive(Serialize)]
    struct Revoke<'a> {
        app: &'a str,
    }

    #[derive(Deserialize)]
    struct Apps {
        apps: Vec<AllowedApp>,
    }

    impl<R: Runtime> Accounts<R> {
        /// Makes the system's account match the login: `None` removes it.
        pub fn set(&self, account: Option<SystemAccount<'_>>) -> Result<(), String> {
            let args =
                SetAccount { name: account.as_ref().map(|a| a.name), server: account.as_ref().map(|a| a.server) };
            self.0.run_mobile_plugin::<serde_json::Value>("setAccount", args).map(drop).map_err(|e| e.to_string())
        }

        pub fn allowed_apps(&self) -> Result<Vec<AllowedApp>, String> {
            self.0
                .run_mobile_plugin::<Apps>("allowedApps", serde_json::json!({}))
                .map(|r| r.apps)
                .map_err(|e| e.to_string())
        }

        pub fn revoke(&self, package: &str) -> Result<(), String> {
            self.0
                .run_mobile_plugin::<serde_json::Value>("revoke", Revoke { app: package })
                .map(drop)
                .map_err(|e| e.to_string())
        }
    }
}
