// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

use std::any::Any;
use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::sync::{
  Mutex, OnceLock,
  mpsc::{self, Receiver, Sender},
};

use electron_rs::{RpcRequest, rpc};
use serde_json::Value;
use tauri_runtime::{
  Cookie, Error, Result, Runtime, UserEvent, WebviewDispatch,
  dpi::{PhysicalPosition, PhysicalSize, Position, Rect, Size},
  webview::{
    DetachedWebview, DocumentTitleChangedHandler, DownloadHandler, InitializationScript,
    NavigationHandler, NewWindowHandler, OnPageLoadHandler, OnWebContentProcessTerminateHandler,
    PendingWebview, WebviewAttributes,
  },
  window::WindowId,
};
use tauri_utils::config::{BackgroundThrottlingPolicy, Color};
use url::Url;

use crate::{
  electron_impl::client,
  runtime::{ElectronRuntime, IPC_TIMEOUT, Message, RuntimeContext, create_webview_err},
};

/// A handle to the native Electron browser backing a Tauri webview.
///
/// This is the runtime-specific webview object exposed through
/// [`tauri_runtime::WebviewDispatch::with_webview`].
#[derive(Clone)]
pub struct Webview {
  browser: ElectronWebviewHandle,
  snapshot: WebviewSnapshot,
  frame_navigation_state: crate::FrameNavigationState,
  popups: Vec<Webview>,
  opener: Option<crate::FrameNavigationState>,
}

/// The native browser behind a [`Webview`], as Electron numbers it.
///
/// Electron owns the browser in another process, so there is no in-process object
/// to hand out. This wraps the `webContents.id` the main process reports, which is
/// the identity every `webview.*` command and every frame notification uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ElectronWebviewHandle {
  web_contents_id: i32,
}

impl ElectronWebviewHandle {
  pub(crate) fn new(web_contents_id: i32) -> Self {
    Self { web_contents_id }
  }

  /// The Electron `webContents.id` of this browser.
  pub fn web_contents_id(&self) -> i32 {
    self.web_contents_id
  }
}

/// Native state sampled on the runtime's own thread immediately before a
/// `with_webview` callback. This does not assert renderer responsiveness or
/// that the view is unobscured on screen.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct WebviewSnapshot {
  /// Native browser identity, distinct for each popup.
  pub browser_id: i32,
  /// Native JavaScript dialog observation. Unknown is distinct from absent.
  pub dialogs: crate::NativeDialogObservation,
  /// All-frame document generation validated against the current native frame
  /// identities and load state. `None` means document admission is unavailable.
  pub document: Option<crate::NativeDocumentToken>,
  /// Runtime window label; a popup the runtime opened as a window of its own has
  /// its own label and is not reported through its opener.
  pub window_label: Option<String>,
  /// Opaque lifetime of the runtime window. Labels and native handle values
  /// may be reused after teardown; this token distinguishes their replacements.
  /// `None` means the runtime could not observe a native window lifetime.
  pub window: Option<crate::NativeWindowToken>,
  /// Whether the actual native parent matches the observed native window.
  ///
  /// Electron keeps the mapping itself: the runtime told the main process which
  /// window hosts this webview when it created it, and only a confirmed
  /// `webview.reparent` changes it. There is no independent native parent to
  /// compare against, so this reports `Some(true)` for a live webview and `None`
  /// once the window is gone.
  pub parent_matches: Option<bool>,
  /// Current bounds relative to the native parent, in physical pixels. Electron
  /// reports the guest's rect in physical pixels, so no logical conversion is
  /// applied.
  pub bounds: Option<Rect>,
  /// Native view visibility. `None` means native inspection was unavailable.
  /// Visibility is separate from occlusion, minimization, and page lifecycle.
  pub visible: Option<bool>,
}

impl Webview {
  pub(crate) fn new(
    browser_id: i32,
    snapshot: WebviewSnapshot,
    frame_navigation_state: crate::FrameNavigationState,
  ) -> Self {
    Self {
      browser: ElectronWebviewHandle::new(browser_id),
      snapshot,
      frame_navigation_state,
      popups: Vec::new(),
      opener: None,
    }
  }

  /// Returns the native state sampled for this `with_webview` callback.
  /// Retaining the handle does not refresh this observation.
  pub fn snapshot(&self) -> &WebviewSnapshot {
    &self.snapshot
  }

  /// Returns read-only live navigation state for this native browser lifetime.
  /// Unlike `snapshot`, this handle follows subsequent native frame events.
  pub fn frame_navigation_state(&self) -> &crate::FrameNavigationState {
    &self.frame_navigation_state
  }

  /// Marks this handle as the popup of that opener lifetime.
  pub(crate) fn set_opener(&mut self, opener: crate::FrameNavigationState) {
    self.opener = Some(opener);
  }

  /// Native popup descendants sampled in this same callback.
  ///
  /// A popup the runtime allowed for `window.open` is a browser of its own, so it
  /// is reported here rather than through a Tauri window label. Popup windows have
  /// no Tauri label and retain their actual opener.
  pub fn popups(&self) -> &[Webview] {
    &self.popups
  }

  /// Exact native opener lifetime, if this is a runtime-owned popup.
  pub fn opener(&self) -> Option<&crate::FrameNavigationState> {
    self.opener.as_ref()
  }

  /// Select an observed document within this runtime-owned browser family.
  /// The returned snapshot is valid only for the current native callback.
  pub fn for_document(&self, document: &crate::NativeDocumentToken) -> Option<&Webview> {
    std::iter::once(self)
      .chain(self.popups.iter())
      .find(|view| view.snapshot.document.as_ref() == Some(document))
  }

  /// Returns the [`ElectronWebviewHandle`] backing this webview.
  ///
  /// The reference hands out a `cef::Browser`, from which the rest of the CEF API
  /// is reachable in-process. Electron owns the browser in another process, so what
  /// is reachable from here is the `webContents.id`: address it with the bridge's
  /// `webview.*` commands.
  pub fn browser(&self) -> ElectronWebviewHandle {
    self.browser
  }
}

/// The Electron release this runtime drives.
///
/// The reference reports the Chromium version it embeds. Electron reports its own
/// version in the handshake (`ready`), and `webview_version` has no runtime handle
/// to read it from, so this reports the release the runtime was built against
/// ([`crate::ELECTRON_API_VERSION_LAST`]) rather than the one that happens to be
/// running.
pub fn webview_version() -> tauri_runtime::Result<String> {
  Ok(crate::ELECTRON_API_VERSION_LAST.to_string())
}

/// The settings a webview is created with, as the bridge can honour them.
///
/// The reference's `BrowserSettingsCallback` gets `cef::BrowserSettings`, a struct
/// with a field for every Chromium content setting. Electron takes the same
/// decisions as named fields of `webview.create`'s config, so this mirrors those
/// and keeps [`additional`](Self::additional) for anything the bridge has no typed
/// field for.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct ElectronBrowserSettings {
  /// Whether page scripts run.
  pub javascript_disabled: bool,
  /// The colour painted behind the page. `None` leaves Chromium's own white.
  pub background_color: Option<Color>,
  /// Whether the page paints its own transparency.
  pub transparent: bool,
  /// Whether the click that activates the window reaches the page.
  pub accept_first_mouse: bool,
  /// Whether a DevTools window may be opened for this webview.
  pub devtools: bool,
  /// How Chromium throttles the page while it is hidden.
  pub background_throttling: Option<BackgroundThrottlingPolicy>,
  /// Settings `webview.create` has no typed field for, sent verbatim.
  pub additional: BTreeMap<String, Value>,
}

/// Maps the subset of [`WebviewAttributes`] that `webview.create` supports.
///
/// The following Tauri webview attributes have no per-webview equivalent in
/// Electron and are intentionally ignored here;
/// [`warn_about_unsupported_attributes`] says so when a webview sets one to
/// something the runtime cannot honour:
/// - `additional_browser_args`: a WebView2 environment option. Chromium's command
///   line is per process, so `Electron::command_line_arg` is the API for it.
/// - `scroll_bar_style`: Chromium's overlay scrollbars are a process-wide
///   feature, reachable through `Electron::enable_features`.
/// - `browser_extensions_enabled`, `extensions_path`: the bridge exposes no
///   extension loading, and Electron loads extensions per session.
/// - `general_autofill_enabled`: already `false` in effect. The runtime turns
///   `autofill.profile_enabled` off on every profile (see `preferences`), and
///   `Electron::profile_preference` is how an application turns it back on.
/// - `allow_link_preview`: Chromium has no link previews, so there is nothing
///   to allow or forbid.
///
/// `data_store_identifier` *is* served, as its own session partition, the same way
/// `data_directory` is (see `request_context`).
///
/// `proxy_url` is handled separately through the session preference,
/// `zoom_hotkeys_enabled` through the main process's input handling, because zoom
/// reaches a page through Chromium's accelerator table rather than through a
/// browser setting, and `user_agent` through `webview.create`'s `userAgent`,
/// because Electron fixes it per webview rather than per process.
///
/// Chromium paints an opaque white background unless the page asks for
/// transparency or names a colour of its own, so a webview gets neither a
/// `backgroundColor` nor `transparent` unless it asked for one.
fn browser_settings_from_webview_attributes(
  webview_attributes: &WebviewAttributes,
) -> ElectronBrowserSettings {
  ElectronBrowserSettings {
    javascript_disabled: webview_attributes.javascript_disabled,
    background_color: webview_attributes.background_color,
    transparent: webview_attributes.transparent,
    accept_first_mouse: webview_attributes.accept_first_mouse,
    devtools: webview_attributes.devtools.unwrap_or(true),
    background_throttling: webview_attributes.background_throttling.clone(),
    additional: BTreeMap::new(),
  }
}

/// Warns about the attributes of a webview that the Electron runtime cannot honour.
///
/// Only a value the runtime would have to act on is reported, so a configuration
/// that leaves an attribute at its default stays quiet. Each message names the
/// runtime-wide API that does the same thing where one exists: an application
/// ported from WebView2 or WKWebView may rely on the attribute, and silently
/// dropping it is the worst outcome.
fn warn_about_unsupported_attributes(label: &str, attributes: &WebviewAttributes) {
  if attributes.additional_browser_args.is_some() {
    log::warn!(
      "webview {label:?} sets additional_browser_args, which the Electron runtime does not \
       support: Chromium's command line is per process, not per webview. Use \
       `Electron::command_line_arg` to pass switches to the browser process."
    );
  }
  if attributes.browser_extensions_enabled || attributes.extensions_path.is_some() {
    log::warn!(
      "webview {label:?} asks for browser extensions, which the Electron runtime does not \
       support: the bridge exposes no extension loading."
    );
  }
  // The overlay variant only exists on Windows; elsewhere the configuration maps
  // to `Default` before it reaches the runtime.
  #[cfg(windows)]
  if !matches!(
    attributes.scroll_bar_style,
    tauri_runtime::webview::ScrollBarStyle::Default
  ) {
    log::warn!(
      "webview {label:?} asks for overlay scrollbars, which the Electron runtime cannot apply \
       per webview: Chromium's overlay scrollbars are a process-wide feature. Enable it for \
       every webview with `Electron::enable_features([\"OverlayScrollbar\"])`."
    );
  }
}

