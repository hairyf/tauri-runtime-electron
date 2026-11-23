// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! The diagnostic environment variables Chromium and Electron read behind the application's
//! back, and the policy that decides whether they are honoured.
//!
//! Chromium is a browser, and a browser is expected to let whoever runs it capture its own
//! traffic and redirect its own crash reports. An application that embeds Chromium inherits
//! those hooks without asking for them, and in a shipped application they are not developer
//! conveniences any more: anyone who can set a variable in the application's environment can
//! decrypt every TLS session it makes, or point its minidumps — which carry process memory —
//! at a server of their choosing. Electron adds a second set of hooks on top, because the
//! process it starts is also a Node process: `NODE_OPTIONS` runs arbitrary JavaScript in the
//! main process before the application does, and `ELECTRON_RUN_AS_NODE` turns the binary
//! into a bare Node interpreter that never reaches the application at all.
//!
//! [`DebugEnvironment`] is the switch. Neither variable group is reachable through an
//! Electron setting, so each is answered where Chromium or Electron reads it.
//!
//! # TLS key logging is answered on the command line
//!
//! `content/browser/network_service_instance_impl.cc` consults `SSLKEYLOGFILE` only when
//! `--ssl-key-log-file` is absent, and an empty switch value logs a warning and creates no
//! key logger. Appending the empty switch to the Electron command line is therefore a
//! complete answer that needs no change to the process environment, and the runtime appends
//! it only when the variable is actually set — so the warning appears exactly when somebody
//! was trying to log keys. Electron is Chromium, so the rule is the same one CEF follows.
//!
//! # The crash reporter and Node overrides are answered in the environment
//!
//! Electron's main process reads its overrides from the environment before any application
//! code runs, so there is nothing to append and the variables have to go. That write is
//! [`std::env::remove_var`], which is why it happens once, during runtime initialization,
//! before the Electron child is spawned — the child inherits the browser process's
//! environment, so a variable removed here is gone from the whole process tree.

/// Whether Chromium and Electron may read their diagnostic environment variables.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DebugEnvironment {
  /// Honour them in development builds (`tauri::is_dev()`), refuse them in release
  /// builds.
  #[default]
  Auto,
  /// Always honour them.
  ///
  /// Appropriate for a build whose users are expected to debug it — an internal tool, a
  /// QA build — and wrong for one handed to the public.
  Allow,
  /// Always refuse them, in every build profile.
  Deny,
}

/// The environment variable whose answer is the `--ssl-key-log-file` switch.
///
/// The network service writes the pre-master secret of every TLS session to the file it
/// names, which is exactly what a packet capture needs to decrypt the application's
/// traffic in full.
const SSL_KEY_LOG_FILE: &str = "SSLKEYLOGFILE";

/// The Electron and Node overrides, and what each one does.
///
/// * `ELECTRON_ENABLE_LOGGING` — turns Chromium's verbose logging on from the
///   environment, which no shipped application asked for.
/// * `ELECTRON_ENABLE_STACK_DUMPING` — dumps a native stack trace of every process to
///   stderr, which is a map of the process for whoever reads it.
/// * `ELECTRON_DEFAULT_ERROR_MODE` — replaces Electron's own crash handler with the
///   Windows error dialog, which blocks the process instead of reporting the crash.
/// * `NODE_OPTIONS` — runs arbitrary JavaScript in the main process before the
///   application starts, which is code execution with the user's full privileges.
/// * `ELECTRON_RUN_AS_NODE` — turns the Electron binary into a plain Node interpreter,
///   so the application never starts and whatever the variable's caller wants runs instead.
/// * `BREAKPAD_DUMP_LOCATION` — redirects where minidumps are written on Windows. A
///   minidump carries stack and heap memory, so a redirected location is a memory
///   exfiltration channel.
///
/// The CEF-only pair the reference removes (`CEF_CRASH_REPORTER_SERVER_URL` and
/// `CEF_CRASH_REPORTER_RATE_LIMIT_ENABLED`) has no meaning here: Electron's crash reporter
/// takes its upload URL from the application's own configuration, not from the environment.
const CRASH_REPORTER_VARIABLES: &[(&str, &str)] = &[
  (
    "ELECTRON_ENABLE_LOGGING",
    "turning on Chromium's verbose logging from the environment",
  ),
  (
    "ELECTRON_ENABLE_STACK_DUMPING",
    "dumping a native stack trace of every process to stderr",
  ),
  (
    "ELECTRON_DEFAULT_ERROR_MODE",
    "replacing Electron's own crash handler with the Windows error dialog",
  ),
  (
    "NODE_OPTIONS",
    "running arbitrary Node code in the main process before the application starts",
  ),
  (
    "ELECTRON_RUN_AS_NODE",
    "turning the Electron binary into a bare Node process that never reaches the application",
  ),
  (
    "BREAKPAD_DUMP_LOCATION",
    "redirecting where minidumps are written",
  ),
];

