// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! Read-only startup diagnostics for an explicitly low-integrity Electron binary.
//! The runtime never changes file security attributes or disables the sandbox.

use std::{ffi::c_void, path::Path, ptr::null_mut};

use windows::{
  core::PCWSTR,
  Win32::{
    Foundation::{LocalFree, ERROR_SUCCESS, HLOCAL},
    Security::{
      Authorization::{GetNamedSecurityInfoW, SE_FILE_OBJECT},
      GetAce, GetSidSubAuthority, GetSidSubAuthorityCount, ACL, LABEL_SECURITY_INFORMATION,
      PSECURITY_DESCRIPTOR, PSID, SYSTEM_MANDATORY_LABEL_ACE,
    },
  },
};

/// `SYSTEM_MANDATORY_LABEL_ACE_TYPE`, absent from the generated bindings.
const SYSTEM_MANDATORY_LABEL_ACE_TYPE: u8 = 0x11;
/// `SECURITY_MANDATORY_MEDIUM_RID`, the level of a normal user process.
const MEDIUM_RID: u32 = 0x2000;

pub(crate) fn check_binary_label(path: &Path) -> Result<(), String> {
  match label_rid(path) {
    Ok(rid) => check_label(path, rid),
    Err(error) => {
      // An unreadable label is not evidence of a low label; preserve spawn errors.
      eprintln!("[electron] could not read the integrity level of {}: {error}", path.display());
      Ok(())
    }
  }
}

fn check_label(path: &Path, rid: Option<u32>) -> Result<(), String> {
  if let Some(rid) = rid.filter(|rid| *rid < MEDIUM_RID) {
    return Err(format!(
      "{} has a low mandatory integrity label (0x{rid:x} < 0x2000). \
       Electron startup was stopped without modifying file security attributes. \
       Review the distribution's provenance and deployment permissions with your administrator; \
       do not disable Chromium's sandbox. STATUS_BREAKPOINT (0x80000003) alone does not identify a cause.",
      path.display()
    ));
  }
  Ok(())
}

/// Returns the explicit label RID; an unlabeled file is treated as medium by Windows.
fn label_rid(path: &Path) -> Result<Option<u32>, String> {
  use std::os::windows::ffi::OsStrExt;
  let wide: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
  let mut sacl = null_mut::<ACL>();
  let mut descriptor = PSECURITY_DESCRIPTOR(null_mut());
  let status = unsafe {
    GetNamedSecurityInfoW(
      PCWSTR(wide.as_ptr()),
      SE_FILE_OBJECT,
      LABEL_SECURITY_INFORMATION,
      None,
      None,
      None,
      Some(&mut sacl),
      &mut descriptor,
    )
  };
  if status != ERROR_SUCCESS {
    return Err(format!("GetNamedSecurityInfoW failed with {}", status.0));
  }
  let result = unsafe { find_label_rid(sacl) };
  unsafe { LocalFree(Some(HLOCAL(descriptor.0))) };
  Ok(result)
}

/// SAFETY: `sacl` must point into a security descriptor owned by the caller.
unsafe fn find_label_rid(sacl: *mut ACL) -> Option<u32> {
  if sacl.is_null() {
    return None;
  }
  for index in 0..u32::from((*sacl).AceCount) {
    let mut ace = null_mut::<c_void>();
    if GetAce(sacl, index, &mut ace).is_err() || ace.is_null() {
      continue;
    }
    let label = ace as *const SYSTEM_MANDATORY_LABEL_ACE;
    if (*label).Header.AceType != SYSTEM_MANDATORY_LABEL_ACE_TYPE {
      continue;
    }
    let sid = PSID(std::ptr::addr_of!((*label).SidStart).cast_mut().cast());
    let count = u32::from(*GetSidSubAuthorityCount(sid));
    if count == 0 {
      continue;
    }
    return Some(*GetSidSubAuthority(sid, count - 1));
  }
  None
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn explicit_low_labels_fail_with_context_without_changing_security() {
    let path = Path::new("runtime/electron.exe");
    for rid in [None, Some(MEDIUM_RID), Some(0x3000)] {
      assert!(check_label(path, rid).is_ok());
    }
    for rid in [0, 0x1000, MEDIUM_RID - 1] {
      let error = check_label(path, Some(rid)).unwrap_err();
      assert!(error.contains("runtime/electron.exe"));
      assert!(error.contains(&format!("0x{rid:x}")));
      assert!(error.contains("without modifying file security attributes"));
    }
  }
}
