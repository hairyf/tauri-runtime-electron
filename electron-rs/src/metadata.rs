use std::{
  collections::hash_map::DefaultHasher,
  ffi::OsStr,
  fs::{self, File},
  hash::{Hash, Hasher},
  io::{Read, Write},
  path::{Component, Path, PathBuf},
};

use crate::{acquisition::DistributionTarget, Error, Metadata};

const MARKER: &str = "electron-rs.json";
const MARKER_VERSION: u32 = 2;

#[cfg(target_os = "macos")]
use std::process::Command;

/// Validate runtime identity fields using the current target.
pub(crate) fn validate(metadata: &Metadata) -> Result<(), Error> {
  validate_for(metadata, &DistributionTarget::current()?)
}

/// Validate build-time metadata before using it as a file or process name.
pub(crate) fn validate_for(metadata: &Metadata, target: &DistributionTarget) -> Result<(), Error> {
  if let Some(name) = metadata.app_name.as_deref() { validate_name(name)?; }
  if let Some(identifier) = metadata.identifier.as_deref() { validate_identifier(identifier)?; }
  for (label, path) in [("icon", metadata.icon.as_deref()), ("host executable", metadata.host_executable.as_deref())] {
    if let Some(path) = path {
      let file = fs::metadata(path).map_err(|error| Error::with_source(format!("inspect {label} {}", path.display()), error))?;
      if !file.is_file() { return Err(format!("{label} is not a file: {}", path.display()).into()); }
    }
  }
  let platform = target.platform;
  if has_branding(metadata) {
    ensure_host_supports(platform)?;
    if platform != "win32" && metadata.host_executable.is_some() {
      return Err(format!("host executable icon fallback is unsupported for target {platform}; supply an explicit icon").into());
    }
    if platform != "win32" && platform != "darwin" && (metadata.identifier.is_some() || metadata.icon.is_some()) {
      return Err(format!("native identifier/icon branding is unsupported for target {platform}").into());
    }
  }
  if let Some(icon) = metadata.icon.as_deref() {
    let extension = icon.extension().and_then(OsStr::to_str).unwrap_or_default();
    let expected = match platform { "win32" => Some("ico"), "darwin" => Some("icns"), _ => None };
    if let Some(expected) = expected {
      if !extension.eq_ignore_ascii_case(expected) {
        return Err(format!("{platform} icon must be a .{expected} file: {}", icon.display()).into());
      }
    }
    #[cfg(windows)]
    if platform == "win32" { crate::pe::validate_icon(icon)?; }
    if platform == "darwin" { validate_icns(icon)?; }
  }
  #[cfg(windows)]
  if platform == "win32" && metadata.icon.is_none() {
    if let Some(host) = metadata.host_executable.as_deref() { crate::pe::validate_host_icon(host)?; }
  }
  Ok(())
}

fn has_branding(metadata: &Metadata) -> bool {
  metadata.app_name.is_some() || metadata.identifier.is_some() || metadata.icon.is_some() || metadata.host_executable.is_some()
}

/// Compare branding and identity; executable headers remain authoritative.
pub(crate) fn matches(dist: &Path, metadata: &Metadata, target: &DistributionTarget, version: &str) -> Result<bool, Error> {
  let Ok(value) = read_marker(&dist.join(MARKER)) else { return Ok(false) };
  if value.get("version").and_then(serde_json::Value::as_u64) != Some(MARKER_VERSION as u64)
    || value.get("electronVersion").and_then(serde_json::Value::as_str) != Some(version)
  {
    return Ok(false);
  }
  if marker_binary(dist, &value, target).ok().flatten().is_none() { return Ok(false); }
  Ok(value.get("appName") == Some(&serde_json::json!(metadata.app_name))
    && value.get("identifier") == Some(&serde_json::json!(metadata.identifier))
    && value.get("fingerprint").and_then(serde_json::Value::as_str) == Some(fingerprint(metadata, target)?.as_str()))
}