/// Whether `policy` refuses the variables in a build where `is_dev` says what it says.
///
/// Kept separate from acting on the answer so the decision can be unit tested without
/// touching the process environment.
fn refuses_debug_variables(policy: DebugEnvironment, is_dev: bool) -> bool {
  match policy {
    DebugEnvironment::Auto => !is_dev,
    DebugEnvironment::Allow => false,
    DebugEnvironment::Deny => true,
  }
}

/// Whether a variable is set to something Chromium would act on.
fn is_set(name: &str) -> bool {
  std::env::var_os(name).is_some_and(|value| !value.is_empty())
}

/// Whether the runtime should append an empty `--ssl-key-log-file`, which is how
/// `SSLKEYLOGFILE` is refused.
///
/// Answers `false` when the variable is not set, so the switch — and the warning Chromium
/// logs for it — only appears when something was actually asking for a key log.
pub(crate) fn neutralizes_tls_key_log(policy: DebugEnvironment, is_dev: bool) -> bool {
  if !refuses_debug_variables(policy, is_dev) || !is_set(SSL_KEY_LOG_FILE) {
    return false;
  }

  log::warn!(
    "ignoring the {SSL_KEY_LOG_FILE} environment variable: it asks Chromium to log the TLS \
     session keys that decrypt this application's network traffic. Set \
     DebugEnvironment::Allow to honour it."
  );
  true
}

/// Removes the Electron and Node overrides from the process environment when `policy`
/// refuses them.
///
/// The name is the reference's; the list is Electron's, because the variables that can
/// redirect a crash report and the ones that can run code in the main process are answered
/// the same way.
///
/// Must run before the Electron child is spawned: the child inherits the environment of
/// the process that spawned it, and the main process reads these before any application
/// code runs, so a removal after the spawn has no effect at all.
///
/// # Safety
///
/// Calls [`std::env::remove_var`], which is unsound while another thread reads or writes
/// the environment concurrently. The caller must be the runtime's own initialization,
/// which runs on the main thread before the Electron child exists and before this runtime
/// starts a thread of its own.
pub(crate) fn remove_crash_reporter_overrides(policy: DebugEnvironment, is_dev: bool) {
  if !refuses_debug_variables(policy, is_dev) {
    return;
  }

  for (name, effect) in CRASH_REPORTER_VARIABLES {
    if !is_set(name) {
      continue;
    }

    log::warn!(
      "ignoring the {name} environment variable: it asks Electron for {effect}. Set \
       DebugEnvironment::Allow to honour it."
    );
    // SAFETY: documented on this function; the runtime applies the policy from its own
    // initialization, before the Electron child is spawned and before it starts a thread
    // of its own.
    unsafe { std::env::remove_var(name) };
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn auto_follows_the_build_profile() {
    assert!(
      !refuses_debug_variables(DebugEnvironment::Auto, true),
      "a development build keeps its debugging hooks"
    );
    assert!(
      refuses_debug_variables(DebugEnvironment::Auto, false),
      "a shipped build must not hand its TLS keys to whoever sets a variable"
    );
  }

  #[test]
  fn explicit_policies_ignore_the_build_profile() {
    for is_dev in [false, true] {
      assert!(!refuses_debug_variables(DebugEnvironment::Allow, is_dev));
      assert!(refuses_debug_variables(DebugEnvironment::Deny, is_dev));
    }
  }

  #[test]
  fn a_permissive_policy_never_touches_the_command_line() {
    // Asserted without depending on the ambient environment: `Allow` short-circuits
    // before the variable is even read.
    assert!(!neutralizes_tls_key_log(DebugEnvironment::Allow, false));
    assert!(!neutralizes_tls_key_log(DebugEnvironment::Auto, true));
  }

  #[test]
  fn every_crash_variable_is_described() {
    // The name and the effect both go into a warning the user reads.
    for (name, effect) in CRASH_REPORTER_VARIABLES {
      assert!(!name.is_empty());
      assert!(!effect.is_empty());
    }
  }

  #[test]
  fn the_tls_variable_is_not_also_removed_from_the_environment() {
    // It is answered on the command line instead, which needs no environment write.
    assert!(
      !CRASH_REPORTER_VARIABLES
        .iter()
        .any(|(name, _)| *name == SSL_KEY_LOG_FILE)
    );
  }

  #[test]
  fn the_cef_only_overrides_are_not_removed_here() {
    // Electron never reads them, so removing them would be a warning about nothing.
    for cef_only in [
      "CEF_CRASH_REPORTER_SERVER_URL",
      "CEF_CRASH_REPORTER_RATE_LIMIT_ENABLED",
    ] {
      assert!(
        !CRASH_REPORTER_VARIABLES
          .iter()
          .any(|(name, _)| *name == cef_only),
        "{cef_only} is not an Electron variable"
      );
    }
  }
}
