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
  RuntimeInitArgs, UserEvent, WindowEventId,
  dpi::{PhysicalPosition, PhysicalSize, Position, Size},
  monitor::Monitor,
  webview::{
    DetachedWebview, DownloadEvent,
    PageLoadEvent, PendingWebview, PermissionKind, PermissionResponse, WebviewIpcHandler,
  },
  window::{
    DetachedWindow, DetachedWindowWebview, PendingWindow, RawWindow, WindowEvent, WindowId,
  },
};
use tauri_utils::Theme;

/// The `tauri-runtime` handler aliases are private in the 2.x line, so these
/// restate the same signatures locally.
type DownloadHandler = dyn Fn(DownloadEvent<'_>) -> bool + Send + Sync;
type UriSchemeProtocolHandler = dyn Fn(
    &str,
    http::Request<Vec<u8>>,
    Box<dyn FnOnce(http::Response<std::borrow::Cow<'static, [u8]>>) + Send>,
  ) + Send
  + Sync;

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
/// Tauri 2 does not hand runtimes the configured schemes, and Electron can
/// only register privileged schemes before `ready`, so the well-known ones are
/// fixed. Custom schemes are still registered per webview below.
pub(crate) const DEFAULT_SCHEMES: &[&str] = &["ipc", "tauri", "asset"];

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
  ipc_handlers: Rc<RefCell<HashMap<u32, Rc<dyn Fn(Request<String>) + Send>>>>,
  /// Tauri's page-load, title and new-window callbacks, by webview serial.
  webview_handlers: Rc<RefCell<HashMap<u32, WebviewHandlers>>>,
  /// Custom protocols registered by Tauri, served through Electron's
  /// `protocol.handle`.
  protocol_handlers: Rc<RefCell<HashMap<String, (String, Rc<UriSchemeProtocolHandler>)>>>,
  /// Page origin per webview label, injected as the `Origin` header on
  /// custom-protocol requests (Electron reports the request URL instead).
  webview_origins: Rc<RefCell<HashMap<String, String>>>,
  /// Download and permission callbacks, by webview serial. Electron asks for
  /// these asynchronously, so the run loop answers over IPC.
  policy_handlers: Rc<RefCell<HashMap<u32, PolicyHandlers>>>,
  /// Native window handle (HWND / NSView / X11 window) per window serial, asked
  /// from Electron once. `window_handle()` hands it to Tauri so host-side code
  /// (muda menus, window effects, Win32 tweaks) works as it does on wry.
  native_handles: Rc<RefCell<HashMap<u32, isize>>>,
}

// SAFETY: we ensure this type is only used on the main thread.
#[allow(clippy::non_send_fields_in_send_ty)]
unsafe impl Send for RuntimeContext {}

// SAFETY: we ensure this type is only used on the main thread.
#[allow(clippy::non_send_fields_in_send_ty)]
unsafe impl Sync for RuntimeContext {}

impl RuntimeContext {
  pub(crate) fn send_message(&self, message: Message) -> Result<()> {
    // Tasks run inline on the main thread; everything else goes to the loop.
    let message = match (std::thread::current().id() == self.main_thread_id, message) {
      (true, Message::Task(task)) => {
        task();
        return Ok(());
      }
      (_, message) => message,
    };
    if self.is_running.load(Ordering::Relaxed) {
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
    self.windows.borrow().get(&id).map(|window| window.serial)
  }

  /// The native handle of a window, as Electron knows it. Cached because Tauri
  /// asks for it on every menu/effect/decoration call.
  pub(crate) fn native_handle(&self, id: WindowId) -> Result<isize> {
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
    handler: Rc<dyn Fn(&WindowEvent) + Send>,
  ) {
    if let Some(window) = self.windows.borrow_mut().get_mut(&window_id) {
      window.handlers.push((event_id, handler));
    }
  }

  pub(crate) fn add_webview_event_handler(
    &self,
    serial: u32,
    event_id: tauri_runtime::WebviewEventId,
    handler: Rc<dyn Fn(&tauri_runtime::window::WebviewEvent) + Send>,
  ) {
    for window in self.windows.borrow_mut().values_mut() {
      if let Some(webview) = window
        .webviews
        .iter_mut()
        .find(|webview| webview.serial == serial)
      {
        webview.handlers.push((event_id, handler));
        return;
      }
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

  fn start(attrs: Electron, custom_schemes: &[&str]) -> Result<Self> {
    crate::platform::dpi::enable_per_monitor_v2();
    let binary = attrs.resolve_binary();
    // Task Manager and Explorer name and icon a process after its executable,
    // so the Electron binary borrows the application's resources.
    if let Ok(app_exe) = std::env::current_exe() {
      crate::branding::apply(&binary, &app_exe);
    }
    let main_script = attrs.resolve_main_script(&binary);
    if !main_script.exists() {
      return Err(create_window_err(format!(
        "Electron main script not found at {}; configure it with Electron::default().main_script(..)",
        main_script.display()
      )));
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
    for scheme in custom_schemes {
      if !schemes.iter().any(|known| known == scheme) {
        schemes.push((*scheme).to_string());
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
    .map_err(create_window_err)?;
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

  /// Reads the knobs the build supplies as environment variables.
  ///
  /// Tauri 2 has no runtime init attributes, so this is how a v2 app configures
  /// the runtime without editing its own code: `TAURI_DEV_URL`,
  /// `TAURI_RUNTIME_ELECTRON_ICON` and `TAURI_RUNTIME_ELECTRON_IDENTIFIER`.
  pub fn from_env() -> Self {
    let mut electron = Self::default();
    if let Ok(dev_url) = std::env::var("TAURI_DEV_URL") {
      electron.dev_url = Some(dev_url.trim_end_matches('/').to_string());
    }
    if let Some(icon) = std::env::var_os("TAURI_RUNTIME_ELECTRON_ICON") {
      electron.icon = Some(PathBuf::from(icon));
    }
    if let Ok(identifier) = std::env::var("TAURI_RUNTIME_ELECTRON_IDENTIFIER") {
      electron.identifier = Some(identifier);
    }
    electron
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

impl<T: UserEvent> Runtime<T> for ElectronRuntime {
  type WindowDispatcher = ElectronWindowDispatcher;
  type WebviewDispatcher = ElectronWebviewDispatcher;
  type Handle = ElectronRuntimeHandle;
  type EventLoopProxy = EventProxy;
  
  fn new(_args: RuntimeInitArgs) -> Result<Self> {
    Self::start(Electron::from_env(), &DEFAULT_SCHEMES)
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
    Self::start(Electron::from_env(), &DEFAULT_SCHEMES)
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
    _after_window_creation: Option<F>,
  ) -> Result<DetachedWindow<T, Self>> {
    create_window_impl(&self.context, pending)
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
  // Electron pumps its own event loop in the main process; there is nothing to
  // iterate here, and callers invoke this in a loop so it must stay silent.
  fn run_iteration<F: FnMut(RunEvent<T>)>(&mut self, _callback: F) {}

  fn run_return<F: FnMut(RunEvent<T>) + 'static>(self, callback: F) -> i32 {
    self.run(callback);

    0
  }

  fn run<F: FnMut(RunEvent<T>) + 'static>(self, mut callback: F) {
    self.is_running.store(true, Ordering::Relaxed);
    callback(RunEvent::Ready);

    'event_loop: loop {
      // Events pushed by the Electron process.
      while let Ok(event) = self.event_rx.try_recv() {
        match event["event"].as_str() {
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
            let handlers = self.context.webview_handlers.borrow();
            if let Some(handler) = handlers
              .get(&(serial as u32))
              .and_then(|handlers| handlers.page_load.as_ref())
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
            let handlers = self.context.webview_handlers.borrow();
            if let Some(handler) = handlers
              .get(&(serial as u32))
              .and_then(|handlers| handlers.title.as_ref())
            {
              handler(title);
            }
          }
          Some("webview.newWindow") => {
            // Tauri 2's new-window features are WebView2/webkit handles that do
            // not exist here, so the request is reported and Electron keeps its
            // deny answer.
            let serial = event["params"]["webviewId"].as_u64().unwrap_or_default();
            eprintln!(
              "[electron] denied new window request for {url} (webview {serial})",
              url = event["params"]["url"].as_str().unwrap_or("")
            );
          }
          Some("webview.processGone") => {
            // Tauri 2 only exposes process termination on macOS/iOS, so this is
            // reported on the console instead of through a handler.
            let serial = event["params"]["webviewId"].as_u64().unwrap_or_default();
            eprintln!(
              "[electron] webview {serial} process gone: {}",
              event["params"]["reason"].as_str().unwrap_or("unknown")
            );
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
                _ => "default",
              }
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
            let Some((label, handler)) = self
              .context
              .protocol_handlers
              .borrow()
              .get(&scheme)
              .cloned()
            else {
              eprintln!("[electron] no protocol handler for {scheme}");
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
                continue;
              }
            };
            // Electron reports the request URL (e.g. `ipc://localhost`) as the
            // Origin header. Tauri validates that header against the page URL to
            // decide between local and remote contexts, so rewrite it to the
            // origin of the webview that issued the request.
            let origin = self
              .context
              .webview_origins
              .borrow()
              .get(&label)
              .cloned();
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

      std::thread::sleep(Duration::from_millis(10));
    }

    self.is_running.store(false, Ordering::Relaxed);
    eprintln!("[electron] run loop finished, shutting the runtime down");
    // Tauri's exit cleanup still calls into the windows (hide/close), so it has
    // to run while Electron is alive; only then can the channel be torn down.
    callback(RunEvent::Exit);
    self.context.ipc.shutdown();
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
  context.windows.borrow_mut().remove(&id);
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

pub(crate) fn create_window_impl<T: UserEvent>(
  context: &RuntimeContext,
  pending: PendingWindow<T, ElectronRuntime>,
) -> Result<DetachedWindow<T, ElectronRuntime>> {
  let PendingWindow {
    label,
    window_builder,
    mut webview,
  } = pending;

  let id = context.next_window_id();
  let serial = context.next_serial();
  let webview_id = webview.as_ref().map(|_| context.next_webview_id());
  let webview_url = webview
    .as_ref()
    .map(|webview| context.resolve_url(&webview.url))
    .unwrap_or_default();

  if let Some(webview) = webview.as_mut() {
    for (scheme, handler) in webview.uri_scheme_protocols.drain() {
      // Scheme handlers answer for the whole session, and Electron's protocol
      // API cannot tell us which webview sent a request, so the first webview
      // (the window's page) keeps ownership of the scheme. Guests then reach
      // Tauri through that page's IPC handler.
      context
        .protocol_handlers
        .borrow_mut()
        .entry(scheme.clone())
        .or_insert_with(|| (label.clone(), Rc::from(handler)));
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
  if let Some(config) = &window_builder.config {
    payload = json!({
      "windowId": serial,
      "label": label,
      "title": config.title,
      "width": config.width,
      "height": config.height,
      "x": config.x,
      "y": config.y,
      "center": config.center,
      "resizable": config.resizable,
      "decorations": config.decorations,
      "transparent": config.transparent,
      "visible": config.visible,
      "alwaysOnTop": config.always_on_top,
    });
  }
  if let Some(icon) = &context.icon {
    payload["icon"] = json!(icon.display().to_string());
  }
  if let (Some(webview), Some(webview_id)) = (&webview, webview_id) {
    payload["webview"] = json!({
      "webviewId": webview_id,
      "url": webview_url.clone(),
      "initScripts": initialization_scripts(webview),
      "policy": policy,
    });
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
    },
  );

  if let (Some(pending_webview), Some(webview_serial)) = (webview.as_mut(), webview_id) {
    if let Some(origin) = origin_of(&webview_url) {
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
        webview_url.clone(),
        handler,
      );
    }
    register_webview_handlers::<T>(context, webview_serial, pending_webview);
  }

  let webview = webview_id.map(|webview_id| DetachedWindowWebview {
    webview: DetachedWebview {
      label: label.clone(),
      dispatcher: ElectronWebviewDispatcher {
        id: webview_id,
        context: context.clone(),
        url: Arc::new(Mutex::new(webview_url)),
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
    },
    webview,
  })
}

pub(crate) fn create_webview_impl<T: UserEvent>(
  context: &RuntimeContext,
  window_id: WindowId,
  mut pending: PendingWebview<T, ElectronRuntime>,
) -> Result<DetachedWebview<T, ElectronRuntime>> {
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
    // See `create_window_impl`: the scheme keeps the first owner's label.
    context
      .protocol_handlers
      .borrow_mut()
      .entry(scheme.clone())
      .or_insert_with(|| (pending.label.clone(), Rc::from(handler)));
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
    "url": url.clone(),
    "initScripts": initialization_scripts(&pending),
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
      url: Arc::new(Mutex::new(url.clone())),
      last_evaluated_script: Default::default(),
    },
  })
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
) -> Vec<String> {
  pending
    .webview_attributes
    .initialization_scripts
    .iter()
    .map(|script| script.script.clone())
    .collect()
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

/// Tauri's webview-scoped callbacks, stored so the run loop can feed them the
/// events Electron forwards.
pub(crate) struct WebviewHandlers {
  pub(crate) page_load: Option<Rc<dyn Fn(url::Url, PageLoadEvent) + Send>>,
  pub(crate) title: Option<Rc<dyn Fn(String) + Send>>,
}

/// The callbacks Electron asks about out of band: a download question and
/// permission requests. Both are answered over IPC instead of blocking.
#[derive(Default)]
pub(crate) struct PolicyHandlers {
  download: Option<Arc<DownloadHandler>>,
  permission: Option<Box<dyn Fn(PermissionKind) -> PermissionResponse + Send + Sync>>,
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
  context.webview_handlers.borrow_mut().insert(
    webview_id,
    WebviewHandlers {
      page_load,
      title,
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
    permission: pending.permission_request_handler.take(),
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

#[cfg(test)]
mod tests {
  use super::*;

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
