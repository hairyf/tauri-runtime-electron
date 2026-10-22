//! Gives the Electron executable the app's name and icon.
//!
//! Explorer and Task Manager read a process's name and icon from its executable
//! — the version resource and the icon group — so a staged `electron.exe` shows
//! up as "Electron". The copy the app launches is rewritten once with the
//! application executable's icon group and version resource.

use std::path::Path;

/// Rewrites `binary` in place so it presents `app_exe`'s name and icon.
///
/// Returns `false` when there is nothing to do or the file cannot be rewritten
/// (not an Electron binary, read-only install, no embedded icon). Branding is
/// cosmetic; it never stops the app from starting.
pub(crate) fn apply(binary: &Path, app_exe: &Path) -> bool {
  imp::apply(binary, app_exe)
}

#[cfg(not(windows))]
mod imp {
  use std::path::Path;

  pub(super) fn apply(_binary: &Path, _app_exe: &Path) -> bool {
    false
  }
}

#[cfg(windows)]
mod imp {
  use std::ffi::OsStr;
  use std::path::Path;
  use std::time::UNIX_EPOCH;

  use windows::core::{BOOL, PCWSTR};
  use windows::Win32::Foundation::{FreeLibrary, HANDLE, HMODULE};
  use windows::Win32::System::LibraryLoader::{
    BeginUpdateResourceW, EndUpdateResourceW, EnumResourceLanguagesW, EnumResourceNamesW,
    FindResourceW, LoadLibraryExW, LoadResource, LockResource, SizeofResource, UpdateResourceW,
    LOAD_LIBRARY_AS_DATAFILE,
  };

  const RT_ICON: u16 = 3;
  const RT_GROUP_ICON: u16 = 14;
  const RT_VERSION: u16 = 16;
  /// Where the copied icon images land in the Electron executable. High enough
  /// to stay clear of the icon ids Electron already ships.
  const FIRST_ICON_ID: u16 = 4096;
  /// `tauri-build` stores the application icon under the shell's default icon
  /// ordinal; older toolchains used id 1.
  const APP_ICON: u16 = 32512;
  const LEGACY_ICON_IDS: std::ops::RangeInclusive<u16> = 1..=32;
  /// Records what the binary was branded with, so a rebuilt app is rebranded
  /// while repeated launches of the same build stay untouched.
  const MARKER: &str = ".tauri-runtime-electron-branded";

  pub(super) fn apply(binary: &Path, app_exe: &Path) -> bool {
    if binary.file_name() != Some(OsStr::new("electron.exe"))
      || !binary.exists()
      || !app_exe.exists()
      || binary == app_exe
    {
      return false;
    }
    let marker = binary.with_file_name(MARKER);
    let stamp = stamp(binary, app_exe);
    if std::fs::read_to_string(&marker).ok().as_deref() == Some(stamp.as_str()) {
      return false;
    }
    let Some((mut group, images)) = read_icon(app_exe) else {
      return false;
    };
    let version = read_version(app_exe);
    rebase(&mut group, images.len());

    let electron_wide = to_wide(binary);
    let electron = unsafe { LoadLibraryExW(PCWSTR(electron_wide.as_ptr()), None, LOAD_LIBRARY_AS_DATAFILE) };
    let Ok(electron) = electron else {
      return false;
    };
    let groups = entries(electron, RT_GROUP_ICON);
    let version_languages = entries(electron, RT_VERSION);
    let _ = unsafe { FreeLibrary(electron) };

    if !write(binary, &group, &images, &groups, &version, &version_languages) {
      return false;
    }
    let _ = std::fs::write(&marker, stamp);
    true
  }

  fn write(
    binary: &Path,
    group: &[u8],
    images: &[Vec<u8>],
    groups: &[(u16, u16)],
    version: &Option<Vec<u8>>,
    version_languages: &[(u16, u16)],
  ) -> bool {
    let wide = to_wide(binary);
    let Ok(update) = (unsafe { BeginUpdateResourceW(PCWSTR(wide.as_ptr()), false) }) else {
      return false;
    };
    for (index, image) in images.iter().enumerate() {
      update_resource(update, RT_ICON, FIRST_ICON_ID + index as u16, 0, image);
    }
    // Every group icon in the Electron executable is replaced, so no Electron
    // icon survives under a lower ordinal.
    let groups: &[(u16, u16)] = if groups.is_empty() { &[(APP_ICON, 0)] } else { groups };
    for (id, language) in groups {
      update_resource(update, RT_GROUP_ICON, *id, *language, group);
    }
    // The version resource carries the product name Task Manager displays.
    if let Some(version) = version {
      let mut written = false;
      for (id, language) in version_languages {
        update_resource(update, RT_VERSION, *id, *language, version);
        written = true;
      }
      if !written {
        update_resource(update, RT_VERSION, 1, 0x0409, version);
      }
    }
    unsafe { EndUpdateResourceW(update, false) }.is_ok()
  }

