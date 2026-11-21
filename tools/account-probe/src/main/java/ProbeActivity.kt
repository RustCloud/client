package com.rustcloud.probe

import android.accounts.Account
import android.accounts.AccountManager
import android.accounts.OperationCanceledException
import android.app.Activity
import android.content.Intent
import android.os.Build
import android.os.Bundle
import android.widget.Button
import android.widget.LinearLayout
import android.widget.TextView
import java.net.HttpURLConnection
import java.net.URL
import java.text.DateFormat
import java.util.Date

/**
 * Checks the phone's RustCloud account as another app would use it: gets a
 * token from the RustCloud client and asks the server who it belongs to.
 */
class ProbeActivity : Activity() {
  private companion object {
    const val TYPE = "com.rustcloud.account"
    const val PICK = 1
  }

  private lateinit var output: TextView

  override fun onCreate(savedInstanceState: Bundle?) {
    super.onCreate(savedInstanceState)
    val pad = (16 * resources.displayMetrics.density).toInt()
    output = TextView(this).apply {
      setTextIsSelectable(true)
      setPadding(0, pad, 0, 0)
      text = "Приложение: $packageName"
    }
    val button = Button(this).apply {
      text = "Войти через RustCloud"
      setOnClickListener { probe() }
    }
    setContentView(LinearLayout(this).apply {
      orientation = LinearLayout.VERTICAL
      setPadding(pad, pad * 3, pad, pad)
      addView(button)
      addView(output)
    })
  }

  private fun show(text: String) {
    output.text = "Приложение: $packageName\n\n$text"
  }

  private fun probe() {
    val accounts = AccountManager.get(this).getAccountsByType(TYPE)
    if (accounts.isNotEmpty()) return token(accounts[0])
    // Apps of other developers do not see the account until the user picks it.
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
      show("Аккаунт не виден, выберите его…")
      val pick = AccountManager.newChooseAccountIntent(null, null, arrayOf(TYPE), null, null, null, null)
      startActivityForResult(pick, PICK)
    } else {
      show("Аккаунта RustCloud нет: войдите в приложении RustCloud.")
    }
  }

  @Deprecated("Deprecated in Java")
  override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
    super.onActivityResult(requestCode, resultCode, data)
    if (requestCode != PICK) return
    val name = data?.getStringExtra(AccountManager.KEY_ACCOUNT_NAME)
    if (resultCode != RESULT_OK || name == null) return show("Аккаунт не выбран.")
    token(Account(name, TYPE))
  }

  private fun token(account: Account) {
    show("Аккаунт: ${account.name}\nЗапрашиваю токен…")
    AccountManager.get(this).getAuthToken(account, "api", null, this, { future ->
      try {
        val result = future.result
        val token = result.getString(AccountManager.KEY_AUTHTOKEN) ?: return@getAuthToken show("Токена нет: $result")
        val server = result.getString("server") ?: return@getAuthToken show("Нет адреса сервера: $result")
        val expires = result.getLong("expiresAt", 0)
        val until = if (expires > 0) DateFormat.getTimeInstance().format(Date(expires)) else "?"
        val head = "Аккаунт: ${account.name}\nСервер: $server\nТокен: ${token.take(12)}… (до $until)\n\n"
        show(head + "Спрашиваю сервер, кто я…")
        Thread {
          val me = try { me(server, token) } catch (e: Exception) { "Ошибка: $e" }
          runOnUiThread { show(head + "GET /api/v1/auth/me → $me") }
        }.start()
      } catch (e: OperationCanceledException) {
        show("Доступ не дан.")
      } catch (e: Exception) {
        show("Ошибка: $e")
      }
    }, null)
  }

  private fun me(server: String, token: String): String {
    val connection = URL(URL(server), "api/v1/auth/me").openConnection() as HttpURLConnection
    connection.setRequestProperty("Authorization", "Bearer $token")
    return try {
      val code = connection.responseCode
      val body = (if (code in 200..299) connection.inputStream else connection.errorStream)
        ?.bufferedReader()?.use { it.readText() }
      "HTTP $code\n$body"
    } finally {
      connection.disconnect()
    }
  }
}
