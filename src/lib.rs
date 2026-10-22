// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

#![allow(dead_code)]
#![allow(missing_docs)]

//! A Tauri runtime that delegates all window and webview work to an Electron main
//! process spawned as a child of the Tauri (Rust) process.
//!
//! ```ignore
//! tauri::Builder::default()
//!   .runtime(tauri_runtime_electron::Electron::default())
//!   .run(tauri::generate_context!())
//!   .unwrap();
//! ```
//!
//! Probe scope: `Runtime::new` spawns Electron and completes a handshake,
//! `Runtime::run` pumps IPC events into the Tauri run loop, window and webview
//! creation/`eval` are forwarded. Everything else is a stub that returns
//! defaults or `Ok(())`, see the TODO markers.

mod ipc;

use ipc::Ipc;
use serde_json::{json, Value};
use tauri_runtime::{
  DeviceEventFilter, Error, EventLoopProxy, ExitRequestedEventAction, Icon, ProgressBarState,
  Result, RunEvent, Runtime, RuntimeHandle, RuntimeInitArgs, RuntimeInitAttrs, UserAttentionType,
  UserEvent, WebviewDispatch, WindowDispatch, WindowEventId,
  dpi::{PhysicalPosition, PhysicalSize, Position, Size},
  monitor::Monitor,
  webview::{DetachedWebview, PendingWebview, WebviewIpcHandler},
  window::{
    CursorIcon, DetachedWindow, DetachedWindowWebview, PendingWindow, RawWindow, WindowBuilder,
    WindowBuilderBase, WindowEvent, WindowId,
  },
};
use tauri_utils::config::WebviewUrl;

#[cfg(target_os = "macos")]
use tauri_utils::TitleBarStyle;
use tauri_utils::{Theme, config::WindowConfig};
use url::Url;

#[cfg(windows)]
use windows::Win32::Foundation::HWND;

use http::Request;
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
const IPC_TIMEOUT: Duration = Duration::from_secs(15);

/// Default time the Electron process gets to start and connect back.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(60);

fn io_err(message: String) -> Box<dyn std::error::Error + Send + Sync> {
  Box::new(std::io::Error::other(message))
}

fn create_window_err(message: String) -> Error {
  Error::CreateWindow(io_err(message))
}

fn create_webview_err(message: String) -> Error {
  Error::CreateWebview(io_err(message))
}

type ShortcutMap = HashMap<String, Box<dyn Fn() + Send + 'static>>;

enum Message {
  Task(Box<dyn FnOnce() + Send>),
  CloseWindow(WindowId),
  DestroyWindow(WindowId),
  UserEvent(Box<dyn Any + Send>),
}

struct Webview {
  /// Numeric id handed to the Electron process.
  serial: u32,
}

struct Window {
  label: String,
  /// Numeric id handed to the Electron process (Tauri's `WindowId` is opaque).
  serial: u32,
  webviews: Vec<Webview>,
}

#[derive(Clone)]
pub struct RuntimeContext {
  is_running: Arc<AtomicBool>,
  windows: Arc<RefCell<HashMap<WindowId, Window>>>,
  shortcuts: Arc<Mutex<ShortcutMap>>,
  run_tx: SyncSender<Message>,
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
  /// Tauri IPC handlers, invoked when the page posts a message through
  /// `window.ipc.postMessage` (re-emitted by Electron as `ipc.message`).
  ///
  /// The handler is closed over the [`DetachedWebview`] it belongs to, so the
  /// map does not need to know the runtime's generic parameters.
  ipc_handlers: Rc<RefCell<HashMap<u32, Rc<dyn Fn(Request<String>) + Send>>>>,
}

// SAFETY: we ensure this type is only used on the main thread.
#[allow(clippy::non_send_fields_in_send_ty)]
unsafe impl Send for RuntimeContext {}

// SAFETY: we ensure this type is only used on the main thread.
#[allow(clippy::non_send_fields_in_send_ty)]
unsafe impl Sync for RuntimeContext {}

impl RuntimeContext {
  fn send_message(&self, message: Message) -> Result<()> {
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
        Message::UserEvent(_) => {}
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

  fn serial_for_window(&self, id: WindowId) -> Option<u32> {
    self.windows.borrow().get(&id).map(|window| window.serial)
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

  fn next_window_event_id(&self) -> WindowEventId {
    self.next_window_event_id.fetch_add(1, Ordering::Relaxed)
  }

  fn next_webview_event_id(&self) -> WindowEventId {
    self.next_webview_event_id.fetch_add(1, Ordering::Relaxed)
  }
}

impl fmt::Debug for RuntimeContext {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("RuntimeContext").finish()
  }
}

#[derive(Debug, Clone)]
pub struct ElectronRuntimeHandle {
  context: RuntimeContext,
}

impl<T: UserEvent> RuntimeHandle<T> for ElectronRuntimeHandle {
  type Runtime = ElectronRuntime;

