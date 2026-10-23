#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
  tauri::Builder::default()
    .plugin(tauri_runtime_electron::window::init())
    .run(tauri::generate_context!("tauri.conf.json"))
    .expect("error while running transparent window example");
}
