// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

use crate::{
  environment, environment::DebugEnvironment, locale, platform, switches,
  webview::ElectronWebviewDispatcher, window::ElectronWindowDispatcher,
  window_handle::ElectronRuntimeHandle,
};
use crate::{rpc, rpc_runtime::Ipc as ElectronProcess};
use serde_json::{json, Value};
use tauri_runtime::{
  dpi::{PhysicalPosition, PhysicalSize, PixelUnit, Position, Size},
  monitor::Monitor,
  webview::{
    DetachedWebview, DownloadEvent, PageLoadEvent, PendingWebview, PermissionKind,
    PermissionResponse, WebviewIpcHandler,
  },
  window::WindowSizeConstraints,
  window::{
    DetachedWindow, DetachedWindowWebview, PendingWindow, RawWindow, WindowEvent, WindowId,
  },
  DeviceEventFilter, Error, EventLoopProxy, ExitRequestedEventAction, Result, RunEvent, Runtime,
  RuntimeInitArgs, UserEvent, WindowEventId,
};
use tauri_utils::Theme;

/// The `tauri-runtime` handler aliases are private in the 2.x line, so these
/// restate the same signatures locally.
type DownloadHandler = dyn Fn(DownloadEvent<'_>) -> bool + Send + Sync;
type IpcHandler = dyn Fn(Request<String>) + Send;
type NavigationHandler = dyn Fn(&url::Url) -> bool + Send;
type WebviewEventHandler = dyn Fn(&tauri_runtime::window::WebviewEvent) + Send;
type WindowEventHandler = dyn Fn(&WindowEvent) + Send;
type UriSchemeProtocolResponder = dyn FnOnce(Response<std::borrow::Cow<'static, [u8]>>) + Send;
type UriSchemeProtocolHandler =
  dyn Fn(&str, http::Request<Vec<u8>>, Box<UriSchemeProtocolResponder>) + Send + Sync;

use http::{Request, Response};
use std::{
  any::Any,
  cell::RefCell,
  collections::HashMap,
  fmt,
  path::{Path, PathBuf},
  rc::Rc,
  sync::{
    atomic::{AtomicBool, AtomicU32, Ordering},
    mpsc::{channel, sync_channel, Receiver, SyncSender},
    Arc, Mutex,
  },
  time::Duration,
};

/// Schemes built into every runtime; application schemes must be known before launch.
pub(crate) const DEFAULT_SCHEMES: &[&str] = &["ipc", "tauri", "asset"];

thread_local! {
  static STARTUP_SCHEMES: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// Register a secure custom protocol for the next runtime created on this thread.
///
/// Call before `Builder::build`/`run`, for example with the schema from the original
/// `Context::pattern()`. Registrations are deduplicated and consumed once at startup;
/// protocol handlers are still supplied by Tauri when it creates each webview.
pub fn register_custom_protocol_scheme(scheme: &str) -> std::result::Result<(), &'static str> {
  let mut bytes = scheme.bytes();
  if !bytes.next().is_some_and(|byte| byte.is_ascii_alphabetic())
    || !bytes.all(|byte| byte.is_ascii_alphanumeric() || b"+-.".contains(&byte))
  {
    return Err("invalid custom protocol scheme");
  }
  let scheme = scheme.to_ascii_lowercase();
  STARTUP_SCHEMES.with(|schemes| {
    let mut schemes = schemes.borrow_mut();
    if !schemes.contains(&scheme) {
      schemes.push(scheme);
    }
  });
  Ok(())
}

fn take_startup_schemes() -> Vec<String> {
  STARTUP_SCHEMES.with(|schemes| std::mem::take(&mut *schemes.borrow_mut()))
}

/// Timeout for a single IPC round trip with the Electron process.
pub(crate) const IPC_TIMEOUT: Duration = Duration::from_secs(15);

/// Default time the Electron process gets to start and connect back.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(60);

fn io_err(message: String) -> Box<dyn std::error::Error + Send + Sync> {
  Box::new(std::io::Error::other(message))
}

pub(crate) fn create_window_err(message: String) -> Error {
  // Tauri 2.s `CreateWindow` carries no payload, so keep the reason on stderr.
  eprintln!("[electron] {message}");
  Error::CreateWindow
}

pub(crate) fn create_webview_err(message: String) -> Error {
  Error::CreateWebview(io_err(message))
}

/// Reports a trait member this backend cannot honour, once per method.
///
/// A stub that quietly returns `Ok(())` is the failure mode this crate is most
/// likely to ship: the app looks fine and nothing happens. Callers that loop
/// (`set_cursor_icon` on every hover) must not fill the console, hence the
/// once-per-method gate.
pub(crate) fn unsupported(method: &'static str, reason: &str) {
  static REPORTED: std::sync::LazyLock<std::sync::Mutex<std::collections::HashSet<&'static str>>> =
    std::sync::LazyLock::new(Default::default);
  if REPORTED.lock().is_ok_and(|mut seen| seen.insert(method)) {
    eprintln!("[electron] {method} is not implemented: {reason}");
  }
}

/// Tauri sizes are physical or logical; the Electron side converts to DIPs.
pub(crate) fn size_payload(size: Size) -> rpc::webview::Extent {
  let size = rpc::window_size(size);
  rpc::webview::Extent {
    width: size.width, height: size.height, physical: Some(size.physical),
  }
}

/// Keep each constraint's unit until the target display is known in Electron.
pub(crate) fn size_constraints_payload(constraints: WindowSizeConstraints) -> rpc::window::SizeConstraints {
  use rpc::window::{ConstraintAxis, SizeConstraint};
  let axis = |unit: Option<PixelUnit>| {
    unit.map(|unit| match unit {
      PixelUnit::Physical(unit) => SizeConstraint::Scaled { value: f64::from(unit.0), physical: true },
      PixelUnit::Logical(unit) => SizeConstraint::Scaled { value: unit.0, physical: false },
    })
  };
  std::collections::BTreeMap::from([
    (ConstraintAxis::MinWidth, axis(constraints.min_width)),
    (ConstraintAxis::MinHeight, axis(constraints.min_height)),
    (ConstraintAxis::MaxWidth, axis(constraints.max_width)),
    (ConstraintAxis::MaxHeight, axis(constraints.max_height)),
  ])
}

pub(crate) fn position_payload(position: Position) -> rpc::webview::Position {
  let position = rpc::window_position(position);
  rpc::webview::Position { x: position.x, y: position.y, physical: Some(position.physical) }
}

pub(crate) fn rect_position(rect: &Value) -> PhysicalPosition<i32> {
  PhysicalPosition {
    x: rect["x"].as_i64().unwrap_or_default() as i32,
    y: rect["y"].as_i64().unwrap_or_default() as i32,
  }
}

pub(crate) fn rect_size(rect: &Value) -> PhysicalSize<u32> {
  PhysicalSize {
    width: rect["width"].as_u64().unwrap_or_default() as u32,
    height: rect["height"].as_u64().unwrap_or_default() as u32,
  }
}

type ShortcutMap = HashMap<String, Box<dyn Fn() + Send + 'static>>;
type ProtocolHandlerMap = HashMap<(String, Option<u32>), (String, Rc<UriSchemeProtocolHandler>)>;

pub(crate) enum Message {
  Task(Box<dyn FnOnce() + Send>),
  CloseWindow(WindowId),
  DestroyWindow(WindowId),
  UserEvent(Box<dyn Any + Send>),
  RequestExit(i32),
}

struct Webview {
  /// Numeric id handed to the Electron process.
  serial: u32,
  /// Label Tauri gave the webview (`navigate` keeps its origin in sync).
  label: String,
  /// Handlers registered by Tauri to observe this webview's events.
  handlers: Vec<(tauri_runtime::WebviewEventId, Rc<WebviewEventHandler>)>,
}

struct Window {
  label: String,
  new_window_request: Option<u64>,
  /// Numeric id handed to the Electron process (Tauri's `WindowId` is opaque).
  serial: u32,
  webviews: Vec<Webview>,
  /// Handlers registered by Tauri to observe this window's events.
  handlers: Vec<(WindowEventId, Rc<WindowEventHandler>)>,
  #[cfg(windows)]
  menu_owner: Option<crate::window::imp::MenuOwner>,
}

#[derive(Clone)]
pub struct RuntimeContext {
  is_running: Arc<AtomicBool>,
  windows: Arc<RefCell<HashMap<WindowId, Window>>>,
  shortcuts: Arc<Mutex<ShortcutMap>>,
  pub(crate) run_tx: SyncSender<Message>,
  main_thread_id: std::thread::ThreadId,
  next_window_id: Arc<AtomicU32>,
  next_webview_id: Arc<AtomicU32>,
  next_serial: Arc<AtomicU32>,
  next_window_event_id: Arc<AtomicU32>,
  next_webview_event_id: Arc<AtomicU32>,
  /// Connection to the Electron main process.
  pub(crate) ipc: Arc<ElectronProcess>,
  /// When set, `tauri://localhost` URLs are rewritten to this origin (dev server).
  dev_url: Option<String>,
  /// Window icon forwarded to Electron on `window.create`.
  icon: Option<PathBuf>,
  /// Tauri IPC handlers, invoked when the page posts a message through
  /// `window.ipc.postMessage` (re-emitted by Electron as `ipc.message`).
  ///
  /// The handler is closed over the [`DetachedWebview`] it belongs to, so the
  /// map does not need to know the runtime's generic parameters.
  ipc_handlers: Arc<RefCell<HashMap<u32, Rc<IpcHandler>>>>,
  /// Tauri's page-load, title and new-window callbacks, by webview serial.
  webview_handlers: Arc<RefCell<HashMap<u32, WebviewHandlers>>>,
  /// Custom protocols registered by Tauri, served through Electron's
  /// `protocol.handle`.
  protocol_handlers: Arc<RefCell<ProtocolHandlerMap>>,
  /// Page origin per webview label, injected as the `Origin` header on
  /// custom-protocol requests (Electron reports the request URL instead).
  webview_origins: Arc<RefCell<HashMap<String, String>>>,
  /// Download and permission callbacks, by webview serial. Electron asks for
  /// these asynchronously, so the run loop answers over IPC.
  policy_handlers: Arc<RefCell<HashMap<u32, PolicyHandlers>>>,
  /// Native window handle per window serial, asked from Electron once.
  /// HWND/X11 IDs permit selected cross-process operations; AppKit pointers
  /// cannot be borrowed from Electron's process.
  native_handles: Arc<RefCell<HashMap<u32, isize>>>,
  pub(crate) new_window_handlers:
    Arc<RefCell<HashMap<u32, Rc<crate::new_window::NewWindowHandler>>>>,
}

// SAFETY: we ensure this type is only used on the main thread.
#[allow(clippy::non_send_fields_in_send_ty)]
unsafe impl Send for RuntimeContext {}

// SAFETY: we ensure this type is only used on the main thread.
#[allow(clippy::non_send_fields_in_send_ty)]
unsafe impl Sync for RuntimeContext {}

impl RuntimeContext {
  /// RefCell state and native menus belong to the runtime's UI thread.
  pub(crate) fn on_main_thread<R: Send + 'static>(
    &self,
    task: impl FnOnce(&Self) -> R + Send + 'static,
  ) -> Result<R> {
    if std::thread::current().id() == self.main_thread_id {
      return Ok(task(self));
    }
    let context = self.clone();
    let (tx, rx) = sync_channel(1);
    self
      .run_tx
      .send(Message::Task(Box::new(move || {
        let _ = tx.send(task(&context));
      })))
      .map_err(|_| Error::FailedToSendMessage)?;
    rx.recv().map_err(|_| Error::FailedToReceiveMessage)
  }

  pub(crate) fn send_message(&self, message: Message) -> Result<()> {
    // Tasks run inline on the main thread; everything else goes to the loop.
    let is_main_thread = std::thread::current().id() == self.main_thread_id;
    let message = match (is_main_thread, message) {
      (true, Message::Task(task)) => {
        task();
        return Ok(());
      }
      (_, message) => message,
    };
    // Like Wry's proxy, explicit exit requests must reach the callback even
    // when requested during setup, before the event loop starts.
    if !is_main_thread
      || matches!(&message, Message::RequestExit(_))
      || self.is_running.load(Ordering::Relaxed)
    {
      self
        .run_tx
        .send(message)
        .map_err(|_| Error::FailedToSendMessage)
    } else {
      match message {
        Message::Task(task) => task(),
        Message::CloseWindow(id) | Message::DestroyWindow(id) => {
          self.forget_window(id);
        }
        // No run loop to deliver it to yet.
        Message::UserEvent(_) | Message::RequestExit(_) => {}
      }
      Ok(())
    }
  }

  fn forget_window(&self, id: WindowId) -> bool {
    let Some(window) = self.windows.borrow_mut().remove(&id) else {
      return false;
    };
    for webview in &window.webviews {
      self.forget_webview_handlers(webview.serial, &webview.label);
    }
    self.native_handles.borrow_mut().remove(&window.serial);
    // MenuOwner deliberately retains its HWND: muda can still reference it.
    true
  }

  pub(crate) fn forget_webview(&self, serial: u32) -> Result<()> {
    self.on_main_thread(move |context| {
      let removed = context
        .windows
        .borrow_mut()
        .values_mut()
        .find_map(|window| {
          let index = window
            .webviews
            .iter()
            .position(|webview| webview.serial == serial)?;
          Some(window.webviews.remove(index))
        });
      if let Some(webview) = removed {
        context.forget_webview_handlers(webview.serial, &webview.label);
      }
    })
  }

  pub(crate) fn reparent_webview(&self, serial: u32, destination: WindowId) -> Result<()> {
    self.on_main_thread(move |context| {
      let (source, target) = {
        let windows = context.windows.borrow();
        let source = windows
          .iter()
          .find_map(|(id, window)| {
            window
              .webviews
              .iter()
              .any(|webview| webview.serial == serial)
              .then_some(*id)
          })
          .ok_or_else(|| create_webview_err("webview is no longer tracked".into()))?;
        let target = windows
          .get(&destination)
          .ok_or_else(|| create_webview_err("destination window does not exist".into()))?
          .serial;
        (source, target)
      };
      if source == destination {
        return Ok(());
      }
      context
        .ipc
        .call(rpc::WebviewReparent {
          webview_id: serial,
          window_id: target,
        })
        .map_err(|error| create_webview_err(error.to_string()))?;
      // Move ownership only after the native move succeeds. Handlers and protocols
      // remain keyed by the unchanged webview ID, including after source close.
      let mut windows = context.windows.borrow_mut();
      let source = windows
        .get_mut(&source)
        .ok_or_else(|| create_webview_err("source window is gone".into()))?;
      let index = source
        .webviews
        .iter()
        .position(|webview| webview.serial == serial)
        .ok_or_else(|| create_webview_err("webview is no longer tracked".into()))?;
      let webview = source.webviews.remove(index);
      windows
        .get_mut(&destination)
        .ok_or_else(|| create_webview_err("destination window is gone".into()))?
        .webviews
        .push(webview);
      Ok(())
    })?
  }

  fn forget_webview_handlers(&self, serial: u32, label: &str) {
    self.ipc_handlers.borrow_mut().remove(&serial);
    self.webview_handlers.borrow_mut().remove(&serial);
    self.new_window_handlers.borrow_mut().remove(&serial);
    self.policy_handlers.borrow_mut().remove(&serial);
    self.webview_origins.borrow_mut().remove(label);
    remove_webview_protocols(&mut self.protocol_handlers.borrow_mut(), serial, label);
  }

  fn next_window_id(&self) -> WindowId {
    self.next_window_id.fetch_add(1, Ordering::Relaxed).into()
  }

  fn next_webview_id(&self) -> u32 {
    self.next_webview_id.fetch_add(1, Ordering::Relaxed)
  }

  fn next_serial(&self) -> u32 {
    self.next_serial.fetch_add(1, Ordering::Relaxed)
  }

  fn window_for_serial(&self, serial: u32) -> Option<WindowId> {
    self
      .windows
      .borrow()
      .iter()
      .find(|(_, window)| window.serial == serial)
      .map(|(id, _)| *id)
  }

  pub(crate) fn serial_for_window(&self, id: WindowId) -> Option<u32> {
    self
      .on_main_thread(move |context| {
        context
          .windows
          .borrow()
          .get(&id)
          .map(|window| window.serial)
      })
      .ok()
      .flatten()
  }

  pub(crate) fn window_for_webview(&self, serial: u32) -> Result<ElectronWindowDispatcher> {
    self.on_main_thread(move |context| {
      context
        .windows
        .borrow()
        .iter()
        .find_map(|(id, window)| {
          window
            .webviews
            .iter()
            .any(|webview| webview.serial == serial)
            .then(|| ElectronWindowDispatcher {
              id: *id,
              context: context.clone(),
              effects_target: crate::window::EffectsTarget::new(window.serial, &context.ipc),
            })
        })
        .ok_or_else(|| create_webview_err("webview is no longer tracked".into()))
    })?
  }

  /// The native handle of a window, as Electron knows it. Cached because Tauri
  /// asks for it on every menu/effect/decoration call.
  pub(crate) fn native_handle(&self, id: WindowId) -> Result<isize> {
    if std::thread::current().id() != self.main_thread_id {
      return self.on_main_thread(move |context| context.native_handle(id))?;
    }
    let serial = self
      .serial_for_window(id)
      .ok_or_else(|| create_window_err("window is gone".to_string()))?;
    if let Some(handle) = self.native_handles.borrow().get(&serial) {
      return Ok(*handle);
    }
    let value = self
      .ipc
      .call(rpc::NativeHandle { window_id: serial })
      .map_err(|error| create_window_err(error.to_string()))?;
    let handle = isize::try_from(value.handle)
      .map_err(|_| create_window_err("Electron native handle exceeds pointer size".into()))?;
    self.native_handles.borrow_mut().insert(serial, handle);
    Ok(handle)
  }

  #[cfg(target_os = "linux")]
  pub(crate) fn x11_window(&self, id: WindowId) -> Result<(u32, u32)> {
    self.on_main_thread(move |context| {
      let serial = context
        .serial_for_window(id)
        .ok_or_else(|| create_window_err("window is gone".into()))?;
      let value = context
        .ipc
        .call(rpc::NativeHandle { window_id: serial })
        .map_err(|error| create_window_err(error.to_string()))?;
      let handle = u32::try_from(value.handle)
        .ok()
        .filter(|value| *value != 0)
        .ok_or_else(|| create_window_err("Electron returned no X11 handle".into()))?;
      let process = value.process_id;
      if process == 0 {
        return Err(create_window_err("Electron returned no X11 processId".into()));
      }
      Ok((handle, process))
    })?
  }

  #[cfg(windows)]
  pub(crate) fn menu_owner_window(&self, id: WindowId) -> Result<isize> {
    if std::thread::current().id() != self.main_thread_id {
      return self.on_main_thread(move |context| context.menu_owner_window(id))?;
    }
    let mut windows = self.windows.borrow_mut();
    let window = windows
      .get_mut(&id)
      .ok_or_else(|| create_window_err("window is gone".into()))?;
    if window.menu_owner.is_none() {
      window.menu_owner = crate::window::imp::MenuOwner::new();
    }
    window
      .menu_owner
      .as_ref()
      .map(|owner| owner.handle)
      .ok_or_else(|| create_window_err("could not create menu owner".into()))
  }

  #[cfg(windows)]
  fn sync_window_menus(&self) {
    // ponytail: O(total menu items) per pump; use native mutation hooks if
    // polling becomes measurable. Only changed menus cross IPC.
    let changed: Vec<_> = self
      .windows
      .borrow()
      .iter()
      .filter_map(|(id, window)| {
        let owner = window.menu_owner.as_ref()?;
        match crate::window::imp::menu_snapshot(owner.handle) {
          Ok(menu) if menu != owner.mirrored => Some((*id, window.serial, menu)),
          Ok(_) => None,
          Err(error) => {
            eprintln!("[electron] menu snapshot failed: {error}");
            None
          }
        }
      })
      .collect();
    for (id, serial, menu) in changed {
      let items = match serde_json::from_value(menu.clone()) {
        Ok(items) => items,
        Err(error) => {
          eprintln!("[electron] menu conversion failed: {error}");
          continue;
        }
      };
      match self.ipc.call(rpc::WindowSetMenu { window_id: serial, menu: items }) {
        Ok(_) => {
          if let Some(owner) = self
            .windows
            .borrow_mut()
            .get_mut(&id)
            .and_then(|window| window.menu_owner.as_mut())
          {
            owner.mirrored = menu;
          }
        }
        Err(error) => eprintln!("[electron] menu mirror failed: {error}"),
      }
    }
  }

  /// Tauri resolves [`WebviewUrl::App`] against the dev server (`devUrl`) or the
  /// `tauri://localhost` protocol and hands the result over as
  /// [`PendingWebview::url`](tauri_runtime::webview::PendingWebview::url), so the
  /// runtime only has to honour an explicit [`Electron::dev_url`] override.
  fn resolve_url(&self, raw: &str) -> String {
    match (&self.dev_url, raw.strip_prefix("tauri://localhost")) {
      (Some(dev), Some(rest)) => format!("{dev}{rest}"),
      _ => raw.to_string(),
    }
  }

  /// Updates the remembered origin after a navigation.
  pub(crate) fn set_webview_origin_by_serial(&self, serial: u32, url: &str) {
    if std::thread::current().id() != self.main_thread_id {
      let url = url.to_owned();
      if let Err(error) =
        self.on_main_thread(move |context| context.set_webview_origin_by_serial(serial, &url))
      {
        eprintln!("[electron] failed to update webview origin: {error}");
      }
      return;
    }
    let label = self.windows.borrow().values().find_map(|window| {
      window
        .webviews
        .iter()
        .find(|webview| webview.serial == serial)
        .map(|webview| webview.label.clone())
    });
    if let Some(label) = label {
      self.set_webview_origin(&label, url);
    }
  }

  /// Remembers the origin Electron should report for custom-protocol requests
  /// issued by `label`'s page.
  pub(crate) fn set_webview_origin(&self, label: &str, url: &str) {
    match origin_of(url) {
      Some(origin) => {
        self
          .webview_origins
          .borrow_mut()
          .insert(label.to_string(), origin);
      }
      None => {
        self.webview_origins.borrow_mut().remove(label);
      }
    }
  }

  pub(crate) fn next_window_event_id(&self) -> WindowEventId {
    self.next_window_event_id.fetch_add(1, Ordering::Relaxed)
  }

  pub(crate) fn next_webview_event_id(&self) -> WindowEventId {
    self.next_webview_event_id.fetch_add(1, Ordering::Relaxed)
  }

  pub(crate) fn add_window_event_handler(
    &self,
    window_id: WindowId,
    event_id: WindowEventId,
    handler: impl Fn(&WindowEvent) + Send + 'static,
  ) {
    if let Err(error) = self.on_main_thread(move |context| {
      if let Some(window) = context.windows.borrow_mut().get_mut(&window_id) {
        window.handlers.push((event_id, Rc::new(handler)));
      }
    }) {
      eprintln!("[electron] failed to register window handler: {error}");
    }
  }

  pub(crate) fn add_webview_event_handler(
    &self,
    serial: u32,
    event_id: tauri_runtime::WebviewEventId,
    handler: impl Fn(&tauri_runtime::window::WebviewEvent) + Send + 'static,
  ) {
    if let Err(error) = self.on_main_thread(move |context| {
      for window in context.windows.borrow_mut().values_mut() {
        if let Some(webview) = window
          .webviews
          .iter_mut()
          .find(|webview| webview.serial == serial)
        {
          webview.handlers.push((event_id, Rc::new(handler)));
          break;
        }
      }
    }) {
      eprintln!("[electron] failed to register webview handler: {error}");
    }
  }

  /// Runs every handler Tauri registered for `window_id`.
  pub(crate) fn emit_window_event(&self, window_id: WindowId, event: &WindowEvent) {
    let handlers: Vec<_> = self
      .windows
      .borrow()
      .get(&window_id)
      .map(|window| {
        window
          .handlers
          .iter()
          .map(|(_, handler)| handler.clone())
          .collect()
      })
      .unwrap_or_default();
    for handler in handlers {
      handler(event);
    }
  }

  pub(crate) fn new_window<T: UserEvent>(&self, params: &Value) -> Result<rpc::webview::NewWindowDecision> {
    use crate::{
      new_window::{with_opener, NewWindowFeatures, NewWindowOpener, NewWindowResponse},
      ElectronWindowBuilder,
    };
    use tauri_runtime::{webview::WebviewAttributes, window::WindowBuilder};
    use tauri_utils::config::{WebviewUrl, WindowConfig};

    let serial = params["webviewId"]
      .as_u64()
      .and_then(|id| u32::try_from(id).ok())
      .ok_or_else(|| create_webview_err("invalid popup source".into()))?;
    self.window_for_webview(serial)?;
    let request = params["request"]
      .as_u64()
      .ok_or_else(|| create_webview_err("invalid popup request".into()))?;
    let url = url::Url::parse(params["url"].as_str().unwrap_or_default())
      .map_err(|error| create_webview_err(error.to_string()))?;
    let features = NewWindowFeatures {
      frame_name: params["frameName"].as_str().unwrap_or_default().into(),
      features: params["features"].as_str().unwrap_or_default().into(),
      disposition: params["disposition"].as_str().unwrap_or_default().into(),
      position: serde_json::from_value(params["position"].clone()).ok(),
      size: serde_json::from_value(params["size"].clone()).ok(),
      opener: NewWindowOpener {
        request,
        ipc: Arc::downgrade(&self.ipc),
      },
    };
    let handler = self.new_window_handlers.borrow().get(&serial).cloned();
    let response = handler
      .map(|handler| handler(url.clone(), features.clone()))
      .unwrap_or(NewWindowResponse::Allow);
    let selected = match response {
      NewWindowResponse::Deny => None,
      NewWindowResponse::Create { window } => {
        if !Arc::ptr_eq(&self.ipc, &window.context.ipc) {
          return Err(create_webview_err(
            "popup destination belongs to another runtime".into(),
          ));
        }
        let windows = self.windows.borrow();
        let target = windows
          .get(&window.id)
          .filter(|window| window.new_window_request == Some(request))
          .ok_or_else(|| {
            create_webview_err("popup destination was not built with this request's opener".into())
          })?;
        Some(target.serial)
      }
      NewWindowResponse::Allow => {
        // A handler may have built a destination before choosing Allow. Destroy
        // those staged shells through Electron so their normal window.closed
        // event still removes them from Tauri's registry.
        let staged: Vec<u32> = self
          .windows
          .borrow()
          .values()
          .filter(|window| window.new_window_request == Some(request))
          .map(|window| window.serial)
          .collect();
        for serial in staged {
          self
            .ipc
            .call(rpc::WindowDestroy { window_id: serial })
            .map_err(|error| create_webview_err(error.to_string()))?;
        }
        let label = format!("__electron_popup_{request}");
        let config = WindowConfig {
          width: features.size.map(|size| size.width).unwrap_or(800.0),
          height: features.size.map(|size| size.height).unwrap_or(600.0),
          x: features.position.map(|position| position.x),
          y: features.position.map(|position| position.y),
          ..Default::default()
        };
        let mut webview = PendingWebview::<T, ElectronRuntime>::new(
          WebviewAttributes::new(WebviewUrl::External(url.clone())),
          label.clone(),
        )?;
        webview.url = url.to_string();
        let pending = with_opener(features.opener(), || {
          let builder = ElectronWindowBuilder::with_config(&config);
          let mut pending = PendingWindow::new(builder, &label)?;
          pending.set_webview(webview);
          Ok::<_, Error>(pending)
        })?;
        let window = create_window_impl(self, pending, None::<fn(RawWindow<'_>)>)?;
        self.serial_for_window(window.id)
      }
    };
    // Rejected staged shells are destroyed by Electron's decision cleanup.
    // Keep their identities until window.closed delivers Destroyed to Tauri.
    Ok(rpc::webview::NewWindowDecision {
      action: if selected.is_some() { rpc::webview::NewWindowAction::Allow } else { rpc::webview::NewWindowAction::Deny },
      window_id: selected,
    })
  }
}

impl fmt::Debug for RuntimeContext {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("RuntimeContext").finish()
  }
}

#[derive(Clone)]
pub struct EventProxy {
  pub(crate) run_tx: SyncSender<Message>,
}

impl fmt::Debug for EventProxy {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("EventProxy").finish()
  }
}

impl<T: UserEvent> EventLoopProxy<T> for EventProxy {
  fn send_event(&self, event: T) -> Result<()> {
    self
      .run_tx
      .send(Message::UserEvent(Box::new(event)))
      .map_err(|_| Error::EventLoopClosed)
  }
}

#[derive(Debug)]
pub struct ElectronRuntime {
  is_running: Arc<AtomicBool>,
  pub context: RuntimeContext,
  run_rx: Receiver<Message>,
  /// Events pushed by the Electron process (`window.closed`, `ipc.closed`, ...).
  event_rx: Receiver<Value>,
}

impl ElectronRuntime {
  fn init(
    ipc: Arc<ElectronProcess>,
    event_rx: Receiver<Value>,
    dev_url: Option<String>,
    icon: Option<PathBuf>,
  ) -> Self {
    let is_running = Arc::new(AtomicBool::new(false));
    let (tx, rx) = sync_channel(256);
    let context = RuntimeContext {
      is_running: is_running.clone(),
      windows: Default::default(),
      shortcuts: Default::default(),
      run_tx: tx,
      main_thread_id: std::thread::current().id(),
      next_window_id: Default::default(),
      next_webview_id: Default::default(),
      next_serial: Default::default(),
      next_window_event_id: Default::default(),
      next_webview_event_id: Default::default(),
      ipc,
      dev_url,
      icon,
      ipc_handlers: Default::default(),
      webview_handlers: Default::default(),
      protocol_handlers: Default::default(),
      webview_origins: Default::default(),
      policy_handlers: Default::default(),
      native_handles: Default::default(),
      new_window_handlers: Default::default(),
    };
    Self {
      is_running,
      context,
      run_rx: rx,
      event_rx,
    }
  }

  fn start(attrs: Electron, custom_schemes: &[&str]) -> Result<Self> {
    crate::platform::dpi::enable_per_monitor_v2();
    let binary = attrs
      .resolve_binary()
      .map_err(|error| create_window_err(error.to_string()))?;
    // Report an explicitly low file label without changing deployment security.
    #[cfg(windows)]
    crate::platform::integrity::check_binary_label(&binary).map_err(create_window_err)?;
    eprintln!("[electron] launching {}", binary.display());
    let mut args = extra_args(&attrs.args);
    switches::warn_about_dangerous_switches(&args);
    // Chromium reads its diagnostic environment variables before any embedder
    // hook runs, so this has to happen before Electron is spawned.
    environment::sanitize(attrs.debug_environment, &mut args);
    if !args.iter().any(|arg| arg.starts_with("--lang")) {
      if let Some(list) = locale::system_accept_language_list() {
        args.push(format!("--lang={list}"));
      }
    }
    // Electron only accepts privileged schemes at startup, so they travel on the
    // command line and are installed before `app.whenReady()`. `asset` is not in
    // Tauri's `custom_schemes` (its handler is registered per webview), but it
    // needs the same privileges for `convertFileSrc` fetches to work.
    let mut schemes: Vec<String> = ["ipc", "tauri", "asset"]
      .iter()
      .map(|scheme| scheme.to_string())
      .collect();
    for scheme in custom_schemes
      .iter()
      .map(|scheme| (*scheme).to_string())
      .chain(take_startup_schemes())
    {
      if !schemes.contains(&scheme) {
        schemes.push(scheme);
      }
    }
    args.push("--tauri-schemes".to_string());
    args.push(schemes.join(","));
    if let Some(identifier) = attrs
      .identifier
      .as_deref()
      .or(option_env!("ELECTRON_RS_TAURI_IDENTIFIER"))
    {
      args.push("--tauri-identifier".to_string());
      args.push(identifier.to_string());
    }
    let ipc = ElectronProcess::connect(
      binary.clone(),
      0,
      args,
      attrs.startup_timeout.unwrap_or(STARTUP_TIMEOUT),
    )
    .map_err(|error| create_window_err(error.to_string()))?;
    let event_rx = ipc
      .events()
      .map_err(|error| create_window_err(error.to_string()))?;
    let icon = attrs.resolve_icon().or_else(|| {
      // A packaged app has no icon file beside it: the bundler embedded the
      // icon in the executable, so copy it back out for Electron.
      std::env::current_exe()
        .ok()
        .and_then(|exe| crate::app_icon::extract(&exe))
    });
    Ok(Self::init(ipc, event_rx, attrs.dev_url, icon))
  }
}

/// Startup options for [`ElectronRuntime`].
///
/// Select the runtime with `tauri::Builder::<ElectronRuntime>::new()`.
#[derive(Debug, Clone, Default)]
pub struct Electron {
  binary: Option<PathBuf>,
  args: Vec<String>,
  /// Dev server origin; `tauri://localhost` URLs are rewritten to it.
  dev_url: Option<String>,
  /// Window icon sent to Electron; defaults to the bundle icon.
  icon: Option<PathBuf>,
  /// Whether Chromium diagnostic environment variables are honoured.
  debug_environment: DebugEnvironment,
  /// App identifier, handed to Electron as its AppUserModelID (taskbar grouping).
  identifier: Option<String>,
  startup_timeout: Option<Duration>,
}

impl Electron {
  pub fn new() -> Self {
    Self::default()
  }

  /// Reads Tauri's official development URL from `TAURI_DEV_URL`.
  pub fn from_env() -> Self {
    let mut electron = Self::default();
    if let Ok(dev_url) = std::env::var("TAURI_DEV_URL") {
      electron.dev_url = Some(dev_url.trim_end_matches('/').to_string());
    }
    electron
  }

  pub fn binary(mut self, binary: impl Into<PathBuf>) -> Self {
    self.binary = Some(binary.into());
    self
  }

  /// Resolve the binary and spawn a separate Node process on the current Tokio runtime.
  ///
  /// Returns a native child with piped stdout/stderr, null stdin and kill-on-drop.
  /// Use its native async I/O, wait and kill methods; this does not start an RPC bridge.
  pub fn node(&self, args: &[&str]) -> std::io::Result<tokio::process::Child> {
    electron_rs::node(self.resolve_binary().map_err(std::io::Error::other)?, args)
  }

  /// Run the resolved binary normally in a separate Electron process.
  /// Clears `ELECTRON_RUN_AS_NODE`; nonzero exits remain `Ok(Output)`.
  pub fn electron(&self, args: &[&str]) -> std::io::Result<std::process::Output> {
    electron_rs::electron(self.resolve_binary().map_err(std::io::Error::other)?, args)
  }

  /// Appended to the Electron command line (after the main script).
  pub fn arg(mut self, arg: impl Into<String>) -> Self {
    self.args.push(arg.into());
    self
  }

  pub fn dev_url(mut self, url: impl Into<String>) -> Self {
    self.dev_url = Some(url.into().trim_end_matches('/').to_string());
    self
  }

  /// Window/taskbar icon; defaults to `bundle.icon` from `tauri.conf.json`.
  pub fn icon(mut self, icon: impl Into<PathBuf>) -> Self {
    self.icon = Some(icon.into());
    self
  }

  pub fn timeout(mut self, timeout: Duration) -> Self {
    self.startup_timeout = Some(timeout);
    self
  }

  /// Whether `SSLKEYLOGFILE`, `NODE_OPTIONS` and friends reach Electron.
  /// `Auto` honours them in debug builds and refuses them in release builds.
  pub fn debug_environment(mut self, policy: DebugEnvironment) -> Self {
    self.debug_environment = policy;
    self
  }

  fn resolve_binary(&self) -> std::result::Result<PathBuf, electron_rs::Error> {
    if let Some(path) = &self.binary {
      return electron_rs::electron_binary(path);
    }
    let executable = std::env::current_exe()?;
    let mut lookup_error = None;
    if let Some(dir) = executable.parent() {
      let product_name =
        option_env!("ELECTRON_RS_TAURI_PRODUCT_NAME").filter(|_| cfg!(target_os = "linux"));
      let app_dir = std::env::var_os("APPDIR").map(PathBuf::from);
      for root in bundled_roots(dir, product_name, app_dir.as_deref()) {
        if root.is_dir() {
          match electron_rs::electron_binary(&root) {
            Ok(binary) => return Ok(binary),
            Err(error) => {
              lookup_error.get_or_insert(error);
            }
          }
        }
      }
    }
    // Release builds must never use a path on the build machine.
    #[cfg(debug_assertions)]
    if let Some(path) = option_env!("ELECTRON_RS_STAGED_BINARY") {
      if Path::new(path).is_file() {
        return electron_rs::electron_binary(path);
      }
    }
    Err(lookup_error.unwrap_or_else(|| "Electron runtime was not bundled; include target/electron as runtime. Runtime downloads are disabled.".into()))
  }

  /// Bundle icon paths are relative to the app's `tauri.conf.json` (the crate
  /// using the runtime), which is not this crate's own manifest dir.
  fn resolve_icon(&self) -> Option<PathBuf> {
    let icon = self.icon.as_ref()?;
    // Electron resolves a relative icon path against its own cwd, so hand it an
    // absolute one. Windows canonicalization yields a verbatim `\\?\` path that
    // Electron's image loader cannot open, so strip that prefix again.
    let absolute = |path: &PathBuf| {
      let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.clone());
      match path.to_string_lossy().strip_prefix(r"\\?\") {
        Some(stripped) => PathBuf::from(stripped),
        None => path,
      }
    };
    if icon.exists() {
      return Some(absolute(icon));
    }
    let mut dir = std::env::current_dir().ok();
    for _ in 0..6 {
      let Some(current) = dir else { break };
      let candidate = current.join(icon);
      if candidate.exists() {
        return Some(absolute(&candidate));
      }
      dir = current.parent().map(PathBuf::from);
    }
    eprintln!("[electron] window icon not found: {}", icon.display());
    None
  }
}

