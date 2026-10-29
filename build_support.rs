//! Automatic discovery for the dependency build script. No application-specific knobs.

use std::{collections::BTreeMap, env, path::{Path, PathBuf}, process::Command};
use serde_json::Value;
use tauri_utils::{config::parse, platform::Target};

/// One preparation at a time: Cargo runs the build script twice for one target (as a
/// build-dependency and as a dependency), and both brand the same output directory.
pub struct PreparationLock {
  path: PathBuf,
}

impl Drop for PreparationLock {
  fn drop(&mut self) {
    let _ = std::fs::remove_file(&self.path);
  }
}

pub fn lock_preparation(root: &Path) -> Result<PreparationLock, String> {
  std::fs::create_dir_all(root).map_err(|error| format!("create Electron output {}: {error}", root.display()))?;
  let path = root.join(".preparation.lock");
  let stale = std::time::Duration::from_secs(600);
  let deadline = std::time::Instant::now() + std::time::Duration::from_secs(900);
  loop {
    match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
      Ok(_) => return Ok(PreparationLock { path }),
      Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
        let aged = std::fs::metadata(&path).and_then(|meta| meta.modified()).ok()
          .and_then(|modified| modified.elapsed().ok())
          .is_some_and(|age| age > stale);
        if aged {
          let _ = std::fs::remove_file(&path);
          continue;
        }
        if std::time::Instant::now() >= deadline {
          return Err(format!("timed out waiting for {}", path.display()));
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
      }
      Err(error) => return Err(format!("create {}: {error}", path.display())),
    }
  }
}

pub struct AppConfig {
  pub name: String,
  pub identifier: Option<String>,
  pub icon: Option<PathBuf>,
  pub inputs: Vec<PathBuf>,
}

/// Keep only the Electron distribution prepared for the current `TARGET`.
///
/// The host-side build-dependency copy and the target-side dependency share one
/// output root. Bundling maps the whole `target/electron` directory, so a host
/// build left behind next to the target distribution ships both: a macOS x64 app
/// then carries `darwin-x64` and `darwin-arm64` and doubles to 945 MB.
pub fn prune_foreign_distributions(root: &Path, keep: &Path) -> Result<Vec<PathBuf>, String> {
  let entries = match std::fs::read_dir(root) {
    Ok(entries) => entries,
    Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
    Err(error) => return Err(format!("read Electron output {}: {error}", root.display())),
  };
  let mut removed = Vec::new();
  for entry in entries.flatten() {
    let path = entry.path();
    if path == keep || !path.is_dir() || !path.join("electron-rs.json").is_file() { continue; }
    std::fs::remove_dir_all(&path)
      .map_err(|error| format!("remove stale Electron distribution {}: {error}", path.display()))?;
    removed.push(path);
  }
  Ok(removed)
}

