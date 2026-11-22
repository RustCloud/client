package com.rustcloud.account

import android.content.ContentProvider
import android.content.ContentValues
import android.database.Cursor
import android.net.Uri
import android.util.Log
import java.security.KeyStore

/**
 * Gives the app's Rust code the CAs the phone trusts, before anything in the
 * process talks to the server: Android creates content providers when it starts
 * the process, ahead of the window and of the account service.
 *
 * rustls cannot read the phone's store, and a self-hosted server is often signed
 * by a home CA that the user installed (Settings → Security → Encryption &
 * credentials → Install a certificate → CA certificate). `AndroidCAStore` lists
 * those ("user:…") with the system ones ("system:…", minus the ones turned off).
 * A CA installed while the app runs counts from its next start.
 */
class TrustStore : ContentProvider() {
  override fun onCreate(): Boolean {
    try {
      val certs = phoneCAs()
      val taken = Native.trust(certs)
      Log.i("RustCloud", "trusting $taken of the phone's ${certs.size} CAs")
    } catch (e: Throwable) {
      // Without them only public CAs work; the app must still start.
      Log.w("RustCloud", "the phone's CAs were not read", e)
    }
    return true
  }

  private fun phoneCAs(): Array<ByteArray> {
    val store = KeyStore.getInstance("AndroidCAStore").apply { load(null) }
    return store.aliases().toList()
      .mapNotNull { alias -> runCatching { store.getCertificate(alias)?.encoded }.getOrNull() }
      .toTypedArray()
  }

  override fun query(uri: Uri, projection: Array<out String>?, selection: String?, args: Array<out String>?, sort: String?): Cursor? = null
  override fun getType(uri: Uri): String? = null
  override fun insert(uri: Uri, values: ContentValues?): Uri? = null
  override fun delete(uri: Uri, selection: String?, args: Array<out String>?): Int = 0
  override fun update(uri: Uri, values: ContentValues?, selection: String?, args: Array<out String>?): Int = 0
}
