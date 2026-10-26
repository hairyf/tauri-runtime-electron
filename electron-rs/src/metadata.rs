use std::{
  collections::hash_map::DefaultHasher,
  ffi::OsStr,
  fs::{self, File},
  hash::{Hash, Hasher},
  io::{Read, Write},
  path::{Component, Path, PathBuf},
};

use crate::{acquisition, Metadata};

const MARKER: &str = "electron-rs.json";
const MARKER_VERSION: u32 = 1;

#[cfg(target_os = "macos")]
use std::process::Command;

/// Validate build-time metadata before it is used as a file or process name.
///
/// This is deliberately crate-private: the parent builder owns the public
/// `Metadata` type and calls this at its trust boundary.
pub(crate) fn validate(metadata: &Metadata) -> Result<(), String> {
  if let Some(name) = metadata.app_name.as_deref() {
    validate_name(name)?;
  }
  if let Some(identifier) = metadata.identifier.as_deref() {
    validate_identifier(identifier)?;
  }
  for (label, path) in [("icon", metadata.icon.as_deref()), ("host executable", metadata.host_executable.as_deref())] {
    if let Some(path) = path {
      let file = fs::metadata(path).map_err(|error| format!("{label} {}: {error}", path.display()))?;
      if !file.is_file() {
        return Err(format!("{label} is not a file: {}", path.display()));
      }
    }
  }

  let platform = acquisition::platform();
  if metadata.app_name.is_some() || metadata.identifier.is_some() || metadata.icon.is_some() || metadata.host_executable.is_some() {
    ensure_host_supports(&platform)?;
    if platform != "win32" && metadata.host_executable.is_some() {
      return Err(format!("host executable icon fallback is unsupported for target {platform}; supply an explicit icon"));
    }
    if platform != "win32" && platform != "darwin" && (metadata.identifier.is_some() || metadata.icon.is_some()) {
      return Err(format!("native identifier/icon branding is unsupported for target {platform}"));
    }
  }
  if let Some(icon) = metadata.icon.as_deref() {
    let extension = icon.extension().and_then(OsStr::to_str).unwrap_or_default();
    let expected = match platform.as_str() {
      "win32" => Some("ico"),
      "darwin" => Some("icns"),
      _ => None,
    };
    if let Some(expected) = expected {
      if !extension.eq_ignore_ascii_case(expected) {
        return Err(format!("{platform} icon must be a .{expected} file: {}", icon.display()));
      }
    }
    #[cfg(windows)]
    if platform == "win32" { crate::pe::validate_icon(icon)?; }
    if platform == "darwin" { validate_icns(icon)?; }
  }
  #[cfg(windows)]
  if metadata.icon.is_none() {
    if let Some(host) = metadata.host_executable.as_deref() { crate::pe::validate_host_icon(host)?; }
  }
  Ok(())
}

/// Return whether the staged distribution already carries exactly this
/// metadata. It never writes and treats a missing marker as unbranded.
pub(crate) fn matches(dist: &Path, metadata: &Metadata) -> Result<bool, String> {
  validate(metadata)?;
  let marker = dist.join(MARKER);
  if !marker.exists() {
    return Ok(metadata.app_name.is_none() && metadata.identifier.is_none() && metadata.icon.is_none() && metadata.host_executable.is_none());
  }
  let Ok(value) = read_marker(&marker) else { return Ok(false) };
  let Some(binary) = marker_binary(dist, &value).unwrap_or(None) else { return Ok(false) };
  let Some(expected) = value.get("fingerprint").and_then(serde_json::Value::as_str) else { return Ok(false) };
  let Some(target) = value.get("target").and_then(serde_json::Value::as_str) else { return Ok(false) };
  let Some(app_name) = value.get("appName").and_then(|name| if name.is_null() { Some(None) } else { name.as_str().map(Some) }) else { return Ok(false) };
  let Some(identifier) = value.get("identifier").and_then(|id| if id.is_null() { Some(None) } else { id.as_str().map(Some) }) else { return Ok(false) };
  Ok(target == acquisition::platform()
    && app_name == metadata.app_name.as_deref()
    && identifier == metadata.identifier.as_deref()
    && binary.is_file()
    && expected == fingerprint(metadata)?)
}