fn bundled_roots(dir: &Path, product_name: Option<&str>, app_dir: Option<&Path>) -> Vec<PathBuf> {
  let mut roots = vec![
    dir.join("runtime"),
    dir.join("resources/runtime"),
    dir.join("../Resources/runtime"),
    dir.join("electron"),
    dir.join("resources/electron"),
  ];
  // The build script validates productName before emitting compile-time metadata.
  if let Some(product_name) = product_name {
    roots.push(dir.join("../lib").join(product_name).join("runtime"));
    if let Some(app_dir) = app_dir {
      roots.push(app_dir.join("usr/lib").join(product_name).join("runtime"));
    }
    roots.push(Path::new("/usr/lib").join(product_name).join("runtime"));
  }
  roots
}

impl<T: UserEvent> Runtime<T> for ElectronRuntime {
  type WindowDispatcher = ElectronWindowDispatcher;
  type WebviewDispatcher = ElectronWebviewDispatcher;
  type Handle = ElectronRuntimeHandle;
  type EventLoopProxy = EventProxy;

  fn new(_args: RuntimeInitArgs) -> Result<Self> {
    Self::start(Electron::from_env(), DEFAULT_SCHEMES)
  }

  #[cfg(any(
    windows,
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd"
  ))]
  fn new_any_thread(_args: RuntimeInitArgs) -> Result<Self> {
    Self::start(Electron::from_env(), DEFAULT_SCHEMES)
  }

  fn create_proxy(&self) -> EventProxy {
    EventProxy {
      run_tx: self.context.run_tx.clone(),
    }
  }

  fn handle(&self) -> Self::Handle {
    ElectronRuntimeHandle {
      context: self.context.clone(),
    }
  }

  fn create_window<F: Fn(RawWindow<'_>) + Send + 'static>(
    &self,
    pending: PendingWindow<T, Self>,
    after_window_creation: Option<F>,
  ) -> Result<DetachedWindow<T, Self>> {
    create_window_impl(&self.context, pending, after_window_creation)
  }

  fn create_webview(
    &self,
    window_id: WindowId,
    pending: PendingWebview<T, Self>,
  ) -> Result<DetachedWebview<T, Self>> {
    create_webview_impl(&self.context, window_id, pending)
  }

  fn primary_monitor(&self) -> Option<Monitor> {
    let displays = self.context.ipc.call(rpc::Displays {}).ok()?;
    Some(platform::monitor_from_display(&displays.primary))
  }

  fn monitor_from_point(&self, x: f64, y: f64) -> Option<Monitor> {
    let display = self.context.ipc.call(rpc::DisplayForPoint { x, y }).ok()?;
    display.as_ref().map(platform::monitor_from_display)
  }

  fn available_monitors(&self) -> Vec<Monitor> {
    self
      .context
      .ipc
      .call(rpc::Displays {})
      .ok()
      .map(|displays| {
        displays
          .displays
          .iter()
          .map(platform::monitor_from_display)
          .collect()
      })
      .unwrap_or_default()
  }

  fn set_theme(&self, theme: Option<Theme>) {
    let theme = match theme {
      Some(Theme::Dark) => electron_rs::Theme::Dark,
      Some(_) => electron_rs::Theme::Light,
      None => electron_rs::Theme::System,
    };
    if let Err(error) = self.context.ipc.call(rpc::AppTheme { theme }) {
      eprintln!("[electron] set theme failed: {error}");
    }
  }

  // Electron exposes these application-level switches through `app`, not the
  // window: macOS activation policy, dock icon visibility, and app show/hide.
  #[cfg(target_os = "macos")]
  #[cfg_attr(docsrs, doc(cfg(target_os = "macos")))]
  fn set_activation_policy(&mut self, activation_policy: tauri_runtime::ActivationPolicy) {
    let policy = match activation_policy {
      tauri_runtime::ActivationPolicy::Regular => rpc::app::ActivationPolicy::Regular,
      tauri_runtime::ActivationPolicy::Accessory => rpc::app::ActivationPolicy::Accessory,
      tauri_runtime::ActivationPolicy::Prohibited => rpc::app::ActivationPolicy::Prohibited,
      _ => rpc::app::ActivationPolicy::Regular,
    };
    if let Err(error) = self.context.ipc.call(rpc::AppActivationPolicy { policy }) {
      eprintln!("[electron] set activation policy failed: {error}");
    }
  }

  #[cfg(target_os = "macos")]
  #[cfg_attr(docsrs, doc(cfg(target_os = "macos")))]
  fn set_dock_visibility(&mut self, visible: bool) {
    if let Err(error) = self.context.ipc.call(rpc::AppDockVisibility { visible }) {
      eprintln!("[electron] set dock visibility failed: {error}");
    }
  }

  #[cfg(target_os = "macos")]
  #[cfg_attr(docsrs, doc(cfg(target_os = "macos")))]
  fn set_activate_ignoring_other_apps(&mut self, ignore: bool) {
    // There is no direct equivalent; focusing with `steal` is the closest.
    if let Err(error) = self.context.ipc.call(rpc::AppFocus { steal: ignore }) {
      eprintln!("[electron] focus app failed: {error}");
    }
  }

  #[cfg(target_os = "macos")]
  #[cfg_attr(docsrs, doc(cfg(target_os = "macos")))]
  fn show(&self) {
    if let Err(error) = self.context.ipc.call(rpc::AppShow {}) {
      eprintln!("[electron] show app failed: {error}");
    }
  }

  #[cfg(target_os = "macos")]
  #[cfg_attr(docsrs, doc(cfg(target_os = "macos")))]
  fn hide(&self) {
    if let Err(error) = self.context.ipc.call(rpc::AppHide {}) {
      eprintln!("[electron] hide app failed: {error}");
    }
  }

  fn set_device_event_filter(&mut self, _filter: DeviceEventFilter) {
    unsupported(
      "Runtime::set_device_event_filter",
      "Electron reports no raw device events",
    );
  }

  #[cfg(any(
    target_os = "macos",
    windows,
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd"
  ))]
  fn run_iteration<F: FnMut(RunEvent<T>)>(&mut self, callback: F) {
    self.pump(callback, true);
  }

  fn run<F: FnMut(RunEvent<T>) + 'static>(self, callback: F) {
    std::process::exit(self.run_return(callback));
  }

  fn run_return<F: FnMut(RunEvent<T>) + 'static>(self, callback: F) -> i32 {
    self.pump(callback, false)
  }

  fn cursor_position(&self) -> Result<PhysicalPosition<f64>> {
    let point = self
      .context
      .ipc
      .call(rpc::CursorPosition {})
      .map_err(|error| create_window_err(error.to_string()))?;
    Ok(PhysicalPosition::new(f64::from(point.x), f64::from(point.y)))
  }
}

