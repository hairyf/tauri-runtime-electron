//! Acquire and brand Electron at build time; launching only reads packaged files.

mod build_support;

use std::{env, path::PathBuf};
use tauri_utils::platform::Target;

fn main() {
  if let Err(error) = prepare() {
    panic!("Electron build failed: {error}");
  }
}

fn prepare() -> Result<(), String> {
  for path in ["build_support.rs", "electron-rs/assets/main.cjs", "electron-rs/assets/preload.cjs"] {
    println!("cargo:rerun-if-changed={path}");
  }
  for key in ["CARGO_TARGET_DIR", "INIT_CWD", "PWD", "TAURI_CONFIG"] {
    println!("cargo:rerun-if-env-changed={key}");
  }
  let target_root = target_dir()?;
  let triple = env::var("TARGET").map_err(|error| error.to_string())?;
  let target = Target::from_triple(&triple);
  let app = build_support::discover(&target_root, target)?;
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
  let directory = target_root.join("electron").join(format!("{}-{}", electron_rs::platform(), electron_rs::arch()));
  let builder = electron_rs::ElectronBuilder::new(electron_rs::DEFAULT_VERSION)
    .output(&directory).metadata(metadata);
  builder.build()?;
  println!("cargo:rustc-env=ELECTRON_RS_STAGED_BINARY={}", builder.binary()?.display());
  Ok(())
}

fn target_dir() -> Result<PathBuf, String> {
  // OUT_DIR belongs to the consuming Cargo build, unlike this dependency's cwd.
  let out = PathBuf::from(env::var_os("OUT_DIR").ok_or("no Cargo OUT_DIR")?);
  let profile = out.ancestors().nth(3).ok_or("unexpected Cargo OUT_DIR layout")?;
  let mut target = profile.parent().ok_or("Cargo profile has no target directory")?;
  if target.file_name().is_some_and(|name| env::var_os("TARGET").as_deref() == Some(name)) {
    target = target.parent().ok_or("Cargo target triple has no parent directory")?;
  }
  Ok(target.to_path_buf())
}
