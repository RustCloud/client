package com.rustcloud.account

import android.accounts.Account
import android.accounts.AccountManager
import android.content.Context
import android.os.Bundle
import android.util.Log

/** What `AccountManager.getAuthToken` answers. Blocks on the network. */
internal object Tokens {
  private const val TAG = "RustCloudAccount"

  /**
   * A token, the server and when the token ends ([Accounts.KEY_SERVER],
   * [Accounts.KEY_EXPIRES_AT]); the app's screen if the user must log in again;
   * or an error.
   */
  fun result(context: Context, account: Account): Bundle {
    val known = AccountManager.get(context).getAccountsByType(Accounts.TYPE)
    if (known.none { it == account }) {
      return error(AccountManager.ERROR_CODE_BAD_ARGUMENTS, "No such RustCloud account")
    }
    val reply = try {
      Native.token(context)
    } catch (e: Throwable) {
      Log.e(TAG, "token failed", e)
      return error(AccountManager.ERROR_CODE_REMOTE_EXCEPTION, e.toString())
    }
    if (reply.has("token")) {
      return Bundle().apply {
        putString(AccountManager.KEY_ACCOUNT_NAME, account.name)
        putString(AccountManager.KEY_ACCOUNT_TYPE, account.type)
        putString(AccountManager.KEY_AUTHTOKEN, reply.getString("token"))
        putString(Accounts.KEY_SERVER, reply.getString("server"))
        if (reply.has("expiresAt")) putLong(Accounts.KEY_EXPIRES_AT, reply.getLong("expiresAt"))
      }
    }
    val message = reply.optString("message")
    if (reply.optString("error") == "login") {
      val login = Accounts.openApp(context)
      if (login != null) return Bundle().apply { putParcelable(AccountManager.KEY_INTENT, login) }
      return error(AccountManager.ERROR_CODE_BAD_AUTHENTICATION, message)
    }
    return error(AccountManager.ERROR_CODE_NETWORK_ERROR, message)
  }

  fun error(code: Int, message: String): Bundle = Bundle().apply {
    putInt(AccountManager.KEY_ERROR_CODE, code)
    putString(AccountManager.KEY_ERROR_MESSAGE, message)
  }
}