impl ElectronRuntime {
  fn pump<T: UserEvent, F: FnMut(RunEvent<T>)>(&self, mut callback: F, once: bool) -> i32 {
    self.is_running.store(true, Ordering::Relaxed);
    if !once {
      callback(RunEvent::Ready);
    }

    let exit_code = 'event_loop: loop {
      // Events pushed by the Electron process.
      loop {
        let event = match self.event_rx.try_recv() {
          Ok(event) => event,
          Err(std::sync::mpsc::TryRecvError::Empty) => break,
          // A full event queue can drop ipc.closed; disconnect is definitive.
          Err(std::sync::mpsc::TryRecvError::Disconnected) => break 'event_loop 0,
        };
        match event["event"].as_str() {
          #[cfg(windows)]
          Some("menu.command") => {
            let params = &event["params"];
            let Some(serial) = params["windowId"]
              .as_u64()
              .and_then(|id| u32::try_from(id).ok())
            else {
              continue;
            };
            let Some(command) = params["commandId"]
              .as_u64()
              .and_then(|id| u32::try_from(id).ok())
            else {
              continue;
            };
            let owner = self
              .context
              .windows
              .borrow()
              .values()
              .find(|window| window.serial == serial)
              .and_then(|window| window.menu_owner.as_ref().map(|owner| owner.handle));
            if let Some(owner) = owner {
              if crate::window::imp::menu_snapshot(owner)
                .is_ok_and(|menu| menu_command_enabled(&menu, command))
              {
                crate::window::imp::dispatch_menu_command(owner, command);
              }
            }
          }
          Some("window.themeChanged") => {
            let params = &event["params"];
            let Some(serial) = params["windowId"]
              .as_u64()
              .and_then(|id| u32::try_from(id).ok())
            else {
              continue;
            };
            let Some(id) = self.context.window_for_serial(serial) else {
              continue;
            };
            let theme = match params["theme"].as_str() {
              Some("dark") => Theme::Dark,
              Some("light") => Theme::Light,
              _ => continue,
            };
            #[cfg(windows)]
            if let Some(dark) = params["dark"].as_bool() {
              if let Err(error) = self
                .context
                .native_handle(id)
                .and_then(|handle| crate::window::imp::set_theme(handle, dark))
              {
                eprintln!("[electron] native caption theme failed: {error}");
              }
            }
            let label = self
              .context
              .windows
              .borrow()
              .get(&id)
              .map(|window| window.label.clone());
            if let Some(label) = label {
              self
                .context
                .emit_window_event(id, &WindowEvent::ThemeChanged(theme));
              callback(RunEvent::WindowEvent {
                label,
                event: WindowEvent::ThemeChanged(theme),
              });
            }
          }
          Some("webview.closed") => {
            if let Some(serial) = event["params"]["webviewId"]
              .as_u64()
              .and_then(|id| u32::try_from(id).ok())
            {
              if let Err(error) = self.context.forget_webview(serial) {
                eprintln!("[electron] webview cleanup failed: {error}");
              }
            }
          }
          Some("app.exitRequested") => {
            if request_exit(None, &mut callback) {
              break 'event_loop 0;
            }
          }
          Some("window.closeRequested") => {
            let serial = event["params"]["windowId"].as_u64().unwrap_or_default() as u32;
            let Some(id) = self.context.window_for_serial(serial) else {
              continue;
            };
            close_window(&self.context, id, &mut callback);
          }
          Some("window.closed") => {
            let serial = event["params"]["windowId"].as_u64().unwrap_or_default() as u32;
            let Some(id) = self.context.window_for_serial(serial) else {
              continue;
            };
            if window_closed(&self.context, id, &mut callback) {
              eprintln!("[electron] last window closed, exiting the loop (serial {serial})");
              break 'event_loop 0;
            }
          }
          Some("window.event") => {
            let params = &event["params"];
            let serial = params["windowId"].as_u64().unwrap_or_default() as u32;
            let Some(id) = self.context.window_for_serial(serial) else {
              continue;
            };
            let Some(label) = self
              .context
              .windows
              .borrow()
              .get(&id)
              .map(|window| window.label.clone())
            else {
              continue;
            };
            let Some(window_event) = window_event_for(params) else {
              continue;
            };
            self.context.emit_window_event(id, &window_event);
            callback(RunEvent::WindowEvent {
              label,
              event: window_event,
            });
          }
          Some("webview.navigationCommitted") => {
            let params = &event["params"];
            if let (Some(serial), Some(url)) = (
              params["webviewId"]
                .as_u64()
                .and_then(|id| u32::try_from(id).ok()),
              params["url"].as_str(),
            ) {
              self.context.set_webview_origin_by_serial(serial, url);
            }
          }
          Some(name @ ("webview.loadStarted" | "webview.loadFinished")) => {
            let params = &event["params"];
            let Some(serial) = params["webviewId"].as_u64() else {
              continue;
            };
            let Ok(url) = url::Url::parse(params["url"].as_str().unwrap_or_default()) else {
              continue;
            };
            let handler = self
              .context
              .webview_handlers
              .borrow()
              .get(&(serial as u32))
              .and_then(|handlers| handlers.page_load.clone());
            if let Some(handler) = handler {
              let page_load = if name == "webview.loadStarted" {
                PageLoadEvent::Started
              } else {
                PageLoadEvent::Finished
              };
              handler(url, page_load);
            }
          }
          Some("webview.titleChanged") => {
            let params = &event["params"];
            let Some(serial) = params["webviewId"].as_u64() else {
              continue;
            };
            let title = params["title"].as_str().unwrap_or_default().to_string();
            let handler = self
              .context
              .webview_handlers
              .borrow()
              .get(&(serial as u32))
              .and_then(|handlers| handlers.title.clone());
            if let Some(handler) = handler {
              handler(title);
            }
          }
          Some("webview.processGone") => {
            let Some(serial) = event["params"]["webviewId"]
              .as_u64()
              .and_then(|serial| u32::try_from(serial).ok())
            else {
              continue;
            };
            // The generic Tauri callback is only exposed on Apple targets.
            #[cfg(any(target_os = "macos", target_os = "ios"))]
            {
              let handler = self
                .context
                .webview_handlers
                .borrow()
                .get(&serial)
                .and_then(|handlers| handlers.process_terminated.clone());
              if let Some(handler) = handler {
                handler();
              }
            }
            eprintln!(
              "[electron] webview {serial} process gone: {}",
              event["params"]["reason"].as_str().unwrap_or("unknown")
            );
          }
          #[cfg(target_os = "macos")]
          Some("app.reopen") => {
            if let Some(has_visible_windows) = event["params"]["hasVisibleWindows"].as_bool() {
              callback(RunEvent::Reopen {
                has_visible_windows,
              });
            }
          }
          Some("webview.newWindow") => {
            let params = &event["params"];
            if let Some(id) = params["id"].as_u64() {
              let response = self
                .context
                .new_window::<T>(params)
                .unwrap_or_else(|error| {
                  eprintln!("[electron] native popup denied: {error}");
                  rpc::webview::NewWindowDecision { action: rpc::webview::NewWindowAction::Deny, window_id: None }
                });
              if let Err(error) = self
                .context
                .ipc
                .call(rpc::NewWindowResponse { id, response })
              {
                eprintln!("[electron] failed to answer native popup: {error}");
              }
            }
          }
          Some("webview.navigationRequested") => {
            let params = &event["params"];
            let (Some(id), Some(serial)) = (params["id"].as_u64(), params["webviewId"].as_u64())
            else {
              continue;
            };
            let handler = u32::try_from(serial).ok().and_then(|serial| {
              self
                .context
                .policy_handlers
                .borrow()
                .get(&serial)
                .and_then(|handlers| handlers.navigation.clone())
            });
            let allow = handler.is_some_and(|handler| {
              url::Url::parse(params["url"].as_str().unwrap_or_default())
                .is_ok_and(|url| handler(&url))
            });
            if let Err(error) = self.context.ipc.call(rpc::NavigationResponse { id, allow }) {
              eprintln!("[electron] failed to answer navigation: {error}");
            }
          }
          Some("download.requested") => {
            let params = &event["params"];
            let (Some(id), Some(serial)) = (params["id"].as_u64(), params["webviewId"].as_u64())
            else {
              continue;
            };
            let mut destination = PathBuf::from(params["destination"].as_str().unwrap_or_default());
            let handler = self
              .context
              .policy_handlers
              .borrow()
              .get(&(serial as u32))
              .and_then(|handlers| handlers.download.clone());
            // A webview without a handler keeps Electron's own destination.
            let allow = match handler {
              Some(handler) => url::Url::parse(params["url"].as_str().unwrap_or_default())
                .map(|url| {
                  handler(DownloadEvent::Requested {
                    url,
                    destination: &mut destination,
                  })
                })
                .unwrap_or(false),
              None => true,
            };
            if let Err(error) = self.context.ipc.call(rpc::DownloadResponse {
              id,
              allow,
              destination: Some(destination.to_string_lossy().into_owned()),
            }) {
              eprintln!("[electron] failed to answer a download request: {error}");
            }
          }
          Some("download.finished") => {
            let params = &event["params"];
            let Some(serial) = params["webviewId"].as_u64() else {
              continue;
            };
            let Ok(url) = url::Url::parse(params["url"].as_str().unwrap_or_default()) else {
              continue;
            };
            let path = params["path"].as_str().map(PathBuf::from);
            let success = params["success"].as_bool().unwrap_or(false);
            let handler = self
              .context
              .policy_handlers
              .borrow()
              .get(&(serial as u32))
              .and_then(|handlers| handlers.download.clone());
            if let Some(handler) = handler {
              handler(DownloadEvent::Finished { url, path, success });
            }
          }
          Some("permission.requested") => {
            let params = &event["params"];
            let (Some(id), Some(serial)) = (params["id"].as_u64(), params["webviewId"].as_u64())
            else {
              continue;
            };
            let kind = permission_kind(params["kind"].as_str().unwrap_or_default());
            let response = {
              let handlers = self.context.policy_handlers.borrow();
              match (
                kind,
                handlers
                  .get(&(serial as u32))
                  .and_then(|handlers| handlers.permission.as_deref()),
              ) {
                (Some(kind), Some(handler)) => permission_response_name(handler(kind)),
                // Tauri's `Default` means the runtime default; Electron's own
                // default for an unhandled permission is to deny it.
                _ => rpc::webview::PermissionDecision::Default,
              }
            };
            if let Err(error) = self
              .context
              .ipc
              .call(rpc::PermissionResponse { id, response })
            {
              eprintln!("[electron] failed to answer a permission request: {error}");
            }
          }
          Some("ipc.message") => {
            let params = &event["params"];
            let Some(serial) = params["webviewId"].as_u64() else {
              continue;
            };
            let Some(handler) = self
              .context
              .ipc_handlers
              .borrow()
              .get(&(serial as u32))
              .cloned()
            else {
              eprintln!("[electron] ipc message for unknown webview {serial}");
              continue;
            };
            let body = params["body"].as_str().unwrap_or_default().to_string();
            let url = params["url"].as_str().unwrap_or_default().to_string();
            // Keep the real sender URL for Tauri's ACL; an invalid or opaque
            // URL must never become the trusted tauri://localhost origin.
            match Request::builder().uri(url).body(body) {
              Ok(request) => handler(request),
              Err(error) => eprintln!("[electron] invalid ipc request url: {error}"),
            }
          }
          Some("protocol.request") => {
            let params = &event["params"];
            let Some(id) = params["id"].as_u64() else {
              continue;
            };
            let scheme = params["scheme"].as_str().unwrap_or_default().to_string();
            if scheme == "ipc" && params["webviewId"].as_u64().is_none() {
              eprintln!("[electron] ipc protocol request has no authenticated webview");
              continue;
            }
            let Some((label, handler)) = self
              .context
              .protocol_handlers
              .borrow()
              .iter()
              .filter(|((registered, owner), _)| {
                registered == &scheme
                  && (scheme != "ipc"
                    || *owner
                      == params["webviewId"]
                        .as_u64()
                        .and_then(|id| u32::try_from(id).ok()))
              })
              .min_by_key(|((_, owner), _)| *owner)
              .map(|(_, handler)| handler.clone())
            else {
              eprintln!("[electron] no protocol handler for {scheme}");
              continue;
            };
            let uri = params["uri"].as_str().unwrap_or_default().to_string();
            let body = params["body"]
              .as_str()
              .and_then(|body| {
                base64::Engine::decode(&base64::engine::general_purpose::STANDARD, body).ok()
              })
              .unwrap_or_default();
            let mut builder = Request::builder()
              .method(params["method"].as_str().unwrap_or("GET"))
              .uri(&uri);
            if let Some(headers) = params["headers"].as_object() {
              for (name, value) in headers {
                if let Some(value) = value.as_str() {
                  builder = builder.header(name.as_str(), value);
                }
              }
            }
            let mut request = match builder.body(body) {
              Ok(request) => request,
              Err(error) => {
                eprintln!("[electron] invalid protocol request {uri}: {error}");
                continue;
              }
            };
            // Never replace a real (including opaque) origin with a local one:
            // Tauri uses it to select the invocation's ACL execution context.
            // Browser-initiated requests without one use the registered owner.
            let origin = self.context.webview_origins.borrow().get(&label).cloned();
            set_missing_origin(request.headers_mut(), origin.as_deref());
            let ipc = self.context.ipc.clone();
            let request_uri = uri.clone();
            let responder: Box<UriSchemeProtocolResponder> =
              Box::new(move |response: Response<std::borrow::Cow<'static, [u8]>>| {
                use base64::Engine;
                let headers: std::collections::BTreeMap<String, String> = response
                  .headers()
                  .iter()
                  .filter_map(|(name, value)| {
                    value
                      .to_str()
                      .ok()
                      .map(|value| (name.to_string(), value.to_string()))
                  })
                  .collect();
                let body =
                  base64::engine::general_purpose::STANDARD.encode(response.body().as_ref());
                if let Err(error) = ipc.call(rpc::ProtocolRespond {
                  id,
                  status: Some(response.status().as_u16()),
                  headers: Some(headers),
                  body: Some(body),
                }) {
                  eprintln!("[electron] failed to answer protocol request {request_uri}: {error}");
                }
              });
            handler(&label, request, responder);
          }
          Some("ipc.closed") => {
            break 'event_loop 0;
          }
          _ => {}
        }
      }