/// Apply build-time branding to a staged Electron distribution.
pub(crate) fn apply(dist: &Path, metadata: &Metadata) -> Result<(), String> {
  validate(metadata)?;
  let has_branding = metadata.app_name.is_some()
    || metadata.identifier.is_some()
    || metadata.icon.is_some()
    || metadata.host_executable.is_some();
  let marker = dist.join(MARKER);
  if !has_branding {
    if marker.exists() {
      fs::remove_file(&marker).map_err(|error| format!("remove {}: {error}", marker.display()))?;
    }
    return Ok(());
  }
  if matches(dist, metadata)? {
    return Ok(());
  }
  validate_dist(dist)?;
  // Do not leave an old path for binary() to trust if a changed build fails.
  if marker.exists() {
    fs::remove_file(&marker).map_err(|error| format!("remove stale {}: {error}", marker.display()))?;
  }

  let platform = acquisition::platform();
  ensure_host_supports(&platform)?;
  let target = match platform.as_str() {
    "win32" => apply_windows(dist, metadata)?,
    "darwin" => apply_macos(dist, metadata)?,
    _ => apply_other(dist, metadata)?,
  };
  let relative = relative_marker_path(dist, &target)?;
  let marker_value = serde_json::json!({
    "version": MARKER_VERSION,
    "target": platform,
    "appName": metadata.app_name,
    "identifier": metadata.identifier,
    "binary": relative,
    "fingerprint": fingerprint(metadata)?,
  });
  let temporary = marker.with_extension("json.tmp");
  let bytes = serde_json::to_vec(&marker_value).map_err(|error| error.to_string())?;
  let mut file = File::create(&temporary).map_err(|error| format!("{}: {error}", temporary.display()))?;
  file.write_all(&bytes).map_err(|error| error.to_string())?;
  file.sync_all().map_err(|error| error.to_string())?;
  drop(file);
  fs::rename(&temporary, &marker).map_err(|error| format!("{}: {error}", marker.display()))
}

/// Locate the staged binary without writing. A valid marker wins even if the
/// caller did not provide an app name; otherwise the target's default path is
/// used. Marker paths are strictly relative and canonicalized inside `dist`.
pub(crate) fn binary(dist: &Path, name: Option<&str>) -> Result<PathBuf, String> {
  if let Some(name) = name {
    validate_name(name)?;
  }
  let marker_path = dist.join(MARKER);
  if marker_path.is_file() {
    let value = read_marker(&marker_path)?;
    if let Some(path) = marker_binary(dist, &value)? {
      if path.is_file() {
        return Ok(path);
      }
    }
  }
  let platform = acquisition::platform();
  let path = match name {
    Some(name) => named_binary(dist, &platform, name),
    None => dist.join(acquisition::executable()),
  };
  if path.is_file() {
    Ok(path)
  } else {
    Err(format!("Electron binary not found: {}", path.display()))
  }
}

fn validate_dist(dist: &Path) -> Result<(), String> {
  let platform = acquisition::platform();
  let binary = match platform.as_str() {
    "win32" => dist.join("electron.exe"),
    "darwin" => dist.join("Electron.app/Contents/MacOS/Electron"),
    _ => dist.join("electron"),
  };
  if !binary.is_file() && !dist.join(MARKER).is_file() {
    return Err(format!("Electron distribution is missing {}", binary.display()));
  }
  Ok(())
}

#[cfg(windows)]
fn apply_windows(dist: &Path, metadata: &Metadata) -> Result<PathBuf, String> {
  let source = dist.join("electron.exe");
  let target = metadata
    .app_name
    .as_deref()
    .map(|name| dist.join(format!("{name}.exe")))
    .unwrap_or_else(|| source.clone());
  rename_if_needed(&source, &target)?;
  if metadata.icon.is_some() || metadata.host_executable.is_some() || metadata.app_name.is_some() {
    crate::pe::apply(&target, metadata.app_name.as_deref(), metadata.icon.as_deref(), metadata.host_executable.as_deref())?;
  }
  Ok(target)
}

#[cfg(not(windows))]
fn apply_windows(_dist: &Path, _metadata: &Metadata) -> Result<PathBuf, String> {
  Err("native Windows branding requires a Windows host".into())
}

fn apply_other(dist: &Path, metadata: &Metadata) -> Result<PathBuf, String> {
  let source = dist.join("electron");
  let target = metadata
    .app_name
    .as_deref()
    .map(|name| dist.join(name))
    .unwrap_or_else(|| source.clone());
  rename_if_needed(&source, &target)?;
  if metadata.identifier.is_some() || metadata.icon.is_some() || metadata.host_executable.is_some() {
    return Err(format!("metadata branding is unsupported for target platform {}", acquisition::platform()));
  }
  Ok(target)
}

