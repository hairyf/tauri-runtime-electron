// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

use crate::platform::monitor_from_display;
use crate::rpc;
use crate::runtime::{
  create_webview_impl, create_window_impl, ElectronRuntime, EventProxy, Message, RuntimeContext,
};
use tauri_runtime::{
  dpi::PhysicalPosition,
  monitor::Monitor,
  webview::{DetachedWebview, PendingWebview},
  window::{DetachedWindow, PendingWindow, RawWindow, WindowId},
  DeviceEventFilter, Result, RuntimeHandle, UserEvent,
};
use tauri_utils::Theme;

#[derive(Debug, Clone)]
pub struct ElectronRuntimeHandle {
  pub(crate) context: RuntimeContext,
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
    let policy = match activation_policy {
      tauri_runtime::ActivationPolicy::Regular => rpc::app::ActivationPolicy::Regular,
      tauri_runtime::ActivationPolicy::Accessory => rpc::app::ActivationPolicy::Accessory,
      tauri_runtime::ActivationPolicy::Prohibited => rpc::app::ActivationPolicy::Prohibited,
      _ => rpc::app::ActivationPolicy::Regular,
    };
    self
      .context
      .ipc
      .call(rpc::AppActivationPolicy { policy })
      .map_err(|error| crate::runtime::create_window_err(error.to_string()))?;
    Ok(())
  }

  #[cfg(target_os = "macos")]
  #[cfg_attr(docsrs, doc(cfg(target_os = "macos")))]
  fn set_dock_visibility(&self, visible: bool) -> Result<()> {
    self
      .context
      .ipc
      .call(rpc::AppDockVisibility { visible })
      .map_err(|error| crate::runtime::create_window_err(error.to_string()))?;
    Ok(())
  }

  fn request_exit(&self, code: i32) -> Result<()> {
    self.context.send_message(Message::RequestExit(code))
  }

  /// Create a new webview window.
  fn create_window<F: Fn(RawWindow<'_>) + Send + 'static>(
    &self,
    pending: PendingWindow<T, Self::Runtime>,
    after_window_creation: Option<F>,
  ) -> Result<DetachedWindow<T, Self::Runtime>> {
    create_window_impl(&self.context, pending, after_window_creation)
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
    #[cfg(feature = "tauri")]
    let f = {
      let client = self.context.ipc.client();
      move || crate::command_bridge::with_client(client, f)
    };
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
    unimplemented!("tauri-runtime-electron: unsupported target, no display handle available");
  }

  fn primary_monitor(&self) -> Result<Option<Monitor>> {
    let displays = self
      .context
      .ipc
      .call(rpc::Displays {})
      .map_err(|error| crate::runtime::create_window_err(error.to_string()))?;
    Ok(Some(monitor_from_display(&displays.primary)))
  }

  fn monitor_from_point(&self, x: f64, y: f64) -> Result<Option<Monitor>> {
    let display = self
      .context
      .ipc
      .call(rpc::DisplayForPoint { x, y })
      .map_err(|error| crate::runtime::create_window_err(error.to_string()))?;
    Ok(display.as_ref().map(monitor_from_display))
  }

  fn available_monitors(&self) -> Result<Vec<Monitor>> {
    let displays = self
      .context
      .ipc
      .call(rpc::Displays {})
      .map_err(|error| crate::runtime::create_window_err(error.to_string()))?;
    Ok(
      displays
        .displays
        .iter()
        .map(monitor_from_display)
        .collect(),
    )
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

  /// Shows the application, but does not automatically focus it.
  #[cfg(target_os = "macos")]
  /// Shows the application (macOS `app.show()`; a no-op elsewhere).
  fn show(&self) -> Result<()> {
    self
      .context
      .ipc
      .call(rpc::AppShow {})
      .map_err(|error| crate::runtime::create_window_err(error.to_string()))?;
    Ok(())
  }

  /// Hides the application.
  #[cfg(target_os = "macos")]
  /// Hides the application (macOS `app.hide()`; a no-op elsewhere).
  fn hide(&self) -> Result<()> {
    self
      .context
      .ipc
      .call(rpc::AppHide {})
      .map_err(|error| crate::runtime::create_window_err(error.to_string()))?;
    Ok(())
  }

  fn set_device_event_filter(&self, _: DeviceEventFilter) {
    crate::runtime::unsupported(
      "RuntimeHandle::set_device_event_filter",
      "Electron reports no raw device events",
    );
  }

  #[cfg(target_os = "android")]
  fn find_class<'a>(
    &self,
    env: &mut jni::JNIEnv<'a>,
    activity: &jni::objects::JObject<'_>,
    name: impl Into<String>,
  ) -> std::result::Result<jni::objects::JClass<'a>, jni::errors::Error> {
    todo!("tauri-runtime-electron: Android is not a supported target")
  }

  #[cfg(target_os = "android")]
  fn run_on_android_context<F>(&self, f: F)
  where
    F: FnOnce(&mut jni::JNIEnv<'_>, &jni::objects::JObject<'_>, &jni::objects::JObject<'_>)
      + Send
      + 'static,
  {
    todo!("tauri-runtime-electron: Android is not a supported target")
  }

  #[cfg(any(target_os = "macos", target_os = "ios"))]
  fn fetch_data_store_identifiers<F: FnOnce(Vec<[u8; 16]>) + Send + 'static>(
    &self,
    _cb: F,
  ) -> Result<()> {
    Err(crate::runtime::create_webview_err(
      "Electron sessions have no WKWebView data store to enumerate".into(),
    ))
  }

  #[cfg(any(target_os = "macos", target_os = "ios"))]
  fn remove_data_store<F: FnOnce(Result<()>) + Send + 'static>(
    &self,
    _uuid: [u8; 16],
    _cb: F,
  ) -> Result<()> {
    Err(crate::runtime::create_webview_err(
      "Electron sessions have no WKWebView data store to remove".into(),
    ))
  }

  fn cursor_position(&self) -> Result<PhysicalPosition<f64>> {
    let point = self
      .context
      .ipc
      .call(rpc::CursorPosition {})
      .map_err(|error| crate::runtime::create_window_err(error.to_string()))?;
    Ok(PhysicalPosition::new(f64::from(point.x), f64::from(point.y)))
  }
}
