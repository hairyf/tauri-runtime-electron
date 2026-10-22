// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

use tauri_runtime::{
  Icon, Result,
  dpi::{PixelUnit, Size},
  window::{WindowBuilder, WindowBuilderBase, WindowSizeConstraints},
};
use tauri_utils::{
  Theme,
  config::{PreventOverflowConfig, PreventOverflowMargin, WindowConfig},
};

#[cfg(target_os = "macos")]
use tauri_runtime::dpi::Position;

#[cfg(target_os = "macos")]
use tauri_utils::TitleBarStyle;

#[cfg(windows)]
use windows::Win32::Foundation::HWND;

/// RGBA icon handed to `WindowBuilder::icon`; the runtime turns it into
/// something Electron can paint.
#[derive(Debug, Clone)]
pub(crate) struct IconData {
  pub rgba: Vec<u8>,
  pub width: u32,
  pub height: u32,
}

/// Mirrors the window onto an Electron `BrowserWindow`.
///
/// Every builder option lands in [`WindowConfig`] (serialized into the
/// `window.create` payload) or in one of the fields below, so nothing is
/// silently dropped. Options Electron cannot express are listed in
/// `unsupported` and reported when the window is created.
#[derive(Debug, Clone)]
pub struct ElectronWindowBuilder {
  pub(crate) config: WindowConfig,
  pub(crate) icon: Option<IconData>,
  pub(crate) unsupported: Vec<&'static str>,
  /// Owning window handle (Windows: sets the owner, not the parent).
  #[cfg(windows)]
  pub(crate) owner: Option<HWND>,
  #[cfg(windows)]
  pub(crate) parent: Option<HWND>,
  /// Parent window (macOS keeps the NSWindow pointer until the window exists).
  #[cfg(target_os = "macos")]
  pub(crate) macos_parent: Option<*mut std::ffi::c_void>,
}

impl Default for ElectronWindowBuilder {
  fn default() -> Self {
    Self {
      config: WindowConfig::default(),
      icon: None,
      unsupported: Vec::new(),
      #[cfg(windows)]
      owner: None,
      #[cfg(windows)]
      parent: None,
    }
  }
}

impl ElectronWindowBuilder {
  fn unsupported(mut self, option: &'static str) -> Self {
    if !self.unsupported.contains(&option) {
      self.unsupported.push(option);
    }
    self
  }

  fn with(mut self, apply: impl FnOnce(&mut WindowConfig)) -> Self {
    apply(&mut self.config);
    self
  }
}

impl WindowBuilderBase for ElectronWindowBuilder {}

impl WindowBuilder for ElectronWindowBuilder {
  fn new() -> Self {
    Self::default()
  }

  fn with_config(config: &WindowConfig) -> Self {
    Self {
      config: config.clone(),
      ..Self::default()
    }
  }

  fn center(self) -> Self {
    self.with(|config| {
      config.center = true;
      config.x = None;
      config.y = None;
    })
  }

  fn position(self, x: f64, y: f64) -> Self {
    self.with(|config| {
      config.x = Some(x);
      config.y = Some(y);
      config.center = false;
    })
  }

  fn inner_size(self, width: f64, height: f64) -> Self {
    self.with(|config| {
      config.width = width;
      config.height = height;
    })
  }

  fn min_inner_size(self, min_width: f64, min_height: f64) -> Self {
    self.with(|config| {
      config.min_width = Some(min_width);
      config.min_height = Some(min_height);
    })
  }

  fn max_inner_size(self, max_width: f64, max_height: f64) -> Self {
    self.with(|config| {
      config.max_width = Some(max_width);
      config.max_height = Some(max_height);
    })
  }

  fn inner_size_constraints(self, constraints: WindowSizeConstraints) -> Self {
    self.with(|config| {
      if let Some(width) = constraints.min_width {
        config.min_width = Some(units(width));
      }
      if let Some(height) = constraints.min_height {
        config.min_height = Some(units(height));
      }
      if let Some(width) = constraints.max_width {
        config.max_width = Some(units(width));
      }
      if let Some(height) = constraints.max_height {
        config.max_height = Some(units(height));
      }
    })
  }

  fn prevent_overflow(self) -> Self {
    self.with(|config| config.prevent_overflow = Some(PreventOverflowConfig::Enable(true)))
  }

  fn prevent_overflow_with_margin(self, margin: Size) -> Self {
    self.with(|config| {
      let physical: tauri_runtime::dpi::PhysicalSize<f64> = margin.to_physical(1.0);
      config.prevent_overflow = Some(PreventOverflowConfig::Margin(PreventOverflowMargin {
        width: physical.width.round() as u32,
        height: physical.height.round() as u32,
      }))
    })
  }

  fn resizable(self, resizable: bool) -> Self {
    self.with(|config| config.resizable = resizable)
  }

  fn maximizable(self, maximizable: bool) -> Self {
    self.with(|config| config.maximizable = maximizable)
  }

  fn minimizable(self, minimizable: bool) -> Self {
    self.with(|config| config.minimizable = minimizable)
  }

  fn closable(self, closable: bool) -> Self {
    self.with(|config| config.closable = closable)
  }

  fn title<S: Into<String>>(self, title: S) -> Self {
    self.with(|config| config.title = title.into())
  }