/// A Chrome DevTools Protocol notification observed on a native browser.
///
/// Observers see the whole browser, including requests issued by the runtime
/// itself and by other callers, so nothing here is scoped to one producer.
/// No notification names a browser, and none has to: an observer is registered
/// on one native browser and never receives another's traffic.
///
/// The bridge exposes no CDP session (`webContents.debugger`), so nothing in this
/// runtime produces one of these today; the type is public API, and the two
/// dispatcher methods that would feed it report unsupported.
#[derive(Debug, Clone)]
pub enum DevToolsProtocol {
  /// The raw agent message, before it is classified as an event or a result.
  Message(Vec<u8>),
  /// An agent event. Events are unsolicited and carry no request identifier.
  Event { method: String, params: Vec<u8> },
  /// The result of one request.
  ///
  /// `message_id` correlates with the `id` of the request that produced it.
  /// Compare it against an identifier obtained from
  /// [`allocate_devtools_message_id`](crate::allocate_devtools_message_id);
  /// a result whose identifier you did not allocate answers someone else's
  /// request. Numeric correlation does not authorize a browser or document.
  MethodResult {
    message_id: i32,
    success: bool,
    result: Vec<u8>,
  },
}

pub(crate) type DevToolsProtocolHandler = dyn Fn(DevToolsProtocol) + Send + Sync;

/// A family of Chrome commands the reference runtime swallows in an application
/// window.
///
/// A Chrome style CEF browser keeps its whole accelerator table live even when it
/// is hosted as a child view with no browser UI, so Ctrl+N opens a real Chrome
/// window next to the app's and Ctrl+P prints the app's own markup. The reference
/// blocks the families below by default and lets an application keep one.
///
/// Electron binds no Chrome accelerator table at all: new-window, print,
/// save-page, view-source, omnibox, history, downloads and settings commands
/// simply do not exist in an Electron window, so there is nothing to block and
/// nothing to keep. The type and the builder method exist for API parity, and a
/// non-empty list is reported as unsupported rather than silently ignored.
///
/// DevTools and zoom are not here: they follow `WebviewAttributes::devtools` and
/// `WebviewAttributes::zoom_hotkeys_enabled`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ChromeCommandGroup {
  /// Ctrl+N, Ctrl+Shift+N, Ctrl+T and the whole tab strip: new window, new incognito
  /// window, new tab, duplicate, restore, reorder, and select tab 1-8.
  WindowAndTab,
  /// Ctrl+P, Ctrl+S, Ctrl+U, Ctrl+O: print, print without preview, save page, view
  /// source, open file, and the PWA install and shortcut commands.
  Document,
  /// Ctrl+L and its neighbours: focus the omnibox, the search box, the toolbar, the menu
  /// bar or the bookmarks bar, plus Home and open-current-URL.
  BrowserChrome,
  /// Ctrl+H, Ctrl+J, Ctrl+D, Ctrl+Shift+Delete and the rest: history, downloads,
  /// bookmarks, settings, clear browsing data, the task manager, sign-in, about and
  /// feedback.
  BrowserSurface,
  /// Alt+Left and Alt+Right: back and forward through the session history.
  History,
}

impl ChromeCommandGroup {
  /// Every group, which is what the reference blocks when a webview allows none.
  /// The blocklist is resolved from this, so a variant added here is blocked by
  /// default there.
  pub(crate) const ALL: &'static [Self] = &[
    Self::WindowAndTab,
    Self::Document,
    Self::BrowserChrome,
    Self::BrowserSurface,
    Self::History,
  ];
}

/// One message a renderer wrote to the JavaScript console.
///
/// Reported on the runtime's own thread. Observing a message neither suppresses
/// Chromium's own logging of it nor changes what DevTools shows.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct ConsoleMessage {
  /// How severe the renderer considers the message.
  pub level: ConsoleMessageLevel,
  /// The message text, already formatted by the renderer the way DevTools shows
  /// it.
  pub message: String,
  /// What wrote the message — a script URL, usually. Empty when Electron names
  /// none.
  pub source: String,
  /// The 1-based line in `source`. Zero when Electron names none.
  pub line: i32,
}

/// The severity of a [`ConsoleMessage`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ConsoleMessageLevel {
  /// `console.debug`.
  Verbose,
  /// `console.log` and `console.info`.
  Info,
  /// `console.warn`.
  Warning,
  /// `console.error`, and messages the renderer itself reports as errors, such as
  /// an uncaught exception or a blocked subresource.
  Error,
  /// A fatal log severity. No console API produces one, and Electron reports no
  /// such level, so this variant is never emitted.
  Fatal,
  /// A severity this build of the runtime does not name.
  Other,
}

/// Synchronous observer of renderer console output.
pub type ConsoleMessageHandler = dyn Fn(ConsoleMessage) + Send + Sync + 'static;

/// Last look at a webview's [`ElectronBrowserSettings`] before it is created.
pub type BrowserSettingsCallback = dyn Fn(&mut ElectronBrowserSettings) + Send + Sync + 'static;

impl ConsoleMessage {
  /// Maps the `level` string of Electron's `console-message` payload.
  ///
  /// Electron reports `debug`, `info`, `warning` and `error`; anything else is
  /// [`ConsoleMessageLevel::Other`] so a future level is forwarded rather than
  /// dropped.
  pub(crate) fn from_electron(level: &str, message: String, source: String, line: i32) -> Self {
    Self {
      level: match level {
        "debug" => ConsoleMessageLevel::Verbose,
        "info" => ConsoleMessageLevel::Info,
        "warning" => ConsoleMessageLevel::Warning,
        "error" => ConsoleMessageLevel::Error,
        _ => ConsoleMessageLevel::Other,
      },
      message,
      source,
      line,
    }
  }
}

