// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

use std::sync::Arc;

/// One browser-process notification about a native Electron frame.
///
/// Notifications are delivered on the runtime's own thread, from the event stream
/// the Electron main process pushes. Handlers must return promptly and must not
/// call APIs that wait for the event loop. Unlike Tauri's portable navigation
/// callbacks, these notifications include child frames.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct FrameEvent {
  /// Native browser identity, the Electron `webContents.id`. Every event delivered
  /// to one webview's observer carries that webview's own identity; a popup the
  /// runtime opened as a window of its own is a different browser and is not
  /// reported here.
  pub browser_id: i32,
  /// Electron's opaque identifier for this native frame lifetime.
  /// Empty for a browser-wide notification when no main frame exists.
  pub frame_id: String,
  /// Whether Electron identifies this as the main frame at callback time.
  pub is_main: bool,
  /// Native lifecycle phase.
  pub kind: FrameEventKind,
}

/// Native lifecycle phases reported for every frame.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum FrameEventKind {
  /// A frame object exists, but may not yet have a renderer connection.
  Created,
  /// Commands can be routed to the renderer. Reattachment is also reported.
  Attached,
  /// The frame can no longer route commands to its renderer.
  Detached,
  /// The native frame object is being destroyed.
  Destroyed,
  /// A navigation was admitted by the existing navigation policy, before commit.
  NavigationStarted { url: url::Url },
  /// A document committed, before its contents begin loading.
  DocumentCommitted { url: url::Url },
  /// Navigation failed or was cancelled. This is not a document-ready signal.
  NavigationFailed { url: url::Url },
  /// An address changed, including a same-document history or fragment change.
  AddressChanged { url: url::Url },
  /// The browser assigned this frame as its main frame.
  MainFrameChanged,
  /// Browser-wide load state. `false` follows every frame's load-end/error
  /// notifications, including cancelled navigation. It does not assert DOM,
  /// application, network-idle, or renderer responsiveness.
  LoadingStateChanged { is_loading: bool },
  /// The renderer terminated. Prior document generations cannot be reused.
  RendererTerminated,
}

/// Synchronous observer for native frame lifecycle events.
pub type FrameEventHandler = dyn Fn(FrameEvent) + Send + Sync + 'static;

/// Reports one native frame notification to an application observer.
///
/// The reference builds the event from a `cef::Browser` and a `cef::Frame` it is
/// handed by a CEF callback. Electron reports the same facts as fields of the
/// event it pushes (`webContents.id`, the frame's own identity, whether it is the
/// main frame), so the caller passes them in and this only assembles the event.
///
/// `electron_impl::client` calls it for every frame moment the bridge reports,
/// including the ones only the runtime's own document-generation bookkeeping
/// observes.
pub(crate) fn emit_frame_event(
  handler: &Option<Arc<FrameEventHandler>>,
  browser_id: i32,
  frame_id: &str,
  is_main: bool,
  kind: FrameEventKind,
) {
  if let Some(handler) = handler {
    handler(FrameEvent {
      browser_id,
      frame_id: frame_id.to_owned(),
      is_main,
      kind,
    });
  }
}
