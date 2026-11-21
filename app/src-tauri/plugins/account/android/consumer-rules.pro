# The JNI functions are found by class and method name (see the app's account.rs).
-keep class com.rustcloud.account.Native { *; }
# Android creates these by name from the manifest; Tauri finds the plugin by name.
-keep class com.rustcloud.account.AccountPlugin { *; }
