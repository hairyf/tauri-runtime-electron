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
  println!("cargo:rerun-if-changed=build_support.rs");
  for key in ["CARGO_TARGET_DIR", "INIT_CWD", "PWD", "TAURI_CONFIG"] {
    println!("cargo:rerun-if-env-changed={key}");
  }
  let mut builder = electron_rs::ElectronBuilder::new(electron_rs::DEFAULT_VERSION);
  let directory = builder.directory().map_err(|error| error.to_string())?;
  let target_root = directory.parent().and_then(|path| path.parent()).ok_or("Electron output has no Cargo target root")?;
  let triple = env::var("TARGET").map_err(|error| error.to_string())?;
  let target = Target::from_triple(&triple);
  let app = build_support::discover(target_root, target)?;
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
    builder = builder.app_name(app.name);
    if matches!(target, Target::Windows | Target::MacOS) {
      if let Some(identifier) = app.identifier { builder = builder.identifier(identifier); }
      if let Some(icon) = app.icon { builder = builder.icon(icon); }
    }
  }
  let builder = builder.output(&directory);
  let prepared = builder.build().map_err(|error| error.to_string())?;
  if let Some(warning) = prepared.cleanup_warning {
    println!("cargo:warning={warning}");
  }
  let output_root = prepared.directory.parent().ok_or("Electron output has no parent")?;
  build_support::prune_foreign_distributions(output_root, &prepared.directory)?;
  println!("cargo:rustc-env=ELECTRON_RS_STAGED_BINARY={}", prepared.binary.display());
  Ok(())
}
