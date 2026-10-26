//! Build-time acquisition using the platform's curl, SHA-256 and ZIP tools.
//! Downloaded archives must match the release checksum before extraction.

use std::{
  env,
  fs::{self, File},
  io::{self, Read, Seek, SeekFrom},
  path::{Path, PathBuf},
  process::Command,
};

use crate::Error;

/// The target being built for, so cross builds fetch the right archive.
pub fn target_triple() -> String {
  env::var("TARGET").unwrap_or_else(|_| host_triple())
}

fn host_triple() -> String {
  host_triple_for(env::consts::ARCH, env::consts::OS)
}

fn host_triple_for(arch: &str, os: &str) -> String {
  let arch = match arch { "x86" => "i686", "arm" => "armv7", other => other };
  let os = if os == "macos" { "apple-darwin" } else { os };
  format!("{arch}-{os}")
}

/// One target snapshot for discovery, validation and acquisition during a build.
#[derive(Debug, Clone)]
pub(crate) struct DistributionTarget {
  pub triple: String,
  pub platform: &'static str,
  pub arch: &'static str,
}

impl DistributionTarget {
  pub(crate) fn current() -> Result<Self, Error> {
    let triple = match env::var("TARGET") {
      Ok(triple) => triple,
      Err(env::VarError::NotPresent) => host_triple(),
      Err(error) => return Err(Error::with_source("invalid Cargo TARGET", error)),
    };
    Self::from_triple(&triple)
  }

  pub(crate) fn from_triple(triple: &str) -> Result<Self, Error> {
    let mut parts = triple.split('-');
    let cpu = parts.next().unwrap_or_default();
    let vendor = parts.next();
    let os = parts.next();
    let platform = match (vendor, os) {
      (Some("apple"), Some("darwin")) => "darwin",
      (_, Some("windows")) | (Some("windows"), None) => "win32",
      (_, Some("linux")) | (Some("linux"), None) if !parts.any(|part| part.starts_with("android")) => "linux",
      _ => return Err(format!("unsupported Electron target operating system: {triple}").into()),
    };
    let arch = match cpu {
      "x86_64" => "x64",
      "aarch64" => "arm64",
      "i686" | "i586" | "i386" => "ia32",
      "armv7" => "armv7l",
      _ => return Err(format!("unsupported Electron target architecture: {triple}").into()),
    };
    if !matches!((platform, arch),
      ("win32", "x64" | "arm64" | "ia32") | ("linux", "x64" | "arm64" | "armv7l") | ("darwin", "x64" | "arm64"))
    {
      return Err(format!("unsupported Electron distribution target: {triple}").into());
    }
    Ok(Self { triple: triple.into(), platform, arch })
  }

  pub(crate) fn executable(&self) -> &'static str {
    match self.platform {
      "win32" => "electron.exe",
      "darwin" => "Electron.app/Contents/MacOS/Electron",
      _ => "electron",
    }
  }

  /// Inspect the executable itself; directory names and markers are not evidence.
  pub(crate) fn verify_binary(&self, binary: &Path) -> Result<(), Error> {
    let mut file = File::open(binary)
      .map_err(|error| Error::with_source(format!("open Electron binary {}", binary.display()), error))?;
    let length = file.metadata()
      .map_err(|error| Error::with_source(format!("inspect {}", binary.display()), error))?.len();
    verify_header(&mut file, length, self)
      .map_err(|error| Error::with_source(format!("Electron binary {} is incompatible with {}", binary.display(), self.triple), error))
  }
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
  if triple.contains("windows") { "win32" } else if triple.contains("apple-darwin") { "darwin" } else { "linux" }.into()
}

/// Architecture name used by Electron's release archives.
pub fn arch() -> String {
  let triple = target_triple();
  match triple.split('-').next().unwrap_or_default() {
    "x86_64" => "x64",
    "aarch64" => "arm64",
    "i686" | "i586" | "i386" => "ia32",
    "armv7" => "armv7l",
    other => other,
  }.into()
}

fn invalid_header(message: impl Into<String>) -> io::Error {
  io::Error::new(io::ErrorKind::InvalidData, message.into())
}

fn read_at<const N: usize>(file: &mut File, offset: u64, length: u64) -> io::Result<[u8; N]> {
  if offset.checked_add(N as u64).is_none_or(|end| end > length) {
    return Err(invalid_header("truncated executable header"));
  }
  file.seek(SeekFrom::Start(offset))?;
  let mut bytes = [0; N];
  file.read_exact(&mut bytes)?;
  Ok(bytes)
}

fn number32(bytes: [u8; 4], little: bool) -> u32 {
  if little { u32::from_le_bytes(bytes) } else { u32::from_be_bytes(bytes) }
}

fn number64(bytes: [u8; 8], little: bool) -> u64 {
  if little { u64::from_le_bytes(bytes) } else { u64::from_be_bytes(bytes) }
}

fn mach_arch(header: &[u8; 32]) -> io::Result<&'static str> {
  let little = match &header[..4] {
    [0xcf, 0xfa, 0xed, 0xfe] => true,
    [0xfe, 0xed, 0xfa, 0xcf] => false,
    _ => return Err(invalid_header("expected a 64-bit Mach-O executable")),
  };
  if number32([header[12], header[13], header[14], header[15]], little) != 2 {
    return Err(invalid_header("Mach-O image is not an executable"));
  }
  match number32([header[4], header[5], header[6], header[7]], little) {
    0x0100_0007 => Ok("x64"),
    0x0100_000c => Ok("arm64"),
    _ => Err(invalid_header("unsupported Mach-O CPU")),
  }
}

