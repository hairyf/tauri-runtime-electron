// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

use crate::runtime::{
  ElectronRuntime, Message, RuntimeContext, IPC_TIMEOUT, create_webview_err,
  position_payload, size_payload,
};
use serde_json::{json, Value};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use tauri_runtime::{
  Result, UserEvent, WebviewDispatch,
  dpi::{PhysicalPosition, PhysicalSize, Position, Size},
  window::WindowId,
};
use url::Url;

#[derive(Debug, Clone)]
pub struct ElectronWebviewDispatcher {
  pub(crate) id: u32,
  pub(crate) context: RuntimeContext,
  pub(crate) url: Arc<Mutex<String>>,
  pub(crate) last_evaluated_script: Arc<Mutex<Option<String>>>,
}

impl ElectronWebviewDispatcher {
  pub fn last_evaluated_script(&self) -> Option<String> {
    self.last_evaluated_script.lock().unwrap().clone()
  }

  /// Queries the guest's physical-pixel rect from the Electron side.
  fn rectangle(&self) -> Result<tauri_runtime::dpi::Rect> {
    let bounds = self.webview_call("webview.bounds", json!({}))?;
    Ok(rect_from_value(&bounds["bounds"]))
  }

}

impl ElectronWebviewDispatcher {
  // Tauri 2's `WebviewDispatch` has no history methods (the webview plugin drives
  // them through `eval`), so these stay inherent for direct use.
  pub fn go_back(&self) -> Result<()> {
    self.webview_call("webview.goBack", json!({})).map(|_| ())
  }

  pub fn can_go_back(&self) -> Result<bool> {
    Ok(self
      .webview_call("webview.canGoBack", json!({}))?
      .as_bool()
      .unwrap_or(false))
  }

  pub fn go_forward(&self) -> Result<()> {
    self.webview_call("webview.goForward", json!({})).map(|_| ())
  }

  pub fn can_go_forward(&self) -> Result<bool> {
    Ok(self
      .webview_call("webview.canGoForward", json!({}))?
      .as_bool()
      .unwrap_or(false))
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
    let id = self.context.next_webview_event_id();
    self.context.add_webview_event_handler(self.id, id, Rc::new(f));
    id
  }

