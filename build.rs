//! Stages an Electron runtime for the current target so consumers can run
//! `cargo install`-style builds without a Node toolchain.
//!
//! Mirrors the resolution order of `@electron/get`: honour an explicit
//! `TAURI_RUNTIME_ELECTRON_ELECTRON_DIR`, reuse a local `node_modules/electron`
//! install, then fall back to the `ELECTRON_CACHE` / `~/.cache/electron` cache,
//! downloading from `ELECTRON_MIRROR` when the artifact is missing and checking
//! it against the release's `SHASUMS256.txt`.
//!
//! Nothing new is pulled into the dependency graph: downloads use `curl`, the
//! digest is checked with `certutil`/`shasum`, and the zip is unpacked with
//! `tar` (libarchive, shipped with Windows 10+ and macOS) or `unzip`.
//!
//! Exports `TAURI_RUNTIME_ELECTRON_ELECTRON_DIR` / `_BINARY` at compile time so
//! the runtime can find the staged copy under `target/electron/<platform>-<arch>`.
//! Failures only warn: a build without network still links and falls back to an
//! `electron` executable on `PATH`.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const VERSION: &str = "45.0.0";

fn main() {
  println!("cargo:rerun-if-changed=build.rs");
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
    .or_else(|| cached_dist())
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

/// The target being built for, so cross builds fetch the right archive.
fn target_triple() -> String {
  env::var("TARGET").unwrap_or_else(|_| format!("{}-{}", env::consts::ARCH, env::consts::OS))
}

/// Executable path inside an Electron distribution.
fn executable() -> String {
  if target_triple().contains("windows") {
    "electron.exe".into()
  } else if target_triple().contains("apple-darwin") {
    "Electron.app/Contents/MacOS/Electron".into()
  } else {
    "electron".into()
  }
}

/// Platform name used by Electron's release archives.
fn platform() -> String {
  let triple = target_triple();
  if triple.contains("windows") {
    "win32".into()
  } else if triple.contains("apple-darwin") {
    "darwin".into()
  } else {
    "linux".into()
  }
}

/// Architecture name used by Electron's release archives.
fn arch() -> String {
  let triple = target_triple();
  let cpu = triple.split('-').next().unwrap_or_default();
  match cpu {
    "x86_64" => "x64",
    "aarch64" => "arm64",
    "i686" | "i586" | "i386" => "ia32",
    "armv7" => "armv7l",
    other => other,
  }
  .into()
}

fn explicit_dir() -> Option<PathBuf> {
  env::var("TAURI_RUNTIME_ELECTRON_ELECTRON_DIR")
    .ok()
    .map(PathBuf::from)
    .filter(|path| path.join(executable()).exists())
}

/// A `node_modules/electron` install between `start` and the filesystem root.
fn local_install(start: &Path) -> Option<PathBuf> {
  let mut dir = Some(start);
  while let Some(current) = dir {
    let candidate = current.join("node_modules").join("electron").join("dist");
    if candidate.join(executable()).exists() {
      return Some(candidate);
    }
    dir = current.parent();
  }
  None
}

/// Cache root, matching `@electron/get`'s `ELECTRON_CACHE` override.
fn cache_root() -> Option<PathBuf> {
  if let Ok(path) = env::var("ELECTRON_CACHE") {
    return Some(PathBuf::from(path));
  }
  let home = if cfg!(windows) {
    env::var("LOCALAPPDATA").ok()
  } else {
    env::var("HOME").ok()
  }?;
  Some(PathBuf::from(home).join(if cfg!(windows) {
    "electron/Cache"
  } else {
    ".cache/electron"
  }))
}

fn artifact() -> String {
  format!("electron-v{VERSION}-{}-{}.zip", platform(), arch())
}

/// An already unpacked and cached artifact for this version/platform.
fn cached_dist() -> Option<PathBuf> {
  let dist = cache_root()?.join(format!("{}-{}", platform(), arch())).join(VERSION);
  dist.join(executable()).exists().then_some(dist)
}

/// Downloads and unpacks the artifact into the cache; returns the dist dir.
fn download() -> Result<PathBuf, String> {
  let cache = cache_root().ok_or("no cache directory")?;
  let mirror = env::var("ELECTRON_MIRROR")
    .unwrap_or_else(|_| "https://github.com/electron/electron/releases/download/".into());
  let dir_name = env::var("ELECTRON_CUSTOM_DIR").unwrap_or_else(|_| format!("v{VERSION}"));
  let base = format!("{}/{}", mirror.trim_end_matches('/'), dir_name.trim_end_matches('/'));
  let name = artifact();
  fs::create_dir_all(&cache).map_err(|error| format!("{}: {error}", cache.display()))?;
  let archive = cache.join(&name);

  // Unpacked copies are versioned, so an older cache entry never shadows this one.
  let dist = cache.join(format!("{}-{}", platform(), arch())).join(VERSION);
  if dist.join(executable()).exists() {
    return Ok(dist);
  }

  if !archive.exists() {
    let url = format!("{base}/{name}");
    run(
      Command::new("curl")
        .args(["-fsSL", "--retry", "2", "-o"])
        .arg(&archive)
        .arg(&url),
    )
    .map_err(|error| format!("download {url}: {error}"))?;
  }
  if let Some(expected) = expected_digest(&format!("{base}/SHASUMS256.txt"), &name)? {
    let actual = digest(&archive)?;
    if !actual.eq_ignore_ascii_case(&expected) {
      let _ = fs::remove_file(&archive);
      return Err(format!("checksum mismatch for {name}: {actual} != {expected}"));
    }
  }

  let _ = fs::remove_dir_all(&dist);
  fs::create_dir_all(&dist).map_err(|error| format!("{}: {error}", dist.display()))?;
  unpack(&archive, &dist).map_err(|error| format!("unpack {name}: {error}"))?;
  Ok(dist)
}

/// `SHASUMS256.txt` is a list of `<digest> *<file>` lines.
fn expected_digest(url: &str, name: &str) -> Result<Option<String>, String> {
  let output = Command::new("curl")
    .args(["-fsSL", "--max-time", "60", url])
    .output()
    .map_err(|error| format!("fetch {url}: {error}"))?;
  if !output.status.success() {
    return Ok(None);
  }
  let text = String::from_utf8_lossy(&output.stdout);
  Ok(text.lines().find_map(|line| {
    let (digest, file) = line.trim().split_once(' ')?;
    (file.trim_start_matches('*') == name).then(|| digest.to_lowercase())
  }))
}

fn digest(path: &Path) -> Result<String, String> {
  // Windows and POSIX disagree on both the tool and the flag order.
  let output = if cfg!(windows) {
    Command::new("certutil")
      .args(["-hashfile", &path.display().to_string(), "SHA256"])
      .output()
  } else {
    Command::new("shasum")
      .args(["-a", "256", &path.display().to_string()])
      .output()
  }
  .map_err(|error| format!("sha256: {error}"))?;
  let text = String::from_utf8_lossy(&output.stdout).to_lowercase();
  text
    .lines()
    .map(str::trim)
    .find(|line| line.len() == 64 && line.chars().all(|c| c.is_ascii_hexdigit()))
    .map(str::to_string)
    .ok_or_else(|| "no sha256 in the hash tool output".to_string())
}

fn unpack(archive: &Path, into: &Path) -> Result<(), String> {
  for (program, args) in [("tar", vec!["-xf"]), ("unzip", vec!["-q"])] {
    let mut command = Command::new(program);
    command.args(&args).arg(archive).arg("-d").arg(into);
    if run(&mut command).is_ok() {
      return Ok(());
    }
  }
  Err("no usable extractor (tar or unzip)".into())
}

fn run(command: &mut Command) -> Result<(), String> {
  match command.status() {
    Ok(status) if status.success() => Ok(()),
    Ok(status) => Err(format!("exit {status}")),
    Err(error) => Err(error.to_string()),
  }
}

/// Copies `source` into `$CARGO_TARGET_DIR/electron/<platform>-<arch>` (once per
/// version) and returns that path.
fn stage(source: &Path) -> Result<PathBuf, String> {
  let target = target_dir()?.join("electron").join(format!("{}-{}", platform(), arch()));
  let marker = target.join(format!(".version-{VERSION}"));
  if marker.exists() {
    return Ok(target);
  }
  let _ = fs::remove_dir_all(&target);
  copy_dir(source, &target)?;
  // Our main-process scripts ship next to the runtime in packaged layouts.
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

fn copy_dir(from: &Path, to: &Path) -> Result<(), String> {
  fs::create_dir_all(to).map_err(|error| format!("{}: {error}", to.display()))?;
  for entry in fs::read_dir(from).map_err(|error| format!("{}: {error}", from.display()))? {
    let entry = entry.map_err(|error| error.to_string())?;
    let target = to.join(entry.file_name());
    let kind = entry.file_type().map_err(|error| error.to_string())?;
    if kind.is_dir() {
      copy_dir(&entry.path(), &target)?;
    } else {
      fs::copy(entry.path(), &target)
        .map_err(|error| format!("{}: {error}", entry.path().display()))?;
    }
  }
  Ok(())
}
