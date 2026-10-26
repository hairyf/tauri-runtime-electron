//! Build-time Electron acquisition and branding, plus runtime process/RPC glue.
//!
//! [`ElectronBuilder`] owns the bundled JavaScript assets. Applications only
//! launch the prepared [`Electron`]; launching never downloads or patches files.

mod acquisition;
mod ipc;
mod metadata;
#[cfg(windows)]
mod pe;

use std::{fs, path::{Path, PathBuf}, process::Command};
pub use ipc::{Electron, Event};

/// Default Electron release used by the Tauri runtime.
pub const DEFAULT_VERSION: &str = "45.0.0";

/// Target platform name used in Electron release archives.
pub fn platform() -> String { acquisition::platform() }
/// Target architecture name used in Electron release archives.
pub fn arch() -> String { acquisition::arch() }
/// Stock executable path relative to an Electron distribution.
pub fn executable() -> String { acquisition::executable() }

/// Identity applied to an application-owned copy, never the shared download.
#[derive(Debug, Clone, Default)]
pub struct Metadata {
  pub app_name: Option<String>,
  pub identifier: Option<String>,
  /// Windows `.ico` or macOS `.icns` file.
  pub icon: Option<PathBuf>,
  /// Optional Windows executable whose embedded icon/version can be reused.
  pub host_executable: Option<PathBuf>,
}

/// Finds/downloads and prepares Electron in a build script.
#[derive(Debug, Clone)]
pub struct ElectronBuilder {
  version: String,
  paths: Vec<PathBuf>,
  output: Option<PathBuf>,
  resources: Option<PathBuf>,
  metadata: Metadata,
}

impl ElectronBuilder {
  pub fn new(version: impl Into<String>) -> Self {
    Self { version: version.into(), paths: Vec::new(), output: None, resources: None, metadata: Metadata::default() }
  }

  /// Add an ordered search path, optionally beginning with `{resources}`.
  pub fn path(mut self, path: impl Into<PathBuf>) -> Self { self.paths.push(path.into()); self }
  /// Resolve `{resources}` search paths against this directory.
  pub fn resources_dir(mut self, path: impl Into<PathBuf>) -> Self { self.resources = Some(path.into()); self }
  /// Override `target/electron/<platform>-<arch>`.
  pub fn output(mut self, path: impl Into<PathBuf>) -> Self { self.output = Some(path.into()); self }
  pub fn metadata(mut self, metadata: Metadata) -> Self { self.metadata = metadata; self }

  /// Prepare a private distribution and its embedded `main.cjs`/`preload.cjs`.
  ///
  /// Call only at build time. Changed branding is reapplied from a clean source;
  /// failed preparation leaves the previous output intact.
  pub fn build(&self) -> Result<(), String> {
    validate_version(&self.version)?;
    metadata::validate(&self.metadata)?;
    let output = self.directory();
    let existing = metadata::binary(&output, None).is_ok();
    if existing && version_matches(&output, &self.version) && metadata::matches(&output, &self.metadata)? {
      return write_assets(&output);
    }

    let mut candidates = self.paths.iter().map(|path| self.expand_resources(path)).collect::<Result<Vec<_>, _>>()?;
    if let Ok(cwd) = std::env::current_dir() {
      if let Some(path) = acquisition::local_install(&cwd) { candidates.push(path); }
    }
    // An unbranded output is a usable source for the first metadata application.
    if existing && version_matches(&output, &self.version) && !output.join("electron-rs.json").exists() {
      candidates.push(output.clone());
    }
    if let Some(path) = acquisition::cached_dist(&self.version) { candidates.push(path); }
    let mut source = None;
    for path in candidates {
      let Ok(dir) = distribution_directory(&path) else { continue };
      if version_matches(&dir, &self.version)
        && (!dir.join("electron-rs.json").exists() || metadata::matches(&dir, &self.metadata)?)
      {
        source = Some(dir);
        break;
      }
    }
    let source = source.map(Ok).unwrap_or_else(|| acquisition::download(&self.version))?;
    let source = fs::canonicalize(&source).map_err(|error| format!("{}: {error}", source.display()))?;
    let parent = output.parent().ok_or("Electron output has no parent")?;
    fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
    let output = fs::canonicalize(parent).map_err(|error| error.to_string())?
      .join(output.file_name().ok_or("Electron output has no directory name")?);
    if source != output && (source.starts_with(&output) || output.starts_with(&source)) {
      return Err("Electron source and output must not contain each other".into());
    }

    // ponytail: one builder per output; use separate outputs for concurrent builds.
    let stage = acquisition::unique_sibling(&output, "stage");
    fs::create_dir(&stage).map_err(|error| format!("{}: {error}", stage.display()))?;
    let result = (|| {
      acquisition::copy_dir(&source, &stage)?;
      write_assets(&stage)?;
      metadata::apply(&stage, &self.metadata)?;
      fs::write(stage.join(".electron-rs-version"), &self.version).map_err(|error| error.to_string())?;
      acquisition::replace_directory(&stage, &output)
    })();
    if result.is_err() { let _ = fs::remove_dir_all(&stage); }
    result
  }

