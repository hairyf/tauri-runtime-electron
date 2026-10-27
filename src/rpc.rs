use electron_rs::RpcRequest;
use serde::{Deserialize, Serialize};
use std::time::Duration;

macro_rules! request {
  ($name:ident, $response:ty, $method:literal { $($(#[$meta:meta])* $field:ident : $type:ty),* $(,)? }) => {
    #[derive(Debug, Serialize)]
    #[serde(rename_all = "camelCase")]
    pub(crate) struct $name { $($(#[$meta])* pub(crate) $field: $type),* }
    impl RpcRequest for $name {
      type Response = $response;
      const METHOD: &'static str = $method;
      fn timeout(&self) -> Duration { Duration::from_secs(15) }
    }
  };
  ($name:ident, $response:ty, $method:literal) => {
    #[derive(Debug, Serialize)]
    pub(crate) struct $name {}
    impl RpcRequest for $name {
      type Response = $response;
      const METHOD: &'static str = $method;
      fn timeout(&self) -> Duration { Duration::from_secs(15) }
    }
  };
}

pub(crate) trait WindowRequest: RpcRequest {
  fn set_window_id(&mut self, id: u32);
}
macro_rules! window_request {
  ($name:ident, $response:ty, $method:literal { $($(#[$meta:meta])* $field:ident : $type:ty),* $(,)? }) => {
    #[derive(Debug, Serialize)]
    #[serde(rename_all = "camelCase")]
    pub(crate) struct $name { pub(crate) window_id: u32, $($(#[$meta])* pub(crate) $field: $type),* }
    impl RpcRequest for $name {
      type Response = $response;
      const METHOD: &'static str = $method;
      fn timeout(&self) -> Duration { Duration::from_secs(15) }
    }
    impl WindowRequest for $name { fn set_window_id(&mut self, id: u32) { self.window_id = id; } }
  };
  ($name:ident, $response:ty, $method:literal) => {
    window_request!($name, $response, $method {});
  };
}

request!(AppTheme, (), "app.theme" { theme: &'static str });
request!(AppFocus, (), "app.focus" { steal: bool });
request!(AppShow, (), "app.show");
request!(AppHide, (), "app.hide");
request!(AppActivationPolicy, (), "app.activationPolicy" { policy: &'static str });
request!(AppDockVisibility, (), "app.dockVisibility" { visible: bool });
request!(AppBadgeLabel, (), "app.badgeLabel" { label: Option<String> });
request!(Displays, DisplayList, "window.displays");
request!(DisplayForPoint, serde_json::Value, "window.displayForPoint" { x: f64, y: f64 });
request!(CursorPosition, Point, "window.cursorPosition");
window_request!(WindowDestroy, (), "window.destroy");
request!(WindowReady, (), "window.ready" { window_id: u32 });
request!(WebviewReparent, (), "webview.reparent" { webview_id: u32, window_id: u32 });
request!(WindowSetMenu, (), "window.setMenu" { window_id: u32, menu: Option<Menu> });
request!(ProtocolRegister, (), "protocol.register" { scheme: String });
request!(ProtocolRespond, (), "protocol.respond" { id: u64, status: u16, headers: serde_json::Map<String, serde_json::Value>, body: String });
request!(NewWindowResponse, (), "webview.newWindowResponse" { id: u64, response: serde_json::Value });
request!(NavigationResponse, (), "webview.navigationResponse" { id: u64, allow: bool });
request!(DownloadResponse, (), "webview.downloadResponse" { id: u64, allow: bool, destination: String });
request!(PermissionResponse, (), "webview.permissionResponse" { id: u64, response: &'static str });

#[derive(Debug, Serialize)]
pub(crate) struct Menu {
  pub(crate) items: Vec<serde_json::Value>,
}
#[derive(Debug, Deserialize)]
pub(crate) struct DisplayList {
  pub(crate) primary: Option<serde_json::Value>,
  #[serde(default)]
  pub(crate) displays: Vec<serde_json::Value>,
}
#[derive(Debug, Deserialize)]
pub(crate) struct Point {
  pub(crate) x: f64,
  pub(crate) y: f64,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NativeHandleResponse {
  pub(crate) handle: i64,
  #[serde(default)]
  pub(crate) process_id: Option<u32>,
}

window_request!(NativeHandle, NativeHandleResponse, "window.nativeHandle");
window_request!(BackgroundMaterial, (), "window.backgroundMaterial" { material: String });
window_request!(Geometry, serde_json::Value, "window.geometry");
window_request!(WindowBackgroundColor, (), "window.backgroundColor" { color: Option<String> });
window_request!(WindowBadgeCount, (), "window.badgeCount" { count: i64 });
window_request!(WindowCenter, (), "window.center");
window_request!(WindowClosable, (), "window.closable" { closable: bool });
window_request!(WindowDecorations, (), "window.decorations" { value: bool });
window_request!(WindowEnabled, (), "window.enabled" { enabled: bool });
window_request!(WindowFocus, (), "window.focus");
window_request!(WindowFocusable, (), "window.focusable" { focusable: bool });
window_request!(WindowFullscreen, (), "window.fullscreen" { fullscreen: bool, #[serde(skip_serializing_if = "Option::is_none")] x: Option<f64>, #[serde(skip_serializing_if = "Option::is_none")] y: Option<f64> });
window_request!(WindowHide, (), "window.hide");
window_request!(WindowIcon, (), "window.icon" { icon: serde_json::Value });
window_request!(WindowIgnoreCursor, (), "window.ignoreCursorEvents" { ignore: bool });
window_request!(WindowMaxSize, (), "window.maxSize" { width: Option<serde_json::Value>, height: Option<serde_json::Value>, physical: Option<bool> });
window_request!(WindowMaximizable, (), "window.maximizable" { maximizable: bool });
window_request!(WindowMaximize, (), "window.maximize");
window_request!(WindowMinSize, (), "window.minSize" { width: Option<serde_json::Value>, height: Option<serde_json::Value>, physical: Option<bool> });
window_request!(WindowMinimizable, (), "window.minimizable" { minimizable: bool });
window_request!(WindowMinimize, (), "window.minimize");
window_request!(WindowPosition, (), "window.position" { x: f64, y: f64, physical: bool });
window_request!(WindowAttention, (), "window.requestAttention" { flash: bool });
window_request!(WindowResizable, (), "window.resizable" { resizable: bool });
window_request!(WindowCursorIcon, (), "window.setCursorIcon" { cursor: String });
window_request!(WindowCursorVisible, (), "window.setCursorVisible" { visible: bool });
window_request!(WindowTitle, (), "window.setTitle" { title: String });
window_request!(WindowShadow, (), "window.shadow" { value: bool });
window_request!(WindowShow, (), "window.show");
window_request!(WindowSimpleFullscreen, (), "window.simpleFullscreen" { fullscreen: bool });
window_request!(WindowSize, (), "window.size" { width: f64, height: f64, physical: bool });
window_request!(WindowSizeConstraints, (), "window.sizeConstraints" { constraints: serde_json::Value });
window_request!(WindowSkipTaskbar, (), "window.skipTaskbar" { skip: bool });
window_request!(WindowTheme, serde_json::Value, "window.themeQuery");
window_request!(WindowSetTheme, (), "window.theme" { theme: &'static str });
window_request!(WindowEnabledQuery, bool, "window.enabledQuery");
window_request!(
  WindowCurrentDisplay,
  serde_json::Value,
  "window.currentDisplay"
);
window_request!(WindowTrafficLight, (), "window.trafficLight" { x: f64, y: f64, physical: bool });
window_request!(WindowUnmaximize, (), "window.unmaximize");
window_request!(WindowUnminimize, (), "window.unminimize");
window_request!(WindowClose, (), "window.close");
window_request!(WindowAlwaysOnTop, (), "window.alwaysOnTop" { always_on_top: bool });
window_request!(WindowVisibleOnAllWorkspaces, (), "window.visibleOnAllWorkspaces" { value: bool });
window_request!(WindowContentProtected, (), "window.contentProtected" { protected: bool });
window_request!(WindowProgressBar, (), "window.progressBar" { state: &'static str, progress: Option<f64>, desktop_filename: Option<String> });
window_request!(WindowOverlayIcon, (), "window.overlayIcon" { icon: Option<serde_json::Value> });
window_request!(WindowTitleBarStyle, (), "window.titleBarStyle" { style: tauri_utils::TitleBarStyle });

request!(WebviewBounds, serde_json::Value, "webview.bounds" { webview_id: u32 });
request!(WebviewGoBack, (), "webview.goBack" { webview_id: u32 });
request!(WebviewCanGoBack, bool, "webview.canGoBack" { webview_id: u32 });
request!(WebviewGoForward, (), "webview.goForward" { webview_id: u32 });
request!(WebviewCanGoForward, bool, "webview.canGoForward" { webview_id: u32 });
request!(WebviewSetZoom, (), "webview.setZoom" { webview_id: u32, scale_factor: f64 });
request!(WebviewNavigate, (), "webview.navigate" { webview_id: u32, url: String });
request!(WebviewReload, (), "webview.reload" { webview_id: u32 });
request!(WebviewPrint, (), "webview.print" { webview_id: u32 });
request!(WebviewClose, (), "webview.close" { webview_id: u32 });
request!(WebviewSetBounds, (), "webview.setBounds" { webview_id: u32, position: Option<serde_json::Value>, size: Option<serde_json::Value> });
request!(WebviewFocus, (), "webview.focus" { webview_id: u32 });
request!(WebviewCookies, Vec<serde_json::Value>, "webview.cookies" { webview_id: u32, url: Option<String> });
request!(WebviewSetCookie, (), "webview.setCookie" { webview_id: u32, url: String, name: String, value: String, domain: Option<String>, path: Option<String>, secure: bool, http_only: Option<bool>, same_site: Option<&'static str> });
request!(WebviewDeleteCookie, (), "webview.deleteCookie" { webview_id: u32, url: String, name: String });
request!(WebviewSetAutoResize, (), "webview.setAutoResize" { webview_id: u32, auto_resize: bool });
request!(WebviewClearBrowsingData, (), "webview.clearBrowsingData" { webview_id: u32 });
request!(WebviewVisibility, (), "webview.visibility" { webview_id: u32, visible: bool });
request!(WebviewUrl, String, "webview.url" { webview_id: u32 });
request!(WebviewBackgroundColor, (), "webview.backgroundColor" { webview_id: u32, color: Option<String> });
request!(WebviewOpenDevtools, (), "webview.openDevtools" { webview_id: u32 });
request!(WebviewCloseDevtools, (), "webview.closeDevtools" { webview_id: u32 });
request!(WebviewIsDevtoolsOpen, bool, "webview.isDevtoolsOpen" { webview_id: u32 });

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EvalScript {
  pub(crate) webview_id: u32,
  pub(crate) script: String,
}
impl RpcRequest for EvalScript {
  type Response = ();
  const METHOD: &'static str = "webview.eval";
  fn deferred(&self) -> bool {
    true
  }
  fn notification(&self) -> bool {
    true
  }
  fn timeout(&self) -> Duration {
    Duration::from_secs(15)
  }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EvalScriptCallback {
  pub(crate) webview_id: u32,
  pub(crate) script: String,
}
impl RpcRequest for EvalScriptCallback {
  type Response = serde_json::Value;
  const METHOD: &'static str = "webview.eval";
  fn deferred(&self) -> bool {
    true
  }
  fn timeout(&self) -> Duration {
    Duration::from_secs(15)
  }
}

impl WindowSize {
  pub(crate) fn from_size(size: tauri_runtime::dpi::Size) -> Self {
    use tauri_runtime::dpi::Size;
    let (width, height, physical) = match size {
      Size::Physical(size) => (f64::from(size.width), f64::from(size.height), true),
      Size::Logical(size) => (size.width, size.height, false),
    };
    Self {
      window_id: 0,
      width,
      height,
      physical,
    }
  }
}
macro_rules! optional_size {
  ($name:ident) => {
    impl $name {
      pub(crate) fn from_size(size: Option<tauri_runtime::dpi::Size>) -> Self {
        let size = size.map(WindowSize::from_size);
        Self {
          window_id: 0,
          width: size.as_ref().map(|s| s.width.into()),
          height: size.as_ref().map(|s| s.height.into()),
          physical: size.map(|s| s.physical),
        }
      }
    }
  };
}
optional_size!(WindowMinSize);
optional_size!(WindowMaxSize);
macro_rules! position {
  ($name:ident) => {
    impl $name {
      pub(crate) fn from_position(position: tauri_runtime::dpi::Position) -> Self {
        use tauri_runtime::dpi::Position;
        let (x, y, physical) = match position {
          Position::Physical(p) => (f64::from(p.x), f64::from(p.y), true),
          Position::Logical(p) => (p.x, p.y, false),
        };
        Self {
          window_id: 0,
          x,
          y,
          physical,
        }
      }
    }
  };
}
position!(WindowPosition);
position!(WindowTrafficLight);

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreatedWindow {
  pub(crate) window_id: u32,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreatedWebview {
  pub(crate) webview_id: u32,
}

request!(WindowCreate, CreatedWindow, "window.create" {
  window_id: u32,
  label: String,
  window: tauri_utils::config::WindowConfig,
  size_constraints: Option<serde_json::Value>,
  window_icon: Option<WindowIconData>,
  icon: Option<String>,
  defer_show: bool,
  #[serde(skip_serializing_if = "Option::is_none")]
  new_window_request: Option<u64>,
  webview: Option<WebviewCreate>,
});
#[derive(Debug, Serialize)]
pub(crate) struct WindowIconData {
  pub(crate) width: u32,
  pub(crate) height: u32,
  pub(crate) bgra: String,
}
request!(WebviewCreate, CreatedWebview, "webview.create" {
  webview_id: u32,
  window_id: Option<u32>,
  url: String,
  bounds: Option<serde_json::Value>,
  auto_resize: bool,
  init_scripts: Vec<serde_json::Value>,
  data_directory: Option<std::path::PathBuf>,
  incognito: bool,
  user_agent: Option<String>,
  accept_first_mouse: bool,
  javascript_disabled: bool,
  devtools: Option<bool>,
  background_throttling: Option<tauri_utils::config::BackgroundThrottlingPolicy>,
  drag_drop_enabled: bool,
  zoom_hotkeys_enabled: bool,
  scroll_bar_style: &'static str,
  transparent: bool,
  focus: bool,
  background_color: Option<tauri_utils::config::Color>,
  policy: serde_json::Value,
});

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn typed_requests_preserve_optional_wire_fields_and_response_types() {
    let fullscreen = serde_json::to_value(WindowFullscreen {
      window_id: 7,
      fullscreen: true,
      x: None,
      y: None,
    })
    .unwrap();
    assert_eq!(
      fullscreen,
      serde_json::json!({ "windowId": 7, "fullscreen": true })
    );
    let request = WindowCreate {
      window_id: 7,
      label: "main".into(),
      window: Default::default(),
      size_constraints: None,
      window_icon: None,
      icon: None,
      defer_show: false,
      new_window_request: None,
      webview: None,
    };
    assert!(!serde_json::to_value(request)
      .unwrap()
      .as_object()
      .unwrap()
      .contains_key("newWindowRequest"));
    let window: <WindowCreate as RpcRequest>::Response =
      serde_json::from_value(serde_json::json!({ "windowId": 7 })).unwrap();
    let webview: <WebviewCreate as RpcRequest>::Response =
      serde_json::from_value(serde_json::json!({ "webviewId": 8 })).unwrap();
    assert_eq!((window.window_id, webview.webview_id), (7, 8));
    let handle: NativeHandleResponse =
      serde_json::from_value(serde_json::json!({ "handle": 17, "processId": 27 })).unwrap();
    assert_eq!(handle.process_id, Some(27));
    let eval = EvalScript {
      webview_id: 8,
      script: "1".into(),
    };
    let callback = EvalScriptCallback {
      webview_id: 8,
      script: "1".into(),
    };
    assert!(
      eval.deferred() && eval.notification() && callback.deferred() && !callback.notification()
    );
  }
}