fn verify_header(file: &mut File, length: u64, target: &DistributionTarget) -> io::Result<()> {
  let magic = read_at::<4>(file, 0, length)?;
  let actual = if magic[..2] == *b"MZ" {
    if target.platform != "win32" { return Err(invalid_header("PE executable requires Windows")); }
    let dos = read_at::<64>(file, 0, length)?;
    let offset = u32::from_le_bytes([dos[60], dos[61], dos[62], dos[63]]) as u64;
    if offset < 64 { return Err(invalid_header("invalid PE header offset")); }
    let pe = read_at::<26>(file, offset, length)?;
    if &pe[..4] != b"PE\0\0" { return Err(invalid_header("missing PE signature")); }
    let machine = u16::from_le_bytes([pe[4], pe[5]]);
    let arch = match machine {
      0x8664 => "x64",
      0xaa64 => "arm64",
      0x014c => "ia32",
      _ => return Err(invalid_header("unsupported PE machine")),
    };
    let optional_size = u16::from_le_bytes([pe[20], pe[21]]) as u64;
    let expected_magic = if arch == "ia32" { 0x010b } else { 0x020b };
    if optional_size < 2 || offset.checked_add(24 + optional_size).is_none_or(|end| end > length)
      || u16::from_le_bytes([pe[24], pe[25]]) != expected_magic
    {
      return Err(invalid_header("PE machine and optional header are inconsistent"));
    }
    arch
  } else if magic == *b"\x7fELF" {
    if target.platform != "linux" { return Err(invalid_header("ELF executable requires Linux")); }
    let header = read_at::<52>(file, 0, length)?;
    let little = match header[5] {
      1 => true,
      2 => false,
      _ => return Err(invalid_header("unsupported ELF byte order")),
    };
    let word = |low, high| if little { u16::from_le_bytes([low, high]) } else { u16::from_be_bytes([low, high]) };
    if header[6] != 1 || !matches!(word(header[16], header[17]), 2 | 3) {
      return Err(invalid_header("ELF image is not an executable"));
    }
    let arch = match (header[4], word(header[18], header[19])) {
      (2, 62) => "x64",
      (2, 183) => "arm64",
      (1, 40) => "armv7l",
      _ => return Err(invalid_header("unsupported or inconsistent ELF class and machine")),
    };
    if header[4] == 2 { read_at::<64>(file, 0, length)?; }
    arch
  } else {
    if target.platform != "darwin" { return Err(invalid_header("unrecognized executable format for target OS")); }
    match magic {
      [0xcf, 0xfa, 0xed, 0xfe] | [0xfe, 0xed, 0xfa, 0xcf] => mach_arch(&read_at::<32>(file, 0, length)?)?,
      [0xca, 0xfe, 0xba, 0xbe] | [0xbe, 0xba, 0xfe, 0xca] | [0xca, 0xfe, 0xba, 0xbf] | [0xbf, 0xba, 0xfe, 0xca] => {
        let little = magic[0] != 0xca;
        let wide = magic[3] == 0xbf || magic[0] == 0xbf;
        let header = read_at::<8>(file, 0, length)?;
        let count = number32([header[4], header[5], header[6], header[7]], little) as u64;
        let entry_size = if wide { 32 } else { 20 };
        if count == 0 || count > 64 || 8 + count * entry_size > length {
          return Err(invalid_header("invalid Mach-O universal architecture table"));
        }
        let mut compatible = false;
        for index in 0..count {
          let offset = 8 + index * entry_size;
          let entry = read_at::<20>(file, offset, length)?;
          let cpu = number32([entry[0], entry[1], entry[2], entry[3]], little);
          let (start, size) = if wide {
            let entry = read_at::<32>(file, offset, length)?;
            (number64([entry[8], entry[9], entry[10], entry[11], entry[12], entry[13], entry[14], entry[15]], little),
             number64([entry[16], entry[17], entry[18], entry[19], entry[20], entry[21], entry[22], entry[23]], little))
          } else {
            (number32([entry[8], entry[9], entry[10], entry[11]], little) as u64,
             number32([entry[12], entry[13], entry[14], entry[15]], little) as u64)
          };
          if start < 8 + count * entry_size || size < 32 || start.checked_add(size).is_none_or(|end| end > length) {
            return Err(invalid_header("Mach-O universal slice is outside executable"));
          }
          let arch = mach_arch(&read_at::<32>(file, start, length)?)?;
          if !matches!((cpu, arch), (0x0100_0007, "x64") | (0x0100_000c, "arm64")) {
            return Err(invalid_header("Mach-O universal table disagrees with slice CPU"));
          }
          compatible |= arch == target.arch;
        }
        if !compatible { return Err(invalid_header("Mach-O universal executable has no target CPU")); }
        return Ok(());
      }
      _ => return Err(invalid_header("unrecognized Mach-O executable header")),
    }
  };
  if actual != target.arch {
    return Err(invalid_header(format!("executable CPU is {actual}, expected {}", target.arch)));
  }
  Ok(())
}

