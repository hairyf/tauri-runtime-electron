//! Build-time Windows resource updates. Freshness belongs to the staging builder.

use std::{fs, path::Path};

use windows::{
  core::{BOOL, PCWSTR},
  Win32::{
    Foundation::{FreeLibrary, GetLastError, HANDLE, HMODULE, ERROR_RESOURCE_DATA_NOT_FOUND, ERROR_RESOURCE_TYPE_NOT_FOUND},
    System::LibraryLoader::{
      BeginUpdateResourceW, EndUpdateResourceW, EnumResourceLanguagesW, EnumResourceNamesW,
      FindResourceExW, FindResourceW, LoadLibraryExW, LoadResource, LockResource, SizeofResource,
      UpdateResourceW, LOAD_LIBRARY_AS_DATAFILE,
    },
  },
};

use crate::Error;

const RT_ICON: u16 = 3;
const RT_GROUP_ICON: u16 = 14;
const RT_VERSION: u16 = 16;
const APP_ICON: u16 = 32512;
const FIRST_ICON_ID: u16 = 4096;

pub(crate) fn apply(binary: &Path, app_name: Option<&str>, icon: Option<&Path>, host: Option<&Path>) -> Result<(), Error> {
  let icon = if let Some(path) = icon {
    Some(read_ico(path)?)
  } else if let Some(path) = host {
    Some(read_host_icon(path)?)
  } else {
    None
  };
  let module = Module::open(binary)?;
  let mut groups = entries(module.0, RT_GROUP_ICON)?;
  let mut versions = Vec::new();
  if let Some(name) = app_name {
    let filename = binary.file_name().map(|name| name.to_string_lossy()).ok_or("Windows binary has no basename")?;
    for (id, language) in entries(module.0, RT_VERSION)? {
      let data = resource(module.0, RT_VERSION, &id, Some(language))?;
      versions.push((id, language, brand_version(&data, name, &filename)?));
    }
    if versions.is_empty() {
      return Err(format!("Electron executable has no version resource: {}", binary.display()).into());
    }
  }
  drop(module);
  if icon.is_none() && versions.is_empty() {
    return Ok(());
  }
  if groups.is_empty() {
    groups.push((ResourceName::Id(APP_ICON), 0));
  }

  let wide = wide_path(binary)?;
  // SAFETY: all names and data buffers stay alive for the synchronous Win32 calls.
  let update = unsafe { BeginUpdateResourceW(PCWSTR(wide.as_ptr()), false) }
    .map_err(|error| Error::with_source(format!("begin resource update {}", binary.display()), error))?;
  let result: Result<(), Error> = (|| {
    if let Some(icon) = &icon {
      // All groups are replaced; use the same image IDs in each group's language.
      let group = rebase_group(&icon.group, FIRST_ICON_ID)?;
      for (id, language) in &groups {
        for (index, image) in icon.images.iter().enumerate() {
          update_resource(update, RT_ICON, &ResourceName::Id(FIRST_ICON_ID + index as u16), *language, image)?;
        }
        update_resource(update, RT_GROUP_ICON, id, *language, &group)?;
      }
    }
    for (id, language, data) in &versions {
      update_resource(update, RT_VERSION, id, *language, data)?;
    }
    Ok(())
  })();
  if let Err(error) = result {
    return match unsafe { EndUpdateResourceW(update, true) } {
      Ok(()) => Err(error),
      Err(discard) => Err(error.context(format!("discard resource update: {discard}"))),
    };
  }
  unsafe { EndUpdateResourceW(update, false) }.map_err(|error| Error::with_source(format!("commit resource update {}", binary.display()), error))
}

pub(crate) fn validate_icon(path: &Path) -> Result<(), Error> {
  read_ico(path).map(|_| ())
}

pub(crate) fn validate_host_icon(path: &Path) -> Result<(), Error> {
  read_host_icon(path).map(|_| ())
}

struct Icon {
  group: Vec<u8>,
  images: Vec<Vec<u8>>,
}

fn read_ico(path: &Path) -> Result<Icon, Error> {
  let bytes = fs::read(path).map_err(|error| Error::with_source(format!("icon {}", path.display()), error))?;
  parse_ico(&bytes).map_err(|error| error.context(format!("invalid ICO {}", path.display())))
}