/// Apply branding and persist verified identity only in the private stage.
pub(crate) fn apply(dist: &Path, metadata: &Metadata, target: &DistributionTarget, version: &str) -> Result<(), Error> {
  validate_for(metadata, target)?;
  if matches(dist, metadata, target, version)? { return Ok(()); }
  binary_for(dist, None, target)?;
  let marker = dist.join(MARKER);
  if marker.exists() {
    fs::remove_file(&marker).map_err(|error| Error::with_source(format!("remove stale {}", marker.display()), error))?;
  }
  let binary = if has_branding(metadata) {
    match target.platform {
      "win32" => apply_windows(dist, metadata)?,
      "darwin" => apply_macos(dist, metadata)?,
      _ => apply_other(dist, metadata)?,
    }
  } else {
    dist.join(target.executable())
  };
  target.verify_binary(&binary)?;
  let relative = relative_marker_path(dist, &binary)?;
  let marker_value = serde_json::json!({
    "version": MARKER_VERSION,
    "target": target.platform,
    "arch": target.arch,
    "electronVersion": version,
    "appName": metadata.app_name,
    "identifier": metadata.identifier,
    "binary": relative,
    "fingerprint": fingerprint(metadata, target)?,
  });
  let temporary = marker.with_extension("json.tmp");
  let bytes = serde_json::to_vec(&marker_value)?;
  let mut file = File::create(&temporary).map_err(|error| Error::with_source(format!("create {}", temporary.display()), error))?;
  file.write_all(&bytes).map_err(|error| Error::with_source(format!("write {}", temporary.display()), error))?;
  file.sync_all().map_err(|error| Error::with_source(format!("sync {}", temporary.display()), error))?;
  drop(file);
  fs::rename(&temporary, &marker).map_err(|error| Error::with_source(format!("publish {}", marker.display()), error))
}

/// Resolve a runtime binary, including legacy markers, without changing files.
pub(crate) fn binary(dist: &Path, name: Option<&str>) -> Result<PathBuf, Error> {
  binary_for(dist, name, &DistributionTarget::current()?)
}

pub(crate) fn binary_for(dist: &Path, name: Option<&str>, target: &DistributionTarget) -> Result<PathBuf, Error> {
  if let Some(name) = name { validate_name(name)?; }
  let marker_path = dist.join(MARKER);
  if marker_path.exists() {
    let value = read_marker(&marker_path)?;
    return marker_binary(dist, &value, target)?
      .ok_or_else(|| format!("metadata marker has no compatible Electron binary: {}", marker_path.display()).into());
  }
  let path = match name {
    Some(name) => named_binary(dist, target.platform, name),
    None => dist.join(target.executable()),
  };
  target.verify_binary(&path)?;
  Ok(path)
}

#[cfg(windows)]
fn apply_windows(dist: &Path, metadata: &Metadata) -> Result<PathBuf, Error> {
  let source = dist.join("electron.exe");
  let target = metadata.app_name.as_deref().map(|name| dist.join(format!("{name}.exe"))).unwrap_or_else(|| source.clone());
  rename_if_needed(&source, &target)?;
  if metadata.icon.is_some() || metadata.host_executable.is_some() || metadata.app_name.is_some() {
    crate::pe::apply(&target, metadata.app_name.as_deref(), metadata.icon.as_deref(), metadata.host_executable.as_deref())?;
  }
  Ok(target)
}

#[cfg(not(windows))]
fn apply_windows(_dist: &Path, _metadata: &Metadata) -> Result<PathBuf, Error> {
  Err("native Windows branding requires a Windows host".into())
}

fn apply_other(dist: &Path, metadata: &Metadata) -> Result<PathBuf, Error> {
  let source = dist.join("electron");
  let target = metadata.app_name.as_deref().map(|name| dist.join(name)).unwrap_or_else(|| source.clone());
  rename_if_needed(&source, &target)?;
  if metadata.identifier.is_some() || metadata.icon.is_some() || metadata.host_executable.is_some() {
    return Err("metadata branding is unsupported for target platform linux".into());
  }
  Ok(target)
}

