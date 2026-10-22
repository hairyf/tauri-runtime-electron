// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

use tauri_runtime::{
  Icon, Result,
  window::{WindowBuilder, WindowBuilderBase},
};
use tauri_utils::{Theme, config::WindowConfig};

#[cfg(target_os = "macos")]
use tauri_runtime::dpi::Position;

#[cfg(target_os = "macos")]
use tauri_utils::TitleBarStyle;

#[cfg(windows)]
use windows::Win32::Foundation::HWND;

#[derive(Debug, Clone, Default)]
pub struct ElectronWindowBuilder {
  /// Window configuration captured by `with_config`, used when creating the
  /// actual Electron `BrowserWindow`.
  pub(crate) config: Option<WindowConfig>,
}

impl WindowBuilderBase for ElectronWindowBuilder {}

impl WindowBuilder for ElectronWindowBuilder {
  fn new() -> Self {
    Self::default()
  }

  fn with_config(config: &WindowConfig) -> Self {
    Self {
      config: Some(config.clone()),
    }
  }

  fn center(self) -> Self {
    self
  }

  fn position(self, x: f64, y: f64) -> Self {
    self
  }

  fn inner_size(self, min_width: f64, min_height: f64) -> Self {
    self
  }

  fn min_inner_size(self, min_width: f64, min_height: f64) -> Self {
    self
  }

  fn max_inner_size(self, max_width: f64, max_height: f64) -> Self {
    self
  }

  fn inner_size_constraints(
    self,
    constraints: tauri_runtime::window::WindowSizeConstraints,
  ) -> Self {
    self
  }

  fn prevent_overflow(self) -> Self {
    self
  }

  fn prevent_overflow_with_margin(self, margin: tauri_runtime::dpi::Size) -> Self {
    self
  }

  fn resizable(self, resizable: bool) -> Self {
    self
  }

  fn maximizable(self, resizable: bool) -> Self {
    self
  }

  fn minimizable(self, resizable: bool) -> Self {
    self
  }

  fn closable(self, resizable: bool) -> Self {
    self
  }

  fn title<S: Into<String>>(self, title: S) -> Self {
    self
  }

  fn fullscreen(self, fullscreen: bool) -> Self {
    self
  }

  fn focused(self, focused: bool) -> Self {
    self
  }

  fn focusable(self, focusable: bool) -> Self {
    self
  }

  fn maximized(self, maximized: bool) -> Self {
    self
  }

  fn visible(self, visible: bool) -> Self {
    self
  }

  fn transparent(self, transparent: bool) -> Self {
    self
  }

  fn decorations(mut self, decorations: bool) -> Self {
    self
      .config
      .get_or_insert_with(WindowConfig::default)
      .decorations = decorations;
    self
  }

  fn always_on_bottom(self, always_on_bottom: bool) -> Self {
    self
  }

  fn always_on_top(self, always_on_top: bool) -> Self {
    self
  }

  fn visible_on_all_workspaces(self, visible_on_all_workspaces: bool) -> Self {
    self
  }

  fn content_protected(self, protected: bool) -> Self {
    self
  }

  fn icon(self, icon: Icon<'_>) -> Result<Self> {
    Ok(self)
  }

  fn skip_taskbar(self, skip: bool) -> Self {
    self
  }

  fn window_classname<S: Into<String>>(self, classname: S) -> Self {
    self
  }

  fn no_redirection_bitmap(self, enable: bool) -> Self {
    self
  }

  fn shadow(self, enable: bool) -> Self {
    self
  }

  #[cfg(windows)]
  fn owner(self, owner: HWND) -> Self {
    self
  }

  #[cfg(windows)]
  fn parent(self, parent: HWND) -> Self {
    self
  }

  #[cfg(target_os = "macos")]
  fn parent(self, parent: *mut std::ffi::c_void) -> Self {
    self
  }

  #[cfg(any(
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd"
  ))]
  fn transient_for(self, parent: *mut std::ffi::c_void) -> Self {
    self
  }

  #[cfg(windows)]
  fn drag_and_drop(self, enabled: bool) -> Self {
    self
  }

  #[cfg(target_os = "macos")]
  fn title_bar_style(self, style: TitleBarStyle) -> Self {
    self
  }

  #[cfg(target_os = "macos")]
  fn traffic_light_position<P: Into<Position>>(self, position: P) -> Self {
    self
  }

  #[cfg(target_os = "macos")]
  fn hidden_title(self, transparent: bool) -> Self {
    self
  }

  #[cfg(target_os = "macos")]
  fn tabbing_identifier(self, identifier: &str) -> Self {
    self
  }

  fn theme(self, theme: Option<Theme>) -> Self {
    self
  }

  fn has_icon(&self) -> bool {
    false
  }

  fn get_theme(&self) -> Option<Theme> {
    None
  }

  fn background_color(self, _color: tauri_utils::config::Color) -> Self {
    self
  }

  #[cfg(target_os = "android")]
  fn activity_name<S: Into<String>>(self, _class_name: S) -> Self {
    self
  }

  #[cfg(target_os = "android")]
  fn created_by_activity_name<S: Into<String>>(self, _class_name: S) -> Self {
    self
  }

  #[cfg(target_os = "ios")]
  fn requested_by_scene_identifier<S: Into<String>>(self, _identifier: S) -> Self {
    self
  }
}
