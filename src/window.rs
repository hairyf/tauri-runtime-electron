// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! Window dispatch.
//!
//! The reference drives winit: a message reaches the event loop, the loop calls
//! the live `winit::window::Window`, and the answer travels back on a channel.
//! Electron owns its windows in another process, so the same messages reach
//! [`handle_window_message`] on the runtime's loop thread and become
//! `electron-rs` RPCs — `window.geometry` for the getters, one command per
//! setter — with the same channel carrying the answer back to the caller.
//!
//! Everything the reference reads from its live window object is therefore read
//! from Electron instead of cached here: `AppWindow::attrs` is the record of what
//! the window was *created* with, not a mirror of its current state.

use std::{
  sync::{
    Arc,
    mpsc::{self, Receiver, Sender},
  },
  time::Duration,
};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use serde_json::{Map, Value, json};
use tauri_runtime::{
  Error, Icon, ProgressBarState, ProgressBarStatus, Result, UserAttentionType, UserEvent,
  WindowDispatch,
  dpi::{PhysicalPosition, PhysicalSize, PixelUnit, Position, Size},
  monitor::Monitor,
  webview::{DetachedWebview, PendingWebview},
  window::{
    CursorIcon, DetachedWindow, DetachedWindowWebview, PendingWindow, RawWindow, WindowId,
    WindowSizeConstraints,
  },
};
use tauri_utils::{Theme, config::Color};

use crate::{
  runtime::{ElectronRuntime, IPC_TIMEOUT, Message, RuntimeContext, create_window_err},
  webview::{self, PageFullscreen, create_webview_detached},
  window_builder::WindowBuilderWrapper,
  window_handle::SendRawWindowHandle,
};

/// How long Electron gets to finish a fullscreen transition. Entering or leaving
/// fullscreen resolves only once the window reported the change, and macOS
/// animates it.
const FULLSCREEN_TIMEOUT: Duration = Duration::from_secs(25);

/// How long restoring a minimized window gets. macOS resolves it on the `restore`
/// event, which arrives after the dock animation.
const UNMINIMIZE_TIMEOUT: Duration = Duration::from_secs(15);

#[cfg(any(
  target_os = "linux",
  target_os = "dragonfly",
  target_os = "freebsd",
  target_os = "netbsd",
  target_os = "openbsd"
))]
pub(crate) struct SendGtkWindow(*mut std::ffi::c_void);

#[cfg(any(
  target_os = "linux",
  target_os = "dragonfly",
  target_os = "freebsd",
  target_os = "netbsd",
  target_os = "openbsd"
))]
unsafe impl Send for SendGtkWindow {}

#[cfg(any(
  target_os = "linux",
  target_os = "dragonfly",
  target_os = "freebsd",
  target_os = "netbsd",
  target_os = "openbsd"
))]
pub(crate) struct SendGtkBox(*mut std::ffi::c_void);

#[cfg(any(
  target_os = "linux",
  target_os = "dragonfly",
  target_os = "freebsd",
  target_os = "netbsd",
  target_os = "openbsd"
))]
unsafe impl Send for SendGtkBox {}

/// The number Electron knows a window by.
///
/// `WindowId` is a newtype with no accessor, but its derived `Ord` orders by the
/// inner `u32`, so an exact binary search recovers the number. The runtime mints
/// every id through `RuntimeContext::next_window_id` and registers the same
/// number with `window.create`, so both sides always agree without a second
/// registry to keep in step.
pub(crate) fn window_number(id: WindowId) -> u32 {
  let mut low = 0u32;
  let mut high = u32::MAX;
  while low < high {
    let middle = low + (high - low) / 2;
    if WindowId::from(middle) < id {
      low = middle + 1;
    } else {
      high = middle;
    }
  }
  low
}

/// Maps a theme name reported by Electron onto Tauri's theme.
pub(crate) fn native_theme_to_tauri_theme(theme: &str) -> Theme {
  match theme {
    "dark" => Theme::Dark,
    _ => Theme::Light,
  }
}

/// Maps a Tauri theme onto the name Electron's `window.theme` takes.
///
/// `None` is `system`: Electron follows the OS and reports changes back through
/// `window.themeChanged`.
pub(crate) fn tauri_theme_to_native_theme(theme: Option<Theme>) -> &'static str {
  match theme {
    Some(Theme::Dark) => "dark",
    Some(_) => "light",
    None => "system",
  }
}

/// Maps the display JSON shape Electron reports onto Tauri's physical-pixel
/// `Monitor`.
///
/// Electron already resolves the work area, which is why the reference needs a
/// native shim per OS; here the value is simply read.
pub(crate) fn native_monitor_to_tauri_monitor(value: &Value) -> Option<Monitor> {
  if !value.is_object() {
    return None;
  }
  let rect_position = |value: &Value| {
    PhysicalPosition::new(
      value["x"].as_i64().unwrap_or_default() as i32,
      value["y"].as_i64().unwrap_or_default() as i32,
    )
  };
  let rect_size = |value: &Value| {
    PhysicalSize::new(
      value["width"].as_u64().unwrap_or_default() as u32,
      value["height"].as_u64().unwrap_or_default() as u32,
    )
  };
  let work_area = &value["workArea"];
  Some(Monitor {
    name: value["name"].as_str().map(str::to_string),
    scale_factor: value["scaleFactor"].as_f64().unwrap_or(1.0),
    position: rect_position(&value["position"]),
    size: rect_size(&value["size"]),
    work_area: tauri_runtime::dpi::PhysicalRect {
      position: rect_position(&work_area["position"]),
      size: rect_size(&work_area["size"]),
    },
  })
}

fn paired_size_constraint(
  width: Option<PixelUnit>,
  height: Option<PixelUnit>,
  unconstrained: u32,
) -> Option<Size> {
  match (width, height) {
    (Some(PixelUnit::Logical(width)), Some(PixelUnit::Logical(height))) => Some(Size::Logical(
      tauri_runtime::dpi::LogicalSize::new(width.into(), height.into()),
    )),
    (Some(PixelUnit::Physical(width)), Some(PixelUnit::Physical(height))) => Some(Size::Physical(
      PhysicalSize::new(width.into(), height.into()),
    )),
    (Some(PixelUnit::Logical(width)), None) => Some(Size::Logical(
      tauri_runtime::dpi::LogicalSize::new(width.into(), unconstrained as f64),
    )),
    (None, Some(PixelUnit::Logical(height))) => Some(Size::Logical(
      tauri_runtime::dpi::LogicalSize::new(unconstrained as f64, height.into()),
    )),
    (Some(PixelUnit::Physical(width)), None) => Some(Size::Physical(PhysicalSize::new(
      width.into(),
      unconstrained,
    ))),
    (None, Some(PixelUnit::Physical(height))) => Some(Size::Physical(PhysicalSize::new(
      unconstrained,
      height.into(),
    ))),
    _ => None,
  }
}

pub(crate) fn min_size_constraint(
  width: Option<PixelUnit>,
  height: Option<PixelUnit>,
) -> Option<Size> {
  paired_size_constraint(width, height, 0)
}

pub(crate) fn max_size_constraint(
  width: Option<PixelUnit>,
  height: Option<PixelUnit>,
) -> Option<Size> {
  paired_size_constraint(width, height, u32::MAX)
}

/// The monitor a fullscreen request targets. Kept as the request came in, rather
/// than as a resolved display, so both entry points share one path.
#[derive(Clone, Copy, Debug)]
pub(crate) enum FullscreenTarget {
  /// The display the window is on (`set_fullscreen(true)`).
  CurrentMonitor,
  /// The display containing this physical position (`set_fullscreen_on_monitor`).
  MonitorAt(PhysicalPosition<f64>),
}

