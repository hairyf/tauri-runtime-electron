//! Tauri adapters for the upstream-owned Electron protocol.

use electron_rs::RpcRequest;
pub(crate) use electron_rs::rpc::{app, protocol, webview, window};
#[cfg(target_os = "macos")]
pub(crate) use app::{
  Focus as AppFocus, Hide as AppHide, SetActivationPolicy as AppActivationPolicy,
  SetDockVisibility as AppDockVisibility, Show as AppShow,
};
pub(crate) use app::{SetBadgeLabel as AppBadgeLabel, SetTheme as AppTheme};
#[cfg(target_os = "linux")]
pub(crate) use window::Ready as WindowReady;
pub(crate) use protocol::{Register as ProtocolRegister, Respond as ProtocolRespond};
pub(crate) use webview::{
  Bounds as WebviewBounds, CanGoBack as WebviewCanGoBack, CanGoForward as WebviewCanGoForward,
  ClearBrowsingData as WebviewClearBrowsingData, Close as WebviewClose,
  CloseDevtools as WebviewCloseDevtools, Cookies as WebviewCookies,
  DeleteCookie as WebviewDeleteCookie, DownloadResponse, Eval as EvalScript,
  Eval as EvalScriptCallback, Focus as WebviewFocus, GoBack as WebviewGoBack,
  GoForward as WebviewGoForward, IsDevtoolsOpen as WebviewIsDevtoolsOpen,
  Navigate as WebviewNavigate, NavigationResponse, NewWindowResponse,
  OpenDevtools as WebviewOpenDevtools, PermissionResponse, Print as WebviewPrint,
  Reload as WebviewReload, Reparent as WebviewReparent, SetAutoResize as WebviewSetAutoResize,
  SetBackgroundColor as WebviewBackgroundColor, SetBounds as WebviewSetBounds,
  SetCookie as WebviewSetCookie, SetVisibility as WebviewVisibility,
  SetZoom as WebviewSetZoom, Url as WebviewUrl,
};
pub(crate) use window::{
  Center as WindowCenter, Close as WindowClose, Create as WindowCreate,
  CreationIcon as WindowIconData, CurrentDisplay as WindowCurrentDisplay, CursorPosition,
  Destroy as WindowDestroy, DisplayForPoint, Displays, Focus as WindowFocus, Geometry,
  Hide as WindowHide, IgnoreCursorEvents as WindowIgnoreCursor, IsEnabled as WindowEnabledQuery,
  Maximize as WindowMaximize, Minimize as WindowMinimize, NativeHandle, QueryTheme as WindowTheme,
  RequestAttention as WindowAttention, SetAlwaysOnTop as WindowAlwaysOnTop,
  SetBackgroundColor as WindowBackgroundColor, SetBackgroundMaterial as BackgroundMaterial,
  SetBadgeCount as WindowBadgeCount, SetClosable as WindowClosable,
  SetContentProtected as WindowContentProtected, SetCursorIcon as WindowCursorIcon,
  SetCursorVisible as WindowCursorVisible, SetDecorations as WindowDecorations,
  SetEnabled as WindowEnabled, SetFocusable as WindowFocusable, SetFullscreen as WindowFullscreen,
  SetIcon as WindowIcon, SetMaxSize as WindowMaxSize, SetMaximizable as WindowMaximizable,
  SetMenu as WindowSetMenu, SetMinSize as WindowMinSize, SetMinimizable as WindowMinimizable,
  SetOverlayIcon as WindowOverlayIcon, SetPosition as WindowPosition,
  SetProgressBar as WindowProgressBar, SetResizable as WindowResizable, SetShadow as WindowShadow,
  SetSimpleFullscreen as WindowSimpleFullscreen, SetSize as WindowSize,
  SetSizeConstraints as WindowSizeConstraints, SetSkipTaskbar as WindowSkipTaskbar,
  SetTheme as WindowSetTheme, SetTitle as WindowTitle, SetTitleBarStyle as WindowTitleBarStyle,
  SetTrafficLight as WindowTrafficLight, SetVisibleOnAllWorkspaces as WindowVisibleOnAllWorkspaces,
  Show as WindowShow, Unmaximize as WindowUnmaximize, Unminimize as WindowUnminimize,
};

