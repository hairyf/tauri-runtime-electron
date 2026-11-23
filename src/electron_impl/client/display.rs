// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! Document title and console output: the Electron equivalent of `DisplayHandler`.
//!
//! The reference's display handler does five things: it claims the cursor, forwards the
//! document title, reports address changes, forwards console messages and reports HTML
//! fullscreen. Three of those have no Electron event, and are documented here rather than
//! synthesised:
//!
//! * **Cursor changes.** Electron reports no cursor change; the cursor is *set*, through
//!   `window.setCursorIcon` / `webview.setCursorIcon` or CSS, and the reference's
//!   `CursorHandler` exists to take the cursor the engine chose. A cursor this runtime did not
//!   observe is not one it may report.
//! * **HTML fullscreen.** Electron makes the hosting window fullscreen itself when a page
//!   enters HTML fullscreen, so there is nothing for the runtime to do; the portable effect is
//!   the window's own `enter-full-screen` event, which `client/mod.rs` already turns into the
//!   window geometry events Tauri has.
//! * **Console messages.** The bridge does not forward `console-message` yet, and
//!   `ElectronWebviewAttributes::console_message_handler` is reported as unsupported where it
//!   is registered. The payload is still read and logged, so the notification is not silently
//!   lost the day the bridge starts sending it.
//!
//! Address changes live in `frame.rs`: they are a frame event, and Electron reports them
//! together with the document commit.

use serde_json::Value;
use tauri_runtime::UserEvent;

use crate::{
  electron_impl::client::{self, webview_id},
  runtime::RuntimeContext,
};

/// The document title changed.
pub(crate) fn on_title_changed<T: UserEvent>(_context: &RuntimeContext<T>, params: &Value) {
  let Some(webview_id) = webview_id(params) else {
    return;
  };
  let Some(title) = params["title"].as_str() else {
    return;
  };

  client::with_client::<T, _>(webview_id, |client| {
    if let Some(handler) = &client.handlers.document_title_changed_handler {
      (*handler)(title.to_owned());
    }
  });
}

/// The renderer wrote to the console.
///
/// Electron's `console-message` fires with the *modern* `(event, details)` signature on the
/// pinned release, where `details.level` is a string rather than a numeric severity. Observing
/// a message changes nothing about Electron's own handling of it: the reference returns 0 from
/// `on_console_message` for exactly that reason, which on this side means never calling
/// `preventDefault()`.
pub(crate) fn on_console_message<T: UserEvent>(_context: &RuntimeContext<T>, params: &Value) {
  let Some(webview_id) = webview_id(params) else {
    return;
  };
  log::debug!(
    "webview {webview_id} logged a {:?} message from {:?}:{}",
    params["level"].as_str().unwrap_or("unknown"),
    params["source"].as_str().unwrap_or_default(),
    params["line"].as_i64().unwrap_or_default()
  );
  crate::unsupported::report("client::display::on_console_message");
}
