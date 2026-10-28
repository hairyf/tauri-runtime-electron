#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
  tauri::Builder::<tauri_runtime_electron::ElectronRuntime>::new()
    .plugin(tauri_runtime_electron::window::init())
    .run(tauri::generate_context!("tauri.conf.json"))
    .expect("error while running transparent window fixture");
}
