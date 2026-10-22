// Copyright 2019-2024 Tauri Programme Within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

use tauri_runtime::{
  Icon, Result,
  dpi::PixelUnit,
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

/// Collects the window a host app asked for.
///
/// Every setter writes into a [`WindowConfig`] because that is the only thing
/// Electron reads when it creates the `BrowserWindow`: a setter that forgets to
/// store its value is a window that silently comes up wrong, which is exactly
/// what happened to `.inner_size()` before this was a plain config.
#[derive(Debug, Clone, Default)]
pub struct ElectronWindowBuilder {
  pub(crate) config: WindowConfig,
}

impl WindowBuilderBase for ElectronWindowBuilder {}

impl WindowBuilder for ElectronWindowBuilder {
  fn new() -> Self {
    Self::default()
  }

  fn with_config(config: &WindowConfig) -> Self {
    Self {
      config: config.clone(),
    }
  }

  fn center(mut self) -> Self {
    self.config.center = true;
    self
  }

  fn position(mut self, x: f64, y: f64) -> Self {
    self.config.x = Some(x);
    self.config.y = Some(y);
    self
  }

  fn inner_size(mut self, width: f64, height: f64) -> Self {
    self.config.width = width;
    self.config.height = height;
    self
  }

  fn min_inner_size(mut self, min_width: f64, min_height: f64) -> Self {
    self.config.min_width = Some(min_width);
    self.config.min_height = Some(min_height);
    self
  }

  fn max_inner_size(mut self, max_width: f64, max_height: f64) -> Self {
    self.config.max_width = Some(max_width);
    self.config.max_height = Some(max_height);
    self
  }

  fn inner_size_constraints(mut self, constraints: WindowSizeConstraints) -> Self {
    let WindowSizeConstraints {
      min_width,
      min_height,
      max_width,
      max_height,
    } = constraints;
    // The config stores logical pixels, so a physical constraint is taken at
    // scale 1: this builder runs before any window (and its scale) exists.
    let to_logical = |unit: PixelUnit| unit.to_logical::<f64>(1.0).into();
    self.config.min_width = min_width.map(to_logical).or(self.config.min_width);
    self.config.min_height = min_height.map(to_logical).or(self.config.min_height);
    self.config.max_width = max_width.map(to_logical).or(self.config.max_width);
    self.config.max_height = max_height.map(to_logical).or(self.config.max_height);
    self
  }

  fn prevent_overflow(mut self) -> Self {
    self.config.prevent_overflow = Some(PreventOverflowConfig::Enable(true));
    self
  }

  fn prevent_overflow_with_margin(mut self, margin: tauri_runtime::dpi::Size) -> Self {
    let margin = margin.to_physical::<u32>(1.0);
    self.config.prevent_overflow = Some(PreventOverflowConfig::Margin(PreventOverflowMargin {
      width: margin.width,
      height: margin.height,
    }));
    self
  }

  fn resizable(mut self, resizable: bool) -> Self {
    self.config.resizable = resizable;
    self
  }

  fn maximizable(mut self, maximizable: bool) -> Self {
    self.config.maximizable = maximizable;
    self
  }

  fn minimizable(mut self, minimizable: bool) -> Self {
    self.config.minimizable = minimizable;
    self
  }

  fn closable(mut self, closable: bool) -> Self {
    self.config.closable = closable;
    self
  }

  fn title<S: Into<String>>(mut self, title: S) -> Self {
    self.config.title = title.into();
    self
  }

  fn fullscreen(mut self, fullscreen: bool) -> Self {
    self.config.fullscreen = fullscreen;
    self
  }

  fn focused(mut self, focused: bool) -> Self {
    self.config.focus = focused;
    self
  }

  fn focusable(mut self, focusable: bool) -> Self {
    self.config.focusable = focusable;
    self
  }

  fn maximized(mut self, maximized: bool) -> Self {
    self.config.maximized = maximized;
    self
  }

  fn visible(mut self, visible: bool) -> Self {
    self.config.visible = visible;
    self
  }

  fn transparent(mut self, transparent: bool) -> Self {
    self.config.transparent = transparent;
    self
  }

  fn decorations(mut self, decorations: bool) -> Self {
    self.config.decorations = decorations;
    self
  }

  fn always_on_bottom(mut self, always_on_bottom: bool) -> Self {
    self.config.always_on_bottom = always_on_bottom;
    self
  }

  fn always_on_top(mut self, always_on_top: bool) -> Self {
    self.config.always_on_top = always_on_top;
    self
  }

  fn visible_on_all_workspaces(mut self, visible_on_all_workspaces: bool) -> Self {
    self.config.visible_on_all_workspaces = visible_on_all_workspaces;
    self
  }

  fn content_protected(mut self, protected: bool) -> Self {
    self.config.content_protected = protected;
    self
  }

  fn icon(self, _icon: Icon<'_>) -> Result<Self> {
    // Electron reads one window icon from the runtime, which already arrives
    // through `RuntimeInitArgs`; a per-window icon has nowhere to go.
    crate::runtime::unsupported(
      "WindowBuilder::icon",
      "Electron takes the window icon from the runtime, not from the window",
    );
    Ok(self)
  }

  fn skip_taskbar(mut self, skip: bool) -> Self {
    self.config.skip_taskbar = skip;
    self
  }

  fn window_classname<S: Into<String>>(mut self, classname: S) -> Self {
    self.config.window_classname = Some(classname.into());
    self
  }

  fn no_redirection_bitmap(mut self, enable: bool) -> Self {
    self.config.no_redirection_bitmap = enable;
    self
  }

  fn shadow(mut self, enable: bool) -> Self {
    self.config.shadow = enable;
    self
  }

  #[cfg(windows)]
  fn owner(self, _owner: HWND) -> Self {
    crate::runtime::unsupported(
      "WindowBuilder::owner",
      "Electron owns its own window, so it cannot be owned by a native handle",
    );
    self
  }

  #[cfg(windows)]
  fn parent(self, _parent: HWND) -> Self {
    crate::runtime::unsupported(
      "WindowBuilder::parent",
      "Electron parents windows to windows, not to native handles",
    );
    self
  }

  #[cfg(target_os = "macos")]
  fn parent(self, _parent: *mut std::ffi::c_void) -> Self {
    crate::runtime::unsupported(
      "WindowBuilder::parent",
      "Electron parents windows to windows, not to native handles",
    );
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
    crate::runtime::unsupported(
      "WindowBuilder::transient_for",
      "an Electron window has no GTK window to be transient for",
    );
    self
  }

  #[cfg(windows)]
  fn drag_and_drop(mut self, enabled: bool) -> Self {
    self.config.drag_drop_enabled = enabled;
    self
  }

  #[cfg(target_os = "macos")]
  fn title_bar_style(mut self, style: TitleBarStyle) -> Self {
    self.config.title_bar_style = style;
    self
  }

  #[cfg(target_os = "macos")]
  fn traffic_light_position<P: Into<Position>>(mut self, position: P) -> Self {
    self.config.traffic_light_position = Some(position.into().to_logical(1.0));
    self
  }

  #[cfg(target_os = "macos")]
  fn hidden_title(mut self, hidden: bool) -> Self {
    self.config.hidden_title = hidden;
    self
  }

  #[cfg(target_os = "macos")]
  fn tabbing_identifier(mut self, identifier: &str) -> Self {
    self.config.tabbing_identifier = Some(identifier.to_string());
    self
  }

  fn theme(mut self, theme: Option<Theme>) -> Self {
    self.config.theme = theme;
    self
  }

  fn has_icon(&self) -> bool {
    false
  }

  fn get_theme(&self) -> Option<Theme> {
    self.config.theme
  }

  fn background_color(mut self, color: tauri_utils::config::Color) -> Self {
    self.config.background_color = Some(color);
    self
  }

  #[cfg(target_os = "android")]
  fn activity_name<S: Into<String>>(mut self, class_name: S) -> Self {
    self.config.activity_name = Some(class_name.into());
    self
  }

  #[cfg(target_os = "android")]
  fn created_by_activity_name<S: Into<String>>(mut self, class_name: S) -> Self {
    self.config.created_by_activity_name = Some(class_name.into());
    self
  }

  #[cfg(target_os = "ios")]
  fn requested_by_scene_identifier<S: Into<String>>(mut self, identifier: S) -> Self {
    self.config.requested_by_scene_identifier = Some(identifier.into());
    self
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn every_setter_lands_in_the_config() {
    let builder = <ElectronWindowBuilder as WindowBuilder>::new()
      .inner_size(1000.0, 800.0)
      .min_inner_size(200.0, 150.0)
      .title("hello")
      .resizable(false)
      .always_on_top(true);
    assert_eq!(builder.config.width, 1000.0);
    assert_eq!(builder.config.height, 800.0);
    assert_eq!(builder.config.min_width, Some(200.0));
    assert_eq!(builder.config.title, "hello");
    assert!(!builder.config.resizable);
    assert!(builder.config.always_on_top);
  }

  #[test]
  fn with_config_keeps_the_hosts_values() {
    let config = WindowConfig {
      width: 640.0,
      height: 480.0,
      ..Default::default()
    };
    let builder = <ElectronWindowBuilder as WindowBuilder>::with_config(&config);
    assert_eq!(builder.config.width, 640.0);
    assert_eq!(builder.config.height, 480.0);
  }
}
