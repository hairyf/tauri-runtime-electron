fn main() -> Result<(), Box<dyn std::error::Error>> {
  println!("cargo:rerun-if-changed=../../../api/window.js");
  std::fs::copy("../../../api/window.js", "window-api.js")?;
  tauri_build::try_build(tauri_build::Attributes::new().plugin(
    "electron-window",
    tauri_build::InlinedPlugin::new().commands(&["set_effects"]),
  ))?;
  Ok(())
}