fn parse_ico(bytes: &[u8]) -> Result<Icon, Error> {
  let count = icon_count(bytes)?;
  let directory_end = 6 + count * 16;
  if directory_end > bytes.len() {
    return Err("truncated icon directory".into());
  }
  let mut group = bytes[..6].to_vec();
  let mut images = Vec::with_capacity(count);
  for index in 0..count {
    let entry = &bytes[6 + index * 16..6 + (index + 1) * 16];
    // ICO directories may leave planes unspecified; the image header is validated below.
    if le16(entry, 4)? > 1 { return Err("icon directory image planes must be zero or one".into()); }
    let size = le32(entry, 8)? as usize;
    let offset = le32(entry, 12)? as usize;
    let end = offset.checked_add(size).ok_or("icon image range overflows")?;
    if entry[3] != 0 || size == 0 || offset < directory_end || end > bytes.len() {
      return Err("invalid icon image range or reserved byte".into());
    }
    let image = &bytes[offset..end];
    validate_image(image, entry[0], entry[1])?;
    group.extend_from_slice(&entry[..12]);
    group.extend_from_slice(&((index + 1) as u16).to_le_bytes());
    images.push(image.to_vec());
  }
  Ok(Icon { group, images })
}

fn rebase_group(group: &[u8], first_id: u16) -> Result<Vec<u8>, Error> {
  let count = icon_count(group)?;
  if group.len() != 6 + count * 14 || u32::from(first_id) + count as u32 > u16::MAX as u32 {
    return Err("icon group has an invalid size or too many images".into());
  }
  let mut group = group.to_vec();
  for index in 0..count {
    let offset = 6 + index * 14 + 12;
    group[offset..offset + 2].copy_from_slice(&(first_id + index as u16).to_le_bytes());
  }
  Ok(group)
}

fn icon_count(bytes: &[u8]) -> Result<usize, Error> {
  if le16(bytes, 0)? != 0 || le16(bytes, 2)? != 1 {
    return Err("expected an icon directory (not a cursor)".into());
  }
  let count = le16(bytes, 4)? as usize;
  if count == 0 {
    return Err("icon directory is empty".into());
  }
  Ok(count)
}

fn validate_image(bytes: &[u8], width: u8, height: u8) -> Result<(), Error> {
  let width = if width == 0 { 256 } else { u32::from(width) };
  let height = if height == 0 { 256 } else { u32::from(height) };
  if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
    if bytes.len() < 33 || bytes[8..12] != 13_u32.to_be_bytes() || &bytes[12..16] != b"IHDR"
      || bytes[16..20] != width.to_be_bytes() || bytes[20..24] != height.to_be_bytes()
    {
      return Err("invalid PNG icon dimensions/header".into());
    }
    let mut offset = 8;
    while offset < bytes.len() {
      let header = bytes.get(offset..offset + 8).ok_or("truncated PNG chunk")?;
      let length = u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as usize;
      let end = offset.checked_add(12).and_then(|start| start.checked_add(length)).ok_or("PNG chunk overflows")?;
      if end > bytes.len() {
        return Err("truncated PNG chunk data".into());
      }
      if &header[4..8] == b"IEND" {
        return if length == 0 && end == bytes.len() { Ok(()) } else { Err("invalid PNG end chunk".into()) };
      }
      offset = end;
    }
    return Err("PNG icon has no end chunk".into());
  }
  let header = le32(bytes, 0)? as usize;
  let bits = le16(bytes, 14)?;
  if !matches!(header, 40 | 108 | 124) || header > bytes.len()
    || le32(bytes, 4)? != width || le32(bytes, 8)? != height * 2
    || le16(bytes, 12)? != 1 || !matches!(bits, 1 | 4 | 8 | 16 | 24 | 32)
    || (!matches!(le32(bytes, 16)?, 0 | 3 | 6))
    || (le32(bytes, 16)? != 0 && bits < 16)
  {
    return Err("invalid DIB icon header/dimensions".into());
  }
  let colors = le32(bytes, 32)? as usize;
  let palette = if bits <= 8 { if colors == 0 { 1 << bits } else { colors } } else { colors };
  let masks = if header == 40 && le32(bytes, 16)? == 3 { 12 } else { 0 };
  let pixel_bytes = (width as usize).checked_mul(bits as usize).ok_or("DIB width overflows")?.div_ceil(32).checked_mul(4).and_then(|size| size.checked_mul(height as usize)).ok_or("DIB pixel size overflows")?;
  let mask_bytes = (width as usize).div_ceil(32).checked_mul(4).and_then(|size| size.checked_mul(height as usize)).ok_or("DIB mask size overflows")?;
  let required = palette.checked_mul(4).and_then(|size| size.checked_add(header + masks + pixel_bytes + mask_bytes)).ok_or("DIB icon size overflows")?;
  if required > bytes.len() {
    return Err("truncated DIB icon pixels/mask".into());
  }
  Ok(())
}