pub(crate) enum WindowMessage {
  Close,
  Destroy,
  ScaleFactor(Sender<Result<f64>>),
  InnerPosition(Sender<Result<PhysicalPosition<i32>>>),
  OuterPosition(Sender<Result<PhysicalPosition<i32>>>),
  InnerSize(Sender<Result<PhysicalSize<u32>>>),
  OuterSize(Sender<Result<PhysicalSize<u32>>>),
  IsFullscreen(Sender<Result<bool>>),
  IsMinimized(Sender<Result<bool>>),
  IsMaximized(Sender<Result<bool>>),
  IsFocused(Sender<Result<bool>>),
  IsDecorated(Sender<Result<bool>>),
  IsResizable(Sender<Result<bool>>),
  IsMaximizable(Sender<Result<bool>>),
  IsMinimizable(Sender<Result<bool>>),
  IsClosable(Sender<Result<bool>>),
  IsVisible(Sender<Result<bool>>),
  IsEnabled(Sender<Result<bool>>),
  IsAlwaysOnTop(Sender<Result<bool>>),
  Title(Sender<Result<String>>),
  CurrentMonitor(Sender<Result<Option<Monitor>>>),
  PrimaryMonitor(Sender<Result<Option<Monitor>>>),
  MonitorFromPoint(Sender<Result<Option<Monitor>>>, f64, f64),
  AvailableMonitors(Sender<Result<Vec<Monitor>>>),
  #[cfg(any(
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd"
  ))]
  GtkWindow(Sender<Result<SendGtkWindow>>),
  #[cfg(any(
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd"
  ))]
  DefaultVBox(Sender<Result<SendGtkBox>>),
  RawWindowHandle(Sender<Result<SendRawWindowHandle>>),
  Theme(Sender<Result<Theme>>),
  Center,
  RequestUserAttention(Option<UserAttentionType>),
  SetEnabled(bool),
  SetResizable(bool),
  SetMaximizable(bool),
  SetMinimizable(bool),
  SetClosable(bool),
  SetTitle(String),
  Maximize,
  Unmaximize,
  Minimize,
  Unminimize,
  Show,
  Hide,
  SetDecorations(bool),
  SetShadow(bool),
  SetAlwaysOnBottom(bool),
  SetAlwaysOnTop(bool),
  SetVisibleOnAllWorkspaces(bool),
  SetContentProtected(bool),
  SetSize(Size),
  SetMinSize(Option<Size>),
  SetMaxSize(Option<Size>),
  SetSizeConstraints(WindowSizeConstraints),
  SetPosition(Position),
  SetFullscreen(bool),
  SetFullscreenOnMonitor(PhysicalPosition<f64>),
  /// F11 (`IDC_FULLSCREEN`) outside page fullscreen.
  ToggleFullscreen,
  /// Applies the fullscreen request held back during a transition, now that it
  /// ended. Electron resolves `window.fullscreen` only once the transition is
  /// over, so the mirror has no request to hold; the variant stays so the
  /// message set matches the reference.
  #[cfg(target_os = "macos")]
  ApplyPendingFullscreen,
  #[cfg(target_os = "macos")]
  SetSimpleFullscreen(bool),
  SetFocus,
  SetFocusable(bool),
  SetIcon(Icon<'static>),
  SetSkipTaskbar(bool),
  SetCursorGrab(bool),
  SetCursorVisible(bool),
  SetCursorIcon(CursorIcon),
  SetCursorPosition(Position),
  SetIgnoreCursorEvents(bool),
  SetProgressBar(ProgressBarState),
  SetBadgeCount(Option<i64>, Option<String>),
  SetBadgeLabel(Option<String>),
  SetOverlayIcon(Option<Icon<'static>>),
  SetTitleBarStyle(tauri_utils::TitleBarStyle),
  SetTrafficLightPosition(Position),
  SetTheme(Option<Theme>),
  SetBackgroundColor(Option<Color>),
  StartDragging,
  StartResizeDragging(tauri_runtime::ResizeDirection),
}

/// Opaque identity of one runtime-owned window lifetime. Reparented webviews
/// observe the destination token; same-label replacements never match.
#[derive(Clone)]
pub struct NativeWindowToken(Arc<()>);

impl NativeWindowToken {
  pub(crate) fn new() -> Self {
    Self(Arc::new(()))
  }
}

impl PartialEq for NativeWindowToken {
  fn eq(&self, other: &Self) -> bool {
    Arc::ptr_eq(&self.0, &other.0)
  }
}

impl Eq for NativeWindowToken {}

impl std::fmt::Debug for NativeWindowToken {
  fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    formatter
      .debug_struct("NativeWindowToken")
      .finish_non_exhaustive()
  }
}

#[cfg(test)]
mod native_window_identity_tests {
  use super::NativeWindowToken;

  #[test]
  fn references_preserve_one_window_and_reject_replacements() {
    let first = NativeWindowToken::new();
    let retained_by_webview = first.clone();
    assert_eq!(first, retained_by_webview);
    let destination = NativeWindowToken::new();
    assert_ne!(retained_by_webview, destination);
    drop(first);
    assert_ne!(retained_by_webview, NativeWindowToken::new());
  }
}

/// The runtime's per-window record.
///
/// The reference keeps the live winit window here and mutates its attributes as
/// the application changes them. Electron's window lives in the main process and
/// can be asked for its state (`window.geometry`, `window.themeQuery`), so this
/// record holds what the window was created with plus the identity webviews
/// observe; nothing here is a stale copy of live state.
///
/// Two fields of the reference have no counterpart. `children` is not here: the
/// live per-webview state lives in `crate::webview`'s registry, because
/// [`RuntimeContext::with_window`] only lends `&AppWindow` and the webview message
/// handler has to mutate it. `pending_activation` is not here either: Electron's
/// `window.ready` reveals the window itself, honouring `visible`, `focus` and
/// `focusable`, so there is no "winit has not mapped it yet" race to retry.
pub(crate) struct AppWindow {
  pub(crate) lifetime: NativeWindowToken,
  #[allow(unused)]
  pub(crate) id: WindowId,
  pub(crate) label: String,
  pub(crate) attrs: AppWindowAttrs,
}

/// The attributes a window is created with.
///
/// The reference stores winit's `WindowAttributes` plus the few Tauri-only flags
/// winit cannot express. Electron has no attribute object — `window.create`
/// takes a config object — so the same attributes are plain fields here and
/// [`window_config`] turns them into that object.
#[derive(Clone, Debug)]
pub(crate) struct AppWindowAttrs {
  pub(crate) title: Option<String>,
  pub(crate) inner_size: Option<Size>,
  pub(crate) min_inner_size: Option<Size>,
  pub(crate) max_inner_size: Option<Size>,
  pub(crate) position: Option<Position>,
  pub(crate) center: bool,
  pub(crate) resizable: bool,
  pub(crate) maximizable: bool,
  pub(crate) minimizable: bool,
  pub(crate) closable: bool,
  pub(crate) maximized: bool,
  pub(crate) fullscreen: bool,
  pub(crate) focused: bool,
  pub(crate) focusable: bool,
  pub(crate) visible: bool,
  pub(crate) transparent: bool,
  pub(crate) decorations: bool,
  pub(crate) always_on_bottom: bool,
  pub(crate) always_on_top: bool,
  pub(crate) visible_on_all_workspaces: bool,
  pub(crate) content_protected: bool,
  pub(crate) icon: Option<Icon<'static>>,
  pub(crate) skip_taskbar: bool,
  pub(crate) background_color: Option<Color>,
  pub(crate) shadow: bool,
  pub(crate) theme: Option<Theme>,
  pub(crate) window_classname: Option<String>,
  pub(crate) prevent_overflow: Option<Size>,
  /// Owner handle recorded by [`WindowBuilder::owner`](tauri_runtime::window::WindowBuilder::owner).
  #[cfg(windows)]
  pub(crate) owner: Option<isize>,
  /// Parent handle recorded by [`WindowBuilder::parent`](tauri_runtime::window::WindowBuilder::parent).
  #[cfg(windows)]
  pub(crate) parent: Option<isize>,
  #[cfg(windows)]
  pub(crate) drag_and_drop: Option<bool>,
  /// Parent `NSWindow*` recorded by the macOS `parent` builder method.
  #[cfg(target_os = "macos")]
  pub(crate) macos_parent: Option<isize>,
  #[cfg(target_os = "macos")]
  pub(crate) title_bar_style: Option<tauri_utils::TitleBarStyle>,
  #[cfg(target_os = "macos")]
  pub(crate) traffic_light_position: Option<Position>,
  #[cfg(target_os = "macos")]
  pub(crate) hidden_title: bool,
  #[cfg(target_os = "macos")]
  pub(crate) tabbing_identifier: Option<String>,
  /// Parent `GtkWindow*` recorded by [`WindowBuilder::transient_for`](tauri_runtime::window::WindowBuilder::transient_for).
  #[cfg(any(
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd"
  ))]
  pub(crate) transient_for: Option<isize>,
}

impl Default for AppWindowAttrs {
  /// The same defaults winit's `WindowAttributes` has, which is what the
  /// reference's builder starts from: a visible, resizable, decorated window with
  /// all three title-bar buttons enabled, focusable and shadowed.
  fn default() -> Self {
    Self {
      title: None,
      inner_size: None,
      min_inner_size: None,
      max_inner_size: None,
      position: None,
      center: false,
      resizable: true,
      maximizable: true,
      minimizable: true,
      closable: true,
      maximized: false,
      fullscreen: false,
      focused: true,
      focusable: true,
      visible: true,
      transparent: false,
      decorations: true,
      always_on_bottom: false,
      always_on_top: false,
      visible_on_all_workspaces: false,
      content_protected: false,
      icon: None,
      skip_taskbar: false,
      background_color: None,
      shadow: true,
      theme: None,
      window_classname: None,
      prevent_overflow: None,
      #[cfg(windows)]
      owner: None,
      #[cfg(windows)]
      parent: None,
      #[cfg(windows)]
      drag_and_drop: None,
      #[cfg(target_os = "macos")]
      macos_parent: None,
      #[cfg(target_os = "macos")]
      title_bar_style: None,
      #[cfg(target_os = "macos")]
      traffic_light_position: None,
      #[cfg(target_os = "macos")]
      hidden_title: false,
      #[cfg(target_os = "macos")]
      tabbing_identifier: None,
      #[cfg(any(
        target_os = "linux",
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd"
      ))]
      transient_for: None,
    }
  }
}

impl AppWindow {
  /// The window-level theme override, if the application set one.
  #[allow(dead_code)]
  pub(crate) fn preferred_theme(&self) -> Option<Theme> {
    self.attrs.theme
  }

  /// The window override, else the app-wide theme.
  #[allow(dead_code)]
  pub(crate) fn resolved_theme(&self, app_wide_theme: Option<Theme>) -> Option<Theme> {
    self.preferred_theme().or(app_wide_theme)
  }
}

/// Whether an icon buffer matches its declared size.
///
/// The reference lets winit validate the buffer; `window.icon` takes the pixels
/// as they are, so the check has to happen here or Electron would receive a
/// mismatched image.
pub(crate) fn tauri_icon_is_valid(icon: &Icon<'_>) -> bool {
  icon
    .rgba
    .len()
    .checked_div(4)
    .is_some_and(|pixels| pixels == (icon.width as usize) * (icon.height as usize))
}

