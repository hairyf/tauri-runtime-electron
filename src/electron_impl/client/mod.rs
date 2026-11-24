// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! The Electron equivalent of CEF's `Client` aggregate.
//!
//! In the reference, one `TauriCefBrowserClient` is installed on the native browser and
//! Chromium calls its handlers back from inside the engine: a `LoadHandler` on every
//! navigation step, a `DisplayHandler` on every title change, a `LifeSpanHandler` when the
//! browser is created or closed. Nothing of that survives in Electron, because the engine is a
//! different process: it reports what happened as a named event on the JSON-RPC socket, and
//! this module is where those events are turned back into the callbacks the application
//! registered.
//!
//! [`dispatch`] is the single fan-out point. It reads `event["event"]` and routes the payload
//! to the module that owns that family of events — one module per CEF handler file, same names
//! — and answers `false` only when the loop must stop, which is `ipc.closed`.
//!
//! # The registry replaces the native client
//!
//! A CEF client carries the application's callbacks because it *is* the object the engine
//! holds. Electron holds nothing, so the callbacks are kept here, keyed by webview id, in
//! [`TauriElectronBrowserClient`] — the same fields in the same shape as
//! `TauriCefBrowserClientHandlers`, registered by the webview factory when it creates a webview
//! and dropped when the webview closes. An event for a webview with no registered client is
//! not an error: the bridge reports events for guests it adopted on its own, and those have no
//! application callbacks to reach.
//!
//! The boxed handlers are `Send` but not `Sync` in `tauri_runtime`'s own spelling, which is why
//! they are boxed rather than shared: an `Arc<dyn Fn + Send>` is not `Send`, and the registry
//! has to be reachable from the thread that owns the run loop.

use std::{
  any::Any,
  cell::RefCell,
  collections::HashMap,
  sync::{Arc, Mutex, OnceLock, mpsc::Receiver},
};

use serde_json::Value;
use tauri_runtime::{
  RunEvent, UserEvent,
  dpi::{PhysicalPosition, PhysicalSize},
  window::{WindowEvent, WindowId},
};
use tauri_utils::Theme;

use crate::{
  ChromeCommandGroup,
  runtime::{ElectronRuntime, Message, RuntimeContext},
};

mod command;
mod context_menu;
mod display;
mod download;
mod drag;
mod frame;
mod keyboard;
mod life_span;
mod load;
mod permission;
mod process;

pub(crate) use drag::{DragDropEventTarget, DragDropState, drag_drop_initialization_script};
pub(crate) use permission::PermissionRequestHandler;

/// The application's `window.ipc.postMessage` entry point.
///
/// The reference hands the handler a `DetachedWebview`, because the browser it describes is a
/// CEF object the client already holds. Building one here means building an
/// `ElectronWebviewDispatcher`, which belongs to the webview factory; the webview id is
/// therefore what travels through this module, and the closure the factory registers turns it
/// back into a dispatcher.
pub(crate) type IpcHandler = Arc<dyn Fn(u32, http::Request<String>) + Send + Sync>;

/// The application callbacks one webview registered, mirroring
/// `TauriCefBrowserClientHandlers`.
pub(crate) struct TauriElectronBrowserClientHandlers<T: UserEvent> {
  pub(crate) frame_event_handler: Option<Arc<crate::FrameEventHandler>>,
  pub(crate) ipc_handler: Option<IpcHandler>,
  pub(crate) on_page_load_handler: Option<Arc<tauri_runtime::webview::OnPageLoadHandler>>,
  pub(crate) document_title_changed_handler:
    Option<Arc<tauri_runtime::webview::DocumentTitleChangedHandler>>,
  pub(crate) navigation_handler: Option<Arc<tauri_runtime::webview::NavigationHandler>>,
  pub(crate) new_window_handler:
    Option<Arc<tauri_runtime::webview::NewWindowHandler<T, ElectronRuntime<T>>>>,
  pub(crate) download_handler: Option<Arc<tauri_runtime::webview::DownloadHandler>>,
  pub(crate) console_message_handler: Option<Arc<crate::ConsoleMessageHandler>>,
  pub(crate) permission_request_handler: Option<Arc<PermissionRequestHandler>>,
  pub(crate) web_content_process_terminate_handler:
    Option<Arc<tauri_runtime::webview::OnWebContentProcessTerminateHandler>>,
}

