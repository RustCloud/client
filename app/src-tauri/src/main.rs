// No console window next to the app on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    rustcloud_client_lib::run()
}