pub(crate) enum WebviewMessage {
  EvaluateScript(String),
  EvaluateScriptWithCallback(String, Box<dyn Fn(String) + Send + 'static>),
  Navigate(Url),
  Reload,
  GoBack,
  CanGoBack(Sender<Result<bool>>),
  GoForward,
  CanGoForward(Sender<Result<bool>>),
  Print,
  Close,
  Show,
  Hide,
  SetPosition(Position),
  SetSize(Size),
  SetBounds(Rect),
  SetFocus,
  Reparent(WindowId, Sender<Result<()>>),
  SetAutoResize(bool),
  SetZoom(f64),
  SetBackgroundColor(Option<Color>),
  ClearAllBrowsingData,
  Url(Sender<Result<String>>),
  Bounds(Sender<Result<Rect>>),
  Position(Sender<Result<PhysicalPosition<i32>>>),
  Size(Sender<Result<PhysicalSize<u32>>>),
  WithWebview(Box<dyn FnOnce(Webview) + Send>),
  CookiesForUrl(Url, Sender<Result<Vec<Cookie<'static>>>>),
  Cookies(Sender<Result<Vec<Cookie<'static>>>>),
  SetCookie(Cookie<'static>),
  DeleteCookie(Cookie<'static>),
  #[cfg(any(debug_assertions, feature = "devtools"))]
  OpenDevTools,
  #[cfg(any(debug_assertions, feature = "devtools"))]
  CloseDevTools,
  #[cfg(any(debug_assertions, feature = "devtools"))]
  IsDevToolsOpen(Sender<bool>),
  SendDevToolsMessage(Vec<u8>, Sender<Result<()>>),
  OnDevToolsProtocol(Arc<DevToolsProtocolHandler>, Sender<Result<()>>),
}

/// A webview's bounds expressed as a fraction of its parent window, used to
/// reposition/resize auto-resize webviews when the parent window changes size.
#[derive(Clone, Copy)]
pub(crate) struct BoundsRate {
  pub(crate) x: f32,
  pub(crate) y: f32,
  pub(crate) width: f32,
  pub(crate) height: f32,
}

impl Default for BoundsRate {
  fn default() -> Self {
    Self {
      x: 0.,
      y: 0.,
      width: 1.,
      height: 1.,
    }
  }
}

/// The application callbacks one webview registered, mirroring
/// `TauriCefBrowserClientHandlers`.
///
/// The reference hands these to `TauriCefBrowserClient` when it builds the client.
/// The IPC handler Tauri registers, after the webview id is bound to it.
pub(crate) type IpcHandler = dyn Fn(u32, http::Request<String>) + Send + Sync;

/// Here they are parked in the live webview state, and
/// `electron_impl::client::with_client` borrows them for the duration of one event
/// — which is exactly the lifetime a CEF callback has. `ipc_handler` is the one
/// shape that changes: the reference's takes a `DetachedWebview`, which the client
/// cannot build, so `webview.rs` wraps it into a closure over the webview id.
pub(crate) struct WebviewHandlers<T: UserEvent> {
  pub(crate) frame_event_handler: Option<Arc<crate::FrameEventHandler>>,
  pub(crate) ipc_handler: Option<Arc<IpcHandler>>,
  pub(crate) on_page_load_handler: Option<Box<OnPageLoadHandler>>,
  pub(crate) document_title_changed_handler: Option<Box<DocumentTitleChangedHandler>>,
  pub(crate) navigation_handler: Option<Box<NavigationHandler>>,
  pub(crate) new_window_handler: Option<Box<NewWindowHandler<T, ElectronRuntime<T>>>>,
  pub(crate) download_handler: Option<Arc<DownloadHandler>>,
  pub(crate) console_message_handler: Option<Arc<ConsoleMessageHandler>>,
  pub(crate) permission_request_handler: Option<Box<client::PermissionRequestHandler>>,
  pub(crate) web_content_process_terminate_handler:
    Option<Box<OnWebContentProcessTerminateHandler>>,
}

/// The runtime's live per-webview state.
///
/// The reference keeps this in `AppWindow::children`. `RuntimeContext` exposes
/// windows immutably here (`with_window` only borrows `&AppWindow`), so the state
/// lives beside the message handler that mutates it. Every entry is created by
/// [`build_and_attach_webview`] and only mutated on the run-loop thread; the
/// registry lock is never held across an RPC round trip.
pub(crate) struct AppWebview {
  pub(crate) webview_id: u32,
  pub(crate) label: String,
  pub(crate) window_id: WindowId,
  /// The native browser identity reported in frame events and snapshots.
  ///
  /// The reference reports `Browser::identifier()`, which CEF hands out
  /// synchronously when the browser is created. Electron names the guest's
  /// `webContents` only in `webview.attached`, after the guest exists, so the
  /// runtime's own webview id is what this reports: it is unique for the life of
  /// the process, so a frame event is never attributed to a different webview, and
  /// it is the id every `webview.*` command already takes.
  pub(crate) browser_id: i32,
  pub(crate) frame_navigation_state: crate::FrameNavigationState,
  /// The popups the runtime opened for this webview's `window.open` calls.
  pub(crate) popup_family: Arc<crate::popup::PopupFamily>,
  pub(crate) dialogs: crate::dialog::DialogState,
  /// Whether a DevTools window may be opened for this webview. The DevTools
  /// *protocol* is a separate question: the bridge exposes no CDP session either
  /// way.
  pub(crate) devtools_enabled: bool,
  pub(crate) bounds_rate: Option<BoundsRate>,
  pub(crate) page_fullscreen: PageFullscreen,
  /// The URL this webview last committed.
  ///
  /// The reference reads it back from `BrowserHost`; Electron reports it through
  /// `webview.url`, which is a round trip, so the value is recorded when the
  /// document commits and [`AppWebview::url`] answers from it.
  pub(crate) url: Arc<Mutex<String>>,
  /// The application's callbacks, type-erased because this registry is not generic
  /// over the application's event type. See [`WebviewHandlers`].
  handlers: Option<Arc<dyn Any + Send + Sync>>,
}

/// Whether the page shows an element through the HTML Fullscreen API.
///
/// Electron reports the transition through `enter-html-full-screen` /
/// `leave-html-full-screen`, which the bridge does not forward, so this stays
/// [`PageFullscreen::Off`] until it does. The window-fullscreen bookkeeping
/// (`owns_window`) is runtime-side and ports as-is.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PageFullscreen {
  #[default]
  Off,
  /// `owns_window` when the window was put in fullscreen for this page, so leaving it
  /// restores the window; a window already fullscreen by the app or the user stays that way.
  On { owns_window: bool },
}

impl AppWebview {
  /// The URL this webview last committed.
  ///
  /// The reference keeps the URL beside the browser because CEF reports it through
  /// `BrowserHost`; Electron reports it through `webview.url`, so the committing
  /// navigation records it here and this answers without a round trip.
  pub fn url(&self) -> String {
    self.url.lock().map(|url| url.clone()).unwrap_or_default()
  }

  /// The application's callbacks, when they are of this application's event type.
  pub(crate) fn handlers<T: UserEvent>(&self) -> Option<&Mutex<WebviewHandlers<T>>> {
    let handlers: &dyn Any = self.handlers.as_deref()?;
    handlers.downcast_ref()
  }

  fn resize<T: UserEvent>(
    &self,
    context: &RuntimeContext<T>,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
  ) {
    // Electron accepts the guest's rect in either unit, so the reference's
    // offscreen branch (which stored a logical rect) has no counterpart: there are
    // no offscreen webviews in this runtime.
    let bounds = Rect {
      position: PhysicalPosition::new(x, y).into(),
      size: PhysicalSize::new(width.max(0) as u32, height.max(0) as u32).into(),
    };
    if let Err(error) = call(
      context,
      rpc::webview::SetBounds {
        webview_id: self.webview_id,
        bounds: bounds_input(bounds),
      },
    ) {
      log::warn!(
        "[electron] resizing webview {:?} failed: {error}",
        self.label
      );
    }
  }

  pub(crate) fn set_bounds<T: UserEvent>(
    &mut self,
    context: &RuntimeContext<T>,
    parent_size: PhysicalSize<u32>,
    scale: f64,
    bounds: Rect,
  ) {
    let position = bounds.position.to_physical::<i32>(scale);
    let size = bounds.size.to_physical::<u32>(scale);

    let x = position.x;
    let y = position.y;
    let w = size.width as i32;
    let h = size.height as i32;

    if self.bounds_rate.is_some() {
      let win_w = parent_size.width.max(1) as f32;
      let win_h = parent_size.height.max(1) as f32;
      self.bounds_rate = Some(BoundsRate {
        x: x as f32 / win_w,
        y: y as f32 / win_h,
        width: w as f32 / win_w,
        height: h as f32 / win_h,
      });
    }

    self.resize(context, x, y, w, h);
  }

  pub(crate) fn set_visible<T: UserEvent>(&self, context: &RuntimeContext<T>, visible: bool) {
    if let Err(error) = call(
      context,
      rpc::webview::SetVisibility {
        webview_id: self.webview_id,
        visible,
      },
    ) {
      log::warn!(
        "[electron] setting webview {:?} visibility failed: {error}",
        self.label
      );
    }
  }

  /// The guest's rect as Electron reports it, in physical pixels.
  pub(crate) fn bounds<T: UserEvent>(&self, context: &RuntimeContext<T>) -> Result<Rect> {
    let snapshot = call(
      context,
      rpc::webview::Bounds {
        webview_id: self.webview_id,
      },
    )?;
    Ok(rect_from_rectangle(snapshot.bounds))
  }

  /// Samples the state a `with_webview` callback sees.
  fn snapshot<T: UserEvent>(&self, context: &RuntimeContext<T>) -> WebviewSnapshot {
    // The reference validates the ready generation against the browser's live
    // frame identities. Electron keeps that list in the main process and the
    // bridge reports no frame list, so the state-only read is the strongest
    // admission this runtime can make: it still requires every tracked frame to be
    // attached and load completion to have been observed.
    let document = self.frame_navigation_state.document();
    let window = context.with_window(self.window_id, |window| {
      (window.label.clone(), window.lifetime.clone())
    });
    let (window_label, window) = match window {
      Some((label, lifetime)) => (Some(label), Some(lifetime)),
      None => (None, None),
    };
    let native = call(
      context,
      rpc::webview::Bounds {
        webview_id: self.webview_id,
      },
    )
    .ok();
    WebviewSnapshot {
      browser_id: self.browser_id,
      dialogs: self.dialogs.snapshot(document.as_ref()),
      document,
      window_label,
      parent_matches: window.is_some().then_some(true),
      window,
      bounds: native
        .as_ref()
        .map(|snapshot| rect_from_rectangle(snapshot.bounds)),
      visible: native.map(|snapshot| snapshot.visible),
    }
  }
}

/// The live webviews, keyed by the id their dispatcher carries.
static WEBVIEWS: OnceLock<Mutex<HashMap<u32, AppWebview>>> = OnceLock::new();

fn webviews() -> &'static Mutex<HashMap<u32, AppWebview>> {
  WEBVIEWS.get_or_init(Default::default)
}

/// Takes one webview out of the registry so a handler can borrow it mutably
/// without holding the registry lock across an RPC round trip.
fn take_webview(webview_id: u32) -> Option<AppWebview> {
  webviews()
    .lock()
    .unwrap_or_else(|poisoned| poisoned.into_inner())
    .remove(&webview_id)
}

/// Puts a webview back after its handler ran.
fn put_webview(child: AppWebview) {
  webviews()
    .lock()
    .unwrap_or_else(|poisoned| poisoned.into_inner())
    .insert(child.webview_id, child);
}

/// Runs `f` with the live state of one webview.
pub(crate) fn with_app_webview<R>(webview_id: u32, f: impl FnOnce(&AppWebview) -> R) -> Option<R> {
  webviews().lock().ok()?.get(&webview_id).map(f)
}

/// The ids of every webview hosted by one window.
pub(crate) fn webview_ids_for_window(window_id: WindowId) -> Vec<u32> {
  webviews()
    .lock()
    .unwrap_or_else(|poisoned| poisoned.into_inner())
    .values()
    .filter(|child| child.window_id == window_id)
    .map(|child| child.webview_id)
    .collect()
}

/// Records the HTML-fullscreen state of one webview.
pub(crate) fn set_page_fullscreen(webview_id: u32, page_fullscreen: PageFullscreen) {
  if let Some(child) = webviews()
    .lock()
    .unwrap_or_else(|poisoned| poisoned.into_inner())
    .get_mut(&webview_id)
  {
    child.page_fullscreen = page_fullscreen;
  }
}

/// Forgets one webview, once Electron has destroyed its browser.
pub(crate) fn forget_webview(webview_id: u32) {
  webviews()
    .lock()
    .unwrap_or_else(|poisoned| poisoned.into_inner())
    .remove(&webview_id);
}

/// Forgets every webview hosted by a window that is going away.
pub(crate) fn forget_window_webviews(window_id: WindowId) {
  webviews()
    .lock()
    .unwrap_or_else(|poisoned| poisoned.into_inner())
    .retain(|_, child| child.window_id != window_id);
}

/// The Electron window number behind a Tauri [`WindowId`].
///
/// `tauri_runtime` hands out `WindowId::from(u32)` and keeps the number private,
/// while every command that addresses a window on the Electron side needs the
/// number. The window factory records it up front and the client records it again
/// from the events Electron sends, which is where both halves appear together;
/// both writes land here.
static WINDOW_NUMBERS: OnceLock<Mutex<HashMap<WindowId, u32>>> = OnceLock::new();

fn window_numbers() -> &'static Mutex<HashMap<WindowId, u32>> {
  WINDOW_NUMBERS.get_or_init(Default::default)
}

/// Pairs a Tauri window with the number Electron knows it by.
pub(crate) fn record_window_number(window_id: WindowId, number: u32) {
  window_numbers()
    .lock()
    .unwrap_or_else(|poisoned| poisoned.into_inner())
    .insert(window_id, number);
}

/// Drops the pair for a window that no longer exists, and every webview it hosted.
pub(crate) fn forget_window_number(window_id: WindowId) {
  window_numbers()
    .lock()
    .unwrap_or_else(|poisoned| poisoned.into_inner())
    .remove(&window_id);
  forget_window_webviews(window_id);
}

fn window_number(window_id: WindowId) -> Result<u32> {
  window_numbers()
    .lock()
    .unwrap_or_else(|poisoned| poisoned.into_inner())
    .get(&window_id)
    .copied()
    .ok_or(Error::WindowNotFound)
}

/// Sends one typed bridge request and decodes its response.
///
/// The bridge's own request types are the wire contract, so call sites name them
/// instead of hand-building JSON; the transport stays the runtime's synchronous
/// IPC.
fn call<T: UserEvent, R: RpcRequest>(
  context: &RuntimeContext<T>,
  request: R,
) -> Result<R::Response> {
  let params =
    serde_json::to_value(&request).map_err(|error| create_webview_err(error.to_string()))?;
  let value = context
    .ipc
    .call(R::METHOD, params, IPC_TIMEOUT)
    .map_err(create_webview_err)?;
  serde_json::from_value(value).map_err(|error| create_webview_err(error.to_string()))
}

fn position_payload(position: Position) -> rpc::webview::Position {
  let (x, y, physical) = match position {
    Position::Physical(position) => (f64::from(position.x), f64::from(position.y), true),
    Position::Logical(position) => (position.x, position.y, false),
  };
  rpc::webview::Position {
    x,
    y,
    physical: Some(physical),
  }
}

fn extent_payload(size: Size) -> rpc::webview::Extent {
  let (width, height, physical) = match size {
    Size::Physical(size) => (f64::from(size.width), f64::from(size.height), true),
    Size::Logical(size) => (size.width, size.height, false),
  };
  rpc::webview::Extent {
    width,
    height,
    physical: Some(physical),
  }
}

fn bounds_input(bounds: Rect) -> rpc::webview::BoundsInput {
  rpc::webview::BoundsInput {
    position: Some(position_payload(bounds.position)),
    size: Some(extent_payload(bounds.size)),
    physical: None,
  }
}

fn rect_from_rectangle(rectangle: rpc::Rectangle) -> Rect {
  Rect {
    position: PhysicalPosition::new(rectangle.x, rectangle.y).into(),
    size: PhysicalSize::new(rectangle.width, rectangle.height).into(),
  }
}

/// The parent window's content size and scale factor, as Electron reports them.
fn window_metrics<T: UserEvent>(
  context: &RuntimeContext<T>,
  window_id: WindowId,
) -> Option<(PhysicalSize<u32>, f64)> {
  let geometry = call(
    context,
    rpc::window::Geometry {
      window_id: window_number(window_id).ok()?,
    },
  )
  .ok()?;
  Some((
    PhysicalSize::new(
      geometry.content_bounds.width,
      geometry.content_bounds.height,
    ),
    geometry.scale_factor,
  ))
}

fn css_color(color: Color) -> String {
  format!(
    "rgba({}, {}, {}, {:.3})",
    color.0,
    color.1,
    color.2,
    color.3 as f64 / 255.0
  )
}

fn rpc_color(color: Color) -> rpc::Color {
  rpc::Color::Rgba([color.0, color.1, color.2, color.3])
}

fn parse_cookies(cookies: &[rpc::webview::Cookie]) -> Vec<Cookie<'static>> {
  cookies
    .iter()
    .map(|cookie| {
      let mut builder = Cookie::build((cookie.name.clone(), cookie.value.clone()))
        .path(cookie.path.clone().unwrap_or_else(|| "/".into()));
      // Host-only cookies must remain host-only when set again.
      if !cookie.host_only.unwrap_or(false)
        && let Some(domain) = &cookie.domain
      {
        builder = builder.domain(domain.clone());
      }
      builder = builder
        .secure(cookie.secure.unwrap_or(false))
        .http_only(cookie.http_only.unwrap_or(false));
      if let Some(same_site) = same_site(cookie.same_site) {
        builder = builder.same_site(same_site);
      }
      if !cookie.session
        && let Some(date) = cookie
          .expiration_date
          .filter(|date| date.is_finite())
          .and_then(|date| {
            cookie::time::OffsetDateTime::from_unix_timestamp_nanos(
              (date * 1_000_000_000.0) as i128,
            )
            .ok()
          })
      {
        builder = builder.expires(date);
      }
      builder.build()
    })
    .collect()
}

fn same_site(value: rpc::webview::SameSite) -> Option<cookie::SameSite> {
  match value {
    rpc::webview::SameSite::Strict => Some(cookie::SameSite::Strict),
    rpc::webview::SameSite::Lax => Some(cookie::SameSite::Lax),
    rpc::webview::SameSite::NoRestriction => Some(cookie::SameSite::None),
    rpc::webview::SameSite::Unspecified => None,
  }
}

fn same_site_payload(value: Option<cookie::SameSite>) -> Option<rpc::webview::SameSite> {
  value.map(|value| match value {
    cookie::SameSite::Strict => rpc::webview::SameSite::Strict,
    cookie::SameSite::Lax => rpc::webview::SameSite::Lax,
    cookie::SameSite::None => rpc::webview::SameSite::NoRestriction,
  })
}

/// Creates a webview inside an existing window.
///
/// The reference posts `Message::CreateWebview` and waits for the event loop, so a
/// failure surfaces to the caller. `webview.create` is a synchronous RPC here, so
/// this performs it inline and needs no round trip through the loop; it is safe on
/// the run-loop thread with the window already registered.
pub(crate) fn create_webview_detached<T: UserEvent>(
  context: &RuntimeContext<T>,
  window_id: WindowId,
  pending: PendingWebview<T, ElectronRuntime<T>>,
) -> Result<DetachedWebview<T, ElectronRuntime<T>>> {
  let label = pending.label.clone();
  let webview_id = context.next_webview_id();
  build_and_attach_webview(context, window_id, webview_id, pending)?;
  Ok(DetachedWebview {
    label,
    dispatcher: ElectronWebviewDispatcher {
      window_id: Arc::new(Mutex::new(window_id)),
      webview_id,
      context: context.clone(),
    },
  })
}

/// Builds a webview and attaches it to `window_id`, then lays it out.
///
/// Works whether the window was just created or has been live for a while, so
/// window and child creation share one path.
fn build_and_attach_webview<T: UserEvent>(
  context: &RuntimeContext<T>,
  window_id: WindowId,
  webview_id: u32,
  pending: PendingWebview<T, ElectronRuntime<T>>,
) -> Result<()> {
  if context.with_window(window_id, |_| ()).is_none() {
    return Err(Error::CreateWebview(
      format!("window {window_id:?} does not exist").into(),
    ));
  }
  let window_number = window_number(window_id)?;
  let (parent_size, scale) =
    window_metrics(context, window_id).unwrap_or((PhysicalSize::new(0, 0), 1.0));

  let PendingWebview {
    label,
    mut webview_attributes,
    runtime_specific_attributes,
    uri_scheme_protocols,
    navigation_handler,
    new_window_handler,
    document_title_changed_handler,
    on_page_load_handler,
    download_handler,
    permission_request_handler,
    web_resource_request_handler,
    on_web_content_process_terminate_handler,
    ipc_handler,
    ..
  } = pending;

  // `PendingWebview::new` hardcodes its `url` field to `tauri://localhost`; the
  // document the application asked for lives in the attributes. Resolving from
  // there is what makes `WebviewUrl::External` and `WebviewUrl::App` mean what
  // they say, and it is what the reference does.
  let url = resolve_webview_url(&webview_attributes.url);

  let bounds_rate = compute_child_bounds_rate(
    webview_attributes.bounds.as_ref(),
    webview_attributes.auto_resize,
    parent_size,
    scale,
  );
  let devtools_enabled = context.devtools_allowed
    && (cfg!(debug_assertions) || cfg!(feature = "devtools"))
    && webview_attributes.devtools.unwrap_or(true);
  let console_message_handler = runtime_specific_attributes.console_message_handler.clone();

  // The reference injects `devtools_shortcut_script()` here because Alloy style
  // keeps none of Chrome's accelerator table. Electron's window binds the chord in
  // the main process (`before-input-event`), so injecting the script as well would
  // toggle DevTools twice — once open, once shut.
  report_unsupported_attributes(&label, &runtime_specific_attributes);
  if web_resource_request_handler.is_some() {
    log::warn!(
      "webview {label:?} registers a web resource request handler, which the Electron runtime \
       cannot drive: the bridge forwards no per-request notification."
    );
    crate::unsupported::report("WebviewAttributes::web_resource_request_handler");
  }

  // Custom protocols are served by the runtime's own registry: the main process
  // reports `protocol.request` and the runtime answers it, so a webview's handlers
  // are registered once per scheme rather than per browser. The reference's handler
  // takes a responder so it can answer later; the registry stores the synchronous
  // shape, because the answer has to travel back over the socket as one
  // `protocol.respond` call anyway.
  for (scheme, handler) in uri_scheme_protocols {
    let label = label.clone();
    let handler: crate::electron_impl::request_handler::SchemeHandler =
      Arc::new(move |request: http::Request<Vec<u8>>| {
        let (tx, rx) = mpsc::channel();
        let responder = Box::new(move |response: http::Response<Cow<'static, [u8]>>| {
          let _ = tx.send(response);
        });
        handler(&label, request, responder);
        rx.recv().unwrap_or_else(|_| {
          http::Response::builder()
            .status(500)
            .body(Cow::Owned(Vec::new()))
            .expect("a 500 response with no headers is always valid")
        })
      });
    context.register_protocol(scheme, handler);
  }

