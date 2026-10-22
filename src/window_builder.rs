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

  fn position(self, _x: f64, _y: f64) -> Self {
    self
  }

  fn inner_size(self, _min_width: f64, _min_height: f64) -> Self {
    self
  }

  fn min_inner_size(self, _min_width: f64, _min_height: f64) -> Self {
    self
  }

  fn max_inner_size(self, _max_width: f64, _max_height: f64) -> Self {
    self
  }

  fn inner_size_constraints(
    self,
    _constraints: tauri_runtime::window::WindowSizeConstraints,
  ) -> Self {
    self
  }

  fn prevent_overflow(self) -> Self {
    self
  }

  fn prevent_overflow_with_margin(self, _margin: tauri_runtime::dpi::Size) -> Self {
    self
  }

  fn resizable(self, _resizable: bool) -> Self {
    self
  }

  fn maximizable(self, _resizable: bool) -> Self {
    self
  }

  fn minimizable(self, _resizable: bool) -> Self {
    self
  }

  fn closable(self, _resizable: bool) -> Self {
    self
  }

  fn title<S: Into<String>>(self, _title: S) -> Self {
    self
  }

  fn fullscreen(self, _fullscreen: bool) -> Self {
    self
  }

  fn focused(self, _focused: bool) -> Self {
    self
  }

  fn focusable(self, _focusable: bool) -> Self {
    self
  }

  fn maximized(self, _maximized: bool) -> Self {
    self
  }

  fn visible(self, _visible: bool) -> Self {
    self
  }

  fn transparent(self, _transparent: bool) -> Self {
    self
  }

  fn decorations(self, _decorations: bool) -> Self {
    self
  }

  fn always_on_bottom(self, _always_on_bottom: bool) -> Self {
    self
  }

  fn always_on_top(self, _always_on_top: bool) -> Self {
    self
  }

  fn visible_on_all_workspaces(self, _visible_on_all_workspaces: bool) -> Self {
    self
  }

  fn content_protected(self, _protected: bool) -> Self {
    self
  }

  fn icon(self, _icon: Icon<'_>) -> Result<Self> {
    Ok(self)
  }

  fn skip_taskbar(self, _skip: bool) -> Self {
    self
  }

  fn window_classname<S: Into<String>>(self, _classname: S) -> Self {
    self
  }

  fn no_redirection_bitmap(self, _enable: bool) -> Self {
    self
  }

  fn shadow(self, _enable: bool) -> Self {
    self
  }

  #[cfg(windows)]
  fn owner(self, _owner: HWND) -> Self {
    self
  }

  #[cfg(windows)]
  fn parent(self, _parent: HWND) -> Self {
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
  fn transient_for(self, _parent: &impl gtk::glib::IsA<gtk::Window>) -> Self {
    self
  }

  #[cfg(windows)]
  fn drag_and_drop(self, _enabled: bool) -> Self {
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

  fn theme(self, _theme: Option<Theme>) -> Self {
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