fn read_host_icon(path: &Path) -> Result<Icon, Error> {
  let module = Module::open(path)?;
  let mut groups = entries(module.0, RT_GROUP_ICON)?;
  groups.sort_by_key(|(id, _)| !matches!(id, ResourceName::Id(APP_ICON)));
  let (id, language) = groups.first().ok_or_else(|| format!("host has no embedded icon: {}", path.display()))?;
  let mut group = resource(module.0, RT_GROUP_ICON, id, Some(*language))?;
  let count = icon_count(&group)?;
  if group.len() != 6 + count * 14 {
    return Err(format!("invalid host icon group: {}", path.display()).into());
  }
  let mut images = Vec::with_capacity(count);
  for index in 0..count {
    let entry = &mut group[6 + index * 14..6 + (index + 1) * 14];
    let image_id = ResourceName::Id(le16(entry, 12)?);
    let image = resource(module.0, RT_ICON, &image_id, Some(*language))
      .or_else(|_| resource(module.0, RT_ICON, &image_id, None))?;
    if image.is_empty() || le32(entry, 8)? as usize != image.len() {
      return Err("host icon image length does not match its group".into());
    }
    validate_image(&image, entry[0], entry[1])?;
    entry[12..14].copy_from_slice(&((index + 1) as u16).to_le_bytes());
    images.push(image);
  }
  Ok(Icon { group, images })
}

#[derive(Clone)]
enum ResourceName {
  Id(u16),
  Text(Vec<u16>),
}

impl ResourceName {
  fn pointer(&self) -> PCWSTR {
    match self {
      Self::Id(id) => ordinal(*id),
      Self::Text(wide) => PCWSTR(wide.as_ptr()),
    }
  }
}

struct Module(HMODULE);

impl Module {
  fn open(path: &Path) -> Result<Self, Error> {
    let absolute = fs::canonicalize(path).map_err(|error| Error::with_source(path.display().to_string(), error))?;
    let wide = wide_path(&absolute)?;
    // LOAD_LIBRARY_AS_DATAFILE never executes the host's code.
    unsafe { LoadLibraryExW(PCWSTR(wide.as_ptr()), None, LOAD_LIBRARY_AS_DATAFILE) }
      .map(Self).map_err(|error| Error::with_source(format!("load PE resources {}", path.display()), error))
  }
}

impl Drop for Module {
  fn drop(&mut self) {
    let _ = unsafe { FreeLibrary(self.0) };
  }
}

fn resource(module: HMODULE, kind: u16, name: &ResourceName, language: Option<u16>) -> Result<Vec<u8>, Error> {
  let handle = unsafe {
    match language {
      Some(language) => FindResourceExW(Some(module), ordinal(kind), name.pointer(), language),
      None => FindResourceW(Some(module), name.pointer(), ordinal(kind)),
    }
  };
  if handle.is_invalid() {
    let error = windows::core::Error::from_thread();
    return Err(Error::with_source(format!("find resource type {kind}"), error));
  }
  let size = unsafe { SizeofResource(Some(module), handle) };
  if size == 0 {
    return Err(format!("empty resource type {kind}").into());
  }
  let loaded = unsafe { LoadResource(Some(module), handle) }.map_err(|error| Error::with_source(format!("load resource type {kind}"), error))?;
  let pointer = unsafe { LockResource(loaded) };
  if pointer.is_null() {
    let error = windows::core::Error::from_thread();
    return Err(Error::with_source(format!("lock resource type {kind}"), error));
  }
  // SAFETY: this module remains loaded, and SizeofResource bounds the returned buffer.
  Ok(unsafe { std::slice::from_raw_parts(pointer as *const u8, size as usize) }.to_vec())
}