impl<T: UserEvent> Default for TauriElectronBrowserClientHandlers<T> {
  fn default() -> Self {
    Self {
      frame_event_handler: None,
      ipc_handler: None,
      on_page_load_handler: None,
      document_title_changed_handler: None,
      navigation_handler: None,
      new_window_handler: None,
      download_handler: None,
      console_message_handler: None,
      permission_request_handler: None,
      web_content_process_terminate_handler: None,
    }
  }
}

/// The state one webview's client needs, mirroring `TauriCefBrowserClient`.
///
/// The fields that describe the *native browser* in the reference — the render handler, the
/// cursor handler — are gone: Electron owns the browser and reports its identity as
/// `contentsId`, which is recorded here the first time the bridge says the guest was adopted.
pub(crate) struct TauriElectronBrowserClient<T: UserEvent> {
  pub(crate) context: RuntimeContext<T>,
  pub(crate) window_id: WindowId,
  pub(crate) webview_id: u32,
  pub(crate) label: String,
  /// The Electron `webContents.id` of this webview's guest, once the bridge named it.
  ///
  /// It is what a `FrameEvent` carries as its native browser identity. Until the bridge reports
  /// it, the webview id stands in: it is unique for the life of the process, so a frame event
  /// is never attributed to a different webview.
  pub(crate) contents_id: Option<i32>,
  pub(crate) devtools_enabled: bool,
  pub(crate) zoom_hotkeys_enabled: bool,
  pub(crate) allowed_chrome_commands: Vec<ChromeCommandGroup>,
  pub(crate) drag_drop_event_target: DragDropEventTarget,
  pub(crate) drag_drop_handler_enabled: bool,
  pub(crate) drag_drop_state: Arc<Mutex<DragDropState>>,
  pub(crate) handlers: TauriElectronBrowserClientHandlers<T>,
}

impl<T: UserEvent> TauriElectronBrowserClient<T> {
  /// The native browser identity reported in frame events.
  pub(crate) fn browser_id(&self) -> i32 {
    self.contents_id.unwrap_or(self.webview_id as i32)
  }
}

/// The clients of every live webview, keyed by webview id.
///
/// A `static` cannot be generic over the application's event type, so the entries are
/// type-erased and [`with_client`] downcasts them back. The registry is what a CEF client is to
/// the reference: the object the engine's notifications are resolved against.
///
/// It is thread-local rather than global because `tauri_runtime`'s handler aliases are `Send`
/// but not `Sync` — an `Arc<dyn Fn + Send>` is not `Send` — so a client cannot cross a thread
/// boundary and a `static` registry could not hold one. Both halves of the contract live on the
/// runtime's own thread: the webview factory registers there, and the run loop that calls
/// [`dispatch`] runs there.
fn with_clients<R>(f: impl FnOnce(&mut HashMap<u32, Box<dyn Any>>) -> R) -> R {
  thread_local! {
    static CLIENTS: RefCell<HashMap<u32, Box<dyn Any>>> = RefCell::new(HashMap::new());
  }
  CLIENTS.with(|clients| f(&mut clients.borrow_mut()))
}

/// Registers the client of a webview that has just been created.
pub(crate) fn register_client<T: UserEvent>(client: TauriElectronBrowserClient<T>) {
  with_clients(|clients| {
    clients.insert(client.webview_id, Box::new(client));
  });
}

/// Forgets a webview's client, which is what a closed webview does.
pub(crate) fn unregister_client(webview_id: u32) {
  with_clients(|clients| {
    clients.remove(&webview_id);
  });
}

