// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

use tauri_runtime::{
  Error, Icon, Result,
  dpi::{PhysicalSize, Size},
  window::{WindowBuilder, WindowBuilderBase, WindowSizeConstraints},
};
use tauri_utils::{
  Theme,
  config::{Color, PreventOverflowConfig, WindowConfig},
};

use crate::window::{
  AppWindowAttrs, max_size_constraint, min_size_constraint, tauri_icon_is_valid,
};

#[cfg(target_os = "macos")]
use tauri_runtime::dpi::Position;

#[cfg(windows)]
use windows::Win32::Foundation::HWND;

#[cfg(target_os = "macos")]
use tauri_utils::TitleBarStyle;

/// The `WindowBuilder` implementation.
///
/// The reference wraps winit's `WindowAttributes`, which is where every attribute
/// it can set lives. Electron has no such object: a window is a `window.create`
/// payload, so [`AppWindowAttrs`] carries the same attributes as plain fields and
/// [`crate::window::create_window_detached`] turns them into that payload.
#[derive(Clone, Default, Debug)]
pub struct WindowBuilderWrapper {
  pub(crate) attrs: AppWindowAttrs,
}

impl WindowBuilderBase for WindowBuilderWrapper {}

impl WindowBuilder for WindowBuilderWrapper {
  fn new() -> Self {
    Self {
      attrs: AppWindowAttrs {
        title: Some("Tauri App".into()),
        visible: true,
        focused: true,
        ..Default::default()
      },
    }
  }

  fn with_config(config: &WindowConfig) -> Self {
    let mut builder = Self::new()
      .title(config.title.to_string())
      .inner_size(config.width, config.height)
      .resizable(config.resizable)
      .fullscreen(config.fullscreen)
      .focused(config.focus)
      .focusable(config.focusable)
      .visible(config.visible)
      .decorations(config.decorations)
      .maximized(config.maximized)
      .content_protected(config.content_protected)
      .closable(config.closable)
      .maximizable(config.maximizable)
      .minimizable(config.minimizable)
      .skip_taskbar(config.skip_taskbar)
      .shadow(config.shadow)
      .visible_on_all_workspaces(config.visible_on_all_workspaces)
      .theme(config.theme);
    if config.always_on_bottom {
      builder = builder.always_on_bottom(true);
    } else if config.always_on_top {
      builder = builder.always_on_top(true);
    }
    builder = builder.transparent(config.transparent);
    let mut constraints = WindowSizeConstraints::default();
    if let Some(min_width) = config.min_width {
      constraints.min_width = Some(tauri_runtime::dpi::LogicalUnit::new(min_width).into());
    }
    if let Some(min_height) = config.min_height {
      constraints.min_height = Some(tauri_runtime::dpi::LogicalUnit::new(min_height).into());
    }
    if let Some(max_width) = config.max_width {
      constraints.max_width = Some(tauri_runtime::dpi::LogicalUnit::new(max_width).into());
    }
    if let Some(max_height) = config.max_height {
      constraints.max_height = Some(tauri_runtime::dpi::LogicalUnit::new(max_height).into());
    }
    builder = builder.inner_size_constraints(constraints);
    if let Some(color) = config.background_color {
      builder = builder.background_color(color);
    }
    if let (Some(x), Some(y)) = (config.x, config.y) {
      builder = builder.position(x, y);
    }
    #[cfg(target_os = "macos")]
    {
      builder = builder
        .hidden_title(config.hidden_title)
        .title_bar_style(config.title_bar_style);
      if let Some(position) = &config.traffic_light_position {
        builder = builder.traffic_light_position(tauri_runtime::dpi::LogicalPosition::new(
          position.x, position.y,
        ));
      }
      if let Some(identifier) = &config.tabbing_identifier {
        builder = builder.tabbing_identifier(identifier);
      }
    }
    if config.center {
      builder = builder.center();
    }
    if let Some(window_classname) = &config.window_classname {
      builder = builder.window_classname(window_classname);
    }
    if let Some(prevent_overflow) = &config.prevent_overflow {
      builder = match prevent_overflow {
        PreventOverflowConfig::Enable(true) => builder.prevent_overflow(),
        PreventOverflowConfig::Margin(margin) => {
          let margin = PhysicalSize::new(margin.width, margin.height);
          builder.prevent_overflow_with_margin(margin.into())
        }
        _ => builder,
      };
    }
    builder
  }

  fn center(mut self) -> Self {
    self.attrs.center = true;
    self
  }