  fn create_proxy(&self) -> EventProxy {
    EventProxy {
      run_tx: self.context.run_tx.clone(),
    }
  }

  #[cfg(target_os = "macos")]
  #[cfg_attr(docsrs, doc(cfg(target_os = "macos")))]
  fn set_activation_policy(
    &self,
    activation_policy: tauri_runtime::ActivationPolicy,
  ) -> Result<()> {
    Ok(())
  }

  #[cfg(target_os = "macos")]
  #[cfg_attr(docsrs, doc(cfg(target_os = "macos")))]
  fn set_dock_visibility(&self, visible: bool) -> Result<()> {
    Ok(())
  }

  fn request_exit(&self, code: i32) -> Result<()> {
    unimplemented!()
  }

  fn custom_scheme_url(&self, scheme: &str, _https: bool) -> String {
    format!("{scheme}://localhost")
  }

  fn webview_version(&self) -> Result<String> {
    Ok("0.0.0".into())
  }

  /// Create a new webview window.
  fn create_window<F: Fn(RawWindow<'_>) + Send + 'static>(
    &self,
    pending: PendingWindow<T, Self::Runtime>,
    _after_window_creation: Option<F>,
  ) -> Result<DetachedWindow<T, Self::Runtime>> {
    create_window_impl(&self.context, pending)
  }

  fn create_webview(
    &self,
    window_id: WindowId,
    pending: PendingWebview<T, Self::Runtime>,
  ) -> Result<DetachedWebview<T, Self::Runtime>> {
    create_webview_impl(&self.context, window_id, pending)
  }

  /// Run a task on the main thread.
  fn run_on_main_thread<F: FnOnce() + Send + 'static>(&self, f: F) -> Result<()> {
    self.context.send_message(Message::Task(Box::new(f)))
  }

  fn display_handle(
    &self,
  ) -> std::result::Result<raw_window_handle::DisplayHandle<'_>, raw_window_handle::HandleError> {
    #[cfg(target_os = "linux")]
    return Ok(unsafe {
      raw_window_handle::DisplayHandle::borrow_raw(raw_window_handle::RawDisplayHandle::Xlib(
        raw_window_handle::XlibDisplayHandle::new(None, 0),
      ))
    });
    #[cfg(target_os = "macos")]
    return Ok(unsafe {
      raw_window_handle::DisplayHandle::borrow_raw(raw_window_handle::RawDisplayHandle::AppKit(
        raw_window_handle::AppKitDisplayHandle::new(),
      ))
    });
    #[cfg(windows)]
    return Ok(unsafe {
      raw_window_handle::DisplayHandle::borrow_raw(raw_window_handle::RawDisplayHandle::Windows(
        raw_window_handle::WindowsDisplayHandle::new(),
      ))
    });
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    unimplemented!();
  }

  fn primary_monitor(&self) -> Result<Option<Monitor>> {
    unimplemented!()
  }

  fn monitor_from_point(&self, x: f64, y: f64) -> Result<Option<Monitor>> {
    unimplemented!()
  }

  fn available_monitors(&self) -> Result<Vec<Monitor>> {
    unimplemented!()
  }

  fn set_theme(&self, theme: Option<Theme>) {
    unimplemented!()
  }

  /// Shows the application, but does not automatically focus it.
  #[cfg(target_os = "macos")]
  fn show(&self) -> Result<()> {
    Ok(())
  }

  /// Hides the application.
  #[cfg(target_os = "macos")]
  fn hide(&self) -> Result<()> {
    Ok(())
  }

  fn set_device_event_filter(&self, _: DeviceEventFilter) {
    // no-op
  }

  #[cfg(target_os = "android")]
  fn find_class<'a>(
    &self,
    env: &mut jni::JNIEnv<'a>,
    activity: &jni::objects::JObject<'_>,
    name: impl Into<String>,
  ) -> std::result::Result<jni::objects::JClass<'a>, jni::errors::Error> {
    todo!()
  }

  #[cfg(target_os = "android")]
  fn run_on_android_context<F>(&self, f: F)
  where
    F: FnOnce(&mut jni::JNIEnv<'_>, &jni::objects::JObject<'_>, &jni::objects::JObject<'_>)
      + Send
      + 'static,
  {
    todo!()
  }

  #[cfg(any(target_os = "macos", target_os = "ios"))]
  fn fetch_data_store_identifiers<F: FnOnce(Vec<[u8; 16]>) + Send + 'static>(
    &self,
    cb: F,
  ) -> Result<()> {
    todo!()
  }

  #[cfg(any(target_os = "macos", target_os = "ios"))]
  fn remove_data_store<F: FnOnce(Result<()>) + Send + 'static>(
    &self,
    uuid: [u8; 16],
    cb: F,
  ) -> Result<()> {
    todo!()
  }

  fn cursor_position(&self) -> Result<PhysicalPosition<f64>> {
    Ok(PhysicalPosition::new(0.0, 0.0))
  }
}

