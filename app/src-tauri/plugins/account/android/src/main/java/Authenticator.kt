package com.rustcloud.account

import android.accounts.AbstractAccountAuthenticator
import android.accounts.Account
import android.accounts.AccountAuthenticatorResponse
import android.accounts.AccountManager
import android.app.Service
import android.content.Context
import android.content.Intent
import android.os.Bundle
import android.os.IBinder
import android.util.Log

/** What the system binds for AccountManager calls about RustCloud accounts. */
class AuthenticatorService : Service() {
  private lateinit var authenticator: Authenticator

  override fun onCreate() {
    super.onCreate()
    authenticator = Authenticator(this)
  }

  override fun onBind(intent: Intent?): IBinder? = authenticator.iBinder
}

/**
 * Runs on binder threads of the app's process, which the system starts for it
 * if needed; blocking there on the network is allowed.
 */
internal class Authenticator(private val context: Context) : AbstractAccountAuthenticator(context) {
  /** Settings → Add account → RustCloud: the app's login screen. */
  override fun addAccount(
    response: AccountAuthenticatorResponse?,
    accountType: String?,
    authTokenType: String?,
    requiredFeatures: Array<out String>?,
    options: Bundle?,
  ): Bundle {
    val login = Accounts.openApp(context)
      ?: return Tokens.error(AccountManager.ERROR_CODE_UNSUPPORTED_OPERATION, "No login screen")
    return Bundle().apply { putParcelable(AccountManager.KEY_INTENT, login) }
  }

  override fun getAuthToken(
    response: AccountAuthenticatorResponse?,
    account: Account?,
    authTokenType: String?,
    options: Bundle?,
  ): Bundle? {
    if (response == null || account == null) {
      return Tokens.error(AccountManager.ERROR_CODE_BAD_ARGUMENTS, "No account")
    }
    // Put there by the system, not by the asking app.
    val uid = options?.getInt(AccountManager.KEY_CALLER_UID, -1) ?: -1
    if (uid < 0) return Tokens.error(AccountManager.ERROR_CODE_BAD_REQUEST, "Unknown caller")
    if (Access.allowed(context, uid)) return Tokens.result(context, account)
    // Another developer's app: the user decides. The asking app starts this screen.
    val consent = ConsentActivity.request(context, response, account, uid)
    return Bundle().apply { putParcelable(AccountManager.KEY_INTENT, consent) }
  }

  /** Settings → Accounts → RustCloud → Remove: the app logs out. */
  override fun getAccountRemovalAllowed(response: AccountAuthenticatorResponse?, account: Account?): Bundle {
    try {
      Native.removed(context)
    } catch (e: Throwable) {
      Log.e("RustCloudAccount", "logout failed", e)
    }
    return Bundle().apply { putBoolean(AccountManager.KEY_BOOLEAN_RESULT, true) }
  }

  override fun getAuthTokenLabel(authTokenType: String?): String = "RustCloud"

  override fun hasFeatures(
    response: AccountAuthenticatorResponse?,
    account: Account?,
    features: Array<out String>?,
  ): Bundle = Bundle().apply { putBoolean(AccountManager.KEY_BOOLEAN_RESULT, features.isNullOrEmpty()) }

  override fun editProperties(response: AccountAuthenticatorResponse?, accountType: String?): Bundle? =
    throw UnsupportedOperationException()

  override fun confirmCredentials(
    response: AccountAuthenticatorResponse?,
    account: Account?,
    options: Bundle?,
  ): Bundle? = null

  override fun updateCredentials(
    response: AccountAuthenticatorResponse?,
    account: Account?,
    authTokenType: String?,
    options: Bundle?,
  ): Bundle? = null
}
