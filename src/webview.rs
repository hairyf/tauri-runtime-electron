// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

use crate::rpc;
use crate::runtime::{
  create_webview_err, position_payload, size_payload, ElectronRuntime, Message, RuntimeContext,
};
use electron_rs::RpcRequest;
use serde_json::Value;
use std::sync::{Arc, Mutex};
use tauri_runtime::{
  dpi::{PhysicalPosition, PhysicalSize, Position, Size},
  window::WindowId,
  Result, UserEvent, WebviewDispatch,
};
use url::Url;

#[derive(Debug, Clone)]
pub struct ElectronWebviewDispatcher {
  pub(crate) id: u32,
  pub(crate) context: RuntimeContext,
  pub(crate) last_evaluated_script: Arc<Mutex<Option<String>>>,
}

impl ElectronWebviewDispatcher {
  pub fn last_evaluated_script(&self) -> Option<String> {
    self.last_evaluated_script.lock().unwrap().clone()
  }

  /// Queries the guest's physical-pixel rect from the Electron side.
  fn rectangle(&self) -> Result<tauri_runtime::dpi::Rect> {
    let bounds = self.webview_call(rpc::WebviewBounds {
      webview_id: self.id,
    })?;
    Ok(rect_from_value(&bounds["bounds"]))
  }
}

impl ElectronWebviewDispatcher {
  // Tauri 2's `WebviewDispatch` has no history methods (the webview plugin drives
  // them through `eval`), so these stay inherent for direct use.
  pub fn go_back(&self) -> Result<()> {
    self
      .webview_call(rpc::WebviewGoBack {
        webview_id: self.id,
      })
      .map(|_| ())
  }

  pub fn can_go_back(&self) -> Result<bool> {
    self.webview_call(rpc::WebviewCanGoBack {
      webview_id: self.id,
    })
  }

  pub fn go_forward(&self) -> Result<()> {
    self
      .webview_call(rpc::WebviewGoForward {
        webview_id: self.id,
      })
      .map(|_| ())
  }

  pub fn can_go_forward(&self) -> Result<bool> {
    self.webview_call(rpc::WebviewCanGoForward {
      webview_id: self.id,
    })
  }
}

impl<T: UserEvent> WebviewDispatch<T> for ElectronWebviewDispatcher {
  type Runtime = ElectronRuntime;