  fn position(mut self, x: f64, y: f64) -> Self {
    self.attrs.position = Some(tauri_runtime::dpi::LogicalPosition::new(x, y).into());
    self
  }

  fn inner_size(mut self, width: f64, height: f64) -> Self {
    self.attrs.inner_size = Some(tauri_runtime::dpi::LogicalSize::new(width, height).into());
    self
  }

  fn min_inner_size(mut self, min_width: f64, min_height: f64) -> Self {
    self.attrs.min_inner_size =
      Some(tauri_runtime::dpi::LogicalSize::new(min_width, min_height).into());
    self
  }

  fn max_inner_size(mut self, max_width: f64, max_height: f64) -> Self {
    self.attrs.max_inner_size =
      Some(tauri_runtime::dpi::LogicalSize::new(max_width, max_height).into());
    self
  }

  fn inner_size_constraints(mut self, constraints: WindowSizeConstraints) -> Self {
    self.attrs.min_inner_size = min_size_constraint(constraints.min_width, constraints.min_height);
    self.attrs.max_inner_size = max_size_constraint(constraints.max_width, constraints.max_height);
    self
  }

  fn prevent_overflow(mut self) -> Self {
    self.attrs.prevent_overflow = Some(PhysicalSize::new(0, 0).into());
    self
  }

  fn prevent_overflow_with_margin(mut self, margin: Size) -> Self {
    self.attrs.prevent_overflow = Some(margin);
    self
  }

  fn resizable(mut self, resizable: bool) -> Self {
    self.attrs.resizable = resizable;
    self
  }

  fn maximizable(mut self, maximizable: bool) -> Self {
    self.attrs.maximizable = maximizable;
    self
  }

  fn minimizable(mut self, minimizable: bool) -> Self {
    self.attrs.minimizable = minimizable;
    self
  }

  fn closable(mut self, closable: bool) -> Self {
    self.attrs.closable = closable;
    self
  }

  fn title<S: Into<String>>(mut self, title: S) -> Self {
    self.attrs.title = Some(title.into());
    self
  }

  fn fullscreen(mut self, fullscreen: bool) -> Self {
    self.attrs.fullscreen = fullscreen;
    self
  }

  fn focused(mut self, focused: bool) -> Self {
    self.attrs.focused = focused;
    self
  }

  /// The reference cannot express this yet and drops it. Electron's
  /// `window.create` takes `focusable`, so the mirror records it and the window
  /// is created with it — the one builder attribute where the mirror is strictly
  /// more capable than the reference.
  fn focusable(mut self, focusable: bool) -> Self {
    self.attrs.focusable = focusable;
    self
  }

  fn maximized(mut self, maximized: bool) -> Self {
    self.attrs.maximized = maximized;
    self
  }

  fn visible(mut self, visible: bool) -> Self {
    self.attrs.visible = visible;
    self
  }

  fn transparent(mut self, transparent: bool) -> Self {
    self.attrs.transparent = transparent;
    self
  }

  fn decorations(mut self, decorations: bool) -> Self {
    self.attrs.decorations = decorations;
    self
  }

  fn always_on_bottom(mut self, always_on_bottom: bool) -> Self {
    self.attrs.always_on_bottom = always_on_bottom;
    self
  }

  fn always_on_top(mut self, always_on_top: bool) -> Self {
    self.attrs.always_on_top = always_on_top;
    self
  }

  fn visible_on_all_workspaces(mut self, visible_on_all_workspaces: bool) -> Self {
    self.attrs.visible_on_all_workspaces = visible_on_all_workspaces;
    self
  }

  fn content_protected(mut self, protected: bool) -> Self {
    self.attrs.content_protected = protected;
    self
  }

  fn icon(mut self, icon: Icon) -> Result<Self> {
    if !tauri_icon_is_valid(&icon) {
      return Err(Error::InvalidIcon(
        "the icon buffer does not match its size".into(),
      ));
    }
    self.attrs.icon = Some(icon.into_owned());
    Ok(self)
  }

  fn skip_taskbar(mut self, skip: bool) -> Self {
    self.attrs.skip_taskbar = skip;
    self
  }

  fn background_color(mut self, color: Color) -> Self {
    self.attrs.background_color = Some(color);
    self
  }

  fn shadow(mut self, enable: bool) -> Self {
    self.attrs.shadow = enable;
    self
  }

