// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

use crate::{
  platform::monitor_from_value,
  runtime::{
    ElectronRuntime, Message, RuntimeContext, IPC_TIMEOUT, create_webview_impl,
    create_window_err, create_window_impl, position_payload, rect_position, rect_size,
    size_payload,
  },
  window_builder::ElectronWindowBuilder,
};
use serde_json::{json, Value};
use std::rc::Rc;
use tauri_runtime::{
  Icon, ProgressBarState, ProgressBarStatus, Result, UserAttentionType, UserEvent, WindowDispatch,
  WindowEventId,
  dpi::{PhysicalPosition, PhysicalSize, PixelUnit, Position, Size},
  monitor::Monitor,
  webview::{DetachedWebview, PendingWebview},
  window::{CursorIcon, DetachedWindow, PendingWindow, RawWindow, WindowEvent, WindowId, WindowSizeConstraints},
};
use tauri_utils::Theme;

#[derive(Debug, Clone)]
pub struct ElectronWindowDispatcher {
  pub(crate) id: WindowId,
  pub(crate) context: RuntimeContext,
}

impl ElectronWindowDispatcher {
  fn window_call(&self, method: &str, params: Value) -> Result<()> {
    self.window_call_value(method, params).map(|_| ())
  }

  fn window_call_value(&self, method: &str, mut params: Value) -> Result<Value> {
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
      .map_err(create_window_err)
  }

  /// Queries the live Electron window geometry and state.
  fn geometry(&self) -> Result<Value> {
    self.window_call_value("window.geometry", json!({}))
  }
}

impl<T: UserEvent> WindowDispatch<T> for ElectronWindowDispatcher {
  type Runtime = ElectronRuntime;

  type WindowBuilder = ElectronWindowBuilder;

