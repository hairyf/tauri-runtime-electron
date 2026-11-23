// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! Dock visibility, which Electron performs itself.
//!
//! The reference implements `TransformProcessType` and the duplicate-icon
//! workaround around it because it owns the `NSApplication`. Electron's
//! `app.dock.hide()` / `app.dock.show()` are the same implementation — the
//! comments below are the reasoning the reference copied from Electron, and they
//! still describe what `app.dockVisibility` does inside its own process.
//!
//! The functions keep the reference's names so the call sites read the same; the
//! runtime reaches the mechanism through `app.dockVisibility`.

use std::time::Duration;

/// How long after a Dock show a hide request is ignored.
const DOCK_SHOW_TIMEOUT: Duration = Duration::from_secs(1);
/// The Dock's bundle identifier, used to activate it before a delayed show.
const DOCK_BUNDLE_IDENTIFIER: &str = "com.apple.dock";

pub(crate) fn set_dock_visibility(visible: bool) {
  if visible {
    set_dock_show();
  } else {
    set_dock_hide();
  }
}

/// Hides the Dock icon.
///
/// `TransformProcessType` from UIElement back to foreground is asynchronous and
/// does not expose a completion signal. Electron found that rapid hide/show
/// cycles can race the macOS Dock and leave duplicate app icons behind, so it
/// ignores hide requests made within [`DOCK_SHOW_TIMEOUT`] of a show. Electron
/// keeps that timestamp itself; there is no `NSApplication` subclass here to keep
/// it on.
fn set_dock_hide() {
  crate::unsupported::report("platform::macos::set_dock_hide (Electron: app.dockVisibility)");
}

/// Shows the Dock icon.
///
/// `TransformProcessType` is buggy when bringing an active UIElement app back to
/// the foreground: Electron works around it by activating the Dock first, then
/// delaying the foreground transform and the app reactivation.
fn set_dock_show() {
  crate::unsupported::report("platform::macos::set_dock_show (Electron: app.dockVisibility)");
}

/// Marks every window as (not) hideable, as the transform requires.
fn set_windows_can_hide(_can_hide: bool) {
  crate::unsupported::report("platform::macos::set_windows_can_hide (Electron owns the windows)");
}

/// Re-runs the foreground transform and the reactivation after a delay.
fn perform_delayed_dock_show() {
  crate::unsupported::report(
    "platform::macos::perform_delayed_dock_show (Electron: app.dockVisibility)",
  );
}

/// Brings the Dock itself forward, so the transform has a foreground owner.
fn activate_dock() {
  crate::unsupported::report("platform::macos::activate_dock (Electron: app.dockVisibility)");
}
