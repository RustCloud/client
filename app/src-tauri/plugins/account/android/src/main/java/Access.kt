package com.rustcloud.account

import android.accounts.Account
import android.accounts.AccountManager
import android.content.Context
import android.content.pm.PackageManager
import android.content.pm.Signature
import android.os.Build
import android.os.Process
import java.security.MessageDigest

/**
 * Who gets tokens: apps signed by our key always, apps of other developers once
 * the user let them in (ConsentActivity). A permission is kept for the package
 * and its signing certificate: an app reinstalled under the same name but signed
 * by someone else is asked about again.
 */
object Access {
  private const val PREFS = "rustcloud_account_access"

  fun allowed(context: Context, uid: Int): Boolean {
    if (sameSigner(context, uid)) return true
    val prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
    return packages(context, uid).any { pkg ->
      val kept = prefs.getString(pkg, null)
      kept != null && kept == signer(context, pkg)
    }
  }

  /** Signed by our key: this app itself and our other apps. */
  private fun sameSigner(context: Context, uid: Int): Boolean =
    uid == Process.myUid() ||
      context.packageManager.checkSignatures(Process.myUid(), uid) == PackageManager.SIGNATURE_MATCH

  /** The packages of an app (several only for a shared user id); empty if hidden from us. */
  fun packages(context: Context, uid: Int): List<String> =
    context.packageManager.getPackagesForUid(uid)?.toList() ?: emptyList()

  /** Lets an app in, from now on; false if its signature cannot be read. */
  fun allow(context: Context, account: Account, pkg: String): Boolean {
    val signer = signer(context, pkg) ?: return false
    context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit().putString(pkg, signer).apply()
    // Apps of other developers do not see the account until allowed (Android 8+).
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
      AccountManager.get(context).setAccountVisibility(account, pkg, AccountManager.VISIBILITY_VISIBLE)
    }
    return true
  }

  /** No more tokens for the app; one it already has works out its 10 minutes. */
  fun revoke(context: Context, pkg: String) {
    context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit().remove(pkg).apply()
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
      val manager = AccountManager.get(context)
      for (account in manager.getAccountsByType(Accounts.TYPE)) {
        manager.setAccountVisibility(account, pkg, AccountManager.VISIBILITY_NOT_VISIBLE)
      }
    }
  }

  /** The apps of other developers the user let in: package and name, by name. */
  fun allowedApps(context: Context): List<Pair<String, String>> =
    context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).all.keys
      .map { it to label(context, it) }
      .sortedBy { it.second.lowercase() }

  /** The app's name in the launcher, or its package if it is gone. */
  fun label(context: Context, pkg: String): String {
    val pm = context.packageManager
    return try {
      pm.getApplicationLabel(pm.getApplicationInfo(pkg, 0)).toString()
    } catch (e: PackageManager.NameNotFoundException) {
      pkg
    }
  }

  /** SHA-256 of the app's signing certificates; null if it is not installed or hidden from us. */
  private fun signer(context: Context, pkg: String): String? {
    val pm = context.packageManager
    val found: Array<Signature>? = try {
      if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
        pm.getPackageInfo(pkg, PackageManager.GET_SIGNING_CERTIFICATES).signingInfo?.apkContentsSigners
      } else {
        @Suppress("DEPRECATION")
        pm.getPackageInfo(pkg, PackageManager.GET_SIGNATURES).signatures
      }
    } catch (e: PackageManager.NameNotFoundException) {
      null
    }
    val certs = found ?: return null
    if (certs.isEmpty()) return null
    return certs
      .map { cert ->
        MessageDigest.getInstance("SHA-256").digest(cert.toByteArray()).joinToString("") { "%02x".format(it) }
      }
      .sorted()
      .joinToString(",")
  }
}
