// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! X11 plumbing the reference needs and Electron owns.
//!
//! The reference opens its own X connection (`with_x11`), borrows CEF's display connection
//! (`with_cef_display`), interns atoms, and sends EWMH client messages to activate a window
//! or change its `_NET_WM_STATE`. All of that exists because the reference owns the X11
//! toplevel and the CEF child windows inside it.
//!
//! Electron owns the X connection here: `window.focus` is the activation request and
//! `window.visibleOnAllWorkspaces` is the `_NET_WM_STATE_STICKY` change, both made from the
//! process that holds the display. `x11-dl` is not among this crate's dependencies, so
//! `with_x11`, `with_cef_display` and `atom` have no connection to run on and are gone; the
//! two request helpers stay, so the call sites keep their names.

use std::os::raw::c_long;

const NET_WM_STATE_REMOVE: c_long = 0;
const NET_WM_STATE_ADD: c_long = 1;
const NET_ACTIVE_WINDOW_SOURCE_PAGER: c_long = 2;
const CURRENT_TIME: c_long = 0;
const CLIENT_MESSAGE: i32 = 33;
const SUBSTRUCTURE_REDIRECT_MASK: c_long = 1 << 20;
const SUBSTRUCTURE_NOTIFY_MASK: c_long = 1 << 19;

/// Ask the window manager to make `xid` the active window.
///
/// The source indication is `2` ("pager"): EWMH tells window managers to treat those requests
/// as if they came from the user, which is what gets past the focus-stealing prevention that
/// would otherwise turn the request into a taskbar highlight. `1` ("application") would need a
/// valid user input timestamp we do not have when a window is created.
///
/// Electron's `window.focus` sends the same client message from its own process.
pub(super) fn activate_window(_xid: c_long) {
  crate::unsupported::report("platform::linux::activate_window (Electron: window.focus)");
}

/// Add or remove one `_NET_WM_STATE` property on `xid`.
///
/// Electron's `window.visibleOnAllWorkspaces` performs the sticky change; `setSkipTaskbar` is
/// a no-op on Linux in Electron, as it is in the reference.
pub(super) fn set_wm_state(_xid: c_long, _add: bool, _atom1: &str, _atom2: Option<&str>) {
  crate::unsupported::report(
    "platform::linux::set_wm_state (Electron: window.visibleOnAllWorkspaces)",
  );
}
