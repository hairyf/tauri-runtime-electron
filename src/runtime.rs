// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

use crate::{
  electron_impl::ipc::Ipc, environment, environment::DebugEnvironment, locale, platform, switches,
  webview::ElectronWebviewDispatcher, window::ElectronWindowDispatcher,
  window_handle::ElectronRuntimeHandle,
};
use serde_json::{json, Value};
use tauri_runtime::{
  DeviceEventFilter, Error, EventLoopProxy, ExitRequestedEventAction, Result, RunEvent, Runtime,
  RuntimeInitArgs, RuntimeInitAttrs, UserEvent, WindowEventId,
  dpi::{PhysicalPosition, PhysicalSize, Position, Size},
  monitor::Monitor,
  webview::{
    DetachedWebview, DownloadEvent, DownloadHandler, NewWindowFeatures, NewWindowResponse,
    PageLoadEvent, PendingWebview, PermissionKind, PermissionResponse, UriSchemeProtocolHandler,
    WebContentProcessTermination, WebContentProcessTerminationReason, WebviewIpcHandler,
  },
  window::{
    DetachedWindow, DetachedWindowWebview, PendingWindow, RawWindow, WindowEvent, WindowId,
  },
};
use tauri_utils::{Theme, config::WebviewUrl};

use http::{Request, Response};
use std::{
  any::Any,
  cell::RefCell,
  collections::HashMap,
  fmt,
  path::{Path, PathBuf},
  rc::Rc,
  sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU32, Ordering},
    mpsc::{Receiver, SyncSender, channel, sync_channel},
  },
  time::Duration,
};


/// Timeout for a single IPC round trip with the Electron process.
pub(crate) const IPC_TIMEOUT: Duration = Duration::from_secs(15);

/// Default time the Electron process gets to start and connect back.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(60);

fn io_err(message: String) -> Box<dyn std::error::Error + Send + Sync> {
  Box::new(std::io::Error::other(message))
}

pub(crate) fn create_window_err(message: String) -> Error {
  Error::CreateWindow(io_err(message))
}

pub(crate) fn create_webview_err(message: String) -> Error {
  Error::CreateWebview(io_err(message))
}

/// Tauri sizes are physical or logical; the Electron side converts to DIPs.
pub(crate) fn size_payload(size: Size) -> Value {
  match size {
    Size::Physical(size) => {
      json!({ "width": size.width, "height": size.height, "physical": true })
    }
    Size::Logical(size) => {
      json!({ "width": size.width, "height": size.height, "physical": false })
    }
  }
}