/// The `window.icon` / `window.overlayIcon` payload.
fn icon_payload(icon: &Icon<'_>) -> Value {
  json!({
    "width": icon.width,
    "height": icon.height,
    "rgba": icon.rgba.as_ref(),
  })
}

/// The `window.create` `windowIcon` payload: a base64 BGRA bitmap.
///
/// Electron builds a `NativeImage` from raw bitmap data, whose byte order is
/// BGRA rather than the RGBA Tauri carries.
fn creation_icon(icon: Option<&Icon<'static>>) -> Option<Value> {
  let icon = icon?;
  let mut bgra = Vec::with_capacity(icon.rgba.len());
  for pixel in icon.rgba.as_chunks::<4>().0 {
    bgra.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
  }
  Some(json!({
    "width": icon.width,
    "height": icon.height,
    "bgra": BASE64.encode(bgra),
  }))
}

/// The CSS keyword Electron injects for a Tauri cursor.
///
/// `window.setCursorIcon` styles every webview of the window, so the mapping is
/// to the CSS cursor names rather than to native cursors.
fn tauri_cursor_to_css(cursor: CursorIcon) -> &'static str {
  match cursor {
    CursorIcon::Default => "default",
    CursorIcon::Crosshair => "crosshair",
    CursorIcon::Hand => "pointer",
    CursorIcon::Arrow => "default",
    CursorIcon::Move => "move",
    CursorIcon::Text => "text",
    CursorIcon::Wait => "wait",
    CursorIcon::Help => "help",
    CursorIcon::Progress => "progress",
    CursorIcon::NotAllowed => "not-allowed",
    CursorIcon::ContextMenu => "context-menu",
    CursorIcon::Cell => "cell",
    CursorIcon::VerticalText => "vertical-text",
    CursorIcon::Alias => "alias",
    CursorIcon::Copy => "copy",
    CursorIcon::NoDrop => "no-drop",
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
    _ => "default",
  }
}

/// Sends one `window.*` command for `id` and waits for its answer.
fn window_call<T: UserEvent>(
  context: &RuntimeContext<T>,
  id: WindowId,
  method: &str,
  mut params: Value,
  timeout: Duration,
) -> Result<Value> {
  if let Some(map) = params.as_object_mut() {
    map.insert("windowId".into(), json!(window_number(id)));
  }
  context
    .ipc
    .call(method, params, timeout)
    .map_err(create_window_err)
}

fn geometry<T: UserEvent>(context: &RuntimeContext<T>, id: WindowId) -> Result<Value> {
  window_call(context, id, "window.geometry", json!({}), IPC_TIMEOUT)
}

/// The scale factor of the primary display, used only to resolve a *physical*
/// creation size, which Electron's config takes in logical pixels.
fn primary_scale_factor<T: UserEvent>(context: &RuntimeContext<T>) -> f64 {
  context
    .ipc
    .call("window.displays", json!({}), IPC_TIMEOUT)
    .ok()
    .and_then(|displays| displays["primary"]["scaleFactor"].as_f64())
    .unwrap_or(1.0)
}

fn size_scale(size: Option<Size>) -> bool {
  matches!(size, Some(Size::Physical(_)))
}

/// The logical width and height a `Size` asks for, and whether it was physical.
fn size_payload(size: Size, scale: f64) -> (f64, f64, bool) {
  match size {
    Size::Logical(size) => (size.width, size.height, false),
    Size::Physical(size) => (size.width as f64 / scale, size.height as f64 / scale, true),
  }
}

/// The `window.create` window config for `attrs`.
///
/// Only the keys Electron's `window.create` understands are emitted. The
/// reference hands the same attributes to winit, which applies them natively; the
/// ones Electron has no counterpart for are reported where the builder recorded
/// them and are never invented here.
fn window_config<T: UserEvent>(
  context: &RuntimeContext<T>,
  attrs: &AppWindowAttrs,
) -> Result<Value> {
  let needs_scale = size_scale(attrs.inner_size)
    || size_scale(attrs.min_inner_size)
    || size_scale(attrs.max_inner_size);
  let scale = if needs_scale {
    primary_scale_factor(context)
  } else {
    1.0
  };

  let mut config = Map::new();
  config.insert(
    "title".into(),
    json!(attrs.title.clone().unwrap_or_else(|| "Tauri App".into())),
  );

  // Electron's `width`/`height` are the content size, which is what Tauri's
  // inner size means; the reference's default is the same 800x600.
  let (width, height) = attrs
    .inner_size
    .map(|size| size_payload(size, scale))
    .map(|(width, height, _)| (width, height))
    .unwrap_or((800.0, 600.0));
  config.insert("width".into(), json!(width));
  config.insert("height".into(), json!(height));

  if let Some(position) = attrs.position {
    let position = position.to_logical::<f64>(scale);
    config.insert("x".into(), json!(position.x));
    config.insert("y".into(), json!(position.y));
  }

  for (key, size, horizontal) in [
    ("minWidth", attrs.min_inner_size, true),
    ("minHeight", attrs.min_inner_size, false),
    ("maxWidth", attrs.max_inner_size, true),
    ("maxHeight", attrs.max_inner_size, false),
  ] {
    let Some(size) = size else { continue };
    let (width, height, physical) = size_payload(size, scale);
    let value = if horizontal { width } else { height };
    config.insert(key.into(), json!({ "value": value, "physical": physical }));
  }

  config.insert("resizable".into(), json!(attrs.resizable));
  config.insert("maximizable".into(), json!(attrs.maximizable));
  config.insert("minimizable".into(), json!(attrs.minimizable));
  config.insert("closable".into(), json!(attrs.closable));
  config.insert("maximized".into(), json!(attrs.maximized));
  config.insert("fullscreen".into(), json!(attrs.fullscreen));
  config.insert("focus".into(), json!(attrs.focused));
  config.insert("focusable".into(), json!(attrs.focusable));
  config.insert("visible".into(), json!(attrs.visible));
  config.insert("transparent".into(), json!(attrs.transparent));
  config.insert("decorations".into(), json!(attrs.decorations));
  config.insert("alwaysOnTop".into(), json!(attrs.always_on_top));
  config.insert("skipTaskbar".into(), json!(attrs.skip_taskbar));
  config.insert("shadow".into(), json!(attrs.shadow));
  config.insert("center".into(), json!(attrs.center));
  config.insert(
    "visibleOnAllWorkspaces".into(),
    json!(attrs.visible_on_all_workspaces),
  );
  config.insert("contentProtected".into(), json!(attrs.content_protected));

  if let Some(color) = attrs.background_color {
    config.insert(
      "backgroundColor".into(),
      json!([color.0, color.1, color.2, color.3]),
    );
  }
  if let Some(theme) = attrs.theme {
    config.insert(
      "theme".into(),
      json!(tauri_theme_to_native_theme(Some(theme))),
    );
  }
  if let Some(margin) = attrs.prevent_overflow {
    let margin = margin.to_physical::<u32>(scale);
    if margin.width == 0 && margin.height == 0 {
      config.insert("preventOverflow".into(), json!(true));
    } else {
      config.insert(
        "preventOverflow".into(),
        json!({ "width": margin.width, "height": margin.height }),
      );
    }
  }

  #[cfg(target_os = "macos")]
  {
    if let Some(style) = attrs.title_bar_style {
      let style = match style {
        tauri_utils::TitleBarStyle::Transparent => "transparent",
        tauri_utils::TitleBarStyle::Overlay => "overlay",
        _ => "visible",
      };
      config.insert("titleBarStyle".into(), json!(style));
    }
    if attrs.hidden_title {
      config.insert("hiddenTitle".into(), json!(true));
    }
    if let Some(position) = attrs.traffic_light_position {
      let position = position.to_logical::<f64>(1.0);
      config.insert(
        "trafficLightPosition".into(),
        json!({ "x": position.x, "y": position.y }),
      );
    }
    if let Some(identifier) = &attrs.tabbing_identifier {
      config.insert("tabbingIdentifier".into(), json!(identifier));
    }
  }

  // Electron has no z-order below normal: every `setAlwaysOnTop` level is above
  // it, and there is no `setAlwaysOnBottom`. The reference stores the level and
  // lets winit apply it.
  if attrs.always_on_bottom {
    crate::unsupported::report("WindowBuilder::always_on_bottom");
  }

  Ok(Value::Object(config))
}

/// The `RawWindow` handed to `after_window_creation`.
///
/// The reference passes pointers borrowed from the window it just created. Here
/// the only borrowed field is the lifetime marker, which is a promoted
/// `PhantomData`, so the callback's borrow is as long as it asks for.
fn raw_window<'a>(handle: isize) -> RawWindow<'a> {
  #[cfg(windows)]
  {
    RawWindow {
      hwnd: handle,
      _marker: &std::marker::PhantomData,
    }
  }

  #[cfg(any(
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd"
  ))]
  {
    // The field is documented as a borrowed `GtkApplicationWindow*` that is never
    // null. Electron draws with Chromium views on Ozone, so there is no GTK window
    // in the process; the runtime reports that instead of inventing one, and the
    // pointer is null for a caller that checks.
    let _ = handle;
    crate::unsupported::report("RawWindow::gtk_window");
    RawWindow {
      gtk_window: std::ptr::null_mut(),
      default_vbox: None,
      _marker: &std::marker::PhantomData,
    }
  }

  #[cfg(not(any(
    windows,
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd"
  )))]
  {
    let _ = handle;
    RawWindow {
      _marker: &std::marker::PhantomData,
    }
  }
}