#[derive(Debug, Clone)]
pub struct ElectronWebviewDispatcher {
  id: u32,
  context: RuntimeContext,
  url: Arc<Mutex<String>>,
  last_evaluated_script: Arc<Mutex<Option<String>>>,
}

impl ElectronWebviewDispatcher {
  pub fn last_evaluated_script(&self) -> Option<String> {
    self.last_evaluated_script.lock().unwrap().clone()
  }
}

#[derive(Debug, Clone)]
pub struct ElectronWindowDispatcher {
  id: WindowId,
  context: RuntimeContext,
}

#[derive(Debug, Clone, Default)]
pub struct ElectronWindowBuilder {
  /// Window configuration captured by `with_config`, used when creating the
  /// actual Electron `BrowserWindow`.
  pub(crate) config: Option<WindowConfig>,
}

impl WindowBuilderBase for ElectronWindowBuilder {}

impl WindowBuilder for ElectronWindowBuilder {
  fn new() -> Self {
    Self::default()
  }

  fn with_config(config: &WindowConfig) -> Self {
    Self {
      config: Some(config.clone()),
    }
  }

  fn center(self) -> Self {
    self
  }

  fn position(self, x: f64, y: f64) -> Self {
    self
  }

  fn inner_size(self, min_width: f64, min_height: f64) -> Self {
    self
  }

  fn min_inner_size(self, min_width: f64, min_height: f64) -> Self {
    self
  }

  fn max_inner_size(self, max_width: f64, max_height: f64) -> Self {
    self
  }

  fn inner_size_constraints(
    self,
    constraints: tauri_runtime::window::WindowSizeConstraints,
  ) -> Self {
    self
  }

  fn prevent_overflow(self) -> Self {
    self
  }

  fn prevent_overflow_with_margin(self, margin: tauri_runtime::dpi::Size) -> Self {
    self
  }

  fn resizable(self, resizable: bool) -> Self {
    self
  }

  fn maximizable(self, resizable: bool) -> Self {
    self
  }

  fn minimizable(self, resizable: bool) -> Self {
    self
  }

  fn closable(self, resizable: bool) -> Self {
    self
  }

  fn title<S: Into<String>>(self, title: S) -> Self {
    self
  }

  fn fullscreen(self, fullscreen: bool) -> Self {
    self
  }

  fn focused(self, focused: bool) -> Self {
    self
  }

  fn focusable(self, focusable: bool) -> Self {
    self
  }

  fn maximized(self, maximized: bool) -> Self {
    self
  }

  fn visible(self, visible: bool) -> Self {
    self
  }

  fn transparent(self, transparent: bool) -> Self {
    self
  }

  fn decorations(self, decorations: bool) -> Self {
    self
  }

  fn always_on_bottom(self, always_on_bottom: bool) -> Self {
    self
  }

  fn always_on_top(self, always_on_top: bool) -> Self {
    self
  }

  fn visible_on_all_workspaces(self, visible_on_all_workspaces: bool) -> Self {
    self
  }

  fn content_protected(self, protected: bool) -> Self {
    self
  }

  fn icon(self, icon: Icon<'_>) -> Result<Self> {
    Ok(self)
  }

  fn skip_taskbar(self, skip: bool) -> Self {
    self
  }

  fn window_classname<S: Into<String>>(self, classname: S) -> Self {
    self
  }

  fn no_redirection_bitmap(self, enable: bool) -> Self {
    self
  }

  fn shadow(self, enable: bool) -> Self {
    self
  }

  #[cfg(windows)]
  fn owner(self, owner: HWND) -> Self {
    self
  }

  #[cfg(windows)]
  fn parent(self, parent: HWND) -> Self {
    self
  }

  #[cfg(target_os = "macos")]
  fn parent(self, parent: *mut std::ffi::c_void) -> Self {
    self
  }

  #[cfg(any(
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd"
  ))]
  fn transient_for(self, parent: *mut std::ffi::c_void) -> Self {
    self
  }

  #[cfg(windows)]
  fn drag_and_drop(self, enabled: bool) -> Self {
    self
  }

  #[cfg(target_os = "macos")]
  fn title_bar_style(self, style: TitleBarStyle) -> Self {
    self
  }

  #[cfg(target_os = "macos")]
  fn traffic_light_position<P: Into<Position>>(self, position: P) -> Self {
    self
  }

  #[cfg(target_os = "macos")]
  fn hidden_title(self, transparent: bool) -> Self {
    self
  }

  #[cfg(target_os = "macos")]
  fn tabbing_identifier(self, identifier: &str) -> Self {
    self
  }

  fn theme(self, theme: Option<Theme>) -> Self {
    self
  }

  fn has_icon(&self) -> bool {
    false
  }

  fn get_theme(&self) -> Option<Theme> {
    None
  }

  fn background_color(self, _color: tauri_utils::config::Color) -> Self {
    self
  }

  #[cfg(target_os = "android")]
  fn activity_name<S: Into<String>>(self, _class_name: S) -> Self {
    self
  }

  #[cfg(target_os = "android")]
  fn created_by_activity_name<S: Into<String>>(self, _class_name: S) -> Self {
    self
  }

  #[cfg(target_os = "ios")]
  fn requested_by_scene_identifier<S: Into<String>>(self, _identifier: S) -> Self {
    self
  }
}