  fn run_on_main_thread<F: FnOnce() + Send + 'static>(&self, f: F) -> Result<()> {
    let webview = self.clone();
    self.context.send_message(Message::Task(Box::new(move || {
      crate::new_window::with_webview(webview, f)
    })))
  }

  fn on_webview_event<F: Fn(&tauri_runtime::window::WebviewEvent) + Send + 'static>(
    &self,
    f: F,
  ) -> tauri_runtime::WebviewEventId {
    let id = self.context.next_webview_event_id();
    self.context.add_webview_event_handler(self.id, id, f);
    id
  }

  /// Electron owns its WebContents in another process. There is no local native
  /// webview object compatible with Tauri's platform-specific downcasts.
  fn with_webview<F: FnOnce(Box<dyn std::any::Any>) + Send + 'static>(&self, _f: F) -> Result<()> {
    Err(create_webview_err(
      "WebviewDispatch::with_webview is unavailable: Electron owns the webview in another process"
        .into(),
    ))
  }

  #[cfg(target_os = "ios")]
  fn with_ios_webview<F: FnOnce(tauri_runtime::webview::IosWebviewHandle) + Send + 'static>(
    &self,
    _f: F,
  ) -> Result<()> {
    Ok(())
  }

  fn open_devtools(&self) {
    if let Err(error) = crate::devtools::open(self) {
      eprintln!("[electron] open devtools failed: {error}");
    }
  }

  fn close_devtools(&self) {
    if let Err(error) = crate::devtools::close(self) {
      eprintln!("[electron] close devtools failed: {error}");
    }
  }

  fn is_devtools_open(&self) -> Result<bool> {
    crate::devtools::is_open(self)
  }

  fn set_zoom(&self, scale_factor: f64) -> Result<()> {
    self
      .webview_call(rpc::WebviewSetZoom {
        webview_id: self.id,
        scale_factor: scale_factor,
      })
      .map(|_| ())
  }

  fn eval_script<S: Into<String>>(&self, script: S) -> Result<()> {
    let script = script.into();
    self
      .last_evaluated_script
      .lock()
      .unwrap()
      .replace(script.clone());
    // Fire and forget: Tauri evaluates scripts from its event loop (channel
    // sends, event emitters), so a page that is busy for longer than
    // `IPC_TIMEOUT` must not block window events and IPC handling behind it.
    self
      .context
      .ipc
      .notify(rpc::EvalScript {
        webview_id: self.id,
        script,
      })
      .map_err(|error| tauri_runtime::Error::CreateWebview(Box::new(error)))
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
    self.context.window_for_webview(self.id)?;
    let run_tx = self.context.run_tx.clone();
    self
      .context
      .ipc
      .dispatch(
        rpc::EvalScriptCallback {
          webview_id: self.id,
          script,
        },
        move |result| {
          // Only a Send channel crosses threads, never the runtime's RefCells.
          let _ = run_tx.send(Message::Task(Box::new(move || match result {
            Ok(value) => callback(value.to_string()),
            Err(error) => eprintln!("[electron] script evaluation failed: {error}"),
          })));
        },
      )
      .map_err(|error| create_webview_err(error.to_string()))
  }

  fn url(&self) -> Result<String> {
    self.current_url()
  }

  fn bounds(&self) -> Result<tauri_runtime::dpi::Rect> {
    self.rectangle()
  }

  fn position(&self) -> Result<PhysicalPosition<i32>> {
    Ok(self.rectangle()?.position.to_physical::<i32>(1.0))
  }

  fn size(&self) -> Result<PhysicalSize<u32>> {
    Ok(self.rectangle()?.size.to_physical::<u32>(1.0))
  }

  fn navigate(&self, url: Url) -> Result<()> {
    self
      .webview_call(rpc::WebviewNavigate {
        webview_id: self.id,
        url: url.to_string(),
      })
      .map(|_| ())
  }

  fn reload(&self) -> Result<()> {
    self
      .webview_call(rpc::WebviewReload {
        webview_id: self.id,
      })
      .map(|_| ())
  }

  fn print(&self) -> Result<()> {
    self
      .webview_call(rpc::WebviewPrint {
        webview_id: self.id,
      })
      .map(|_| ())
  }

  fn close(&self) -> Result<()> {
    // Native views are owned independently; closing a page leaves its window alive.
    self
      .webview_call(rpc::WebviewClose {
        webview_id: self.id,
      })
      .map(|_| ())
  }

  fn set_bounds(&self, bounds: tauri_runtime::dpi::Rect) -> Result<()> {
    self
      .webview_call(rpc::WebviewSetBounds {
        webview_id: self.id,
        position: Some(position_payload(bounds.position)),
        size: Some(size_payload(bounds.size)),
      })
      .map(|_| ())
  }

  fn set_size(&self, size: Size) -> Result<()> {
    self
      .webview_call(rpc::WebviewSetBounds {
        webview_id: self.id,
        position: None,
        size: Some(size_payload(size)),
      })
      .map(|_| ())
  }

  fn set_position(&self, position: Position) -> Result<()> {
    self
      .webview_call(rpc::WebviewSetBounds {
        webview_id: self.id,
        position: Some(position_payload(position)),
        size: None,
      })
      .map(|_| ())
  }

  fn set_focus(&self) -> Result<()> {
    self
      .webview_call(rpc::WebviewFocus {
        webview_id: self.id,
      })
      .map(|_| ())
  }

  fn reparent(&self, window_id: WindowId) -> Result<()> {
    self.context.reparent_webview(self.id, window_id)
  }

  fn cookies(&self) -> Result<Vec<tauri_runtime::Cookie<'static>>> {
    let cookies = self.webview_call(rpc::WebviewCookies {
      webview_id: self.id,
      url: None,
    })?;
    Ok(parse_cookies(&cookies))
  }

  fn cookies_for_url(&self, url: Url) -> Result<Vec<tauri_runtime::Cookie<'static>>> {
    let cookies = self.webview_call(rpc::WebviewCookies {
      webview_id: self.id,
      url: Some(url.to_string()),
    })?;
    Ok(parse_cookies(&cookies))
  }

  fn set_cookie(&self, cookie: tauri_runtime::Cookie<'_>) -> Result<()> {
    let url = self.cookie_url(&cookie)?;
    self
      .webview_call(rpc::WebviewSetCookie {
        webview_id: self.id,
        url: url,
        name: cookie.name().to_owned(),
        value: cookie.value().to_owned(),
        domain: cookie.domain().map(str::to_owned),
        path: cookie.path().map(str::to_owned),
        secure: cookie.secure().unwrap_or(false),
        http_only: cookie.http_only(),
        same_site: same_site_name(cookie.same_site()),
      })
      .map(|_| ())
  }

  fn delete_cookie(&self, cookie: tauri_runtime::Cookie<'_>) -> Result<()> {
    let url = self.cookie_url(&cookie)?;
    self
      .webview_call(rpc::WebviewDeleteCookie {
        webview_id: self.id,
        url: url,
        name: cookie.name().to_owned(),
      })
      .map(|_| ())
  }

  /// Guests follow their CSS box; Electron's own auto-resize is forwarded for
  /// hosts that want the contents to track the window instead.
  fn set_auto_resize(&self, auto_resize: bool) -> Result<()> {
    self
      .webview_call(rpc::WebviewSetAutoResize {
        webview_id: self.id,
        auto_resize: auto_resize,
      })
      .map(|_| ())
  }

  fn clear_all_browsing_data(&self) -> Result<()> {
    self
      .webview_call(rpc::WebviewClearBrowsingData {
        webview_id: self.id,
      })
      .map(|_| ())
  }

  fn hide(&self) -> Result<()> {
    self
      .webview_call(rpc::WebviewVisibility {
        webview_id: self.id,
        visible: false,
      })
      .map(|_| ())
  }

  fn show(&self) -> Result<()> {
    self
      .webview_call(rpc::WebviewVisibility {
        webview_id: self.id,
        visible: true,
      })
      .map(|_| ())
  }

  fn set_background_color(&self, color: Option<tauri_utils::config::Color>) -> Result<()> {
    self
      .webview_call(rpc::WebviewBackgroundColor {
        webview_id: self.id,
        color: color.map(css_color),
      })
      .map(|_| ())
  }
}