  fn run_on_main_thread<F: FnOnce() + Send + 'static>(&self, f: F) -> Result<()> {
    self.context.send_message(Message::Task(Box::new(f)))
  }

  fn on_window_event<F: Fn(&WindowEvent) + Send + 'static>(&self, f: F) -> WindowEventId {
    let id = self.context.next_window_event_id();
    self.context.add_window_event_handler(self.id, id, Rc::new(f));
    id
  }

  fn scale_factor(&self) -> Result<f64> {
    Ok(self.geometry()?["scaleFactor"].as_f64().unwrap_or(1.0))
  }

  fn inner_position(&self) -> Result<PhysicalPosition<i32>> {
    Ok(rect_position(&self.geometry()?["contentBounds"]))
  }

  fn outer_position(&self) -> Result<PhysicalPosition<i32>> {
    Ok(rect_position(&self.geometry()?["bounds"]))
  }

  fn inner_size(&self) -> Result<PhysicalSize<u32>> {
    Ok(rect_size(&self.geometry()?["contentBounds"]))
  }

  fn outer_size(&self) -> Result<PhysicalSize<u32>> {
    Ok(rect_size(&self.geometry()?["bounds"]))
  }

  fn is_fullscreen(&self) -> Result<bool> {
    Ok(self.geometry()?["fullscreen"].as_bool().unwrap_or(false))
  }

  fn is_minimized(&self) -> Result<bool> {
    Ok(self.geometry()?["minimized"].as_bool().unwrap_or(false))
  }

  fn is_maximized(&self) -> Result<bool> {
    Ok(self.geometry()?["maximized"].as_bool().unwrap_or(false))
  }

  fn is_focused(&self) -> Result<bool> {
    Ok(self.geometry()?["focused"].as_bool().unwrap_or(false))
  }

  fn is_decorated(&self) -> Result<bool> {
    Ok(self.geometry()?["decorated"].as_bool().unwrap_or(true))
  }

  fn is_resizable(&self) -> Result<bool> {
    Ok(self.geometry()?["resizable"].as_bool().unwrap_or(true))
  }

  fn is_maximizable(&self) -> Result<bool> {
    Ok(self.geometry()?["maximizable"].as_bool().unwrap_or(true))
  }

  fn is_minimizable(&self) -> Result<bool> {
    Ok(self.geometry()?["minimizable"].as_bool().unwrap_or(true))
  }

  fn is_closable(&self) -> Result<bool> {
    Ok(self.geometry()?["closable"].as_bool().unwrap_or(true))
  }

  fn is_visible(&self) -> Result<bool> {
    Ok(self.geometry()?["visible"].as_bool().unwrap_or(true))
  }

  fn title(&self) -> Result<String> {
    Ok(self
      .geometry()?
      .get("title")
      .and_then(Value::as_str)
      .unwrap_or_default()
      .to_string())
  }

  fn current_monitor(&self) -> Result<Option<Monitor>> {
    Ok(monitor_from_value(
      &self.window_call_value("window.currentDisplay", json!({}))?,
    ))
  }

  fn primary_monitor(&self) -> Result<Option<Monitor>> {
    let displays = self.window_call_value("window.displays", json!({}))?;
    Ok(monitor_from_value(&displays["primary"]))
  }

  fn monitor_from_point(&self, x: f64, y: f64) -> Result<Option<Monitor>> {
    Ok(monitor_from_value(
      &self.window_call_value("window.displayForPoint", json!({ "x": x, "y": y }))?,
    ))
  }

  fn available_monitors(&self) -> Result<Vec<Monitor>> {
    let displays = self.window_call_value("window.displays", json!({}))?;
    Ok(
      displays["displays"]
        .as_array()
        .map(|list| list.iter().filter_map(monitor_from_value).collect())
        .unwrap_or_default(),
    )
  }

  fn theme(&self) -> Result<Theme> {
    let query = self.window_call_value("window.themeQuery", json!({}))?;
    Ok(match query["theme"].as_str() {
      Some("dark") => Theme::Dark,
      _ => Theme::Light,
    })
  }

  #[cfg(any(
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd"
  ))]
  fn gtk_window(&self) -> Result<*mut std::ffi::c_void> {
    Err(crate::runtime::create_window_err(
      "Electron draws no GTK window".into(),
    ))
  }

  #[cfg(any(
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd"
  ))]
  fn default_vbox(&self) -> Result<*mut std::ffi::c_void> {
    Err(crate::runtime::create_window_err(
      "Electron draws no GTK box".into(),
    ))
  }

  #[cfg(target_os = "android")]
  fn activity_name(&self) -> Result<String> {
    Err(crate::runtime::create_window_err(
      "Electron does not run on Android".into(),
    ))
  }

  #[cfg(target_os = "ios")]
  fn scene_identifier(&self) -> Result<String> {
    Err(crate::runtime::create_window_err(
      "Electron does not run on iOS".into(),
    ))
  }

  fn window_handle(
    &self,
  ) -> std::result::Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
    // Electron owns the window, but it will tell us its native handle; Tauri's
    // host-side code (muda menus, window effects, Win32 tweaks) needs it.
    let handle = self
      .context
      .native_handle(self.id)
      .map_err(|_| raw_window_handle::HandleError::Unavailable)?;
    let handle = std::num::NonZeroIsize::new(handle)
      .ok_or(raw_window_handle::HandleError::Unavailable)?;
    // SAFETY: the handle stays valid while the window exists, and Tauri uses it
    // synchronously from the main thread.
    #[cfg(windows)]
    let raw = raw_window_handle::RawWindowHandle::Win32(
      raw_window_handle::Win32WindowHandle::new(handle),
    );
    #[cfg(target_os = "macos")]
    let raw = raw_window_handle::RawWindowHandle::AppKit(
      raw_window_handle::AppKitWindowHandle::new(
        std::ptr::NonNull::new(handle.get() as *mut std::ffi::c_void)
          .ok_or(raw_window_handle::HandleError::Unavailable)?,
      ),
    );
    #[cfg(all(unix, not(target_os = "macos")))]
    let raw = raw_window_handle::RawWindowHandle::Xlib(
      raw_window_handle::XlibWindowHandle::new(handle.get() as std::os::raw::c_ulong),
    );
    #[cfg(not(any(windows, unix)))]
    return Err(raw_window_handle::HandleError::Unavailable);

    #[allow(unreachable_code)]
    Ok(unsafe { raw_window_handle::WindowHandle::borrow_raw(raw) })
  }

  fn center(&self) -> Result<()> {
    self.window_call("window.center", json!({}))
  }

  fn request_user_attention(&self, request_type: Option<UserAttentionType>) -> Result<()> {
    let flash = request_type.is_some();
    self.window_call("window.requestAttention", json!({ "flash": flash }))
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
    self.window_call("window.resizable", json!({ "resizable": resizable }))
  }

  fn set_maximizable(&self, maximizable: bool) -> Result<()> {
    self.window_call("window.maximizable", json!({ "maximizable": maximizable }))
  }

  fn set_minimizable(&self, minimizable: bool) -> Result<()> {
    self.window_call("window.minimizable", json!({ "minimizable": minimizable }))
  }

  fn set_closable(&self, closable: bool) -> Result<()> {
    self.window_call("window.closable", json!({ "closable": closable }))
  }

  fn set_title<S: Into<String>>(&self, title: S) -> Result<()> {
    self.window_call("window.setTitle", json!({ "title": title.into() }))
  }

  fn maximize(&self) -> Result<()> {
    self.window_call("window.maximize", json!({}))
  }

  fn unmaximize(&self) -> Result<()> {
    self.window_call("window.unmaximize", json!({}))
  }

  fn minimize(&self) -> Result<()> {
    self.window_call("window.minimize", json!({}))
  }

  fn unminimize(&self) -> Result<()> {
    self.window_call("window.unminimize", json!({}))
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

  // Electron cannot flip the frame through its own API, but the window is a real
  // Win32 window: toggle the caption/thick-frame styles and let it repaint.
  fn set_decorations(&self, decorations: bool) -> Result<()> {
    #[cfg(windows)]
    {
      imp::set_decorations(self.context.native_handle(self.id)?, decorations);
      // Keep the main process' idea of the frame in sync for `is_decorated`.
      self.window_call("window.decorations", json!({ "decorations": decorations }))
    }
    #[cfg(not(windows))]
    {
      // A `BrowserWindow` frame is fixed at creation time.
      let _ = decorations;
      crate::unsupported::report("Window::set_decorations");
      Ok(())
    }
  }

  // Electron has no per-window drop shadow API.
  fn set_shadow(&self, shadow: bool) -> Result<()> {
    self.window_call("window.shadow", json!({ "value": shadow }))
  }

  // Electron has no "always at the bottom" flag; push the native window to the
  // bottom of the z-order when it is requested.
  fn set_always_on_bottom(&self, always_on_bottom: bool) -> Result<()> {
    #[cfg(windows)]
    if always_on_bottom {
      imp::send_to_bottom(self.context.native_handle(self.id)?);
    }
    #[cfg(not(windows))]
    let _ = always_on_bottom;
    Ok(())
  }

  fn set_always_on_top(&self, always_on_top: bool) -> Result<()> {
    self.window_call(
      "window.alwaysOnTop",
      json!({ "alwaysOnTop": always_on_top }),
    )
  }

  fn set_visible_on_all_workspaces(&self, visible_on_all_workspaces: bool) -> Result<()> {
    self.window_call(
      "window.visibleOnAllWorkspaces",
      json!({ "value": visible_on_all_workspaces }),
    )
  }

  fn set_content_protected(&self, protected: bool) -> Result<()> {
    self.window_call("window.contentProtected", json!({ "protected": protected }))
  }

  fn set_size(&self, size: Size) -> Result<()> {
    self.window_call("window.size", size_payload(size))
  }

  fn set_min_size(&self, size: Option<Size>) -> Result<()> {
    self.window_call("window.minSize", size.map(size_payload).unwrap_or(json!(null)))
  }

  fn set_max_size(&self, size: Option<Size>) -> Result<()> {
    self.window_call("window.maxSize", size.map(size_payload).unwrap_or(json!(null)))
  }

  fn set_position(&self, position: Position) -> Result<()> {
    self.window_call("window.position", position_payload(position))
  }

  fn set_fullscreen(&self, fullscreen: bool) -> Result<()> {
    self.window_call("window.fullscreen", json!({ "fullscreen": fullscreen }))
  }

  fn set_fullscreen_on_monitor(&self, position: PhysicalPosition<f64>) -> Result<()> {
    self.window_call(
      "window.fullscreen",
      json!({ "fullscreen": true, "x": position.x, "y": position.y }),
    )
  }

  #[cfg(target_os = "macos")]
  fn set_simple_fullscreen(&self, enable: bool) -> Result<()> {
    self.window_call("window.simpleFullscreen", json!({ "fullscreen": enable }))
  }

  fn set_focus(&self) -> Result<()> {
    self.window_call("window.focus", json!({}))
  }

  fn set_focusable(&self, focusable: bool) -> Result<()> {
    self.window_call("window.focusable", json!({ "focusable": focusable }))
  }

  fn set_icon(&self, icon: Icon<'_>) -> Result<()> {
    self.window_call("window.icon", json!({ "icon": icon_payload(icon) }))
  }

  fn set_skip_taskbar(&self, skip: bool) -> Result<()> {
    self.window_call("window.skipTaskbar", json!({ "skip": skip }))
  }

  // Confining the pointer is a native operation; a page cannot do it.
  fn set_cursor_grab(&self, grab: bool) -> Result<()> {
    #[cfg(windows)]
    {
      imp::clip_cursor(self.context.native_handle(self.id)?, grab);
      Ok(())
    }
    #[cfg(not(windows))]
    {
      let _ = grab;
      crate::unsupported::report("Window::set_cursor_grab");
      Ok(())
    }
  }

  fn set_cursor_visible(&self, visible: bool) -> Result<()> {
    self.window_call("window.setCursorVisible", json!({ "visible": visible }))
  }

  fn set_cursor_icon(&self, icon: CursorIcon) -> Result<()> {
    self.window_call("window.setCursorIcon", json!({ "cursor": cursor_css(icon) }))
  }

  fn set_cursor_position<Pos: Into<Position>>(&self, position: Pos) -> Result<()> {
    #[cfg(windows)]
    {
      let scale = self.geometry()?["scaleFactor"].as_f64().unwrap_or(1.0);
      let position: PhysicalPosition<i32> = position.into().to_physical(scale);
      imp::set_cursor_position(position);
      Ok(())
    }
    #[cfg(not(windows))]
    {
      let _ = position;
      crate::unsupported::report("Window::set_cursor_position");
      Ok(())
    }
  }

  fn set_ignore_cursor_events(&self, ignore: bool) -> Result<()> {
    self.window_call("window.ignoreCursorEvents", json!({ "ignore": ignore }))
  }

  // Drag a frameless window the Win32 way: hand the window a caption drag.
  fn start_dragging(&self) -> Result<()> {
    #[cfg(windows)]
    {
      imp::start_dragging(self.context.native_handle(self.id)?);
      Ok(())
    }
    #[cfg(not(windows))]
    Ok(())
  }

  fn start_resize_dragging(&self, direction: tauri_runtime::ResizeDirection) -> Result<()> {
    #[cfg(windows)]
    {
      imp::start_resize_dragging(self.context.native_handle(self.id)?, direction);
      Ok(())
    }
    #[cfg(not(windows))]
    {
      let _ = direction;
      crate::unsupported::report("Window::start_resize_dragging");
      Ok(())
    }
  }

  fn set_progress_bar(&self, progress_state: ProgressBarState) -> Result<()> {
    let state = match progress_state.status {
      None | Some(ProgressBarStatus::None) => "none",
      Some(ProgressBarStatus::Normal) => "normal",
      Some(ProgressBarStatus::Indeterminate) => "indeterminate",
      Some(ProgressBarStatus::Paused) => "paused",
      Some(ProgressBarStatus::Error) => "error",
    };
    self.window_call(
      "window.progressBar",
      json!({
        "state": state,
        "progress": progress_state.progress.map(|progress| progress as f64 / 100.0),
        "desktopFilename": progress_state.desktop_filename,
      }),
    )
  }

  fn set_badge_count(&self, count: Option<i64>, _desktop_filename: Option<String>) -> Result<()> {
    self.window_call("window.badgeCount", json!({ "count": count.unwrap_or(0) }))
  }

  // macOS badge labels need a string API Electron does not expose; the dock
  // badge goes through `set_badge_count`.
  fn set_badge_label(&self, label: Option<String>) -> Result<()> {
    self.window_call("app.badgeLabel", json!({ "label": label }))
  }

  fn set_overlay_icon(&self, icon: Option<Icon<'_>>) -> Result<()> {
    self.window_call(
      "window.overlayIcon",
      json!({ "icon": icon.map(icon_payload) }),
    )
  }

  /// A `BrowserWindow`'s title bar style is fixed when the window is created;
  /// `window.create` already carries it, so only a post-creation change is out
  /// of reach.
  fn set_title_bar_style(&self, _style: tauri_utils::TitleBarStyle) -> Result<()> {
    // No Electron equivalent: BrowserWindow.titleBarStyle is creation-time only.
    crate::unsupported::report("Window::set_title_bar_style");
    Ok(())
  }

  fn set_traffic_light_position(&self, position: Position) -> Result<()> {
    self.window_call("window.trafficLight", position_payload(position))
  }

  fn set_size_constraints(&self, constraints: WindowSizeConstraints) -> Result<()> {
    let scale = self.geometry()?["scaleFactor"]
      .as_f64()
      .unwrap_or(1.0)
      .max(1.0);
    let dip = |unit: Option<PixelUnit>| -> Option<f64> {
      unit.map(|unit| match unit {
        PixelUnit::Physical(unit) => unit.0 as f64 / scale,
        PixelUnit::Logical(unit) => unit.0,
      })
    };
    let min_width = dip(constraints.min_width).unwrap_or(0.0);
    let min_height = dip(constraints.min_height).unwrap_or(0.0);
    let max_width = dip(constraints.max_width).unwrap_or(0.0);
    let max_height = dip(constraints.max_height).unwrap_or(0.0);
    self.window_call(
      "window.minSize",
      json!({ "width": min_width, "height": min_height, "physical": false }),
    )?;
    self.window_call(
      "window.maxSize",
      json!({ "width": max_width, "height": max_height, "physical": false }),
    )
  }

  fn set_theme(&self, theme: Option<Theme>) -> Result<()> {
    let theme = match theme {
      Some(Theme::Dark) => "dark",
      Some(_) => "light",
      None => "system",
    };
    self.window_call("window.theme", json!({ "theme": theme }))
  }

  fn set_enabled(&self, enabled: bool) -> Result<()> {
    self.window_call("window.enabled", json!({ "enabled": enabled }))
  }

  fn is_enabled(&self) -> Result<bool> {
    Ok(
      self.window_call_value("window.enabledQuery", json!({}))?
        .as_bool()
        .unwrap_or(true),
    )
  }

  fn is_always_on_top(&self) -> Result<bool> {
    Ok(self.geometry()?["alwaysOnTop"].as_bool().unwrap_or(false))
  }

  fn set_background_color(&self, color: Option<tauri_utils::config::Color>) -> Result<()> {
    self.window_call("window.backgroundColor", json!({ "color": color.map(color_css) }))
  }
}

