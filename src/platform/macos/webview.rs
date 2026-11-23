// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! Webview-level AppKit mechanisms.
//!
//! The reference implements these on `AppWebview` because a CEF browser is an `NSView` it
//! has to add to the window's content view, hide, move and remove by hand. Electron's
//! webviews are composited views inside the window's own surface, so that work belongs to
//! Electron: `webview.setBounds`, `webview.visibility`, `webview.backgroundColor`,
//! `webview.reparent` and `webview.close` are the RPCs the runtime uses.
//!
//! The functions keep the reference's names and take the webview's native handle instead of
//! an `AppWebview`. On macOS that handle is the `NSView*` Electron reports, so [`nsview`]
//! still answers with the view itself.

use std::ffi::c_void;

use tauri_runtime::dpi::Rect;
use tauri_utils::config::Color;

use crate::platform::NativeWindow;

/// The `NSView` Electron created for the webview.
///
/// `window.nativeHandle` is documented as the `NSView*` on macOS, so the reported pointer is
/// the view. It is not retained here: the view belongs to Electron for the webview's
/// lifetime, and this crate only ever passes the pointer back to the platform APIs.
pub(crate) fn nsview(webview: &NativeWindow) -> *mut c_void {
  webview.as_ptr()
}

/// Whether the webview's view sits in the window's content view.
///
/// The reference walks `superview` from the browser's `NSView`. Electron's webview is a
/// composited view owned by the window's `WebContentsView` hierarchy, and the runtime already
/// tracks which window a webview belongs to, so the parent comparison is an id comparison.
pub(crate) fn native_parent_matches(
  _webview: &NativeWindow,
  _parent: &NativeWindow,
) -> Option<bool> {
  crate::unsupported::report(
    "AppWebview::native_parent_matches (no NSView bindings; compare webview ids)",
  );
  None
}

pub(crate) fn native_visible(_webview: &NativeWindow) -> Option<bool> {
  crate::unsupported::report("AppWebview::native_visible (Electron: webview.bounds)");
  None
}

pub(crate) fn set_background_color(_webview: &NativeWindow, _color: Option<Color>) {
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

/// Removes the webview's view from its superview.
///
/// The reference has to do this itself because CEF creates the view and only reports
/// `WindowDestroyed` when its `dealloc` runs. Electron closes the webview through
/// `webview.close`, which tears down the `WebContentsView` in its own process.
pub(crate) fn destroy_host_window(_webview: &NativeWindow) {
  crate::unsupported::report("AppWebview::destroy_host_window (Electron: webview.close)");
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