/// Resolve once per build. OUT_DIR identifies the consuming Cargo target root.
/// Outside build scripts, use Cargo's configured target directory; a non-Cargo
/// caller falls back to `cwd/target`.
pub(crate) fn output_directory(explicit: Option<&Path>, target: &DistributionTarget) -> Result<PathBuf, Error> {
  resolve_output(
    explicit,
    target,
    env::var_os("OUT_DIR").as_deref().map(Path::new),
    env::var_os("CARGO_TARGET_DIR").as_deref().map(Path::new),
    &env::current_dir,
  )
}

fn absolute_path(path: &Path, current_dir: &impl Fn() -> io::Result<PathBuf>) -> Result<PathBuf, Error> {
  if path.is_absolute() { return Ok(path.to_path_buf()); }
  current_dir().map(|cwd| cwd.join(path))
    .map_err(|error| Error::with_source("resolve current directory for Electron output", error))
}

fn resolve_output(
  explicit: Option<&Path>,
  target: &DistributionTarget,
  out_dir: Option<&Path>,
  cargo_target_dir: Option<&Path>,
  current_dir: &impl Fn() -> io::Result<PathBuf>,
) -> Result<PathBuf, Error> {
  if let Some(explicit) = explicit { return absolute_path(explicit, current_dir); }
  let root = if let Some(out) = out_dir {
    let out = absolute_path(out, current_dir)?;
    let package = out.parent().filter(|_| out.file_name().is_some_and(|name| name == "out"))
      .ok_or("unexpected Cargo OUT_DIR layout: expected <target>/<profile>/build/<package>/out")?;
    let build = package.parent().filter(|build| build.file_name().is_some_and(|name| name == "build"))
      .ok_or("unexpected Cargo OUT_DIR layout: missing build directory")?;
    let profile = build.parent().ok_or("Cargo OUT_DIR has no profile directory")?;
    let mut root = profile.parent().ok_or("Cargo profile has no target directory")?;
    if root.file_name().is_some_and(|name| name == target.triple.as_str()) {
      let configured = cargo_target_dir.map(|path| absolute_path(path, current_dir)).transpose()?;
      let configured_is_root = configured.as_deref().is_some_and(|configured| {
        configured == root || fs::canonicalize(configured).ok()
          .zip(fs::canonicalize(root).ok()).is_some_and(|(configured, root)| configured == root)
      });
      if !configured_is_root {
        root = root.parent().ok_or("Cargo target triple has no parent directory")?;
      }
    }
    root.to_path_buf()
  } else if let Some(root) = cargo_target_dir {
    absolute_path(root, current_dir)?
  } else {
    let cwd = current_dir().map_err(|error| Error::with_source("resolve current directory for Electron output", error))?;
    let manifest = cwd.ancestors().map(|path| path.join("Cargo.toml")).find(|path| path.is_file());
    project_target_root(&cwd, manifest.as_deref())?
  };
  Ok(root.join("electron").join(format!("{}-{}", target.platform, target.arch)))
}

fn project_target_root(cwd: &Path, manifest: Option<&Path>) -> Result<PathBuf, Error> {
  match manifest {
    Some(manifest) => cargo_target_root(cwd, manifest),
    None => Ok(cwd.join("target")),
  }
}

fn cargo_target_root(cwd: &Path, manifest: &Path) -> Result<PathBuf, Error> {
  let mut command = Command::new(env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
  command.current_dir(cwd).args(["metadata", "--format-version", "1", "--no-deps", "--offline", "--manifest-path"]).arg(manifest);
  // Unit tests pass an environment snapshot; keep their child process consistent.
  #[cfg(test)]
  command.env_remove("CARGO_TARGET_DIR");
  let output = command.output()
    .map_err(|error| Error::with_source(format!("cargo metadata for {}", manifest.display()), error))?;
  if !output.status.success() {
    return Err(format!("cargo metadata for {} failed ({}): {}", manifest.display(), output.status, String::from_utf8_lossy(&output.stderr).trim()).into());
  }
  let value: serde_json::Value = serde_json::from_slice(&output.stdout)
    .map_err(|error| Error::with_source(format!("cargo metadata for {}", manifest.display()), error))?;
  let root = value.get("target_directory").and_then(serde_json::Value::as_str)
    .ok_or("cargo metadata has no target_directory")?;
  let root = PathBuf::from(root);
  if !root.is_absolute() { return Err("cargo metadata target_directory is not absolute".into()); }
  Ok(root)
}

pub(crate) fn unique_sibling(path: &Path, kind: &str) -> PathBuf {
  use std::sync::atomic::{AtomicU64, Ordering};
  static NEXT: AtomicU64 = AtomicU64::new(0);
  let serial = NEXT.fetch_add(1, Ordering::Relaxed);
  path.with_file_name(format!(".{}-{kind}-{}-{serial}", path.file_name().unwrap_or_default().to_string_lossy(), std::process::id()))
}

/// Renaming stage to output commits publication. Later cleanup is diagnostic.
pub(crate) fn replace_directory(stage: &Path, output: &Path) -> Result<Option<Error>, Error> {
  replace_directory_with(stage, output, &mut |from, to| fs::rename(from, to), &mut |path| fs::remove_dir_all(path))
}

#[derive(Debug)]
struct PublishFailure {
  publish: io::Error,
  rollback: io::Error,
}

impl std::fmt::Display for PublishFailure {
  fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    write!(formatter, "{}; restore failed: {}", self.publish, self.rollback)
  }
}

