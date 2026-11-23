// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! Helpers for the Chromium command line the runtime hands to Electron.
//!
//! Two problems live here.
//!
//! # A switch value replaces, it does not accumulate
//!
//! Chromium's `CommandLine::AppendSwitchNative` stores switches in a map keyed by name,
//! so appending a switch that is already there overwrites the previous value; nothing in
//! Chrome, Electron or Electron's patches installs a `DuplicateSwitchHandler` that would
//! merge them instead. For most switches that is what an embedder wants — the runtime
//! relies on it so an application's own switch wins over a runtime default.
//!
//! For the comma-delimited ones it is a trap. The runtime fills `--disable-features`
//! before it appends the application's own switches, with a list that keeps Chromium from
//! accepting switches from the process command line in a release build, and an application
//! that appends its own `--disable-features` silently drops all of it. The same map is
//! behind `app.commandLine.appendSwitch` in the main process, so replaying the switches
//! there has exactly the same behaviour. [`append_merged_switch`] is the merging append
//! those switches need, and [`REPLACING_SWITCHES`] is the list an application is warned
//! about when it reaches for the raw API instead.
//!
//! # Some switches turn security off
//!
//! Chrome itself keeps a list of command line flags that "stability and security will
//! suffer" from and shows an infobar naming them. A shipped Tauri application ignores its
//! process command line entirely (see `Electron::allow_chromium_command_line_args`), so
//! the only way one of those flags reaches Chromium is the application putting it there.
//! [`warn_about_dangerous_switches`] says so out loud.

/// Switches whose value Chromium or Electron has already set by the time the application's
/// own switches are appended, and which therefore replace rather than extend.
///
/// Each entry names the API that sets the same thing without dropping what is there.
pub(crate) const REPLACING_SWITCHES: &[(&str, &str)] = &[
  ("disable-features", "Electron::disable_features"),
  ("enable-features", "Electron::enable_features"),
  ("js-flags", "Electron::javascript_flags"),
];

/// Chromium switches that turn off a security boundary, mirroring the list Chrome warns
/// about in `chrome/browser/ui/startup/bad_flags_prompt.cc`.
///
/// Advisory only: the runtime still appends whatever the application asked for. A name
/// that a future Chromium renames simply stops matching, which costs a warning and
/// nothing else.
const DANGEROUS_SWITCHES: &[&str] = &[
  // Web platform boundaries.
  "disable-web-security",
  "allow-running-insecure-content",
  "ignore-certificate-errors",
  "ignore-certificate-errors-spki-list",
  "unsafely-treat-insecure-origin-as-secure",
  "allow-insecure-localhost",
  "disable-site-isolation-trials",
  "enable-blink-features",
  "disable-blink-features",
  "enable-unsafe-webgpu",
  "disable-hid-blocklist",
  "unsafely-allow-protected-media-identifier-for-domain",
  // Process sandbox.
  "no-sandbox",
  "disable-gpu-sandbox",
  "disable-setuid-sandbox",
  "disable-seccomp-filter-sandbox",
  "disable-namespace-sandbox",
  "disable-landlock-sandbox",
  "disable-webnn-compiler-sandbox",
  "allow-sandbox-debugging",
  "allow-third-party-modules",
  "single-process",
  // Traffic capture and redirection.
  "host-resolver-rules",
  "host-rules",
  "ssl-key-log-file",
  "log-net-log",
  "net-log-capture-mode",
  // Media and input.
  "disable-webrtc-encryption",
  "use-fake-ui-for-media-stream",
  "enable-speech-dispatcher",
  "enable-gpu-benchmarking",
  // Electron's own Node side: these open a debugger on the main process, which runs with
  // the user's full privileges and can execute arbitrary code there. `--remote-debugging-port`
  // is deliberately absent — the runtime sets it itself through `RemoteDebugging`, so
  // warning about it would fire on the runtime's own switch rather than the application's.
  "inspect",
  "inspect-brk",
];

/// Splits a comma-delimited switch value, dropping the empty entries Chromium ignores.
fn split_list(value: &str) -> impl Iterator<Item = &str> {
  value
    .split(',')
    .map(str::trim)
    .filter(|part| !part.is_empty())
}