fn icon_payload(icon: Icon<'_>) -> Value {
  json!({
    "width": icon.width,
    "height": icon.height,
    "rgba": icon.rgba.as_ref(),
  })
}

fn color_css(color: tauri_utils::config::Color) -> String {
  format!(
    "rgba({}, {}, {}, {:.3})",
    color.0,
    color.1,
    color.2,
    color.3 as f64 / 255.0
  )
}

/// CSS `cursor` keyword for a window cursor; Electron has no native cursor API,
/// so the page's stylesheet is the channel.
fn cursor_css(icon: CursorIcon) -> &'static str {
  match icon {
    CursorIcon::Default | CursorIcon::Arrow => "default",
    CursorIcon::Crosshair => "crosshair",
    CursorIcon::Hand => "pointer",
    CursorIcon::Move => "move",
    CursorIcon::Text => "text",
    CursorIcon::VerticalText => "vertical-text",
    CursorIcon::Wait => "wait",
    CursorIcon::Help => "help",
    CursorIcon::Progress => "progress",
    CursorIcon::NotAllowed | CursorIcon::NoDrop => "not-allowed",
    CursorIcon::ContextMenu => "context-menu",
    CursorIcon::Cell => "cell",
    CursorIcon::Alias => "alias",
    CursorIcon::Copy => "copy",
    CursorIcon::Grab => "grab",
    CursorIcon::Grabbing => "grabbing",
    CursorIcon::AllScroll => "all-scroll",
    CursorIcon::ZoomIn => "zoom-in",
    CursorIcon::ZoomOut => "zoom-out",
    CursorIcon::EResize => "e-resize",
    CursorIcon::NResize => "n-resize",
    CursorIcon::NeResize => "ne-resize",
    CursorIcon::NwResize => "nw-resize",
    CursorIcon::SResize => "s-resize",
    CursorIcon::SeResize => "se-resize",
    CursorIcon::SwResize => "sw-resize",
    CursorIcon::WResize => "w-resize",
    CursorIcon::EwResize => "ew-resize",
    CursorIcon::NsResize => "ns-resize",
    CursorIcon::NeswResize => "nesw-resize",
    CursorIcon::NwseResize => "nwse-resize",
    CursorIcon::ColResize => "col-resize",
    CursorIcon::RowResize => "row-resize",
    // `CursorIcon` is `#[non_exhaustive]`; unknown cursors keep the default.
    _ => "default",
  }
}

