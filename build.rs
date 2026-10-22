//! Stages an Electron runtime for the current target so `cargo run` works
//! without a Node toolchain.
//!
//! Resolution order and download behaviour live in `src/acquisition_shared.rs`
//! (the same file the runtime uses), so a build never disagrees with a launch.
//! The staged copy lands in `target/electron/<platform>-<arch>` and is exported
//! as `TAURI_RUNTIME_ELECTRON_ELECTRON_DIR` / `_BINARY` for the runtime.
//!
//! Failures only warn: a build without network still links, and the runtime
//! downloads Electron on first launch instead.

include!("src/acquisition_shared.rs");

fn main() {
  println!("cargo:rerun-if-changed=build.rs");
  println!("cargo:rerun-if-changed=src/acquisition_shared.rs");
  println!("cargo:rerun-if-changed=electron/main.cjs");
  println!("cargo:rerun-if-changed=electron/preload.cjs");
  println!("cargo:rerun-if-env-changed=TAURI_RUNTIME_ELECTRON_ELECTRON_DIR");
  println!("cargo:rerun-if-env-changed=TAURI_RUNTIME_ELECTRON_DOWNLOAD");
  println!("cargo:rerun-if-env-changed=ELECTRON_MIRROR");
  println!("cargo:rerun-if-env-changed=ELECTRON_CUSTOM_DIR");
  println!("cargo:rerun-if-env-changed=ELECTRON_CACHE");

  if env::var("TAURI_RUNTIME_ELECTRON_DOWNLOAD").is_ok_and(|value| value == "0") {
    return;
  }

  let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap_or_default());
  let searched = [manifest.clone(), env::current_dir().unwrap_or_default()];
  let source = match explicit_dir()
    .or_else(|| searched.iter().find_map(|dir| local_install(dir)))
    .or_else(cached_dist)
  {
    Some(path) => path,
    None => match download() {
      Ok(path) => path,
      Err(error) => {
        println!("cargo:warning=electron download skipped: {error}");
        return;
      }
    },
  };

  match stage(&source) {
    Ok(staged) => {
      println!(
        "cargo:rustc-env=TAURI_RUNTIME_ELECTRON_ELECTRON_DIR={}",
        staged.display()
      );
      let binary = staged.join(executable());
      println!(
        "cargo:rustc-env=TAURI_RUNTIME_ELECTRON_ELECTRON_BINARY={}",
        binary.display()
      );
      // Re-stage when someone deletes or moves the runtime, like cef-dll-sys does.
      println!("cargo:rerun-if-changed={}", staged.display());
    }
    Err(error) => println!("cargo:warning=could not stage electron: {error}"),
  }
}


/// Copies `source` into `$CARGO_TARGET_DIR/electron/<platform>-<arch>` (once per
/// version) and returns that path.
fn stage(source: &Path) -> Result<PathBuf, String> {
  let target = target_dir()?
    .join("electron")
    .join(format!("{}-{}", platform(), arch()));
  let marker = target.join(format!(".version-{VERSION}"));
  if !marker.exists() {
    let _ = fs::remove_dir_all(&target);
    copy_dir(source, &target)?;
  }
  // The main-process scripts change far more often than the distribution, so
  // they are refreshed on every build instead of only with a fresh stage.
  let scripts = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap_or_default()).join("electron");
  for name in ["main.cjs", "preload.cjs"] {
    let from = scripts.join(name);
    if from.exists() {
      fs::copy(&from, target.join(name))
        .map_err(|error| format!("{}: {error}", from.display()))?;
    }
  }
  fs::write(marker, VERSION).map_err(|error| format!("{}: {error}", target.display()))?;
  Ok(target)
}

/// `$CARGO_TARGET_DIR`, else the profile directory above `OUT_DIR`.
fn target_dir() -> Result<PathBuf, String> {
  if let Ok(dir) = env::var("CARGO_TARGET_DIR") {
    return Ok(PathBuf::from(dir));
  }
  let out = PathBuf::from(env::var("OUT_DIR").map_err(|_| "no OUT_DIR")?);
  out
    .ancestors()
    .find(|dir| {
      dir
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == "debug" || name == "release")
    })
    .and_then(Path::parent)
    .map(Path::to_path_buf)
    .ok_or_else(|| format!("could not locate the target directory above {}", out.display()))
}
