// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! Webview-level X11 mechanisms.
//!
//! The reference implements these on `AppWebview` because a CEF browser is a native X11 child
//! window that has to be reparented into the GTK content area's 24-bit host, mapped, unmapped
//! and moved with `XMoveResizeWindow`. Electron's webviews are composited views inside the
//! window's own surface, so there is no child window to host: `webview.setBounds`,
//! `webview.visibility`, `webview.reparent` and `webview.close` are the RPCs the runtime uses.
//!
//! The functions keep the reference's names and take the webview's native handle instead of
//! an `AppWebview`.

use std::os::raw::c_ulong;

use tauri_runtime::dpi::Rect;
use tauri_utils::config::Color;

use crate::platform::NativeWindow;

/// The X11 window Electron created for the webview.
///
/// `window.nativeHandle` is the X11 window id on Linux, so the reported handle is the value
/// the reference reads out of CEF's browser host.
pub(crate) fn xid(webview: &NativeWindow) -> c_ulong {
  webview.handle() as c_ulong
}

/// Whether the webview's window sits under the window's X11 host.
///
/// The reference queries the X11 parent with `XQueryTree`. Electron's webview is a composited
/// view rather than a child window, and the runtime already tracks which window a webview
/// belongs to, so the parent comparison is an id comparison.
pub(crate) fn native_parent_matches(
  _webview: &NativeWindow,
  _parent: &NativeWindow,
) -> Option<bool> {
  crate::unsupported::report(
    "AppWebview::native_parent_matches (no X11 child window; compare webview ids)",
  );
  None
}

pub(crate) fn native_visible(_webview: &NativeWindow) -> Option<bool> {
  crate::unsupported::report("AppWebview::native_visible (Electron: webview.bounds)");
  None
}

pub(crate) fn set_background_color(_webview: &NativeWindow, _color: Option<Color>) {
  // Native child-window background is not equivalent to Chromium's rendered background; the
  // reference leaves it to `BrowserSettings`, and Electron takes it as
  // `webview.backgroundColor`.
  crate::unsupported::report(
    "AppWebview::set_background_color (Electron: webview.backgroundColor)",
  );
}

pub(crate) fn bounds(_webview: &NativeWindow) -> Option<Rect> {
  crate::unsupported::report("AppWebview::bounds (Electron: webview.bounds)");
  None
}

pub(crate) fn reparent(_webview: &NativeWindow, _parent: &NativeWindow) {
  crate::unsupported::report("AppWebview::reparent (Electron: webview.reparent)");
}

pub(crate) fn apply_visible(_webview: &NativeWindow, _visible: bool) {
  crate::unsupported::report("AppWebview::apply_visible (Electron: webview.visibility)");
}

pub(crate) fn apply_physical_bounds(
  _webview: &NativeWindow,
  _scale: f64,
  _x: i32,
  _y: i32,
  _width: i32,
  _height: i32,
) {
  crate::unsupported::report("AppWebview::apply_physical_bounds (Electron: webview.setBounds)");
}
