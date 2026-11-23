// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! The Electron equivalent of `BrowserProcessHandler`: process-wide lifecycle signalling.
//!
//! The reference's handler exists because CEF asks the embedder about things that happen
//! to the *browser process* rather than to one browser: the context finished initializing,
//! the external message pump wants work scheduled, a child process is about to launch, a
//! second launch of the same application arrived. Electron owns its own process tree and
//! its own message loop, so what is left here is the handshake, the exit request and the
//! deep-link forwarding — and the one event that ends the loop.

use std::sync::{
  Arc,
  atomic::{AtomicBool, Ordering},
};

use serde_json::Value;
use tauri_runtime::{RunEvent, UserEvent};

use crate::runtime::{Message, RuntimeContext};

/// The process-wide client state, mirroring `TauriCefBrowserProcessHandler`.
pub(crate) struct TauriElectronBrowserProcessHandler<T: UserEvent> {
  context: RuntimeContext<T>,
  context_initialized: Arc<AtomicBool>,
  deep_link_schemes: Vec<String>,
}

impl<T: UserEvent> TauriElectronBrowserProcessHandler<T> {
  pub(crate) fn new(
    context: RuntimeContext<T>,
    context_initialized: Arc<AtomicBool>,
    deep_link_schemes: Vec<String>,
  ) -> Self {
    Self {
      context,
      context_initialized,
      deep_link_schemes,
    }
  }

  /// The flag the runtime's startup blocks on, mirroring `on_context_initialized`.
  ///
  /// In the reference this is a CEF callback that fires once the browser process has a
  /// usable context. Here it is the handshake: `electron-rs` has already matched the
  /// `ready` line and the socket is connected, so the flag is what tells the runtime the
  /// main process is answering.
  pub(crate) fn on_context_initialized(&self) {
    self.context_initialized.store(true, Ordering::SeqCst);
  }

  /// Schedules work for the message pump.
  ///
  /// Absent: Electron runs its own loop in its own process, and this runtime never pumps
  /// one. The reference needs it because CEF is driven by an external pump the embedder
  /// owns, and a task posted to the UI thread only runs when someone asks for it.
  pub(crate) fn on_schedule_message_pump_work(&self, _delay_ms: i64) {
    crate::unsupported::report("TauriElectronBrowserProcessHandler::on_schedule_message_pump_work");
  }

  /// Adds the switches a child process needs before it starts.
  ///
  /// Absent: Electron launches its own renderer, GPU and utility processes, and their
  /// command lines are built by Electron. The reference appends an API-version switch
  /// because a CEF child never sees the embedder's builder, which has no counterpart.
  pub(crate) fn on_before_child_process_launch(&self) {
    crate::unsupported::report(
      "TauriElectronBrowserProcessHandler::on_before_child_process_launch",
    );
  }

  /// Forwards a second launch's deep link to the application.
  ///
  /// The reference reads the relaunching command line and keeps the arguments that parse
  /// as URLs of a configured scheme. The same filtering is what the runtime's own argv
  /// handling applies, so a second launch and a cold start agree on what a deep link is.
  ///
  /// Returns whether anything was forwarded; a launch with no deep link is not an error.
  pub(crate) fn on_already_running_app_relaunch<F: FnMut(RunEvent<T>)>(
    &self,
    arguments: &[String],
    callback: &mut F,
  ) -> bool {
    let urls = deep_link_arguments(arguments, &self.deep_link_schemes);
    if urls.is_empty() {
      return false;
    }
    callback(RunEvent::Opened { urls });
    true
  }

  /// The context of this handler, for the free functions below.
  fn context(&self) -> &RuntimeContext<T> {
    &self.context
  }
}

/// Keeps only the arguments that parse as URLs of a configured deep-link scheme.
fn deep_link_arguments<I>(arguments: I, schemes: &[String]) -> Vec<url::Url>
where
  I: IntoIterator,
  I::Item: AsRef<str>,
{
  arguments
    .into_iter()
    .filter_map(|argument| url::Url::parse(argument.as_ref()).ok())
    .filter(|url| schemes.iter().any(|scheme| scheme == url.scheme()))
    .collect()
}

/// The handshake line.
///
/// `electron-rs` consumes the first line of the socket to complete the handshake, so a
/// `ready` event reaching the run loop at all means a second one was sent; the version it
/// names is already on [`crate::electron_impl::ipc::Ipc`].
pub(crate) fn on_ready<T: UserEvent>(_context: &RuntimeContext<T>, params: &Value) {
  log::debug!(
    "the Electron main process reported itself ready (version {})",
    params["electron"].as_str().unwrap_or("unknown")
  );
}

