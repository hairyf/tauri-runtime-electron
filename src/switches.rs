// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! Helpers for the Chromium command line the runtime hands to Electron.
//!
//! Chromium stores switches in a map keyed by name, so appending one that is
//! already there overwrites the previous value. That is what an embedder wants
//! for most switches — the application's value wins over the runtime default —
//! but it is a trap for the comma-delimited lists Chromium fills in itself.

/// Switches whose value Chromium has already set by the time the application's
/// own switches are appended, and which therefore replace rather than extend.
pub(crate) const REPLACING_SWITCHES: &[&str] = &["disable-features", "enable-features", "js-flags", "lang"];

/// Chromium switches that turn off a security boundary, mirroring the list
/// Chrome warns about in `chrome/browser/ui/startup/bad_flags_prompt.cc`.
///
/// Advisory only: the runtime still appends whatever the application asked for.
const DANGEROUS_SWITCHES: &[&str] = &[
  "disable-web-security",
  "no-sandbox",
  "allow-running-insecure-content",
  "disable-site-isolation-trials",
  "disable-features-site-per-process",
];

fn switch_name(arg: &str) -> Option<&str> {
  arg.strip_prefix("--")?.split('=').next()
}

fn switch_value(arg: &str) -> Option<&str> {
  arg.strip_prefix("--")?.split_once('=').map(|(_, value)| value)
}

/// Appends `--name=value`, merging it with a value already present.
///
/// Chromium's `--disable-features` list is filled before the application's own
/// switches are appended, and an application that appends its own would
/// silently drop it. Merging keeps both.
pub(crate) fn append_merged_switch(args: &mut Vec<String>, name: &str, value: &str) {
  if let Some(index) = args.iter().rposition(|arg| switch_name(arg) == Some(name)) {
    let existing = switch_value(&args[index]).unwrap_or_default();
    let merged = if existing.is_empty() {
      value.to_string()
    } else if value.is_empty() {
      existing.to_string()
    } else {
      format!("{existing},{value}")
    };
    args[index] = format!("--{name}={merged}");
    return;
  }
  args.push(format!("--{name}={value}"));
}

/// Prints a warning for each documented-dangerous switch the application asked
/// for, so the flag shows up in the logs instead of in a CVE.
pub(crate) fn warn_about_dangerous_switches(args: &[String]) {
  for arg in args {
    if let Some(name) = switch_name(arg) {
      if DANGEROUS_SWITCHES.contains(&name) {
        eprintln!("[electron] WARNING: --{name} turns off a security boundary");
      }
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn merges_into_existing_value() {
    let mut args = vec!["--disable-features=Foo".to_string()];
    append_merged_switch(&mut args, "disable-features", "Bar");
    assert_eq!(args, ["--disable-features=Foo,Bar"]);
  }

  #[test]
  fn appends_when_absent() {
    let mut args = Vec::new();
    append_merged_switch(&mut args, "disable-features", "Bar");
    assert_eq!(args, ["--disable-features=Bar"]);
  }
}