  fn fullscreen(self, fullscreen: bool) -> Self {
    self.with(|config| config.fullscreen = fullscreen)
  }

  fn focused(self, focused: bool) -> Self {
    self.with(|config| config.focus = focused)
  }

  fn focusable(self, focusable: bool) -> Self {
    self.with(|config| config.focusable = focusable)
  }

  fn maximized(self, maximized: bool) -> Self {
    self.with(|config| config.maximized = maximized)
  }

  fn visible(self, visible: bool) -> Self {
    self.with(|config| config.visible = visible)
  }

  fn transparent(self, transparent: bool) -> Self {
    self.with(|config| config.transparent = transparent)
  }

  fn decorations(self, decorations: bool) -> Self {
    self.with(|config| config.decorations = decorations)
  }

  fn always_on_bottom(self, always_on_bottom: bool) -> Self {
    self.with(|config| config.always_on_bottom = always_on_bottom)
  }

  fn always_on_top(self, always_on_top: bool) -> Self {
    self.with(|config| config.always_on_top = always_on_top)
  }

  fn visible_on_all_workspaces(self, visible: bool) -> Self {
    self.with(|config| config.visible_on_all_workspaces = visible)
  }

  fn content_protected(self, protected: bool) -> Self {
    self.with(|config| config.content_protected = protected)
  }

  fn icon(self, icon: Icon<'_>) -> Result<Self> {
    Ok(Self {
      icon: Some(IconData {
        rgba: icon.rgba.into_owned(),
        width: icon.width,
        height: icon.height,
      }),
      ..self
    })
  }

  fn skip_taskbar(self, skip: bool) -> Self {
    self.with(|config| config.skip_taskbar = skip)
  }

  fn window_classname<S: Into<String>>(self, classname: S) -> Self {
    self.with(|config| config.window_classname = Some(classname.into()))
  }

  fn no_redirection_bitmap(self, enable: bool) -> Self {
    self.with(|config| config.no_redirection_bitmap = enable)
  }

  fn shadow(self, enable: bool) -> Self {
    self.with(|config| config.shadow = enable)
  }

  #[cfg(windows)]
  fn owner(mut self, owner: HWND) -> Self {
    self.owner = Some(owner);
    self
  }

  #[cfg(windows)]
  fn parent(mut self, parent: HWND) -> Self {
    self.parent = Some(parent);
    self
  }

  #[cfg(target_os = "macos")]
  fn parent(mut self, parent: *mut std::ffi::c_void) -> Self {
    // The pointer is one of our own windows: the runtime maps it back to a
    // serial and Electron then calls `setParentWindow`.
    self.macos_parent = Some(parent);
    self
  }

  #[cfg(any(
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd"
  ))]
  fn transient_for(self, _parent: *mut std::ffi::c_void) -> Self {
    // No Electron equivalent: GTK parent handles mean nothing to Electron.
    // GTK parent handles mean nothing to Electron.
    self.unsupported("transient_for")
  }

  #[cfg(windows)]
  fn drag_and_drop(self, enabled: bool) -> Self {
    self.with(|config| config.drag_drop_enabled = enabled)
  }

  #[cfg(target_os = "macos")]
  fn title_bar_style(self, style: TitleBarStyle) -> Self {
    self.with(|config| config.title_bar_style = style)
  }

  #[cfg(target_os = "macos")]
  fn traffic_light_position<P: Into<Position>>(self, position: P) -> Self {
    self.with(|config| {
      // `dpi::LogicalPosition` and the config's own type are unrelated structs.
      let logical = position.into().to_logical::<f64>(1.0);
      config.traffic_light_position = Some(tauri_utils::config::LogicalPosition {
        x: logical.x,
        y: logical.y,
      })
    })
  }

  #[cfg(target_os = "macos")]
  fn hidden_title(self, hidden: bool) -> Self {
    self.with(|config| config.hidden_title = hidden)
  }

  #[cfg(target_os = "macos")]
  fn tabbing_identifier(self, identifier: &str) -> Self {
    self.with(|config| config.tabbing_identifier = Some(identifier.to_string()))
  }

  fn theme(self, theme: Option<Theme>) -> Self {
    self.with(|config| config.theme = theme)
  }

  fn has_icon(&self) -> bool {
    self.icon.is_some()
  }

  fn get_theme(&self) -> Option<Theme> {
    self.config.theme
  }

  fn background_color(self, color: tauri_utils::config::Color) -> Self {
    self.with(|config| config.background_color = Some(color))
  }

  #[cfg(target_os = "android")]
  fn activity_name<S: Into<String>>(self, class_name: S) -> Self {
    self.with(|config| config.activity_name = Some(class_name.into()))
  }

  #[cfg(target_os = "android")]
  fn created_by_activity_name<S: Into<String>>(self, class_name: S) -> Self {
    self.with(|config| config.created_by_activity_name = Some(class_name.into()))
  }

  #[cfg(target_os = "ios")]
  fn requested_by_scene_identifier<S: Into<String>>(self, identifier: S) -> Self {
    self.with(|config| config.requested_by_scene_identifier = Some(identifier.into()))
  }
}

/// Collapses a constraint unit to the logical pixels Electron works in.
fn units(unit: PixelUnit) -> f64 {
  match unit {
    PixelUnit::Physical(value) => f64::from(value.0),
    PixelUnit::Logical(value) => value.0,
  }
}
