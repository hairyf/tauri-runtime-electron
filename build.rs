//! Acquire and brand Electron at build time; launching only reads packaged files.

mod build_support;

use std::env;
use tauri_utils::platform::Target;

fn main() {
  if let Err(error) = prepare() {
    panic!("Electron build failed: {error}");
  }
}

fn prepare() -> Result<(), String> {
  for path in ["build_support.rs", "electron-rs/assets/main.cjs", "electron-rs/assets/internal.cjs", "electron-rs/assets/commands.cjs", "electron-rs/assets/preload.cjs", "electron-rs/assets/ipc.cjs"] {
    println!("cargo:rerun-if-changed={path}");
  }
  for key in ["CARGO_TARGET_DIR", "INIT_CWD", "PWD", "TAURI_CONFIG"] {
    println!("cargo:rerun-if-env-changed={key}");
  }
  let builder = electron_rs::ElectronBuilder::new(electron_rs::DEFAULT_VERSION);
  let directory = builder.directory().map_err(|error| error.to_string())?;
  let target_root = directory.parent().and_then(|path| path.parent()).ok_or("Electron output has no Cargo target root")?;
  let triple = env::var("TARGET").map_err(|error| error.to_string())?;
  let target = Target::from_triple(&triple);
  let app = build_support::discover(target_root, target)?;
  let mut metadata = electron_rs::Metadata::default();
  if let Some(app) = app {
    for input in &app.inputs {
      println!("cargo:rerun-if-changed={}", input.display());
    }
    if let Some(icon) = &app.icon {
      println!("cargo:rerun-if-changed={}", icon.display());
    }
    println!("cargo:rustc-env=ELECTRON_RS_TAURI_PRODUCT_NAME={}", app.name);
    if let Some(identifier) = &app.identifier {
      println!("cargo:rustc-env=ELECTRON_RS_TAURI_IDENTIFIER={identifier}");
    }
    metadata.app_name = Some(app.name);
    if matches!(target, Target::Windows | Target::MacOS) {
      metadata.identifier = app.identifier;
      metadata.icon = app.icon;
    }
  }
  let builder = builder.output(&directory).metadata(metadata);
  let prepared = builder.build().map_err(|error| error.to_string())?;
  if let Some(warning) = prepared.cleanup_warning {
    println!("cargo:warning={warning}");
  }
  println!("cargo:rustc-env=ELECTRON_RS_STAGED_BINARY={}", prepared.binary.display());
  Ok(())
}