  let frame_navigation_state = crate::FrameNavigationState::new();
  let popup_family = Arc::new(crate::popup::PopupFamily::new(
    frame_navigation_state.clone(),
  ));
  let dialogs = crate::dialog::DialogState::new(frame_navigation_state.clone());
  // The URL the document is created with is the first one `url()` reports; later
  // commits replace it.
  let initial_url = url.clone();

  let initialization_scripts = initialization_scripts(&mut webview_attributes);
  let mut settings = browser_settings_from_webview_attributes(&webview_attributes);
  // Applied last so an application can override what the runtime mapped.
  if let Some(callback) = &runtime_specific_attributes.browser_settings_callback {
    callback(&mut settings);
  }
  warn_about_unsupported_attributes(&label, &webview_attributes);

  let config = rpc::webview::Config {
    webview_id,
    url,
    auto_resize: Some(webview_attributes.auto_resize),
    bounds: webview_attributes.bounds.map(bounds_input),
    // Only the decisions the event stream can actually answer are turned on: a
    // policy flag makes the main process block the operation until the runtime
    // replies, and the bridge forwards no navigation decision request.
    policy: Some(rpc::webview::Policy {
      navigation: None,
      download: Some(download_handler.is_some()),
      permission: Some(permission_request_handler.is_some()),
    }),
    incognito: Some(webview_attributes.incognito),
    data_directory: webview_attributes
      .data_directory
      .as_ref()
      .map(|path| path.display().to_string()),
    user_agent: webview_attributes.user_agent.clone(),
    init_scripts: Some(
      initialization_scripts
        .iter()
        .map(|script| rpc::webview::InitScript::Detailed {
          script: script.script.clone(),
          main_frame_only: script.for_main_frame_only,
        })
        .collect(),
    ),
    preload_scripts: None,
    background_color: settings.background_color.map(rpc_color),
    transparent: settings.transparent.then_some(true),
    accept_first_mouse: Some(settings.accept_first_mouse),
    javascript_disabled: Some(settings.javascript_disabled),
    devtools: Some(devtools_enabled),
    background_throttling: settings
      .background_throttling
      .map(|policy| match policy {
        BackgroundThrottlingPolicy::Disabled => "disabled",
        BackgroundThrottlingPolicy::Suspend => "suspend",
        BackgroundThrottlingPolicy::Throttle => "throttle",
      })
      .map(str::to_owned),
    additional: settings.additional,
  };

  call(
    context,
    rpc::webview::Create {
      window_id: window_number,
      config,
    },
  )?;

  // The reference hands the application callbacks to `TauriCefBrowserClient` and
  // keeps the client on the browser. Here they are parked in the live webview
  // state, which is what `electron_impl::client` borrows when an event names this
  // webview. The IPC handler is the one that changes shape: the reference's takes a
  // `DetachedWebview`, which the client cannot build, so it is wrapped into a
  // closure over the webview id that builds one.
  let ipc_handler = ipc_handler.map(|handler| {
    let handler = Arc::new(Mutex::new(handler));
    let context = context.clone();
    let label = label.clone();
    Arc::new(move |webview_id: u32, request: http::Request<String>| {
      let Ok(handler) = handler.lock() else {
        return;
      };
      handler(
        DetachedWebview {
          label: label.clone(),
          dispatcher: ElectronWebviewDispatcher {
            window_id: Arc::new(Mutex::new(window_id)),
            webview_id,
            context: context.clone(),
          },
        },
        request,
      );
    }) as Arc<dyn Fn(u32, http::Request<String>) + Send + Sync>
  });

  let handlers: WebviewHandlers<T> = WebviewHandlers {
    frame_event_handler: runtime_specific_attributes.frame_event_handler.clone(),
    ipc_handler,
    on_page_load_handler,
    document_title_changed_handler,
    navigation_handler,
    new_window_handler,
    download_handler,
    console_message_handler,
    permission_request_handler,
    web_content_process_terminate_handler: on_web_content_process_terminate_handler,
  };

