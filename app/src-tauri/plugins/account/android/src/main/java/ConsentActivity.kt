package com.rustcloud.account

import android.accounts.Account
import android.accounts.AccountAuthenticatorResponse
import android.accounts.AccountManager
import android.app.Activity
import android.app.AlertDialog
import android.content.Context
import android.content.Intent
import android.content.res.Configuration
import android.os.Bundle
import java.util.UUID
import java.util.concurrent.ConcurrentHashMap

/**
 * "App X asks for your RustCloud account": a warning for apps of other
 * developers. The asking app starts it, so it is exported; it trusts nothing in
 * the intent but a random id that the authenticator handed out with the request
 * (an app starting it by itself has no request to answer).
 */
class ConsentActivity : Activity() {
  private class Request(val response: AccountAuthenticatorResponse, val account: Account, val uid: Int)

  companion object {
    private const val EXTRA_REQUEST = "com.rustcloud.account.request"
    private val pending = ConcurrentHashMap<String, Request>()

    fun request(context: Context, response: AccountAuthenticatorResponse, account: Account, uid: Int): Intent {
      val id = UUID.randomUUID().toString()
      pending[id] = Request(response, account, uid)
      return Intent(context, ConsentActivity::class.java).putExtra(EXTRA_REQUEST, id)
    }
  }

  private var request: Request? = null
  private var answered = false

  override fun onCreate(savedInstanceState: Bundle?) {
    super.onCreate(savedInstanceState)
    val request = intent.getStringExtra(EXTRA_REQUEST)?.let { pending.remove(it) }
    if (request == null) {
      finish()
      return
    }
    this.request = request
    // Visible now: an app that starts our screen becomes visible to us.
    val pkg = Access.packages(this, request.uid).firstOrNull()
    val who = if (pkg != null) "«${Access.label(this, pkg)}» ($pkg)" else "Неизвестное приложение"
    val once = if (pkg == null) "\n\nОпределить приложение не удалось: доступ будет дан только на этот раз." else ""
    AlertDialog.Builder(this, dialogTheme())
      .setTitle("Доступ к аккаунту RustCloud")
      .setMessage(
        "Приложение $who просит доступ к вашему аккаунту RustCloud ${request.account.name}.\n\n" +
          "Это приложение не от разработчиков RustCloud. С доступом оно сможет от вашего имени " +
          "читать, изменять и удалять ваши файлы на сервере.\n\n" +
          "Разрешайте, только если вы сами установили это приложение и доверяете ему. " +
          "Отозвать доступ можно в RustCloud на вкладке «Доступ».$once",
      )
      .setNegativeButton("Запретить") { _, _ -> deny() }
      .setPositiveButton("Разрешить") { _, _ -> allow(request, pkg) }
      .setOnCancelListener { deny() }
      .setOnDismissListener { finish() }
      .show()
  }

  override fun onDestroy() {
    deny() // closed without an answer
    super.onDestroy()
  }

  private fun allow(request: Request, pkg: String?) {
    if (answered) return
    answered = true
    if (pkg != null) Access.allow(this, request.account, pkg)
    val context = applicationContext
    // The token may take the network: off the main thread.
    Thread { request.response.onResult(Tokens.result(context, request.account)) }.start()
  }

  private fun deny() {
    if (answered) return
    answered = true
    request?.response?.onError(AccountManager.ERROR_CODE_CANCELED, "The user denied access to the RustCloud account")
  }

  private fun dialogTheme(): Int {
    val night = resources.configuration.uiMode and Configuration.UI_MODE_NIGHT_MASK == Configuration.UI_MODE_NIGHT_YES
    return if (night) android.R.style.Theme_DeviceDefault_Dialog_Alert else android.R.style.Theme_DeviceDefault_Light_Dialog_Alert
  }
}