pub fn discover(target_dir: &Path, target: Target) -> Result<Option<AppConfig>, String> {
  let mut manifests = Vec::new();
  for seed in [env::var_os("INIT_CWD").map(PathBuf::from), env::var_os("PWD").map(PathBuf::from), target_dir.parent().map(Path::to_path_buf)].into_iter().flatten() {
    if !seed.is_absolute() { continue; }
    if let Some(manifest) = seed.ancestors().find_map(|dir| {
      [dir.join("src-tauri/Cargo.toml"), dir.join("Cargo.toml")].into_iter().find(|path| path.is_file())
    }) {
      let manifest = manifest.canonicalize().map_err(|error| error.to_string())?;
      if !manifests.contains(&manifest) { manifests.push(manifest); }
    }
  }
  for manifest in manifests {
    let output = Command::new(env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
      .args(["metadata", "--no-deps", "--format-version", "1", "--offline", "--manifest-path"])
      .arg(&manifest).output().map_err(|error| format!("cargo metadata: {error}"))?;
    if !output.status.success() {
      return Err(format!("cargo metadata {}: {}", manifest.display(), String::from_utf8_lossy(&output.stderr)));
    }
    let metadata: Value = serde_json::from_slice(&output.stdout).map_err(|error| error.to_string())?;
    let mut apps = BTreeMap::new();
    for package in metadata["packages"].as_array().ok_or("cargo metadata has no packages")? {
      let Some(path) = package["manifest_path"].as_str().map(Path::new) else { continue };
      let Some(root) = path.parent() else { continue };
      if parse::folder_has_configuration_file(target, root) {
        let name = package["name"].as_str().ok_or("Cargo package has no name")?;
        apps.insert(root.to_path_buf(), name.to_owned());
      }
    }
    if apps.len() > 1 {
      return Err(format!("cannot determine the Tauri application: multiple configs in {} ({})", manifest.display(), apps.keys().map(|path| path.display().to_string()).collect::<Vec<_>>().join(", ")));
    }
    if let Some((root, package_name)) = apps.into_iter().next() {
      let (mut config, mut inputs) = parse::read_from(target, &root).map_err(|error| error.to_string())?;
      if let Some(patch) = env::var_os("TAURI_CONFIG") {
        let patch: Value = serde_json::from_str(&patch.to_string_lossy()).map_err(|error| format!("TAURI_CONFIG: {error}"))?;
        json_patch::merge(&mut config, &patch);
      }
      inputs.push(root.join("Cargo.toml"));
      return identity(&root, &package_name, &config, target, inputs).map(Some);
    }
  }
  Ok(None)
}

fn identity(root: &Path, package_name: &str, config: &Value, target: Target, inputs: Vec<PathBuf>) -> Result<AppConfig, String> {
  let name = match config.get("productName") {
    None | Some(Value::Null) => package_name.to_owned(),
    Some(Value::String(name)) => name.clone(),
    _ => return Err("Tauri productName must be a string".into()),
  };
  let identifier = match config.get("identifier") {
    None | Some(Value::Null) => None,
    Some(Value::String(identifier)) => Some(identifier.clone()),
    _ => return Err("Tauri identifier must be a string".into()),
  };
  if name.is_empty() || name == "." || name == ".."
    || name.chars().any(|ch| ch.is_control() || "<>:\"/\\|?*".contains(ch))
    || name.ends_with(['.', ' '])
  {
    return Err("Tauri productName must be a valid executable filename".into());
  }
  if identifier.as_deref().is_some_and(|identifier| {
    identifier.is_empty() || identifier.len() > 255
      || identifier.split('.').any(|part| part.is_empty() || !part.bytes().all(|ch| ch.is_ascii_alphanumeric() || ch == b'-'))
  }) {
    return Err("Tauri identifier must contain non-empty ASCII alphanumeric/hyphen components separated by dots".into());
  }
  let extension = match target { Target::Windows => Some("ico"), Target::MacOS => Some("icns"), _ => None };
  let icon = extension.and_then(|extension| {
    config.pointer("/bundle/icon").and_then(Value::as_array).into_iter().flatten()
      .filter_map(Value::as_str).map(|path| root.join(path))
      .find(|path| path.extension().is_some_and(|value| value.eq_ignore_ascii_case(extension)))
      .or_else(|| {
        let default = root.join(format!("icons/icon.{extension}"));
        default.is_file().then_some(default)
      })
  });
  if let Some(icon) = &icon {
    if !icon.is_file() { return Err(format!("Tauri bundle icon not found: {}", icon.display())); }
  }
  Ok(AppConfig { name, identifier, icon, inputs })
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn foreign_distributions_are_removed_and_plain_files_survive() {
    let root = std::env::temp_dir().join(format!("runtime-electron-prune-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let keep = root.join("darwin-x64");
    let foreign = root.join("darwin-arm64");
    for directory in [&keep, &foreign] {
      std::fs::create_dir_all(directory).unwrap();
      std::fs::write(directory.join("electron-rs.json"), "{}").unwrap();
    }
    let plain = root.join("notes");
    std::fs::create_dir_all(&plain).unwrap();
    std::fs::write(root.join("README"), "keep").unwrap();

    let removed = prune_foreign_distributions(&root, &keep).unwrap();

    assert_eq!(removed, vec![foreign.clone()], "only the foreign distribution is reported");
    assert!(keep.join("electron-rs.json").is_file(), "the target distribution must survive");
    assert!(!foreign.exists(), "the host distribution must be removed");
    assert!(plain.is_dir(), "directories without a marker are not Electron distributions");
    assert!(root.join("README").is_file(), "plain files are untouched");
    std::fs::remove_dir_all(&root).ok();
  }

  #[test]
  fn missing_output_root_is_not_an_error() {
    let root = std::env::temp_dir().join(format!("runtime-electron-absent-{}", std::process::id()));
    let keep = root.join("darwin-arm64");
    assert_eq!(prune_foreign_distributions(&root, &keep).unwrap(), Vec::<PathBuf>::new());
  }

  #[test]
  fn merged_tauri_identity_uses_platform_icon_and_package_name_fallback() {
    let root = Path::new(".");
    let mut config = serde_json::json!({"productName":"Base App","identifier":"org.example.app","bundle":{"icon":["icon.png"]}});
    json_patch::merge(&mut config, &serde_json::json!({"productName":null,"bundle":{"icon":[]}}));
    let app = identity(root, "consumer-package", &config, Target::Linux, Vec::new()).unwrap();
    assert_eq!(app.name, "consumer-package");
    assert_eq!(app.identifier.as_deref(), Some("org.example.app"));
    assert!(app.icon.is_none());
    assert!(identity(root, "consumer", &serde_json::json!({"productName":123}), Target::Windows, Vec::new()).is_err());
    for name in ["../app", "bad\nname", ".", "app "] {
      assert!(identity(root, "consumer", &serde_json::json!({"productName":name}), Target::Linux, Vec::new()).is_err());
    }
    for identifier in ["..", "../app", "org..app", "org.app\n"] {
      assert!(identity(root, "consumer", &serde_json::json!({"identifier":identifier}), Target::Linux, Vec::new()).is_err());
    }
  }
}