fn entries(module: HMODULE, kind: u16) -> Result<Vec<(ResourceName, u16)>, Error> {
  let mut names: Vec<ResourceName> = Vec::new();
  let found = unsafe { EnumResourceNamesW(Some(module), ordinal(kind), Some(collect_name), &mut names as *mut _ as isize) };
  if !found.as_bool() {
    let error = unsafe { GetLastError() };
    if error == ERROR_RESOURCE_TYPE_NOT_FOUND || error == ERROR_RESOURCE_DATA_NOT_FOUND {
      return Ok(Vec::new());
    }
    let error = windows::core::Error::from_thread();
    return Err(Error::with_source(format!("enumerate resource type {kind}"), error));
  }
  let mut pairs = Vec::new();
  for name in names {
    let mut languages: Vec<u16> = Vec::new();
    unsafe { EnumResourceLanguagesW(Some(module), ordinal(kind), name.pointer(), Some(collect_language), &mut languages as *mut _ as isize) }
      .map_err(|error| Error::with_source(format!("enumerate resource languages for type {kind}"), error))?;
    pairs.extend(languages.into_iter().map(|language| (name.clone(), language)));
  }
  Ok(pairs)
}

unsafe extern "system" fn collect_name(_module: HMODULE, _kind: PCWSTR, name: PCWSTR, context: isize) -> BOOL {
  // SAFETY: EnumResourceNamesW supplies an ordinal or a live NUL-terminated name;
  // context points to the Vec owned by the synchronous caller.
  let names = unsafe { &mut *(context as *mut Vec<ResourceName>) };
  names.push(if name.0 as usize <= u16::MAX as usize {
    ResourceName::Id(name.0 as usize as u16)
  } else {
    ResourceName::Text(unsafe { name.as_wide() }.iter().copied().chain([0]).collect())
  });
  true.into()
}

unsafe extern "system" fn collect_language(_module: HMODULE, _kind: PCWSTR, _name: PCWSTR, language: u16, context: isize) -> BOOL {
  unsafe { &mut *(context as *mut Vec<u16>) }.push(language);
  true.into()
}

fn update_resource(update: HANDLE, kind: u16, name: &ResourceName, language: u16, bytes: &[u8]) -> Result<(), Error> {
  let size = u32::try_from(bytes.len()).map_err(|_| "resource is larger than 4 GiB")?;
  unsafe { UpdateResourceW(update, ordinal(kind), name.pointer(), language, Some(bytes.as_ptr().cast()), size) }
    .map_err(|error| Error::with_source(format!("update resource type {kind}, language {language}"), error))
}

fn ordinal(id: u16) -> PCWSTR {
  PCWSTR(id as usize as *const u16)
}

fn wide_path(path: &Path) -> Result<Vec<u16>, Error> {
  use std::os::windows::ffi::OsStrExt;
  let mut wide: Vec<_> = path.as_os_str().encode_wide().collect();
  if wide.contains(&0) {
    return Err("PE path contains a NUL".into());
  }
  wide.push(0);
  Ok(wide)
}

fn le16(bytes: &[u8], offset: usize) -> Result<u16, Error> {
  let data = bytes.get(offset..offset + 2).ok_or("truncated resource WORD")?;
  Ok(u16::from_le_bytes([data[0], data[1]]))
}

fn le32(bytes: &[u8], offset: usize) -> Result<u32, Error> {
  let data = bytes.get(offset..offset + 4).ok_or("truncated resource DWORD")?;
  Ok(u32::from_le_bytes([data[0], data[1], data[2], data[3]]))
}

struct VersionNode {
  key: String,
  kind: u16,
  value: Vec<u8>,
  children: Vec<VersionNode>,
}

impl VersionNode {
  fn parse(bytes: &[u8], depth: usize) -> Result<Self, Error> {
    if depth > 8 {
      return Err("version resource nesting exceeds the Windows version schema".into());
    }
    let length = le16(bytes, 0)? as usize;
    let bytes = bytes.get(..length).filter(|bytes| bytes.len() >= 6).ok_or("invalid version node length")?;
    let value_length = le16(bytes, 2)? as usize;
    let kind = le16(bytes, 4)?;
    if kind > 1 {
      return Err("invalid version value type".into());
    }
    let mut offset = 6;
    let mut key = Vec::new();
    loop {
      let ch = le16(bytes, offset)?;
      offset += 2;
      if ch == 0 { break; }
      key.push(ch);
    }
    let key = String::from_utf16(&key).map_err(|error| Error::with_source("invalid version key UTF-16", error))?;
    let start = (offset + 3) & !3;
    let value_bytes = value_length.checked_mul(if kind == 1 { 2 } else { 1 }).ok_or("version value length overflows")?;
    let end = start.checked_add(value_bytes).ok_or("version value range overflows")?;
    let value = bytes.get(start..end).ok_or("truncated version value")?.to_vec();
    let mut children = Vec::new();
    offset = if value_bytes == 0 { start } else { (end + 3) & !3 };
    while offset < length {
      if bytes[offset..].iter().all(|byte| *byte == 0) { break; }
      let child_length = le16(bytes, offset)? as usize;
      if child_length < 6 {
        return Err("invalid version child length".into());
      }
      let child = bytes.get(offset..offset + child_length).ok_or("truncated version child")?;
      children.push(Self::parse(child, depth + 1)?);
      offset = (offset + child_length + 3) & !3;
    }
    Ok(Self { key, kind, value, children })
  }