  put_webview(AppWebview {
    webview_id,
    label,
    window_id,
    browser_id: webview_id as i32,
    frame_navigation_state,
    popup_family,
    dialogs,
    devtools_enabled,
    bounds_rate,
    page_fullscreen: PageFullscreen::Off,
    url: Arc::new(Mutex::new(initial_url)),
    handlers: Some(Arc::new(Mutex::new(handlers))),
  });
  layout_app_window(context, window_id);
  Ok(())
}

/// The URL a [`WebviewUrl`](tauri_utils::config::WebviewUrl) asks to load.
///
/// `App` paths live under the runtime's own custom scheme, which is
/// `tauri://localhost` here — the same form the reference uses, and the one
/// `convertFileSrc` and the IPC origin check expect.
fn resolve_webview_url(url: &tauri_utils::config::WebviewUrl) -> String {
  match url {
    tauri_utils::config::WebviewUrl::External(url) => url.to_string(),
    tauri_utils::config::WebviewUrl::CustomProtocol(url) => url.to_string(),
    tauri_utils::config::WebviewUrl::App(path) => {
      let path = path.to_string_lossy().replace('\\', "/");
      let path = path.trim_start_matches('/');
      if path.is_empty() {
        "tauri://localhost".to_string()
      } else {
        format!("tauri://localhost/{path}")
      }
    }
    // `WebviewUrl` is `#[non_exhaustive]`: a variant this build does not know
    // about cannot be resolved, and the app root is the honest fallback.
    _ => "tauri://localhost".to_string(),
  }
}

/// Reports the per-webview configuration this runtime cannot honour.
fn report_unsupported_attributes(label: &str, attributes: &ElectronWebviewAttributes) {
  if attributes.offscreen.is_some() {
    log::warn!(
      "webview {label:?} asks for offscreen rendering, which the Electron runtime does not \
       support: Electron has no windowless browser, and `capturePage` captures an existing view \
       rather than replacing its render target."
    );
    crate::unsupported::report("ElectronWebviewAttributes::offscreen");
  }
  if matches!(attributes.runtime_style, Some(RuntimeStyle::Chrome)) {
    log::warn!(
      "webview {label:?} asks for Chrome runtime style, which the Electron runtime does not \
       have: an Electron window never shows browser chrome."
    );
    crate::unsupported::report("ElectronWebviewAttributes::runtime_style");
  }
  if !attributes.allowed_chrome_commands.is_empty() {
    log::warn!(
      "webview {label:?} asks to keep Chrome commands, which the Electron runtime does not \
       have: Electron binds no Chrome accelerator table, so there is nothing to swallow and \
       nothing to keep."
    );
    crate::unsupported::report("ElectronWebviewAttributes::allowed_chrome_commands");
  }
  if attributes.frame_event_handler.is_some() {
    log::warn!(
      "webview {label:?} registers a frame event handler, which the Electron runtime cannot \
       drive: the bridge forwards no per-frame notification, so only the runtime's own \
       document-generation bookkeeping sees frame events."
    );
    crate::unsupported::report("ElectronWebviewAttributes::frame_event_handler");
  }
  if attributes.console_message_handler.is_some() {
    log::warn!(
      "webview {label:?} registers a console message handler, which the Electron runtime \
       cannot drive: the bridge forwards no `console-message` notification."
    );
    crate::unsupported::report("ElectronWebviewAttributes::console_message_handler");
  }
}

/// Routes one webview message from the run loop.
pub(crate) fn handle_webview_message<T: UserEvent>(
  context: &RuntimeContext<T>,
  webview_id: u32,
  message: WebviewMessage,
) {
  // Window-dependent messages read window metrics (bounds, size, position,
  // auto-resize, `with_webview`, reparent). Route them before taking the child so
  // those reads do not overlap with the child borrow.
  match message {
    WebviewMessage::SetBounds(_)
    | WebviewMessage::SetSize(_)
    | WebviewMessage::SetPosition(_)
    | WebviewMessage::Position(_)
    | WebviewMessage::Size(_)
    | WebviewMessage::SetAutoResize(_)
    | WebviewMessage::WithWebview(_)
    | WebviewMessage::Reparent(_, _) => {
      handle_window_dependent_webview_message(context, webview_id, message);
    }
    // Closing a webview destroys its guest, so its state goes with it. The
    // reference keeps the child until CEF confirms the close; Electron's
    // `webview.close` is the teardown itself.
    WebviewMessage::Close => {
      let Some(child) = take_webview(webview_id) else {
        return;
      };
      match call(context, rpc::webview::Close { webview_id }) {
        Ok(()) => {
          forget_webview(webview_id);
        }
        Err(error) => {
          log::warn!(
            "[electron] closing webview {:?} failed: {error}",
            child.label
          );
          put_webview(child);
        }
      }
    }
    message => {
      let Some(mut child) = take_webview(webview_id) else {
        return;
      };
      handle_window_independent_webview_message(context, &mut child, message);
      put_webview(child);
    }
  }
}

fn handle_window_dependent_webview_message<T: UserEvent>(
  context: &RuntimeContext<T>,
  webview_id: u32,
  message: WebviewMessage,
) {
  let Some(mut child) = take_webview(webview_id) else {
    return;
  };

  let mut reparent_reply = None;
  match message {
    WebviewMessage::SetBounds(bounds) => {
      if let Some((parent_size, scale)) = window_metrics(context, child.window_id) {
        child.set_bounds(context, parent_size, scale, bounds);
      }
    }
    WebviewMessage::SetSize(size) => {
      if let Some((parent_size, scale)) = window_metrics(context, child.window_id) {
        let bounds = child.bounds(context).unwrap_or_default();
        child.set_bounds(
          context,
          parent_size,
          scale,
          Rect {
            position: bounds.position,
            size,
          },
        );
      }
    }
    WebviewMessage::SetPosition(position) => {
      if let Some((parent_size, scale)) = window_metrics(context, child.window_id) {
        let bounds = child.bounds(context).unwrap_or_default();
        child.set_bounds(
          context,
          parent_size,
          scale,
          Rect {
            position,
            size: bounds.size,
          },
        );
      }
    }
    // Electron reports the guest's rect in physical pixels, so the window scale
    // factor the reference applies to these two has nothing left to convert.
    WebviewMessage::Position(tx) => {
      let position = child
        .bounds(context)
        .map(|bounds| bounds.position.to_physical::<i32>(1.0));
      let _ = tx.send(position);
    }
    WebviewMessage::Size(tx) => {
      let size = child
        .bounds(context)
        .map(|bounds| bounds.size.to_physical::<u32>(1.0));
      let _ = tx.send(size);
    }
    WebviewMessage::SetAutoResize(auto_resize) => {
      if let Err(error) = call(
        context,
        rpc::webview::SetAutoResize {
          webview_id: child.webview_id,
          auto_resize,
        },
      ) {
        log::warn!(
          "[electron] setting auto-resize on webview {:?} failed: {error}",
          child.label
        );
      }
      if auto_resize {
        if let Some((parent_size, scale)) = window_metrics(context, child.window_id) {
          let bounds = child.bounds(context).ok();
          child.bounds_rate = compute_child_bounds_rate(bounds.as_ref(), true, parent_size, scale);
        }
      } else {
        child.bounds_rate = None;
      }
    }
    WebviewMessage::Reparent(target_window_id, tx) => {
      let result = reparent_webview(context, &mut child, target_window_id);
      reparent_reply = Some((tx, result));
    }
    WebviewMessage::WithWebview(callback) => {
      let snapshot = child.snapshot(context);
      let mut native = Webview::new(
        snapshot.browser_id,
        snapshot,
        child.frame_navigation_state.clone(),
      );
      native.popups = child.popup_family.observe();
      callback(native);
    }
    _ => unreachable!("window-independent message routed to window-dependent handler"),
  }

  put_webview(child);
  if let Some((tx, result)) = reparent_reply {
    let _ = tx.send(result);
  }
}

/// Moves a webview into another window, re-applying its bounds against the
/// destination's content area.
fn reparent_webview<T: UserEvent>(
  context: &RuntimeContext<T>,
  child: &mut AppWebview,
  target_window_id: WindowId,
) -> Result<()> {
  if child.window_id == target_window_id {
    return Ok(());
  }
  if context.with_window(target_window_id, |_| ()).is_none() {
    return Err(Error::WindowNotFound);
  }
  let target_window = window_number(target_window_id)?;
  // The page leaves HTML fullscreen with the window it was shown in fullscreen on,
  // and that window goes back to how it was before the page took it fullscreen.
  child.page_fullscreen = PageFullscreen::Off;
  call(
    context,
    rpc::webview::Reparent {
      webview_id: child.webview_id,
      window_id: target_window,
    },
  )?;
  child.window_id = target_window_id;
  // The parent size must be the area the child is laid out into, which is the
  // window's content area - `layout_app_window` applies `bounds_rate` against the
  // same value.
  if let Some((parent_size, scale)) = window_metrics(context, target_window_id) {
    let bounds = child.bounds(context).unwrap_or(Rect {
      position: PhysicalPosition::new(0, 0).into(),
      size: parent_size.into(),
    });
    child.set_bounds(context, parent_size, scale, bounds);
  }
  Ok(())
}

