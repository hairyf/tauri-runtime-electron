// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! Event-loop-level operations on Linux/BSD.
//!
//! The reference implements these on winit's `ActiveEventLoop` and reaches the X server
//! directly. Electron owns the loop and the X connection, so the receiver is this crate's
//! [`NativeEventLoop`] stand-in and the operations report where the X server would have been
//! asked.

use tauri_runtime::{Error, Result, dpi::PhysicalPosition};

use crate::platform::{EventLoopExt, NativeEventLoop};

use super::taskbar;

impl EventLoopExt for dyn NativeEventLoop + '_ {
  /// Sets the launcher badge count.
  ///
  /// Electron's `window.badgeCount` drives the same Unity launcher entry, with `libunity`
  /// loaded by Electron instead of by this crate.
  fn set_badge_count(&self, count: Option<i64>, desktop_filename: Option<String>) {
    taskbar::set_badge_count(count, desktop_filename);
  }

  fn set_badge_label(&self, _label: Option<String>) {
    // Unsupported on Linux/BSD
  }

  /// The cursor position in physical pixels.
  ///
  /// The reference asks the X server with `XQueryPointer`. This crate opens no X connection —
  /// Electron owns the display — and Electron reports the finished value from
  /// `window.cursorPosition`, so this path cannot answer and says so.
  fn cursor_position(&self) -> Result<PhysicalPosition<f64>> {
    crate::unsupported::report("EventLoopExt::cursor_position (Electron: window.cursorPosition)");
    Err(Error::FailedToGetCursorPosition)
  }
}