impl<T: UserEvent> WebviewDispatch<T> for ElectronWebviewDispatcher {
  type Runtime = ElectronRuntime;

  fn run_on_main_thread<F: FnOnce() + Send + 'static>(&self, f: F) -> Result<()> {
    self.context.send_message(Message::Task(Box::new(f)))
  }

  fn on_webview_event<F: Fn(&tauri_runtime::window::WebviewEvent) + Send + 'static>(
    &self,
    f: F,
  ) -> tauri_runtime::WebviewEventId {
    self.context.next_window_event_id()
  }

  fn with_webview<F: FnOnce(()) + Send + 'static>(&self, _f: F) -> Result<()> {
    Ok(())
  }

  #[cfg(target_os = "ios")]
  fn with_ios_webview<F: FnOnce(tauri_runtime::webview::IosWebviewHandle) + Send + 'static>(
    &self,
    _f: F,
  ) -> Result<()> {
    Ok(())
  }

  fn open_devtools(&self) {}

  fn close_devtools(&self) {}

  fn is_devtools_open(&self) -> Result<bool> {
    Ok(false)
  }

  fn set_zoom(&self, scale_factor: f64) -> Result<()> {
    Ok(())
  }

  fn eval_script<S: Into<String>>(&self, script: S) -> Result<()> {
    let script = script.into();
    self
      .last_evaluated_script
      .lock()
      .unwrap()
      .replace(script.clone());
    self
      .context
      .ipc
      .call(
        "webview.eval",
        json!({ "webviewId": self.id, "script": script }),
        IPC_TIMEOUT,
      )
      .map(|_| ())
      .map_err(create_webview_err)
  }

  fn eval_script_with_callback<S: Into<String>>(
    &self,
    script: S,
    callback: impl Fn(String) + Send + 'static,
  ) -> Result<()> {
    let script = script.into();
    self
      .last_evaluated_script
      .lock()
      .unwrap()
      .replace(script.clone());
    match self.context.ipc.call(
      "webview.eval",
      json!({ "webviewId": self.id, "script": script }),
      IPC_TIMEOUT,
    ) {
      Ok(value) => {
        callback(value.to_string());
        Ok(())
      }
      Err(e) => Err(create_webview_err(e)),
    }
  }

  fn url(&self) -> Result<String> {
    Ok(self.url.lock().unwrap().clone())
  }

  fn bounds(&self) -> Result<tauri_runtime::dpi::Rect> {
    Ok(tauri_runtime::dpi::Rect::default())
  }

  fn position(&self) -> Result<PhysicalPosition<i32>> {
    Ok(PhysicalPosition { x: 0, y: 0 })
  }

  fn size(&self) -> Result<PhysicalSize<u32>> {
    Ok(PhysicalSize {
      width: 0,
      height: 0,
    })
  }

  fn navigate(&self, url: Url) -> Result<()> {
    *self.url.lock().unwrap() = url.to_string();
    Ok(())
  }

  fn reload(&self) -> Result<()> {
    Ok(())
  }

  fn go_back(&self) -> Result<()> {
    Ok(())
  }

  fn can_go_back(&self) -> Result<bool> {
    Ok(false)
  }

  fn go_forward(&self) -> Result<()> {
    Ok(())
  }

  fn can_go_forward(&self) -> Result<bool> {
    Ok(false)
  }

  fn print(&self) -> Result<()> {
    Ok(())
  }

  fn close(&self) -> Result<()> {
    Ok(())
  }

  fn set_bounds(&self, bounds: tauri_runtime::dpi::Rect) -> Result<()> {
    Ok(())
  }

  fn set_size(&self, _size: Size) -> Result<()> {
    Ok(())
  }

  fn set_position(&self, _position: Position) -> Result<()> {
    Ok(())
  }

  fn set_focus(&self) -> Result<()> {
    Ok(())
  }

  fn reparent(&self, window_id: WindowId) -> Result<()> {
    Ok(())
  }

  fn cookies(&self) -> Result<Vec<tauri_runtime::Cookie<'static>>> {
    Ok(Vec::new())
  }

  fn cookies_for_url(&self, url: Url) -> Result<Vec<tauri_runtime::Cookie<'static>>> {
    Ok(Vec::new())
  }

  fn set_cookie(&self, cookie: tauri_runtime::Cookie<'_>) -> Result<()> {
    Ok(())
  }

  fn delete_cookie(&self, cookie: tauri_runtime::Cookie<'_>) -> Result<()> {
    Ok(())
  }

  fn set_auto_resize(&self, auto_resize: bool) -> Result<()> {
    Ok(())
  }

  fn clear_all_browsing_data(&self) -> Result<()> {
    Ok(())
  }

  fn hide(&self) -> Result<()> {
    Ok(())
  }

  fn show(&self) -> Result<()> {
    Ok(())
  }

  fn set_background_color(&self, color: Option<tauri_utils::config::Color>) -> Result<()> {
    Ok(())
  }
}