fn handle_window_independent_webview_message<T: UserEvent>(
  context: &RuntimeContext<T>,
  child: &mut AppWebview,
  message: WebviewMessage,
) {
  let webview_id = child.webview_id;
  match message {
    WebviewMessage::EvaluateScript(script) => {
      if let Err(error) = call(
        context,
        rpc::webview::Eval {
          webview_id,
          script,
          document_generation: None,
        },
      ) {
        log::warn!(
          "[electron] evaluating a script in webview {:?} failed: {error}",
          child.label
        );
      }
    }
    WebviewMessage::EvaluateScriptWithCallback(script, callback) => {
      // The reference runs the callback on the UI thread with the JSON string of
      // a `Runtime.evaluate` result and reports a thrown exception as `""`.
      // `webview.eval` returns the value itself, so the same string is produced
      // here, and a failed evaluation reports `""` rather than surfacing an error
      // the callback has no way to carry.
      let result = call(
        context,
        rpc::webview::Eval {
          webview_id,
          script,
          document_generation: None,
        },
      );
      callback(match result {
        Ok(value) => eval_result_to_json(&value),
        Err(error) => {
          log::warn!(
            "[electron] evaluating a script in webview {:?} failed: {error}",
            child.label
          );
          String::new()
        }
      });
    }
    WebviewMessage::Navigate(url) => {
      if let Err(error) = call(
        context,
        rpc::webview::Navigate {
          webview_id,
          url: url.to_string(),
        },
      ) {
        log::warn!(
          "[electron] navigating webview {:?} failed: {error}",
          child.label
        );
      } else if let Ok(mut current) = child.url.lock() {
        // The request was accepted, so this is the URL the document is heading to.
        // A commit that lands elsewhere replaces it again.
        *current = url.to_string();
      }
    }
    WebviewMessage::Reload => log_call(
      child,
      call(context, rpc::webview::Reload { webview_id }),
      "reload",
    ),
    WebviewMessage::GoBack => log_call(
      child,
      call(context, rpc::webview::GoBack { webview_id }),
      "go back",
    ),
    WebviewMessage::CanGoBack(tx) => {
      let _ = tx.send(call(context, rpc::webview::CanGoBack { webview_id }));
    }
    WebviewMessage::GoForward => log_call(
      child,
      call(context, rpc::webview::GoForward { webview_id }),
      "go forward",
    ),
    WebviewMessage::CanGoForward(tx) => {
      let _ = tx.send(call(context, rpc::webview::CanGoForward { webview_id }));
    }
    WebviewMessage::Close => {
      unreachable!("`Close` is handled before the webview is taken out of the registry")
    }
    WebviewMessage::SetFocus => log_call(
      child,
      call(context, rpc::webview::Focus { webview_id }),
      "focus",
    ),
    WebviewMessage::Url(tx) => {
      let _ = tx.send(call(context, rpc::webview::Url { webview_id }));
    }
    WebviewMessage::Bounds(tx) => {
      let _ = tx.send(child.bounds(context));
    }
    WebviewMessage::WithWebview(_) => {
      unreachable!("window-dependent message routed to window-independent handler")
    }
    WebviewMessage::Print => log_call(
      child,
      call(context, rpc::webview::Print { webview_id }),
      "print",
    ),
    WebviewMessage::Show => child.set_visible(context, true),
    WebviewMessage::Hide => child.set_visible(context, false),
    WebviewMessage::SetZoom(scale_factor) => {
      // Electron's zoom factor is linear (1.0 = 100%), so the reference's
      // conversion to CEF's logarithmic level is dropped.
      log_call(
        child,
        call(
          context,
          rpc::webview::SetZoom {
            webview_id,
            scale_factor,
          },
        ),
        "set zoom",
      );
    }
    WebviewMessage::SetBackgroundColor(color) => log_call(
      child,
      call(
        context,
        rpc::webview::SetBackgroundColor {
          webview_id,
          color: color.map(css_color),
        },
      ),
      "set background colour",
    ),
    WebviewMessage::ClearAllBrowsingData => log_call(
      child,
      call(context, rpc::webview::ClearBrowsingData { webview_id }),
      "clear browsing data",
    ),
    WebviewMessage::CookiesForUrl(url, tx) => {
      let _ = tx.send(
        call(
          context,
          rpc::webview::Cookies {
            webview_id,
            url: Some(url.to_string()),
          },
        )
        .map(|cookies| parse_cookies(&cookies)),
      );
    }
    WebviewMessage::Cookies(tx) => {
      let _ = tx.send(
        call(
          context,
          rpc::webview::Cookies {
            webview_id,
            url: None,
          },
        )
        .map(|cookies| parse_cookies(&cookies)),
      );
    }
    WebviewMessage::SetCookie(cookie) => {
      let Some(url) = page_url(context, child) else {
        log::warn!(
          "[electron] not setting a cookie on webview {:?}: it has no page URL to scope it to",
          child.label
        );
        return;
      };
      log_call(
        child,
        call(
          context,
          rpc::webview::SetCookie {
            webview_id,
            cookie: rpc::webview::CookieInit {
              url,
              name: cookie.name().to_owned(),
              value: cookie.value().to_owned(),
              domain: cookie.domain().map(str::to_owned),
              path: cookie.path().map(str::to_owned),
              secure: Some(cookie.secure().unwrap_or(false)),
              http_only: cookie.http_only(),
              same_site: same_site_payload(cookie.same_site()),
              expiration_date: cookie
                .expires_datetime()
                .map(|date| date.unix_timestamp_nanos() as f64 / 1_000_000_000.0),
            },
          },
        ),
        "set cookie",
      );
    }
    WebviewMessage::DeleteCookie(cookie) => {
      let Some(url) = page_url(context, child) else {
        log::warn!(
          "[electron] not deleting a cookie on webview {:?}: it has no page URL to scope it to",
          child.label
        );
        return;
      };
      log_call(
        child,
        call(
          context,
          rpc::webview::DeleteCookie {
            webview_id,
            url,
            name: cookie.name().to_owned(),
          },
        ),
        "delete cookie",
      );
    }
    // Refused here rather than by Chromium: the policy that would have told
    // Chromium to refuse it also switches off the DevTools window this runtime
    // starts every webview with. See `DevToolsPolicy`.
    #[cfg(any(debug_assertions, feature = "devtools"))]
    WebviewMessage::OpenDevTools => {
      if child.devtools_enabled {
        log_call(
          child,
          call(context, rpc::webview::OpenDevtools { webview_id }),
          "open devtools",
        );
      } else {
        log::warn!(
          "not opening devtools for webview {:?}: they are disabled for this webview \
           or by Electron::devtools",
          child.label
        );
      }
    }
    #[cfg(any(debug_assertions, feature = "devtools"))]
    WebviewMessage::CloseDevTools => log_call(
      child,
      call(context, rpc::webview::CloseDevtools { webview_id }),
      "close devtools",
    ),
    #[cfg(any(debug_assertions, feature = "devtools"))]
    WebviewMessage::IsDevToolsOpen(tx) => {
      let _ = tx.send(call(context, rpc::webview::IsDevtoolsOpen { webview_id }).unwrap_or(false));
    }
    WebviewMessage::SendDevToolsMessage(_, tx) => {
      // The bridge exposes no CDP session: `webContents.debugger` needs
      // `webview.debugger.*` commands that do not exist, and Electron's debugger
      // takes a method plus params rather than an id-carrying frame, so a caller's
      // `id` could not reach Chromium even with them.
      crate::unsupported::report("Webview::send_dev_tools_message");
      let _ = tx.send(Err(Error::FailedToSendMessage));
    }
    WebviewMessage::OnDevToolsProtocol(_, tx) => {
      crate::unsupported::report("Webview::on_dev_tools_protocol");
      let _ = tx.send(Err(Error::FailedToSendMessage));
    }
    WebviewMessage::SetBounds(_)
    | WebviewMessage::SetSize(_)
    | WebviewMessage::SetPosition(_)
    | WebviewMessage::Position(_)
    | WebviewMessage::Size(_)
    | WebviewMessage::SetAutoResize(_)
    | WebviewMessage::Reparent(_, _) => {
      unreachable!("window-dependent message routed to window-independent handler")
    }
  }
}

/// Logs a failed fire-and-forget command, naming the webview and the operation.
fn log_call<T>(child: &AppWebview, result: Result<T>, operation: &str) {
  if let Err(error) = result {
    log::warn!(
      "[electron] {operation} on webview {:?} failed: {error}",
      child.label
    );
  }
}

/// The URL of the document currently loaded in a webview.
///
/// The reference reads it from the browser's main frame and scopes a cookie to it.
/// Electron requires a URL for every cookie operation and reports the committed
/// one, so the same page URL is used here; a cookie whose `domain` disagrees with
/// the page is therefore set against the page's URL, exactly as the reference does.
fn page_url<T: UserEvent>(context: &RuntimeContext<T>, child: &AppWebview) -> Option<String> {
  let url = call(
    context,
    rpc::webview::Url {
      webview_id: child.webview_id,
    },
  )
  .ok()?;
  Url::parse(&url).ok()?;
  Some(url)
}

/// Extracts the JSON string a `Runtime.evaluate` result would have carried.
///
/// `webview.eval` returns the value itself, so a `null`/`undefined` result maps to
/// the empty string the reference reports for a missing `value` member, and
/// everything else is serialised the way `returnByValue` would have.
fn eval_result_to_json(value: &Value) -> String {
  if value.is_null() {
    String::new()
  } else {
    value.to_string()
  }
}

#[derive(Clone, Copy, Debug)]
pub enum RuntimeStyle {
  Alloy,
  Chrome,
}

/// The Electron-specific webview attributes, set through
/// [`WebviewWindowBuilderElectronExt`](crate::WebviewWindowBuilderElectronExt).
///
/// # Permission requests
///
/// The runtime honours `WebviewAttributes::on_permission_request` — an `Allow`
/// grants without showing a prompt, a `Deny` refuses without one. Electron asks
/// the runtime through `permission.requested` only for a webview whose
/// `permission_request_handler` is set, which is what the runtime configures when
/// the application registers one.
///
/// ## Most decisions are made once per origin and then persist
///
/// Chromium consults a permission prompt only while the stored content setting
/// for that (origin, permission) still says "ask", and answering the prompt
/// persists the decision to the on-disk profile. Everything routed through the
/// prompt therefore reaches the handler **once per origin and permission, ever**,
/// including across restarts, and nothing calls back to say the app changed its
/// mind: revoking a grant means rewriting the content setting through the session
/// preference.
///
/// Camera and microphone are the exception. Chromium routes *every*
/// `getUserMedia()` call through the media path, so those two do reach the handler
/// on each call and a changing answer is honored.
///
/// ## Permissions Tauri has no kind for arrive as `PermissionKind::Other`
///
/// Chromium has more request types than Tauri has kinds. Storage access, FedCM,
/// protocol handler registration, idle detection, local and loopback network
/// access, web app installation, the WebXR sessions, hand tracking, keyboard lock
/// and disk quota all arrive as `PermissionKind::Other`, as does any request type
/// a future Chromium build adds.
///
/// Failing closed is deliberate, but it means a handler written elsewhere as
/// `match kind { Camera => Allow, _ => Deny }` hard-denies all of them here, and
/// denying storage access or FedCM breaks third-party SSO flows outright. Return
/// `PermissionResponse::Default` for the kinds you did not mean to answer about,
/// and Chromium's own handling runs for them unchanged.
///
/// ## `PermissionKind::DisplayCapture` is never granted by an `Allow`
///
/// A `getDisplayMedia()` request the handler answers `Allow` is handed back to
/// Chromium, which shows the desktop media picker. A `Deny` refuses it outright.
///
/// Granting it from the handler would grant *everything*: a desktop video bit with
/// no requested source synthesises the full desktop and returns it with no picker
/// at all. Since `PermissionKind::DisplayCapture` names no screen, window or tab, a
/// blanket `.on_permission_request(|_| PermissionResponse::Allow)` would silently
/// hand any page in the webview a full-desktop stream.
///
/// # Chrome accelerators an app window does not get
///
/// The reference has to swallow Chrome's accelerator table because a Chrome style
/// browser keeps it live even hosted as a child view with no browser UI. Electron
/// binds no such table: new window and tab, the tab strip, history, downloads and
/// settings, print, save page, view source and the omnibox focus commands simply do
/// not exist in an Electron window, so [`ChromeCommandGroup`] and
/// [`allow_chrome_commands`](crate::WebviewWindowBuilderElectronExt::allow_chrome_commands)
/// exist for API parity and a non-empty list is reported as unsupported.
///
/// Two of the reference's exclusions do not apply here:
///
/// - **Zoom.** `WebviewAttributes::zoom_hotkeys_enabled` is honoured by the main
///   process's input handling, and it **defaults to `false`**, so Ctrl+Plus,
///   Ctrl+Minus and Ctrl+0 do not zoom unless the webview opted in. Ctrl+mouse
///   wheel zoom is unaffected either way. `WebviewDispatch::set_zoom` is
///   untouched.
///
/// - **History.** Alt+Left and Alt+Right are not bound at all, so they do not
///   navigate the session history. Unlike the reference there is no placeholder
///   first document: `webview.create` takes the real URL together with the
///   initialization scripts, so the app's first screen is history entry #1 and
///   going back from it leaves the page. `WebviewDispatch::go_back` and
///   `go_forward` are untouched.
#[derive(Default, Clone)]
pub struct ElectronWebviewAttributes {
  /// Windowless rendering and application-provided callbacks.
  ///
  /// Electron has no windowless browser, so a webview that sets this is reported
  /// as unsupported and created as an ordinary view.
  pub offscreen: Option<crate::Offscreen>,
  /// The browser runtime style, see [`RuntimeStyle`]. Electron has no Alloy/Chrome
  /// distinction; [`RuntimeStyle::Chrome`] is reported as unsupported.
  pub runtime_style: Option<RuntimeStyle>,
  /// Observer of the native lifecycle events of every frame of the webview.
  ///
  /// Scoped to this webview's own native browser — its main frame and its child
  /// frames. Every notification carries that one
  /// [`browser_id`](crate::FrameEvent::browser_id). A popup the runtime opened as
  /// a window of its own is a separate browser that navigates wherever its own
  /// content goes, and a [`FrameEvent`](crate::FrameEvent) carries the full URL, so
  /// popups are never reported here. Observe them through [`Webview::popups`].
  pub frame_event_handler: Option<Arc<crate::FrameEventHandler>>,
  /// Observer of the messages the renderer writes to the JavaScript console.
  ///
  /// Scoped to this webview's own native browser, so neither a popup's output nor
  /// that of a DevTools window opened on this webview is reported here.
  pub console_message_handler: Option<Arc<ConsoleMessageHandler>>,
  /// Families of Chrome commands this webview keeps rather than swallows.
  ///
  /// Empty by default. Electron binds no Chrome accelerator table, so a non-empty
  /// list is reported as unsupported.
  pub allowed_chrome_commands: Vec<ChromeCommandGroup>,
  /// Last look at the [`ElectronBrowserSettings`] before the webview is created.
  ///
  /// Runs after the runtime has mapped the portable [`WebviewAttributes`], so it
  /// can change what the runtime decided as well as set the fields that have no
  /// portable equivalent.
  pub browser_settings_callback: Option<Arc<BrowserSettingsCallback>>,
}