/// Whether the window is in fullscreen once the requests made so far are applied.
///
/// The reference consults a request held back during a macOS transition. Electron
/// resolves `window.fullscreen` only when the transition finished, so the live
/// state is the answer.
fn fullscreen_requested<T: UserEvent>(context: &RuntimeContext<T>, id: WindowId) -> bool {
  geometry(context, id)
    .map(|geometry| geometry["fullscreen"].as_bool().unwrap_or(false))
    .unwrap_or(false)
}

/// Applies a fullscreen request; `None` leaves fullscreen.
///
/// `window.fullscreen` is deferred: it answers once the window reported the
/// change, and with `x`/`y` it first moves the window onto the display containing
/// that physical position, which is the reference's `MonitorAt` lookup.
fn request_fullscreen<T: UserEvent>(
  context: &RuntimeContext<T>,
  id: WindowId,
  target: Option<FullscreenTarget>,
) {
  let params = match target {
    None => json!({ "fullscreen": false }),
    Some(FullscreenTarget::CurrentMonitor) => json!({ "fullscreen": true }),
    Some(FullscreenTarget::MonitorAt(position)) => {
      json!({ "fullscreen": true, "x": position.x, "y": position.y })
    }
  };
  if let Err(error) = window_call(context, id, "window.fullscreen", params, FULLSCREEN_TIMEOUT) {
    log::warn!("[electron] window fullscreen failed: {error}");
  }
}

/// A fullscreen request from the app or the user rather than a page: the window's
/// fullscreen state is theirs from now on. Leaving fullscreen also takes every
/// page out of HTML fullscreen, which would otherwise keep its fullscreen layout
/// in a normal window.
fn request_window_fullscreen<T: UserEvent>(
  context: &RuntimeContext<T>,
  id: WindowId,
  target: Option<FullscreenTarget>,
) {
  for webview_id in webview::webview_ids_for_window(id) {
    let Some(page_fullscreen) =
      webview::with_app_webview(webview_id, |webview| webview.page_fullscreen)
    else {
      continue;
    };
    if page_fullscreen == PageFullscreen::Off {
      continue;
    }
    if target.is_some() {
      webview::set_page_fullscreen(webview_id, PageFullscreen::On { owns_window: false });
    } else {
      webview::set_page_fullscreen(webview_id, PageFullscreen::Off);
      // Leaving HTML fullscreen is a page-side transition: the reference calls
      // CEF's `BrowserHost::exit_fullscreen`, and Electron offers no main-process
      // equivalent (`webContents` has no exit-fullscreen command).
      crate::unsupported::report("AppWindow::request_window_fullscreen: leaving HTML fullscreen");
    }
  }
  request_fullscreen(context, id, target);
}

/// F11: leaves fullscreen when the window is fullscreen or a page shows an
/// element in HTML fullscreen, and enters it otherwise.
fn toggle_fullscreen<T: UserEvent>(context: &RuntimeContext<T>, id: WindowId) {
  let fullscreen = fullscreen_requested(context, id)
    || webview::webview_ids_for_window(id)
      .into_iter()
      .any(|webview_id| {
        webview::with_app_webview(webview_id, |webview| webview.page_fullscreen)
          .is_some_and(|page| page != PageFullscreen::Off)
      });
  request_window_fullscreen(
    context,
    id,
    (!fullscreen).then_some(FullscreenTarget::CurrentMonitor),
  );
}

/// A page of `webview_id` entered or left HTML fullscreen.
///
/// The window enters fullscreen with the first page that needs it and leaves once
/// no page that put it there is still fullscreen; a window the app or the user
/// made fullscreen stays that way.
#[allow(dead_code)]
pub(crate) fn set_page_fullscreen<T: UserEvent>(
  context: &RuntimeContext<T>,
  id: WindowId,
  webview_id: u32,
  fullscreen: bool,
) {
  let window_fullscreen = fullscreen_requested(context, id);
  let Some(page_fullscreen) =
    webview::with_app_webview(webview_id, |webview| webview.page_fullscreen)
  else {
    return;
  };

  if fullscreen {
    if page_fullscreen != PageFullscreen::Off {
      return;
    }
    webview::set_page_fullscreen(
      webview_id,
      PageFullscreen::On {
        owns_window: !window_fullscreen,
      },
    );
    if !window_fullscreen {
      request_fullscreen(context, id, Some(FullscreenTarget::CurrentMonitor));
    }
    return;
  }

  if page_fullscreen != (PageFullscreen::On { owns_window: true }) {
    webview::set_page_fullscreen(webview_id, PageFullscreen::Off);
    return;
  }
  // Another page still in fullscreen keeps the window there, and restores it when
  // it leaves.
  let mut still_fullscreen = None;
  for other in webview::webview_ids_for_window(id) {
    if other == webview_id {
      continue;
    }
    if webview::with_app_webview(other, |webview| webview.page_fullscreen)
      .is_some_and(|page| page != PageFullscreen::Off)
    {
      still_fullscreen = Some(other);
      break;
    }
  }
  webview::set_page_fullscreen(webview_id, PageFullscreen::Off);
  match still_fullscreen {
    Some(other) => {
      webview::set_page_fullscreen(other, PageFullscreen::On { owns_window: true });
    }
    None => request_fullscreen(context, id, None),
  }
}