      // Messages sent by Tauri (window operations run on the main thread).
      while let Ok(message) = self.run_rx.try_recv() {
        match message {
          Message::Task(task) => task(),
          Message::CloseWindow(id) => {
            if close_window(&self.context, id, &mut callback) {
              break 'event_loop 0;
            }
          }
          Message::DestroyWindow(id) => {
            if window_closed(&self.context, id, &mut callback) {
              break 'event_loop 0;
            }
          }
          Message::UserEvent(event) => {
            if let Ok(event) = event.downcast::<T>() {
              callback(RunEvent::UserEvent(*event));
            }
          }
          Message::RequestExit(code) => {
            if request_exit(Some(code), &mut callback) {
              break 'event_loop code;
            }
          }
        }
      }

      callback(RunEvent::MainEventsCleared);

      // Native UI (tray icons, menus) lives on this thread and posts window
      // messages here; Electron owns its own process, so nothing else pumps them.
      pump_thread_messages();
      #[cfg(windows)]
      {
        for (owner, method) in crate::window::imp::take_menu_actions() {
          let target = self
            .context
            .windows
            .borrow()
            .iter()
            .find_map(|(id, window)| {
              window
                .menu_owner
                .as_ref()
                .filter(|menu| menu.handle == owner)
                .map(|_| (*id, window.serial))
            });
          let Some((id, serial)) = target else { continue };
          if method == "window.close" {
            close_window(&self.context, id, &mut callback);
          } else {
            let result = match method {
              "window.minimize" => self
                .context
                .ipc
                .call(rpc::WindowMinimize { window_id: serial }),
              "window.maximize" => self
                .context
                .ipc
                .call(rpc::WindowMaximize { window_id: serial }),
              _ => continue,
            };
            if let Err(error) = result {
              eprintln!("[electron] native menu action failed: {error}");
            }
          }
        }
        self.context.sync_window_menus();
      }
      if once {
        return 0;
      }
      std::thread::sleep(Duration::from_millis(10));
    };

    self.is_running.store(false, Ordering::Relaxed);
    eprintln!("[electron] run loop finished, shutting the runtime down");
    // Tauri's exit cleanup still calls into the windows (hide/close), so it has
    // to run while Electron is alive; only then can the channel be torn down.
    callback(RunEvent::Exit);
    self.context.ipc.shutdown();
    exit_code
  }
}