#[cfg(target_os = "macos")]
fn apply_macos(dist: &Path, metadata: &Metadata) -> Result<PathBuf, String> {
  let source_app = dist.join("Electron.app");
  let app = metadata
    .app_name
    .as_deref()
    .map(|name| dist.join(format!("{name}.app")))
    .unwrap_or_else(|| source_app.clone());
  if source_app != app && app.exists() && !source_app.exists() {
    let plist = app.join("Contents/Info.plist");
    if !plist.is_file() || !metadata.app_name.as_deref().is_some_and(|name| plist_has_value(&plist, "CFBundleExecutable", name)) {
      return Err(format!("refusing to brand existing bundle without matching metadata: {}", app.display()));
    }
  }
  rename_if_needed(&source_app, &app)?;
  let source_binary = app.join("Contents/MacOS/Electron");
  let target_binary = metadata
    .app_name
    .as_deref()
    .map(|name| app.join("Contents/MacOS").join(name))
    .unwrap_or_else(|| source_binary.clone());
  rename_if_needed(&source_binary, &target_binary)?;
  if let Some(name) = metadata.app_name.as_deref() {
    rename_helpers(&app, name)?;
  }
  patch_plist(&app, metadata)?;
  if metadata.icon.is_some() || metadata.identifier.is_some() || metadata.app_name.is_some() {
    codesign(&app)?;
  }
  Ok(target_binary)
}

#[cfg(target_os = "macos")]
fn plist_has_value(plist: &Path, key: &str, expected: &str) -> bool {
  Command::new("/usr/libexec/PlistBuddy")
    .args(["-c", &format!("Print :{key}")])
    .arg(plist)
    .output()
    .is_ok_and(|output| output.status.success() && String::from_utf8_lossy(&output.stdout).trim() == expected)
}

#[cfg(not(target_os = "macos"))]
fn apply_macos(_dist: &Path, _metadata: &Metadata) -> Result<PathBuf, String> {
  Err("cannot brand a macOS Electron distribution on a non-macOS host".into())
}

fn ensure_host_supports(platform: &str) -> Result<(), String> {
  if platform == "win32" && !cfg!(windows) {
    return Err("cannot brand a Windows Electron distribution on a non-Windows host".into());
  }
  if platform == "darwin" && !cfg!(target_os = "macos") {
    return Err("cannot brand a macOS Electron distribution on a non-macOS host".into());
  }
  Ok(())
}

fn named_binary(dist: &Path, platform: &str, name: &str) -> PathBuf {
  match platform {
    "win32" => dist.join(format!("{name}.exe")),
    "darwin" => dist.join(format!("{name}.app/Contents/MacOS/{name}")),
    _ => dist.join(name),
  }
}

fn rename_if_needed(source: &Path, target: &Path) -> Result<(), String> {
  if source == target || !source.exists() {
    if target.exists() {
      return Ok(());
    }
    if source == target {
      return Err(format!("Electron binary not found: {}", source.display()));
    }
    return Err(format!("Electron binary not found: {}", source.display()));
  }
  if target.exists() {
    return Err(format!("refusing to overwrite {}", target.display()));
  }
  fs::rename(source, target).map_err(|error| format!("{} -> {}: {error}", source.display(), target.display()))
}

fn validate_name(name: &str) -> Result<(), String> {
  if name.is_empty() || name == "." || name == ".." {
    return Err("app_name must not be empty or a dot path".into());
  }
  if name.chars().any(|ch| ch.is_control() || "<>:\"/\\|?*".contains(ch)) {
    return Err("app_name contains an invalid filename character or control character".into());
  }
  if name.ends_with('.') || name.ends_with(' ') {
    return Err("app_name must not end with a dot or space".into());
  }
  // Leave room for the longest Electron helper suffix and the bundle extension.
  if name.encode_utf16().count() > 220 || name.len() > 220 {
    return Err("app_name is too long for an executable/helper filename".into());
  }
  let reserved = name.split('.').next().unwrap_or_default().trim_end_matches(' ').to_ascii_uppercase();
  if matches!(reserved.as_str(), "CON" | "PRN" | "AUX" | "NUL")
    || (reserved.len() == 4 && (reserved.starts_with("COM") || reserved.starts_with("LPT")) && reserved[3..].parse::<u8>().is_ok_and(|n| (1..=9).contains(&n)))
  {
    return Err(format!("app_name is a Windows reserved filename: {name}"));
  }
  Ok(())
}

fn validate_identifier(identifier: &str) -> Result<(), String> {
  if identifier.is_empty() || identifier == "." || identifier == ".." || identifier.len() > 255
    || identifier.split('.').any(|part| part.is_empty() || !part.bytes().all(|ch| ch.is_ascii_alphanumeric() || ch == b'-'))
  {
    return Err("identifier must contain non-empty ASCII alphanumeric/hyphen components separated by dots (at most 255 bytes)".into());
  }
  Ok(())
}