pub(crate) fn position_payload(position: Position) -> Value {
  match position {
    Position::Physical(position) => json!({ "x": position.x, "y": position.y, "physical": true }),
    Position::Logical(position) => json!({ "x": position.x, "y": position.y, "physical": false }),
  }
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

pub(crate) enum Message {
  Task(Box<dyn FnOnce() + Send>),
  CloseWindow(WindowId),
  DestroyWindow(WindowId),
  UserEvent(Box<dyn Any + Send>),
  Exit,
}

struct Webview {
  /// Numeric id handed to the Electron process.
  serial: u32,
  /// Label Tauri gave the webview (`navigate` keeps its origin in sync).
  label: String,
  /// Handlers registered by Tauri to observe this webview's events.
  handlers: Vec<(tauri_runtime::WebviewEventId, Rc<dyn Fn(&tauri_runtime::window::WebviewEvent) + Send>)>,
}

struct Window {
  label: String,
  /// Numeric id handed to the Electron process (Tauri's `WindowId` is opaque).
  serial: u32,
  webviews: Vec<Webview>,
  /// Handlers registered by Tauri to observe this window's events.
  handlers: Vec<(WindowEventId, Rc<dyn Fn(&WindowEvent) + Send>)>,
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
  pub(crate) ipc: Arc<Ipc>,
  /// When set, `tauri://localhost` URLs are rewritten to this origin (dev server).
  dev_url: Option<String>,
  /// Window icon forwarded to Electron on `window.create`.
  icon: Option<PathBuf>,
  /// Tauri IPC handlers, invoked when the page posts a message through
  /// `window.ipc.postMessage` (re-emitted by Electron as `ipc.message`).
  ///
  /// The handler is closed over the [`DetachedWebview`] it belongs to, so the
  /// map does not need to know the runtime's generic parameters.
  ipc_handlers: Arc<RefCell<HashMap<u32, Rc<dyn Fn(Request<String>) + Send>>>>,
  /// Tauri's page-load, title and new-window callbacks, by webview serial.
  webview_handlers: Arc<RefCell<HashMap<u32, WebviewHandlers>>>,
  /// Custom protocols registered by Tauri, served through Electron's
  /// `protocol.handle`.
  protocol_handlers: Arc<RefCell<HashMap<String, (String, Rc<UriSchemeProtocolHandler>)>>>,
  /// IPC responses retain their MIME type and the authenticated caller's label.
  ipc_protocol_handlers: Arc<RefCell<HashMap<u32, (String, Rc<UriSchemeProtocolHandler>)>>>,
  /// Page origin per webview label, injected as the `Origin` header on
  /// custom-protocol requests (Electron reports the request URL instead).
  webview_origins: Arc<RefCell<HashMap<String, String>>>,
  /// Download and permission callbacks, by webview serial. Electron asks for
  /// these asynchronously, so the run loop answers over IPC.
  policy_handlers: Arc<RefCell<HashMap<u32, PolicyHandlers>>>,
  /// Native window handle (HWND / NSView / X11 window) per window serial, asked
  /// from Electron once. Used for the Win32 tweaks Electron has no API for;
  /// `window_handle()` gives Tauri this handle everywhere except Windows menus,
  /// which need a window of the calling thread (see `window::imp`).
  native_handles: Arc<RefCell<HashMap<u32, isize>>>,
}

// SAFETY: Arc clones may cross threads; RefCell state and Rc callbacks are only
// accessed on the runtime UI thread, through on_main_thread when called elsewhere.
#[allow(clippy::non_send_fields_in_send_ty)]
unsafe impl Send for RuntimeContext {}

// SAFETY: Arc clones may cross threads; RefCell state and Rc callbacks are only
// accessed on the runtime UI thread, through on_main_thread when called elsewhere.
#[allow(clippy::non_send_fields_in_send_ty)]
unsafe impl Sync for RuntimeContext {}

impl RuntimeContext {
  /// Only this thread may access the RefCells and their non-Sync callbacks.
  fn on_main_thread<R: Send + 'static>(
    &self,
    task: impl FnOnce(&Self) -> R + Send + 'static,
  ) -> Result<R> {
    if std::thread::current().id() == self.main_thread_id {
      return Ok(task(self));
    }
    let context = self.clone();
    let (tx, rx) = sync_channel(1);
    self.run_tx.send(Message::Task(Box::new(move || {
      let _ = tx.send(task(&context));
    }))).map_err(|_| Error::FailedToSendMessage)?;
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
    if !is_main_thread || self.is_running.load(Ordering::Relaxed) {
      self
        .run_tx
        .send(message)
        .map_err(|_| Error::FailedToSendMessage)
    } else {
      match message {
        Message::Task(task) => task(),
        Message::CloseWindow(id) | Message::DestroyWindow(id) => {
          self.windows.borrow_mut().remove(&id);
        }
        // No run loop to deliver it to yet.
        Message::UserEvent(_) | Message::Exit => {}
      }
      Ok(())
    }
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
    self.on_main_thread(move |context| {
      context.windows.borrow().get(&id).map(|window| window.serial)
    }).ok().flatten()
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
      .call(
        "window.nativeHandle",
        json!({ "windowId": serial }),
        IPC_TIMEOUT,
      )
      .map_err(create_window_err)?;
    let handle = value["handle"]
      .as_i64()
      .ok_or_else(|| create_window_err("electron returned no native handle".to_string()))?;
    let handle = handle as isize;
    self.native_handles.borrow_mut().insert(serial, handle);
    Ok(handle)
  }

  #[cfg(windows)]
  pub(crate) fn menu_owner_window(&self, id: WindowId) -> Result<isize> {
    if std::thread::current().id() != self.main_thread_id {
      return self.on_main_thread(move |context| context.menu_owner_window(id))?;
    }
    let mut windows = self.windows.borrow_mut();
    let window = windows.get_mut(&id).ok_or_else(|| create_window_err("window is gone".into()))?;
    if window.menu_owner.is_none() {
      window.menu_owner = crate::window::imp::MenuOwner::new();
    }
    window.menu_owner.as_ref().map(|owner| owner.handle)
      .ok_or_else(|| create_window_err("could not create menu owner".into()))
  }

  #[cfg(windows)]
  fn sync_window_menus(&self) {
    // ponytail: O(total menu items) per pump; use native mutation hooks if large
    // menus make polling measurable. IPC and Electron rebuild only on changes.
    let changed: Vec<_> = self.windows.borrow().iter().filter_map(|(id, window)| {
      let owner = window.menu_owner.as_ref()?;
      match crate::window::imp::menu_snapshot(owner.handle) {
        Ok(menu) if menu != owner.mirrored => Some((*id, window.serial, menu)),
        Ok(_) => None,
        Err(error) => { eprintln!("[electron] menu snapshot failed: {error}"); None }
      }
    }).collect();
    for (id, serial, menu) in changed {
      match self.ipc.call("window.setMenu", json!({ "windowId": serial, "menu": menu }), IPC_TIMEOUT) {
        Ok(_) => {
          if let Some(owner) = self.windows.borrow_mut().get_mut(&id).and_then(|window| window.menu_owner.as_mut()) {
            owner.mirrored = menu;
          }
        }
        Err(error) => eprintln!("[electron] menu mirror failed: {error}"),
      }
    }
  }

  /// Tauri hands us `tauri://localhost/...` in production builds; when a dev
  /// server URL is configured we rewrite to it so the probe can load Vite.
  fn resolve_url(&self, url: &WebviewUrl) -> String {
    let raw = match url {
      WebviewUrl::External(url) => url.to_string(),
      WebviewUrl::App(path) => format!(
        "tauri://localhost/{}",
        path.to_string_lossy().trim_start_matches('/')
      ),
      WebviewUrl::CustomProtocol(url) => url.to_string(),
      // `WebviewUrl` is non-exhaustive.
      _ => String::new(),
    };
    match (&self.dev_url, raw.strip_prefix("tauri://localhost")) {
      (Some(dev), Some(rest)) => format!("{dev}{rest}"),
      _ => raw,
    }
  }

  /// Updates the remembered origin after a navigation.
  pub(crate) fn set_webview_origin_by_serial(&self, serial: u32, url: &str) {
    if std::thread::current().id() != self.main_thread_id {
      let url = url.to_owned();
      if let Err(error) = self.on_main_thread(move |context| context.set_webview_origin_by_serial(serial, &url)) {
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
        if let Some(webview) = window.webviews.iter_mut().find(|webview| webview.serial == serial) {
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
    ipc: Arc<Ipc>,
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
      ipc_protocol_handlers: Default::default(),
      webview_origins: Default::default(),
      policy_handlers: Default::default(),
      native_handles: Default::default(),
    };
    Self {
      is_running,
      context,
      run_rx: rx,
      event_rx,
    }
  }

  fn start(attrs: Electron, custom_schemes: &[String]) -> Result<Self> {
    let binary = attrs.resolve_binary();
    let main_script = attrs.resolve_main_script(&binary);
    if !main_script.exists() {
      return Err(Error::CreateWindow(io_err(format!(
        "Electron main script not found at {}; configure it with Electron::default().main_script(..)",
        main_script.display()
      ))));
    }
    eprintln!(
      "[electron] launching {} {}",
      binary.display(),
      main_script.display()
    );
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
    for scheme in custom_schemes.iter().chain(&attrs.schemes) {
      let scheme = scheme.to_ascii_lowercase();
      if !valid_scheme(&scheme) {
        return Err(create_window_err(format!("invalid custom URL scheme: {scheme:?}")));
      }
      if !schemes.contains(&scheme) {
        schemes.push(scheme);
      }
    }
    args.push("--tauri-schemes".to_string());
    args.push(schemes.join(","));
    if let Some(identifier) = &attrs.identifier {
      args.push("--tauri-identifier".to_string());
      args.push(identifier.clone());
    }
    let (ipc, event_rx) = Ipc::launch(
      &binary,
      &main_script,
      &args,
      attrs.startup_timeout.unwrap_or(STARTUP_TIMEOUT),
    )
    .map_err(|message| Error::CreateWindow(io_err(message)))?;
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

/// Selects the [`ElectronRuntime`], e.g.
/// `tauri::Builder::default().runtime(Electron::default())`.
#[derive(Debug, Clone, Default)]
pub struct Electron {
  binary: Option<PathBuf>,
  main_script: Option<PathBuf>,
  args: Vec<String>,
  /// Extra application protocols that must be privileged before Electron starts.
  schemes: Vec<String>,
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

  pub fn binary(mut self, binary: impl Into<PathBuf>) -> Self {
    self.binary = Some(binary.into());
    self
  }

  pub fn main_script(mut self, script: impl Into<PathBuf>) -> Self {
    self.main_script = Some(script.into());
    self
  }

  /// Appended to the Electron command line (after the main script).
  pub fn arg(mut self, arg: impl Into<String>) -> Self {
    self.args.push(arg.into());
    self
  }

  /// Privileges an application protocol before Electron starts.
  ///
  /// Tauri 3 alpha does not yet include builder-registered protocols in
  /// `RuntimeInitArgs::custom_schemes`; name each of those here too.
  pub fn custom_scheme(mut self, scheme: impl Into<String>) -> Self {
    self.schemes.push(scheme.into());
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

  fn resolve_binary(&self) -> PathBuf {
    if let Some(binary) = std::env::var_os("TAURI_RUNTIME_ELECTRON_BINARY") {
      return PathBuf::from(binary);
    }
    if let Some(binary) = &self.binary {
      return binary.clone();
    }
    // `build.rs` stages a runtime under `target/electron/<platform>-<arch>` and
    // reports it here, which covers `cargo install`-style builds without Node.
    if let Some(staged) = option_env!("TAURI_RUNTIME_ELECTRON_ELECTRON_BINARY") {
      let staged = PathBuf::from(staged);
      if staged.exists() {
        return staged;
      }
    }
    // A packaged app ships the runtime next to (or under `resources/` beside)
    // its executable, so those layouts beat anything found in the source tree.
    // `"resources": { "target/electron": "runtime" }` lands the staged payload in
    // `runtime/<platform>-<arch>/`, which the bundle target below covers.
    if let Ok(executable) = std::env::current_exe() {
      if let Some(dir) = executable.parent() {
        for candidate in [
          dir.join("electron").join(electron_executable()),
          dir.join("resources").join("electron").join(electron_executable()),
          dir
            .join("runtime")
            .join(format!("{}-{}", crate::acquisition::platform(), crate::acquisition::arch()))
            .join(electron_executable()),
          dir
            .join("resources")
            .join("runtime")
            .join(format!("{}-{}", crate::acquisition::platform(), crate::acquisition::arch()))
            .join(electron_executable()),
          dir.join("runtime").join(electron_executable()),
        ] {
          if candidate.exists() {
            return candidate;
          }
        }
      }
    }
    // Probe the usual pnpm/npm layout next to (or above) the current directory.
    let name = format!("node_modules/electron/dist/{}", electron_executable());
    let mut dir = std::env::current_dir().ok();
    for _ in 0..6 {
      let Some(current) = dir else { break };
      let candidate = current.join(&name);
      if candidate.exists() {
        return candidate;
      }
      dir = current.parent().map(PathBuf::from);
    }
    // A packaged app ships no Electron payload; the first launch fetches it into
    // the user cache (verified against the release checksums) and later launches
    // reuse that copy.
    match crate::acquisition::ensure_installed() {
      Ok(binary) => return binary,
      Err(error) => eprintln!("[electron] could not acquire Electron: {error}"),
    }
    PathBuf::from("electron")
  }

  fn resolve_main_script(&self, binary: &Path) -> PathBuf {
    if let Some(script) = &self.main_script {
      return script.clone();
    }
    // The staged runtime keeps `main.cjs` beside the Electron binary, which is
    // the only layout a packaged app is guaranteed to have.
    let sibling = binary.with_file_name("main.cjs");
    if sibling.exists() {
      return sibling;
    }
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("electron/main.cjs");
    if manifest.exists() {
      return manifest;
    }
    // Packaged layout: the scripts are copied next to the Electron binary.
    if let Some(binary) = option_env!("TAURI_RUNTIME_ELECTRON_ELECTRON_BINARY") {
      let sibling = PathBuf::from(binary).with_file_name("main.cjs");
      if sibling.exists() {
        return sibling;
      }
    }
    if let Ok(executable) = std::env::current_exe() {
      if let Some(dir) = executable.parent() {
        for candidate in [dir.join("main.cjs"), dir.join("electron").join("main.cjs")] {
          if candidate.exists() {
            return candidate;
          }
        }
      }
    }
    manifest
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

impl<T: UserEvent> RuntimeInitAttrs<T> for Electron {
  type Runtime = ElectronRuntime;

  fn apply_config(&mut self, config: &tauri_utils::config::Config) -> Result<()> {
    // Debug builds are what `tauri dev` drives (the same condition Tauri's own
    // `cfg(dev)` uses), so `tauri://localhost` must resolve to the dev server:
    // the embedded assets are not bundled in that mode.
    if self.dev_url.is_none() && cfg!(debug_assertions) {
      self.dev_url = config
        .build
        .dev_url
        .as_ref()
        .map(|url| url.as_str().trim_end_matches('/').to_string());
    }
    if self.identifier.is_none() {
      self.identifier = Some(config.identifier.clone());
    }
    if self.icon.is_some() {
      return Ok(());
    }
    // Prefer the `.ico` on Windows (crisp at every size), otherwise whatever the
    // bundle list points at first.
    let prefer_ico = cfg!(windows);
    let icon = config
      .bundle
      .icon
      .iter()
      .find(|icon| prefer_ico && icon.ends_with(".ico"))
      .or_else(|| config.bundle.icon.iter().find(|icon| icon.ends_with(".png")))
      .or_else(|| config.bundle.icon.first());
    if let Some(icon) = icon {
      // Keep it relative: it is relative to the app's `src-tauri`, not to this
      // crate, so `resolve_icon` searches upward from the working directory.
      self.icon = Some(PathBuf::from(icon));
    }
    Ok(())
  }
}

/// The app identifier comes from Tauri's `RuntimeInitArgs`, not from these
/// attributes, but Electron needs it on the command line before it starts: it
/// names the Chromium profile, so two Tauri apps sharing the cached executable
/// would otherwise share (and corrupt) one profile.
fn with_identifier(mut attrs: Electron, identifier: String) -> Electron {
  if attrs.identifier.is_none() && !identifier.is_empty() {
    attrs.identifier = Some(identifier);
  }
  attrs
}

impl ElectronRuntime {
  /// Processes the events Electron has sent since the last call. `run` loops
  /// over this forever; `run_iteration` calls it once and returns, which is what
  /// Tauri's polling drivers need.
  fn pump<T: UserEvent, F: FnMut(RunEvent<T>)>(&mut self, mut callback: &mut F, once: bool) {
    // Polling drivers call this repeatedly: messages Tauri sends must queue for
    // the next pump instead of running inline.
    self.is_running.store(true, Ordering::Relaxed);
    if !once {
      callback(RunEvent::Ready);
    }

    'event_loop: loop {
      // Events pushed by the Electron process.
      while let Ok(event) = self.event_rx.try_recv() {
        match event["event"].as_str() {
          #[cfg(windows)]
          Some("menu.command") => {
            let params = &event["params"];
            let Some(serial) = params["windowId"].as_u64().and_then(|id| u32::try_from(id).ok()) else { continue };
            let Some(command) = params["commandId"].as_u64().and_then(|id| u32::try_from(id).ok()) else { continue };
            let owner = self.context.windows.borrow().values()
              .find(|window| window.serial == serial)
              .and_then(|window| window.menu_owner.as_ref().map(|owner| owner.handle));
            if let Some(owner) = owner {
              if crate::window::imp::menu_snapshot(owner).is_ok_and(|menu| menu_command_enabled(&menu, command)) {
                crate::window::imp::dispatch_menu_command(owner, command);
              }
            }
          }
          Some("window.themeChanged") => {
            let params = &event["params"];
            let Some(serial) = params["windowId"].as_u64().and_then(|id| u32::try_from(id).ok()) else { continue };
            let Some(id) = self.context.window_for_serial(serial) else { continue };
            let theme = match params["theme"].as_str() {
              Some("dark") => Theme::Dark,
              Some("light") => Theme::Light,
              _ => continue,
            };
            #[cfg(windows)]
            if let Some(dark) = params["dark"].as_bool() {
              match self.context.native_handle(id).map_err(|error| error.to_string())
                .and_then(|handle| crate::window::imp::set_theme(handle, dark))
              {
                Ok(()) => {},
                Err(error) => eprintln!("[electron] native window theme failed: {error}"),
              }
            }
            let label = self.context.windows.borrow().get(&id).map(|window| window.label.clone());
            if let Some(label) = label {
              self.context.emit_window_event(id, &WindowEvent::ThemeChanged(theme));
              callback(RunEvent::WindowEvent { label, event: WindowEvent::ThemeChanged(theme) });
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
              break 'event_loop;
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
          Some(name @ ("webview.loadStarted" | "webview.loadFinished")) => {
            let params = &event["params"];
            let Some(serial) = params["webviewId"].as_u64() else {
              continue;
            };
            let Ok(url) = url::Url::parse(params["url"].as_str().unwrap_or_default()) else {
              continue;
            };
            let handler = self.context.webview_handlers.borrow()
              .get(&(serial as u32)).and_then(|handlers| handlers.page_load.clone());
            if let Some(handler) = handler
            {
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
            let handler = self.context.webview_handlers.borrow()
              .get(&(serial as u32)).and_then(|handlers| handlers.title.clone());
            if let Some(handler) = handler
            {
              handler(title);
            }
          }
          Some("webview.newWindow") => {
            let params = &event["params"];
            let Some(serial) = params["webviewId"].as_u64() else {
              continue;
            };
            let Ok(url) = url::Url::parse(params["url"].as_str().unwrap_or_default()) else {
              continue;
            };
            let features = params["features"].as_str().unwrap_or_default();
            // Electron always denies the popup: the answer may create a Tauri
            // window (whose URL is loaded into it), allow a bare popup, or deny.
            let handler = self.context.webview_handlers.borrow()
              .get(&(serial as u32)).and_then(|handlers| handlers.new_window.clone());
            let response = handler.map(|handler| handler(url.clone(), features));
            match response {
              Some(NewWindowResponse::Create { window_id }) => {
                let target = self.context.serial_for_window(window_id);
                match target {
                  Some(window_serial) => {
                    if let Err(error) = self.context.ipc.call(
                      "webview.navigate",
                      json!({ "webviewId": window_serial, "url": url.as_str() }),
                      IPC_TIMEOUT,
                    ) {
                      eprintln!("[electron] failed to open {url} in window: {error}");
                    }
                  }
                  None => eprintln!("[electron] new window requested for a closed window: {url}"),
                }
              }
              Some(NewWindowResponse::Allow) => {
                let (size, _position) = crate::popup::window_features(features);
                if let Err(error) = self.context.ipc.call(
                  "webview.openUrl",
                  json!({
                    "url": url.as_str(),
                    "width": size.map(|size| size.width),
                    "height": size.map(|size| size.height),
                  }),
                  IPC_TIMEOUT,
                ) {
                  eprintln!("[electron] failed to open popup {url}: {error}");
                }
              }
              Some(NewWindowResponse::Deny) | None => {
                eprintln!("[electron] denied new window request for {url}");
              }
            }
          }
          Some("webview.processGone") => {
            let params = &event["params"];
            let Some(serial) = params["webviewId"].as_u64() else {
              continue;
            };
            let termination = WebContentProcessTermination {
              reason: termination_reason(params["reason"].as_str().unwrap_or_default()),
              error_code: params["exitCode"].as_i64().map(|code| code as i32),
              error_string: params["reason"].as_str().map(str::to_string),
            };
            eprintln!(
              "[electron] webview {serial} process gone: {reason}",
              reason = termination.error_string.as_deref().unwrap_or("unknown")
            );
            let handler = self.context.webview_handlers.borrow()
              .get(&(serial as u32)).and_then(|handlers| handlers.process_gone.clone());
            if let Some(handler) = handler
            {
              handler(termination);
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
            if let Err(error) = self.context.ipc.call(
              "webview.downloadResponse",
              json!({ "id": id, "allow": allow, "destination": destination }),
              IPC_TIMEOUT,
            ) {
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
            let handler = self.context.policy_handlers.borrow()
              .get(&(serial as u32)).and_then(|handlers| handlers.permission.clone());
            let response = match (kind, handler) {
              (Some(kind), Some(handler)) => permission_response_name(handler(kind)),
              // Tauri's `Default` means the runtime default; Electron's own
              // default for an unhandled permission is to deny it.
              _ => "default",
            };
            if let Err(error) = self.context.ipc.call(
              "webview.permissionResponse",
              json!({ "id": id, "response": response }),
              IPC_TIMEOUT,
            ) {
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
            match Request::builder().uri(url).body(body) {
              Ok(request) => handler(request),
              Err(error) => eprintln!("[electron] invalid ipc request url: {error}"),
            }
          }
          Some("protocol.request") => {
            let params = &event["params"];
            let Some(id) = params["id"].as_u64() else { continue };
            let scheme = params["scheme"].as_str().unwrap_or_default().to_string();
            let selected = if scheme == "ipc" {
              params["webviewId"].as_u64().and_then(|serial| {
                self.context.ipc_protocol_handlers.borrow().get(&(serial as u32)).cloned()
              })
            } else {
              self.context.protocol_handlers.borrow().get(&scheme).cloned()
            };
            let Some((label, handler)) = selected else {
              let _ = self.context.ipc.call("protocol.respond", json!({ "id": id, "status": 403 }), IPC_TIMEOUT);
              continue;
            };
            let uri = params["uri"].as_str().unwrap_or_default().to_string();
            let body = params["body"]
              .as_str()
              .and_then(|body| base64::Engine::decode(&base64::engine::general_purpose::STANDARD, body).ok())
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
                let _ = self.context.ipc.call("protocol.respond", json!({ "id": id, "status": 400 }), IPC_TIMEOUT);
                continue;
              }
            };
            // Electron reports the request URL (e.g. `ipc://localhost`) as the
            // Origin header. Tauri validates that header against the page URL to
            // decide between local and remote contexts, so rewrite it to the
            // origin of the webview that issued the request.
            let origin = if scheme == "ipc" {
              params["pageUrl"].as_str().and_then(origin_of)
            } else {
              self.context.webview_origins.borrow().get(&label).cloned()
            };
            if let Some(origin) = origin {
              if let Ok(value) = http::HeaderValue::from_str(&origin) {
                request.headers_mut().insert(http::header::ORIGIN, value);
              }
            }
            let ipc = self.context.ipc.clone();
            let request_uri = uri.clone();
            let responder: Box<dyn FnOnce(Response<std::borrow::Cow<'static, [u8]>>) + Send> =
              Box::new(move |response: Response<std::borrow::Cow<'static, [u8]>>| {
                use base64::Engine;
                let headers: serde_json::Map<String, Value> = response
                  .headers()
                  .iter()
                  .filter_map(|(name, value)| {
                    value
                      .to_str()
                      .ok()
                      .map(|value| (name.to_string(), Value::String(value.to_string())))
                  })
                  .collect();
                let body = base64::engine::general_purpose::STANDARD.encode(response.body().as_ref());
                if let Err(error) = ipc.call(
                  "protocol.respond",
                  json!({
                    "id": id,
                    "status": response.status().as_u16(),
                    "headers": headers,
                    "body": body,
                  }),
                  IPC_TIMEOUT,
                ) {
                  eprintln!("[electron] failed to answer protocol request {request_uri}: {error}");
                }
              });
            handler(&label, request, responder);
          }
          Some("ipc.closed") => break 'event_loop,
          _ => {}
        }
      }

      // Messages sent by Tauri (window operations run on the main thread).
      while let Ok(message) = self.run_rx.try_recv() {
        match message {
          Message::Task(task) => task(),
          Message::CloseWindow(id) => {
            if close_window(&self.context, id, &mut callback) {
              break 'event_loop;
            }
          }
          Message::DestroyWindow(id) => {
            let removed = self.context.windows.borrow_mut().remove(&id).is_some();
            if removed && exit_if_empty(&self.context, &mut callback) {
              break 'event_loop;
            }
          }
          Message::UserEvent(event) => {
            if let Ok(event) = event.downcast::<T>() {
              callback(RunEvent::UserEvent(*event));
            }
          }
          Message::Exit => break 'event_loop,
        }
      }

      callback(RunEvent::MainEventsCleared);

      // Native UI (tray icons, menus) lives on this thread and posts window
      // messages here; Electron owns its own process, so nothing else pumps them.
      pump_thread_messages();
      #[cfg(windows)]
      {
        for (owner, method) in crate::window::imp::take_menu_actions() {
          let target = self.context.windows.borrow().iter().find_map(|(id, window)| {
            window.menu_owner.as_ref().filter(|menu| menu.handle == owner)
              .map(|_| (*id, window.serial))
          });
          let Some((id, serial)) = target else { continue };
          if method == "window.close" {
            if close_window(&self.context, id, &mut callback) {
              break 'event_loop;
            }
          } else if let Err(error) = self.context.ipc.call(method, json!({ "windowId": serial }), IPC_TIMEOUT) {
            eprintln!("[electron] native menu action failed: {error}");
          }
        }
        self.context.sync_window_menus();
      }

      if once {
        break;
      }

      std::thread::sleep(Duration::from_millis(10));
    }

    if once {
      return;
    }

    self.is_running.store(false, Ordering::Relaxed);
    eprintln!("[electron] run loop finished, shutting the runtime down");
    self.context.ipc.shutdown();
    callback(RunEvent::Exit);
  }

}

impl<T: UserEvent> Runtime<T> for ElectronRuntime {
  type WindowDispatcher = ElectronWindowDispatcher;
  type WebviewDispatcher = ElectronWebviewDispatcher;
  type Handle = ElectronRuntimeHandle;
  type EventLoopProxy = EventProxy;
  type RuntimeWebviewAttributes = ();
  type RuntimeInitAttrs = Electron;
  type WindowOpener = ();
  type Webview = ();

  fn new(args: RuntimeInitArgs<Electron>) -> Result<Self> {
    let identifier = args.identifier.clone();
    Self::start(with_identifier(args.runtime_init_attrs, identifier), &args.custom_schemes)
  }

  #[cfg(any(
    windows,
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd"
  ))]
  fn new_any_thread(args: RuntimeInitArgs<Electron>) -> Result<Self> {
    let identifier = args.identifier.clone();
    Self::start(with_identifier(args.runtime_init_attrs, identifier), &args.custom_schemes)
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
    let displays = self
      .context
      .ipc
      .call("window.displays", json!({}), IPC_TIMEOUT)
      .ok()?;
    platform::monitor_from_value(&displays["primary"])
  }

  fn monitor_from_point(&self, x: f64, y: f64) -> Option<Monitor> {
    let display = self
      .context
      .ipc
      .call("window.displayForPoint", json!({ "x": x, "y": y }), IPC_TIMEOUT)
      .ok()?;
    platform::monitor_from_value(&display)
  }

  fn available_monitors(&self) -> Vec<Monitor> {
    self
      .context
      .ipc
      .call("window.displays", json!({}), IPC_TIMEOUT)
      .ok()
      .and_then(|displays| {
        displays["displays"]
          .as_array()
          .map(|list| list.iter().filter_map(platform::monitor_from_value).collect())
      })
      .unwrap_or_default()
  }

  fn set_theme(&self, theme: Option<Theme>) {
    let theme = match theme {
      Some(Theme::Dark) => "dark",
      Some(_) => "light",
      None => "system",
    };
    if let Err(error) = self
      .context
      .ipc
      .call("app.theme", json!({ "theme": theme }), IPC_TIMEOUT)
    {
      eprintln!("[electron] set theme failed: {error}");
    }
  }

  // Electron exposes these application-level switches through `app`, not the
  // window: macOS activation policy, dock icon visibility, and app show/hide.
  #[cfg(target_os = "macos")]
  #[cfg_attr(docsrs, doc(cfg(target_os = "macos")))]
  fn set_activation_policy(&mut self, activation_policy: tauri_runtime::ActivationPolicy) {
    let policy = match activation_policy {
      tauri_runtime::ActivationPolicy::Regular => "regular",
      tauri_runtime::ActivationPolicy::Accessory => "accessory",
      tauri_runtime::ActivationPolicy::Prohibited => "prohibited",
      _ => "regular",
    };
    if let Err(error) = self.context.ipc.call(
      "app.activationPolicy",
      json!({ "policy": policy }),
      IPC_TIMEOUT,
    ) {
      eprintln!("[electron] set activation policy failed: {error}");
    }
  }

  #[cfg(target_os = "macos")]
  #[cfg_attr(docsrs, doc(cfg(target_os = "macos")))]
  fn set_dock_visibility(&mut self, visible: bool) {
    if let Err(error) = self.context.ipc.call(
      "app.dockVisibility",
      json!({ "visible": visible }),
      IPC_TIMEOUT,
    ) {
      eprintln!("[electron] set dock visibility failed: {error}");
    }
  }

  #[cfg(target_os = "macos")]
  #[cfg_attr(docsrs, doc(cfg(target_os = "macos")))]
  fn set_activate_ignoring_other_apps(&mut self, ignore: bool) {
    // There is no direct equivalent; focusing with `steal` is the closest.
    if let Err(error) = self
      .context
      .ipc
      .call("app.focus", json!({ "steal": ignore }), IPC_TIMEOUT)
    {
      eprintln!("[electron] focus app failed: {error}");
    }
  }

  #[cfg(target_os = "macos")]
  #[cfg_attr(docsrs, doc(cfg(target_os = "macos")))]
  fn show(&self) {
    if let Err(error) = self.context.ipc.call("app.show", json!({}), IPC_TIMEOUT) {
      eprintln!("[electron] show app failed: {error}");
    }
  }

  #[cfg(target_os = "macos")]
  #[cfg_attr(docsrs, doc(cfg(target_os = "macos")))]
  fn hide(&self) {
    if let Err(error) = self.context.ipc.call("app.hide", json!({}), IPC_TIMEOUT) {
      eprintln!("[electron] hide app failed: {error}");
    }
  }

  fn set_device_event_filter(&mut self, _filter: DeviceEventFilter) {
    // No Electron equivalent: Chromium owns the input pipeline and never hands
    // this runtime a device event stream to filter.
    crate::unsupported::report("Runtime::set_device_event_filter");
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
  fn run_iteration<F: FnMut(RunEvent<T>)>(&mut self, mut callback: F) {
    self.pump(&mut callback, true);
  }

  fn run_return<F: FnMut(RunEvent<T>) + 'static>(self, callback: F) -> i32 {
    self.run(callback);

    0
  }

  fn run<F: FnMut(RunEvent<T>) + 'static>(self, mut callback: F) {
    let mut runtime = self;
    runtime.pump(&mut callback, false);
  }

  fn cursor_position(&self) -> Result<PhysicalPosition<f64>> {
    let point = self
      .context
      .ipc
      .call("window.cursorPosition", json!({}), IPC_TIMEOUT)
      .map_err(create_window_err)?;
    Ok(PhysicalPosition::new(
      point["x"].as_f64().unwrap_or_default(),
      point["y"].as_f64().unwrap_or_default(),
    ))
  }
}

/// Name of the Electron executable on the host platform.
fn electron_executable() -> &'static str {
  if cfg!(windows) {
    "electron.exe"
  } else {
    "electron"
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
  if let Some(window) = context.windows.borrow_mut().remove(&id) {
    for webview in window.webviews {
      context.ipc_protocol_handlers.borrow_mut().remove(&webview.serial);
      context.ipc_handlers.borrow_mut().remove(&webview.serial);
      context.webview_handlers.borrow_mut().remove(&webview.serial);
      context.policy_handlers.borrow_mut().remove(&webview.serial);
      context.webview_origins.borrow_mut().remove(&webview.label);
    }
    context.native_handles.borrow_mut().remove(&window.serial);
  }
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
    if let Err(error) = context.ipc.call(
      "window.destroy",
      serde_json::json!({ "windowId": serial }),
      IPC_TIMEOUT,
    ) {
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
  let (tx, rx) = channel();
  callback(RunEvent::ExitRequested { code: None, tx });
  !matches!(rx.try_recv(), Ok(ExitRequestedEventAction::Prevent))
}

#[cfg(any(windows, target_os = "macos"))]
fn run_window_creation_callback(callback: impl FnOnce(RawWindow<'_>), _handle: isize) {
  callback(RawWindow {
    #[cfg(windows)]
    hwnd: _handle,
    _marker: &std::marker::PhantomData,
  });
}

pub(crate) fn create_window_impl<T: UserEvent, F: Fn(RawWindow<'_>) + Send + 'static>(
  context: &RuntimeContext,
  pending: PendingWindow<T, ElectronRuntime>,
  after_window_creation: Option<F>,
) -> Result<DetachedWindow<T, ElectronRuntime>> {
  if std::thread::current().id() != context.main_thread_id {
    return context.on_main_thread(move |context| create_window_impl(context, pending, after_window_creation))?;
  }
  let PendingWindow {
    label,
    window_builder,
    mut webview,
  } = pending;

  let id = context.next_window_id();
  let serial = context.next_serial();
  let webview_id = webview.as_ref().map(|_| context.next_webview_id());

  if let Some(webview) = webview.as_mut() {
    for (scheme, handler) in webview.uri_scheme_protocols.drain() {
      let handler: Rc<UriSchemeProtocolHandler> = Rc::from(handler);
      if scheme == "ipc" {
        context.ipc_protocol_handlers.borrow_mut().insert(
          webview_id.expect("a pending webview has a serial"), (webview.label.clone(), handler.clone()),
        );
      }
      // Asset protocols are session-wide; commands use an authenticated
      // per-webview handler instead of inheriting the first window's identity.
      context
        .protocol_handlers
        .borrow_mut()
        .entry(scheme.clone())
        .or_insert_with(|| (webview.label.clone(), handler));
      if let Err(error) = context.ipc.call(
        "protocol.register",
        json!({ "scheme": scheme }),
        IPC_TIMEOUT,
      ) {
        eprintln!("[electron] failed to register protocol {scheme}: {error}");
      }
    }
  }

  // Handlers Electron must ask about later; announced in the create payload so
  // the main process knows whether to intercept downloads and permissions.
  let policy = match (webview.as_mut(), webview_id) {
    (Some(pending_webview), Some(webview_serial)) => {
      let (download, permission) =
        register_policy_handlers(context, webview_serial, pending_webview);
      json!({ "download": download, "permission": permission })
    }
    _ => json!(null),
  };

  let mut payload = json!({ "windowId": serial, "label": label });
  // The whole window configuration travels to the main process; `main.cjs`
  // applies what Electron can express and reports the rest.
  payload["window"] =
    serde_json::to_value(&window_builder.config).map_err(|error| create_window_err(error.to_string()))?;
  if let Some(icon) = &window_builder.icon {
    use base64::Engine as _;
    // Electron builds an image from raw BGRA pixels, so swap the channels here
    // instead of shipping a PNG encoder.
    let mut bgra = icon.rgba.clone();
    for pixel in bgra.chunks_exact_mut(4) {
      pixel.swap(0, 2);
    }
    payload["windowIcon"] = json!({
      "bgra": base64::engine::general_purpose::STANDARD.encode(bgra),
      "width": icon.width,
      "height": icon.height,
    });
  }
  if let Some(icon) = &context.icon {
    payload["icon"] = json!(icon.display().to_string());
  }
  if let (Some(webview), Some(webview_id)) = (&webview, webview_id) {
    payload["webview"] = json!({
      "webviewId": webview_id,
      "url": context.resolve_url(&webview.webview_attributes.url),
      "initScripts": initialization_scripts(webview)?,
      "policy": policy,
    });
  }

  if !window_builder.unsupported.is_empty() {
    eprintln!(
      "[electron] window {label}: ignoring unsupported builder options: {}",
      window_builder.unsupported.join(", ")
    );
  }

  context
    .ipc
    .call("window.create", payload, IPC_TIMEOUT)
    .map_err(create_window_err)?;
  eprintln!("[electron] created window {label} (serial {serial})");

  context.windows.borrow_mut().insert(
    id,
    Window {
      label: label.clone(),
      serial,
      webviews: webview_id
        .map(|serial| Webview {
          serial,
          label: label.clone(),
          handlers: Vec::new(),
        })
        .into_iter()
        .collect(),
      handlers: Vec::new(),
      #[cfg(windows)]
      menu_owner: None,
    },
  );

  // Options Electron has no API for, applied to the native window handle.
  #[cfg(windows)]
  if window_builder.owner.is_some()
    || window_builder.parent.is_some()
    || window_builder.config.always_on_bottom
  {
    let handle = context.native_handle(id)?;
    if window_builder.config.always_on_bottom {
      crate::window::imp::send_to_bottom(handle);
    }
    if let Some(owner) = window_builder.owner.or(window_builder.parent) {
      // Tauri resolves parent/owner through window_handle(), so translate our
      // per-window menu owner back to the actual Electron window.
      let parent_id = context.windows.borrow().iter().find_map(|(id, window)| {
        window.menu_owner.as_ref().filter(|menu| menu.handle == owner).map(|_| *id)
      });
      let owner = match parent_id {
        Some(id) => context.native_handle(id)?,
        None => owner,
      };
      crate::window::imp::set_owner(handle, owner);
    }
  }

  // Tauri hands over a raw NSWindow pointer; Electron only understands its own
  // windows, so map the pointer back to the window it belongs to.
  #[cfg(target_os = "macos")]
  if let Some(parent) = window_builder.macos_parent {
    let parent_serial = context
      .native_handles
      .borrow()
      .iter()
      .find(|(_, handle)| **handle == parent)
      .map(|(serial, _)| *serial);
    match parent_serial {
      Some(parent_serial) => {
        if let Err(error) = context.ipc.call(
          "window.setParentWindow",
          json!({ "windowId": serial, "parentId": parent_serial }),
          IPC_TIMEOUT,
        ) {
          eprintln!("[electron] window {label}: parenting failed: {error}");
        }
      }
      None => crate::unsupported::report("WindowBuilder::parent (unknown window)"),
    }
  }

  if let (Some(pending_webview), Some(webview_serial)) = (webview.as_mut(), webview_id) {
    let url = context.resolve_url(&pending_webview.webview_attributes.url);
    if let Some(origin) = origin_of(&url) {
      context
        .webview_origins
        .borrow_mut()
        .insert(label.clone(), origin);
    }
    if let Some(handler) = pending_webview.ipc_handler.take() {
      register_ipc_handler::<T>(
        context,
        webview_serial,
        label.clone(),
        url,
        handler,
      );
    }
    register_webview_handlers::<T>(context, webview_serial, pending_webview);
  }

  if let Some(callback) = after_window_creation {
    #[cfg(any(windows, target_os = "macos"))]
    {
      let context = context.clone();
      let (tx, rx) = sync_channel(1);
      #[cfg(windows)]
      let task_context = context.clone();
      let task = Message::Task(Box::new(move || {
        let result: Result<()> = (|| {
          #[cfg(windows)]
          let handle = task_context.menu_owner_window(id)?;
          #[cfg(target_os = "macos")]
          let handle = 0;
          run_window_creation_callback(callback, handle);
          Ok(())
        })();
        let _ = tx.send(result);
      }));
      if std::thread::current().id() == context.main_thread_id {
        context.send_message(task)?;
      } else {
        // send_message runs tasks inline before the loop starts, which would
        // create muda's owner on a worker. Always queue this native callback.
        context.run_tx.send(task).map_err(|_| Error::FailedToSendMessage)?;
      }
      rx.recv().map_err(|_| Error::FailedToReceiveMessage)??;
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
      // RawWindow requires a valid GtkApplicationWindow. Electron has none;
      // never hand a GTK consumer a null or a foreign-process pointer.
      let _ = callback;
      crate::unsupported::report("after_window_creation (Electron has no GTK window)");
    }
  }

  let webview = webview_id.map(|webview_id| DetachedWindowWebview {
    webview: DetachedWebview {
      label: label.clone(),
      dispatcher: ElectronWebviewDispatcher {
        id: webview_id,
        context: context.clone(),
        url: Arc::new(Mutex::new(
          webview
            .as_ref()
            .map(|webview| context.resolve_url(&webview.webview_attributes.url))
            .unwrap_or_default(),
        )),
        last_evaluated_script: Default::default(),
      },
    },
    use_https_scheme: false,
    devtools: webview
      .as_ref()
      .and_then(|webview| webview.webview_attributes.devtools),
  });

  Ok(DetachedWindow {
    id,
    label,
    dispatcher: ElectronWindowDispatcher {
      id,
      context: context.clone(),
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
    return context.on_main_thread(move |context| create_webview_impl(context, window_id, pending))?;
  }
  let serial = context
    .serial_for_window(window_id)
    .ok_or_else(|| create_webview_err("window does not exist".into()))?;
  let webview_id = context.next_webview_id();

  let url = context.resolve_url(&pending.webview_attributes.url);
  if let Some(origin) = origin_of(&url) {
    context
      .webview_origins
      .borrow_mut()
      .insert(pending.label.clone(), origin);
  }

  // Schemes must be installed before the page is loaded, otherwise the initial
  // `tauri://` navigation fails with ERR_FAILED.
  for (scheme, handler) in pending.uri_scheme_protocols.drain() {
    let handler: Rc<UriSchemeProtocolHandler> = Rc::from(handler);
    if scheme == "ipc" {
      context.ipc_protocol_handlers.borrow_mut().insert(webview_id, (pending.label.clone(), handler.clone()));
    }
    context
      .protocol_handlers
      .borrow_mut()
      .entry(scheme.clone())
      .or_insert_with(|| (pending.label.clone(), handler));
    if let Err(error) = context.ipc.call(
      "protocol.register",
      json!({ "scheme": scheme }),
      IPC_TIMEOUT,
    ) {
      eprintln!("[electron] failed to register protocol {scheme}: {error}");
    }
  }

  let (download, permission) = register_policy_handlers(context, webview_id, &mut pending);
  let payload = json!({
    "windowId": serial,
    "webviewId": webview_id,
    "url": url,
    "initScripts": initialization_scripts(&pending)?,
    "bounds": pending.webview_attributes.bounds.map(|bounds| json!({
      "position": position_payload(bounds.position),
      "size": size_payload(bounds.size),
    })),
    "autoResize": pending.webview_attributes.auto_resize,
    "policy": { "download": download, "permission": permission },
  });
  context
    .ipc
    .call("webview.create", payload, IPC_TIMEOUT)
    .map_err(create_webview_err)?;

  if let Some(window) = context.windows.borrow_mut().get_mut(&window_id) {
    window.webviews.push(Webview {
      serial: webview_id,
      label: pending.label.clone(),
      handlers: Vec::new(),
    });
  }

  if let Some(handler) = pending.ipc_handler.take() {
    register_ipc_handler::<T>(
      context,
      webview_id,
      pending.label.clone(),
      url.clone(),
      handler,
    );
  }
  register_webview_handlers::<T>(context, webview_id, &mut pending);

  Ok(DetachedWebview {
    label: pending.label,
    dispatcher: ElectronWebviewDispatcher {
      id: webview_id,
      context: context.clone(),
      url: Arc::new(Mutex::new(
        context.resolve_url(&pending.webview_attributes.url),
      )),
      last_evaluated_script: Default::default(),
    },
  })
}

fn valid_scheme(scheme: &str) -> bool {
  let mut bytes = scheme.bytes();
  bytes.next().is_some_and(|byte| byte.is_ascii_alphabetic())
    && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'.'))
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
  url: String,
  handler: WebviewIpcHandler<T, ElectronRuntime>,
) {
  let detached = DetachedWebview::<T, ElectronRuntime> {
    label,
    dispatcher: ElectronWebviewDispatcher {
      id: webview_id,
      context: context.clone(),
      url: Arc::new(Mutex::new(url)),
      last_evaluated_script: Default::default(),
    },
  };
  let handler: Rc<dyn Fn(Request<String>) + Send> =
    Rc::new(move |request| handler(detached.clone(), request));
  context.ipc_handlers.borrow_mut().insert(webview_id, handler);
}

fn initialization_scripts<T: UserEvent, R: Runtime<T>>(
  pending: &PendingWebview<T, R>,
) -> Result<Vec<String>> {
  pending
    .webview_attributes
    .initialization_scripts
    .iter()
    .map(|script| native_ipc_script(&script.script))
    .collect()
}

fn native_ipc_script(script: &str) -> Result<String> {
  // A protocol Request has no trusted sender. Keep Tauri's typed protocol,
  // invoke key and isolation encryption, but transport it through native IPC.
  if script.contains("const __TAURI_INVOKE_KEY__ =")
    && script.contains("const fetchChannelDataCommand =")
    && script.contains("Object.defineProperty(window.__TAURI_INTERNALS__, 'postMessage'")
  {
    let marker = "let customProtocolIpcFailed = false";
    if script.matches(marker).count() != 1 {
      return Err(create_webview_err("Tauri IPC transport changed; native sender attribution requires an update".into()));
    }
    Ok(script.replacen(marker, NATIVE_IPC_FETCH, 1))
  } else {
    Ok(script.to_owned())
  }
}

const NATIVE_IPC_FETCH: &str = r#"let customProtocolIpcFailed = false
  const fetch = async (url, options) => {
    const body = Array.isArray(options.body) ? new Uint8Array(options.body) : options.body
    const reply = await window.ipc.fetch({
      url, method: options.method,
      headers: [...options.headers],
      body: new Uint8Array(await new Response(body).arrayBuffer())
    })
    const bytes = Uint8Array.from(atob(reply.body ?? ''), c => c.charCodeAt(0))
    return new Response(bytes, { status: reply.status, headers: reply.headers })
  }"#;

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

/// Tauri's webview-scoped callbacks, stored so the run loop can feed them the
/// events Electron forwards.
pub(crate) struct WebviewHandlers {
  pub(crate) page_load: Option<Rc<dyn Fn(url::Url, PageLoadEvent) + Send>>,
  pub(crate) title: Option<Rc<dyn Fn(String) + Send>>,
  pub(crate) new_window:
    Option<Rc<dyn Fn(url::Url, &str) -> NewWindowResponse + Send + Sync>>,
  pub(crate) process_gone: Option<Rc<dyn Fn(WebContentProcessTermination) + Send>>,
}

/// The callbacks Electron asks about out of band: a download question and
/// permission requests. Both are answered over IPC instead of blocking.
#[derive(Default)]
pub(crate) struct PolicyHandlers {
  download: Option<Arc<DownloadHandler>>,
  permission: Option<Arc<dyn Fn(PermissionKind) -> PermissionResponse + Send + Sync>>,
}

impl PolicyHandlers {
  fn is_empty(&self) -> bool {
    self.download.is_none() && self.permission.is_none()
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

fn permission_response_name(response: PermissionResponse) -> &'static str {
  match response {
    PermissionResponse::Allow => "allow",
    PermissionResponse::Deny => "deny",
    _ => "default",
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
  let process_gone = pending
    .on_web_content_process_terminate_handler
    .take()
    .map(|handler| {
      Rc::new(move |termination: WebContentProcessTermination| handler(termination))
        as Rc<dyn Fn(WebContentProcessTermination) + Send>
    });
  let new_window = pending.new_window_handler.take().map(|handler| {
    Rc::new(move |url: url::Url, features: &str| {
      let (size, position) = crate::popup::window_features(features);
      handler(
        url,
        NewWindowFeatures::<T, ElectronRuntime>::new(size, position, ()),
      )
    }) as Rc<dyn Fn(url::Url, &str) -> NewWindowResponse + Send + Sync>
  });
  context.webview_handlers.borrow_mut().insert(
    webview_id,
    WebviewHandlers {
      page_load,
      title,
      new_window,
      process_gone,
    },
  );
}

/// Moves the download and permission callbacks off the attributes and records
/// which of them Electron should ask about. Returns `(download, permission)`.
fn register_policy_handlers<T: UserEvent>(
  context: &RuntimeContext,
  webview_id: u32,
  pending: &mut PendingWebview<T, ElectronRuntime>,
) -> (bool, bool) {
  let handlers = PolicyHandlers {
    download: pending.download_handler.take(),
    permission: pending.permission_request_handler.take().map(Arc::from),
  };
  let wired = (handlers.download.is_some(), handlers.permission.is_some());
  if !handlers.is_empty() {
    context
      .policy_handlers
      .borrow_mut()
      .insert(webview_id, handlers);
  }
  wired
}

/// Maps Electron's `render-process-gone` reason onto Tauri's classification.
fn termination_reason(reason: &str) -> WebContentProcessTerminationReason {
  match reason {
    "clean-exit" => WebContentProcessTerminationReason::Normal,
    "abnormal-exit" => WebContentProcessTerminationReason::Abnormal,
    "killed" => WebContentProcessTerminationReason::Killed,
    "crashed" => WebContentProcessTerminationReason::Crashed,
    "oom" => WebContentProcessTerminationReason::OutOfMemory,
    "launch-failed" => WebContentProcessTerminationReason::LaunchFailed,
    "integrity-failure" => WebContentProcessTerminationReason::IntegrityFailure,
    _ => WebContentProcessTerminationReason::Unknown,
  }
}

/// Reject stale, disabled, hidden, separator and submenu commands, including
/// children of a disabled submenu. WM_COMMAND carries only a 16-bit item ID.
#[cfg(any(windows, test))]
fn menu_command_enabled(menu: &Value, command: u32) -> bool {
  command <= u16::MAX as u32 && menu.as_array().is_some_and(|items| items.iter().any(|item| {
    item["enabled"].as_bool() == Some(true) && item["type"] != "separator" &&
      if item["submenu"].is_array() {
        menu_command_enabled(&item["submenu"], command)
      } else {
        item["commandId"].as_u64() == Some(command as u64)
      }
  }))
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn worker_state_access_and_reentrant_handlers_stay_on_the_ui_thread() {
    let (_events, event_rx) = channel();
    let runtime = ElectronRuntime::init(Ipc::test_connection(), event_rx, None, None);
    let context = runtime.context.clone();
    let id = context.next_window_id();
    context.windows.borrow_mut().insert(id, Window {
      label: "worker".into(), serial: 42,
      webviews: vec![Webview { serial: 7, label: "guest".into(), handlers: vec![] }],
      handlers: vec![],
      #[cfg(windows)]
      menu_owner: None,
    });
    let main_thread = std::thread::current().id();
    assert_eq!(context.on_main_thread(|_| std::thread::current().id()).unwrap(), main_thread);
    let worker = std::thread::spawn(move || {
      assert_eq!(context.serial_for_window(id), Some(42));
      let events = context.clone();
      context.add_window_event_handler(id, 1, move |_| {
        assert_eq!(std::thread::current().id(), main_thread);
        events.on_main_thread(|context| context.windows.borrow_mut().clear()).unwrap();
      });
      context.add_webview_event_handler(7, 2, |_| {});
      context.set_webview_origin_by_serial(7, "https://example.com/path");
      context.send_message(Message::Task(Box::new(move || {
        assert_eq!(std::thread::current().id(), main_thread);
      }))).unwrap();
      context.on_main_thread(move |context| {
        assert_eq!(std::thread::current().id(), main_thread);
        assert_eq!(context.webview_origins.borrow().get("guest").map(String::as_str), Some("https://example.com"));
        assert_eq!(context.windows.borrow()[&id].webviews[0].handlers.len(), 1);
        context.emit_window_event(id, &WindowEvent::Destroyed);
        assert!(context.windows.borrow().is_empty());
      }).unwrap();
    });
    // A worker started during setup must queue tasks even before run() starts.
    while !worker.is_finished() {
      match runtime.run_rx.recv_timeout(Duration::from_millis(100)) {
        Ok(Message::Task(task)) => task(),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {},
        _ => panic!("unexpected runtime task"),
      }
    }
    worker.join().unwrap();
  }

  #[test]
  fn validates_custom_scheme_names() {
    for scheme in ["stream", "test+v1", "a-b.c", "UPPER", "x1"] {
      assert!(valid_scheme(scheme), "{scheme:?}");
    }
    for scheme in ["", "1stream", "stream,ipc", "a b", "https://", "a\n", "中文"] {
      assert!(!valid_scheme(scheme), "{scheme:?}");
    }
  }

  #[test]
  fn selects_native_ipc_without_changing_user_scripts() {
    let source = "const __TAURI_INVOKE_KEY__ = 'key';\nconst fetchChannelDataCommand = 'channel';\nlet customProtocolIpcFailed = false;\nObject.defineProperty(window.__TAURI_INTERNALS__, 'postMessage', {});";
    let native = native_ipc_script(source).unwrap();
    assert_eq!(native, source.replace("let customProtocolIpcFailed = false", NATIVE_IPC_FETCH));
    assert!(native.contains("const fetch = async"));
    assert!(native.contains("let customProtocolIpcFailed = false"));
    assert!(native_ipc_script(&source.replace("let customProtocolIpcFailed = false", "let changed = false")).is_err());
    let user_script = "let customProtocolIpcFailed = false";
    assert_eq!(native_ipc_script(user_script).unwrap(), user_script);
  }

  #[cfg(windows)]
  #[test]
  fn window_creation_callback_initializes_the_menu_owner_once() {
    use std::cell::Cell;
    use windows::Win32::{Foundation::HWND, UI::WindowsAndMessaging::{
      AppendMenuW, CreateMenu, DestroyMenu, DestroyWindow, MF_STRING, SetMenu,
    }};
    let owner = crate::window::imp::MenuOwner::new().unwrap();
    let menu = unsafe { CreateMenu() }.unwrap();
    unsafe { AppendMenuW(menu, MF_STRING, 1001, windows::core::w!("Builder menu")) }.unwrap();
    let calls = Cell::new(0);
    let thread = std::thread::current().id();
    run_window_creation_callback(|raw| {
      calls.set(calls.get() + 1);
      assert_eq!(std::thread::current().id(), thread);
      assert_eq!(raw.hwnd, owner.handle);
      unsafe { SetMenu(HWND(raw.hwnd as *mut _), Some(menu)) }.unwrap();
    }, owner.handle);
    assert_eq!(calls.get(), 1);
    let snapshot = crate::window::imp::menu_snapshot(owner.handle).unwrap();
    assert_eq!(snapshot[0]["label"], "Builder menu");
    assert_eq!(snapshot[0]["commandId"], 1001);
    // This test owns the menu and has no muda association, so it can clean up.
    unsafe {
      SetMenu(HWND(owner.handle as *mut _), None).unwrap();
      DestroyMenu(menu).unwrap();
      DestroyWindow(HWND(owner.handle as *mut _)).unwrap();
    }
  }

  #[test]
  fn accepts_only_live_enabled_menu_commands() {
    let mut menu = json!([{ "commandId": 900, "enabled": true, "submenu": [
      { "commandId": 1001, "enabled": true, "checked": false },
      { "commandId": 1002, "enabled": false },
      { "type": "separator", "commandId": 1003, "enabled": true }
    ] }]);
    assert!(menu_command_enabled(&menu, 1001));
    for command in [900, 1002, 1003, 9999, 65536] {
      assert!(!menu_command_enabled(&menu, command));
    }
    menu[0]["enabled"] = json!(false);
    assert!(!menu_command_enabled(&menu, 1001));
    assert!(!menu_command_enabled(&Value::Null, 1001));
    menu[0]["enabled"] = json!(true);
    menu[0]["submenu"] = json!([]);
    assert!(!menu_command_enabled(&menu, 1001));
  }

  #[test]
  fn maps_electron_termination_reasons() {
    assert_eq!(termination_reason("crashed"), WebContentProcessTerminationReason::Crashed);
    assert_eq!(termination_reason("oom"), WebContentProcessTerminationReason::OutOfMemory);
    assert_eq!(termination_reason("clean-exit"), WebContentProcessTerminationReason::Normal);
    assert_eq!(termination_reason("nonsense"), WebContentProcessTerminationReason::Unknown);
  }

  #[test]
  fn maps_permission_names_both_ways() {
    assert_eq!(permission_kind("camera"), Some(PermissionKind::Camera));
    assert_eq!(permission_kind("clipboard-read"), Some(PermissionKind::ClipboardRead));
    assert_eq!(permission_kind("openExternal"), None);
    assert_eq!(permission_response_name(PermissionResponse::Allow), "allow");
    assert_eq!(permission_response_name(PermissionResponse::Deny), "deny");
    assert_eq!(permission_response_name(PermissionResponse::Default), "default");
  }
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
