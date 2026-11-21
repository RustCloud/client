package com.rustcloud.account

import android.content.Context
import org.json.JSONObject

/**
 * The app's Rust code (`account.rs` of the app): it keeps the session and is the
 * only one to renew tokens. The refresh token changes on every renewal and the
 * server ends the session when an old one comes back, so a second renewer here
 * would log the user out.
 *
 * The calls block on the network: never on the main thread.
 */
internal object Native {
  init {
    // Already loaded when the app's window is open; the system may start the
    // process for the authenticator alone.
    System.loadLibrary("rustcloud_client_lib")
  }

  @JvmStatic private external fun nativeToken(dataDir: String): String
  @JvmStatic private external fun nativeRemoved(dataDir: String)

  /** `{"token", "server", "expiresAt"}` or `{"error": "login" | "network", "message"}`. */
  fun token(context: Context): JSONObject = JSONObject(nativeToken(dataDir(context)))

  /** The account was removed in Settings: log out as the app's "Выйти" does. */
  fun removed(context: Context) = nativeRemoved(dataDir(context))

  // Where Tauri keeps the app's data (its app_data_dir on Android): session.json.
  private fun dataDir(context: Context): String = context.dataDir.absolutePath
}
