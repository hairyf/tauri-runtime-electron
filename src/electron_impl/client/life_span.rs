// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! Browser lifetime and popups: the Electron equivalent of `LifeSpanHandler`.
//!
//! The reference's handler owns four moments of a browser's life: it registers a popup once
//! CEF created it, it answers `window.open`, it takes over the close so that closing a webview
//! does not close the whole window, and it reports the close. Electron has no callback for any
//! of them; it has `window.closeRequested`, `window.closed`, `webview.closed`,
//! `webview.processGone` and a *blocking* `webview.newWindow` request that the main thread
//! waits on.
//!
//! # What is deliberately not ported
//!
//! `check_and_reload_if_blank` exists because a CEF network-service crash can leave a browser
//! parked on `about:blank`, and the reference reloads the initial URL for up to five seconds
//! to recover. Electron has no such failure mode — a crashed renderer is reported as
//! `webview.processGone` and the guest is reloaded by the bridge — so porting the polling loop
//! would only add a thread that races the runtime's own navigation.
//!
//! # The popup family is the bridge's
//!
//! A CEF popup is a native browser the embedder has to adopt, so the reference keeps a
//! `PopupFamily` reservation protocol to bind it to the opener. Electron's
//! `setWindowOpenHandler` is a blocking request the bridge owns end to end: the answer says
//! whether the popup may open and, when the application named a window, which one to create it
//! as. There is no separate native browser for this runtime to adopt, so the bookkeeping stays
//! in the bridge.

use serde_json::{Value, json};
use tauri_runtime::{
  UserEvent,
  dpi::{LogicalPosition, LogicalSize},
  webview::{NewWindowFeatures, NewWindowResponse, WebContentProcessTermination},
  window::{WindowEvent, WindowId},
};

use crate::{
  electron_impl::{
    client::{self, emit_frame_event, request_id, send_window_event, webview_id, window_id},
    request_handler,
  },
  runtime::{IPC_TIMEOUT, RuntimeContext},
};

/// The application asked to close a window.
///
/// The reference takes the close over for the same reason it does on the webview side: CEF's
/// default would notify the browser's top-level parent window, and every Tauri webview is a
/// child browser parented to a shared window. Here the close request arrives as an event, the
/// application gets the portable `CloseRequested` with its veto, and the window is destroyed
/// only when the veto was not used.
pub(crate) fn on_close_requested<T: UserEvent>(context: &RuntimeContext<T>, params: &Value) {
  let Some(serial) = client::raw_window_id(params) else {
    return;
  };
  let window_id = WindowId::from(serial);
  client::register_window_serial(window_id, serial);

  let (signal_tx, signal_rx) = std::sync::mpsc::channel();
  send_window_event(
    context,
    window_id,
    WindowEvent::CloseRequested { signal_tx },
  );

  // The application answers from inside its `RunEvent` callback, which runs on this very
  // thread and before the loop continues, so waiting here would deadlock. The answer is
  // therefore collected on a thread of its own, which then performs the close.
  let context = context.clone();
  std::thread::spawn(move || {
    let prevented = signal_rx.recv().unwrap_or(false);
    if prevented {
      return;
    }
    if let Err(error) =
      context
        .ipc
        .call("window.destroy", json!({ "windowId": serial }), IPC_TIMEOUT)
    {
      log::warn!("failed to close window {serial} after the application allowed it: {error}");
    }
  });
}

/// A window is gone.
///
/// `window.closed` is the last thing Electron says about a window: the native object is
/// already destroyed, so the runtime drops its record — and every webview that belonged to it,
/// which is what keeps a later event from being routed into a window that no longer exists.
pub(crate) fn on_window_closed<T: UserEvent>(context: &RuntimeContext<T>, params: &Value) {
  let Some(window_id) = window_id(params) else {
    return;
  };

  let webviews = client::unregister_window_clients::<T>(window_id);
  if !webviews.is_empty() {
    log::debug!("window {window_id:?} closed with the webviews {webviews:?}");
  }
  context.remove_window(window_id);
  client::forget_window(window_id);
  send_window_event(context, window_id, WindowEvent::Destroyed);
}

/// A webview is gone.
///
/// Tauri has no portable event for "a webview closed": the reference reports it through
/// `Message::BrowserClosed`, which the runtime turns into the window's `Destroyed` event only
/// when it was the window's last webview. Here the bridge owns that decision — it destroys the
/// hosting window itself when a guest was the only one — so all this does is forget the live
/// state, which is what stops a closed webview's id from being routed to.
pub(crate) fn on_webview_closed<T: UserEvent>(_context: &RuntimeContext<T>, params: &Value) {
  let Some(webview_id) = webview_id(params) else {
    return;
  };
  let label = client::label_of::<T>(webview_id);
  client::unregister_client(webview_id);
  log::debug!("webview {webview_id} ({label:?}) closed");
}

