// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! Window-level AppKit mechanisms.
//!
//! The reference implements these on the window it created, reaching the `NSWindow` through
//! the `NSView` winit made the content view. Electron's window is created in its own process
//! and reports its `NSView*` as `window.nativeHandle`, so the view is still reachable — but
//! the `NSWindow` behind it and every AppKit call the reference makes on it are Electron's
//! own: `window.enabled`, `window.trafficLight`, `window.backgroundColor`,
//! `window.visibleOnAllWorkspaces` and `window.titleBarStyle` are the RPCs the runtime uses.
//!
//! The `NSWindow`, `NSView` and `NSColor` bindings are not among the AppKit features this
//! crate enables, so what remains here is the handle plumbing and the reporting.

use std::ffi::c_void;

use tauri_runtime::dpi::Position;
use tauri_utils::{TitleBarStyle, config::Color};

use crate::platform::NativeWindow;

/// The `NSWindow` behind a window's content view.
///
/// The reference sends `-[NSView window]` to the content view winit created. Electron reports
/// the same `NSView*`, but reading its window needs the `NSWindow` binding this crate does not
/// enable, and nothing on this side needs the object: every window operation is an RPC.
pub(crate) fn nswindow(_window: &NativeWindow) -> Option<*mut c_void> {
  crate::unsupported::report("platform::macos::nswindow (no NSWindow binding)");
  None
}

impl NativeWindow {
  /// The handle handed to native APIs.
  ///
  /// CEF wraps the window handle in its own type; Electron reports the `NSView*` itself, so
  /// the indirection collapses to the same value as [`Self::nsview`].
  pub(crate) fn cef_host_handle(&self) -> *mut c_void {
    self.nsview()
  }

  /// The `NSView` Electron created for the window.
  pub(crate) fn nsview(&self) -> *mut c_void {
    self.as_ptr()
  }

  /// Enables or disables the window.
  ///
  /// The reference attaches a translucent sheet over the window, which is exactly what
  /// Electron's `window.enabled` does on macOS (`native_window_mac.mm`).
  pub(crate) fn set_enabled(&self, _enabled: bool) {
    crate::unsupported::report("AppWindow::set_enabled (Electron: window.enabled)");
  }

  pub(crate) fn is_enabled(&self) -> bool {
    crate::unsupported::report("AppWindow::is_enabled (Electron: window.enabledQuery)");
    true
  }

  /// Moves the traffic lights to `position`.
  ///
  /// Electron's `win.setWindowButtonPosition({x, y})` is the same inset, and it re-applies it
  /// itself on relayout — which is why the reference's observer is not needed here.
  pub(crate) fn set_traffic_light_position(&self, _position: &Position) {
    crate::unsupported::report(
      "AppWindow::set_traffic_light_position (Electron: window.trafficLight)",
    );
  }

  /// Restore the traffic light position after a window appearance change.
  ///
  /// Changing the appearance rebuilds the titlebar like a geometry change does, but posts no
  /// window notification, so the reference has to ask for the inset to be restored. Electron
  /// keeps the position in its own window state and re-applies it on relayout, so there is
  /// nothing to restore from here.
  pub(crate) fn reapply_traffic_light_position_after_appearance_change(&self) {
    crate::unsupported::report(
      "AppWindow::reapply_traffic_light_position_after_appearance_change (Electron keeps its own position)",
    );
  }

  /// Applies a title bar style.
  ///
  /// Electron's `titleBarStyle` is a constructor option (`hidden`, `hiddenInset`,
  /// `customButtonsOnHover`); it cannot be changed after the window exists, and the runtime
  /// routes the request through `window.titleBarStyle`, which reaches the same AppKit shim.
  pub(crate) fn set_title_bar_style(&self, _style: TitleBarStyle) {
    crate::unsupported::report("AppWindow::set_title_bar_style (Electron: window.titleBarStyle)");
  }

  /// Re-derives whether the window can be dragged by its background.
  ///
  /// The reference decides this from the title bar style and the simple-fullscreen flag, both
  /// of which Electron keeps in its own window state; `win.setMovable` is the RPC that
  /// applies the answer.
  pub(crate) fn apply_movability(&self) {
    crate::unsupported::report("AppWindow::apply_movability (Electron: window.movable)");
  }

  pub(crate) fn set_visible_on_all_workspaces(&self, _visible: bool) {
    crate::unsupported::report(
      "AppWindow::set_visible_on_all_workspaces (Electron: window.visibleOnAllWorkspaces)",
    );
  }

  pub(crate) fn set_background_color(&self, _color: Option<Color>) {
    crate::unsupported::report(
      "AppWindow::set_background_color (Electron: window.backgroundColor)",
    );
  }
}