/// The socket is gone: this is the one event that stops the run loop.
///
/// The channel closing is the authoritative signal — `electron-rs` sends `ipc.closed` on a
/// best-effort basis and the event can be lost when its buffer is full — so the run loop
/// also stops when the receiver disconnects. Returning `false` here makes the common case
/// immediate instead of waiting for the next read to fail.
pub(crate) fn on_ipc_closed<T: UserEvent>(_context: &RuntimeContext<T>, _params: &Value) -> bool {
  log::debug!("the Electron main process closed the connection");
  false
}

/// The application was asked to quit.
///
/// Electron's `before-quit` has no veto of its own here: the reference's equivalent is
/// Tauri's `ExitRequested`, which the application can prevent, so the request travels
/// through the run loop as [`Message::RequestExit`] rather than ending the process here.
pub(crate) fn on_exit_requested<T: UserEvent>(context: &RuntimeContext<T>) {
  if let Err(error) = context.send_message(Message::RequestExit(0)) {
    log::warn!("failed to forward the Electron exit request: {error}");
  }
}

/// The application was re-activated with no window of its own.
///
/// Only macOS reports it, and only macOS has a portable event for it; elsewhere the
/// platform has no "activate" notion for an application that is already running.
pub(crate) fn on_reopen<T: UserEvent, F: FnMut(RunEvent<T>)>(
  _context: &RuntimeContext<T>,
  params: &Value,
  callback: &mut F,
) {
  #[cfg(target_os = "macos")]
  {
    callback(RunEvent::Reopen {
      has_visible_windows: params["hasVisibleWindows"].as_bool().unwrap_or(false),
    });
  }
  #[cfg(not(target_os = "macos"))]
  {
    let _ = (params, callback);
    crate::unsupported::report("client::process::on_reopen");
  }
}

/// A browser lease was requested for a webview.
///
/// Absent: `browser.openRequested` asks the embedder to host a `webContents` the bridge
/// borrowed for a `webHost` session. That is the bridge's own bookkeeping — it owns the
/// lease, the partition and the cookie jar — and Tauri has no browser-lease concept to
/// answer it with, so nothing is granted.
pub(crate) fn on_browser_open_requested<T: UserEvent>(
  _context: &RuntimeContext<T>,
  params: &Value,
) {
  log::debug!(
    "ignoring a browser lease request for webview {:?}",
    params["webviewId"].as_u64()
  );
  crate::unsupported::report("client::process::on_browser_open_requested");
}

/// The process handler of a runtime, built from its startup state.
pub(crate) fn browser_process_handler<T: UserEvent>(
  context: &RuntimeContext<T>,
  context_initialized: Arc<AtomicBool>,
  deep_link_schemes: Vec<String>,
) -> TauriElectronBrowserProcessHandler<T> {
  let handler = TauriElectronBrowserProcessHandler::new(
    context.clone(),
    context_initialized,
    deep_link_schemes,
  );
  handler.on_context_initialized();
  handler
}

#[cfg(test)]
mod tests {
  use super::deep_link_arguments;

  fn schemes() -> Vec<String> {
    vec!["tauri".to_string(), "myapp".to_string()]
  }

  #[test]
  fn keeps_configured_deep_links_in_order() {
    let arguments = [
      "myapp://first".to_string(),
      "--some-switch".to_string(),
      "tauri://second".to_string(),
      "https://example.test/".to_string(),
    ];
    let urls = deep_link_arguments(&arguments, &schemes());
    assert_eq!(
      urls.iter().map(url::Url::as_str).collect::<Vec<_>>(),
      ["myapp://first", "tauri://second"]
    );
  }

  #[test]
  fn drops_everything_that_is_not_a_configured_link() {
    let arguments = [
      "https://example.test/".to_string(),
      "not a url".to_string(),
      "other://scheme".to_string(),
    ];
    assert!(deep_link_arguments(&arguments, &schemes()).is_empty());
  }

  #[test]
  fn an_empty_scheme_list_keeps_nothing() {
    let arguments = ["tauri://first".to_string()];
    assert!(deep_link_arguments(&arguments, &[]).is_empty());
  }

  #[test]
  fn scheme_matching_is_exact() {
    let arguments = ["tauri2://first".to_string(), "tauri://second".to_string()];
    let urls = deep_link_arguments(&arguments, &schemes());
    assert_eq!(
      urls.iter().map(url::Url::as_str).collect::<Vec<_>>(),
      ["tauri://second"]
    );
  }
}
