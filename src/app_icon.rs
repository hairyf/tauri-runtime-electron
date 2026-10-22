//! Reads the icon embedded in the application executable.
//!
//! A packaged Tauri app ships no icon file: the bundler embeds `bundle.icon`
//! into the executable's resources. Electron wants an image path for
//! `BrowserWindow::icon`, so the group icon is copied out of the executable into
//! a temporary `.ico`. `app.getFileIcon` cannot be used for this: the shell
//! answers with a generic application icon instead of the embedded resources.

use std::path::{Path, PathBuf};

/// Writes the executable's icon into the temp dir and returns its path.
pub(crate) fn extract(exe: &Path) -> Option<PathBuf> {
  imp::extract(exe)
}

#[cfg(not(windows))]
mod imp {
  use std::path::{Path, PathBuf};

  pub(super) fn extract(_exe: &Path) -> Option<PathBuf> {
    None
  }
}

#[cfg(windows)]
mod imp {
  use std::path::{Path, PathBuf};

  use windows::core::PCWSTR;
  use windows::Win32::Foundation::{FreeLibrary, HMODULE};
  use windows::Win32::System::LibraryLoader::{
    FindResourceW, LoadLibraryExW, LoadResource, LockResource, SizeofResource,
    LOAD_LIBRARY_AS_DATAFILE,
  };

  const RT_ICON: u16 = 3;
  const RT_GROUP_ICON: u16 = 14;
  /// `tauri-build` stores the application icon under the shell's default icon
  /// ordinal; older toolchains used id 1.
  const APP_ICON: u16 = 32512;
  const LEGACY_ICON_IDS: std::ops::RangeInclusive<u16> = 1..=32;

  pub(super) fn extract(exe: &Path) -> Option<PathBuf> {
    let wide = to_wide(exe);
    let module = unsafe { LoadLibraryExW(PCWSTR(wide.as_ptr()), None, LOAD_LIBRARY_AS_DATAFILE) }
      .ok()?;
    let result = unsafe { read_group_icon(module) }.and_then(write_icon);
    let _ = unsafe { FreeLibrary(module) };
    result
  }

  unsafe fn read_group_icon(module: HMODULE) -> Option<Vec<u8>> {
    // A `const` item yields a fresh temporary per use, which cannot be borrowed
    // mutably by `find_map`, so iterate a local copy instead.
    let mut legacy_ids = LEGACY_ICON_IDS;
    let group = find(module, RT_GROUP_ICON, APP_ICON)
      .or_else(|| legacy_ids.find_map(|id| find(module, RT_GROUP_ICON, id)))?;
    // GRPICONDIR: reserved, type, count, then count GRPICONDIRENTRY of 14 bytes:
    // width, height, colors, reserved, planes, bit count, bytes, id.
    if group.len() < 6 {
      return None;
    }
    let count = u16::from_le_bytes([group[4], group[5]]) as usize;
    if count == 0 || group.len() < 6 + count * 14 {
      return None;
    }

    // Header plus one directory entry per image; every image blob follows.
    let mut header = Vec::with_capacity(count * 16);
    let mut images: Vec<Vec<u8>> = Vec::with_capacity(count);
    let mut offset = 6 + count * 16;
    for index in 0..count {
      let entry = &group[6 + index * 14..6 + (index + 1) * 14];
      let size = u16::from_le_bytes([entry[8], entry[9]]) as usize;
      let id = u16::from_le_bytes([entry[12], entry[13]]);
      let data = find(module, RT_ICON, id)?;
      if data.len() < size {
        return None;
      }
      header.extend_from_slice(&entry[..8]);
      header.extend_from_slice(&(size as u32).to_le_bytes());
      header.extend_from_slice(&(offset as u32).to_le_bytes());
      images.push(data[..size].to_vec());
      offset += size;
    }

    let mut icon = Vec::with_capacity(offset);
    // ICONDIR: reserved = 0, type = 1 (icon), count.
    icon.extend_from_slice(&0u16.to_le_bytes());
    icon.extend_from_slice(&1u16.to_le_bytes());
    icon.extend_from_slice(&(count as u16).to_le_bytes());
    icon.extend_from_slice(&header);
    for data in images {
      icon.extend_from_slice(&data);
    }
    Some(icon)
  }

  fn find(module: HMODULE, kind: u16, id: u16) -> Option<Vec<u8>> {
    let name = PCWSTR(id as usize as *const u16);
    let ty = PCWSTR(kind as usize as *const u16);
    let handle = unsafe { FindResourceW(Some(module), name, ty) };
    let size = unsafe { SizeofResource(Some(module), handle) };
    if size == 0 {
      return None;
    }
    let data = unsafe { LoadResource(Some(module), handle) }.ok()?;
    let pointer = unsafe { LockResource(data) };
    if pointer.is_null() {
      return None;
    }
    Some(unsafe { std::slice::from_raw_parts(pointer as *const u8, size as usize) }.to_vec())
  }

  fn write_icon(bytes: Vec<u8>) -> Option<PathBuf> {
    let path = std::env::temp_dir().join("tauri-runtime-electron-app.ico");
    std::fs::write(&path, bytes).ok()?;
    Some(path)
  }

  fn to_wide(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str().encode_wide().chain([0]).collect()
  }
}
