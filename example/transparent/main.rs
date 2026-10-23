#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
  tauri::Builder::default()
    .run(tauri::generate_context!("tauri.conf.json"))
    .expect("error while running transparent window example");
}
