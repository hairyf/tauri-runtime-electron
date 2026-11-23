// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! Frame lifecycle: the Electron equivalent of `FrameHandler`.
//!
//! CEF reports five frame moments — created, attached, detached, destroyed, and the main frame
//! changing — and every one of them carries the native frame object, which is what binds a
//! browser identity into the runtime's document bookkeeping. Electron exposes `frame-created`
//! for out-of-process iframes and nothing else: there is no attach, detach or destroy
//! notification for a guest frame, and the main frame is the `webContents` itself.
//!
//! So the event set is reduced, deliberately and visibly: `Created`, `MainFrameChanged` and
//! (from `webview.navigationCommitted`) `DocumentCommitted` and `AddressChanged` are reported,
//! while `Attached`, `Detached` and `Destroyed` are never synthesised. Inventing them would
//! tell an observer that a frame had become routable, or stopped being routable, at a moment
//! the runtime did not observe — and a document-generation barrier built on that answer would
//! admit a document that no longer exists.
//!
//! The events still drive the runtime's own bookkeeping: the webview's
//! [`FrameNavigationState`](crate::FrameNavigationState) is what `WebviewSnapshot::document`
//! is validated against, and it needs exactly this sequence to reach a ready generation.

use serde_json::Value;
use tauri_runtime::UserEvent;

use crate::{
  electron_impl::client::{self, MAIN_FRAME_ID, emit_frame_event, webview_id},
  runtime::RuntimeContext,
};

/// The bridge adopted a guest and named the native browser behind it.
///
/// `contentsId` is Electron's `webContents.id`, which is the native browser identity a
/// `FrameEvent` reports; `webview.rs` records it when it creates the guest, so it is already
/// on the live state by the time this event arrives. The three events are emitted in the order
/// the document bookkeeping expects: the frame exists, it is this browser's main frame, and it
/// can be routed to.
pub(crate) fn on_attached<T: UserEvent>(_context: &RuntimeContext<T>, params: &Value) {
  let Some(webview_id) = webview_id(params) else {
    return;
  };

  client::with_client::<T, _>(webview_id, |client| {
    for kind in [
      crate::FrameEventKind::Created,
      crate::FrameEventKind::MainFrameChanged,
      crate::FrameEventKind::Attached,
    ] {
      emit_frame_event(
        &client.handlers.frame_event_handler,
        client.browser_id(),
        MAIN_FRAME_ID,
        true,
        kind,
      );
    }
  });
}

/// A document committed in the webview's main frame.
///
/// This is where the reference's `DocumentCommitted` and `AddressChanged` come from: CEF
/// reports the commit and the address change separately (`on_load_start` and
/// `on_address_change`), while Electron reports one `did-navigate` for both, including the
/// same-document history and fragment changes the address event describes.
pub(crate) fn on_navigation_committed<T: UserEvent>(_context: &RuntimeContext<T>, params: &Value) {
  let Some(webview_id) = webview_id(params) else {
    return;
  };
  let Ok(url) = url::Url::parse(params["url"].as_str().unwrap_or_default()) else {
    return;
  };

  client::with_client::<T, _>(webview_id, |client| {
    for kind in [
      crate::FrameEventKind::DocumentCommitted { url: url.clone() },
      crate::FrameEventKind::AddressChanged { url },
    ] {
      emit_frame_event(
        &client.handlers.frame_event_handler,
        client.browser_id(),
        MAIN_FRAME_ID,
        true,
        kind,
      );
    }
  });
}