/// The single consumer of [`WindowMessage`].
///
/// The reference borrows the window out of the app's window map and dispatches on
/// it. Here every arm is one `electron-rs` command, so the window record is only
/// read (for its label) and never borrowed across a call.
pub(crate) fn handle_window_message<T: UserEvent>(
  context: &RuntimeContext<T>,
  id: WindowId,
  message: WindowMessage,
) {
  // Close and Destroy are handled first: they are the two messages that must
  // reach Electron even when the runtime no longer tracks the window, since the
  // native close is what makes the client handler drop it.
  match message {
    WindowMessage::Close => {
      let _ = window_call(context, id, "window.close", json!({}), IPC_TIMEOUT);
      return;
    }
    WindowMessage::Destroy => {
      let _ = window_call(context, id, "window.destroy", json!({}), IPC_TIMEOUT);
      return;
    }
    _ => {}
  }

  match message {
    WindowMessage::Close | WindowMessage::Destroy => unreachable!("handled before the dispatch"),
    WindowMessage::ScaleFactor(tx) => {
      let _ = tx.send(
        geometry(context, id).map(|geometry| geometry["scaleFactor"].as_f64().unwrap_or(1.0)),
      );
    }
    WindowMessage::InnerPosition(tx) => {
      let _ = tx.send(geometry(context, id).map(|geometry| {
        PhysicalPosition::new(
          geometry["contentBounds"]["x"].as_i64().unwrap_or_default() as i32,
          geometry["contentBounds"]["y"].as_i64().unwrap_or_default() as i32,
        )
      }));
    }
    WindowMessage::OuterPosition(tx) => {
      let _ = tx.send(geometry(context, id).map(|geometry| {
        PhysicalPosition::new(
          geometry["bounds"]["x"].as_i64().unwrap_or_default() as i32,
          geometry["bounds"]["y"].as_i64().unwrap_or_default() as i32,
        )
      }));
    }
    WindowMessage::InnerSize(tx) => {
      let _ = tx.send(geometry(context, id).map(|geometry| {
        PhysicalSize::new(
          geometry["contentBounds"]["width"]
            .as_u64()
            .unwrap_or_default() as u32,
          geometry["contentBounds"]["height"]
            .as_u64()
            .unwrap_or_default() as u32,
        )
      }));
    }
    WindowMessage::OuterSize(tx) => {
      let _ = tx.send(geometry(context, id).map(|geometry| {
        PhysicalSize::new(
          geometry["bounds"]["width"].as_u64().unwrap_or_default() as u32,
          geometry["bounds"]["height"].as_u64().unwrap_or_default() as u32,
        )
      }));
    }
    WindowMessage::IsFullscreen(tx) => {
      let _ = tx.send(
        geometry(context, id).map(|geometry| geometry["fullscreen"].as_bool().unwrap_or(false)),
      );
    }
    WindowMessage::IsMinimized(tx) => {
      let _ = tx.send(
        geometry(context, id).map(|geometry| geometry["minimized"].as_bool().unwrap_or(false)),
      );
    }
    WindowMessage::IsMaximized(tx) => {
      let _ = tx.send(
        geometry(context, id).map(|geometry| geometry["maximized"].as_bool().unwrap_or(false)),
      );
    }
    WindowMessage::IsFocused(tx) => {
      let _ = tx
        .send(geometry(context, id).map(|geometry| geometry["focused"].as_bool().unwrap_or(false)));
    }
    WindowMessage::IsDecorated(tx) => {
      let _ = tx.send(
        geometry(context, id).map(|geometry| geometry["decorated"].as_bool().unwrap_or(true)),
      );
    }
    WindowMessage::IsResizable(tx) => {
      let _ = tx.send(
        geometry(context, id).map(|geometry| geometry["resizable"].as_bool().unwrap_or(true)),
      );
    }
    WindowMessage::IsMaximizable(tx) => {
      let _ = tx.send(
        geometry(context, id).map(|geometry| geometry["maximizable"].as_bool().unwrap_or(true)),
      );
    }
    WindowMessage::IsMinimizable(tx) => {
      let _ = tx.send(
        geometry(context, id).map(|geometry| geometry["minimizable"].as_bool().unwrap_or(true)),
      );
    }
    WindowMessage::IsClosable(tx) => {
      let _ = tx
        .send(geometry(context, id).map(|geometry| geometry["closable"].as_bool().unwrap_or(true)));
    }
    WindowMessage::IsVisible(tx) => {
      let _ = tx
        .send(geometry(context, id).map(|geometry| geometry["visible"].as_bool().unwrap_or(true)));
    }
    WindowMessage::IsEnabled(tx) => {
      let _ = tx.send(
        window_call(context, id, "window.enabledQuery", json!({}), IPC_TIMEOUT)
          .map(|enabled| enabled.as_bool().unwrap_or(true)),
      );
    }
    WindowMessage::IsAlwaysOnTop(tx) => {
      let _ = tx.send(
        geometry(context, id).map(|geometry| geometry["alwaysOnTop"].as_bool().unwrap_or(false)),
      );
    }
    WindowMessage::Title(tx) => {
      let _ = tx.send(
        geometry(context, id)
          .map(|geometry| geometry["title"].as_str().unwrap_or_default().to_string()),
      );
    }
    WindowMessage::CurrentMonitor(tx) => {
      let _ = tx.send(
        window_call(context, id, "window.currentDisplay", json!({}), IPC_TIMEOUT)
          .map(|display| native_monitor_to_tauri_monitor(&display)),
      );
    }
    WindowMessage::PrimaryMonitor(tx) => {
      let _ = tx.send(
        window_call(context, id, "window.displays", json!({}), IPC_TIMEOUT)
          .map(|displays| native_monitor_to_tauri_monitor(&displays["primary"])),
      );
    }
    WindowMessage::MonitorFromPoint(tx, x, y) => {
      let _ = tx.send(
        window_call(
          context,
          id,
          "window.displayForPoint",
          json!({ "x": x, "y": y }),
          IPC_TIMEOUT,
        )
        .map(|display| native_monitor_to_tauri_monitor(&display)),
      );
    }
    WindowMessage::AvailableMonitors(tx) => {
      let _ = tx.send(
        window_call(context, id, "window.displays", json!({}), IPC_TIMEOUT).map(|displays| {
          displays["displays"]
            .as_array()
            .map(|list| {
              list
                .iter()
                .filter_map(native_monitor_to_tauri_monitor)
                .collect()
            })
            .unwrap_or_default()
        }),
      );
    }
    #[cfg(any(
      target_os = "linux",
      target_os = "dragonfly",
      target_os = "freebsd",
      target_os = "netbsd",
      target_os = "openbsd"
    ))]
    WindowMessage::GtkWindow(tx) => {
      // Electron draws with Chromium views on Ozone: there is no
      // `GtkApplicationWindow` in the process to hand out, so the "never null"
      // contract cannot be honoured.
      crate::unsupported::report("Window::gtk_window");
      let _ = tx.send(Err(create_window_err(
        "this runtime has no GTK window".into(),
      )));
    }
    #[cfg(any(
      target_os = "linux",
      target_os = "dragonfly",
      target_os = "freebsd",
      target_os = "netbsd",
      target_os = "openbsd"
    ))]
    WindowMessage::DefaultVBox(tx) => {
      crate::unsupported::report("Window::default_vbox");
      let _ = tx.send(Err(create_window_err("this runtime has no GTK box".into())));
    }
    WindowMessage::RawWindowHandle(tx) => {
      let handle = context.native_handle(id).and_then(|handle| {
        SendRawWindowHandle::from_native(handle)
          .ok_or_else(|| create_window_err("electron reported no usable native handle".into()))
      });
      let _ = tx.send(handle);
    }
    WindowMessage::Theme(tx) => {
      let _ = tx.send(
        window_call(context, id, "window.themeQuery", json!({}), IPC_TIMEOUT)
          .map(|theme| native_theme_to_tauri_theme(theme["theme"].as_str().unwrap_or("light"))),
      );
    }
    WindowMessage::Center => {
      let _ = window_call(context, id, "window.center", json!({}), IPC_TIMEOUT);
    }
    WindowMessage::RequestUserAttention(attention) => {
      // `flashFrame` is the only attention Electron exposes, and on macOS it is
      // always critical, so the Informational/Critical distinction is lost.
      let _ = window_call(
        context,
        id,
        "window.requestAttention",
        json!({ "flash": attention.is_some() }),
        IPC_TIMEOUT,
      );
    }
    WindowMessage::SetEnabled(value) => {
      let _ = window_call(
        context,
        id,
        "window.enabled",
        json!({ "enabled": value }),
        IPC_TIMEOUT,
      );
    }
    WindowMessage::SetResizable(value) => {
      let _ = window_call(
        context,
        id,
        "window.resizable",
        json!({ "resizable": value }),
        IPC_TIMEOUT,
      );
    }
    WindowMessage::SetMaximizable(value) => {
      let _ = window_call(
        context,
        id,
        "window.maximizable",
        json!({ "maximizable": value }),
        IPC_TIMEOUT,
      );
    }
    WindowMessage::SetMinimizable(value) => {
      let _ = window_call(
        context,
        id,
        "window.minimizable",
        json!({ "minimizable": value }),
        IPC_TIMEOUT,
      );
    }
    WindowMessage::SetClosable(value) => {
      let _ = window_call(
        context,
        id,
        "window.closable",
        json!({ "closable": value }),
        IPC_TIMEOUT,
      );
    }
    WindowMessage::SetTitle(title) => {
      let _ = window_call(
        context,
        id,
        "window.setTitle",
        json!({ "title": title }),
        IPC_TIMEOUT,
      );
    }
    WindowMessage::Maximize => {
      let _ = window_call(context, id, "window.maximize", json!({}), IPC_TIMEOUT);
    }
    WindowMessage::Unmaximize => {
      let _ = window_call(context, id, "window.unmaximize", json!({}), IPC_TIMEOUT);
    }
    WindowMessage::Minimize => {
      let _ = window_call(context, id, "window.minimize", json!({}), IPC_TIMEOUT);
    }
    WindowMessage::Unminimize => {
      let _ = window_call(
        context,
        id,
        "window.unminimize",
        json!({}),
        UNMINIMIZE_TIMEOUT,
      );
    }
    WindowMessage::Show => {
      let _ = window_call(context, id, "window.show", json!({}), IPC_TIMEOUT);
    }
    WindowMessage::Hide => {
      let _ = window_call(context, id, "window.hide", json!({}), IPC_TIMEOUT);
    }
    WindowMessage::SetDecorations(value) => {
      let _ = window_call(
        context,
        id,
        "window.decorations",
        json!({ "value": value }),
        IPC_TIMEOUT,
      );
    }
    WindowMessage::SetShadow(value) => {
      // `setHasShadow` is real on macOS only: on Windows the shadow belongs to
      // DWM and Electron deliberately stopped touching it, and on Linux only the
      // client-side frame insets change.
      #[cfg(target_os = "macos")]
      {
        let _ = window_call(
          context,
          id,
          "window.shadow",
          json!({ "value": value }),
          IPC_TIMEOUT,
        );
      }
      #[cfg(not(target_os = "macos"))]
      {
        let _ = value;
        crate::unsupported::report("Window::set_shadow");
      }
    }
    WindowMessage::SetAlwaysOnBottom(value) => {
      if value {
        // Electron has no z-order below normal.
        crate::unsupported::report("Window::set_always_on_bottom");
      }
    }
    WindowMessage::SetAlwaysOnTop(value) => {
      if let Err(error) = window_call(
        context,
        id,
        "window.alwaysOnTop",
        json!({ "alwaysOnTop": value }),
        IPC_TIMEOUT,
      ) {
        log::warn!("[electron] set_always_on_top({value}) failed: {error}");
      }
    }
    WindowMessage::SetVisibleOnAllWorkspaces(value) => {
      #[cfg(any(
        target_os = "macos",
        target_os = "linux",
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd"
      ))]
      {
        let _ = window_call(
          context,
          id,
          "window.visibleOnAllWorkspaces",
          json!({ "value": value }),
          IPC_TIMEOUT,
        );
      }
      #[cfg(windows)]
      {
        let _ = value;
        // Electron documents `setVisibleOnAllWorkspaces` as doing nothing on
        // Windows, which is what the reference does there too.
        crate::unsupported::report("Window::set_visible_on_all_workspaces");
      }
    }
    WindowMessage::SetContentProtected(value) => {
      let _ = window_call(
        context,
        id,
        "window.contentProtected",
        json!({ "protected": value }),
        IPC_TIMEOUT,
      );
    }
    WindowMessage::SetSize(size) => {
      let scale = if size_scale(Some(size)) {
        primary_scale_factor(context)
      } else {
        1.0
      };
      let (width, height, physical) = size_payload(size, scale);
      let _ = window_call(
        context,
        id,
        "window.size",
        json!({ "width": width, "height": height, "physical": physical }),
        IPC_TIMEOUT,
      );
    }
    WindowMessage::SetMinSize(size) => {
      let params = size_axis_params(context, size);
      let _ = window_call(context, id, "window.minSize", params, IPC_TIMEOUT);
    }
    WindowMessage::SetMaxSize(size) => {
      let params = size_axis_params(context, size);
      let _ = window_call(context, id, "window.maxSize", params, IPC_TIMEOUT);
    }
    WindowMessage::SetSizeConstraints(constraints) => {
      // Electron takes the four axes independently, so the per-axis constraints
      // survive here instead of being collapsed into a min/max pair.
      let mut axes = Map::new();
      for (key, axis) in [
        ("minWidth", constraints.min_width),
        ("minHeight", constraints.min_height),
        ("maxWidth", constraints.max_width),
        ("maxHeight", constraints.max_height),
      ] {
        let Some(axis) = axis else { continue };
        let (value, physical) = match axis {
          PixelUnit::Logical(value) => (f64::from(value), false),
          PixelUnit::Physical(value) => (f64::from(value), true),
        };
        axes.insert(key.into(), json!({ "value": value, "physical": physical }));
      }
      let _ = window_call(
        context,
        id,
        "window.sizeConstraints",
        json!({ "constraints": Value::Object(axes) }),
        IPC_TIMEOUT,
      );
    }
    WindowMessage::SetPosition(position) => {
      let physical = matches!(position, Position::Physical(_));
      let scale = if physical {
        primary_scale_factor(context)
      } else {
        1.0
      };
      let position = position.to_logical::<f64>(scale);
      let _ = window_call(
        context,
        id,
        "window.position",
        json!({ "x": position.x, "y": position.y, "physical": physical }),
        IPC_TIMEOUT,
      );
    }
    WindowMessage::SetFullscreen(value) => {
      request_window_fullscreen(
        context,
        id,
        value.then_some(FullscreenTarget::CurrentMonitor),
      );
    }
    WindowMessage::SetFullscreenOnMonitor(position) => {
      request_window_fullscreen(context, id, Some(FullscreenTarget::MonitorAt(position)));
    }
    WindowMessage::ToggleFullscreen => toggle_fullscreen(context, id),
    #[cfg(target_os = "macos")]
    WindowMessage::ApplyPendingFullscreen => {
      // Electron holds no request back: `window.fullscreen` answers only once the
      // transition ended, so there is nothing to replay here.
    }
    #[cfg(target_os = "macos")]
    WindowMessage::SetSimpleFullscreen(value) => {
      let _ = window_call(
        context,
        id,
        "window.simpleFullscreen",
        json!({ "fullscreen": value }),
        FULLSCREEN_TIMEOUT,
      );
    }
    WindowMessage::SetFocus => {
      // On macOS `window.focus` already asks the application to activate with
      // `app.focus({ steal: true })` first, which is the reference's native nudge.
      let _ = window_call(context, id, "window.focus", json!({}), IPC_TIMEOUT);
    }
    WindowMessage::SetFocusable(value) => {
      let _ = window_call(
        context,
        id,
        "window.focusable",
        json!({ "focusable": value }),
        IPC_TIMEOUT,
      );
    }
    WindowMessage::SetIcon(icon) => {
      let _ = window_call(
        context,
        id,
        "window.icon",
        json!({ "icon": icon_payload(&icon) }),
        IPC_TIMEOUT,
      );
    }
    WindowMessage::SetSkipTaskbar(value) => {
      // Electron implements `setSkipTaskbar` on Windows only; on Linux and macOS
      // the call is an empty body, which is what the reference does there too.
      #[cfg(windows)]
      {
        let _ = window_call(
          context,
          id,
          "window.skipTaskbar",
          json!({ "skip": value }),
          IPC_TIMEOUT,
        );
      }
      #[cfg(not(windows))]
      {
        let _ = value;
        crate::unsupported::report("Window::set_skip_taskbar");
      }
    }
    WindowMessage::SetCursorGrab(value) => {
      let _ = value;
      // No Electron or Chromium API confines the OS cursor to a window; pointer
      // lock is page-initiated and not equivalent.
      crate::unsupported::report("Window::set_cursor_grab");
    }
    WindowMessage::SetCursorVisible(value) => {
      let _ = window_call(
        context,
        id,
        "window.setCursorVisible",
        json!({ "visible": value }),
        IPC_TIMEOUT,
      );
    }
    WindowMessage::SetCursorIcon(icon) => {
      let _ = window_call(
        context,
        id,
        "window.setCursorIcon",
        json!({ "cursor": tauri_cursor_to_css(icon) }),
        IPC_TIMEOUT,
      );
    }
    WindowMessage::SetCursorPosition(position) => {
      let _ = position;
      // No Electron API warps the OS cursor; it needs a native shim per platform.
      crate::unsupported::report("Window::set_cursor_position");
    }
    WindowMessage::SetIgnoreCursorEvents(value) => {
      let _ = window_call(
        context,
        id,
        "window.ignoreCursorEvents",
        json!({ "ignore": value }),
        IPC_TIMEOUT,
      );
    }
    WindowMessage::SetProgressBar(state) => {
      // Electron's range is 0..=1, a negative value removes the bar, and a value
      // above 1 is indeterminate on Windows.
      let progress = match state.status {
        None => -1.0,
        Some(ProgressBarStatus::Indeterminate) => 2.0,
        Some(_) => state
          .progress
          .map(|progress| progress as f64 / 100.0)
          .unwrap_or(0.0),
      };
      let status = match state.status {
        Some(ProgressBarStatus::Paused) => "paused",
        Some(ProgressBarStatus::Error) => "error",
        None => "none",
        _ => "normal",
      };
      let _ = window_call(
        context,
        id,
        "window.progressBar",
        json!({ "state": status, "progress": progress }),
        IPC_TIMEOUT,
      );
    }
    WindowMessage::SetBadgeCount(count, desktop_filename) => {
      if desktop_filename.is_some() {
        // Electron resolves the launcher entry from the application's own
        // `.desktop` file; a per-call filename has no counterpart.
        crate::unsupported::report("Window::set_badge_count: desktop_filename");
      }
      // The command is app-wide and takes an unsigned count: `None` clears it and
      // a negative count clamps to 0.
      let count = count.unwrap_or(0).max(0) as u32;
      let _ = context
        .ipc
        .call("window.badgeCount", json!({ "count": count }), IPC_TIMEOUT);
    }
    WindowMessage::SetBadgeLabel(label) => {
      #[cfg(target_os = "macos")]
      {
        let _ = context
          .ipc
          .call("app.badgeLabel", json!({ "label": label }), IPC_TIMEOUT);
      }
      #[cfg(not(target_os = "macos"))]
      {
        let _ = label;
        // A dock label exists on macOS only; the reference no-ops elsewhere too.
        crate::unsupported::report("Window::set_badge_label");
      }
    }
    WindowMessage::SetOverlayIcon(icon) => {
      #[cfg(windows)]
      {
        let _ = window_call(
          context,
          id,
          "window.overlayIcon",
          json!({
            "icon": icon.as_ref().map(icon_payload),
            "description": "",
          }),
          IPC_TIMEOUT,
        );
      }
      #[cfg(not(windows))]
      {
        let _ = icon;
        // `setOverlayIcon` is a Windows taskbar feature; macOS has an empty body
        // and Linux has nothing at all.
        crate::unsupported::report("Window::set_overlay_icon");
      }
    }
    WindowMessage::SetTitleBarStyle(style) => {
      #[cfg(target_os = "macos")]
      {
        let style = match style {
          tauri_utils::TitleBarStyle::Transparent => "transparent",
          tauri_utils::TitleBarStyle::Overlay => "overlay",
          _ => "visible",
        };
        let _ = window_call(
          context,
          id,
          "window.titleBarStyle",
          json!({ "style": style }),
          IPC_TIMEOUT,
        );
      }
      #[cfg(not(target_os = "macos"))]
      {
        let _ = style;
        // Electron's title bar style is a constructor option; it cannot be
        // rebuilt after creation.
        crate::unsupported::report("Window::set_title_bar_style");
      }
    }
    WindowMessage::SetTrafficLightPosition(position) => {
      #[cfg(target_os = "macos")]
      {
        let physical = matches!(position, Position::Physical(_));
        let position = position.to_logical::<f64>(1.0);
        let _ = window_call(
          context,
          id,
          "window.trafficLight",
          json!({ "x": position.x, "y": position.y, "physical": physical }),
          IPC_TIMEOUT,
        );
      }
      #[cfg(not(target_os = "macos"))]
      {
        let _ = position;
        crate::unsupported::report("Window::set_traffic_light_position");
      }
    }
    WindowMessage::SetTheme(theme) => {
      let _ = window_call(
        context,
        id,
        "window.theme",
        json!({ "theme": tauri_theme_to_native_theme(theme) }),
        IPC_TIMEOUT,
      );
    }
    WindowMessage::SetBackgroundColor(color) => {
      let color = color.map(|color| json!([color.0, color.1, color.2, color.3]));
      let _ = window_call(
        context,
        id,
        "window.backgroundColor",
        json!({ "color": color }),
        IPC_TIMEOUT,
      );
    }
    WindowMessage::StartDragging => {
      #[cfg(target_os = "macos")]
      {
        let _ = window_call(context, id, "window.startDrag", json!({}), IPC_TIMEOUT);
      }
      #[cfg(not(target_os = "macos"))]
      {
        // Windows and Linux need a native shim (`ReleaseCapture` plus
        // `WM_NCLBUTTONDOWN`, or `_NET_WM_MOVERESIZE`); the bridge has neither.
        crate::unsupported::report("Window::start_dragging");
      }
    }
    WindowMessage::StartResizeDragging(direction) => {
      let _ = direction;
      // Neither Electron nor the bridge starts a native resize drag; macOS has no
      // public equivalent at all.
      crate::unsupported::report("Window::start_resize_dragging");
    }
  }
}