#[cfg(target_os = "macos")]
fn apply_macos(dist: &Path, metadata: &Metadata) -> Result<PathBuf, Error> {
  let source_app = dist.join("Electron.app");
  let app = metadata.app_name.as_deref().map(|name| dist.join(format!("{name}.app"))).unwrap_or_else(|| source_app.clone());
  if source_app != app && app.exists() && !source_app.exists() {
    let plist = app.join("Contents/Info.plist");
    if !plist.is_file() || !metadata.app_name.as_deref().is_some_and(|name| plist_has_value(&plist, "CFBundleExecutable", name)) {
      return Err(format!("refusing to brand existing bundle without matching metadata: {}", app.display()).into());
    }
  }
  rename_if_needed(&source_app, &app)?;
  let source_binary = app.join("Contents/MacOS/Electron");
  let target_binary = metadata.app_name.as_deref().map(|name| app.join("Contents/MacOS").join(name)).unwrap_or_else(|| source_binary.clone());
  rename_if_needed(&source_binary, &target_binary)?;
  if let Some(name) = metadata.app_name.as_deref() { rename_helpers(&app, name)?; }
  patch_plist(&app, metadata)?;
  if metadata.icon.is_some() || metadata.identifier.is_some() || metadata.app_name.is_some() { codesign(&app)?; }
  Ok(target_binary)
}

#[cfg(target_os = "macos")]
fn plist_has_value(plist: &Path, key: &str, expected: &str) -> bool {
  Command::new("/usr/libexec/PlistBuddy").args(["-c", &format!("Print :{key}")]).arg(plist).output()
    .is_ok_and(|output| output.status.success() && String::from_utf8_lossy(&output.stdout).trim() == expected)
}

#[cfg(not(target_os = "macos"))]
fn apply_macos(_dist: &Path, _metadata: &Metadata) -> Result<PathBuf, Error> {
  Err("cannot brand a macOS Electron distribution on a non-macOS host".into())
}

fn ensure_host_supports(platform: &str) -> Result<(), Error> {
  if platform == "win32" && !cfg!(windows) { return Err("cannot brand a Windows Electron distribution on a non-Windows host".into()); }
  if platform == "darwin" && !cfg!(target_os = "macos") { return Err("cannot brand a macOS Electron distribution on a non-macOS host".into()); }
  Ok(())
}

fn named_binary(dist: &Path, platform: &str, name: &str) -> PathBuf {
  match platform {
    "win32" => dist.join(format!("{name}.exe")),
    "darwin" => dist.join(format!("{name}.app/Contents/MacOS/{name}")),
    _ => dist.join(name),
  }
}

fn rename_if_needed(source: &Path, target: &Path) -> Result<(), Error> {
  if source == target || !source.exists() {
    return if target.exists() { Ok(()) } else { Err(format!("Electron binary not found: {}", source.display()).into()) };
  }
  if target.exists() { return Err(format!("refusing to overwrite {}", target.display()).into()); }
  fs::rename(source, target).map_err(|error| Error::with_source(format!("rename {} -> {}", source.display(), target.display()), error))
}

fn validate_name(name: &str) -> Result<(), Error> {
  if name.is_empty() || name == "." || name == ".." { return Err("app_name must not be empty or a dot path".into()); }
  if name.chars().any(|ch| ch.is_control() || "<>:\"/\\|?*".contains(ch)) {
    return Err("app_name contains an invalid filename character or control character".into());
  }
  if name.ends_with('.') || name.ends_with(' ') { return Err("app_name must not end with a dot or space".into()); }
  // Leave room for the longest Electron helper suffix and the bundle extension.
  if name.encode_utf16().count() > 220 || name.len() > 220 { return Err("app_name is too long for an executable/helper filename".into()); }
  let reserved = name.split('.').next().unwrap_or_default().trim_end_matches(' ').to_ascii_uppercase();
  if matches!(reserved.as_str(), "CON" | "PRN" | "AUX" | "NUL")
    || (reserved.len() == 4 && (reserved.starts_with("COM") || reserved.starts_with("LPT")) && reserved[3..].parse::<u8>().is_ok_and(|n| (1..=9).contains(&n)))
  {
    return Err(format!("app_name is a Windows reserved filename: {name}").into());
  }
  Ok(())
}

