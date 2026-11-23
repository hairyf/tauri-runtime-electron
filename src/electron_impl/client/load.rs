// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! Loading state and navigation policy: the Electron equivalent of `LoadHandler`, plus the
//! main-frame half of the reference's `RequestHandler::on_before_browse`.
//!
//! CEF reports the load sequence in four callbacks — loading-state change, load start, load
//! error, load end — and the navigation policy in a fifth, `on_before_browse`, which can
//! cancel a navigation before it starts. Electron reports the same sequence as
//! `webview.loadStarted` / `webview.loadFinished` / `webview.loadFailed` and asks for the
//! policy through a *blocking* `webview.navigationRequested` request that the main thread
//! waits on.
//!
//! Two differences worth naming. Electron's load events are main-frame only, so the
//! reference's "every frame" rule for `DocumentCommitted` is served by
//! `webview.navigationCommitted` in `frame.rs` instead. And there is no separate loading-state
//! event: the state is derived from the start and end of a load, which is exactly the pair the
//! reference's `is_loading` flag describes.

use serde_json::{Value, json};
use tauri_runtime::{UserEvent, webview::PageLoadEvent};

use crate::{
  electron_impl::client::{self, MAIN_FRAME_ID, emit_frame_event, request_id, webview_id},
  runtime::{IPC_TIMEOUT, RuntimeContext},
};

/// A document started loading.
///
/// `PageLoadEvent::Started` is main-frame only in the reference, which is all Electron
/// reports; the loading state is browser-wide, so it is emitted for the webview.
pub(crate) fn on_load_started<T: UserEvent>(_context: &RuntimeContext<T>, params: &Value) {
  let Some(webview_id) = webview_id(params) else {
    return;
  };
  let url = params["url"].as_str().unwrap_or_default().to_owned();

  client::with_client::<T, _>(webview_id, |client| {
    emit_frame_event(
      &client.handlers.frame_event_handler,
      client.browser_id(),
      MAIN_FRAME_ID,
      true,
      crate::FrameEventKind::LoadingStateChanged { is_loading: true },
    );
    if let (Some(handler), Ok(url)) = (&client.handlers.on_page_load_handler, url::Url::parse(&url))
    {
      (*handler)(url, PageLoadEvent::Started);
    }
  });
}

/// A document finished loading.
pub(crate) fn on_load_finished<T: UserEvent>(_context: &RuntimeContext<T>, params: &Value) {
  let Some(webview_id) = webview_id(params) else {
    return;
  };
  let url = params["url"].as_str().unwrap_or_default().to_owned();

  client::with_client::<T, _>(webview_id, |client| {
    emit_frame_event(
      &client.handlers.frame_event_handler,
      client.browser_id(),
      MAIN_FRAME_ID,
      true,
      crate::FrameEventKind::LoadingStateChanged { is_loading: false },
    );
    if let (Some(handler), Ok(url)) = (&client.handlers.on_page_load_handler, url::Url::parse(&url))
    {
      (*handler)(url, PageLoadEvent::Finished);
    }
  });
}

/// A navigation failed or was cancelled.
///
/// The bridge already suppresses `ERR_ABORTED` (`-3`), which is what an ordinary interrupted
/// navigation reports, so what arrives here is a failure the application should see. The load
/// state is cleared as well: a failed navigation is still the end of a load, and leaving the
/// webview marked as loading would keep every document generation unready.
pub(crate) fn on_load_failed<T: UserEvent>(_context: &RuntimeContext<T>, params: &Value) {
  let Some(webview_id) = webview_id(params) else {
    return;
  };
  let Ok(url) = url::Url::parse(params["url"].as_str().unwrap_or_default()) else {
    return;
  };

  client::with_client::<T, _>(webview_id, |client| {
    emit_frame_event(
      &client.handlers.frame_event_handler,
      client.browser_id(),
      MAIN_FRAME_ID,
      true,
      crate::FrameEventKind::NavigationFailed { url },
    );
    emit_frame_event(
      &client.handlers.frame_event_handler,
      client.browser_id(),
      MAIN_FRAME_ID,
      true,
      crate::FrameEventKind::LoadingStateChanged { is_loading: false },
    );
  });
}

/// The bridge is about to navigate and is waiting for a decision.
///
/// This is the reference's `on_before_browse`: an unparseable URL passes (the renderer asked
/// for something the runtime has no opinion about), a handler returning `false` cancels the
/// navigation, and only an admitted navigation is reported as `NavigationStarted` — so a
/// native observer's barrier is never stranded on a navigation that was denied.
pub(crate) fn on_navigation_requested<T: UserEvent>(context: &RuntimeContext<T>, params: &Value) {
  let Some(id) = request_id(params) else {
    log::debug!("ignoring a `webview.navigationRequested` request without an id");
    return;
  };
  let Some(webview_id) = webview_id(params) else {
    respond(context, id, true);
    return;
  };
  let target = params["url"].as_str().unwrap_or_default();

  let Ok(url) = url::Url::parse(target) else {
    respond(context, id, true);
    return;
  };

  // The answer is computed under the webview lock and sent after it is released: the bridge's
  // blocked thread processes `webview.navigationResponse` re-entrantly, so an RPC from inside
  // the lock would hold the live webview state for the duration of a round trip.
  let allowed = client::with_client::<T, _>(webview_id, |client| {
    let allowed = client
      .handlers
      .navigation_handler
      .as_ref()
      .is_none_or(|handler| (*handler)(&url));
    if allowed {
      emit_frame_event(
        &client.handlers.frame_event_handler,
        client.browser_id(),
        MAIN_FRAME_ID,
        true,
        crate::FrameEventKind::NavigationStarted { url },
      );
    }
    allowed
  })
  .unwrap_or(true);

  respond(context, id, allowed);
}

/// Answers a blocking navigation request.
fn respond<T: UserEvent>(context: &RuntimeContext<T>, id: u64, allow: bool) {
  if let Err(error) = context.ipc.call(
    "webview.navigationResponse",
    json!({ "id": id, "allow": allow }),
    IPC_TIMEOUT,
  ) {
    log::warn!("failed to answer the Electron navigation request {id}: {error}");
  }
}