impl ElectronWindowDispatcher {
  fn window_call(&self, method: &str, mut params: Value) -> Result<()> {
    let serial = self
      .context
      .serial_for_window(self.id)
      .ok_or_else(|| create_window_err("window is no longer tracked".into()))?;
    if let Some(map) = params.as_object_mut() {
      map.insert("windowId".into(), json!(serial));
    }
    self
      .context
      .ipc
      .call(method, params, IPC_TIMEOUT)
      .map(|_| ())
      .map_err(create_window_err)
  }
}

impl<T: UserEvent> WindowDispatch<T> for ElectronWindowDispatcher {
  type Runtime = ElectronRuntime;

  type WindowBuilder = ElectronWindowBuilder;

  fn run_on_main_thread<F: FnOnce() + Send + 'static>(&self, f: F) -> Result<()> {
    self.context.send_message(Message::Task(Box::new(f)))
  }

  fn on_window_event<F: Fn(&WindowEvent) + Send + 'static>(&self, f: F) -> WindowEventId {
    self.context.next_window_event_id()
  }

  fn scale_factor(&self) -> Result<f64> {
    Ok(1.0)
  }

  fn inner_position(&self) -> Result<PhysicalPosition<i32>> {
    Ok(PhysicalPosition { x: 0, y: 0 })
  }

  fn outer_position(&self) -> Result<PhysicalPosition<i32>> {
    Ok(PhysicalPosition { x: 0, y: 0 })
  }

  fn inner_size(&self) -> Result<PhysicalSize<u32>> {
    Ok(PhysicalSize {
      width: 0,
      height: 0,
    })
  }

  fn outer_size(&self) -> Result<PhysicalSize<u32>> {
    Ok(PhysicalSize {
      width: 0,
      height: 0,
    })
  }

  fn is_fullscreen(&self) -> Result<bool> {
    Ok(false)
  }

  fn is_minimized(&self) -> Result<bool> {
    Ok(false)
  }

  fn is_maximized(&self) -> Result<bool> {
    Ok(false)
  }

  fn is_focused(&self) -> Result<bool> {
    Ok(false)
  }

  fn is_decorated(&self) -> Result<bool> {
    Ok(false)
  }

  fn is_resizable(&self) -> Result<bool> {
    Ok(false)
  }

  fn is_maximizable(&self) -> Result<bool> {
    Ok(true)
  }

  fn is_minimizable(&self) -> Result<bool> {
    Ok(true)
  }

  fn is_closable(&self) -> Result<bool> {
    Ok(true)
  }

  fn is_visible(&self) -> Result<bool> {
    Ok(true)
  }

  fn title(&self) -> Result<String> {
    Ok(String::new())
  }

  fn current_monitor(&self) -> Result<Option<Monitor>> {
    Ok(None)
  }

  fn primary_monitor(&self) -> Result<Option<Monitor>> {
    Ok(None)
  }

  fn monitor_from_point(&self, x: f64, y: f64) -> Result<Option<Monitor>> {
    Ok(None)
  }

  fn available_monitors(&self) -> Result<Vec<Monitor>> {
    Ok(Vec::new())
  }

  fn theme(&self) -> Result<Theme> {
    Ok(Theme::Light)
  }

  #[cfg(any(
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd"
  ))]
  fn gtk_window(&self) -> Result<*mut std::ffi::c_void> {
    unimplemented!()
  }

  #[cfg(any(
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd"
  ))]
  fn default_vbox(&self) -> Result<*mut std::ffi::c_void> {
    unimplemented!()
  }

  #[cfg(target_os = "android")]
  fn activity_name(&self) -> Result<String> {
    unimplemented!()
  }

  #[cfg(target_os = "ios")]
  fn scene_identifier(&self) -> Result<String> {
    unimplemented!()
  }

  fn window_handle(
    &self,
  ) -> std::result::Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
    #[cfg(target_os = "linux")]
    return unsafe {
      Ok(raw_window_handle::WindowHandle::borrow_raw(
        raw_window_handle::RawWindowHandle::Xlib(raw_window_handle::XlibWindowHandle::new(0)),
      ))
    };
    #[cfg(target_os = "macos")]
    return unsafe {
      Ok(raw_window_handle::WindowHandle::borrow_raw(
        raw_window_handle::RawWindowHandle::AppKit(raw_window_handle::AppKitWindowHandle::new(
          std::ptr::NonNull::from(&()).cast(),
        )),
      ))
    };
    #[cfg(windows)]
    return unsafe {
      Ok(raw_window_handle::WindowHandle::borrow_raw(
        raw_window_handle::RawWindowHandle::Win32(raw_window_handle::Win32WindowHandle::new(
          std::num::NonZeroIsize::MIN,
        )),
      ))
    };
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    unimplemented!();
  }

  fn center(&self) -> Result<()> {
    Ok(())
  }

  fn request_user_attention(&self, request_type: Option<UserAttentionType>) -> Result<()> {
    Ok(())
  }

  fn create_window<F: Fn(RawWindow<'_>) + Send + 'static>(
    &mut self,
    pending: PendingWindow<T, Self::Runtime>,
    _after_window_creation: Option<F>,
  ) -> Result<DetachedWindow<T, Self::Runtime>> {
    create_window_impl(&self.context, pending)
  }

  fn create_webview(
    &mut self,
    pending: PendingWebview<T, Self::Runtime>,
  ) -> Result<DetachedWebview<T, Self::Runtime>> {
    create_webview_impl(&self.context, self.id, pending)
  }

  fn set_resizable(&self, resizable: bool) -> Result<()> {
    Ok(())
  }

  fn set_maximizable(&self, maximizable: bool) -> Result<()> {
    Ok(())
  }

  fn set_minimizable(&self, minimizable: bool) -> Result<()> {
    Ok(())
  }

  fn set_closable(&self, closable: bool) -> Result<()> {
    Ok(())
  }

  fn set_title<S: Into<String>>(&self, title: S) -> Result<()> {
    self.window_call("window.setTitle", json!({ "title": title.into() }))
  }

  fn maximize(&self) -> Result<()> {
    Ok(())
  }

  fn unmaximize(&self) -> Result<()> {
    Ok(())
  }

  fn minimize(&self) -> Result<()> {
    Ok(())
  }

  fn unminimize(&self) -> Result<()> {
    Ok(())
  }

  fn show(&self) -> Result<()> {
    self.window_call("window.show", json!({}))
  }

  fn hide(&self) -> Result<()> {
    self.window_call("window.hide", json!({}))
  }

  fn close(&self) -> Result<()> {
    // Let Electron tear the window down; it reports back with `window.closed`,
    // which drives the `CloseRequested` event in the run loop.
    self.window_call("window.close", json!({}))?;
    self.context.send_message(Message::CloseWindow(self.id))?;
    Ok(())
  }

  fn destroy(&self) -> Result<()> {
    self.window_call("window.destroy", json!({}))?;
    self.context.send_message(Message::DestroyWindow(self.id))?;
    Ok(())
  }

  fn set_decorations(&self, decorations: bool) -> Result<()> {
    Ok(())
  }

  fn set_shadow(&self, shadow: bool) -> Result<()> {
    Ok(())
  }

  fn set_always_on_bottom(&self, always_on_bottom: bool) -> Result<()> {
    Ok(())
  }

  fn set_always_on_top(&self, always_on_top: bool) -> Result<()> {
    Ok(())
  }

  fn set_visible_on_all_workspaces(&self, visible_on_all_workspaces: bool) -> Result<()> {
    Ok(())
  }

  fn set_content_protected(&self, protected: bool) -> Result<()> {
    Ok(())
  }

  fn set_size(&self, size: Size) -> Result<()> {
    Ok(())
  }

  fn set_min_size(&self, size: Option<Size>) -> Result<()> {
    Ok(())
  }

  fn set_max_size(&self, size: Option<Size>) -> Result<()> {
    Ok(())
  }

  fn set_position(&self, position: Position) -> Result<()> {
    Ok(())
  }

  fn set_fullscreen(&self, fullscreen: bool) -> Result<()> {
    Ok(())
  }

  fn set_fullscreen_on_monitor(&self, position: PhysicalPosition<f64>) -> Result<()> {
    Ok(())
  }

  #[cfg(target_os = "macos")]
  fn set_simple_fullscreen(&self, enable: bool) -> Result<()> {
    Ok(())
  }

  fn set_focus(&self) -> Result<()> {
    self.window_call("window.focus", json!({}))
  }

  fn set_focusable(&self, focusable: bool) -> Result<()> {
    Ok(())
  }

  fn set_icon(&self, icon: Icon<'_>) -> Result<()> {
    Ok(())
  }

  fn set_skip_taskbar(&self, skip: bool) -> Result<()> {
    Ok(())
  }

  fn set_cursor_grab(&self, grab: bool) -> Result<()> {
    Ok(())
  }

  fn set_cursor_visible(&self, visible: bool) -> Result<()> {
    Ok(())
  }

  fn set_cursor_icon(&self, icon: CursorIcon) -> Result<()> {
    Ok(())
  }

  fn set_cursor_position<Pos: Into<Position>>(&self, position: Pos) -> Result<()> {
    Ok(())
  }

  fn set_ignore_cursor_events(&self, ignore: bool) -> Result<()> {
    Ok(())
  }

  fn start_dragging(&self) -> Result<()> {
    Ok(())
  }

  fn start_resize_dragging(&self, direction: tauri_runtime::ResizeDirection) -> Result<()> {
    Ok(())
  }

  fn set_progress_bar(&self, progress_state: ProgressBarState) -> Result<()> {
    Ok(())
  }

  fn set_badge_count(&self, count: Option<i64>, desktop_filename: Option<String>) -> Result<()> {
    Ok(())
  }

  fn set_badge_label(&self, label: Option<String>) -> Result<()> {
    Ok(())
  }

  fn set_overlay_icon(&self, icon: Option<Icon<'_>>) -> Result<()> {
    Ok(())
  }

  fn set_title_bar_style(&self, style: tauri_utils::TitleBarStyle) -> Result<()> {
    Ok(())
  }

  fn set_traffic_light_position(&self, position: Position) -> Result<()> {
    Ok(())
  }

  fn set_size_constraints(
    &self,
    constraints: tauri_runtime::window::WindowSizeConstraints,
  ) -> Result<()> {
    Ok(())
  }

  fn set_theme(&self, theme: Option<Theme>) -> Result<()> {
    Ok(())
  }

  fn set_enabled(&self, enabled: bool) -> Result<()> {
    Ok(())
  }

  fn is_enabled(&self) -> Result<bool> {
    Ok(true)
  }

  fn is_always_on_top(&self) -> Result<bool> {
    Ok(false)
  }

  fn set_background_color(&self, color: Option<tauri_utils::config::Color>) -> Result<()> {
    Ok(())
  }
}

