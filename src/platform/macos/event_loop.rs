// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! Event-loop-level operations on macOS.
//!
//! The reference implements these on winit's `ActiveEventLoop` and reaches
//! AppKit directly. Electron owns the loop, so the receiver is this crate's
//! [`NativeEventLoop`] stand-in; the operations that are pure AppKit still work,
//! and the ones that need the application's own bookkeeping report.

use objc2::MainThreadMarker;
use objc2_app_kit::{NSApp, NSApplication};
use tauri_runtime::{Error, ProgressBarState, Result, dpi::PhysicalPosition};

use crate::platform::{EventLoopExt, NativeEventLoop};

use super::{application, dock, progress};

impl EventLoopExt for dyn NativeEventLoop + '_ {
  fn set_activation_policy(&self, policy: tauri_runtime::ActivationPolicy) {
    let Some(mtm) = MainThreadMarker::new() else {
      return;
    };

    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(application::activation_policy(policy));
  }

  /// Shows or hides the Dock icon.
  ///
  /// Electron implements the transform and the duplicate-icon workaround inside
  /// its own process; `app.dockVisibility` is the path that reaches it.
  fn set_dock_visibility(&self, visible: bool) {
    dock::set_dock_visibility(visible);
  }

  fn show_application(&self) {
    let Some(mtm) = MainThreadMarker::new() else {
      return;
    };

    NSApp(mtm).unhide(None);
  }

  fn hide_application(&self) {
    let Some(mtm) = MainThreadMarker::new() else {
      return;
    };

    NSApp(mtm).hide(None);
  }

  fn set_progress_bar(&self, state: ProgressBarState) {
    progress::set_dock_progress_bar(state);
  }

  fn set_badge_count(&self, count: Option<i64>, _desktop_filename: Option<String>) {
    self.set_badge_label(count.map(|count| count.to_string()));
  }

  /// Sets the Dock tile badge label.
  ///
  /// The reference writes `NSDockTile.badgeLabel` directly. Electron's
  /// `app.badgeLabel` is the same call, made from the process that owns the Dock
  /// tile — `NSDockTile` is not among the AppKit bindings this crate enables.
  fn set_badge_label(&self, _label: Option<String>) {
    crate::unsupported::report("EventLoopExt::set_badge_label (Electron: app.badgeLabel)");
  }

  /// The cursor position in physical pixels.
  ///
  /// The reference reads `NSEvent.mouseLocation`, flips it against the primary
  /// screen and scales it to physical pixels. The flip needs `NSScreen`, which is
  /// not among the AppKit bindings this crate enables, and Electron reports the
  /// finished value from `window.cursorPosition` — so this path cannot answer and
  /// says so instead of returning a half-converted position.
  fn cursor_position(&self) -> Result<PhysicalPosition<f64>> {
    crate::unsupported::report("EventLoopExt::cursor_position (Electron: window.cursorPosition)");
    Err(Error::FailedToGetCursorPosition)
  }
}
