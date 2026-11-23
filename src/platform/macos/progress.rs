// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! The Dock tile progress bar.
//!
//! The reference subclasses `NSProgressIndicator` and draws the bar itself, because the Dock
//! tile has no progress API. Electron's `window.progressBar` is the same design — its
//! `NSProgressIndicator` subclass lives in `native_window_mac.mm` — so the drawing code has
//! no counterpart here: the runtime sends the state over `window.progressBar` and Electron
//! owns the tile.

use tauri_runtime::ProgressBarState;

/// Sets the progress shown on the application's Dock tile.
///
/// `state.progress` is a percentage; `state.status` selects the bar colour. Electron applies
/// both, and hides the bar when the status is [`tauri_runtime::ProgressBarStatus::None`].
pub fn set_dock_progress_bar(_state: ProgressBarState) {
  crate::unsupported::report(
    "platform::macos::set_dock_progress_bar (Electron: window.progressBar)",
  );
}