  fn encode(&self) -> Result<Vec<u8>, Error> {
    let value_length = if self.kind == 1 { self.value.len() / 2 } else { self.value.len() };
    let value_length = u16::try_from(value_length).map_err(|_| "version value is too large")?;
    let mut bytes = vec![0, 0];
    bytes.extend_from_slice(&value_length.to_le_bytes());
    bytes.extend_from_slice(&self.kind.to_le_bytes());
    for ch in self.key.encode_utf16().chain([0]) { bytes.extend_from_slice(&ch.to_le_bytes()); }
    bytes.resize((bytes.len() + 3) & !3, 0);
    bytes.extend_from_slice(&self.value);
    for child in &self.children {
      bytes.resize((bytes.len() + 3) & !3, 0);
      bytes.extend_from_slice(&child.encode()?);
    }
    let length = u16::try_from(bytes.len()).map_err(|_| "rebuilt version resource exceeds 65535 bytes")?;
    bytes[..2].copy_from_slice(&length.to_le_bytes());
    Ok(bytes)
  }

  fn set_string(&mut self, key: &str, value: &str) {
    let value: Vec<u8> = value.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect();
    if let Some(node) = self.children.iter_mut().find(|node| node.key == key) {
      node.kind = 1;
      node.value = value;
    } else {
      self.children.push(Self { key: key.into(), kind: 1, value, children: Vec::new() });
    }
  }
}