impl std::error::Error for PublishFailure {
  fn source(&self) -> Option<&(dyn std::error::Error + 'static)> { Some(&self.publish) }
}

fn replace_directory_with(
  stage: &Path,
  output: &Path,
  rename: &mut impl FnMut(&Path, &Path) -> io::Result<()>,
  remove_backup: &mut impl FnMut(&Path) -> io::Result<()>,
) -> Result<Option<Error>, Error> {
  let exists = match fs::symlink_metadata(output) {
    Ok(_) => true,
    Err(error) if error.kind() == io::ErrorKind::NotFound => false,
    Err(error) => return Err(Error::with_source(format!("inspect output {}", output.display()), error)),
  };
  if !exists {
    rename(stage, output).map_err(|error| Error::with_source(format!("publish {}", output.display()), error))?;
    return Ok(None);
  }
  let backup = unique_sibling(output, "previous");
  rename(output, &backup).map_err(|error| Error::with_source(format!("back up {} to {}", output.display(), backup.display()), error))?;
  if let Err(error) = rename(stage, output) {
    if let Err(rollback) = rename(&backup, output) {
      let state = match fs::symlink_metadata(output) {
        Ok(_) => "exists",
        Err(error) if error.kind() == io::ErrorKind::NotFound => "is absent",
        Err(_) => "could not be inspected",
      };
      return Err(Error::with_source(
        format!("publish {} failed; previous output remains at {}; current output {state}", output.display(), backup.display()),
        PublishFailure { publish: error, rollback },
      ));
    }
    return Err(Error::with_source(format!("publish {}; previous output restored", output.display()), error));
  }
  Ok(remove_backup(&backup).err().map(|error| Error::with_source(
    format!("new output {} is active; previous backup left at {} because cleanup failed", output.display(), backup.display()),
    error,
  )))
}

fn dist_in(dir: &Path, target: &DistributionTarget) -> Option<PathBuf> {
  target.verify_binary(&dir.join(target.executable())).ok().map(|_| dir.to_path_buf())
}

/// A compatible `node_modules/electron` install between start and the root.
pub(crate) fn local_install(start: &Path, target: &DistributionTarget) -> Option<PathBuf> {
  start.ancestors().find_map(|current| dist_in(&current.join("node_modules/electron/dist"), target))
}

/// Require at least one matching version file, and reject conflicting evidence.
pub(crate) fn version_matches(dir: &Path, version: &str) -> bool {
  let mut found = false;
  for name in ["version", ".electron-rs-version"] {
    match fs::read_to_string(dir.join(name)) {
      Ok(actual) if actual.trim().strip_prefix('v').unwrap_or(actual.trim()) == version => found = true,
      Err(error) if error.kind() == io::ErrorKind::NotFound => (),
      _ => return false,
    }
  }
  found
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

fn artifact(version: &str, target: &DistributionTarget) -> String {
  format!("electron-v{version}-{}-{}.zip", target.platform, target.arch)
}

fn verified_dist(dir: &Path, version: &str, target: &DistributionTarget) -> Option<PathBuf> {
  if !version_matches(dir, version) { return None; }
  dist_in(dir, target)
}

/// An unpacked artifact whose version and executable headers match this target.
pub(crate) fn cached_dist(version: &str, target: &DistributionTarget) -> Option<PathBuf> {
  let dist = cache_root()?.join(format!("{}-{}", target.platform, target.arch)).join(version);
  verified_dist(&dist, version, target)
}

/// Download and unpack only at build time, verifying before publishing the cache.
pub(crate) fn download(version: &str, target: &DistributionTarget) -> Result<PathBuf, Error> {
  let cache = cache_root().ok_or("no cache directory")?;
  let base = format!("https://github.com/electron/electron/releases/download/v{version}");
  let name = artifact(version, target);
  fs::create_dir_all(&cache).map_err(|error| Error::with_source(format!("create {}", cache.display()), error))?;
  let archive = cache.join(&name);
  let dist = cache.join(format!("{}-{}", target.platform, target.arch)).join(version);
  if let Some(dist) = verified_dist(&dist, version, target) { return Ok(dist); }

  // ponytail: one writer per cache artifact; separate caches for concurrent builds.
  if !archive.exists() {
    let url = format!("{base}/{name}");
    println!("[electron] downloading Electron {version} from {url}");
    run(Command::new("curl").args(["-fsSL", "--retry", "2", "-o"]).arg(&archive).arg(&url))
      .map_err(|error| error.context(format!("download {url}")))?;
  }
  let expected = expected_digest(&format!("{base}/SHASUMS256.txt"), &name)?;
  let actual = digest(&archive)?;
  if !actual.eq_ignore_ascii_case(&expected) {
    let _ = fs::remove_file(&archive);
    return Err(format!("checksum mismatch for {name}: {actual} != {expected}").into());
  }

  let parent = dist.parent().ok_or("cache path has no parent")?;
  fs::create_dir_all(parent).map_err(|error| Error::with_source(format!("create {}", parent.display()), error))?;
  let stage = unique_sibling(&dist, "unpack");
  fs::create_dir(&stage).map_err(|error| Error::with_source(format!("create {}", stage.display()), error))?;
  let result = (|| {
    unpack(&archive, &stage).map_err(|error| error.context(format!("unpack {name}")))?;
    target.verify_binary(&stage.join(target.executable()))?;
    if !version_matches(&stage, version) {
      return Err(format!("{name} has no matching Electron {version} version evidence").into());
    }
    if let Some(warning) = replace_directory(&stage, &dist)? { eprintln!("[electron] {warning}"); }
    Ok(dist)
  })();
  if result.is_err() { let _ = fs::remove_dir_all(&stage); }
  result
}

/// `SHASUMS256.txt` is a list of `<digest> *<file>` lines.
fn expected_digest(url: &str, name: &str) -> Result<String, Error> {
  let output = Command::new("curl").args(["-fsSL", "--max-time", "60", url]).output()
    .map_err(|error| Error::with_source(format!("fetch {url}"), error))?;
  if !output.status.success() {
    return Err(format!("cannot verify Electron archive: fetch {url} failed ({}): {}", output.status, String::from_utf8_lossy(&output.stderr).trim()).into());
  }
  checksum(&String::from_utf8_lossy(&output.stdout), name)
    .ok_or_else(|| format!("no SHA-256 for {name} in {url}").into())
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

fn digest(path: &Path) -> Result<String, Error> {
  let path_display = path.display().to_string();
  let candidates: [(&str, &[&str]); 3] = if cfg!(windows) {
    [("certutil", &["-hashfile", &path_display, "SHA256"]), ("sha256sum", &[&path_display]), ("openssl", &["dgst", "-sha256", &path_display])]
  } else {
    [("shasum", &["-a", "256", &path_display]), ("sha256sum", &[&path_display]), ("openssl", &["dgst", "-sha256", &path_display])]
  };
  let mut failure = None;
  for (program, args) in candidates {
    let output = match Command::new(program).args(args).output() {
      Ok(output) => output,
      Err(error) => { failure = Some(Error::with_source(format!("run SHA-256 tool {program}"), error)); continue; }
    };
    if !output.status.success() {
      failure = Some(format!("SHA-256 tool {program} failed ({}): {}", output.status, String::from_utf8_lossy(&output.stderr).trim()).into());
      continue;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    if let Some(digest) = text.split_whitespace().find(|token| token.len() == 64 && token.bytes().all(|byte| byte.is_ascii_hexdigit())) {
      return Ok(digest.to_ascii_lowercase());
    }
    failure = Some(format!("no SHA-256 in {program} output").into());
  }
  Err(failure.unwrap_or_else(|| "no SHA-256 tool available".into()).context("cannot verify Electron archive"))
}

fn unpack(archive: &Path, into: &Path) -> Result<(), Error> {
  let mut tar = Command::new("tar");
  tar.arg("-xf").arg(archive).arg("-C").arg(into);
  if run(&mut tar).is_ok() { return Ok(()); }
  let mut unzip = Command::new("unzip");
  unzip.args(["-q", "-o"]).arg(archive).arg("-d").arg(into);
  run(&mut unzip).map_err(|error| error.context("no usable extractor (tar or unzip)"))
}

fn run(command: &mut Command) -> Result<(), Error> {
  let program = command.get_program().to_string_lossy().into_owned();
  match command.status() {
    Ok(status) if status.success() => Ok(()),
    Ok(status) => Err(format!("{program} exited with {status}").into()),
    Err(error) => Err(Error::with_source(format!("run {program}"), error)),
  }
}

pub(crate) fn copy_dir(from: &Path, to: &Path) -> Result<(), Error> {
  fs::create_dir_all(to).map_err(|error| Error::with_source(format!("create {}", to.display()), error))?;
  for entry in fs::read_dir(from).map_err(|error| Error::with_source(format!("read {}", from.display()), error))? {
    let entry = entry.map_err(|error| Error::with_source(format!("read entry in {}", from.display()), error))?;
    let target = to.join(entry.file_name());
    let kind = entry.file_type().map_err(|error| Error::with_source(format!("inspect {}", entry.path().display()), error))?;
    if kind.is_dir() {
      copy_dir(&entry.path(), &target)?;
    } else if kind.is_symlink() {
      copy_symlink(entry.path(), &target)?;
    } else {
      fs::copy(entry.path(), &target).map_err(|error| Error::with_source(format!("copy {} to {}", entry.path().display(), target.display()), error))?;
    }
  }
  Ok(())
}

#[cfg(unix)]
fn copy_symlink(from: PathBuf, to: &Path) -> Result<(), Error> {
  let link = fs::read_link(&from).map_err(|error| Error::with_source(format!("read link {}", from.display()), error))?;
  std::os::unix::fs::symlink(&link, to).map_err(|error| Error::with_source(format!("create link {}", to.display()), error))
}

#[cfg(not(unix))]
fn copy_symlink(from: PathBuf, to: &Path) -> Result<(), Error> {
  fs::copy(&from, to).map(|_| ())
    .map_err(|error| Error::with_source(format!("copy link {} to {}", from.display(), to.display()), error))
}

/// Header-only, deliberately non-runnable executable fixtures shared by unit tests.
#[cfg(test)]
pub(crate) fn test_binary(target: &DistributionTarget) -> Vec<u8> {
  match target.platform {
    "win32" => {
      let mut bytes = vec![0; 90];
      bytes[..2].copy_from_slice(b"MZ");
      bytes[60..64].copy_from_slice(&64_u32.to_le_bytes());
      bytes[64..68].copy_from_slice(b"PE\0\0");
      let machine: u16 = match target.arch { "x64" => 0x8664, "arm64" => 0xaa64, _ => 0x014c };
      bytes[68..70].copy_from_slice(&machine.to_le_bytes());
      bytes[84..86].copy_from_slice(&2_u16.to_le_bytes());
      let magic: u16 = if target.arch == "ia32" { 0x010b } else { 0x020b };
      bytes[88..90].copy_from_slice(&magic.to_le_bytes());
      bytes
    }
    "darwin" => {
      let mut bytes = vec![0; 32];
      bytes[..4].copy_from_slice(&[0xcf, 0xfa, 0xed, 0xfe]);
      let cpu: u32 = if target.arch == "x64" { 0x0100_0007 } else { 0x0100_000c };
      bytes[4..8].copy_from_slice(&cpu.to_le_bytes());
      bytes[12..16].copy_from_slice(&2_u32.to_le_bytes());
      bytes
    }
    _ => {
      let mut bytes = vec![0; 64];
      bytes[..4].copy_from_slice(b"\x7fELF");
      bytes[4] = if target.arch == "armv7l" { 1 } else { 2 };
      bytes[5] = 1;
      bytes[6] = 1;
      bytes[16..18].copy_from_slice(&3_u16.to_le_bytes());
      let machine: u16 = match target.arch { "x64" => 62, "arm64" => 183, _ => 40 };
      bytes[18..20].copy_from_slice(&machine.to_le_bytes());
      bytes
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn temporary(name: &str) -> PathBuf {
    let root = unique_sibling(&Path::new(env!("CARGO_MANIFEST_DIR")).join("target/electron-acquisition"), name);
    fs::create_dir_all(&root).unwrap();
    root
  }

  fn target(triple: &str) -> DistributionTarget { DistributionTarget::from_triple(triple).unwrap() }

  #[test]
  fn checksum_requires_a_full_hex_digest_for_the_exact_artifact() {
    let name = artifact("45.0.0", &target("aarch64-unknown-linux-gnu"));
    assert_eq!(name, "electron-v45.0.0-linux-arm64.zip");
    let digest = "Ab".repeat(32);
    let invalid = format!("short *{name}\n{} *{name}\n{digest} *{name}.bak\n", "g".repeat(64));
    assert_eq!(checksum(&invalid, &name), None);
    assert_eq!(checksum(&format!("{invalid}{digest} *{name}\n"), &name), Some(digest.to_ascii_lowercase()));
  }

  #[test]
  fn target_rejects_unsupported_platforms_and_architectures() {
    for triple in ["x86_64-unknown-freebsd", "wasm32-unknown-unknown", "riscv64gc-unknown-linux-gnu", "i686-apple-darwin", "x86_64-apple-ios", "aarch64-linux-android", "armv7-unknown-linux-androideabi"] {
      assert!(DistributionTarget::from_triple(triple).is_err(), "{triple}");
    }
  }

  #[test]
  fn host_architecture_names_resolve_without_cargo_target() {
    for (arch, os, expected) in [("x86", "windows", "ia32"), ("arm", "linux", "armv7l"), ("aarch64", "macos", "arm64"), ("x86_64", "macos", "x64")] {
      assert_eq!(target(&host_triple_for(arch, os)).arch, expected);
    }
  }

  #[test]
  fn executable_headers_identify_the_os_and_cpu_without_running_them() {
    let root = temporary("headers");
    let binary = root.join("renamed-binary");
    let triples = ["x86_64-pc-windows-msvc", "aarch64-pc-windows-msvc", "i686-pc-windows-msvc", "x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu", "armv7-unknown-linux-gnueabihf", "x86_64-apple-darwin", "aarch64-apple-darwin"];
    for triple in triples {
      let expected = target(triple);
      fs::write(&binary, test_binary(&expected)).unwrap();
      expected.verify_binary(&binary).unwrap();
      for other in triples {
        let other = target(other);
        if other.platform != expected.platform || other.arch != expected.arch {
          assert!(other.verify_binary(&binary).is_err(), "{triple} accepted as {}", other.triple);
        }
      }
    }
    fs::remove_dir_all(root).unwrap();
  }

  #[test]
  fn running_test_executable_matches_its_host_headers() {
    let host = DistributionTarget::from_triple(&host_triple()).unwrap();
    host.verify_binary(&env::current_exe().unwrap()).unwrap();
  }

  #[test]
  fn elf_machine_is_read_using_its_declared_byte_order() {
    let root = temporary("elf-endian");
    let binary = root.join("electron");
    let expected = target("aarch64-unknown-linux-gnu");
    let mut bytes = test_binary(&expected);
    bytes[5] = 2;
    bytes[16..18].copy_from_slice(&3_u16.to_be_bytes());
    bytes[18..20].copy_from_slice(&183_u16.to_be_bytes());
    fs::write(&binary, bytes).unwrap();
    expected.verify_binary(&binary).unwrap();
    fs::remove_dir_all(root).unwrap();
  }

  #[test]
  fn universal_macho_checks_the_actual_cpu_in_each_slice() {
    let root = temporary("universal");
    let binary = root.join("Electron");
    let x64 = target("x86_64-apple-darwin");
    let arm64 = target("aarch64-apple-darwin");
    for wide in [false, true] {
      let entry_size = if wide { 32 } else { 20 };
      let start = 8 + 2 * entry_size;
      let mut bytes = vec![0; start + 64];
      bytes[..4].copy_from_slice(if wide { &[0xca, 0xfe, 0xba, 0xbf] } else { &[0xca, 0xfe, 0xba, 0xbe] });
      bytes[4..8].copy_from_slice(&2_u32.to_be_bytes());
      for (index, cpu) in [0x0100_0007_u32, 0x0100_000c].iter().enumerate() {
        let entry = 8 + index * entry_size;
        bytes[entry..entry + 4].copy_from_slice(&cpu.to_be_bytes());
        let offset = start + index * 32;
        if wide {
          bytes[entry + 8..entry + 16].copy_from_slice(&(offset as u64).to_be_bytes());
          bytes[entry + 16..entry + 24].copy_from_slice(&32_u64.to_be_bytes());
        } else {
          bytes[entry + 8..entry + 12].copy_from_slice(&(offset as u32).to_be_bytes());
          bytes[entry + 12..entry + 16].copy_from_slice(&32_u32.to_be_bytes());
        }
      }
      bytes[start..start + 32].copy_from_slice(&test_binary(&x64));
      bytes[start + 32..].copy_from_slice(&test_binary(&arm64));
      fs::write(&binary, bytes).unwrap();
      x64.verify_binary(&binary).unwrap();
      arm64.verify_binary(&binary).unwrap();
    }
    fs::remove_dir_all(root).unwrap();
  }

  #[test]
  fn cache_identity_requires_actual_headers_and_consistent_version_evidence() {
    let root = temporary("cache-identity");
    let expected = target("x86_64-unknown-linux-gnu");
    let dir = root.join("linux-x64/45.0.0");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("electron"), test_binary(&expected)).unwrap();
    assert!(verified_dist(&dir, "45.0.0", &expected).is_none());
    fs::write(dir.join("version"), "v45.0.0\n").unwrap();
    assert_eq!(verified_dist(&dir, "45.0.0", &expected), Some(dir.clone()));
    fs::write(dir.join(".electron-rs-version"), "44.0.0").unwrap();
    assert!(verified_dist(&dir, "45.0.0", &expected).is_none());
    fs::remove_file(dir.join(".electron-rs-version")).unwrap();
    fs::write(dir.join("electron"), test_binary(&target("aarch64-unknown-linux-gnu"))).unwrap();
    assert!(verified_dist(&dir, "45.0.0", &expected).is_none());
    fs::write(dir.join("electron"), b"unknown binary").unwrap();
    assert!(verified_dist(&dir, "45.0.0", &expected).is_none());
    fs::remove_dir_all(root).unwrap();
  }

  #[test]
  fn output_uses_consuming_out_dir_for_native_cross_and_custom_profiles() {
    let root = temporary("out-dir");
    let target = target("aarch64-unknown-linux-gnu");
    for suffix in ["debug/build/pkg/out", "aarch64-unknown-linux-gnu/release/build/pkg/out", "aarch64-unknown-linux-gnu/custom/build/pkg/out"] {
      let output = resolve_output(None, &target, Some(&root.join(suffix)), Some(&root.join("ignored")), &|| panic!("absolute Cargo inputs need no cwd")).unwrap();
      assert_eq!(output, root.join("electron/linux-arm64"));
    }
    fs::remove_dir_all(root).unwrap();
  }

  #[test]
  fn configured_target_root_named_like_triple_survives_native_and_cross_layouts() {
    let root = temporary("target-root-name");
    let target = target("x86_64-pc-windows-msvc");
    let configured = root.join(&target.triple);
    fs::create_dir_all(&configured).unwrap();
    for relative in [false, true] {
      let configured_input = if relative { Path::new(&target.triple) } else { &configured };
      for profile in [configured.join("debug"), configured.join(&target.triple).join("release")] {
        let out = profile.join("build/pkg/out");
        fs::create_dir_all(&out).unwrap();
        assert_eq!(resolve_output(None, &target, Some(&out), Some(configured_input), &|| Ok(root.clone())).unwrap(), configured.join("electron/win32-x64"));
      }
    }
    fs::remove_dir_all(root).unwrap();
  }

  #[test]
  fn explicit_output_wins_over_invalid_cargo_environment() {
    let root = temporary("explicit");
    let expected = target("x86_64-unknown-linux-gnu");
    let explicit = root.join("private-output");
    assert_eq!(resolve_output(Some(&explicit), &expected, Some(Path::new("invalid")), Some(Path::new("ignored")), &|| panic!("absolute output needs no cwd")).unwrap(), explicit);
    assert!(resolve_output(None, &expected, Some(Path::new("invalid")), None, &|| Ok(root.clone())).is_err());
    assert_eq!(resolve_output(Some(Path::new("relative")), &expected, Some(Path::new("invalid")), None, &|| Ok(root.clone())).unwrap(), root.join("relative"));
    fs::remove_dir_all(root).unwrap();
  }

  #[test]
  fn output_honors_explicit_target_directory_and_non_cargo_fallback() {
    let root = temporary("fallback");
    let expected = target("x86_64-unknown-linux-gnu");
    assert_eq!(resolve_output(None, &expected, None, Some(Path::new("build-target")), &|| Ok(root.clone())).unwrap(), root.join("build-target/electron/linux-x64"));
    assert_eq!(project_target_root(&root, None).unwrap(), root.join("target"));
    let error = resolve_output(None, &expected, None, None, &|| Err(io::Error::new(io::ErrorKind::NotFound, "cwd removed"))).unwrap_err();
    assert!(std::error::Error::source(&error).unwrap().is::<io::Error>());
    fs::remove_dir_all(root).unwrap();
  }

  #[test]
  fn cargo_metadata_resolves_workspace_member_and_external_target_config() {
    let root = temporary("workspace");
    let workspace = root.join("workspace");
    let member = workspace.join("member");
    fs::create_dir_all(member.join("src")).unwrap();
    fs::create_dir_all(workspace.join(".cargo")).unwrap();
    fs::write(workspace.join("Cargo.toml"), "[workspace]\nmembers = [\"member\"]\nresolver = \"2\"\n").unwrap();
    fs::write(member.join("Cargo.toml"), "[package]\nname = \"output-fixture\"\nversion = \"0.0.0\"\nedition = \"2021\"\n").unwrap();
    fs::write(member.join("src/lib.rs"), "").unwrap();
    fs::write(workspace.join(".cargo/config.toml"), "[build]\ntarget-dir = \"../external-target\"\n").unwrap();
    let expected = target("x86_64-unknown-linux-gnu");
    let output = resolve_output(None, &expected, None, None, &|| Ok(member.clone())).unwrap();
    fs::create_dir_all(&output).unwrap();
    assert_eq!(fs::canonicalize(output).unwrap(), fs::canonicalize(root.join("external-target/electron/linux-x64")).unwrap());
    fs::remove_dir_all(root).unwrap();
  }

  #[test]
  fn cargo_metadata_failure_is_reported_instead_of_guessing_target() {
    let root = temporary("invalid-project");
    fs::write(root.join("Cargo.toml"), "invalid manifest").unwrap();
    let error = resolve_output(None, &target("x86_64-unknown-linux-gnu"), None, None, &|| Ok(root.clone())).unwrap_err();
    assert!(error.to_string().contains("cargo metadata"));
    assert!(error.to_string().contains("failed"));
    fs::remove_dir_all(root).unwrap();
  }

  #[test]
  fn failed_publish_restores_the_previous_output() {
    let root = temporary("rollback");
    let output = root.join("output");
    fs::create_dir(&output).unwrap();
    fs::write(output.join("previous"), b"old").unwrap();
    let error = replace_directory(&root.join("missing-stage"), &output).unwrap_err();
    assert_eq!(fs::read(output.join("previous")).unwrap(), b"old");
    assert!(error.to_string().contains("previous output restored"));
    assert!(std::error::Error::source(&error).unwrap().is::<io::Error>());
    fs::remove_dir_all(root).unwrap();
  }

  #[test]
  fn cleanup_failure_is_success_after_publication_commits() {
    let root = temporary("cleanup");
    let output = root.join("output");
    let stage = root.join("stage");
    fs::create_dir(&output).unwrap();
    fs::write(output.join("previous"), b"old").unwrap();
    fs::create_dir(&stage).unwrap();
    fs::write(stage.join("new"), b"new").unwrap();
    let mut backup = None;
    let warning = replace_directory_with(&stage, &output, &mut |from, to| fs::rename(from, to), &mut |path| {
      backup = Some(path.to_path_buf());
      Err(io::Error::new(io::ErrorKind::PermissionDenied, "cleanup denied"))
    }).unwrap().unwrap();
    assert_eq!(fs::read(output.join("new")).unwrap(), b"new");
    assert_eq!(fs::read(backup.unwrap().join("previous")).unwrap(), b"old");
    assert!(warning.to_string().contains("new output"));
    assert!(warning.to_string().contains("is active"));
    assert!(warning.to_string().contains("backup left"));
    assert!(std::error::Error::source(&warning).unwrap().is::<io::Error>());
    fs::remove_dir_all(root).unwrap();
  }

  #[test]
  fn failed_rollback_names_the_surviving_backup_and_output_state() {
    let root = temporary("rollback-failure");
    let output = root.join("output");
    let stage = root.join("stage");
    fs::create_dir(&output).unwrap();
    fs::write(output.join("previous"), b"old").unwrap();
    fs::create_dir(&stage).unwrap();
    let mut backup = None;
    let error = replace_directory_with(&stage, &output, &mut |from, to| {
      if from == output {
        backup = Some(to.to_path_buf());
        fs::rename(from, to)
      } else {
        Err(io::Error::new(io::ErrorKind::PermissionDenied, "rename denied"))
      }
    }, &mut |_| panic!("no cleanup before commit")).unwrap_err();
    let backup = backup.unwrap();
    assert_eq!(fs::read(backup.join("previous")).unwrap(), b"old");
    assert!(!output.exists());
    assert!(error.to_string().contains(&backup.display().to_string()));
    assert!(error.to_string().contains("current output is absent"));
    fs::remove_dir_all(root).unwrap();
  }
}