/// The renderer of a webview terminated.
///
/// This is the reference's `on_render_process_terminated`, with Electron's `reason` string in
/// place of CEF's `TerminationStatus`. It is reported twice on purpose: as a frame event, so
/// the document generation the runtime held is revoked, and to the application's termination
/// handler, which is the portable half.
pub(crate) fn on_process_gone<T: UserEvent>(_context: &RuntimeContext<T>, params: &Value) {
  let Some(webview_id) = webview_id(params) else {
    return;
  };
  let reason = params["reason"].as_str().unwrap_or_default();
  let exit_code = params["exitCode"].as_i64().unwrap_or_default();

  client::with_client::<T, _>(webview_id, |client| {
    emit_frame_event(
      &client.handlers.frame_event_handler,
      client.browser_id(),
      "",
      false,
      crate::FrameEventKind::RendererTerminated,
    );
    if let Some(handler) = &client.handlers.web_content_process_terminate_handler {
      (*handler)(WebContentProcessTermination {
        reason: request_handler::termination_reason(reason),
        error_code: i32::try_from(exit_code).ok(),
        error_string: (!reason.is_empty()).then(|| reason.to_owned()),
      });
    }
  });
}

/// `window.open` reached the bridge, which is waiting for an answer.
///
/// The reference answers CEF's `on_before_popup` with one of three outcomes and, for `Allow`,
/// hands the browser a reduced client. Electron's request is blocking, so the answer is an
/// RPC: `allow` lets the bridge create the popup, `allow` with a `windowId` asks it to create
/// that Tauri window instead, and `deny` cancels the popup.
pub(crate) fn on_new_window<T: UserEvent>(context: &RuntimeContext<T>, params: &Value) {
  let Some(id) = request_id(params) else {
    log::debug!("ignoring a `webview.newWindow` request without an id");
    return;
  };
  let Some(webview_id) = webview_id(params) else {
    respond(context, id, &json!({ "action": "deny" }));
    return;
  };

  let target = params["url"].as_str().unwrap_or_default();
  let Ok(url) = url::Url::parse(target) else {
    // The reference cancels the popup when the target URL does not parse: there is no
    // document to open and no origin to attribute it to.
    log::debug!("denying a popup whose target URL does not parse: {target:?}");
    respond(context, id, &json!({ "action": "deny" }));
    return;
  };

  // `window.open` features are CSS pixels, which map to logical units.
  let size = (|| {
    Some(LogicalSize::new(
      params["size"]["width"].as_f64()?,
      params["size"]["height"].as_f64()?,
    ))
  })();
  let position = (|| {
    Some(LogicalPosition::new(
      params["position"]["x"].as_f64()?,
      params["position"]["y"].as_f64()?,
    ))
  })();

  // The blocking request names the popup's target URL and nothing about the document that
  // opened it, so the opener's source URL cannot be reported: an origin recorded for the
  // webview is not a document URL, and reporting one as the other would be a lie an
  // application could act on.
  let response = client::with_client::<T, _>(webview_id, |client| {
    match &client.handlers.new_window_handler {
      Some(handler) => (*handler)(
        url,
        NewWindowFeatures::new(size, position, crate::NewWindowOpener::new(None)),
      ),
      // No handler means the application did not ask to be consulted, and the reference allows
      // the popup in that case.
      None => NewWindowResponse::Allow,
    }
  })
  .unwrap_or(NewWindowResponse::Allow);

  let decision = match response {
    NewWindowResponse::Allow => json!({ "action": "allow" }),
    // The reference cannot transplant a popup's contents into an existing browser and
    // navigates the designated window instead. Electron's bridge does the transplant itself,
    // so the window is named in the answer and the popup opens there.
    NewWindowResponse::Create { window_id } => match client::window_serial(window_id) {
      Some(serial) => json!({ "action": "allow", "windowId": serial }),
      None => {
        log::warn!(
          "window {window_id:?} was designated for a popup but the runtime has no Electron \
           serial for it; the popup opens in a window of its own"
        );
        json!({ "action": "allow" })
      }
    },
    NewWindowResponse::Deny => json!({ "action": "deny" }),
  };

  respond(context, id, &decision);
}

/// Answers a blocking popup request.
fn respond<T: UserEvent>(context: &RuntimeContext<T>, id: u64, decision: &Value) {
  if let Err(error) = context.ipc.call(
    "webview.newWindowResponse",
    json!({ "id": id, "response": decision }),
    IPC_TIMEOUT,
  ) {
    log::warn!("failed to answer the Electron popup request {id}: {error}");
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn every_electron_termination_reason_is_classified() {
    use tauri_runtime::webview::WebContentProcessTerminationReason as Reason;
    assert_eq!(
      request_handler::termination_reason("crashed"),
      Reason::Crashed
    );
    assert_eq!(
      request_handler::termination_reason("oom"),
      Reason::OutOfMemory
    );
    assert_eq!(
      request_handler::termination_reason("killed"),
      Reason::Killed
    );
    assert_eq!(request_handler::termination_reason(""), Reason::Unknown);
  }
}
