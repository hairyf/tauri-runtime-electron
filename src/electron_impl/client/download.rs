// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! Downloads: the Electron equivalent of `DownloadHandler`.
//!
//! This is the closest one-to-one mapping in the whole scope. CEF asks `can_download`, then
//! `on_before_download` with a suggested name and a callback that continues the download, then
//! `on_download_updated` on every progress tick; Electron asks once through a blocking
//! `download.requested` and reports the outcome once through `download.finished`. The save
//! dialog is the one thing to get right: the reference shows it exactly when the application
//! did *not* choose a path, which on this side is `destination: null` in the answer.

use std::path::PathBuf;

use serde_json::{Value, json};
use tauri_runtime::{UserEvent, webview::DownloadEvent};

use crate::{
  electron_impl::client::{self, request_id, webview_id},
  runtime::{IPC_TIMEOUT, RuntimeContext},
};

/// A download was requested and the bridge is waiting for an answer.
///
/// The reference's `can_download` answers "yes" unconditionally and leaves the validation to
/// `on_before_download`, because a client whose download handler is NULL cancels every
/// download. Electron has no such gate: the bridge asks for every download, and the
/// application's `Requested` answer is the only decision there is.
pub(crate) fn on_requested<T: UserEvent>(context: &RuntimeContext<T>, params: &Value) {
  let Some(id) = request_id(params) else {
    log::debug!("ignoring a `download.requested` request without an id");
    return;
  };
  let Some(webview_id) = webview_id(params) else {
    respond(context, id, false, None);
    return;
  };
  let Ok(url) = url::Url::parse(params["url"].as_str().unwrap_or_default()) else {
    // The reference refuses a download whose URL does not parse: there is nothing to report to
    // the application and nothing to name in an answer.
    respond(context, id, false, None);
    return;
  };

  let suggested_path = params["destination"]
    .as_str()
    .map(PathBuf::from)
    .unwrap_or_default();
  let mut destination = suggested_path.clone();

  // The decision is made under the webview lock and sent after it is released, so the bridge's
  // blocked thread never talks to a runtime that is holding its own live state.
  let decision = client::with_client::<T, _>(webview_id, |client| {
    let Some(handler) = client.handlers.download_handler.as_ref() else {
      // No handler means the application did not ask to be consulted; the reference shows the
      // save dialog in that case, which is `destination: null`.
      return Some((true, None));
    };
    if !(*handler)(DownloadEvent::Requested {
      url,
      destination: &mut destination,
    }) {
      return Some((false, None));
    }
    // A destination the application did not touch is the one Electron suggested, and the
    // reference shows the save dialog for exactly that case.
    let chosen =
      (destination != suggested_path).then(|| destination.to_string_lossy().into_owned());
    Some((true, chosen))
  })
  .flatten()
  .unwrap_or((true, None));

  respond(context, id, decision.0, decision.1);
}

/// A download finished, successfully or not.
///
/// The reference only calls the handler when the download is complete or cancelled, never on a
/// progress tick; `download.finished` is that same single report.
pub(crate) fn on_finished<T: UserEvent>(_context: &RuntimeContext<T>, params: &Value) {
  let Some(webview_id) = webview_id(params) else {
    return;
  };
  let Ok(url) = url::Url::parse(params["url"].as_str().unwrap_or_default()) else {
    return;
  };
  let path = params["path"]
    .as_str()
    .filter(|path| !path.is_empty())
    .map(PathBuf::from);
  let success = params["success"].as_bool().unwrap_or(false);

  client::with_client::<T, _>(webview_id, |client| {
    if let Some(handler) = &client.handlers.download_handler {
      (*handler)(DownloadEvent::Finished { url, path, success });
    }
  });
}

/// Answers a blocking download request.
fn respond<T: UserEvent>(
  context: &RuntimeContext<T>,
  id: u64,
  allow: bool,
  destination: Option<String>,
) {
  if let Err(error) = context.ipc.call(
    "webview.downloadResponse",
    json!({ "id": id, "allow": allow, "destination": destination }),
    IPC_TIMEOUT,
  ) {
    log::warn!("failed to answer the Electron download request {id}: {error}");
  }
}
