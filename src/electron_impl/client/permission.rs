// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! The application's permission policy, as Electron asks for it.
//!
//! The reference's handler exists because CEF has two entry points that behave very
//! differently once answered: `on_show_permission_prompt` is reached only while Chromium's
//! stored content setting still says "ask", and continuing it persists the answer into the
//! on-disk profile for good, while `on_request_media_access_permission` is reached on every
//! `getUserMedia()` call and persists nothing. Electron has one entry point —
//! `session.setPermissionRequestHandler`, which the bridge installs and which asks this
//! runtime through a blocking `permission.requested` — and it persists nothing, so an
//! application's changing answer is honoured on every request.
//!
//! # One kind per request
//!
//! CEF asks about a bitmask of permission types and is answered once for all of them, which is
//! why the reference has to combine per-kind answers and let a refusal win. Electron asks about
//! one permission at a time, so [`AppDecision`] has a single answer to combine — but the rule
//! it encodes is kept, because it is the rule the application's handler was written against.
//!
//! # Unmapped permissions are [`PermissionKind::Other`]
//!
//! [`PERMISSION_KINDS`] is a partial map: Electron has permission names Tauri has no kind for —
//! `fullscreen`, `openExternal`, `clipboard-sanitized-write`, and the aggregate `media` that
//! covers camera *and* microphone at once — and everything left over arrives as
//! [`PermissionKind::Other`]. Failing closed is deliberate, but it means a handler written as
//! `match kind { Camera => Allow, _ => Deny }` hard-denies all of them. A handler that only
//! means to answer about the kinds it names should return [`PermissionResponse::Default`].
//!
//! # Desktop capture is never granted from here
//!
//! A `PermissionKind` names no screen, window or tab, so an application answering `Allow` for
//! `DisplayCapture` would be granting a stream it cannot describe. Electron routes
//! `getDisplayMedia()` through `session.setDisplayMediaRequestHandler`, a separate handler that
//! names the source, so a display-capture request is answered `default` and left to it. The
//! reference reaches the same conclusion from the other direction: it defers the whole request
//! to CEF rather than accepting a full-desktop stream.

use serde_json::{Value, json};
use tauri_runtime::{
  UserEvent,
  webview::{PermissionKind, PermissionResponse},
};

use crate::{
  electron_impl::client::{self, request_id, webview_id},
  runtime::{IPC_TIMEOUT, RuntimeContext},
};

/// The application's answer to a permission request, as
/// `PendingWebview::permission_request_handler` carries it.
///
/// `tauri_runtime::webview` declares the same alias but keeps it private, so it is redeclared
/// here; both name one and the same type.
pub(crate) type PermissionRequestHandler =
  dyn Fn(PermissionKind) -> PermissionResponse + Send + Sync;

/// The [`PermissionKind`] the application is asked about for each permission name.
///
/// The names are Electron's own, which is what the bridge forwards; the short spellings the
/// bridge's policy layer uses (`camera`, `microphone`, `midi`) are listed alongside them,
/// because either spelling reaches this table depending on which Electron API raised the
/// request.
///
/// `media` is deliberately absent: it is Electron's aggregate for camera *and* microphone, and
/// answering it as either one would grant the other. It arrives as [`PermissionKind::Other`],
/// which a handler has to answer explicitly.
const PERMISSION_KINDS: &[(&str, PermissionKind)] = &[
  ("camera", PermissionKind::Camera),
  ("microphone", PermissionKind::Microphone),
  ("geolocation", PermissionKind::Geolocation),
  ("notifications", PermissionKind::Notifications),
  ("clipboard-read", PermissionKind::ClipboardRead),
  ("display-capture", PermissionKind::DisplayCapture),
  ("midi", PermissionKind::Midi),
  ("midiSysex", PermissionKind::Midi),
  ("sensors", PermissionKind::Sensors),
  ("local-fonts", PermissionKind::LocalFonts),
  ("window-management", PermissionKind::WindowManagement),
  ("pointerLock", PermissionKind::PointerLock),
  ("automatic-downloads", PermissionKind::AutomaticDownloads),
  ("file-system-access", PermissionKind::FileSystemAccess),
  (
    "media-key-system-access",
    PermissionKind::MediaKeySystemAccess,
  ),
];

/// The permission whose answer must never come from this handler.
const DISPLAY_CAPTURE: &str = "display-capture";

/// How the application answered a permission request.
///
/// CEF asks about a bitmask and is answered once for all of it, so the per-kind answers have to
/// be combined: [`Self::Denied`] wins over everything, because a refusal must never end up
/// granting the rest of the request, and a request is granted only when the application allowed
/// every type in it. Electron asks one kind at a time, so the combination is a single answer —
/// the variants are kept because they are the shape the rule is written in.
enum AppDecision {
  /// The application denied the permission.
  Denied,
  /// The application allowed it.
  Allowed,
  /// The application left it to the platform. Also the answer when the webview has no
  /// permission handler.
  NoOpinion,
}

/// The permission handler of one webview, mirroring `TauriCefPermissionHandler`.
pub(crate) struct TauriElectronPermissionHandler<'a> {
  permission_request_handler: Option<&'a PermissionRequestHandler>,
}