fn brand_version(bytes: &[u8], name: &str, filename: &str) -> Result<Vec<u8>, Error> {
  let mut root = VersionNode::parse(bytes, 0)?;
  if root.key != "VS_VERSION_INFO" || root.value.len() < 52 || le32(&root.value, 0)? != 0xFEEF04BD {
    return Err("invalid VS_VERSION_INFO fixed data".into());
  }
  let strings = root.children.iter_mut().find(|node| node.key == "StringFileInfo").ok_or("Electron version resource has no StringFileInfo")?;
  if strings.children.is_empty() {
    return Err("Electron version resource has no string tables".into());
  }
  for table in &mut strings.children {
    for key in ["ProductName", "FileDescription", "InternalName"] { table.set_string(key, name); }
    table.set_string("OriginalFilename", filename);
  }
  root.encode()
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn io_and_utf16_errors_retain_their_sources() {
    let error = validate_icon(Path::new("")).unwrap_err();
    assert!(std::error::Error::source(&error).unwrap().is::<std::io::Error>());
    let error = VersionNode::parse(&[12, 0, 0, 0, 0, 0, 0, 0xd8, 0, 0, 0, 0], 0).err().unwrap();
    assert!(std::error::Error::source(&error).unwrap().is::<std::string::FromUtf16Error>());
  }

  #[test]
  fn version_rebuild_preserves_fixed_data_and_aligned_utf16_fields() {
    let mut fixed = vec![0; 52];
    fixed[..4].copy_from_slice(&0xFEEF04BD_u32.to_le_bytes());
    let mut table = VersionNode { key: "040904b0".into(), kind: 1, value: Vec::new(), children: Vec::new() };
    table.set_string("ProductName", "Electron");
    table.set_string("FileVersion", "45.0.0");
    let root = VersionNode {
      key: "VS_VERSION_INFO".into(), kind: 0, value: fixed.clone(),
      children: vec![VersionNode { key: "StringFileInfo".into(), kind: 1, value: Vec::new(), children: vec![table] }],
    };
    let branded = brand_version(&root.encode().unwrap(), "App 🦀 longer than Electron", "App 🦀 longer than Electron.exe").unwrap();
    let parsed = VersionNode::parse(&branded, 0).unwrap();
    let fields: Vec<_> = parsed.children[0].children[0].children.iter().map(|node| {
      let text: Vec<_> = node.value.chunks_exact(2).map(|ch| u16::from_le_bytes([ch[0], ch[1]])).collect();
      (node.key.as_str(), String::from_utf16(&text[..text.len() - 1]).unwrap())
    }).collect();
    assert_eq!((parsed.value, fields), (fixed, vec![
      ("ProductName", "App 🦀 longer than Electron".into()), ("FileVersion", "45.0.0".into()),
      ("FileDescription", "App 🦀 longer than Electron".into()), ("InternalName", "App 🦀 longer than Electron".into()),
      ("OriginalFilename", "App 🦀 longer than Electron.exe".into()),
    ]));
    assert!(VersionNode::parse(&branded[..branded.len() - 1], 0).is_err());
  }

  #[test]
  fn ico_parser_rebases_images_and_rejects_truncated_data() {
    let mut bytes = vec![0, 0, 1, 0, 1, 0, 1, 1, 0, 0, 1, 0, 32, 0, 48, 0, 0, 0, 22, 0, 0, 0];
    let mut dib = vec![0; 48];
    dib[..4].copy_from_slice(&40_u32.to_le_bytes());
    dib[4..8].copy_from_slice(&1_u32.to_le_bytes());
    dib[8..12].copy_from_slice(&2_u32.to_le_bytes());
    dib[12..14].copy_from_slice(&1_u16.to_le_bytes());
    dib[14..16].copy_from_slice(&32_u16.to_le_bytes());
    bytes.extend_from_slice(&dib);
    for planes in [0_u16, 1] {
      bytes[10..12].copy_from_slice(&planes.to_le_bytes());
      let icon = parse_ico(&bytes).unwrap();
      assert_eq!((le16(&icon.group, 18).unwrap(), icon.images), (1, vec![dib.clone()]));
      assert!(parse_ico(&bytes[..bytes.len() - 1]).is_err());
    }
    bytes[10..12].copy_from_slice(&2_u16.to_le_bytes());
    assert!(parse_ico(&bytes).is_err());
    bytes[10..12].copy_from_slice(&0_u16.to_le_bytes());
    bytes[34..36].copy_from_slice(&0_u16.to_le_bytes());
    assert!(parse_ico(&bytes).is_err(), "DIB image planes must still equal one");
  }

  #[test]
  #[ignore = "set ELECTRON_RS_PE_SMOKE to an Electron executable; patches temporary copies only"]
  fn actual_electron_branding_smoke() {
    let source = std::env::var_os("ELECTRON_RS_PE_SMOKE").expect("ELECTRON_RS_PE_SMOKE");
    let root = std::env::var_os("ELECTRON_RS_PE_TEMP").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let directory = root.join(format!("electron-rs-pe-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    fs::create_dir(&directory).unwrap();
    let target = directory.join("Native Metadata 🦀.exe");
    fs::copy(&source, &target).unwrap();
    apply(&target, Some("Native Metadata 🦀"), None, None).unwrap();
    apply(&target, Some("Native Metadata 🦀"), None, Some(Path::new(&source))).unwrap();
    if let Some(icon) = std::env::var_os("ELECTRON_RS_ICO_SMOKE") {
      apply(&target, Some("Native Metadata 🦀"), Some(Path::new(&icon)), None).unwrap();
    }
    let module = Module::open(&target).unwrap();
    for (id, language) in entries(module.0, RT_VERSION).unwrap() {
      let data = resource(module.0, RT_VERSION, &id, Some(language)).unwrap();
      let root = VersionNode::parse(&data, 0).unwrap();
      let strings = root.children.iter().find(|node| node.key == "StringFileInfo").unwrap();
      for table in &strings.children {
        for (key, expected) in [("ProductName", "Native Metadata 🦀"), ("FileDescription", "Native Metadata 🦀"), ("OriginalFilename", "Native Metadata 🦀.exe")] {
          let field = table.children.iter().find(|node| node.key == key).unwrap();
          assert_eq!(field.value, expected.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect::<Vec<_>>());
        }
      }
    }
    drop(module);
    let _ = read_host_icon(&target).unwrap();
    assert!(!directory.join(".tauri-runtime-electron-branded").exists());
    fs::remove_dir_all(directory).unwrap();
  }
}