/// Translates an Electron window event into the Tauri equivalent.
fn window_event_for(params: &Value) -> Option<WindowEvent> {
  match params["event"].as_str()? {
    "resize" => Some(WindowEvent::Resized(rect_size(&params["contentBounds"]))),
    "move" => Some(WindowEvent::Moved(rect_position(&params["bounds"]))),
    "focus" => Some(WindowEvent::Focused(true)),
    "blur" => Some(WindowEvent::Focused(false)),
    _ => None,
  }
}

/// Emits `WindowEvent::CloseRequested` for `id`; returns `true` if the runtime
/// should exit (window was removable and the app has no windows left).
/// Asks Tauri whether the window may close. Electron keeps the window alive
/// until the answer comes back, so `prevent_close` is actually honoured.
/// Returns `true` when the close should proceed.
fn request_close<T: UserEvent, F: FnMut(RunEvent<T>)>(
  context: &RuntimeContext,
  id: WindowId,
  callback: &mut F,
) -> bool {
  let Some(label) = context
    .windows
    .borrow()
    .get(&id)
    .map(|window| window.label.clone())
  else {
    return false;
  };

  let (tx, rx) = channel();
  let close_requested = WindowEvent::CloseRequested { signal_tx: tx };
  context.emit_window_event(id, &close_requested);
  callback(RunEvent::WindowEvent {
    label,
    event: close_requested,
  });
  !matches!(rx.try_recv(), Ok(true))
}