impl<'a> TauriElectronPermissionHandler<'a> {
  pub(crate) fn new(permission_request_handler: Option<&'a PermissionRequestHandler>) -> Self {
    Self {
      permission_request_handler,
    }
  }

  /// Asks the application about `kind`, as [`AppDecision`] describes.
  fn decide(&self, kind: PermissionKind) -> AppDecision {
    let Some(handler) = self.permission_request_handler else {
      return AppDecision::NoOpinion;
    };
    match (*handler)(kind) {
      PermissionResponse::Deny => AppDecision::Denied,
      PermissionResponse::Allow => AppDecision::Allowed,
      PermissionResponse::Default => AppDecision::NoOpinion,
    }
  }

  /// The answer to send the bridge for one permission request.
  pub(crate) fn answer(&self, permission: &str) -> PermissionResponse {
    // Never answered from here: the source a screen share exposes is named by
    // `setDisplayMediaRequestHandler`, which this runtime does not replace.
    if permission == DISPLAY_CAPTURE {
      return PermissionResponse::Default;
    }

    match self.decide(permission_kind(permission)) {
      // Answer for the application, whatever the platform default would be: neither Electron's
      // own prompt nor a blanket grant may override an explicit answer.
      AppDecision::Denied => PermissionResponse::Deny,
      AppDecision::Allowed => PermissionResponse::Allow,
      // The platform default runs unchanged, which is what lets Electron show its own prompt —
      // and record the outcome the way a browser does.
      AppDecision::NoOpinion => PermissionResponse::Default,
    }
  }
}

/// The [`PermissionKind`] of one Electron permission name.
fn permission_kind(permission: &str) -> PermissionKind {
  PERMISSION_KINDS
    .iter()
    .find(|(name, _)| *name == permission)
    .map(|(_, kind)| *kind)
    .unwrap_or(PermissionKind::Other)
}

/// The bridge is waiting for a permission decision.
pub(crate) fn on_requested<T: UserEvent>(context: &RuntimeContext<T>, params: &Value) {
  let Some(id) = request_id(params) else {
    log::debug!("ignoring a `permission.requested` request without an id");
    return;
  };
  let permission = params["kind"].as_str().unwrap_or_default();

  // The answer is computed under the webview lock and sent after it is released: the bridge's
  // blocked thread processes `webview.permissionResponse` re-entrantly.
  let response = webview_id(params)
    .and_then(|webview_id| {
      client::with_client::<T, _>(webview_id, |client| {
        TauriElectronPermissionHandler::new(client.handlers.permission_request_handler.as_deref())
          .answer(permission)
      })
    })
    .unwrap_or(PermissionResponse::Default);

  respond(context, id, response);
}

/// Answers a blocking permission request.
fn respond<T: UserEvent>(context: &RuntimeContext<T>, id: u64, response: PermissionResponse) {
  if let Err(error) = context.ipc.call(
    "webview.permissionResponse",
    json!({ "id": id, "response": response.to_string() }),
    IPC_TIMEOUT,
  ) {
    log::warn!("failed to answer the Electron permission request {id}: {error}");
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn answer_with(handler: &PermissionRequestHandler, permission: &str) -> PermissionResponse {
    TauriElectronPermissionHandler::new(Some(handler)).answer(permission)
  }

  #[test]
  fn every_kind_electron_names_has_a_portable_answer() {
    for (name, kind) in PERMISSION_KINDS {
      assert_ne!(
        *kind,
        PermissionKind::Other,
        "{name} is mapped to a kind Tauri has no name for"
      );
    }
  }

  #[test]
  fn the_aggregate_media_permission_fails_closed() {
    assert_eq!(permission_kind("media"), PermissionKind::Other);
    assert_eq!(permission_kind("fullscreen"), PermissionKind::Other);
    assert_eq!(permission_kind(""), PermissionKind::Other);
  }

  #[test]
  fn an_application_answer_is_never_overridden() {
    let allow = |_: PermissionKind| PermissionResponse::Allow;
    let deny = |_: PermissionKind| PermissionResponse::Deny;
    let default = |_: PermissionKind| PermissionResponse::Default;
    assert_eq!(answer_with(&allow, "camera"), PermissionResponse::Allow);
    assert_eq!(answer_with(&deny, "camera"), PermissionResponse::Deny);
    assert_eq!(answer_with(&default, "camera"), PermissionResponse::Default);
  }

  #[test]
  fn desktop_capture_is_never_answered_from_the_permission_handler() {
    let allow = |_: PermissionKind| PermissionResponse::Allow;
    assert_eq!(
      answer_with(&allow, DISPLAY_CAPTURE),
      PermissionResponse::Default
    );
  }

  #[test]
  fn a_webview_without_a_handler_defers_to_the_platform() {
    let handler = TauriElectronPermissionHandler::new(None);
    assert_eq!(handler.answer("geolocation"), PermissionResponse::Default);
  }

  #[test]
  fn the_application_is_asked_about_the_mapped_kind() {
    // The handler is stored as a `'static` callback, so the observation buffer is
    // shared rather than borrowed.
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let observed = std::sync::Arc::clone(&seen);
    let handler = move |kind: PermissionKind| {
      observed.lock().unwrap().push(kind);
      PermissionResponse::Allow
    };
    answer_with(&handler, "midiSysex");
    assert_eq!(*seen.lock().unwrap(), [PermissionKind::Midi]);
  }
}