/// Runs `f` against the client of `webview_id`, when one is registered.
pub(crate) fn with_client<T: UserEvent, R>(
  webview_id: u32,
  f: impl FnOnce(&TauriElectronBrowserClient<T>) -> R,
) -> Option<R> {
  with_clients(|clients| {
    clients
      .get(&webview_id)
      .and_then(|client| client.downcast_ref::<TauriElectronBrowserClient<T>>())
      .map(f)
  })
}

/// Runs `f` against the client of `webview_id` with write access.
pub(crate) fn with_client_mut<T: UserEvent, R>(
  webview_id: u32,
  f: impl FnOnce(&mut TauriElectronBrowserClient<T>) -> R,
) -> Option<R> {
  with_clients(|clients| {
    clients
      .get_mut(&webview_id)
      .and_then(|client| client.downcast_mut::<TauriElectronBrowserClient<T>>())
      .map(f)
  })
}

/// Records the Electron `webContents.id` the bridge reported for a webview.
pub(crate) fn set_contents_id<T: UserEvent>(webview_id: u32, contents_id: i32) {
  with_client_mut::<T, _>(webview_id, |client| client.contents_id = Some(contents_id));
}

/// The window hosting a webview, as the client registry knows it.
pub(crate) fn window_id_of<T: UserEvent>(webview_id: u32) -> Option<WindowId> {
  with_client::<T, _>(webview_id, |client| client.window_id)
}

/// The label of a webview, as the client registry knows it.
pub(crate) fn label_of<T: UserEvent>(webview_id: u32) -> Option<String> {
  with_client::<T, _>(webview_id, |client| client.label.clone())
}

/// Forgets every client hosted by `window_id`, answering the webview ids that were dropped.
pub(crate) fn unregister_window_clients<T: UserEvent>(window_id: WindowId) -> Vec<u32> {
  with_clients(|clients| {
    let hosted: Vec<u32> = clients
      .iter()
      .filter(|(_, client)| {
        client
          .downcast_ref::<TauriElectronBrowserClient<T>>()
          .is_some_and(|client| client.window_id == window_id)
      })
      .map(|(webview_id, _)| *webview_id)
      .collect();
    for webview_id in &hosted {
      clients.remove(webview_id);
    }
    hosted
  })
}

/// The Electron window serial each Tauri window id was reported under.
///
/// Tauri's `WindowId` is opaque — it converts *from* a `u32` and offers no way back — while
/// every `window.*` RPC names a window by the serial the bridge allocated. The serial is
/// recorded from the events Electron sends, which is where both halves appear together, and
/// [`register_window_serial`] lets the window factory record it up front.
fn window_serials() -> &'static Mutex<HashMap<WindowId, u32>> {
  static WINDOW_SERIALS: OnceLock<Mutex<HashMap<WindowId, u32>>> = OnceLock::new();
  WINDOW_SERIALS.get_or_init(Default::default)
}

/// Records the Electron serial of a Tauri window id.
pub(crate) fn register_window_serial(window_id: WindowId, serial: u32) {
  window_serials()
    .lock()
    .unwrap_or_else(|poisoned| poisoned.into_inner())
    .insert(window_id, serial);
}

/// The Electron serial of a Tauri window id, once it is known.
pub(crate) fn window_serial(window_id: WindowId) -> Option<u32> {
  window_serials()
    .lock()
    .unwrap_or_else(|poisoned| poisoned.into_inner())
    .get(&window_id)
    .copied()
}

/// Forgets everything recorded about a window that is gone.
pub(crate) fn forget_window(window_id: WindowId) {
  window_serials()
    .lock()
    .unwrap_or_else(|poisoned| poisoned.into_inner())
    .remove(&window_id);
  scale_factors()
    .lock()
    .unwrap_or_else(|poisoned| poisoned.into_inner())
    .remove(&window_id);
}

/// The events Electron pushed, as the run loop sees them.
///
/// `electron-rs` creates the channel at launch and buffers it, so a late reader still sees the
/// backlog. The run loop drains it once per iteration instead of blocking on it: the same
/// iteration also has to serve Tauri's own message queue, and a blocking read here would
/// starve it.
pub(crate) struct EventStream {
  receiver: Receiver<electron_rs::Event>,
}