/// The `window.minSize` / `window.maxSize` payload for one axis pair.
fn size_axis_params<T: UserEvent>(context: &RuntimeContext<T>, size: Option<Size>) -> Value {
  let Some(size) = size else {
    // Absent axes clear the constraint.
    return json!({});
  };
  let physical = size_scale(Some(size));
  let scale = if physical {
    primary_scale_factor(context)
  } else {
    1.0
  };
  let (width, height, physical) = size_payload(size, scale);
  json!({ "width": width, "height": height, "physical": physical })
}

#[derive(Debug, Clone)]
pub struct ElectronWindowDispatcher<T: UserEvent> {
  pub(crate) window_id: WindowId,
  pub(crate) context: RuntimeContext<T>,
}

fn getter<T: UserEvent, R>(
  context: &RuntimeContext<T>,
  message: Message<T>,
  receiver: Receiver<Result<R>>,
) -> Result<R> {
  context.send_message(message)?;
  receiver.recv().map_err(|_| Error::FailedToReceiveMessage)?
}

macro_rules! window_getter {
  ($self:ident, $variant:ident) => {{
    let (tx, rx) = mpsc::channel();
    getter(
      &$self.context,
      Message::Window($self.window_id, WindowMessage::$variant(tx)),
      rx,
    )
  }};
}