fn validate_identifier(identifier: &str) -> Result<(), Error> {
  if identifier.is_empty() || identifier == "." || identifier == ".." || identifier.len() > 255
    || identifier.split('.').any(|part| part.is_empty() || !part.bytes().all(|ch| ch.is_ascii_alphanumeric() || ch == b'-'))
  {
    return Err("identifier must contain non-empty ASCII alphanumeric/hyphen components separated by dots (at most 255 bytes)".into());
  }
  Ok(())
}

fn read_marker(path: &Path) -> Result<serde_json::Value, Error> {
  let bytes = fs::read(path).map_err(|error| Error::with_source(format!("read {}", path.display()), error))?;
  let value: serde_json::Value = serde_json::from_slice(&bytes)
    .map_err(|error| Error::with_source(format!("invalid {}", path.display()), error))?;
  // Old markers are read-only compatible when the executable independently matches.
  if !matches!(value.get("version").and_then(serde_json::Value::as_u64), Some(1 | 2)) {
    return Err(format!("unsupported metadata marker: {}", path.display()).into());
  }
  Ok(value)
}

fn marker_binary(dist: &Path, value: &serde_json::Value, target: &DistributionTarget) -> Result<Option<PathBuf>, Error> {
  let Some(relative) = value.get("binary").and_then(serde_json::Value::as_str) else { return Err("metadata marker has no binary path".into()); };
  let relative_path = Path::new(relative);
  if relative.is_empty() || relative.contains('\\') || relative.contains(':')
    || relative.chars().any(char::is_control)
    || relative.split('/').any(|part| part.is_empty() || part == "." || part == "..")
    || relative_path.is_absolute() || relative_path.components().any(|component| !matches!(component, Component::Normal(_)))
  {
    return Err(format!("metadata marker binary path is not a relative normal path: {relative}").into());
  }
  if value.get("target").and_then(serde_json::Value::as_str) != Some(target.platform) { return Ok(None); }
  if value.get("version").and_then(serde_json::Value::as_u64) == Some(MARKER_VERSION as u64)
    && value.get("arch").and_then(serde_json::Value::as_str) != Some(target.arch)
  {
    return Ok(None);
  }
  let candidate = dist.join(relative_path);
  if !candidate.exists() { return Ok(None); }
  if !candidate.is_file() { return Err(format!("metadata marker binary is not a file: {}", candidate.display()).into()); }
  let root = fs::canonicalize(dist).map_err(|error| Error::with_source(format!("resolve {}", dist.display()), error))?;
  let canonical = fs::canonicalize(&candidate).map_err(|error| Error::with_source(format!("resolve {}", candidate.display()), error))?;
  if !canonical.starts_with(&root) { return Err(format!("metadata marker binary escapes distribution: {relative}").into()); }
  target.verify_binary(&candidate)?;
  Ok(Some(candidate))
}

fn relative_marker_path(dist: &Path, binary: &Path) -> Result<String, Error> {
  let relative = binary.strip_prefix(dist)
    .map_err(|error| Error::with_source(format!("branded binary is outside distribution: {}", binary.display()), error))?;
  if relative.components().any(|component| !matches!(component, Component::Normal(_))) {
    return Err(format!("branded binary is not a normal relative path: {}", binary.display()).into());
  }
  let relative = relative.to_string_lossy().replace('\\', "/");
  if relative.is_empty() || relative.contains(':') || relative.chars().any(char::is_control)
    || relative.split('/').any(|part| part.is_empty() || part == "." || part == "..")
  {
    return Err(format!("branded binary is not a safe relative path: {relative}").into());
  }
  Ok(relative)
}

