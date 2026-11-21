package com.rustcloud.account

import android.app.Activity
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSArray
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin

@InvokeArg
class SetAccountArgs {
  var name: String? = null
  var server: String? = null
}

@InvokeArg
class RevokeArgs {
  lateinit var app: String
}

/** What the app's Rust code calls (the crate's `Accounts`). */
@TauriPlugin
class AccountPlugin(private val activity: Activity) : Plugin(activity) {
  @Command
  fun setAccount(invoke: Invoke) {
    try {
      val args = invoke.parseArgs(SetAccountArgs::class.java)
      Accounts.publish(activity, args.name, args.server)
      invoke.resolve()
    } catch (e: Exception) {
      invoke.reject(e.message ?: e.toString())
    }
  }

  @Command
  fun allowedApps(invoke: Invoke) {
    val apps = JSArray()
    for ((pkg, label) in Access.allowedApps(activity)) {
      apps.put(JSObject().apply {
        put("package", pkg)
        put("label", label)
      })
    }
    invoke.resolve(JSObject().apply { put("apps", apps) })
  }

  @Command
  fun revoke(invoke: Invoke) {
    try {
      Access.revoke(activity, invoke.parseArgs(RevokeArgs::class.java).app)
      invoke.resolve()
    } catch (e: Exception) {
      invoke.reject(e.message ?: e.toString())
    }
  }
}
