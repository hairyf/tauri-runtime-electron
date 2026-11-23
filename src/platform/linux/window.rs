// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! Window-level X11 and GTK mechanisms.
//!
//! The reference implements these on the window it created, reaching GTK4 for the widget tree
//! and raw Xlib for the X11 host and the EWMH requests. Electron owns the window here: its
//! `BrowserWindow` is a GTK/Views window it drives itself, so `window.enabled`,
//! `window.progressBar`, `window.backgroundColor` and `window.visibleOnAllWorkspaces` are the
//! RPCs the runtime uses, and `setSkipTaskbar` is a no-op on Linux in both.

use std::os::raw::c_ulong;

use tauri_runtime::{ProgressBarState, dpi::PhysicalSize};
use tauri_utils::config::Color;

use crate::platform::NativeWindow;

use super::{taskbar, utils::set_wm_state};

/// The 24-bit X11 parent CEF browser children were reparented into.
///
/// GTK owned the toplevel layout and menu widgets while CEF created native X11 child windows,
/// so the reference kept a host window sized to GTK's content box — with a 24-bit TrueColor
/// visual, because CEF did not render correctly with the inherited GTK visual on all X11
/// setups — and reparented every browser into it.
///
/// Electron's webviews are composited views inside the window's own surface, so there is no
/// child window, no colormap and no GTK `layout` hook to keep in sync. The type stays as a
/// placeholder so the window record has the same shape; every method reports.
pub(crate) struct CefX11Host;

impl CefX11Host {
  /// Creates the host for a window.
  ///
  /// The reference builds the GTK content box and the X11 container here and returns `None`
  /// when the window has no GTK surface or the 24-bit visual is unavailable. There is nothing
  /// to create, so the placeholder is always available.
  pub(crate) fn new() -> Option<Self> {
    Some(Self)
  }

  /// Shows or hides the host.
  ///
  /// The reference maps or unmaps the X11 container, because a window that only hosts
  /// offscreen webviews must not show the host. Electron's window visibility is
  /// `window.show` / `window.hide`.
  pub(crate) fn set_visible(&self, _visible: bool) {
    crate::unsupported::report("CefX11Host::set_visible (Electron: window.show / window.hide)");
  }

  /// The size the host was last laid out at.
  ///
  /// The reference keeps GTK's content-box size to lay the CEF children out against. Electron
  /// reports the content size from `window.geometry`, so there is none to cache here.
  pub(crate) fn size(&self) -> PhysicalSize<u32> {
    PhysicalSize::new(0, 0)
  }

  /// Whether the host was resized since the CEF children were last laid out.
  ///
  /// The reference needs this because a menu bar attached, hidden or shown changes the content
  /// box without resizing the toplevel, so winit emits no resize event. Electron reports the
  /// webview bounds itself and the runtime lays webviews out on every resize it does see, so
  /// there is no pending relayout to report.
  pub(crate) fn take_needs_relayout(&self) -> bool {
    false
  }
}

impl NativeWindow {
  /// The handle handed to native APIs.
  ///
  /// CEF takes the X11 window id of its host; Electron reports the toplevel's own id, so the
  /// indirection collapses to the same value as [`Self::xid`].
  pub(crate) fn cef_host_handle(&self) -> c_ulong {
    self.xid()
  }

  pub(crate) fn xid(&self) -> c_ulong {
    self.handle() as c_ulong
  }

  /// Raises and activates the window.
  ///
  /// Electron's `window.focus` sends the same `_NET_ACTIVE_WINDOW` request from the process
  /// that holds the X connection.
  pub(crate) fn raise_native(&self) {
    crate::unsupported::report("AppWindow::raise_native (Electron: window.focus)");
  }

  /// Applies the transient parent recorded by the window builder, if any.
  ///
  /// Electron's `window.setParentWindow` is the same `set_transient_for` on the GTK window.
  pub(crate) fn apply_transient_for(&self) {
    crate::unsupported::report("AppWindow::apply_transient_for (Electron: window.setParentWindow)");
  }

  /// Greys the window out.
  ///
  /// The reference only makes the GTK widget tree insensitive: the CEF browsers are foreign
  /// X11 children GTK does not dispatch events for, so the web contents stay interactive.
  /// Electron's `window.enabled` covers the whole window.
  pub(crate) fn set_enabled(&self, _enabled: bool) {
    crate::unsupported::report("AppWindow::set_enabled (Electron: window.enabled)");
  }

  pub(crate) fn is_enabled(&self) -> bool {
    crate::unsupported::report("AppWindow::is_enabled (Electron: window.enabledQuery)");
    true
  }

  /// Applies the window background color.
  ///
  /// The reference installs a display-wide CSS provider carrying a per-window class, and drops
  /// the previous provider on every call because GTK cannot replace one. Electron's
  /// `window.backgroundColor` paints the background inside its own window.
  pub(crate) fn set_background_color(&self, _color: Option<Color>) {
    crate::unsupported::report(
      "AppWindow::set_background_color (Electron: window.backgroundColor)",
    );
  }

  pub(crate) fn set_skip_taskbar(&self, skip: bool) {
    set_wm_state(self.xid(), skip, "_NET_WM_STATE_SKIP_TASKBAR", None);
  }

  pub(crate) fn set_visible_on_all_workspaces(&self, visible: bool) {
    set_wm_state(self.xid(), visible, "_NET_WM_STATE_STICKY", None);
  }

  pub(crate) fn set_progress_bar(&self, state: ProgressBarState) {
    taskbar::set_progress_bar(state);
  }
}