#[derive(Clone)]
pub struct EventProxy {
  run_tx: SyncSender<Message>,
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
  fn init(ipc: Arc<Ipc>, event_rx: Receiver<Value>, dev_url: Option<String>) -> Self {
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
      ipc_handlers: Default::default(),
    };
    Self {
      is_running,
      context,
      run_rx: rx,
      event_rx,
    }
  }

  fn start(attrs: Electron) -> Result<Self> {
    let binary = attrs.resolve_binary();
    let main_script = attrs.resolve_main_script();
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
    let (ipc, event_rx) = Ipc::launch(
      &binary,
      &main_script,
      &extra_args(&attrs.args),
      attrs.startup_timeout.unwrap_or(STARTUP_TIMEOUT),
    )
    .map_err(|message| Error::CreateWindow(io_err(message)))?;
    Ok(Self::init(ipc, event_rx, attrs.dev_url))
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

  pub fn dev_url(mut self, url: impl Into<String>) -> Self {
    self.dev_url = Some(url.into().trim_end_matches('/').to_string());
    self
  }

  pub fn timeout(mut self, timeout: Duration) -> Self {
    self.startup_timeout = Some(timeout);
    self
  }

  fn resolve_binary(&self) -> PathBuf {
    if let Some(binary) = std::env::var_os("TAURI_RUNTIME_ELECTRON_BINARY") {
      return PathBuf::from(binary);
    }
    if let Some(binary) = &self.binary {
      return binary.clone();
    }
    // Probe the usual pnpm/npm layout next to (or above) the current directory.
    let name = if cfg!(windows) {
      "node_modules/electron/dist/electron.exe"
    } else {
      "node_modules/electron/dist/electron"
    };
    let mut dir = std::env::current_dir().ok();
    for _ in 0..6 {
      let Some(current) = dir else { break };
      let candidate = current.join(name);
      if candidate.exists() {
        return candidate;
      }
      dir = current.parent().map(PathBuf::from);
    }
    PathBuf::from("electron")
  }

  fn resolve_main_script(&self) -> PathBuf {
    self.main_script.clone().unwrap_or_else(|| {
      Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("electron")
        .join("main.cjs")
    })
  }
}