fn fingerprint(metadata: &Metadata, target: &DistributionTarget) -> Result<String, Error> {
  let mut hasher = DefaultHasher::new();
  target.platform.hash(&mut hasher);
  target.arch.hash(&mut hasher);
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

fn hash_file(path: &Path, hasher: &mut DefaultHasher) -> Result<(), Error> {
  let mut file = File::open(path).map_err(|error| Error::with_source(format!("open {}", path.display()), error))?;
  let mut buffer = [0_u8; 64 * 1024];
  loop {
    let count = file.read(&mut buffer).map_err(|error| Error::with_source(format!("read {}", path.display()), error))?;
    if count == 0 { break; }
    buffer[..count].hash(hasher);
  }
  Ok(())
}

#[cfg(target_os = "macos")]
fn patch_plist(app: &Path, metadata: &Metadata) -> Result<(), Error> {
  let plist = app.join("Contents/Info.plist");
  if !plist.is_file() { return Err(format!("missing macOS Info.plist: {}", plist.display()).into()); }
  if let Some(name) = metadata.app_name.as_deref() {
    for key in ["CFBundleExecutable", "CFBundleName", "CFBundleDisplayName"] { plist_string(&plist, key, name)?; }
  }
  if let Some(identifier) = metadata.identifier.as_deref() { plist_string(&plist, "CFBundleIdentifier", identifier)?; }
  if let Some(icon) = metadata.icon.as_deref() {
    validate_icns(icon)?;
    let destination = app.join("Contents/Resources/app.icns");
    fs::copy(icon, &destination).map_err(|error| Error::with_source(format!("copy {}", destination.display()), error))?;
    plist_string(&plist, "CFBundleIconFile", "app.icns")?;
  }
  Ok(())
}

#[cfg(target_os = "macos")]
fn plist_string(plist: &Path, key: &str, value: &str) -> Result<(), Error> {
  let replace = Command::new("plutil").args(["-replace", key, "-string", value]).arg(plist).output()
    .map_err(|error| Error::with_source(format!("start plutil for {}", plist.display()), error))?;
  if replace.status.success() { return Ok(()); }
  let insert = Command::new("plutil").args(["-insert", key, "-string", value]).arg(plist).output()
    .map_err(|error| Error::with_source(format!("start plutil for {}", plist.display()), error))?;
  if insert.status.success() { Ok(()) } else {
    Err(format!("plutil could not set {key} in {} ({}): {}", plist.display(), insert.status, String::from_utf8_lossy(&insert.stderr)).into())
  }
}

fn validate_icns(path: &Path) -> Result<(), Error> {
  let bytes = fs::read(path).map_err(|error| Error::with_source(format!("read {}", path.display()), error))?;
  if bytes.len() < 16 || &bytes[..4] != b"icns" { return Err(format!("invalid ICNS icon: {}", path.display()).into()); }
  let declared = u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize;
  if declared != bytes.len() { return Err(format!("invalid ICNS length: {}", path.display()).into()); }
  let mut offset = 8;
  while offset < declared {
    let chunk = bytes.get(offset..offset + 8).ok_or("truncated ICNS chunk")?;
    let length = u32::from_be_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]) as usize;
    if length < 8 || length > declared - offset { return Err(format!("invalid ICNS chunk: {}", path.display()).into()); }
    offset += length;
  }
  Ok(())
}

#[cfg(target_os = "macos")]
fn rename_helpers(app: &Path, name: &str) -> Result<(), Error> {
  let frameworks = app.join("Contents/Frameworks");
  if !frameworks.is_dir() { return Ok(()); }
  let entries = fs::read_dir(&frameworks).map_err(|error| Error::with_source(format!("read {}", frameworks.display()), error))?;
  for entry in entries {
    let entry = entry?;
    let old_path = entry.path();
    let old_file = entry.file_name();
    let old_file = old_file.to_string_lossy();
    let Some(old_base) = old_file.strip_suffix(".app") else { continue };
    let Some(suffix) = old_base.strip_prefix("Electron Helper") else { continue };
    if !old_path.is_dir() { continue; }
    let new_base = format!("{name} Helper{suffix}");
    let new_path = frameworks.join(format!("{new_base}.app"));
    if old_path != new_path {
      if new_path.exists() { return Err(format!("refusing to overwrite helper bundle: {}", new_path.display()).into()); }
      fs::rename(&old_path, &new_path).map_err(|error| Error::with_source(format!("rename {} -> {}", old_path.display(), new_path.display()), error))?;
    }
    let old_executable = new_path.join("Contents/MacOS").join(old_base);
    let new_executable = new_path.join("Contents/MacOS").join(&new_base);
    rename_if_needed(&old_executable, &new_executable)?;
    let plist = new_path.join("Contents/Info.plist");
    if plist.is_file() {
      for key in ["CFBundleExecutable", "CFBundleName", "CFBundleDisplayName"] { plist_string(&plist, key, &new_base)?; }
    }
  }
  Ok(())
}