  /// Resolve the prepared binary without downloading or changing files.
  pub fn binary(&self) -> Result<PathBuf, String> {
    metadata::binary(&self.directory(), self.metadata.app_name.as_deref())
  }

  /// Output root, also containing the crate-owned main/preload scripts.
  pub fn directory(&self) -> PathBuf {
    let path = self.output.clone().unwrap_or_else(acquisition::default_output);
    if path.is_absolute() { path } else { std::env::current_dir().unwrap_or_default().join(path) }
  }

  /// Execute a prepared binary in Node mode, without starting the RPC bridge.
  pub fn node(&self, args: &[&str]) -> Result<std::process::Output, String> {
    let output = Command::new(self.binary()?).args(args).env("ELECTRON_RUN_AS_NODE", "1")
      .output().map_err(|error| error.to_string())?;
    if output.status.success() { Ok(output) } else { Err(String::from_utf8_lossy(&output.stderr).into_owned()) }
  }

  fn expand_resources(&self, path: &Path) -> Result<PathBuf, String> {
    if let Some(rest) = path.to_string_lossy().strip_prefix("{resources}") {
      let root = self.resources.as_ref().ok_or("{resources} search path requires resources_dir")?;
      return Ok(root.join(rest.trim_start_matches(['/', '\\'])));
    }
    Ok(path.to_path_buf())
  }
}

/// Locate a prepared (possibly renamed) binary or a stock distribution.
///
/// Accepts an executable, a distribution root, or the bundled `runtime` root
/// containing `<platform>-<arch>`. This function is read-only and never downloads.
pub fn find_binary(path: impl AsRef<Path>) -> Result<PathBuf, String> {
  let path = path.as_ref();
  if path.is_file() { return Ok(path.to_path_buf()); }
  if path.join("electron-rs.json").is_file() {
    return metadata::binary(path, None);
  }
  if let Ok(binary) = metadata::binary(path, None) { return Ok(binary); }
  let platform_dir = path.join(format!("{}-{}", platform(), arch()));
  metadata::binary(&platform_dir, None)
    .map_err(|_| format!("Electron binary not found in {}; bundle target/electron as runtime (runtime downloads are disabled)", path.display()))
}

fn distribution_directory(path: &Path) -> Result<PathBuf, String> {
  let binary = find_binary(path)?;
  let root = if platform() == "darwin" {
    binary.parent().and_then(Path::parent).and_then(Path::parent).and_then(Path::parent)
  } else {
    binary.parent()
  };
  root.map(Path::to_path_buf).ok_or_else(|| format!("invalid Electron distribution path: {}", binary.display()))
}

fn validate_version(version: &str) -> Result<(), String> {
  if version.len() > 100 || !version.starts_with(|ch: char| ch.is_ascii_digit())
    || !version.chars().all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-'))
  {
    return Err(format!("invalid Electron version: {version:?}"));
  }
  Ok(())
}

fn version_matches(dir: &Path, version: &str) -> bool {
  ["version", ".electron-rs-version"].iter()
    .find_map(|name| fs::read_to_string(dir.join(name)).ok())
    .is_none_or(|actual| actual.trim().trim_start_matches('v') == version)
}

fn write_assets(dir: &Path) -> Result<(), String> {
  for (name, bytes) in [("main.cjs", include_bytes!("../assets/main.cjs").as_slice()), ("preload.cjs", include_bytes!("../assets/preload.cjs").as_slice())] {
    let path = dir.join(name);
    if fs::read(&path).ok().as_deref() != Some(bytes) {
      fs::write(&path, bytes).map_err(|error| format!("{}: {error}", path.display()))?;
    }
  }
  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn builder_refreshes_assets_and_resolves_the_existing_resource_mapping() {
    let root = std::env::temp_dir().join(format!("electron-builder-{}", std::process::id()));
    let root = acquisition::unique_sibling(&root, "test");
    let source = root.join("source");
    let output = root.join("runtime").join(format!("{}-{}", platform(), arch()));
    let binary = source.join(executable());
    fs::create_dir_all(binary.parent().unwrap())
      .unwrap_or_else(|error| panic!("create fixture {}: {error}", binary.display()));
    fs::write(&binary, b"test executable").unwrap();
    fs::write(source.join("version"), DEFAULT_VERSION).unwrap();
    let builder = ElectronBuilder::new(DEFAULT_VERSION).path(&source).output(&output);
    builder.build().unwrap();
    fs::write(output.join("main.cjs"), b"stale").unwrap();
    fs::remove_file(output.join("preload.cjs")).unwrap();
    builder.build().unwrap();
    assert_eq!(fs::read(output.join("main.cjs")).unwrap(), include_bytes!("../assets/main.cjs"));
    assert!(output.join("preload.cjs").is_file());
    assert_eq!(find_binary(root.join("runtime")).unwrap(), builder.binary().unwrap());
    assert!(find_binary(root.join("missing")).is_err());
    assert!(validate_version("../escape").is_err());
    fs::remove_dir_all(root).unwrap();
  }
}