  /// Electron guests have no native handle to hand out: they are `<webview>`
  /// elements composited by the renderer, so callers get `()` boxed.
  fn with_webview<F: FnOnce(Box<dyn std::any::Any>) + Send + 'static>(&self, f: F) -> Result<()> {
    crate::runtime::unsupported(
      "WebviewDispatch::with_webview",
      "Electron guests have no native handle to hand out",
    );
    f(Box::new(()));
    Ok(())
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
      .webview_call("webview.setZoom", json!({ "scaleFactor": scale_factor }))
      .map(|_| ())
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
    self.rectangle()
  }

  fn position(&self) -> Result<PhysicalPosition<i32>> {
    Ok(self.rectangle()?.position.to_physical::<i32>(1.0))
  }

  fn size(&self) -> Result<PhysicalSize<u32>> {
    Ok(self.rectangle()?.size.to_physical::<u32>(1.0))
  }

  fn navigate(&self, url: Url) -> Result<()> {
    *self.url.lock().unwrap() = url.to_string();
    self
      .context
      .set_webview_origin_by_serial(self.id, url.as_str());
    self
      .webview_call("webview.navigate", json!({ "url": url.to_string() }))
      .map(|_| ())
  }

  fn reload(&self) -> Result<()> {
    self.webview_call("webview.reload", json!({})).map(|_| ())
  }

  fn print(&self) -> Result<()> {
    self.webview_call("webview.print", json!({})).map(|_| ())
  }

  fn close(&self) -> Result<()> {
    self.webview_call("webview.close", json!({})).map(|_| ())
  }

  fn set_bounds(&self, bounds: tauri_runtime::dpi::Rect) -> Result<()> {
    self.webview_call("webview.setBounds", json!({
      "position": position_payload(bounds.position),
      "size": size_payload(bounds.size),
    })).map(|_| ())
  }

  fn set_size(&self, size: Size) -> Result<()> {
    self.webview_call("webview.setBounds", json!({ "size": size_payload(size) })).map(|_| ())
  }

  fn set_position(&self, position: Position) -> Result<()> {
    self.webview_call("webview.setBounds", json!({ "position": position_payload(position) })).map(|_| ())
  }

  fn set_focus(&self) -> Result<()> {
    self.webview_call("webview.focus", json!({})).map(|_| ())
  }

  /// A `<webview>` guest is anchored to the page that hosts it; Electron has no
  /// API to move a guest into another window.
  fn reparent(&self, _window_id: WindowId) -> Result<()> {
    crate::runtime::unsupported(
      "WebviewDispatch::reparent",
      "an Electron <webview> guest cannot move to another window",
    );
    Ok(())
  }

  fn cookies(&self) -> Result<Vec<tauri_runtime::Cookie<'static>>> {
    let cookies = self.webview_call("webview.cookies", json!({}))?;
    Ok(parse_cookies(&cookies))
  }

  fn cookies_for_url(&self, url: Url) -> Result<Vec<tauri_runtime::Cookie<'static>>> {
    let cookies = self.webview_call("webview.cookies", json!({ "url": url.to_string() }))?;
    Ok(parse_cookies(&cookies))
  }

  fn set_cookie(&self, cookie: tauri_runtime::Cookie<'_>) -> Result<()> {
    let url = self.cookie_url(&cookie);
    self
      .webview_call(
        "webview.setCookie",
        json!({
          "url": url,
          "name": cookie.name(),
          "value": cookie.value(),
          "domain": cookie.domain(),
          "path": cookie.path(),
          "secure": cookie.secure().unwrap_or(false),
          "httpOnly": cookie.http_only(),
          "sameSite": same_site_name(cookie.same_site()),
        }),
      )
      .map(|_| ())
  }

  fn delete_cookie(&self, cookie: tauri_runtime::Cookie<'_>) -> Result<()> {
    let url = self.cookie_url(&cookie);
    self
      .webview_call(
        "webview.deleteCookie",
        json!({ "url": url, "name": cookie.name() }),
      )
      .map(|_| ())
  }

  /// Guests follow their CSS box; Electron's own auto-resize is forwarded for
  /// hosts that want the contents to track the window instead.
  fn set_auto_resize(&self, auto_resize: bool) -> Result<()> {
    self
      .webview_call("webview.setAutoResize", json!({ "autoResize": auto_resize }))
      .map(|_| ())
  }

  fn clear_all_browsing_data(&self) -> Result<()> {
    self
      .webview_call("webview.clearBrowsingData", json!({}))
      .map(|_| ())
  }

  fn hide(&self) -> Result<()> {
    self
      .webview_call("webview.visibility", json!({ "visible": false }))
      .map(|_| ())
  }

  fn show(&self) -> Result<()> {
    self
      .webview_call("webview.visibility", json!({ "visible": true }))
      .map(|_| ())
  }

  fn set_background_color(&self, color: Option<tauri_utils::config::Color>) -> Result<()> {
    self
      .webview_call("webview.backgroundColor", json!({ "color": color.map(css_color) }))
      .map(|_| ())
  }
}

impl ElectronWebviewDispatcher {
  /// Electron cookies are stored against a URL; derive one from the cookie.
  fn cookie_url(&self, cookie: &tauri_runtime::Cookie<'_>) -> String {
    if let Some(domain) = cookie.domain() {
      let scheme = if cookie.secure().unwrap_or(false) { "https" } else { "http" };
      return format!("{scheme}://{domain}/");
    }
    self
      .url
      .lock()
      .unwrap()
      .parse::<Url>()
      .map(|url| format!("{}://{}/", url.scheme(), url.host_str().unwrap_or("localhost")))
      .unwrap_or_else(|_| "http://localhost/".into())
  }
}

impl ElectronWebviewDispatcher {
  pub(crate) fn webview_call(&self, method: &str, mut params: Value) -> Result<Value> {
    if let Some(map) = params.as_object_mut() {
      map.insert("webviewId".into(), json!(self.id));
    }
    self
      .context
      .ipc
      .call(method, params, IPC_TIMEOUT)
      .map_err(create_webview_err)
  }
}

/// Converts the physical-pixel rect reported by `electron/main.cjs`.
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

fn parse_cookies(value: &Value) -> Vec<tauri_runtime::Cookie<'static>> {
  value
    .as_array()
    .map(|cookies| cookies.iter().filter_map(cookie_from_value).collect())
    .unwrap_or_default()
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