impl<T: UserEvent> WindowDispatch<T> for ElectronWindowDispatcher<T> {
  type Runtime = ElectronRuntime<T>;
  type WindowBuilder = WindowBuilderWrapper;

  fn run_on_main_thread<F: FnOnce() + Send + 'static>(&self, f: F) -> Result<()> {
    self.context.send_message(Message::Task(Box::new(f)))
  }

  fn scale_factor(&self) -> Result<f64> {
    window_getter!(self, ScaleFactor)
  }

  fn inner_position(&self) -> Result<PhysicalPosition<i32>> {
    window_getter!(self, InnerPosition)
  }

  fn outer_position(&self) -> Result<PhysicalPosition<i32>> {
    window_getter!(self, OuterPosition)
  }

  fn inner_size(&self) -> Result<PhysicalSize<u32>> {
    window_getter!(self, InnerSize)
  }

  fn outer_size(&self) -> Result<PhysicalSize<u32>> {
    window_getter!(self, OuterSize)
  }

  fn is_fullscreen(&self) -> Result<bool> {
    window_getter!(self, IsFullscreen)
  }

  fn is_minimized(&self) -> Result<bool> {
    window_getter!(self, IsMinimized)
  }

  fn is_maximized(&self) -> Result<bool> {
    window_getter!(self, IsMaximized)
  }

  fn is_focused(&self) -> Result<bool> {
    window_getter!(self, IsFocused)
  }

  fn is_decorated(&self) -> Result<bool> {
    window_getter!(self, IsDecorated)
  }

  fn is_resizable(&self) -> Result<bool> {
    window_getter!(self, IsResizable)
  }

  fn is_maximizable(&self) -> Result<bool> {
    window_getter!(self, IsMaximizable)
  }

  fn is_minimizable(&self) -> Result<bool> {
    window_getter!(self, IsMinimizable)
  }

  fn is_closable(&self) -> Result<bool> {
    window_getter!(self, IsClosable)
  }

  fn is_visible(&self) -> Result<bool> {
    window_getter!(self, IsVisible)
  }

  fn is_enabled(&self) -> Result<bool> {
    window_getter!(self, IsEnabled)
  }

  fn is_always_on_top(&self) -> Result<bool> {
    window_getter!(self, IsAlwaysOnTop)
  }

  fn title(&self) -> Result<String> {
    window_getter!(self, Title)
  }

  fn current_monitor(&self) -> Result<Option<Monitor>> {
    window_getter!(self, CurrentMonitor)
  }

  fn primary_monitor(&self) -> Result<Option<Monitor>> {
    window_getter!(self, PrimaryMonitor)
  }

  fn monitor_from_point(&self, x: f64, y: f64) -> Result<Option<Monitor>> {
    let (tx, rx) = mpsc::channel();
    getter(
      &self.context,
      Message::Window(self.window_id, WindowMessage::MonitorFromPoint(tx, x, y)),
      rx,
    )
  }

  fn available_monitors(&self) -> Result<Vec<Monitor>> {
    window_getter!(self, AvailableMonitors)
  }

  #[cfg(any(
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd"
  ))]
  fn gtk_window(&self) -> Result<*mut std::ffi::c_void> {
    window_getter!(self, GtkWindow).map(|gtk_window| gtk_window.0)
  }

  #[cfg(any(
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd"
  ))]
  fn default_vbox(&self) -> Result<*mut std::ffi::c_void> {
    window_getter!(self, DefaultVBox).map(|gtk_box| gtk_box.0)
  }

  fn window_handle(
    &self,
  ) -> std::result::Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
    let handle: Result<SendRawWindowHandle> = window_getter!(self, RawWindowHandle);
    let handle = handle.map_err(|_| raw_window_handle::HandleError::Unavailable)?;
    Ok(unsafe { raw_window_handle::WindowHandle::borrow_raw(handle.0) })
  }

  fn theme(&self) -> Result<Theme> {
    window_getter!(self, Theme)
  }

  fn center(&self) -> Result<()> {
    self
      .context
      .send_message(Message::Window(self.window_id, WindowMessage::Center))
  }

  fn request_user_attention(&self, request_type: Option<UserAttentionType>) -> Result<()> {
    self.context.send_message(Message::Window(
      self.window_id,
      WindowMessage::RequestUserAttention(request_type),
    ))
  }

  fn create_window<F: Fn(RawWindow) + Send + 'static>(
    &mut self,
    pending: PendingWindow<T, Self::Runtime>,
    after_window_creation: Option<F>,
  ) -> Result<DetachedWindow<T, Self::Runtime>> {
    create_window_detached(&self.context, pending, after_window_creation)
  }

  fn create_webview(
    &mut self,
    pending: PendingWebview<T, Self::Runtime>,
  ) -> Result<DetachedWebview<T, Self::Runtime>> {
    create_webview_detached(&self.context, self.window_id, pending)
  }

  fn set_resizable(&self, resizable: bool) -> Result<()> {
    self.context.send_message(Message::Window(
      self.window_id,
      WindowMessage::SetResizable(resizable),
    ))
  }

  fn set_enabled(&self, enabled: bool) -> Result<()> {
    self.context.send_message(Message::Window(
      self.window_id,
      WindowMessage::SetEnabled(enabled),
    ))
  }

  fn set_maximizable(&self, maximizable: bool) -> Result<()> {
    self.context.send_message(Message::Window(
      self.window_id,
      WindowMessage::SetMaximizable(maximizable),
    ))
  }

  fn set_minimizable(&self, minimizable: bool) -> Result<()> {
    self.context.send_message(Message::Window(
      self.window_id,
      WindowMessage::SetMinimizable(minimizable),
    ))
  }

  fn set_closable(&self, closable: bool) -> Result<()> {
    self.context.send_message(Message::Window(
      self.window_id,
      WindowMessage::SetClosable(closable),
    ))
  }

  fn set_title<S: Into<String>>(&self, title: S) -> Result<()> {
    self.context.send_message(Message::Window(
      self.window_id,
      WindowMessage::SetTitle(title.into()),
    ))
  }

  fn maximize(&self) -> Result<()> {
    self
      .context
      .send_message(Message::Window(self.window_id, WindowMessage::Maximize))
  }

  fn unmaximize(&self) -> Result<()> {
    self
      .context
      .send_message(Message::Window(self.window_id, WindowMessage::Unmaximize))
  }

  fn minimize(&self) -> Result<()> {
    self
      .context
      .send_message(Message::Window(self.window_id, WindowMessage::Minimize))
  }

  fn unminimize(&self) -> Result<()> {
    self
      .context
      .send_message(Message::Window(self.window_id, WindowMessage::Unminimize))
  }

  fn show(&self) -> Result<()> {
    self
      .context
      .send_message(Message::Window(self.window_id, WindowMessage::Show))
  }

  fn hide(&self) -> Result<()> {
    self
      .context
      .send_message(Message::Window(self.window_id, WindowMessage::Hide))
  }

  fn close(&self) -> Result<()> {
    self
      .context
      .send_message(Message::Window(self.window_id, WindowMessage::Close))
  }

  fn destroy(&self) -> Result<()> {
    self
      .context
      .send_message(Message::Window(self.window_id, WindowMessage::Destroy))
  }

  fn set_decorations(&self, decorations: bool) -> Result<()> {
    self.context.send_message(Message::Window(
      self.window_id,
      WindowMessage::SetDecorations(decorations),
    ))
  }

  fn set_shadow(&self, enable: bool) -> Result<()> {
    self.context.send_message(Message::Window(
      self.window_id,
      WindowMessage::SetShadow(enable),
    ))
  }

  fn set_always_on_bottom(&self, always_on_bottom: bool) -> Result<()> {
    self.context.send_message(Message::Window(
      self.window_id,
      WindowMessage::SetAlwaysOnBottom(always_on_bottom),
    ))
  }

  fn set_always_on_top(&self, always_on_top: bool) -> Result<()> {
    self.context.send_message(Message::Window(
      self.window_id,
      WindowMessage::SetAlwaysOnTop(always_on_top),
    ))
  }

  fn set_visible_on_all_workspaces(&self, visible_on_all_workspaces: bool) -> Result<()> {
    self.context.send_message(Message::Window(
      self.window_id,
      WindowMessage::SetVisibleOnAllWorkspaces(visible_on_all_workspaces),
    ))
  }

  fn set_background_color(&self, color: Option<Color>) -> Result<()> {
    self.context.send_message(Message::Window(
      self.window_id,
      WindowMessage::SetBackgroundColor(color),
    ))
  }

  fn set_content_protected(&self, protected: bool) -> Result<()> {
    self.context.send_message(Message::Window(
      self.window_id,
      WindowMessage::SetContentProtected(protected),
    ))
  }

  fn set_size(&self, size: Size) -> Result<()> {
    self.context.send_message(Message::Window(
      self.window_id,
      WindowMessage::SetSize(size),
    ))
  }

  fn set_min_size(&self, size: Option<Size>) -> Result<()> {
    self.context.send_message(Message::Window(
      self.window_id,
      WindowMessage::SetMinSize(size),
    ))
  }

  fn set_max_size(&self, size: Option<Size>) -> Result<()> {
    self.context.send_message(Message::Window(
      self.window_id,
      WindowMessage::SetMaxSize(size),
    ))
  }

  fn set_size_constraints(&self, constraints: WindowSizeConstraints) -> Result<()> {
    self.context.send_message(Message::Window(
      self.window_id,
      WindowMessage::SetSizeConstraints(constraints),
    ))
  }

  fn set_position(&self, position: Position) -> Result<()> {
    self.context.send_message(Message::Window(
      self.window_id,
      WindowMessage::SetPosition(position),
    ))
  }

  fn set_fullscreen(&self, fullscreen: bool) -> Result<()> {
    self.context.send_message(Message::Window(
      self.window_id,
      WindowMessage::SetFullscreen(fullscreen),
    ))
  }

  fn set_fullscreen_on_monitor(&self, position: PhysicalPosition<f64>) -> Result<()> {
    self.context.send_message(Message::Window(
      self.window_id,
      WindowMessage::SetFullscreenOnMonitor(position),
    ))
  }

  #[cfg(target_os = "macos")]
  fn set_simple_fullscreen(&self, enable: bool) -> Result<()> {
    self.context.send_message(Message::Window(
      self.window_id,
      WindowMessage::SetSimpleFullscreen(enable),
    ))
  }

  fn set_focus(&self) -> Result<()> {
    self
      .context
      .send_message(Message::Window(self.window_id, WindowMessage::SetFocus))
  }

  fn set_focusable(&self, focusable: bool) -> Result<()> {
    self.context.send_message(Message::Window(
      self.window_id,
      WindowMessage::SetFocusable(focusable),
    ))
  }

  fn set_icon(&self, icon: Icon) -> Result<()> {
    self.context.send_message(Message::Window(
      self.window_id,
      WindowMessage::SetIcon(icon.into_owned()),
    ))
  }

  fn set_skip_taskbar(&self, skip: bool) -> Result<()> {
    self.context.send_message(Message::Window(
      self.window_id,
      WindowMessage::SetSkipTaskbar(skip),
    ))
  }

  fn set_cursor_grab(&self, grab: bool) -> Result<()> {
    self.context.send_message(Message::Window(
      self.window_id,
      WindowMessage::SetCursorGrab(grab),
    ))
  }

  fn set_cursor_visible(&self, visible: bool) -> Result<()> {
    self.context.send_message(Message::Window(
      self.window_id,
      WindowMessage::SetCursorVisible(visible),
    ))
  }

  fn set_cursor_icon(&self, icon: CursorIcon) -> Result<()> {
    self.context.send_message(Message::Window(
      self.window_id,
      WindowMessage::SetCursorIcon(icon),
    ))
  }

  fn set_cursor_position<Pos: Into<Position>>(&self, position: Pos) -> Result<()> {
    self.context.send_message(Message::Window(
      self.window_id,
      WindowMessage::SetCursorPosition(position.into()),
    ))
  }

  fn set_ignore_cursor_events(&self, ignore: bool) -> Result<()> {
    self.context.send_message(Message::Window(
      self.window_id,
      WindowMessage::SetIgnoreCursorEvents(ignore),
    ))
  }

  fn start_dragging(&self) -> Result<()> {
    self.context.send_message(Message::Window(
      self.window_id,
      WindowMessage::StartDragging,
    ))
  }

  fn start_resize_dragging(&self, direction: tauri_runtime::ResizeDirection) -> Result<()> {
    self.context.send_message(Message::Window(
      self.window_id,
      WindowMessage::StartResizeDragging(direction),
    ))
  }

  fn set_badge_count(&self, count: Option<i64>, desktop_filename: Option<String>) -> Result<()> {
    self.context.send_message(Message::Window(
      self.window_id,
      WindowMessage::SetBadgeCount(count, desktop_filename),
    ))
  }

  fn set_badge_label(&self, label: Option<String>) -> Result<()> {
    self.context.send_message(Message::Window(
      self.window_id,
      WindowMessage::SetBadgeLabel(label),
    ))
  }

  fn set_overlay_icon(&self, icon: Option<Icon>) -> Result<()> {
    self.context.send_message(Message::Window(
      self.window_id,
      WindowMessage::SetOverlayIcon(icon.map(Icon::into_owned)),
    ))
  }

  fn set_progress_bar(&self, progress_state: ProgressBarState) -> Result<()> {
    self.context.send_message(Message::Window(
      self.window_id,
      WindowMessage::SetProgressBar(progress_state),
    ))
  }

  fn set_title_bar_style(&self, style: tauri_utils::TitleBarStyle) -> Result<()> {
    self.context.send_message(Message::Window(
      self.window_id,
      WindowMessage::SetTitleBarStyle(style),
    ))
  }

  fn set_traffic_light_position(&self, position: Position) -> Result<()> {
    self.context.send_message(Message::Window(
      self.window_id,
      WindowMessage::SetTrafficLightPosition(position),
    ))
  }

  fn set_theme(&self, theme: Option<Theme>) -> Result<()> {
    self.context.send_message(Message::Window(
      self.window_id,
      WindowMessage::SetTheme(theme),
    ))
  }
}