impl EventStream {
  pub(crate) fn new(receiver: Receiver<electron_rs::Event>) -> Self {
    Self { receiver }
  }

  /// Every event that arrived since the last drain, in wire order.
  pub(crate) fn drain(&self) -> Vec<electron_rs::Event> {
    let mut batch = Vec::new();
    while let Ok(event) = self.receiver.try_recv() {
      batch.push(event);
    }
    batch
  }

  /// Blocks for the next event, or answers [`None`] once the channel is closed.
  pub(crate) fn next(&self) -> Option<electron_rs::Event> {
    self.receiver.recv().ok()
  }

  /// The channel itself, for a run loop that owns the drain.
  pub(crate) fn into_inner(self) -> Receiver<electron_rs::Event> {
    self.receiver
  }
}

/// The frame identity this runtime reports for a webview's main frame.
///
/// CEF names every frame with an opaque identifier the embedder passes back; Electron reports
/// no frame identity at all, and the main frame *is* the `webContents`. A stable name is what
/// the document-generation bookkeeping needs: the main frame's identity does not change across
/// a navigation, only the document that occupies it does.
pub(crate) const MAIN_FRAME_ID: &str = "main";

/// Reports one native frame lifecycle phase.
///
/// The reference builds the event from the `cef::Browser` and `cef::Frame` the callback carried
/// and hands it to the client's observer. Electron reports no native handles, so the identity
/// travels as data. The runtime's own document-generation bookkeeping is
/// `webview.rs`'s to advance from the same events; what this delivers is the application's
/// observer, which `webview.rs` reports as unsupported while the bridge forwards no per-frame
/// notification.
pub(crate) fn emit_frame_event(
  handler: &Option<Arc<crate::FrameEventHandler>>,
  browser_id: i32,
  frame_id: &str,
  is_main: bool,
  kind: crate::FrameEventKind,
) {
  crate::frame::emit_frame_event(handler, browser_id, frame_id, is_main, kind);
}

/// Hands a window event to Tauri's run loop.
pub(crate) fn send_window_event<T: UserEvent>(
  context: &RuntimeContext<T>,
  window_id: WindowId,
  event: WindowEvent,
) {
  let _ = context.send_message(Message::WindowEvent(window_id, Box::new(event)));
}

/// The `windowId` of an event payload.
pub(crate) fn window_id(params: &Value) -> Option<WindowId> {
  raw_window_id(params).map(WindowId::from)
}

/// The `windowId` of an event payload, as the number the bridge knows it by.
pub(crate) fn raw_window_id(params: &Value) -> Option<u32> {
  params["windowId"]
    .as_u64()
    .and_then(|id| u32::try_from(id).ok())
}

/// The `webviewId` of an event payload.
pub(crate) fn webview_id(params: &Value) -> Option<u32> {
  params["webviewId"]
    .as_u64()
    .and_then(|id| u32::try_from(id).ok())
}

/// The `id` a blocking policy request carries, which its answer has to echo.
pub(crate) fn request_id(params: &Value) -> Option<u64> {
  params["id"].as_u64()
}

/// A physical-pixel size from a `bounds` payload.
fn physical_size(bounds: &Value) -> Option<PhysicalSize<u32>> {
  let width = bounds["width"].as_u64()?;
  let height = bounds["height"].as_u64()?;
  Some(PhysicalSize::new(
    u32::try_from(width).ok()?,
    u32::try_from(height).ok()?,
  ))
}

/// The size of the area a window's webviews are laid out into.
///
/// The bridge reports both rectangles: `bounds` is the window including its frame, `contentBounds`
/// is what the frame leaves. Tauri's `Resized` is the surface size the reference reads from its
/// own window record, so the content rectangle is the one that means the same thing — a decorated
/// window that reported the outer size would lay its webviews out under the title bar and past the
/// bottom edge.
fn content_size(params: &Value) -> Option<PhysicalSize<u32>> {
  physical_size(&params["contentBounds"]).or_else(|| physical_size(&params["bounds"]))
}

