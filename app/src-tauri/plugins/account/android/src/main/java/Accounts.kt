package com.rustcloud.account

import android.accounts.Account
import android.accounts.AccountManager
import android.content.Context
import android.content.Intent

/** The RustCloud account in Settings → Accounts: at most one, the app's login. */
object Accounts {
  const val TYPE = "com.rustcloud.account"

  /** User data of the account (readable by apps signed by us) and a key of token results. */
  const val KEY_SERVER = "server"

  /** When the access token ends, milliseconds since 1970 (a key of token results). */
  const val KEY_EXPIRES_AT = "expiresAt"

  /** Makes the system's account match the app's login; `name == null` removes it. */
  fun publish(context: Context, name: String?, server: String?) {
    val manager = AccountManager.get(context)
    val existing = manager.getAccountsByType(TYPE)
    for (account in existing) {
      if (account.name != name) manager.removeAccountExplicitly(account)
    }
    if (name == null) return
    val account = existing.firstOrNull { it.name == name }
      ?: Account(name, TYPE).also { manager.addAccountExplicitly(it, null, null) }
    manager.setUserData(account, KEY_SERVER, server)
  }

  /** The app's own screen: to log in, or to log in again. */
  fun openApp(context: Context): Intent? =
    context.packageManager.getLaunchIntentForPackage(context.packageName)
      ?.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
}