  fn update_resource(update: HANDLE, kind: u16, id: u16, language: u16, data: &[u8]) {
    let _ = unsafe {
      UpdateResourceW(
        update,
        PCWSTR(kind as usize as *const u16),
        PCWSTR(id as usize as *const u16),
        language,
        Some(data.as_ptr() as *const core::ffi::c_void),
        data.len() as u32,
      )
    };
  }

  /// GRPICONDIR entries point at `RT_ICON` ids; point them at the ids written
  /// above.
  fn rebase(group: &mut [u8], count: usize) {
    for index in 0..count {
      let offset = 6 + index * 14 + 12;
      if let Some(slot) = group.get_mut(offset..offset + 2) {
        slot.copy_from_slice(&(FIRST_ICON_ID + index as u16).to_le_bytes());
      }
    }
  }

  /// The icon group and its images, out of the application executable.
  fn read_icon(exe: &Path) -> Option<(Vec<u8>, Vec<Vec<u8>>)> {
    let wide = to_wide(exe);
    let module = unsafe { LoadLibraryExW(PCWSTR(wide.as_ptr()), None, LOAD_LIBRARY_AS_DATAFILE) }.ok()?;
    let group = find(module, RT_GROUP_ICON, APP_ICON).or_else(|| {
      LEGACY_ICON_IDS
        .into_iter()
        .find_map(|id| find(module, RT_GROUP_ICON, id))
    });
    let result = group.and_then(|group| {
      if group.len() < 6 {
        return None;
      }
      let count = u16::from_le_bytes([group[4], group[5]]) as usize;
      if count == 0 || group.len() < 6 + count * 14 {
        return None;
      }
      let images = (0..count)
        .map(|index| {
          let entry = &group[6 + index * 14..6 + (index + 1) * 14];
          let size = u32::from_le_bytes([entry[8], entry[9], entry[10], entry[11]]) as usize;
          let id = u16::from_le_bytes([entry[12], entry[13]]);
          let data = find(module, RT_ICON, id)?;
          data.get(..size).map(<[u8]>::to_vec)
        })
        .collect::<Option<Vec<_>>>()?;
      Some((group, images))
    });
    let _ = unsafe { FreeLibrary(module) };
    result
  }

  fn read_version(exe: &Path) -> Option<Vec<u8>> {
    let wide = to_wide(exe);
    let module = unsafe { LoadLibraryExW(PCWSTR(wide.as_ptr()), None, LOAD_LIBRARY_AS_DATAFILE) }.ok()?;
    let version = find(module, RT_VERSION, 1);
    let _ = unsafe { FreeLibrary(module) };
    version
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

  /// `(id, language)` pairs of one resource type in `module`.
  fn entries(module: HMODULE, kind: u16) -> Vec<(u16, u16)> {
    let ty = PCWSTR(kind as usize as *const u16);
    let mut ids: Vec<u16> = Vec::new();
    let _ = unsafe {
      EnumResourceNamesW(
        Some(module),
        ty,
        Some(collect_name),
        &mut ids as *mut Vec<u16> as isize,
      )
    };
    let mut pairs = Vec::new();
    for id in ids {
      let mut languages: Vec<u16> = Vec::new();
      let _ = unsafe {
        EnumResourceLanguagesW(
          Some(module),
          ty,
          PCWSTR(id as usize as *const u16),
          Some(collect_language),
          &mut languages as *mut Vec<u16> as isize,
        )
      };
      pairs.extend(languages.into_iter().map(|language| (id, language)));
    }
    pairs
  }

  unsafe extern "system" fn collect_name(
    _module: HMODULE,
    _kind: PCWSTR,
    name: PCWSTR,
    lparam: isize,
  ) -> BOOL {
    let ids = unsafe { &mut *(lparam as *mut Vec<u16>) };
    ids.push(name.0 as usize as u16);
    true.into()
  }

  unsafe extern "system" fn collect_language(
    _module: HMODULE,
    _kind: PCWSTR,
    _name: PCWSTR,
    language: u16,
    lparam: isize,
  ) -> BOOL {
    let languages = unsafe { &mut *(lparam as *mut Vec<u16>) };
    languages.push(language);
    true.into()
  }

  /// Identifies the pair of files the marker describes.
  fn stamp(binary: &Path, app_exe: &Path) -> String {
    let label = |path: &Path| {
      let meta = std::fs::metadata(path).ok();
      let modified = meta
        .as_ref()
        .and_then(|meta| meta.modified().ok())
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
      format!("{}:{}", meta.map(|meta| meta.len()).unwrap_or_default(), modified)
    };
    format!("{} {}", label(binary), label(app_exe))
  }

  fn to_wide(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str().encode_wide().chain([0]).collect()
  }
}