impl ElectronWebviewDispatcher {
  fn current_url(&self) -> Result<String> {
    self.webview_call(rpc::WebviewUrl {
      webview_id: self.id,
    })
  }

  /// Use the committed page, not the builder URL or a requested navigation.
  fn cookie_url(&self, cookie: &tauri_runtime::Cookie<'_>) -> Result<String> {
    let raw = match cookie.domain() {
      Some(domain) => format!(
        "{}://{}/",
        if cookie.secure().unwrap_or(false) {
          "https"
        } else {
          "http"
        },
        domain.trim_start_matches('.')
      ),
      None => self.current_url()?,
    };
    let mut url = Url::parse(&raw).map_err(|error| create_webview_err(error.to_string()))?;
    if let Some(path) = cookie.path() {
      url.set_path(path);
    }
    Ok(url.into())
  }
}

impl ElectronWebviewDispatcher {
  pub(crate) fn webview_call<R: RpcRequest>(&self, request: R) -> Result<R::Response> {
    self
      .context
      .ipc
      .call(request)
      .map_err(|error| tauri_runtime::Error::CreateWebview(Box::new(error)))
  }
}

/// Converts the physical-pixel rect reported by `electron-rs/assets/main.cjs`.
fn rect_from_value(value: &Value) -> tauri_runtime::dpi::Rect {
  let position = Position::Physical(PhysicalPosition::new(
    value["x"].as_i64().unwrap_or_default() as i32,
    value["y"].as_i64().unwrap_or_default() as i32,
  ));
  let size = Size::Physical(PhysicalSize::new(
    value["width"].as_u64().unwrap_or_default() as u32,
    value["height"].as_u64().unwrap_or_default() as u32,
  ));
  tauri_runtime::dpi::Rect { position, size }
}

fn parse_cookies(value: &[Value]) -> Vec<tauri_runtime::Cookie<'static>> {
  value.iter().filter_map(cookie_from_value).collect()
}

fn cookie_from_value(value: &Value) -> Option<tauri_runtime::Cookie<'static>> {
  let name = value["name"].as_str()?.to_string();
  let cookie_value = value["value"].as_str().unwrap_or_default().to_string();
  let mut builder = tauri_runtime::Cookie::build((name, cookie_value))
    .path(value["path"].as_str().unwrap_or("/").to_string());
  if let Some(domain) = value["domain"].as_str() {
    builder = builder.domain(domain.to_string());
  }
  builder = builder
    .secure(value["secure"].as_bool().unwrap_or(false))
    .http_only(value["httpOnly"].as_bool().unwrap_or(false));
  if let Some(same_site) = value["sameSite"].as_str() {
    builder = builder.same_site(same_site_name_to_same_site(same_site));
  }
  Some(builder.build())
}

fn same_site_name(value: Option<cookie::SameSite>) -> Option<&'static str> {
  value.map(|value| match value {
    cookie::SameSite::Strict => "strict",
    cookie::SameSite::Lax => "lax",
    cookie::SameSite::None => "no_restriction",
  })
}

fn same_site_name_to_same_site(value: &str) -> cookie::SameSite {
  match value {
    "strict" => cookie::SameSite::Strict,
    "no_restriction" => cookie::SameSite::None,
    _ => cookie::SameSite::Lax,
  }
}

fn css_color(color: tauri_utils::config::Color) -> String {
  format!(
    "rgba({}, {}, {}, {:.3})",
    color.0,
    color.1,
    color.2,
    color.3 as f64 / 255.0
  )
}
