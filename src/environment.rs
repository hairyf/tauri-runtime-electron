// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! The diagnostic environment variables Chromium and Electron read behind the
//! application's back, and the policy that decides whether they are honoured.
//!
//! A browser is expected to let whoever runs it capture its own traffic and
//! redirect its own crash reports. An application that embeds Chromium inherits
//! those hooks without asking for them, and in a shipped application they are
//! not developer conveniences any more: with `SSLKEYLOGFILE` set, every TLS
//! session the application makes can be decrypted from a packet capture, and
//! with `NODE_OPTIONS` or `ELECTRON_RUN_AS_NODE` set, the Electron process can
//! be turned into something else entirely.
//!
//! [`DebugEnvironment`] is the switch.

/// Whether Chromium and Electron may read their diagnostic environment
/// variables.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DebugEnvironment {
  /// Honour them in development builds, refuse them in release builds.
  #[default]
  Auto,
  /// Always honour them.
  Allow,
  /// Always refuse them, in every build profile.
  Deny,
}

impl DebugEnvironment {
  fn honours(self) -> bool {
    match self {
      Self::Auto => cfg!(debug_assertions),
      Self::Allow => true,
      Self::Deny => false,
    }
  }
}

/// Chromium's TLS key log variable.
///
/// The network service writes the pre-master secret of every TLS session to the
/// file it names, which is what a packet capture needs to decrypt the
/// application's traffic in full. Chromium consults it only when the
/// `--ssl-key-log-file` switch is absent, and an empty switch value logs a
/// warning and creates no key logger — so appending the empty switch is a
/// complete answer that needs no change to the process environment.
const SSL_KEY_LOG_FILE: &str = "SSLKEYLOGFILE";

/// Electron/Node read these before the embedder gets a say, so the only answer
/// is to take them out of the environment Electron is spawned with.
const DIAGNOSTIC_ENV_VARS: &[&str] = &["NODE_OPTIONS", "ELECTRON_RUN_AS_NODE", "ELECTRON_ENABLE_LOGGING"];

/// Applies `policy` before Electron is spawned.
///
/// Refusing the TLS key log needs a Chromium switch (see above); the Node and
/// Electron overrides are answered by removing them from this process's
/// environment, which the child inherits.
pub(crate) fn sanitize(policy: DebugEnvironment, args: &mut Vec<String>) {
  if policy.honours() {
    return;
  }
  if std::env::var_os(SSL_KEY_LOG_FILE).is_some() {
    args.push("--ssl-key-log-file".to_string());
    args.push(String::new());
  }
  for name in DIAGNOSTIC_ENV_VARS {
    if std::env::var_os(name).is_some() {
      eprintln!("[electron] ignoring {name}: set DebugEnvironment::Allow to honour it");
      std::env::remove_var(name);
    }
  }
}