#[cfg(target_os = "macos")]
fn codesign(app: &Path) -> Result<(), Error> {
  let output = Command::new("codesign")
    .args(["--force", "--deep", "--sign", "-", "--preserve-metadata=entitlements,requirements"])
    .arg(app).output().map_err(|error| Error::with_source(format!("start codesign for {}", app.display()), error))?;
  if output.status.success() { Ok(()) } else {
    Err(format!("codesign failed for {} ({}): {}; production builds still require Developer ID signing", app.display(), output.status, String::from_utf8_lossy(&output.stderr)).into())
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn marker_path_rejects_traversal() {
    let target = DistributionTarget::current().unwrap();
    let value = serde_json::json!({"version": MARKER_VERSION, "binary": "../outside"});
    let error = marker_binary(Path::new("dist"), &value, &target).unwrap_err();
    assert!(error.to_string().contains("relative normal"));
  }

  #[test]
  fn marker_path_rejects_windows_traversal_on_every_host() {
    let target = DistributionTarget::current().unwrap();
    let value = serde_json::json!({"version": MARKER_VERSION, "target": target.platform, "binary": "..\\outside.exe"});
    assert!(marker_binary(Path::new("dist"), &value, &target).unwrap_err().to_string().contains("relative normal"));
  }

  #[test]
  fn marker_identity_requires_matching_headers_and_preserves_legacy_runtime_lookup() {
    let root = crate::acquisition::unique_sibling(&Path::new(env!("CARGO_MANIFEST_DIR")).join("target/electron-marker"), "test");
    let target = DistributionTarget::from_triple("x86_64-unknown-linux-gnu").unwrap();
    let other = DistributionTarget::from_triple("aarch64-unknown-linux-gnu").unwrap();
    fs::create_dir_all(&root).unwrap();
    let executable = root.join(target.executable());
    fs::write(&executable, crate::acquisition::test_binary(&target)).unwrap();
    let metadata = Metadata::default();
    apply(&root, &metadata, &target, crate::DEFAULT_VERSION).unwrap();
    assert!(matches(&root, &metadata, &target, crate::DEFAULT_VERSION).unwrap());
    assert!(!matches(&root, &metadata, &target, "44.0.0").unwrap());
    let marker = root.join(MARKER);
    let mut value = read_marker(&marker).unwrap();
    value["arch"] = serde_json::json!(other.arch);
    fs::write(&marker, serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(binary_for(&root, None, &target).is_err());
    value["arch"] = serde_json::json!(target.arch);
    fs::write(&marker, serde_json::to_vec(&value).unwrap()).unwrap();
    fs::write(&executable, crate::acquisition::test_binary(&other)).unwrap();
    assert!(binary_for(&root, None, &target).is_err());
    assert!(!matches(&root, &metadata, &target, crate::DEFAULT_VERSION).unwrap());
    fs::write(&executable, crate::acquisition::test_binary(&target)).unwrap();
    value["version"] = serde_json::json!(1);
    value.as_object_mut().unwrap().remove("arch");
    fs::write(&marker, serde_json::to_vec(&value).unwrap()).unwrap();
    assert_eq!(binary_for(&root, None, &target).unwrap(), executable);
    assert!(!matches(&root, &metadata, &target, crate::DEFAULT_VERSION).unwrap());
    fs::remove_dir_all(root).unwrap();
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