impl<T: UserEvent> RuntimeInitAttrs<T> for Electron {
  type Runtime = ElectronRuntime;
}

impl<T: UserEvent> From<Electron> for tauri_runtime::dynamic::DynRuntimeInitAttrs<T> {
  fn from(attrs: Electron) -> Self {
    Self::new(attrs)
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
    Self::start(args.runtime_init_attrs)
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
    Self::start(args.runtime_init_attrs)
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
    unimplemented!()
  }

  fn monitor_from_point(&self, x: f64, y: f64) -> Option<Monitor> {
    unimplemented!()
  }

  fn available_monitors(&self) -> Vec<Monitor> {
    unimplemented!()
  }

  fn set_theme(&self, theme: Option<Theme>) {
    unimplemented!()
  }

  #[cfg(target_os = "macos")]
  #[cfg_attr(docsrs, doc(cfg(target_os = "macos")))]
  fn set_activation_policy(&mut self, activation_policy: tauri_runtime::ActivationPolicy) {}

  #[cfg(target_os = "macos")]
  #[cfg_attr(docsrs, doc(cfg(target_os = "macos")))]
  fn set_dock_visibility(&mut self, visible: bool) {}

  #[cfg(target_os = "macos")]
  #[cfg_attr(docsrs, doc(cfg(target_os = "macos")))]
  fn set_activate_ignoring_other_apps(&mut self, ignore: bool) {}

