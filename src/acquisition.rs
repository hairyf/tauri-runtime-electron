//! Locates the Electron distribution at runtime, downloading it into the
//! user's cache on first launch when the build did not stage one.
//!
//! Packaged apps therefore ship without any Electron payload: the first run
//! fetches it (verifying the release checksum) and later runs reuse the cache.

use std::path::PathBuf;

#[path = "acquisition_shared.rs"]
mod shared;

const MAIN_SCRIPT: &str = include_str!("../electron/main.cjs");
const PRELOAD_SCRIPT: &str = include_str!("../electron/preload.cjs");

/// Returns an executable with `main.cjs`/`preload.cjs` beside it, downloading
/// the distribution when the machine has no cached copy yet.
pub(crate) fn ensure_installed() -> Result<PathBuf, String> {
  shared::install(MAIN_SCRIPT, PRELOAD_SCRIPT)
}

pub(crate) fn platform() -> String {
  shared::platform()
}

pub(crate) fn arch() -> String {
  shared::arch()
}