impl std::fmt::Debug for ElectronWebviewAttributes {
  fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    formatter
      .debug_struct("ElectronWebviewAttributes")
      .field("runtime_style", &self.runtime_style)
      .field("offscreen", &self.offscreen.is_some())
      .field("frame_event_handler", &self.frame_event_handler.is_some())
      .field(
        "console_message_handler",
        &self.console_message_handler.is_some(),
      )
      .field("allowed_chrome_commands", &self.allowed_chrome_commands)
      .field(
        "browser_settings_callback",
        &self.browser_settings_callback.is_some(),
      )
      .finish()
  }
}

#[derive(Debug, Clone)]
pub struct ElectronInitScript {
  pub(crate) script: String,
  for_main_frame_only: bool,
}

impl ElectronInitScript {
  /// Whether this script must run in a document loaded in the given frame.
  ///
  /// The reference needs this for the custom-protocol path, which injects the
  /// scripts into the HTML it serves. Electron hands the scripts to the main
  /// process with their frame scope attached, so the bridge applies the same filter
  /// and only the tests read this.
  #[allow(dead_code)]
  pub(crate) fn runs_in_frame(&self, is_main_frame: bool) -> bool {
    is_main_frame || !self.for_main_frame_only
  }

  fn new(script: InitializationScript) -> Self {
    Self {
      script: script.script,
      for_main_frame_only: script.for_main_frame_only,
    }
  }
}

pub(crate) fn initialization_scripts(
  attrs: &mut WebviewAttributes,
) -> Arc<Vec<ElectronInitScript>> {
  let mut initialization_scripts = Vec::new();

  if attrs.drag_drop_handler_enabled {
    initialization_scripts.push(ElectronInitScript::new(
      client::drag_drop_initialization_script(),
    ));
  }

  // The attribute is drained, so a second call sees no application scripts.
  initialization_scripts.extend(
    std::mem::take(&mut attrs.initialization_scripts)
      .into_iter()
      .map(ElectronInitScript::new),
  );

  Arc::new(initialization_scripts)
}

#[derive(Debug, Clone)]
pub struct ElectronWebviewDispatcher<T: UserEvent> {
  pub(crate) window_id: Arc<Mutex<WindowId>>,
  pub(crate) webview_id: u32,
  pub(crate) context: RuntimeContext<T>,
}

impl<T: UserEvent> ElectronWebviewDispatcher<T> {
  /// Sends a UTF-8 encoded Chrome DevTools Protocol message to the DevTools agent.
  ///
  /// The message's `id` must come from
  /// [`allocate_devtools_message_id`](crate::allocate_devtools_message_id), the
  /// allocator every caller on this browser shares. A hardcoded or
  /// self-incremented `id` can consume another caller's
  /// [`DevToolsProtocol::MethodResult`].
  ///
  /// The bridge exposes no CDP session, so this reports unsupported and returns
  /// [`Error::FailedToSendMessage`] rather than pretending the message was sent.
  pub fn send_dev_tools_message(&self, message: &[u8]) -> Result<()> {
    let (tx, rx) = mpsc::channel();
    self.context.send_message(Message::Webview(
      self.webview_id,
      WebviewMessage::SendDevToolsMessage(message.to_vec(), tx),
    ))?;
    rx.recv().map_err(|_| Error::FailedToReceiveMessage)?
  }

  /// Observes the [`DevToolsProtocol`] traffic of this browser.
  ///
  /// The observer receives every message on the browser, including the runtime's
  /// own requests, so results must be matched against an identifier obtained from
  /// [`allocate_devtools_message_id`](crate::allocate_devtools_message_id).
  ///
  /// Scoped to this webview's own native browser. A popup the runtime opened as a
  /// window of its own is a separate browser whose protocol traffic is never
  /// reported here.
  ///
  /// The bridge exposes no CDP session, so this reports unsupported and returns
  /// [`Error::FailedToSendMessage`] rather than registering an observer that can
  /// never fire.
  pub fn on_dev_tools_protocol<F: Fn(DevToolsProtocol) + Send + Sync + 'static>(
    &self,
    f: F,
  ) -> Result<()> {
    let (tx, rx) = mpsc::channel();
    let handler =
      Arc::new(move |protocol: DevToolsProtocol| f(protocol)) as Arc<DevToolsProtocolHandler>;
    self.context.send_message(Message::Webview(
      self.webview_id,
      WebviewMessage::OnDevToolsProtocol(handler, tx),
    ))?;
    rx.recv().map_err(|_| Error::FailedToReceiveMessage)?
  }
}

fn getter<T: UserEvent, R>(
  context: &RuntimeContext<T>,
  message: Message<T>,
  receiver: Receiver<Result<R>>,
) -> Result<R> {
  context.send_message(message)?;
  receiver.recv().map_err(|_| Error::FailedToReceiveMessage)?
}

macro_rules! webview_getter {
  ($self:ident, $variant:ident) => {{
    let (tx, rx) = mpsc::channel();
    // The message carries only the webview id, so no window lock is taken here:
    // the reference has to drop it before blocking because its message names the
    // window, and holding it would deadlock the thread that must service the
    // getter.
    getter(
      &$self.context,
      Message::Webview($self.webview_id, WebviewMessage::$variant(tx)),
      rx,
    )
  }};
}

impl<T: UserEvent> WebviewDispatch<T> for ElectronWebviewDispatcher<T> {
  type Runtime = ElectronRuntime<T>;

  fn run_on_main_thread<F: FnOnce() + Send + 'static>(&self, f: F) -> Result<()> {
    self.context.on_main_thread(move |_| f())
  }

  fn with_webview<F: FnOnce(<Self::Runtime as Runtime<T>>::Webview) + Send + 'static>(
    &self,
    f: F,
  ) -> Result<()> {
    self.context.send_message(Message::Webview(
      self.webview_id,
      WebviewMessage::WithWebview(Box::new(f)),
    ))
  }

  fn open_devtools(&self) {
    #[cfg(any(debug_assertions, feature = "devtools"))]
    {
      let _ = self.context.send_message(Message::Webview(
        self.webview_id,
        WebviewMessage::OpenDevTools,
      ));
    }
    #[cfg(not(any(debug_assertions, feature = "devtools")))]
    log::warn!(
      "devtools are not available: enable the `devtools` feature of `tauri-runtime-electron`"
    );
  }

  fn close_devtools(&self) {
    #[cfg(any(debug_assertions, feature = "devtools"))]
    {
      let _ = self.context.send_message(Message::Webview(
        self.webview_id,
        WebviewMessage::CloseDevTools,
      ));
    }
  }

  fn is_devtools_open(&self) -> Result<bool> {
    #[cfg(any(debug_assertions, feature = "devtools"))]
    {
      let (tx, rx) = mpsc::channel();
      self.context.send_message(Message::Webview(
        self.webview_id,
        WebviewMessage::IsDevToolsOpen(tx),
      ))?;
      rx.recv().map_err(|_| Error::FailedToReceiveMessage)
    }
    #[cfg(not(any(debug_assertions, feature = "devtools")))]
    {
      Ok(false)
    }
  }

  fn url(&self) -> Result<String> {
    webview_getter!(self, Url)
  }

  fn bounds(&self) -> Result<Rect> {
    webview_getter!(self, Bounds)
  }

  fn position(&self) -> Result<PhysicalPosition<i32>> {
    webview_getter!(self, Position)
  }

  fn size(&self) -> Result<PhysicalSize<u32>> {
    webview_getter!(self, Size)
  }

  fn navigate(&self, url: Url) -> Result<()> {
    self.context.send_message(Message::Webview(
      self.webview_id,
      WebviewMessage::Navigate(url),
    ))
  }

  fn reload(&self) -> Result<()> {
    self
      .context
      .send_message(Message::Webview(self.webview_id, WebviewMessage::Reload))
  }

  fn go_back(&self) -> Result<()> {
    self
      .context
      .send_message(Message::Webview(self.webview_id, WebviewMessage::GoBack))
  }

  fn can_go_back(&self) -> Result<bool> {
    webview_getter!(self, CanGoBack)
  }

  fn go_forward(&self) -> Result<()> {
    self
      .context
      .send_message(Message::Webview(self.webview_id, WebviewMessage::GoForward))
  }

  fn can_go_forward(&self) -> Result<bool> {
    webview_getter!(self, CanGoForward)
  }

  fn print(&self) -> Result<()> {
    self
      .context
      .send_message(Message::Webview(self.webview_id, WebviewMessage::Print))
  }

  fn close(&self) -> Result<()> {
    self
      .context
      .send_message(Message::Webview(self.webview_id, WebviewMessage::Close))
  }

  fn set_bounds(&self, bounds: Rect) -> Result<()> {
    self.context.send_message(Message::Webview(
      self.webview_id,
      WebviewMessage::SetBounds(bounds),
    ))
  }

  fn set_size(&self, size: Size) -> Result<()> {
    self.context.send_message(Message::Webview(
      self.webview_id,
      WebviewMessage::SetSize(size),
    ))
  }

  fn set_position(&self, position: Position) -> Result<()> {
    self.context.send_message(Message::Webview(
      self.webview_id,
      WebviewMessage::SetPosition(position),
    ))
  }

  fn set_focus(&self) -> Result<()> {
    self
      .context
      .send_message(Message::Webview(self.webview_id, WebviewMessage::SetFocus))
  }

  fn hide(&self) -> Result<()> {
    self
      .context
      .send_message(Message::Webview(self.webview_id, WebviewMessage::Hide))
  }

  fn show(&self) -> Result<()> {
    self
      .context
      .send_message(Message::Webview(self.webview_id, WebviewMessage::Show))
  }

  fn eval_script<S: Into<String>>(&self, script: S) -> Result<()> {
    self.context.send_message(Message::Webview(
      self.webview_id,
      WebviewMessage::EvaluateScript(script.into()),
    ))
  }

  fn eval_script_with_callback<S: Into<String>>(
    &self,
    script: S,
    callback: impl Fn(String) + Send + 'static,
  ) -> Result<()> {
    self.context.send_message(Message::Webview(
      self.webview_id,
      WebviewMessage::EvaluateScriptWithCallback(script.into(), Box::new(callback)),
    ))
  }

  fn reparent(&self, window_id: WindowId) -> Result<()> {
    let (tx, rx) = mpsc::channel();
    self.context.send_message(Message::Webview(
      self.webview_id,
      WebviewMessage::Reparent(window_id, tx),
    ))?;
    let result = rx.recv().map_err(|_| Error::FailedToReceiveMessage)?;
    if result.is_ok() {
      *self
        .window_id
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = window_id;
    }
    result
  }

  fn cookies_for_url(&self, url: Url) -> Result<Vec<Cookie<'static>>> {
    let (tx, rx) = mpsc::channel();
    self.context.send_message(Message::Webview(
      self.webview_id,
      WebviewMessage::CookiesForUrl(url, tx),
    ))?;
    rx.recv().map_err(|_| Error::FailedToReceiveMessage)?
  }

  fn cookies(&self) -> Result<Vec<Cookie<'static>>> {
    webview_getter!(self, Cookies)
  }

  fn set_cookie(&self, cookie: Cookie<'_>) -> Result<()> {
    self.context.send_message(Message::Webview(
      self.webview_id,
      WebviewMessage::SetCookie(cookie.into_owned()),
    ))
  }

  fn delete_cookie(&self, cookie: Cookie<'_>) -> Result<()> {
    self.context.send_message(Message::Webview(
      self.webview_id,
      WebviewMessage::DeleteCookie(cookie.into_owned()),
    ))
  }

  fn set_auto_resize(&self, auto_resize: bool) -> Result<()> {
    self.context.send_message(Message::Webview(
      self.webview_id,
      WebviewMessage::SetAutoResize(auto_resize),
    ))
  }

  fn set_zoom(&self, scale_factor: f64) -> Result<()> {
    self.context.send_message(Message::Webview(
      self.webview_id,
      WebviewMessage::SetZoom(scale_factor),
    ))
  }

  fn set_background_color(&self, color: Option<Color>) -> Result<()> {
    self.context.send_message(Message::Webview(
      self.webview_id,
      WebviewMessage::SetBackgroundColor(color),
    ))
  }

  fn clear_all_browsing_data(&self) -> Result<()> {
    self.context.send_message(Message::Webview(
      self.webview_id,
      WebviewMessage::ClearAllBrowsingData,
    ))
  }
}