/// A physical-pixel position from a `bounds` payload.
fn physical_position(bounds: &Value) -> Option<PhysicalPosition<i32>> {
  let x = bounds["x"].as_i64()?;
  let y = bounds["y"].as_i64()?;
  Some(PhysicalPosition::new(
    i32::try_from(x).ok()?,
    i32::try_from(y).ok()?,
  ))
}

/// The scale factor each window was last reported at.
///
/// Electron reports the factor with every `window.event`, while Tauri only wants to hear about
/// it when it changes, which is what a `ScaleFactorChanged` event says.
fn scale_factors() -> &'static Mutex<HashMap<WindowId, f64>> {
  static SCALE_FACTORS: OnceLock<Mutex<HashMap<WindowId, f64>>> = OnceLock::new();
  SCALE_FACTORS.get_or_init(Default::default)
}

/// Emits `ScaleFactorChanged` when Electron reports a factor the window did not have.
fn observe_scale_factor<T: UserEvent>(
  context: &RuntimeContext<T>,
  window_id: WindowId,
  params: &Value,
) {
  let Some(scale_factor) = params["scaleFactor"].as_f64() else {
    return;
  };
  let previous = scale_factors()
    .lock()
    .unwrap_or_else(|poisoned| poisoned.into_inner())
    .insert(window_id, scale_factor);
  // The first report is the window's current factor, not a change of it: a
  // `ScaleFactorChanged` for it would tell the application the window had moved.
  if previous.is_none_or(|previous| (previous - scale_factor).abs() < f64::EPSILON) {
    return;
  }
  let Some(size) = content_size(params) else {
    return;
  };
  crate::webview::layout_app_window(context, window_id, size);
  send_window_event(
    context,
    window_id,
    WindowEvent::ScaleFactorChanged {
      scale_factor,
      new_inner_size: size,
    },
  );
}

/// Turns one `window.event` into the portable window events Tauri has.
fn on_window_event<T: UserEvent>(context: &RuntimeContext<T>, params: &Value) {
  let Some(window_id) = window_id(params) else {
    return;
  };
  if let Some(serial) = raw_window_id(params) {
    register_window_serial(window_id, serial);
  }
  let Some(name) = params["event"].as_str() else {
    return;
  };

  match name {
    "resize" => {
      if let Some(size) = content_size(params) {
        // The children follow the window before the application hears about the
        // resize, which is the order the reference uses: a handler that reads a
        // webview's bounds must see the new ones.
        crate::webview::layout_app_window(context, window_id, size);
        send_window_event(context, window_id, WindowEvent::Resized(size));
      }
    }
    "move" => {
      if let Some(position) = physical_position(&params["bounds"]) {
        send_window_event(context, window_id, WindowEvent::Moved(position));
      }
    }
    "focus" => send_window_event(context, window_id, WindowEvent::Focused(true)),
    "blur" => send_window_event(context, window_id, WindowEvent::Focused(false)),
    // Tauri has no portable window event for the maximized or fullscreen state, and
    // synthesising one would tell an application something the reference never reports: the
    // geometry these change already arrives as `resize`/`move`, and the state itself is asked
    // from Electron through `window.geometry`.
    "maximize" | "unmaximize" | "enter-full-screen" | "leave-full-screen" => {
      log::debug!("window {window_id:?} reported the `{name}` state change");
    }
    other => log::debug!("ignoring the unhandled window event `{other}`"),
  }

  observe_scale_factor(context, window_id, params);
}