/// Bind upstream requests to the runtime's tracked Tauri window identity.
pub(crate) trait WindowRequest: RpcRequest {
  fn set_window_id(&mut self, id: u32);
}
macro_rules! window_requests {
  ($($request:ty),* $(,)?) => {$(
    impl WindowRequest for $request {
      fn set_window_id(&mut self, id: u32) { self.window_id = id; }
    }
  )*};
}
window_requests!(
  BackgroundMaterial, Geometry, WindowBackgroundColor, WindowCenter, WindowClosable,
  WindowDecorations, WindowEnabled, WindowFocus, WindowFocusable, WindowFullscreen,
  WindowHide, WindowIcon, WindowIgnoreCursor, WindowMaxSize, WindowMaximizable,
  WindowMaximize, WindowMinSize, WindowMinimizable, WindowMinimize, WindowPosition,
  WindowAttention, WindowResizable, WindowCursorIcon, WindowCursorVisible, WindowTitle,
  WindowShadow, WindowShow, WindowSimpleFullscreen, WindowSize, WindowSizeConstraints,
  WindowSkipTaskbar, WindowTheme, WindowSetTheme, WindowEnabledQuery, WindowCurrentDisplay,
  WindowTrafficLight, WindowUnmaximize, WindowUnminimize, WindowClose, WindowDestroy,
  WindowAlwaysOnTop, WindowVisibleOnAllWorkspaces, WindowContentProtected,
  WindowProgressBar, WindowOverlayIcon, WindowTitleBarStyle,
);

pub(crate) fn title_bar_style(style: tauri_utils::TitleBarStyle) -> tauri_runtime::Result<window::TitleBarStyle> {
  match style {
    tauri_utils::TitleBarStyle::Visible => Ok(window::TitleBarStyle::Visible),
    tauri_utils::TitleBarStyle::Transparent => Ok(window::TitleBarStyle::Transparent),
    tauri_utils::TitleBarStyle::Overlay => Ok(window::TitleBarStyle::Overlay),
    _ => Err(crate::runtime::create_window_err("unsupported title bar style".into())),
  }
}

pub(crate) fn window_size(size: tauri_runtime::dpi::Size) -> WindowSize {
  use tauri_runtime::dpi::Size;
  let (width, height, physical) = match size {
    Size::Physical(size) => (f64::from(size.width), f64::from(size.height), true),
    Size::Logical(size) => (size.width, size.height, false),
  };
  WindowSize { window_id: 0, width, height, physical }
}

macro_rules! optional_size {
  ($function:ident, $request:ident) => {
    pub(crate) fn $function(size: Option<tauri_runtime::dpi::Size>) -> $request {
      let size = size.map(window_size);
      $request {
        window_id: 0,
        width: size.as_ref().map(|size| size.width),
        height: size.as_ref().map(|size| size.height),
        physical: size.is_some_and(|size| size.physical),
      }
    }
  };
}
optional_size!(window_min_size, WindowMinSize);
optional_size!(window_max_size, WindowMaxSize);

macro_rules! position {
  ($function:ident, $request:ident) => {
    pub(crate) fn $function(position: tauri_runtime::dpi::Position) -> $request {
      use tauri_runtime::dpi::Position;
      let (x, y, physical) = match position {
        Position::Physical(position) => (f64::from(position.x), f64::from(position.y), true),
        Position::Logical(position) => (position.x, position.y, false),
      };
      $request { window_id: 0, x, y, physical }
    }
  };
}
position!(window_position, WindowPosition);
position!(window_traffic_light, WindowTrafficLight);

#[cfg(test)]
mod tests {
  use super::*;
  use tauri_runtime::dpi::{LogicalSize, PhysicalSize, Size};

  #[test]
  fn size_adapters_preserve_units_and_clear_both_limits() {
    let physical = window_min_size(Some(Size::Physical(PhysicalSize::new(640, 480))));
    assert_eq!(serde_json::to_value(physical).unwrap(), serde_json::json!({
      "windowId": 0, "width": 640.0, "height": 480.0, "physical": true,
    }));
    let logical = window_max_size(Some(Size::Logical(LogicalSize::new(100.5, 200.5))));
    assert!(!logical.physical);
    assert_eq!(logical.width, Some(100.5));
    assert_eq!(serde_json::to_value(window_min_size(None)).unwrap(),
      serde_json::json!({"windowId": 0, "physical": false}));
  }
}
