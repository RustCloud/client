fn main() {
    // No commands for the page: only the app's Rust code talks to the plugin.
    tauri_plugin::Builder::new(&[]).android_path("android").build();
}