  #[cfg(target_os = "macos")]
  #[cfg_attr(docsrs, doc(cfg(target_os = "macos")))]
  fn show(&self) {}

  #[cfg(target_os = "macos")]
  #[cfg_attr(docsrs, doc(cfg(target_os = "macos")))]
  fn hide(&self) {}

  fn set_device_event_filter(&mut self, filter: DeviceEventFilter) {}

  #[cfg(any(
    target_os = "macos",
    windows,
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd"
  ))]
  fn run_iteration<F: FnMut(RunEvent<T>)>(&mut self, callback: F) {}

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
          Some("window.closed") => {
            let serial = event["params"]["windowId"].as_u64().unwrap_or_default() as u32;
            let Some(id) = self.context.window_for_serial(serial) else {
              continue;
            };
            if window_closed(&self.context, id, &mut callback) {
              break 'event_loop;
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
          Some("ipc.closed") => break 'event_loop,
          _ => {}
        }
      }

      // Messages sent by Tauri (window operations run on the main thread).
      while let Ok(message) = self.run_rx.try_recv() {
        match message {
          Message::Task(task) => task(),
          Message::CloseWindow(id) => {
            if window_closed(&self.context, id, &mut callback) {
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
        }
      }

      callback(RunEvent::MainEventsCleared);

      std::thread::sleep(Duration::from_millis(10));
    }

    self.is_running.store(false, Ordering::Relaxed);
    self.context.ipc.shutdown();
    callback(RunEvent::Exit);
  }

  fn cursor_position(&self) -> Result<PhysicalPosition<f64>> {
    Ok(PhysicalPosition::new(0.0, 0.0))
  }
}

/// Emits `WindowEvent::CloseRequested` for `id`; returns `true` if the runtime
/// should exit (window was removable and the app has no windows left).
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

  let (tx, rx) = channel();
  callback(RunEvent::WindowEvent {
    label,
    event: WindowEvent::CloseRequested { signal_tx: tx },
  });
  if matches!(rx.try_recv(), Ok(true)) {
    return false;
  }

  context.windows.borrow_mut().remove(&id);
  exit_if_empty(context, callback)
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

fn create_window_impl<T: UserEvent>(
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
    });
  }
  if let (Some(webview), Some(webview_id)) = (&webview, webview_id) {
    payload["webview"] = json!({
      "webviewId": webview_id,
      "url": context.resolve_url(&webview.webview_attributes.url),
      "initScripts": initialization_scripts(webview),
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
        .map(|serial| Webview { serial })
        .into_iter()
        .collect(),
    },
  );

  if let (Some(pending_webview), Some(webview_serial)) = (webview.as_mut(), webview_id) {
    let url = context.resolve_url(&pending_webview.webview_attributes.url);
    if let Some(handler) = pending_webview.ipc_handler.take() {
      register_ipc_handler::<T>(
        context,
        webview_serial,
        label.clone(),
        url,
        handler,
      );
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

fn create_webview_impl<T: UserEvent>(
  context: &RuntimeContext,
  window_id: WindowId,
  mut pending: PendingWebview<T, ElectronRuntime>,
) -> Result<DetachedWebview<T, ElectronRuntime>> {
  let serial = context
    .serial_for_window(window_id)
    .ok_or_else(|| create_webview_err("window does not exist".into()))?;
  let webview_id = context.next_webview_id();

  let url = context.resolve_url(&pending.webview_attributes.url);
  let payload = json!({
    "windowId": serial,
    "webviewId": webview_id,
    "url": url,
    "initScripts": initialization_scripts(&pending),
  });
  context
    .ipc
    .call("webview.create", payload, IPC_TIMEOUT)
    .map_err(create_webview_err)?;

  if let Some(window) = context.windows.borrow_mut().get_mut(&window_id) {
    window.webviews.push(Webview { serial: webview_id });
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