  /// An owned window needs a native parent handle. Electron parents windows by
  /// *window id* (`window.setParentWindow`), which a builder cannot name before
  /// the window exists, so the handle is recorded and never applied.
  #[cfg(windows)]
  fn owner(mut self, owner: HWND) -> Self {
    crate::unsupported::report("WindowBuilder::owner");
    self.attrs.owner = Some(owner.0 as isize);
    self
  }

  /// A child window is a native concept Electron's `BaseWindow` does not have.
  #[cfg(windows)]
  fn parent(mut self, parent: HWND) -> Self {
    crate::unsupported::report("WindowBuilder::parent");
    self.attrs.parent = Some(parent.0 as isize);
    self
  }

  #[cfg(windows)]
  fn drag_and_drop(mut self, enabled: bool) -> Self {
    crate::unsupported::report("WindowBuilder::drag_and_drop");
    self.attrs.drag_and_drop = Some(enabled);
    self
  }

  /// A macOS child window is added with `addChildWindow:`, which needs the parent
  /// `NSWindow*`. Electron's equivalent parents by window id, which a builder
  /// cannot name before the window exists.
  ///
  /// The reference adopts the parent `NSWindow` to read its content view; this
  /// runtime does not adopt it, so the reference it was handed is released here
  /// and the pointer is only recorded.
  #[cfg(target_os = "macos")]
  fn parent(mut self, parent: *mut std::ffi::c_void) -> Self {
    use objc2::rc::Retained;
    use objc2_app_kit::NSWindow;

    crate::unsupported::report("WindowBuilder::parent");
    if let Some(nswindow) = std::ptr::NonNull::new(parent) {
      // SAFETY: the caller transferred a reference to this `NSWindow`, as the
      // reference implementation assumes when it adopts it.
      let _ = unsafe { Retained::<NSWindow>::from_raw(nswindow.as_ptr() as _) };
    }
    self.attrs.macos_parent = Some(parent as isize);
    self
  }

  /// `transient_for` receives the parent as transfer full, and the implementation
  /// must release that reference even when it does not support transient windows.
  /// The reference adopts it into the attribute; Electron parents by window id, so
  /// the reference is released here instead and only the pointer is recorded.
  #[cfg(any(
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd"
  ))]
  fn transient_for(mut self, parent: *mut std::ffi::c_void) -> Self {
    use gtk::glib::translate::FromGlibPtrFull;

    crate::unsupported::report("WindowBuilder::transient_for");
    if !parent.is_null() {
      // SAFETY: `transient_for` receives the parent as transfer full, so the
      // wrapper adopts the reference and releases it when it is dropped here.
      let _ = unsafe { gtk::Window::from_glib_full(parent as *mut gtk::ffi::GtkWindow) };
    }
    self.attrs.transient_for = Some(parent as isize);
    self
  }

  #[cfg(target_os = "macos")]
  fn title_bar_style(mut self, style: TitleBarStyle) -> Self {
    self.attrs.title_bar_style = Some(style);
    self
  }

  #[cfg(target_os = "macos")]
  fn traffic_light_position<P: Into<Position>>(mut self, position: P) -> Self {
    self.attrs.traffic_light_position = Some(position.into());
    self
  }

  #[cfg(target_os = "macos")]
  fn hidden_title(mut self, hidden: bool) -> Self {
    self.attrs.hidden_title = hidden;
    self
  }

  #[cfg(target_os = "macos")]
  fn tabbing_identifier(mut self, identifier: &str) -> Self {
    self.attrs.tabbing_identifier = Some(identifier.to_string());
    self
  }

  fn theme(mut self, theme: Option<Theme>) -> Self {
    self.attrs.theme = theme;
    self
  }

  /// Electron windows have no class name to set: the window is a `BaseWindow`
  /// whose class belongs to the Electron binary.
  fn window_classname<S: Into<String>>(mut self, window_classname: S) -> Self {
    crate::unsupported::report("WindowBuilder::window_classname");
    self.attrs.window_classname = Some(window_classname.into());
    self
  }

  /// `WS_EX_NOREDIRECTIONBITMAP` is a Win32 style. Electron decides its own
  /// redirection surface, so there is nothing to set.
  fn no_redirection_bitmap(self, _enable: bool) -> Self {
    crate::unsupported::report("WindowBuilder::no_redirection_bitmap");
    self
  }

  fn has_icon(&self) -> bool {
    self.attrs.icon.is_some()
  }

  fn get_theme(&self) -> Option<Theme> {
    self.attrs.theme
  }
}