/// Creates a window and waits for Electron to build it.
///
/// The reference posts `Message::CreateWindow` and waits for the event loop. The
/// mirror has no creation message — the run loop only drains Electron's events —
/// so the window is created here, on the caller's thread, and registered before
/// anything else can observe it.
pub(crate) fn create_window_detached<T, F>(
  context: &RuntimeContext<T>,
  pending: PendingWindow<T, ElectronRuntime<T>>,
  after_window_creation: Option<F>,
) -> Result<DetachedWindow<T, ElectronRuntime<T>>>
where
  T: UserEvent,
  F: Fn(RawWindow) + Send + 'static,
{
  let label = pending.label.clone();
  let window_id = context.next_window_id();
  let number = window_number(window_id);
  let (use_https_scheme, devtools) = pending
    .webview
    .as_ref()
    .map(|webview| {
      (
        webview.webview_attributes.use_https_scheme,
        webview.webview_attributes.devtools,
      )
    })
    .unwrap_or((false, None));

  let mut attrs = pending.window_builder.attrs.clone();
  if attrs.theme.is_none() {
    attrs.theme = context.theme();
  }
  let config = window_config(context, &attrs)?;

  let mut params = Map::new();
  params.insert("windowId".into(), json!(number));
  params.insert("window".into(), config);
  // The window is created hidden and revealed by `window.ready` once its webview
  // exists, so the user never sees an empty frame.
  params.insert("deferShow".into(), json!(true));
  if let Some(icon) = creation_icon(attrs.icon.as_ref()) {
    params.insert("windowIcon".into(), icon);
  }
  context
    .ipc
    .call("window.create", Value::Object(params), IPC_TIMEOUT)
    .map_err(create_window_err)?;

  webview::record_window_number(window_id, number);
  let center = attrs.center;
  context.insert_window(
    window_id,
    AppWindow {
      lifetime: NativeWindowToken::new(),
      id: window_id,
      label: label.clone(),
      attrs,
    },
  );

  // The window exists from here on, so every failure below has to take it down
  // again instead of leaving a detached, dead window behind.
  let rollback = |context: &RuntimeContext<T>, window_id: WindowId, number: u32| {
    let _ = context
      .ipc
      .call("window.destroy", json!({ "windowId": number }), IPC_TIMEOUT);
    context.remove_window(window_id);
    webview::forget_window_number(window_id);
  };

  if let Some(after_window_creation) = after_window_creation {
    match context.native_handle(window_id) {
      Ok(handle) => after_window_creation(raw_window(handle)),
      Err(error) => {
        rollback(context, window_id, number);
        return Err(error);
      }
    }
  }

  let webview = match pending.webview {
    Some(pending_webview) => {
      let webview = match create_webview_detached(context, window_id, pending_webview) {
        Ok(webview) => webview,
        Err(error) => {
          rollback(context, window_id, number);
          return Err(error);
        }
      };
      Some(DetachedWindowWebview {
        webview,
        use_https_scheme,
        devtools,
      })
    }
    None => None,
  };

  // `window.ready` is what maps the window, honouring `visible`, `focus` and
  // `focusable`; a window created invisible stays invisible.
  context
    .ipc
    .call("window.ready", json!({ "windowId": number }), IPC_TIMEOUT)
    .map_err(create_window_err)?;

  // Electron centres a window at creation on macOS only; elsewhere the request
  // has to be made once the window exists.
  if center && !cfg!(target_os = "macos") {
    let _ = window_call(context, window_id, "window.center", json!({}), IPC_TIMEOUT);
  }

  Ok(DetachedWindow {
    id: window_id,
    label,
    dispatcher: ElectronWindowDispatcher {
      window_id,
      context: context.clone(),
    },
    webview,
  })
}