/// Win32 tweaks that Electron has no API for but the window still supports.
#[cfg(windows)]
pub(crate) mod imp {
  use tauri_runtime::{ResizeDirection, dpi::PhysicalPosition};
  use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT, WPARAM};
  use windows::Win32::Graphics::Gdi::ClientToScreen;
  use windows::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture;
  use windows::Win32::UI::WindowsAndMessaging::{
    ClipCursor, GWLP_HWNDPARENT, GetClientRect, GetWindowLongPtrW, HTCAPTION,
    HTBOTTOM, HTBOTTOMLEFT, HTBOTTOMRIGHT, HTLEFT, HTRIGHT, HTTOP, HTTOPLEFT, HTTOPRIGHT, HWND_BOTTOM,
    SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOZORDER,
    SetWindowLongPtrW, SetCursorPos, SetWindowPos, SendMessageW, WM_NCLBUTTONDOWN, WS_CAPTION,
    WS_THICKFRAME,
    WS_MAXIMIZEBOX, WS_MINIMIZEBOX, WS_SYSMENU, GWL_STYLE, SWP_NOMOVE, SWP_NOSIZE,
  };

  fn hwnd(handle: isize) -> HWND {
    HWND(handle as *mut std::ffi::c_void)
  }

  pub(crate) fn send_to_bottom(handle: isize) {
    // SAFETY: z-order only, no geometry or focus change.
    unsafe {
      let _ = SetWindowPos(
        hwnd(handle),
        Some(HWND_BOTTOM),
        0,
        0,
        0,
        0,
        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
      );
    }
  }

  pub(super) fn set_decorations(handle: isize, decorations: bool) {
    let window = hwnd(handle);
    let style = unsafe { GetWindowLongPtrW(window, GWL_STYLE) } as u32;
    let frame = WS_CAPTION | WS_THICKFRAME | WS_SYSMENU | WS_MINIMIZEBOX | WS_MAXIMIZEBOX;
    let style = if decorations {
      style | frame.0
    } else {
      style & !frame.0
    };
    // SAFETY: plain Win32 style juggling on a window we were handed.
    unsafe {
      SetWindowLongPtrW(window, GWL_STYLE, style as isize);
      let _ = SetWindowPos(
        window,
        None,
        0,
        0,
        0,
        0,
        SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_FRAMECHANGED,
      );
    }
  }

  pub(super) fn start_dragging(handle: isize) {
    let window = hwnd(handle);
    // SAFETY: the canonical frameless-drag trick; the message is handled by the
    // window's non-client path and only starts a move loop.
    unsafe {
      let _ = ReleaseCapture();
      SendMessageW(
        window,
        WM_NCLBUTTONDOWN,
        Some(WPARAM(HTCAPTION as usize)),
        Some(LPARAM(0)),
      );
    }
  }

  /// Same trick as `start_dragging`, for the resize borders.
  pub(crate) fn start_resize_dragging(handle: isize, direction: ResizeDirection) {
    let hit = match direction {
      ResizeDirection::East => HTRIGHT,
      ResizeDirection::North => HTTOP,
      ResizeDirection::NorthEast => HTTOPRIGHT,
      ResizeDirection::NorthWest => HTTOPLEFT,
      ResizeDirection::South => HTBOTTOM,
      ResizeDirection::SouthEast => HTBOTTOMRIGHT,
      ResizeDirection::SouthWest => HTBOTTOMLEFT,
      ResizeDirection::West => HTLEFT,
    };
    // SAFETY: same non-client message path as the caption drag above.
    unsafe {
      let _ = ReleaseCapture();
      SendMessageW(hwnd(handle), WM_NCLBUTTONDOWN, Some(WPARAM(hit as usize)), Some(LPARAM(0)));
    }
  }

  /// Confines the pointer to the window's client area (or releases it).
  pub(crate) fn clip_cursor(handle: isize, grab: bool) {
    // SAFETY: geometry query plus a pointer clip; no memory is handed out.
    unsafe {
      if !grab {
        let _ = ClipCursor(None);
        return;
      }
      let mut rect = RECT::default();
      if GetClientRect(hwnd(handle), &mut rect).is_err() {
        return;
      }
      let mut top_left = POINT { x: rect.left, y: rect.top };
      let mut bottom_right = POINT { x: rect.right, y: rect.bottom };
      let _ = ClientToScreen(hwnd(handle), &mut top_left);
      let _ = ClientToScreen(hwnd(handle), &mut bottom_right);
      let clip = RECT {
        left: top_left.x,
        top: top_left.y,
        right: bottom_right.x,
        bottom: bottom_right.y,
      };
      let _ = ClipCursor(Some(&clip));
    }
  }

  pub(crate) fn set_cursor_position(position: PhysicalPosition<i32>) {
    // SAFETY: plain cursor move in screen coordinates.
    unsafe {
      let _ = SetCursorPos(position.x, position.y);
    }
  }

  /// Owned windows follow their owner's z-order and lifetime.
  pub(crate) fn set_owner(handle: isize, owner: isize) {
    // SAFETY: GWLP_HWNDPARENT takes a window handle; no memory ownership moves.
    unsafe {
      SetWindowLongPtrW(hwnd(handle), GWLP_HWNDPARENT, owner);
    }
  }
}
