//! Build-time acquisition using the platform's curl, SHA-256 and ZIP tools.
//! Downloaded archives must match the release checksum before extraction.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The target being built for, so cross builds fetch the right archive.
pub fn target_triple() -> String {
  // `TARGET` is set for build scripts and missing at runtime; `env::consts::OS`
  // spells macOS "macos", while the triples below match on "apple-darwin".
  env::var("TARGET").unwrap_or_else(|_| {
    let os = match env::consts::OS {
      "macos" => "apple-darwin".to_string(),
      other => other.to_string(),
    };
    format!("{}-{}", env::consts::ARCH, os)
  })
}

/// Executable path inside an Electron distribution.
pub fn executable() -> String {
  if target_triple().contains("windows") {
    "electron.exe".into()
  } else if target_triple().contains("apple-darwin") {
    "Electron.app/Contents/MacOS/Electron".into()
  } else {
    "electron".into()
  }
}

/// Platform name used by Electron's release archives.
pub fn platform() -> String {
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
pub fn arch() -> String {
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
pub fn dist_in(dir: &Path) -> Option<PathBuf> {
  dir.join(executable()).is_file().then(|| dir.to_path_buf())
}

pub fn default_output() -> PathBuf {
  PathBuf::from(env::var_os("CARGO_TARGET_DIR").unwrap_or_else(|| "target".into()))
    .join("electron").join(format!("{}-{}", platform(), arch()))
}

pub fn unique_sibling(path: &Path, kind: &str) -> PathBuf {
  use std::sync::atomic::{AtomicU64, Ordering};
  static NEXT: AtomicU64 = AtomicU64::new(0);
  let serial = NEXT.fetch_add(1, Ordering::Relaxed);
  path.with_file_name(format!(".{}-{kind}-{}-{serial}", path.file_name().unwrap_or_default().to_string_lossy(), std::process::id()))
}

/// Publish a prepared directory without discarding the previous output on failure.
pub fn replace_directory(stage: &Path, output: &Path) -> Result<(), String> {
  if !output.exists() {
    return fs::rename(stage, output).map_err(|error| format!("{}: {error}", output.display()));
  }
  let backup = unique_sibling(output, "previous");
  fs::rename(output, &backup).map_err(|error| format!("{}: {error}", output.display()))?;
  if let Err(error) = fs::rename(stage, output) {
    fs::rename(&backup, output).map_err(|rollback| {
      format!("publish {}: {error}; previous output remains at {} (restore failed: {rollback})", output.display(), backup.display())
    })?;
    return Err(format!("publish {}: {error}", output.display()));
  }
  fs::remove_dir_all(&backup).map_err(|error| format!("clean previous output {}: {error}", backup.display()))
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

/// Shared cache under the host platform's normal cache directory.
pub(crate) fn cache_root() -> Option<PathBuf> {
  if cfg!(windows) {
    return env::var_os("LOCALAPPDATA").map(|root| PathBuf::from(root).join("electron/Cache"));
  }
  if cfg!(target_os = "macos") {
    return env::var_os("HOME").map(|root| PathBuf::from(root).join("Library/Caches/electron"));
  }
  env::var_os("XDG_CACHE_HOME")
    .map(PathBuf::from)
    .filter(|root| root.is_absolute())
    .or_else(|| env::var_os("HOME").map(|root| PathBuf::from(root).join(".cache")))
    .map(|root| root.join("electron"))
}

fn artifact(version: &str) -> String {
  format!("electron-v{version}-{}-{}.zip", platform(), arch())
}

/// An already unpacked and cached artifact for this version/platform.
pub fn cached_dist(version: &str) -> Option<PathBuf> {
  let dist = cache_root()?
    .join(format!("{}-{}", platform(), arch()))
    .join(version);
  dist_in(&dist)
}

/// Downloads and unpacks the artifact into the cache; returns the dist dir.
pub fn download(version: &str) -> Result<PathBuf, String> {
  let cache = cache_root().ok_or("no cache directory")?;
  let base = format!("https://github.com/electron/electron/releases/download/v{version}");
  let name = artifact(version);
  fs::create_dir_all(&cache).map_err(|error| format!("{}: {error}", cache.display()))?;
  let archive = cache.join(&name);

  // Unpacked copies are versioned, so an older cache entry never shadows this one.
  let dist = cache
    .join(format!("{}-{}", platform(), arch()))
    .join(version);
  if let Some(dist) = dist_in(&dist) {
    return Ok(dist);
  }

  if !archive.exists() {
    let url = format!("{base}/{name}");
    println!("[electron] downloading Electron {version} from {url}");
    run(
      Command::new("curl")
        .args(["-fsSL", "--retry", "2", "-o"])
        .arg(&archive)
        .arg(&url),
    )
    .map_err(|error| format!("download {url}: {error}"))?;
  }
  let expected = expected_digest(&format!("{base}/SHASUMS256.txt"), &name)?;
  let actual = digest(&archive)?;
  if !actual.eq_ignore_ascii_case(&expected) {
    let _ = fs::remove_file(&archive);
    return Err(format!("checksum mismatch for {name}: {actual} != {expected}"));
  }

  fs::create_dir_all(dist.parent().ok_or("cache path has no parent")?)
    .map_err(|error| error.to_string())?;
  let stage = unique_sibling(&dist, "unpack");
  fs::create_dir(&stage).map_err(|error| format!("{}: {error}", stage.display()))?;
  let result = (|| {
    unpack(&archive, &stage).map_err(|error| format!("unpack {name}: {error}"))?;
    if dist_in(&stage).is_none() { return Err(format!("{name} has no {}", executable())); }
    replace_directory(&stage, &dist)?;
    Ok(dist)
  })();
  if result.is_err() { let _ = fs::remove_dir_all(&stage); }
  result
}

/// `SHASUMS256.txt` is a list of `<digest> *<file>` lines.
fn expected_digest(url: &str, name: &str) -> Result<String, String> {
  let output = Command::new("curl")
    .args(["-fsSL", "--max-time", "60", url])
    .output()
    .map_err(|error| format!("fetch {url}: {error}"))?;
  if !output.status.success() {
    return Err(format!("cannot verify Electron archive: fetch {url} failed ({})", output.status));
  }
  checksum(&String::from_utf8_lossy(&output.stdout), name)
    .ok_or_else(|| format!("no SHA-256 for {name} in {url}"))
}

fn checksum(text: &str, name: &str) -> Option<String> {
  text.lines().find_map(|line| {
    let mut parts = line.split_whitespace();
    let digest = parts.next()?;
    let file = parts.next()?.trim_start_matches('*');
    (file == name && digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit()))
      .then(|| digest.to_ascii_lowercase())
  })
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
  // `certutil`/`shasum`/`sha256sum` print "digest  path" while `openssl` prints
  // "SHA2-256(path)= digest", so look for a digest-sized token rather than a
  // line of that length.
  text
    .split_whitespace()
    .find(|token| token.len() == 64 && token.chars().all(|c| c.is_ascii_hexdigit()))
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

pub fn copy_dir(from: &Path, to: &Path) -> Result<(), String> {
  fs::create_dir_all(to).map_err(|error| format!("{}: {error}", to.display()))?;
  for entry in fs::read_dir(from).map_err(|error| format!("{}: {error}", from.display()))? {
    let entry = entry.map_err(|error| error.to_string())?;
    let target = to.join(entry.file_name());
    let kind = entry.file_type().map_err(|error| error.to_string())?;
    if kind.is_dir() {
      copy_dir(&entry.path(), &target)?;
    } else if kind.is_symlink() {
      copy_symlink(entry.path(), &target)?;
    } else {
      fs::copy(entry.path(), &target)
        .map_err(|error| format!("{}: {error}", entry.path().display()))?;
    }
  }
  Ok(())
}

#[cfg(unix)]
fn copy_symlink(from: PathBuf, to: &Path) -> Result<(), String> {
  let link = fs::read_link(&from).map_err(|error| format!("{}: {error}", from.display()))?;
  std::os::unix::fs::symlink(&link, to).map_err(|error| format!("{}: {error}", to.display()))
}

#[cfg(not(unix))]
fn copy_symlink(from: PathBuf, to: &Path) -> Result<(), String> {
  fs::copy(&from, to)
    .map(|_| ())
    .map_err(|error| format!("{}: {error}", from.display()))
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn checksum_requires_a_full_hex_digest_for_the_exact_artifact() {
    let name = artifact("45.0.0");
    let digest = "Ab".repeat(32);
    let invalid = format!("short *{name}\n{} *{name}\n{digest} *{name}.bak\n", "g".repeat(64));
    assert_eq!(checksum(&invalid, &name), None);
    assert_eq!(checksum(&format!("{invalid}{digest} *{name}\n"), &name), Some(digest.to_ascii_lowercase()));
  }
}