/// The value a merging append should write, given what the switch already holds.
///
/// Returns [`None`] when there would be nothing to write, so the caller leaves the
/// command line alone rather than appending an empty switch.
///
/// `values` entries may themselves be comma-delimited, so a caller that collected
/// `["A,B", "C"]` gets the same result as one that collected `["A", "B", "C"]`.
fn merged_switch_value(existing: &str, values: &[String]) -> Option<String> {
  let mut merged: Vec<&str> = split_list(existing).collect();

  for value in values.iter().flat_map(|value| split_list(value)) {
    if !merged.contains(&value) {
      merged.push(value);
    }
  }

  (!merged.is_empty()).then(|| merged.join(","))
}

/// Merges `values` into the comma-delimited switch `name` already on `args`, preserving
/// what is there and skipping duplicates.
///
/// Chromium reads the *last* value appended for a switch, and there is no API to edit one
/// in place, so the merge is read, remove, append — over the argv the runtime is about to
/// hand the Electron child, which is the same map `app.commandLine` builds in the main
/// process.
pub(crate) fn append_merged_switch(args: &mut Vec<String>, name: &str, values: &[String]) {
  if values.is_empty() {
    return;
  }

  // The last element naming the switch is the value Chromium would use.
  let existing = args
    .iter()
    .rev()
    .find_map(|argument| switch_value(argument, name))
    .unwrap_or_default()
    .to_owned();

  let Some(merged) = merged_switch_value(&existing, values) else {
    return;
  };

  // Appending alone would leave the old spelling in argv beside the new one, and the two
  // would disagree for anything that re-parses the command line; removing first keeps one
  // spelling per switch.
  args.retain(|argument| switch_key(argument) != name);
  args.push(format!("--{name}={merged}"));
}

/// The value `argument` gives the switch `name`, when it names it.
fn switch_value<'a>(argument: &'a str, name: &str) -> Option<&'a str> {
  let (key, value) = argument.trim_start_matches('-').split_once('=')?;
  (key == name).then_some(value)
}

/// Strips the `-`/`--` prefix Chromium tolerates on a switch name, and the `=value` an
/// argv element carries, so a switch spelled either way is recognised.
fn switch_key(argument: &str) -> &str {
  argument
    .trim_start_matches('-')
    .split('=')
    .next()
    .unwrap_or_default()
}

/// Warns about application switches that replace a value the runtime or Electron depends
/// on.
///
/// The reference calls this from the runtime's command-line assembly, once the application's
/// own switches are known. This runtime merges the two feature switches itself and appends
/// the application's switches verbatim, so the crate has no call site for it; it is kept
/// because the warning is the right answer for any caller that does hand an application's
/// raw switch list over.
#[allow(dead_code)]
pub(crate) fn warn_about_replacing_switches(args: &[String]) {
  for argument in args {
    let key = switch_key(argument);
    if let Some((_, replacement)) = REPLACING_SWITCHES.iter().find(|(name, _)| *name == key) {
      log::warn!(
        "the --{key} switch replaces the value Chromium and Electron already set rather than \
         adding to it, which drops entries the runtime depends on. Use {replacement} instead."
      );
    }
  }
}

