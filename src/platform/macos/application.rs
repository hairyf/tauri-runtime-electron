// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! The application object, which Electron owns.
//!
//! The reference has to subclass `NSApplication` and install its own delegate:
//! CEF needs the app to answer `CefAppProtocol`, and it needs `openURLs`,
//! `shouldTerminate` and `shouldHandleReopen` forwarded to the runtime so a deep
//! link or a Dock click reaches the application. Electron's main process is the
//! `NSApplication` owner here, and it already forwards those as the `open-file`,
//! `activate` and `before-quit` app events, so the subclass cannot be installed
//! and does not need to be.
//!
//! What is left is the shape: [`setup_application`] still validates the process
//! before any native startup UI is shown, [`AppDelegateEvent`] still names the
//! four events, and [`activate_application`] still brings the app forward.

use std::fmt;

use objc2::{MainThreadMarker, runtime::NSObjectProtocol, sel};
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};

/// The application-level events the reference's delegate forwarded.
///
/// Electron reports the same four as `app.on('before-quit')`,
/// `app.on('activate')`, `app.on('accessibility-support-changed')` and
/// `app.on('open-file')`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AppDelegateEvent {
  /// The application was asked to terminate.
  TryTerminate,
  /// The Dock icon was clicked while the application was running.
  Reopen { has_visible_windows: bool },
  /// The user turned the accessibility "enhanced interface" setting on or off.
  AccessibilityChanged { enabled: bool },
  /// The OS asked the application to open these URLs.
  OpenURLs { urls: Vec<url::Url> },
}

/// The reference's `NSApplicationDelegate`.
///
/// It is a plain Rust value here: the delegate class itself is `absent` because
/// Electron installs its own and forwards the events above, but the runtime still
/// needs somewhere to keep the handler it was given.
pub(crate) struct AppDelegate {
  on_event: Box<dyn Fn(AppDelegateEvent)>,
}

impl fmt::Debug for AppDelegate {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("AppDelegate").finish_non_exhaustive()
  }
}

impl AppDelegate {
  fn new(on_event: Box<dyn Fn(AppDelegateEvent)>) -> Self {
    Self { on_event }
  }

  fn emit(&self, event: AppDelegateEvent) {
    (self.on_event)(event);
  }
}

/// Make this application the active one.
///
/// `activateIgnoringOtherApps:` is deprecated since macOS 14 and is largely
/// ignored there: activation became cooperative, so an app that asks the old way
/// while another app owns the foreground simply stays behind. `-[NSApplication
/// activate]` is the replacement, so prefer it whenever the running system has it
/// and keep the legacy call for older releases. Electron's `app.focus({ steal:
/// true })` is the same logic inside its own process.
pub(crate) fn activate_application() {
  let Some(mtm) = MainThreadMarker::new() else {
    return;
  };

  let app = NSApplication::sharedApplication(mtm);
  if app.respondsToSelector(sel!(activate)) {
    app.activate();
  } else {
    #[allow(deprecated)]
    app.activateIgnoringOtherApps(true);
  }
}

/// Validates the AppKit application before displaying native startup UI.
///
/// Call this on the main thread before any code presents a recovery dialog or
/// other native window. It does not initialize Electron, its event loop, or a
/// browser profile, and repeated calls — including the later runtime
/// initialization — are safe.
///
/// The reference creates its `NSApplication` subclass here and asserts that the
/// singleton is an instance of it. Electron creates the singleton in its own
/// process, so the validation is what stays: the call must be on the main thread,
/// and AppKit's application must exist.
///
/// # Panics
///
/// Panics outside the main thread.
pub fn setup_application() {
  let mtm = MainThreadMarker::new().expect("macOS application must start on the main thread");
  let _app = NSApplication::sharedApplication(mtm);
}

/// Installs the handler for the application events above.
///
/// The reference sets this as `NSApplication.delegate` and keeps the only strong
/// reference to the delegate object. Electron owns the delegate here, so the
/// handler is kept in the returned value and the events arrive through the
/// `electron-rs` event stream instead.
pub(crate) fn set_application_event_handler(
  on_event: Box<dyn Fn(AppDelegateEvent)>,
) -> AppDelegate {
  let delegate = AppDelegate::new(on_event);
  crate::unsupported::report(
    "platform::macos::set_application_event_handler (Electron: app.on('open-file' | 'activate' | 'before-quit'))",
  );
  delegate
}

/// The activation policy AppKit uses for a Tauri policy.
pub(crate) fn activation_policy(
  policy: tauri_runtime::ActivationPolicy,
) -> NSApplicationActivationPolicy {
  match policy {
    tauri_runtime::ActivationPolicy::Regular => NSApplicationActivationPolicy::Regular,
    tauri_runtime::ActivationPolicy::Accessory => NSApplicationActivationPolicy::Accessory,
    tauri_runtime::ActivationPolicy::Prohibited => NSApplicationActivationPolicy::Prohibited,
    _ => NSApplicationActivationPolicy::Regular,
  }
}