/// Reposition every child webview to follow the parent window size.
///
/// Children with a bounds rate (auto-resize / window-filling) are recomputed
/// from the current window size; children with fixed bounds keep whatever bounds
/// they were last given.
pub(crate) fn layout_app_window<T: UserEvent>(context: &RuntimeContext<T>, window_id: WindowId) {
  let Some((parent_size, _)) = window_metrics(context, window_id) else {
    return;
  };
  let win_w = parent_size.width as f32;
  let win_h = parent_size.height as f32;
  for webview_id in webview_ids_for_window(window_id) {
    let Some(child) = take_webview(webview_id) else {
      continue;
    };
    if let Some(rate) = child.bounds_rate {
      let x = (rate.x * win_w).round() as i32;
      let y = (rate.y * win_h).round() as i32;
      let w = (rate.width * win_w).round() as i32;
      let h = (rate.height * win_h).round() as i32;
      child.resize(context, x, y, w, h);
    }
    put_webview(child);
  }
}

/// Compute the bounds rate of a child webview relative to its parent window.
///
/// For webviews filling the window, the default rate is used, otherwise the rate is
/// computed from the current bounds and parent size if auto_resize is enabled,
/// otherwise None is returned.
pub(crate) fn compute_child_bounds_rate(
  bounds: Option<&Rect>,
  auto_resize: bool,
  parent_size: PhysicalSize<u32>,
  scale: f64,
) -> Option<BoundsRate> {
  let Some(bounds) = bounds else {
    return Some(BoundsRate::default());
  };

  if !auto_resize {
    return None;
  }

  let min_w = parent_size.width.max(1) as i32;
  let min_h = parent_size.height.max(1) as i32;

  let pos = bounds.position.to_physical::<i32>(scale);
  let size = bounds.size.to_physical::<u32>(scale);

  let x = pos.x;
  let y = pos.y;
  let w = size.width;
  let h = size.height;

  Some(BoundsRate {
    x: x as f32 / min_w as f32,
    y: y as f32 / min_h as f32,
    width: w as f32 / min_w as f32,
    height: h as f32 / min_h as f32,
  })
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn background_is_transparent_only_on_request() {
    let mut attributes = WebviewAttributes::new(tauri_utils::config::WebviewUrl::default());
    let settings = browser_settings_from_webview_attributes(&attributes);
    // Nothing is sent: Chromium paints its own opaque white.
    assert_eq!(settings.background_color, None);
    assert!(!settings.transparent);

    attributes.transparent = true;
    let settings = browser_settings_from_webview_attributes(&attributes);
    assert_eq!(settings.background_color, None);
    assert!(settings.transparent);

    attributes.background_color = Some(Color(1, 2, 3, 255));
    let settings = browser_settings_from_webview_attributes(&attributes);
    assert_eq!(settings.background_color, Some(Color(1, 2, 3, 255)));
    assert_eq!(
      rpc_color(settings.background_color.unwrap()),
      rpc::Color::Rgba([1, 2, 3, 255])
    );
  }

  #[test]
  fn console_levels_map_onto_the_portable_enum() {
    for (level, expected) in [
      ("debug", ConsoleMessageLevel::Verbose),
      ("info", ConsoleMessageLevel::Info),
      ("warning", ConsoleMessageLevel::Warning),
      ("error", ConsoleMessageLevel::Error),
      ("fatal", ConsoleMessageLevel::Other),
    ] {
      let message = ConsoleMessage::from_electron(level, "m".into(), "s".into(), 7);
      assert_eq!(message.level, expected);
      assert_eq!(message.message, "m");
      assert_eq!(message.source, "s");
      assert_eq!(message.line, 7);
    }
  }

  #[test]
  fn every_chrome_command_group_is_blocked_by_default() {
    // `ALL` is the blocklist's source of truth, so a variant added to the enum
    // must be listed here to be blocked.
    assert_eq!(
      ChromeCommandGroup::ALL,
      &[
        ChromeCommandGroup::WindowAndTab,
        ChromeCommandGroup::Document,
        ChromeCommandGroup::BrowserChrome,
        ChromeCommandGroup::BrowserSurface,
        ChromeCommandGroup::History,
      ]
    );
    assert!(
      ElectronWebviewAttributes::default()
        .allowed_chrome_commands
        .is_empty()
    );
  }

  #[test]
  fn an_unset_bounds_rate_fills_the_window_and_auto_resize_scales_it() {
    let parent = PhysicalSize::new(800, 600);
    assert_eq!(
      compute_child_bounds_rate(None, false, parent, 1.0).map(|rate| (
        rate.x,
        rate.y,
        rate.width,
        rate.height
      )),
      Some((0., 0., 1., 1.))
    );
    let bounds = Rect {
      position: PhysicalPosition::new(80, 60).into(),
      size: PhysicalSize::new(400, 300).into(),
    };
    assert!(compute_child_bounds_rate(Some(&bounds), false, parent, 1.0).is_none());
    let rate = compute_child_bounds_rate(Some(&bounds), true, parent, 1.0).unwrap();
    assert_eq!(
      (rate.x, rate.y, rate.width, rate.height),
      (0.1, 0.1, 0.5, 0.5)
    );
  }

  #[test]
  fn bounds_keep_the_units_they_were_given_in() {
    let logical = bounds_input(Rect {
      position: tauri_runtime::dpi::LogicalPosition::new(10., 20.).into(),
      size: tauri_runtime::dpi::LogicalSize::new(300., 200.).into(),
    });
    assert_eq!(
      serde_json::to_value(logical).unwrap(),
      serde_json::json!({
        "position": { "x": 10.0, "y": 20.0, "physical": false },
        "size": { "width": 300.0, "height": 200.0, "physical": false },
      })
    );
    let physical = bounds_input(Rect {
      position: PhysicalPosition::new(10, 20).into(),
      size: PhysicalSize::new(300, 200).into(),
    });
    assert_eq!(
      serde_json::to_value(physical).unwrap(),
      serde_json::json!({
        "position": { "x": 10.0, "y": 20.0, "physical": true },
        "size": { "width": 300.0, "height": 200.0, "physical": true },
      })
    );
  }

  #[test]
  fn an_evaluation_result_matches_the_devtools_return_by_value_string() {
    assert_eq!(eval_result_to_json(&Value::Null), "");
    assert_eq!(eval_result_to_json(&Value::from("text")), "\"text\"");
    assert_eq!(eval_result_to_json(&Value::from(42)), "42");
    assert_eq!(
      eval_result_to_json(&serde_json::json!({ "a": [1, true] })),
      "{\"a\":[1,true]}"
    );
  }

  #[test]
  fn initialization_scripts_put_the_drag_script_first_and_drain_the_attribute() {
    let mut attributes = WebviewAttributes::new(tauri_utils::config::WebviewUrl::default());
    attributes.drag_drop_handler_enabled = true;
    attributes.initialization_scripts = vec![
      InitializationScript {
        script: "window.a = 1;".into(),
        for_main_frame_only: true,
      },
      InitializationScript {
        script: "window.b = 2;".into(),
        for_main_frame_only: false,
      },
    ];
    let scripts = initialization_scripts(&mut attributes);
    assert_eq!(scripts.len(), 3);
    // The drag script runs in every frame, as in the reference.
    assert!(scripts[0].runs_in_frame(false));
    assert!(scripts[1].runs_in_frame(true));
    assert!(!scripts[1].runs_in_frame(false));
    assert!(scripts[2].runs_in_frame(false));
    // The attribute is drained, so a second call sees only the drag script the
    // handler still asks for.
    let remaining = initialization_scripts(&mut attributes);
    assert_eq!(remaining.len(), 1);
    assert!(remaining[0].runs_in_frame(false));
  }
}