/// Forgets a window Electron already destroyed. Returns `true` when the app
/// should exit because no window is left.
fn window_closed<T: UserEvent, F: FnMut(RunEvent<T>)>(
  context: &RuntimeContext,
  id: WindowId,
  callback: &mut F,
) -> bool {
  let Some(label) = context
    .windows
    .borrow()
    .get(&id)
    .map(|window| window.label.clone())
  else {
    return false;
  };

  context.emit_window_event(id, &WindowEvent::Destroyed);
  context.forget_window(id);
  callback(RunEvent::WindowEvent {
    label,
    event: WindowEvent::Destroyed,
  });
  exit_if_empty(context, callback)
}

/// Runs the close handshake for a window that asked to close: ask Tauri, and
/// only tell Electron to destroy the window when nobody prevented it.
fn close_window<T: UserEvent, F: FnMut(RunEvent<T>)>(
  context: &RuntimeContext,
  id: WindowId,
  callback: &mut F,
) -> bool {
  if !request_close(context, id, callback) {
    return false;
  }
  if let Some(serial) = context.serial_for_window(id) {
    if let Err(error) = context.ipc.call(rpc::WindowDestroy { window_id: serial }) {
      eprintln!("[electron] failed to destroy window {serial}: {error}");
    }
  }
  false
}

/// Emits `RunEvent::ExitRequested` when no windows remain; returns `true` when
/// the app should exit.
fn exit_if_empty<T: UserEvent, F: FnMut(RunEvent<T>)>(
  context: &RuntimeContext,
  callback: &mut F,
) -> bool {
  if !context.windows.borrow().is_empty() {
    return false;
  }
  request_exit(None, callback)
}

/// Shares Wry's synchronous veto handshake for implicit and explicit exits.
fn request_exit<T: UserEvent, F: FnMut(RunEvent<T>)>(code: Option<i32>, callback: &mut F) -> bool {
  let (tx, rx) = channel();
  callback(RunEvent::ExitRequested { code, tx });
  !matches!(rx.try_recv(), Ok(ExitRequestedEventAction::Prevent))
}

#[cfg(any(windows, test))]
fn menu_command_enabled(menu: &Value, command: u32) -> bool {
  command <= u16::MAX as u32
    && menu.as_array().is_some_and(|items| {
      items.iter().any(|item| {
        item["enabled"].as_bool() == Some(true)
          && item["type"] != "separator"
          && if item["submenu"].is_array() {
            menu_command_enabled(&item["submenu"], command)
          } else {
            item["commandId"].as_u64() == Some(command as u64)
          }
      })
    })
}

fn remove_webview_protocols(protocols: &mut ProtocolHandlerMap, serial: u32, label: &str) {
  protocols.retain(|(_, owner), (registered_label, _)| {
    *owner != Some(serial) && (owner.is_some() || registered_label != label)
  });
}

