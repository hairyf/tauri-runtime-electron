//! Build-time Electron acquisition and branding, plus runtime process/RPC glue.
//!
//! [`ElectronBuilder`] owns the bundled JavaScript assets. Applications only
//! launch the prepared [`Electron`]; launching never downloads or patches files.

mod acquisition;
mod command;
mod error;
mod ipc;
mod metadata;
#[cfg(windows)]
mod pe;

use std::{fs, io, path::{Path, PathBuf}, process::{Command as ProcessCommand, Output}};
use acquisition::DistributionTarget;
pub use command::{Command, CommandChunk, CommandOutput, OutputStream};
pub use ipc::{Electron, Event, RpcRequest};

pub use error::{ElectronError, Error};

/// The output published by one successful build, with paths resolved once.
#[derive(Debug)]
pub struct PreparedDistribution {
  /// Absolute application-owned distribution directory.
  pub directory: PathBuf,
  /// Absolute prepared executable path within `directory`.
  pub binary: PathBuf,
  /// Publication succeeded, but its previous backup could not be removed.
  /// The diagnostic names the backup left for later cleanup.
  pub cleanup_warning: Option<Error>,
}

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
  /// Set the display name applied to the private distribution.
  pub fn app_name(mut self, name: impl Into<String>) -> Self { self.metadata.app_name = Some(name.into()); self }
  /// Set the application identifier used by desktop integration.
  pub fn identifier(mut self, identifier: impl Into<String>) -> Self { self.metadata.identifier = Some(identifier.into()); self }
  /// Set a Windows `.ico` or macOS `.icns` application icon.
  pub fn icon(mut self, path: impl Into<PathBuf>) -> Self { self.metadata.icon = Some(path.into()); self }
  /// Reuse Windows icon/version resources from this executable.
  pub fn host_executable(mut self, path: impl Into<PathBuf>) -> Self { self.metadata.host_executable = Some(path.into()); self }
  /// Replace all branding settings; individual fluent setters can override them.
  pub fn metadata(mut self, metadata: Metadata) -> Self { self.metadata = metadata; self }

  /// Prepare and publish a private distribution and its embedded scripts.
  ///
  /// Call only at build time. Every change, including script repairs, is made in
  /// a sibling staging directory. Preparation errors leave the old output intact.
  /// Publication errors restore it when possible; a failed restore names its backup.
  /// Once publication commits, backup cleanup failure is a successful result with
  /// [`PreparedDistribution::cleanup_warning`]. One writer owns each output.
  pub fn build(&self) -> Result<PreparedDistribution, Error> {
    validate_version(&self.version)?;
    let target = DistributionTarget::current()?;
    metadata::validate_for(&self.metadata, &target)?;
    let output = acquisition::output_directory(self.output.as_deref(), &target)?;
    let parent = output.parent().ok_or("Electron output has no parent")?;
    fs::create_dir_all(parent).map_err(|error| Error::with_source(format!("create {}", parent.display()), error))?;
    let canonical_output = fs::canonicalize(parent)
      .map_err(|error| Error::with_source(format!("resolve {}", parent.display()), error))?
      .join(output.file_name().ok_or("Electron output has no directory name")?);
    let existing = compatible_distribution(&output, &self.version, &target).is_ok();
    let reusable = existing && metadata::matches(&output, &self.metadata, &target, &self.version)?;
    if reusable && assets_match(&output, &target) {
      let binary = metadata::binary_for(&output, None, &target)?;
      return Ok(PreparedDistribution { directory: output, binary, cleanup_warning: None });
    }

    let source = if reusable {
      output.clone()
    } else {
      let mut candidates = self.paths.iter().map(|path| self.expand_resources(path)).collect::<Result<Vec<_>, _>>()?;
      let cwd = std::env::current_dir().map_err(|error| Error::with_source("resolve current directory", error))?;
      if let Some(path) = acquisition::local_install(&cwd, &target) { candidates.push(path); }
      // A verified, unbranded output can seed its first metadata application.
      if existing && !output.join("electron-rs.json").exists() { candidates.push(output.clone()); }
      if let Some(path) = acquisition::cached_dist(&self.version, &target) { candidates.push(path); }
      let mut source = None;
      for path in candidates {
        let Ok(dir) = compatible_distribution(&path, &self.version, &target) else { continue };
        if !dir.join("electron-rs.json").exists() || metadata::matches(&dir, &self.metadata, &target, &self.version)? {
          source = Some(dir);
          break;
        }
      }
      match source { Some(source) => source, None => acquisition::download(&self.version, &target)? }
    };
    let source = fs::canonicalize(&source)
      .map_err(|error| Error::with_source(format!("resolve {}", source.display()), error))?;
    if source != canonical_output && (source.starts_with(&canonical_output) || canonical_output.starts_with(&source)) {
      return Err("Electron source and output must not contain each other".into());
    }

    // ponytail: one builder per output; use separate outputs for concurrent builds.
    let stage = acquisition::unique_sibling(&output, "stage");
    fs::create_dir(&stage).map_err(|error| Error::with_source(format!("create {}", stage.display()), error))?;
    let result = (|| {
      acquisition::copy_dir(&source, &stage)?;
      write_assets(&stage)?;
      compile_native_assets(&stage, &target)?;
      metadata::apply(&stage, &self.metadata, &target, &self.version)?;
      let version_path = stage.join(".electron-rs-version");
      fs::write(&version_path, &self.version)
        .map_err(|error| Error::with_source(format!("write {}", version_path.display()), error))?;
      let staged_binary = metadata::binary_for(&stage, self.metadata.app_name.as_deref(), &target)?;
      let relative = staged_binary.strip_prefix(&stage)
        .map_err(|error| Error::with_source("prepared binary is outside staging", error))?;
      let binary = output.join(relative);
      // No fallible preparation remains after the directory rename commits.
      let cleanup_warning = acquisition::replace_directory(&stage, &output)?;
      Ok(PreparedDistribution { directory: output, binary, cleanup_warning })
    })();
    if result.is_err() {
      if let Err(error) = fs::remove_dir_all(&stage) {
        if error.kind() != io::ErrorKind::NotFound { eprintln!("[electron] clean staging {}: {error}", stage.display()); }
      }
    }
    result
  }

  /// Resolve the prepared binary without downloading or changing files.
  /// Prefer the paths returned by `build()` for the same build invocation.
  pub fn binary(&self) -> Result<PathBuf, Error> {
    let target = DistributionTarget::current()?;
    let directory = acquisition::output_directory(self.output.as_deref(), &target)?;
    metadata::binary_for(&directory, self.metadata.app_name.as_deref(), &target)
  }

  /// Resolve the output root in the current Cargo context without writing.
  /// Explicit output wins; outside Cargo/project context the default is `cwd/target`.
  pub fn directory(&self) -> Result<PathBuf, Error> {
    acquisition::output_directory(self.output.as_deref(), &DistributionTarget::current()?)
  }

  /// Run the prepared binary in Node mode without starting the RPC bridge.
  /// Nonzero exits are `Ok(Output)`; inspect `status`, `stdout` and `stderr`.
  pub fn node(&self, args: &[&str]) -> io::Result<Output> {
    let binary = self.binary().map_err(io::Error::other)?;
    node(&binary, args)
  }

  /// Run the prepared binary in Electron mode without starting the RPC bridge.
  /// Clears `ELECTRON_RUN_AS_NODE`; nonzero exits return `Ok(Output)`.
  pub fn electron(&self, args: &[&str]) -> io::Result<Output> {
    let binary = self.binary().map_err(io::Error::other)?;
    electron(&binary, args)
  }

  fn expand_resources(&self, path: &Path) -> Result<PathBuf, Error> {
    if let Some(rest) = path.to_string_lossy().strip_prefix("{resources}") {
      let root = self.resources.as_ref().ok_or("{resources} search path requires resources_dir")?;
      return Ok(root.join(rest.trim_start_matches(['/', '\\'])));
    }
    Ok(path.to_path_buf())
  }
}

