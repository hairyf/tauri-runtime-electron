//! Prepares the Electron distribution this crate drives, and tells the bundler
//! where it is.
//!
//! Mirrors `tauri-runtime-cef`'s build script: the runtime crate, not the
//! application, owns its webview engine payload. `cargo:runtime=electron` and
//! `cargo:electron_dir=…` are exposed to the build scripts of crates depending on
//! this one as `DEP_TAURI_RUNTIME_ELECTRON_RUNTIME` and
//! `DEP_TAURI_RUNTIME_ELECTRON_ELECTRON_DIR`, which is how `tauri-build` and the
//! bundler learn that the application embeds its own Chromium.

mod build_support;

use std::env;
use tauri_utils::platform::Target;

fn main() {
  println!("cargo:rerun-if-changed=build.rs");
  println!("cargo:rerun-if-changed=build_support.rs");
  // The bundler ships the distribution this build prepares. Emitting the
  // instruction before the preparation means a failed download still leaves the
  // bundler with a coherent (if empty) answer instead of a stale one.
  println!("cargo:runtime=electron");
  if let Err(error) = prepare() {
    panic!("Electron build failed: {error}");
  }
}

fn prepare() -> Result<(), String> {
  for key in ["CARGO_TARGET_DIR", "INIT_CWD", "PWD", "TAURI_CONFIG"] {
    println!("cargo:rerun-if-env-changed={key}");
  }
  let mut builder = electron_rs::ElectronBuilder::new(electron_rs::DEFAULT_VERSION);
  // A crate built outside an application (`cargo check` of the library alone, a
  // workspace with no binary target) has no Cargo build directory to stage into.
  // That is not a failure: the runtime resolves the distribution at launch, and a
  // real application build always has one.
  let Ok(directory) = builder.directory() else {
    println!(
      "cargo:warning=no Cargo build directory in this context; Electron is staged by the application build"
    );
    return Ok(());
  };
  let target_root = directory
    .parent()
    .and_then(|path| path.parent())
    .ok_or("Electron output has no Cargo target root")?;
  let output_root = directory.parent().ok_or("Electron output has no parent")?;
  let _preparation = build_support::lock_preparation(output_root)?;
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
    println!(
      "cargo:rustc-env=ELECTRON_RS_TAURI_PRODUCT_NAME={}",
      app.name
    );
    if let Some(identifier) = &app.identifier {
      println!("cargo:rustc-env=ELECTRON_RS_TAURI_IDENTIFIER={identifier}");
    }
    builder = builder.app_name(app.name);
    if matches!(target, Target::Windows | Target::MacOS) {
      if let Some(identifier) = app.identifier {
        builder = builder.identifier(identifier);
      }
      if let Some(icon) = app.icon {
        builder = builder.icon(icon);
      }
    }
  }
  let builder = builder.output(&directory);
  let prepared = builder.build().map_err(|error| error.to_string())?;
  if let Some(warning) = prepared.cleanup_warning {
    println!("cargo:warning={warning}");
  }
  let output_root = prepared
    .directory
    .parent()
    .ok_or("Electron output has no parent")?;
  build_support::prune_foreign_distributions(output_root, &prepared.directory)?;
  // The bundler copies this directory into the application bundle. It is the same
  // path the runtime resolves at launch, so a packaged app and a `cargo run` find
  // the same distribution.
  println!("cargo:electron_dir={}", prepared.directory.display());
  println!(
    "cargo:rustc-env=ELECTRON_RS_STAGED_BINARY={}",
    prepared.binary.display()
  );
  Ok(())
}