pub(crate) fn create_window_impl<T: UserEvent, F: Fn(RawWindow<'_>) + Send + 'static>(
  context: &RuntimeContext,
  pending: PendingWindow<T, ElectronRuntime>,
  after_window_creation: Option<F>,
) -> Result<DetachedWindow<T, ElectronRuntime>> {
  if std::thread::current().id() != context.main_thread_id {
    return context.on_main_thread(move |context| {
      create_window_impl(context, pending, after_window_creation)
    })?;
  }
  let PendingWindow {
    label,
    window_builder,
    mut webview,
  } = pending;

  if let Some(webview) = &webview {
    validate_webview(webview)?;
  }
  #[cfg(target_os = "macos")]
  if window_builder.macos_parent.is_some() {
    return Err(create_window_err(
      "Electron cannot use a process-local AppKit parent pointer".into(),
    ));
  }
  let opener = window_builder.new_window_opener.as_ref();
  if opener
    .is_some_and(|opener| !std::sync::Weak::ptr_eq(&opener.ipc, &Arc::downgrade(&context.ipc)))
  {
    return Err(create_window_err(
      "native popup opener belongs to another runtime".into(),
    ));
  }
  let new_window_request = opener.map(|opener| opener.request);
  let id = context.next_window_id();
  let serial = context.next_serial();
  let webview_id = webview.as_ref().map(|_| context.next_webview_id());
  let webview_url = webview
    .as_ref()
    .map(|webview| context.resolve_url(&webview.url))
    .unwrap_or_default();

  #[cfg(windows)]
  if window_builder.owner.is_some() && window_builder.parent.is_some() {
    return Err(create_window_err(
      "a Windows window cannot have both an owner and a parent".into(),
    ));
  }

  #[cfg(windows)]
  let resolve_native_handle = |handle| {
    let menu_owner = context.windows.borrow().iter().find_map(|(id, window)| {
      window
        .menu_owner
        .as_ref()
        .filter(|menu| menu.handle == handle)
        .map(|_| *id)
    });
    match menu_owner {
      Some(id) => context.native_handle(id),
      None => Ok(handle),
    }
  };
  #[cfg(windows)]
  let owner = window_builder
    .owner
    .map(resolve_native_handle)
    .transpose()?;
  #[cfg(windows)]
  let parent = window_builder
    .parent
    .map(resolve_native_handle)
    .transpose()?;
  #[cfg(windows)]
  if let Some(handle) = owner.or(parent) {
    crate::window::imp::validate_window_handle(handle)?;
  }

  // Build all fallible payload data before installing handlers or protocols.
  let mut payload = window_create_payload(&window_builder, serial)?;
  #[cfg(target_os = "linux")]
  let defer_show = !window_builder.config.closable
    && window_builder.config.visible
    && new_window_request.is_none();
  #[cfg(target_os = "linux")]
  if defer_show {
    payload.defer_show = Some(true);
  }
  if let Some(request) = new_window_request {
    payload.new_window_request = Some(request);
  }
  if let Some(icon) = &context.icon {
    payload.icon = Some(icon.display().to_string());
  }

  if let Some(webview) = webview.as_mut() {
    for (scheme, handler) in webview.uri_scheme_protocols.drain() {
      // Keep every owner's handler so closing one window does not discard the
      // session-wide asset protocol still used by another window.
      let owner = webview_id;
      context
        .protocol_handlers
        .borrow_mut()
        .entry((scheme.clone(), owner))
        .or_insert_with(|| (webview.label.clone(), Rc::from(handler)));
      if let Err(error) = context.ipc.call(rpc::ProtocolRegister {
        scheme: scheme.clone(),
      }) {
        eprintln!("[electron] failed to register protocol {scheme}: {error}");
      }
    }
  }

  // Handlers Electron must ask about later; announced in the create payload so
  // the main process knows whether to intercept downloads and permissions.
  let policy = match (webview.as_mut(), webview_id) {
    (Some(pending_webview), Some(webview_serial)) => {
      register_policy_handlers(context, webview_serial, pending_webview)
    }
    _ => rpc::webview::Policy::default(),
  };
  if let (Some(webview), Some(webview_id)) = (&webview, webview_id) {
    payload.webview = Some(webview_create_payload(
      webview,
      webview_id,
      &webview_url,
      policy,
    ));
  }

  if let Err(error) = context.ipc.call(payload) {
    if let (Some(webview), Some(serial)) = (webview.as_ref(), webview_id) {
      context.forget_webview_handlers(serial, &webview.label);
    }
    return Err(create_window_err(error.to_string()));
  }
  eprintln!("[electron] created window {label} (serial {serial})");

  context.windows.borrow_mut().insert(
    id,
    Window {
      label: label.clone(),
      new_window_request,
      serial,
      webviews: webview_id
        .map(|serial| Webview {
          serial,
          label: webview
            .as_ref()
            .map(|pending| pending.label.clone())
            .unwrap_or_else(|| label.clone()),
          handlers: Vec::new(),
        })
        .into_iter()
        .collect(),
      handlers: Vec::new(),
      #[cfg(windows)]
      menu_owner: None,
    },
  );

  let setup_result = (|| -> Result<()> {
    #[cfg(target_os = "linux")]
    if !window_builder.config.closable {
      let window = context.x11_window(id)?;
      crate::platform::linux_window::set_closable(window, false).map_err(create_window_err)?;
      if defer_show {
        context
          .ipc
          .call(rpc::WindowReady { window_id: serial })
          .map_err(|error| create_window_err(error.to_string()))?;
      }
    }
    #[cfg(windows)]
    if window_builder.owner.is_some()
      || window_builder.parent.is_some()
      || window_builder.config.always_on_bottom
    {
      let handle = context.native_handle(id)?;
      crate::window::imp::validate_window_handle(handle)?;
      if window_builder.config.always_on_bottom {
        crate::window::imp::send_to_bottom(handle);
      }
      if let Some(owner) = owner {
        crate::window::imp::set_owner(handle, owner)?;
      }
      if let Some(parent) = parent {
        crate::window::imp::set_parent(handle, parent)?;
      }
    }

    if let (Some(pending_webview), Some(webview_serial)) = (webview.as_mut(), webview_id) {
      context.set_webview_origin(&pending_webview.label, &webview_url);
      if let Some(handler) = pending_webview.ipc_handler.take() {
        register_ipc_handler::<T>(
          context,
          webview_serial,
          pending_webview.label.clone(),
          handler,
        );
      }
      register_webview_handlers::<T>(context, webview_serial, pending_webview);
    }

    if let Some(handler) = after_window_creation {
      #[cfg(any(windows, target_os = "macos"))]
      handler(RawWindow {
        #[cfg(windows)]
        hwnd: context.menu_owner_window(id)?,
        _marker: &std::marker::PhantomData,
      });
      #[cfg(not(any(windows, target_os = "macos")))]
      {
        let _ = handler;
        unsupported(
          "Runtime::create_window::after_window_creation",
          "Electron cannot supply the GTK window required by RawWindow",
        );
      }
    }
    Ok(())
  })();
  if let Err(error) = setup_result {
    if let Err(cleanup_error) = context.ipc.call(rpc::WindowDestroy { window_id: serial }) {
      eprintln!("[electron] failed to destroy window {serial} after setup error: {cleanup_error}");
    }
    context.forget_window(id);
    return Err(error);
  }

  let webview = webview_id.map(|webview_id| DetachedWindowWebview {
    webview: DetachedWebview {
      label: webview
        .as_ref()
        .map(|pending| pending.label.clone())
        .unwrap_or_else(|| label.clone()),
      dispatcher: ElectronWebviewDispatcher {
        id: webview_id,
        context: context.clone(),
        last_evaluated_script: Default::default(),
      },
    },
    use_https_scheme: false,
  });

  Ok(DetachedWindow {
    id,
    label,
    dispatcher: ElectronWindowDispatcher {
      id,
      context: context.clone(),
      effects_target: crate::window::EffectsTarget::new(serial, &context.ipc),
    },
    webview,
  })
}

pub(crate) fn create_webview_impl<T: UserEvent>(
  context: &RuntimeContext,
  window_id: WindowId,
  mut pending: PendingWebview<T, ElectronRuntime>,
) -> Result<DetachedWebview<T, ElectronRuntime>> {
  if std::thread::current().id() != context.main_thread_id {
    return context
      .on_main_thread(move |context| create_webview_impl(context, window_id, pending))?;
  }
  validate_webview(&pending)?;
  let serial = context
    .serial_for_window(window_id)
    .ok_or_else(|| create_webview_err("window does not exist".into()))?;
  let webview_id = context.next_webview_id();

  let url = context.resolve_url(&pending.url);
  if let Some(origin) = origin_of(&url) {
    context
      .webview_origins
      .borrow_mut()
      .insert(pending.label.clone(), origin);
  }

  // Schemes must be installed before the page is loaded, otherwise the initial
  // `tauri://` navigation fails with ERR_FAILED.
  for (scheme, handler) in pending.uri_scheme_protocols.drain() {
    let owner = Some(webview_id);
    context
      .protocol_handlers
      .borrow_mut()
      .entry((scheme.clone(), owner))
      .or_insert_with(|| (pending.label.clone(), Rc::from(handler)));
    if let Err(error) = context.ipc.call(rpc::ProtocolRegister {
      scheme: scheme.clone(),
    }) {
      eprintln!("[electron] failed to register protocol {scheme}: {error}");
    }
  }

  let policy = register_policy_handlers(context, webview_id, &mut pending);
  let payload = rpc::webview::Create {
    window_id: serial,
    config: webview_create_payload(&pending, webview_id, &url, policy),
  };
  if let Err(error) = context.ipc.call(payload) {
    context.forget_webview_handlers(webview_id, &pending.label);
    return Err(create_webview_err(error.to_string()));
  }

  if let Some(window) = context.windows.borrow_mut().get_mut(&window_id) {
    window.webviews.push(Webview {
      serial: webview_id,
      label: pending.label.clone(),
      handlers: Vec::new(),
    });
  }

  if let Some(handler) = pending.ipc_handler.take() {
    register_ipc_handler::<T>(context, webview_id, pending.label.clone(), handler);
  }
  register_webview_handlers::<T>(context, webview_id, &mut pending);

  Ok(DetachedWebview {
    label: pending.label,
    dispatcher: ElectronWebviewDispatcher {
      id: webview_id,
      context: context.clone(),
      last_evaluated_script: Default::default(),
    },
  })
}

fn window_create_payload(
  builder: &crate::window_builder::ElectronWindowBuilder,
  serial: u32,
) -> Result<rpc::WindowCreate> {
  let window_icon = builder.icon.as_ref().map(|icon| {
    use base64::Engine as _;
    let mut bgra = icon.rgba.clone();
    for pixel in bgra.chunks_exact_mut(4) {
      pixel.swap(0, 2);
    }
    rpc::WindowIconData {
      width: icon.width,
      height: icon.height,
      bgra: base64::engine::general_purpose::STANDARD.encode(bgra),
    }
  });
  // Tauri serializes title bar styles in PascalCase; the upstream contract is lowercase.
  let mut config = serde_json::to_value(&builder.config)
    .map_err(|error| create_window_err(error.to_string()))?;
  config["titleBarStyle"] = serde_json::to_value(rpc::title_bar_style(builder.config.title_bar_style)?)
    .map_err(|error| create_window_err(error.to_string()))?;
  Ok(rpc::WindowCreate {
    window_id: serial,
    // Keep unmodeled Tauri settings in the upstream config's extension map.
    window: serde_json::from_value(config)
      .map_err(|error| create_window_err(error.to_string()))?,
    size_constraints: builder.size_constraints.map(size_constraints_payload),
    window_icon,
    icon: None,
    defer_show: Some(false),
    new_window_request: None,
    webview: None,
  })
}

/// Main pages and native children use exactly the same webview preferences.
fn webview_create_payload<T: UserEvent>(
  pending: &PendingWebview<T, ElectronRuntime>,
  serial: u32,
  url: &str,
  policy: rpc::webview::Policy,
) -> rpc::webview::Config {
  let attributes = &pending.webview_attributes;
  let scroll_bar_style = match attributes.scroll_bar_style {
    #[cfg(windows)]
    tauri_runtime::webview::ScrollBarStyle::FluentOverlay => "fluentOverlay",
    _ => "default",
  };
  rpc::webview::Config {
    webview_id: serial,
    url: url.to_owned(),
    bounds: attributes.bounds.as_ref().map(|bounds| rpc::webview::BoundsInput {
      position: Some(position_payload(bounds.position)), size: Some(size_payload(bounds.size)), physical: None,
    }),
    auto_resize: Some(attributes.auto_resize),
    init_scripts: Some(initialization_scripts(pending)),
    data_directory: attributes.data_directory.as_ref().map(|path| path.to_string_lossy().into_owned()),
    incognito: Some(attributes.incognito),
    user_agent: attributes.user_agent.clone(),
    accept_first_mouse: Some(attributes.accept_first_mouse),
    javascript_disabled: Some(attributes.javascript_disabled),
    devtools: attributes.devtools,
    background_throttling: attributes.background_throttling.as_ref().map(|policy| match policy {
      tauri_utils::config::BackgroundThrottlingPolicy::Disabled => "disabled",
      tauri_utils::config::BackgroundThrottlingPolicy::Throttle => "throttle",
      tauri_utils::config::BackgroundThrottlingPolicy::Suspend => "suspend",
    }.to_owned()),
    transparent: Some(attributes.transparent),
    background_color: attributes.background_color.map(|color| electron_rs::rpc::Color::Rgba([color.0, color.1, color.2, color.3])),
    policy: Some(policy),
    additional: std::collections::BTreeMap::from([
      ("dragDropEnabled".into(), json!(attributes.drag_drop_handler_enabled)),
      ("zoomHotkeysEnabled".into(), json!(attributes.zoom_hotkeys_enabled)),
      ("scrollBarStyle".into(), json!(scroll_bar_style)),
      ("focus".into(), json!(attributes.focus)),
    ]),
  }
}

/// Extra Electron switches, e.g. `TAURI_RUNTIME_ELECTRON_ARGS="--remote-debugging-port=9222"`.
fn extra_args(args: &[String]) -> Vec<String> {
  let mut all = args.to_vec();
  if let Ok(extra) = std::env::var("TAURI_RUNTIME_ELECTRON_ARGS") {
    all.extend(extra.split_whitespace().map(str::to_string));
  }
  all
}

/// Stores Tauri's IPC handler so the run loop can feed it messages posted by
/// the page (`window.ipc.postMessage`). The closure captures the detached
/// webview so the stored type does not depend on `T`.
fn register_ipc_handler<T: UserEvent>(
  context: &RuntimeContext,
  webview_id: u32,
  label: String,
  handler: WebviewIpcHandler<T, ElectronRuntime>,
) {
  let detached = DetachedWebview::<T, ElectronRuntime> {
    label,
    dispatcher: ElectronWebviewDispatcher {
      id: webview_id,
      context: context.clone(),
      last_evaluated_script: Default::default(),
    },
  };
  let handler: Rc<IpcHandler> = Rc::new(move |request| handler(detached.clone(), request));
  context
    .ipc_handlers
    .borrow_mut()
    .insert(webview_id, handler);
}

/// Preserve frame scope through Electron's sandboxed preload.
fn initialization_scripts<T: UserEvent, R: Runtime<T>>(
  pending: &PendingWebview<T, R>,
) -> Vec<rpc::webview::InitScript> {
  pending.webview_attributes.initialization_scripts.iter().map(|script| {
    rpc::webview::InitScript::Detailed {
      script: script.script.clone(), main_frame_only: script.for_main_frame_only,
    }
  }).collect()
}

fn validate_webview<T: UserEvent, R: Runtime<T>>(pending: &PendingWebview<T, R>) -> Result<()> {
  if pending.new_window_handler.is_some() {
    return Err(create_webview_err(
      "new-window callbacks require a native opener in tauri-runtime 2.x; Electron cannot supply a WebView2/WKWebView/GTK object".into(),
    ));
  }
  Ok(())
}

/// Scheme+host of `url`, e.g. `tauri://localhost` — the origin Tauri sees for
/// a local page.
fn origin_of(url: &str) -> Option<String> {
  let url = url::Url::parse(url).ok()?;
  let host = url.host_str()?;
  Some(match url.port() {
    Some(port) => format!("{}://{host}:{port}", url.scheme()),
    None => format!("{}://{host}", url.scheme()),
  })
}

fn set_missing_origin(headers: &mut http::HeaderMap, fallback: Option<&str>) {
  if !headers.contains_key(http::header::ORIGIN) {
    if let Some(value) = fallback.and_then(|origin| http::HeaderValue::from_str(origin).ok()) {
      headers.insert(http::header::ORIGIN, value);
    }
  }
}

/// Tauri's webview-scoped callbacks, stored so the run loop can feed them the
/// events Electron forwards.
pub(crate) struct WebviewHandlers {
  pub(crate) page_load: Option<Rc<dyn Fn(url::Url, PageLoadEvent) + Send>>,
  pub(crate) title: Option<Rc<dyn Fn(String) + Send>>,
  #[cfg(any(target_os = "macos", target_os = "ios"))]
  process_terminated: Option<Rc<dyn Fn() + Send>>,
}

/// The callbacks Electron asks about out of band: a download question and
/// permission requests. Both are answered over IPC instead of blocking.
#[derive(Default)]
pub(crate) struct PolicyHandlers {
  download: Option<Arc<DownloadHandler>>,
  permission: Option<Box<dyn Fn(PermissionKind) -> PermissionResponse + Send + Sync>>,
  navigation: Option<Rc<NavigationHandler>>,
}

impl PolicyHandlers {
  fn is_empty(&self) -> bool {
    self.download.is_none() && self.permission.is_none() && self.navigation.is_none()
  }
}

/// Permission kinds the runtime can name; Electron reports richer strings that
/// also cover permissions Tauri has no kind for.
fn permission_kind(name: &str) -> Option<PermissionKind> {
  Some(match name {
    "microphone" => PermissionKind::Microphone,
    "camera" => PermissionKind::Camera,
    "geolocation" => PermissionKind::Geolocation,
    "notifications" => PermissionKind::Notifications,
    "clipboard-read" => PermissionKind::ClipboardRead,
    "display-capture" => PermissionKind::DisplayCapture,
    "midi" => PermissionKind::Midi,
    "sensors" => PermissionKind::Sensors,
    "media-key-system-access" => PermissionKind::MediaKeySystemAccess,
    _ => return None,
  })
}

fn permission_response_name(response: PermissionResponse) -> rpc::webview::PermissionDecision {
  match response {
    PermissionResponse::Allow => rpc::webview::PermissionDecision::Allow,
    PermissionResponse::Deny => rpc::webview::PermissionDecision::Deny,
    _ => rpc::webview::PermissionDecision::Default,
  }
}

fn register_webview_handlers<T: UserEvent>(
  context: &RuntimeContext,
  webview_id: u32,
  pending: &mut PendingWebview<T, ElectronRuntime>,
) {
  let page_load = pending.on_page_load_handler.take().map(|handler| {
    Rc::new(move |url: url::Url, event: PageLoadEvent| handler(url, event))
      as Rc<dyn Fn(url::Url, PageLoadEvent) + Send>
  });
  let title = pending
    .document_title_changed_handler
    .take()
    .map(|handler| Rc::new(move |title: String| handler(title)) as Rc<dyn Fn(String) + Send>);
  context.webview_handlers.borrow_mut().insert(
    webview_id,
    WebviewHandlers {
      page_load,
      title,
      #[cfg(any(target_os = "macos", target_os = "ios"))]
      process_terminated: pending
        .on_web_content_process_terminate_handler
        .take()
        .map(Rc::from),
    },
  );
}

/// Move generic policy callbacks off the attributes before loading the page.
fn register_policy_handlers<T: UserEvent>(
  context: &RuntimeContext,
  webview_id: u32,
  pending: &mut PendingWebview<T, ElectronRuntime>,
) -> rpc::webview::Policy {
  let handlers = PolicyHandlers {
    download: pending.download_handler.take(),
    permission: pending.permission_request_handler.take(),
    navigation: pending.navigation_handler.take().map(Rc::from),
  };
  let wired = rpc::webview::Policy {
    download: Some(handlers.download.is_some()),
    permission: Some(handlers.permission.is_some()),
    navigation: Some(handlers.navigation.is_some()),
  };
  if !handlers.is_empty() {
    context
      .policy_handlers
      .borrow_mut()
      .insert(webview_id, handlers);
  }
  wired
}

/// Drains the thread's window-message queue.
///
/// `tray-icon` and `muda` (used by `tauri::tray` and the menu API) create hidden
/// windows on the calling thread and only dispatch their callbacks from that
/// thread's message loop. Electron pumps its own process, not this one, so the
/// runtime has to do it.
#[cfg(windows)]
fn pump_thread_messages() {
  use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE,
  };

  let mut message = MSG::default();
  while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
    unsafe {
      let _ = TranslateMessage(&message);
      DispatchMessageW(&message);
    }
  }
}

