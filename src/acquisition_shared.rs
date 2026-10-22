// Acquisition of the Electron distribution shared by the build script and the
// runtime.
//
// Mirrors the resolution order of `@electron/get`: an explicit
// `TAURI_RUNTIME_ELECTRON_ELECTRON_DIR` wins, then a local
// `node_modules/electron` install, then the `ELECTRON_CACHE` /
// `~/.cache/electron` cache, downloading from `ELECTRON_MIRROR` when the
// artifact is missing and checking it against the release's
// `SHASUMS256.txt`.
//
// Nothing new is pulled into the dependency graph: downloads use `curl`, the
// digest is checked with `certutil`/`shasum`, and the zip is unpacked with
// `tar` (libarchive, shipped with Windows 10+ and macOS) or `unzip`.
//
// `include!`d by `build.rs` and by `src/acquisition.rs`, so the two paths can
// never disagree about a version, a mirror or a checksum.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub(crate) const VERSION: &str = "45.0.0";

/// The target being built for, so cross builds fetch the right archive.
pub(crate) fn target_triple() -> String {
  env::var("TARGET").unwrap_or_else(|_| format!("{}-{}", env::consts::ARCH, env::consts::OS))
}

/// Executable path inside an Electron distribution.
pub(crate) fn executable() -> String {
  if target_triple().contains("windows") {
    "electron.exe".into()
  } else if target_triple().contains("apple-darwin") {
    "Electron.app/Contents/MacOS/Electron".into()
  } else {
    "electron".into()
  }
}

/// Platform name used by Electron's release archives.
pub(crate) fn platform() -> String {
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
pub(crate) fn arch() -> String {
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

/// The directory an Electron distribution root is expected in, if it exists.
pub(crate) fn dist_in(dir: &Path) -> Option<PathBuf> {
  dir.join(executable()).exists().then(|| dir.to_path_buf())
}

pub(crate) fn explicit_dir() -> Option<PathBuf> {
  env::var("TAURI_RUNTIME_ELECTRON_ELECTRON_DIR")
    .ok()
    .map(PathBuf::from)
    .and_then(|path| dist_in(&path))
}

/// A `node_modules/electron` install between `start` and the filesystem root.
pub(crate) fn local_install(start: &Path) -> Option<PathBuf> {
  let mut dir = Some(start);
  while let Some(current) = dir {
    let candidate = current.join("node_modules").join("electron").join("dist");
    if let Some(dist) = dist_in(&candidate) {
      return Some(dist);
    }
    dir = current.parent();
  }
  None
}

/// Cache root, matching `@electron/get`'s `ELECTRON_CACHE` override.
pub(crate) fn cache_root() -> Option<PathBuf> {
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
pub(crate) fn cached_dist() -> Option<PathBuf> {
  let dist = cache_root()?
    .join(format!("{}-{}", platform(), arch()))
    .join(VERSION);
  dist_in(&dist)
}

/// Downloads and unpacks the artifact into the cache; returns the dist dir.
pub(crate) fn download() -> Result<PathBuf, String> {
  let cache = cache_root().ok_or("no cache directory")?;
  let mirror = env::var("ELECTRON_MIRROR")
    .unwrap_or_else(|_| "https://github.com/electron/electron/releases/download/".into());
  let dir_name = env::var("ELECTRON_CUSTOM_DIR").unwrap_or_else(|_| format!("v{VERSION}"));
  let base = format!(
    "{}/{}",
    mirror.trim_end_matches('/'),
    dir_name.trim_end_matches('/')
  );
  let name = artifact();
  fs::create_dir_all(&cache).map_err(|error| format!("{}: {error}", cache.display()))?;
  let archive = cache.join(&name);

  // Unpacked copies are versioned, so an older cache entry never shadows this one.
  let dist = cache
    .join(format!("{}-{}", platform(), arch()))
    .join(VERSION);
  if let Some(dist) = dist_in(&dist) {
    return Ok(dist);
  }

  if !archive.exists() {
    let url = format!("{base}/{name}");
    println!("[electron] downloading Electron {VERSION} from {url}");
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
  dist_in(&dist).ok_or_else(|| format!("{} has no {}", dist.display(), executable()))
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
  // Windows and POSIX disagree on the tool, the flag order and even whether
  // the digest is printed first, so try the common ones and pick the digest.
  let path_display = path.display().to_string();
  let candidates: [(&str, &[&str]); 3] = if cfg!(windows) {
    [
      ("certutil", &["-hashfile", &path_display, "SHA256"]),
      ("sha256sum", &[&path_display]),
      ("openssl", &["dgst", "-sha256", &path_display]),
    ]
  } else {
    [
      ("shasum", &["-a", "256", &path_display]),
      ("sha256sum", &[&path_display]),
      ("openssl", &["dgst", "-sha256", &path_display]),
    ]
  };
  let mut output = None;
  for (program, args) in candidates {
    if let Ok(result) = Command::new(program).args(args).output() {
      if result.status.success() {
        output = Some(result);
        break;
      }
    }
  }
  let output = output.ok_or_else(|| "no sha256 tool available".to_string())?;
  let text = String::from_utf8_lossy(&output.stdout).to_lowercase();
  text
    .lines()
    .map(str::trim)
    .find(|line| line.len() == 64 && line.chars().all(|c| c.is_ascii_hexdigit()))
    .map(str::to_string)
    .ok_or_else(|| "no sha256 in the hash tool output".to_string())
}

fn unpack(archive: &Path, into: &Path) -> Result<(), String> {
  // bsdtar (Windows 10+, macOS) takes `-C`, unzip takes `-d`.
  let tar = Command::new("tar")
    .arg("-xf")
    .arg(archive)
    .arg("-C")
    .arg(into)
    .status();
  if tar.is_ok_and(|status| status.success()) {
    return Ok(());
  }
  let unzip = Command::new("unzip")
    .arg("-q")
    .arg("-o")
    .arg(archive)
    .arg("-d")
    .arg(into)
    .status();
  if unzip.is_ok_and(|status| status.success()) {
    return Ok(());
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

pub(crate) fn copy_dir(from: &Path, to: &Path) -> Result<(), String> {
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

/// Ensures a cached Electron distribution with our main-process scripts next to
/// its executable, downloading it when necessary. Returns the executable.
#[allow(dead_code)] // shared with the build script, which only stages the distribution.
pub(crate) fn install(main_cjs: &str, preload_cjs: &str) -> Result<PathBuf, String> {
  let dist = match cached_dist() {
    Some(dist) => dist,
    None => download()?,
  };
  // The scripts are ours and change with the crate, so they are refreshed on
  // every run rather than only when missing.
  for (name, contents) in [("main.cjs", main_cjs), ("preload.cjs", preload_cjs)] {
    let script = dist.join(name);
    fs::write(&script, contents).map_err(|error| format!("{}: {error}", script.display()))?;
  }
  Ok(dist.join(executable()))
}
