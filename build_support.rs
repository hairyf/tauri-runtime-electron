//! Automatic discovery for the dependency build script. No application-specific knobs.

use std::{collections::BTreeMap, env, path::{Path, PathBuf}, process::Command};
use serde_json::Value;
use tauri_utils::{config::parse, platform::Target};

pub struct AppConfig {
  pub name: String,
  pub identifier: Option<String>,
  pub icon: Option<PathBuf>,
  pub inputs: Vec<PathBuf>,
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