/// Locate a compatible prepared binary or a stock distribution without writing.
///
/// Accepts an executable, a distribution root, or a resource root containing
/// `<platform>-<arch>`. Executable headers establish OS/architecture compatibility;
/// this function never downloads, launches, or repairs files.
pub fn find_binary(path: impl AsRef<Path>) -> Result<PathBuf, Error> {
  find_binary_for(path.as_ref(), &DistributionTarget::current()?)
}

/// Run an Electron binary in Node mode, without starting the RPC bridge.
/// Blocks until exit; nonzero exits return `Ok(Output)` with both output streams.
pub fn node(binary: impl AsRef<Path>, args: &[&str]) -> io::Result<Output> {
  run_command(binary.as_ref(), args, true)
}

/// Run an Electron binary normally, explicitly clearing `ELECTRON_RUN_AS_NODE`.
/// Blocks until exit; nonzero exits return `Ok(Output)` with both output streams.
pub fn electron(binary: impl AsRef<Path>, args: &[&str]) -> io::Result<Output> {
  run_command(binary.as_ref(), args, false)
}

fn find_binary_for(path: &Path, target: &DistributionTarget) -> Result<PathBuf, Error> {
  if path.is_file() {
    target.verify_binary(path)?;
    return Ok(path.to_path_buf());
  }
  if path.join("electron-rs.json").exists() || path.join(target.executable()).exists() {
    return metadata::binary_for(path, None, target);
  }
  let platform_dir = path.join(format!("{}-{}", target.platform, target.arch));
  metadata::binary_for(&platform_dir, None, target).map_err(|error| error.context(format!(
    "Electron binary not found in {}; bundle target/electron as runtime (runtime downloads are disabled)", path.display()
  )))
}