#[cfg(not(windows))]
fn pump_thread_messages() {}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn window_creation_preserves_config_icons_and_mixed_constraints() {
    use base64::Engine as _;
    use tauri_runtime::dpi::{LogicalUnit, PhysicalUnit};
    use tauri_runtime::window::WindowBuilder;
    let config = tauri_utils::config::WindowConfig {
      focus: false,
      hidden_title: true,
      title_bar_style: tauri_utils::TitleBarStyle::Overlay,
      traffic_light_position: Some(tauri_utils::config::LogicalPosition { x: 12.5, y: 8.25 }),
      user_agent: Some("root-parity".into()),
      prevent_overflow: Some(tauri_utils::config::PreventOverflowConfig::Enable(true)),
      ..Default::default()
    };
    let builder = crate::window_builder::ElectronWindowBuilder::with_config(&config)
      .inner_size_constraints(WindowSizeConstraints {
        min_width: Some(PixelUnit::Physical(PhysicalUnit(250))),
        min_height: Some(PixelUnit::Logical(LogicalUnit(120.5))),
        max_width: None,
        max_height: Some(PixelUnit::Physical(PhysicalUnit(900))),
      })
      .icon(tauri_runtime::Icon {
        rgba: std::borrow::Cow::Borrowed(&[1, 2, 3, 4, 5, 6, 7, 8]),
        width: 2,
        height: 1,
      })
      .unwrap();
    let payload =
      serde_json::to_value(window_create_payload(&builder, 42).unwrap()).unwrap();
    // Both typed configs must decode the same Tauri configuration, despite enum casing/omitted nulls.
    let restored: tauri_utils::config::WindowConfig = serde_json::from_value(payload["window"].clone()).unwrap();
    assert_eq!(restored, config);
    assert_eq!(payload["window"]["titleBarStyle"], "overlay");
    assert_eq!(payload["window"]["trafficLightPosition"], json!({"x": 12.5, "y": 8.25}));
    assert_eq!(payload["windowId"], 42);
    assert_eq!(
      payload["sizeConstraints"],
      json!({
        "minWidth": { "value": 250.0, "physical": true },
        "minHeight": { "value": 120.5, "physical": false },
        "maxWidth": null,
        "maxHeight": { "value": 900.0, "physical": true },
      })
    );
    assert_eq!(
      base64::engine::general_purpose::STANDARD
        .decode(payload["windowIcon"]["bgra"].as_str().unwrap())
        .unwrap(),
      [3, 2, 1, 4, 7, 6, 5, 8]
    );
  }

  #[test]
  fn webview_preferences_reach_the_shared_creation_payload() {
    let mut attributes =
      tauri_runtime::webview::WebviewAttributes::new(tauri_utils::config::WebviewUrl::default());
    attributes.user_agent = Some("child-agent".into());
    attributes.accept_first_mouse = true;
    attributes.javascript_disabled = true;
    attributes.background_throttling =
      Some(tauri_utils::config::BackgroundThrottlingPolicy::Disabled);
    attributes.drag_drop_handler_enabled = false;
    attributes.devtools = Some(false);
    let pending = PendingWebview::<(), ElectronRuntime>::new(attributes, "child").unwrap();
    let payload = serde_json::to_value(webview_create_payload(
      &pending,
      8,
      "tauri://localhost",
      rpc::webview::Policy { navigation: Some(true), ..Default::default() },
    ))
    .unwrap();
    assert_eq!(payload["userAgent"], "child-agent");
    assert_eq!(payload["acceptFirstMouse"], true);
    assert_eq!(payload["javascriptDisabled"], true);
    assert_eq!(payload["backgroundThrottling"], "disabled");
    assert_eq!(payload["dragDropEnabled"], false);
    assert_eq!(payload["devtools"], false);
    assert_eq!(payload["policy"]["navigation"], true);
  }

  #[test]
  fn cleanup_removes_closed_protocol_owners_and_preserves_live_assets() {
    let handler: Rc<UriSchemeProtocolHandler> = Rc::new(|_, _, _| {});
    let mut protocols = ProtocolHandlerMap::from([
      (("ipc".into(), Some(1)), ("closed".into(), handler.clone())),
      (
        ("asset".into(), Some(1)),
        ("closed".into(), handler.clone()),
      ),
      (("asset".into(), Some(2)), ("live".into(), handler)),
    ]);
    remove_webview_protocols(&mut protocols, 1, "closed");
    assert_eq!(
      protocols.keys().cloned().collect::<Vec<_>>(),
      [("asset".to_string(), Some(2))]
    );
  }

  #[test]
  fn mirrored_menu_dispatch_uses_only_live_enabled_leaf_commands() {
    let menu = json!([
      { "enabled": true, "commandId": 7 },
      { "enabled": false, "commandId": 8 },
      { "enabled": true, "commandId": 9, "submenu": [{ "enabled": true, "commandId": 10 }] },
      { "enabled": false, "submenu": [{ "enabled": true, "commandId": 11 }] },
      { "enabled": true, "type": "separator", "commandId": 12 },
    ]);
    assert!(menu_command_enabled(&menu, 7));
    assert!(menu_command_enabled(&menu, 10));
    for command in [8, 9, 11, 12, 13, 65536] {
      assert!(!menu_command_enabled(&menu, command));
    }
  }

  #[test]
  fn bundled_roots_include_linux_product_paths_after_adjacent_resources() {
    let dir = Path::new("/opt/example/bin");
    let app_dir = Path::new("/appimage");
    let adjacent = vec![
      dir.join("runtime"),
      dir.join("resources/runtime"),
      dir.join("../Resources/runtime"),
      dir.join("electron"),
      dir.join("resources/electron"),
    ];
    assert_eq!(bundled_roots(dir, None, Some(app_dir)), adjacent);
    let mut expected = adjacent;
    expected.push(dir.join("../lib/Example App/runtime"));
    expected.push(app_dir.join("usr/lib/Example App/runtime"));
    expected.push(PathBuf::from("/usr/lib/Example App/runtime"));
    assert_eq!(
      bundled_roots(dir, Some("Example App"), Some(app_dir)),
      expected
    );
    expected.remove(6);
    assert_eq!(bundled_roots(dir, Some("Example App"), None), expected);
  }

  #[test]
  fn validates_deduplicates_and_consumes_startup_schemes() {
    assert!(take_startup_schemes().is_empty());
    for scheme in [
      "",
      "1bad",
      "isolation://localhost",
      "white space",
      "a,b",
      "über",
      "x\ny",
    ] {
      assert!(
        register_custom_protocol_scheme(scheme).is_err(),
        "{scheme:?}"
      );
    }
    for scheme in ["Isolation-Test", "isolation-test", "custom+1.2"] {
      register_custom_protocol_scheme(scheme).unwrap();
    }
    assert!(std::thread::spawn(take_startup_schemes)
      .join()
      .unwrap()
      .is_empty());
    assert_eq!(take_startup_schemes(), ["isolation-test", "custom+1.2"]);
    assert!(take_startup_schemes().is_empty());
  }

  #[test]
  fn keeps_real_protocol_origins() {
    let mut headers = http::HeaderMap::new();
    set_missing_origin(&mut headers, Some("http://tauri.localhost"));
    assert_eq!(headers[http::header::ORIGIN], "http://tauri.localhost");
    for origin in ["https://remote.example", "null"] {
      headers.insert(http::header::ORIGIN, origin.parse().unwrap());
      set_missing_origin(&mut headers, Some("http://tauri.localhost"));
      assert_eq!(headers[http::header::ORIGIN], origin);
    }
    headers.clear();
    set_missing_origin(&mut headers, Some("invalid\norigin"));
    assert!(!headers.contains_key(http::header::ORIGIN));
  }

  #[test]
  fn maps_permission_names_both_ways() {
    assert_eq!(permission_kind("camera"), Some(PermissionKind::Camera));
    assert_eq!(
      permission_kind("clipboard-read"),
      Some(PermissionKind::ClipboardRead)
    );
    assert_eq!(permission_kind("openExternal"), None);
    assert_eq!(permission_response_name(PermissionResponse::Allow), rpc::webview::PermissionDecision::Allow);
    assert_eq!(permission_response_name(PermissionResponse::Deny), rpc::webview::PermissionDecision::Deny);
    assert_eq!(
      permission_response_name(PermissionResponse::Default),
      rpc::webview::PermissionDecision::Default
    );
  }
}
