// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

use crate::runtime::{
  ElectronRuntime, EventProxy, Message, RuntimeContext, create_webview_impl, create_window_impl,
};
use crate::platform::monitor_from_value;
use serde_json::json;
use tauri_runtime::{
  DeviceEventFilter, Result, RuntimeHandle, UserEvent,
  dpi::PhysicalPosition,
  monitor::Monitor,
  webview::{DetachedWebview, PendingWebview},
  window::{DetachedWindow, PendingWindow, RawWindow, WindowId},
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
      tauri_runtime::ActivationPolicy::Regular => "regular",
      tauri_runtime::ActivationPolicy::Accessory => "accessory",
      tauri_runtime::ActivationPolicy::Prohibited => "prohibited",
    };
    self
      .context
      .ipc
      .call(
        "app.activationPolicy",
        json!({ "policy": policy }),
        crate::runtime::IPC_TIMEOUT,
      )
      .map_err(crate::runtime::create_window_err)?;
    Ok(())
  }

  #[cfg(target_os = "macos")]
  #[cfg_attr(docsrs, doc(cfg(target_os = "macos")))]
  fn set_dock_visibility(&self, visible: bool) -> Result<()> {
    self
      .context
      .ipc
      .call(
        "app.dockVisibility",
        json!({ "visible": visible }),
        crate::runtime::IPC_TIMEOUT,
      )
      .map_err(crate::runtime::create_window_err)?;
    Ok(())
  }

  fn request_exit(&self, _code: i32) -> Result<()> {
    self.context.send_message(Message::Exit)
  }

  fn custom_scheme_url(&self, scheme: &str, _https: bool) -> String {
    format!("{scheme}://localhost")
  }

  fn webview_version(&self) -> Result<String> {
    Ok(self.context.ipc.version.clone())
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
    let displays = self
      .context
      .ipc
      .call("window.displays", json!({}), crate::runtime::IPC_TIMEOUT)
      .map_err(crate::runtime::create_window_err)?;
    Ok(monitor_from_value(&displays["primary"]))
  }

  fn monitor_from_point(&self, x: f64, y: f64) -> Result<Option<Monitor>> {
    let display = self
      .context
      .ipc
      .call(
        "window.displayForPoint",
        json!({ "x": x, "y": y }),
        crate::runtime::IPC_TIMEOUT,
      )
      .map_err(crate::runtime::create_window_err)?;
    Ok(monitor_from_value(&display))
  }

  fn available_monitors(&self) -> Result<Vec<Monitor>> {
    let displays = self
      .context
      .ipc
      .call("window.displays", json!({}), crate::runtime::IPC_TIMEOUT)
      .map_err(crate::runtime::create_window_err)?;
    Ok(
      displays["displays"]
        .as_array()
        .map(|list| list.iter().filter_map(monitor_from_value).collect())
        .unwrap_or_default(),
    )
  }

  fn set_theme(&self, theme: Option<Theme>) {
    let theme = match theme {
      Some(Theme::Dark) => "dark",
      Some(_) => "light",
      None => "system",
    };
    if let Err(error) = self.context.ipc.call(
      "app.theme",
      json!({ "theme": theme }),
      crate::runtime::IPC_TIMEOUT,
    ) {
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
      .call("app.show", json!({}), crate::runtime::IPC_TIMEOUT)
      .map_err(crate::runtime::create_window_err)?;
    Ok(())
  }

  /// Hides the application.
  #[cfg(target_os = "macos")]
  /// Hides the application (macOS `app.hide()`; a no-op elsewhere).
  fn hide(&self) -> Result<()> {
    self
      .context
      .ipc
      .call("app.hide", json!({}), crate::runtime::IPC_TIMEOUT)
      .map_err(crate::runtime::create_window_err)?;
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
    let point = self
      .context
      .ipc
      .call("window.cursorPosition", json!({}), crate::runtime::IPC_TIMEOUT)
      .map_err(crate::runtime::create_window_err)?;
    Ok(PhysicalPosition::new(
      point["x"].as_f64().unwrap_or_default(),
      point["y"].as_f64().unwrap_or_default(),
    ))
  }
}