fn compatible_distribution(path: &Path, version: &str, target: &DistributionTarget) -> Result<PathBuf, Error> {
  let binary = find_binary_for(path, target)?;
  let root = if target.platform == "darwin" {
    binary.parent().and_then(Path::parent).and_then(Path::parent).and_then(Path::parent)
  } else {
    binary.parent()
  }.ok_or_else(|| Error::from(format!("invalid Electron distribution path: {}", binary.display())))?;
  if !acquisition::version_matches(root, version) {
    return Err(format!("Electron {} has no consistent version evidence for {version}", root.display()).into());
  }
  Ok(root.to_path_buf())
}

fn validate_version(version: &str) -> Result<(), Error> {
  if version.len() > 100 || !version.starts_with(|ch: char| ch.is_ascii_digit())
    || !version.chars().all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-'))
  {
    return Err(format!("invalid Electron version: {version:?}").into());
  }
  Ok(())
}

// Generated at Cargo build time from the complete tsdown output tree.
include!(concat!(env!("OUT_DIR"), "/assets.rs"));

fn assets_match(dir: &Path, target: &DistributionTarget) -> bool {
  (target.platform != "darwin" || fs::symlink_metadata(dir.join("appkit.node")).is_ok_and(|metadata| metadata.is_file()))
    && ASSETS.iter().all(|(name, bytes)| {
    let path = dir.join(name);
    fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.is_file())
      && fs::read(path).is_ok_and(|actual| actual == *bytes)
  })
}

fn write_assets(dir: &Path) -> Result<(), Error> {
  for (name, bytes) in ASSETS {
    let path = dir.join(name);
    if let Some(parent) = path.parent() {
      let relative = parent.strip_prefix(dir).map_err(|error| Error::with_source("asset outside distribution", error))?;
      let mut directory = dir.to_path_buf();
      for component in relative.components() {
        directory.push(component);
        if fs::symlink_metadata(&directory).is_ok_and(|metadata| metadata.is_symlink()) {
          fs::remove_file(&directory).map_err(|error| Error::with_source(format!("remove asset directory link {}", directory.display()), error))?;
        }
        fs::create_dir_all(&directory).map_err(|error| Error::with_source(format!("create {}", directory.display()), error))?;
      }
    }
    if fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.is_symlink()) {
      fs::remove_file(&path).map_err(|error| Error::with_source(format!("remove script link {}", path.display()), error))?;
    }
    if fs::read(&path).ok().as_deref() != Some(*bytes) {
      fs::write(&path, bytes).map_err(|error| Error::with_source(format!("write {}", path.display()), error))?;
    }
  }
  Ok(())
}

fn compile_native_assets(dir: &Path, target: &DistributionTarget) -> Result<(), Error> {
  if target.platform != "darwin" { return Ok(()); }
  if !cfg!(target_os = "macos") {
    return Err("building the AppKit bridge requires a macOS host with Xcode Command Line Tools".into());
  }
  let binary = dir.join("appkit.node");
  if fs::symlink_metadata(&binary).is_ok_and(|metadata| metadata.is_symlink()) {
    fs::remove_file(&binary).map_err(|error| Error::with_source("remove AppKit bridge link", error))?;
  }
  let arch = if target.arch == "arm64" { "arm64" } else { "x86_64" };
  let output = ProcessCommand::new("clang++")
    .args(["-std=c++17", "-fobjc-arc", "-bundle", "-undefined", "dynamic_lookup", "-framework", "AppKit", "-arch", arch, "-mmacosx-version-min=12.0"])
    .arg(dir.join("appkit.mm")).arg("-o").arg(&binary)
    .output().map_err(|error| Error::with_source("start AppKit bridge compiler", error))?;
  if !output.status.success() {
    return Err(format!("AppKit bridge compilation failed ({}): {}", output.status, String::from_utf8_lossy(&output.stderr)).into());
  }
  let output = ProcessCommand::new("codesign").args(["--force", "--sign", "-"]).arg(&binary)
    .output().map_err(|error| Error::with_source("start AppKit bridge signing", error))?;
  if !output.status.success() {
    return Err(format!("AppKit bridge signing failed ({}): {}", output.status, String::from_utf8_lossy(&output.stderr)).into());
  }
  Ok(())
}