fn read_marker(path: &Path) -> Result<serde_json::Value, String> {
  let bytes = fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
  let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|error| format!("invalid {}: {error}", path.display()))?;
  if value.get("version").and_then(serde_json::Value::as_u64) != Some(MARKER_VERSION as u64) {
    return Err(format!("unsupported metadata marker: {}", path.display()));
  }
  Ok(value)
}

fn marker_binary(dist: &Path, value: &serde_json::Value) -> Result<Option<PathBuf>, String> {
  let Some(relative) = value.get("binary").and_then(serde_json::Value::as_str) else {
    return Err("metadata marker has no binary path".into());
  };
  let relative_path = Path::new(relative);
  if relative.is_empty() || relative.contains('\\') || relative.contains(':')
    || relative.chars().any(char::is_control)
    || relative.split('/').any(|part| part.is_empty() || part == "." || part == "..")
    || relative_path.is_absolute() || relative_path.components().any(|component| !matches!(component, Component::Normal(_)))
  {
    return Err(format!("metadata marker binary path is not a relative normal path: {relative}"));
  }
  if value.get("target").and_then(serde_json::Value::as_str) != Some(acquisition::platform().as_str()) {
    return Ok(None);
  }
  let candidate = dist.join(relative_path);
  if !candidate.exists() {
    return Ok(None);
  }
  if !candidate.is_file() {
    return Err(format!("metadata marker binary is not a file: {}", candidate.display()));
  }
  let root = fs::canonicalize(dist).map_err(|error| format!("{}: {error}", dist.display()))?;
  let canonical = fs::canonicalize(&candidate).map_err(|error| format!("{}: {error}", candidate.display()))?;
  if !canonical.starts_with(&root) {
    return Err(format!("metadata marker binary escapes distribution: {relative}"));
  }
  Ok(Some(candidate))
}

fn relative_marker_path(dist: &Path, binary: &Path) -> Result<String, String> {
  let relative = binary
    .strip_prefix(dist)
    .map_err(|_| format!("branded binary is outside distribution: {}", binary.display()))?;
  if relative.components().any(|component| !matches!(component, Component::Normal(_))) {
    return Err(format!("branded binary is not a normal relative path: {}", binary.display()));
  }
  let relative = relative.to_string_lossy().replace('\\', "/");
  if relative.is_empty() || relative.contains(':') || relative.chars().any(char::is_control)
    || relative.split('/').any(|part| part.is_empty() || part == "." || part == "..")
  {
    return Err(format!("branded binary is not a safe relative path: {}", binary.display()));
  }
  Ok(relative)
}

fn fingerprint(metadata: &Metadata) -> Result<String, String> {
  let mut hasher = DefaultHasher::new();
  acquisition::platform().hash(&mut hasher);
  metadata.app_name.hash(&mut hasher);
  metadata.identifier.hash(&mut hasher);
  for (tag, path) in [(b"icon".as_slice(), metadata.icon.as_deref()), (b"host".as_slice(), metadata.host_executable.as_deref())] {
    tag.hash(&mut hasher);
    if let Some(path) = path {
      path.to_string_lossy().hash(&mut hasher);
      hash_file(path, &mut hasher)?;
    }
  }
  Ok(format!("{:016x}", hasher.finish()))
}

fn hash_file(path: &Path, hasher: &mut DefaultHasher) -> Result<(), String> {
  let mut file = File::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
  let mut buffer = [0_u8; 64 * 1024];
  loop {
    let count = file.read(&mut buffer).map_err(|error| format!("{}: {error}", path.display()))?;
    if count == 0 {
      break;
    }
    buffer[..count].hash(hasher);
  }
  Ok(())
}

#[cfg(target_os = "macos")]
fn patch_plist(app: &Path, metadata: &Metadata) -> Result<(), String> {
  let plist = app.join("Contents/Info.plist");
  if !plist.is_file() {
    return Err(format!("missing macOS Info.plist: {}", plist.display()));
  }
  if let Some(name) = metadata.app_name.as_deref() {
    for key in ["CFBundleExecutable", "CFBundleName", "CFBundleDisplayName"] {
      plist_string(&plist, key, name)?;
    }
  }
  if let Some(identifier) = metadata.identifier.as_deref() {
    plist_string(&plist, "CFBundleIdentifier", identifier)?;
  }
  if let Some(icon) = metadata.icon.as_deref() {
    validate_icns(icon)?;
    let destination = app.join("Contents/Resources/app.icns");
    fs::copy(icon, &destination).map_err(|error| format!("{}: {error}", destination.display()))?;
    plist_string(&plist, "CFBundleIconFile", "app.icns")?;
  }
  Ok(())
}