/// Warns about application switches that turn off a security boundary.
#[allow(dead_code)]
pub(crate) fn warn_about_dangerous_switches(args: &[String]) {
  for argument in args {
    let key = switch_key(argument);
    if DANGEROUS_SWITCHES.contains(&key) {
      log::warn!(
        "the --{key} switch turns off a Chromium security boundary. Chrome itself warns \
         its users when it is set; do not ship it."
      );
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(ToString::to_string).collect()
  }

  #[test]
  fn a_list_is_split_the_way_chromium_splits_it() {
    let parts: Vec<_> = split_list("A,,B , C,").collect();
    assert_eq!(parts, ["A", "B", "C"]);
  }

  fn merge(existing: &str, values: &[&str]) -> Option<String> {
    merged_switch_value(existing, &strings(values))
  }

  #[test]
  fn merging_keeps_what_electron_already_disabled() {
    // The whole point: the runtime's own crash-avoidance entry survives the application's.
    assert_eq!(
      merge("AllowCommandLineSwitchesFromProcess", &["MediaRouter"]).as_deref(),
      Some("AllowCommandLineSwitchesFromProcess,MediaRouter")
    );
  }

  #[test]
  fn merging_onto_an_unset_switch_writes_only_the_new_values() {
    assert_eq!(merge("", &["A", "B"]).as_deref(), Some("A,B"));
  }

  #[test]
  fn a_value_already_present_is_not_repeated() {
    assert_eq!(
      merge("A,B", &["B", "C", "B"]).as_deref(),
      Some("A,B,C"),
      "a duplicate would be harmless to Chromium but makes the switch unreadable"
    );
  }

  #[test]
  fn entries_may_themselves_be_comma_delimited() {
    assert_eq!(merge("A", &["B,C", "D"]).as_deref(), Some("A,B,C,D"));
  }

  #[test]
  fn nothing_to_write_leaves_the_command_line_alone() {
    // An empty switch value is not the same as an absent switch, so it must not be
    // appended: `--disable-features=` reads as "disable nothing named".
    assert_eq!(merge("", &[""]), None);
    assert_eq!(merge("", &[]), None);
  }

  #[test]
  fn appending_merges_into_the_switch_already_on_the_command_line() {
    let mut args = strings(&[
      "--no-first-run",
      "--disable-features=AllowCommandLineSwitchesFromProcess",
    ]);
    append_merged_switch(&mut args, "disable-features", &strings(&["MediaRouter"]));
    assert_eq!(
      args,
      [
        "--no-first-run",
        "--disable-features=AllowCommandLineSwitchesFromProcess,MediaRouter"
      ]
    );
  }

  #[test]
  fn appending_leaves_one_spelling_of_the_switch() {
    // Chromium reads the last value appended, so a leftover spelling would be dead weight
    // that re-parsers still see — and the value it would read is the one merged here, not
    // the first spelling's.
    let mut args = strings(&[
      "--disable-features=A",
      "--disable-features=B",
      "--no-first-run",
    ]);
    append_merged_switch(&mut args, "disable-features", &strings(&["C"]));
    assert_eq!(args, ["--no-first-run", "--disable-features=B,C"]);
  }

  #[test]
  fn appending_onto_an_unset_switch_adds_it() {
    let mut args = strings(&["--no-first-run"]);
    append_merged_switch(
      &mut args,
      "enable-features",
      &strings(&["OverlayScrollbar"]),
    );
    assert_eq!(
      args,
      ["--no-first-run", "--enable-features=OverlayScrollbar"]
    );
  }

  #[test]
  fn appending_nothing_leaves_the_command_line_alone() {
    let mut args = strings(&["--no-first-run"]);
    append_merged_switch(&mut args, "disable-features", &[]);
    append_merged_switch(&mut args, "disable-features", &strings(&[""]));
    assert_eq!(args, ["--no-first-run"]);
  }

  #[test]
  fn switch_names_are_recognised_with_or_without_a_prefix() {
    assert_eq!(switch_key("--disable-features"), "disable-features");
    assert_eq!(switch_key("-disable-features"), "disable-features");
    assert_eq!(switch_key("disable-features"), "disable-features");
    // An argv element carries its value in the same string.
    assert_eq!(switch_key("--disable-features=A,B"), "disable-features");
    assert_eq!(
      switch_value("--disable-features=A,B", "disable-features"),
      Some("A,B")
    );
    assert_eq!(switch_value("--disable-features", "disable-features"), None);
    assert_eq!(switch_value("--no-sandbox", "disable-features"), None);
  }

  #[test]
  fn every_replacing_switch_names_a_replacement() {
    for (name, replacement) in REPLACING_SWITCHES {
      assert!(!name.is_empty());
      assert!(
        replacement.starts_with("Electron::"),
        "the warning tells the user what to call instead"
      );
    }
  }

  #[test]
  fn the_two_lists_answer_different_questions() {
    // `disable-features` replaces the runtime's crash-avoidance list, which is a
    // correctness problem rather than a security one, so it is warned about but not called
    // dangerous.
    assert!(
      REPLACING_SWITCHES
        .iter()
        .any(|(name, _)| *name == "disable-features")
    );
    assert!(!DANGEROUS_SWITCHES.contains(&"disable-features"));

    // The blink feature switches turn boundaries off but overwrite nothing the runtime
    // set, so they are only on the dangerous list.
    assert!(DANGEROUS_SWITCHES.contains(&"enable-blink-features"));
    assert!(
      !REPLACING_SWITCHES
        .iter()
        .any(|(name, _)| *name == "enable-blink-features")
    );
  }

  #[test]
  fn the_dangerous_list_covers_chromiums_boundaries_and_electrons_debuggers() {
    // One name from each group, so a group cannot be dropped silently.
    for name in [
      "disable-web-security",
      "no-sandbox",
      "ssl-key-log-file",
      "enable-gpu-benchmarking",
      "inspect",
    ] {
      assert!(DANGEROUS_SWITCHES.contains(&name), "{name}");
    }
    // The runtime sets this one itself, so it must not be reported as the application's.
    assert!(!DANGEROUS_SWITCHES.contains(&"remote-debugging-port"));
  }
}