fn run_command(binary: &Path, args: &[&str], node: bool) -> io::Result<Output> {
  let mut command = ProcessCommand::new(binary);
  command.args(args);
  if node {
    command.env("ELECTRON_RUN_AS_NODE", "1");
  } else {
    command.env_remove("ELECTRON_RUN_AS_NODE");
  }
  #[cfg(windows)]
  {
    use std::os::windows::process::CommandExt;
    command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
  }
  command.output()
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn builder_refreshes_assets_and_resolves_the_existing_resource_mapping() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/electron-builder");
    let root = acquisition::unique_sibling(&root, "test");
    let source = root.join("source");
    let output = root.join("runtime").join(format!("{}-{}", platform(), arch()));
    let target = DistributionTarget::current().unwrap();
    let binary = source.join(target.executable());
    fs::create_dir_all(binary.parent().unwrap())
      .unwrap_or_else(|error| panic!("create fixture {}: {error}", binary.display()));
    fs::write(&binary, acquisition::test_binary(&target)).unwrap();
    fs::write(source.join("version"), DEFAULT_VERSION).unwrap();
    let builder = ElectronBuilder::new(DEFAULT_VERSION).path(&source).output(&output);
    let prepared = builder.build().unwrap();
    assert_eq!(prepared.directory, output);
    assert_eq!(prepared.binary, output.join(target.executable()));
    assert!(prepared.cleanup_warning.is_none());
    let marker: serde_json::Value = serde_json::from_slice(&fs::read(output.join("electron-rs.json")).unwrap()).unwrap();
    assert_eq!(marker["target"], target.platform);
    assert_eq!(marker["arch"], target.arch);
    assert_eq!(marker["electronVersion"], DEFAULT_VERSION);
    fs::write(output.join("main.cjs"), b"stale").unwrap();
    fs::remove_file(output.join("preload.cjs")).unwrap();
    fs::write(output.join("runtime/commands.cjs"), b"stale commands").unwrap();
    fs::remove_file(output.join("socket/ipc.cjs")).unwrap();
    if target.platform == "darwin" {
      assert!(output.join("appkit.node").is_file());
      fs::remove_file(output.join("appkit.node")).unwrap();
    }
    builder.build().unwrap();
    assert!(assets_match(&output, &target));
    for (name, bytes) in ASSETS {
      assert_eq!(fs::read(output.join(name)).unwrap(), *bytes, "staged asset {name}");
    }
    assert_eq!(find_binary(root.join("runtime")).unwrap(), builder.binary().unwrap());
    assert!(find_binary(root.join("missing")).is_err());
    assert!(validate_version("../escape").is_err());
    fs::remove_dir_all(root).unwrap();
  }

  #[test]
  fn failed_asset_repair_leaves_the_old_distribution_unchanged() {
    let root = acquisition::unique_sibling(&Path::new(env!("CARGO_MANIFEST_DIR")).join("target/electron-repair"), "test");
    let target = DistributionTarget::current().unwrap();
    let source = root.join("source");
    let binary = source.join(target.executable());
    fs::create_dir_all(binary.parent().unwrap()).unwrap();
    fs::write(&binary, acquisition::test_binary(&target)).unwrap();
    fs::write(source.join("version"), DEFAULT_VERSION).unwrap();
    let output = root.join("output");
    let builder = ElectronBuilder::new(DEFAULT_VERSION).path(&source).output(&output);
    builder.build().unwrap();
    fs::write(output.join("main.cjs"), b"old script").unwrap();
    fs::remove_file(output.join("preload.cjs")).unwrap();
    // A directory in place of an asset produces an ordinary preparation I/O error.
    fs::create_dir(output.join("preload.cjs")).unwrap();
    let marker = fs::read(output.join("electron-rs.json")).unwrap();
    let binary = fs::read(output.join(target.executable())).unwrap();
    let error = builder.build().unwrap_err();
    assert!(std::error::Error::source(&error).unwrap().is::<io::Error>());
    assert_eq!(fs::read(output.join("main.cjs")).unwrap(), b"old script");
    assert!(output.join("preload.cjs").is_dir());
    assert_eq!(fs::read(output.join("electron-rs.json")).unwrap(), marker);
    assert_eq!(fs::read(output.join(target.executable())).unwrap(), binary);
    fs::remove_dir_all(root).unwrap();
  }

  #[test]
  fn distribution_identity_rejects_unknown_and_mismatched_sources() {
    let root = acquisition::unique_sibling(&Path::new(env!("CARGO_MANIFEST_DIR")).join("target/electron-identity"), "test");
    let target = DistributionTarget::from_triple("x86_64-unknown-linux-gnu").unwrap();
    let other = DistributionTarget::from_triple("aarch64-unknown-linux-gnu").unwrap();
    fs::create_dir_all(&root).unwrap();
    let binary = root.join(target.executable());
    fs::write(&binary, acquisition::test_binary(&target)).unwrap();
    assert!(compatible_distribution(&root, DEFAULT_VERSION, &target).is_err());
    fs::write(root.join("version"), DEFAULT_VERSION).unwrap();
    assert_eq!(compatible_distribution(&root, DEFAULT_VERSION, &target).unwrap(), root);
    fs::write(&binary, acquisition::test_binary(&other)).unwrap();
    assert!(compatible_distribution(&root, DEFAULT_VERSION, &target).is_err());
    fs::write(&binary, acquisition::test_binary(&target)).unwrap();
    fs::write(root.join(".electron-rs-version"), "44.0.0").unwrap();
    assert!(compatible_distribution(&root, DEFAULT_VERSION, &target).is_err());
    fs::remove_dir_all(root).unwrap();
  }

  #[test]
  fn node_worker() {
    if std::env::args().any(|arg| arg == "tests::node_worker") {
      let mode = match std::env::var_os("ELECTRON_RUN_AS_NODE") {
        Some(value) if value == "1" => "node",
        None => "electron",
        value => panic!("unexpected Node mode: {value:?}"),
      };
      print!("{mode} stdout");
      eprint!("{mode} stderr");
      std::process::exit(23);
    }
  }

  #[test]
  fn node_preserves_process_status_and_both_output_streams() {
    let binary = std::env::current_exe().unwrap();
    let output = run_command(&binary, &["--exact", "tests::node_worker", "--nocapture"], true).unwrap();
    assert_eq!(output.status.code(), Some(23));
    assert!(String::from_utf8_lossy(&output.stdout).contains("node stdout"));
    assert!(String::from_utf8_lossy(&output.stderr).contains("node stderr"));
    let missing = acquisition::unique_sibling(&Path::new(env!("CARGO_MANIFEST_DIR")).join("target/missing-node"), "test");
    assert_eq!(run_command(&missing, &[], true).unwrap_err().kind(), io::ErrorKind::NotFound);
  }

  #[test]
  fn electron_clears_inherited_node_mode_and_preserves_output() {
    let binary = std::env::current_exe().unwrap();
    // Isolate the inherited variable instead of mutating parallel tests' environment.
    if std::env::var_os("ELECTRON_RS_COMMAND_TEST").is_none() {
      let output = ProcessCommand::new(&binary)
        .args(["--exact", "tests::electron_clears_inherited_node_mode_and_preserves_output", "--nocapture"])
        .env("ELECTRON_RS_COMMAND_TEST", "1")
        .env("ELECTRON_RUN_AS_NODE", "1").output().unwrap();
      assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
      return;
    }
    let output = run_command(&binary, &["--exact", "tests::node_worker", "--nocapture"], false).unwrap();
    assert_eq!(output.status.code(), Some(23));
    assert!(String::from_utf8_lossy(&output.stdout).contains("electron stdout"));
    assert!(String::from_utf8_lossy(&output.stderr).contains("electron stderr"));
    assert_eq!(std::env::var_os("ELECTRON_RUN_AS_NODE").as_deref(), Some(std::ffi::OsStr::new("1")));
    let missing = acquisition::unique_sibling(&Path::new(env!("CARGO_MANIFEST_DIR")).join("target/missing-electron"), "test");
    assert_eq!(run_command(&missing, &[], false).unwrap_err().kind(), io::ErrorKind::NotFound);
  }
}
