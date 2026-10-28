#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use tauri_runtime_electron::ElectronRuntime;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tauri::Builder::<ElectronRuntime>::new()
        .run(tauri::generate_context!())?;
    Ok(())
}