/// Turns `window.themeChanged` into the portable `ThemeChanged` event.
fn on_theme_changed<T: UserEvent>(context: &RuntimeContext<T>, params: &Value) {
  let Some(window_id) = window_id(params) else {
    return;
  };
  if let Some(serial) = raw_window_id(params) {
    register_window_serial(window_id, serial);
  }
  let theme = match params["theme"].as_str() {
    Some("dark") => Theme::Dark,
    Some("light") => Theme::Light,
    // `dark` is the authoritative boolean the bridge sends alongside the name; an unknown name
    // is still classifiable through it.
    _ => match params["dark"].as_bool() {
      Some(true) => Theme::Dark,
      Some(false) => Theme::Light,
      None => return,
    },
  };
  send_window_event(context, window_id, WindowEvent::ThemeChanged(theme));
}

/// Delivers one `window.ipc.postMessage` to the application's IPC handler.
///
/// This is the browser-side half of the reference's `cef_impl/ipc.rs`: the preload script
/// exposes `window.ipc.postMessage`, the bridge forwards what it was given as
/// `{ webviewId, documentGeneration, url, body }`, and the body becomes the request the
/// application's handler sees. A URL `http::Request` refuses drops the message, exactly as it
/// does in the reference.
fn on_ipc_message<T: UserEvent>(_context: &RuntimeContext<T>, params: &Value) {
  let Some(webview_id) = webview_id(params) else {
    return;
  };
  let url = params["url"].as_str().unwrap_or_default();
  let body = params["body"].as_str().unwrap_or_default().to_owned();
  let Ok(request) = http::Request::builder().uri(url).body(body) else {
    log::debug!("dropping an IPC message whose URL `http::Request` refused: {url}");
    return;
  };

  with_client::<T, _>(webview_id, |client| {
    if let Some(handler) = client.handlers.ipc_handler.as_ref() {
      (*handler)(webview_id, request);
    }
  });
}

/// Routes one event from the Electron main process.
///
/// Returns `false` when the loop must stop, which is `ipc.closed` and nothing else: every other
/// event is a report about a window, a webview or a policy request, and losing one must not end
/// the application.
pub(crate) fn dispatch<T: UserEvent, F: FnMut(RunEvent<T>)>(
  context: &RuntimeContext<T>,
  event: electron_rs::Event,
  callback: &mut F,
) -> bool {
  let Some(name) = event["event"].as_str() else {
    // `electron-rs` routes RPC replies and `command.output` before publishing, so a line
    // without an event name is a shape this runtime does not know.
    log::debug!("ignoring an Electron message without an event name");
    return true;
  };
  let params = &event["params"];

  match name {
    "ipc.closed" => return process::on_ipc_closed(context, params),
    "ready" => process::on_ready(context, params),
    "ipc.message" => on_ipc_message(context, params),
    "window.event" => on_window_event(context, params),
    "window.themeChanged" => on_theme_changed(context, params),
    "window.closeRequested" => life_span::on_close_requested(context, params),
    "window.closed" => life_span::on_window_closed(context, params),
    "webview.closed" => life_span::on_webview_closed(context, params),
    "webview.processGone" => life_span::on_process_gone(context, params),
    "webview.newWindow" => life_span::on_new_window(context, params),
    "webview.attached" => frame::on_attached(context, params),
    "webview.navigationCommitted" => frame::on_navigation_committed(context, params),
    "webview.loadStarted" => load::on_load_started(context, params),
    "webview.loadFinished" => load::on_load_finished(context, params),
    "webview.loadFailed" => load::on_load_failed(context, params),
    "webview.navigationRequested" => load::on_navigation_requested(context, params),
    "webview.titleChanged" => display::on_title_changed(context, params),
    "webview.consoleMessage" => display::on_console_message(context, params),
    "download.requested" => download::on_requested(context, params),
    "download.finished" => download::on_finished(context, params),
    "permission.requested" => permission::on_requested(context, params),
    "protocol.request" => {
      crate::electron_impl::request_handler::on_protocol_request(context, params)
    }
    "menu.command" | "app.menuCommand" => command::on_menu_command(context, params),
    "app.exitRequested" => process::on_exit_requested(context),
    "app.reopen" => process::on_reopen(context, params, callback),
    "browser.openRequested" => process::on_browser_open_requested(context, params),
    other => log::debug!("ignoring the unhandled Electron event `{other}`"),
  }

  true
}