#[cfg(target_os = "macos")]
fn plist_string(plist: &Path, key: &str, value: &str) -> Result<(), String> {
  let replace = Command::new("plutil").args(["-replace", key, "-string", value]).arg(plist).status();
  if replace.is_ok_and(|status| status.success()) {
    return Ok(());
  }
  let insert = Command::new("plutil").args(["-insert", key, "-string", value]).arg(plist).status();
  if insert.is_ok_and(|status| status.success()) {
    Ok(())
  } else {
    Err(format!("plutil could not set {key} in {}", plist.display()))
  }
}

fn validate_icns(path: &Path) -> Result<(), String> {
  let bytes = fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
  if bytes.len() < 16 || &bytes[..4] != b"icns" {
    return Err(format!("invalid ICNS icon: {}", path.display()));
  }
  let declared = u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize;
  if declared != bytes.len() {
    return Err(format!("invalid ICNS length: {}", path.display()));
  }
  let mut offset = 8;
  while offset < declared {
    let chunk = bytes.get(offset..offset + 8).ok_or("truncated ICNS chunk")?;
    let length = u32::from_be_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]) as usize;
    if length < 8 || length > declared - offset {
      return Err(format!("invalid ICNS chunk: {}", path.display()));
    }
    offset += length;
  }
  Ok(())
}

#[cfg(target_os = "macos")]
fn rename_helpers(app: &Path, name: &str) -> Result<(), String> {
  let frameworks = app.join("Contents/Frameworks");
  if !frameworks.is_dir() {
    return Ok(());
  }
  let entries = fs::read_dir(&frameworks).map_err(|error| format!("{}: {error}", frameworks.display()))?;
  for entry in entries {
    let entry = entry.map_err(|error| error.to_string())?;
    let old_path = entry.path();
    let old_file = entry.file_name();
    let old_file = old_file.to_string_lossy();
    let Some(old_base) = old_file.strip_suffix(".app") else { continue };
    let Some(suffix) = old_base.strip_prefix("Electron Helper") else { continue };
    if !old_path.is_dir() { continue; }
    let new_base = format!("{name} Helper{suffix}");
    let new_path = frameworks.join(format!("{new_base}.app"));
    if old_path != new_path {
      if new_path.exists() {
        return Err(format!("refusing to overwrite helper bundle: {}", new_path.display()));
      }
      fs::rename(&old_path, &new_path).map_err(|error| format!("{} -> {}: {error}", old_path.display(), new_path.display()))?;
    }
    let old_executable = new_path.join("Contents/MacOS").join(old_base);
    let new_executable = new_path.join("Contents/MacOS").join(&new_base);
    rename_if_needed(&old_executable, &new_executable)?;
    let plist = new_path.join("Contents/Info.plist");
    if plist.is_file() {
      for key in ["CFBundleExecutable", "CFBundleName", "CFBundleDisplayName"] {
        plist_string(&plist, key, &new_base)?;
      }
    }
  }
  Ok(())
}

#[cfg(target_os = "macos")]
fn codesign(app: &Path) -> Result<(), String> {
  let status = Command::new("codesign")
    .args(["--force", "--deep", "--sign", "-", "--preserve-metadata=entitlements,requirements"])
    .arg(app)
    .status();
  if status.is_ok_and(|status| status.success()) {
    Ok(())
  } else {
    Err(format!("codesign failed for {}; production builds still require Developer ID signing", app.display()))
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn marker_path_rejects_traversal() {
    let value = serde_json::json!({"version": MARKER_VERSION, "binary": "../outside"});
    let error = marker_binary(Path::new("dist"), &value).unwrap_err();
    assert!(error.contains("relative normal"));
  }

  #[test]
  fn marker_path_rejects_windows_traversal_on_every_host() {
    let value = serde_json::json!({"version": MARKER_VERSION, "target": acquisition::platform(), "binary": "..\\outside.exe"});
    assert!(marker_binary(Path::new("dist"), &value).unwrap_err().contains("relative normal"));
  }

  #[test]
  fn identifier_rejects_dot_paths_and_malformed_components() {
    assert!(validate_identifier(".").is_err());
    assert!(validate_identifier("..").is_err());
    assert!(validate_identifier("com..example").is_err());
    assert!(validate_identifier("com.example-app").is_ok());
  }

  #[test]
  fn name_validation_rejects_windows_reserved_names() {
    assert!(validate_name("CON").is_err());
    assert!(validate_name("good-name").is_ok());
  }
}
