// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! The languages the user asked their operating system for, as Chromium wants
//! them.
//!
//! Chromium sends `Accept-Language` derived from its own locale, and Electron
//! picks the system locale by default — but only the one the Chromium shutdown
//! path reports. Spelling the list out keeps `Accept-Language` in the user's
//! preference order and lets a page branch on `navigator.language` the same way
//! a browser would.

/// The user's preferred languages as a comma-delimited list of BCP-47 codes, or
/// [`None`] when the system does not say.
///
/// The result is appended as `--lang=` unless the application set its own.
pub(crate) fn system_accept_language_list() -> Option<String> {
  let primary = preferred_language()?;
  let mut list = vec![primary.clone()];
  if let Some((base, _)) = primary.split_once('-') {
    list.push(base.to_string());
  }
  Some(list.join(","))
}

#[cfg(windows)]
fn preferred_language() -> Option<String> {
  use windows::Win32::Globalization::GetUserDefaultLocaleName;
  // LOCALE_NAME_MAX_LENGTH
  let mut buffer = [0u16; 85];
  let len = unsafe { GetUserDefaultLocaleName(&mut buffer) };
  if len <= 1 {
    return None;
  }
  let name = String::from_utf16_lossy(&buffer[..len as usize - 1]);
  // `C`/`POSIX` mean "no preference"; asking a server for them is worse than
  // asking for nothing.
  (!name.is_empty() && name != "C" && name != "POSIX").then_some(name)
}

#[cfg(not(windows))]
fn preferred_language() -> Option<String> {
  let raw = std::env::var("LC_ALL")
    .or_else(|_| std::env::var("LC_MESSAGES"))
    .or_else(|_| std::env::var("LANG"))
    .ok()?;
  // `pt_BR.UTF-8@euro` -> `pt-BR`
  let tag = raw.split('.').next().unwrap_or_default().replace('_', "-");
  let tag = tag.split('@').next().unwrap_or_default().trim().to_string();
  (!tag.is_empty() && tag != "C" && tag != "POSIX").then_some(tag)
}
