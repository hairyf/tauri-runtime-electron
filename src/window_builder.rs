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

#[derive(Debug, Clone)]
pub(crate) struct IconData {
  pub rgba: Vec<u8>,
  pub width: u32,
  pub height: u32,
}

/// Collects the window config and options that need the live Electron window.
#[derive(Debug, Clone, Default)]
pub struct ElectronWindowBuilder {
  pub(crate) config: WindowConfig,
  pub(crate) size_constraints: Option<WindowSizeConstraints>,
  pub(crate) icon: Option<IconData>,
  pub(crate) new_window_opener: Option<crate::new_window::NewWindowOpener>,
  #[cfg(windows)]
  pub(crate) owner: Option<isize>,
  #[cfg(windows)]
  pub(crate) parent: Option<isize>,
  #[cfg(target_os = "macos")]
  pub(crate) macos_parent: Option<isize>,
}

impl WindowBuilderBase for ElectronWindowBuilder {}

impl WindowBuilder for ElectronWindowBuilder {
  fn new() -> Self {
    Self { new_window_opener: crate::new_window::take_opener(), ..Self::default() }
  }

  fn with_config(config: &WindowConfig) -> Self {
    Self {
      config: config.clone(),
      new_window_opener: crate::new_window::take_opener(),
      ..Self::default()
    }
  }

  fn center(mut self) -> Self {
    self.config.center = true;
    self.config.x = None;
    self.config.y = None;
    self
  }

  fn position(mut self, x: f64, y: f64) -> Self {
    self.config.x = Some(x);
    self.config.y = Some(y);
    self.config.center = false;
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
    if let Some(constraints) = self.size_constraints.as_mut() {
      constraints.min_width = Some(PixelUnit::Logical(min_width.into()));
      constraints.min_height = Some(PixelUnit::Logical(min_height.into()));
    }
    self
  }

  fn max_inner_size(mut self, max_width: f64, max_height: f64) -> Self {
    self.config.max_width = Some(max_width);
    self.config.max_height = Some(max_height);
    if let Some(constraints) = self.size_constraints.as_mut() {
      constraints.max_width = Some(PixelUnit::Logical(max_width.into()));
      constraints.max_height = Some(PixelUnit::Logical(max_height.into()));
    }
    self
  }

  fn inner_size_constraints(mut self, constraints: WindowSizeConstraints) -> Self {
    // Physical units cannot become logical until Electron knows the display.
    // As with the baseline builder, omitted axes keep earlier builder limits.
    let previous = self.size_constraints.unwrap_or_else(|| WindowSizeConstraints {
      min_width: self.config.min_width.map(|value| PixelUnit::Logical(value.into())),
      min_height: self.config.min_height.map(|value| PixelUnit::Logical(value.into())),
      max_width: self.config.max_width.map(|value| PixelUnit::Logical(value.into())),
      max_height: self.config.max_height.map(|value| PixelUnit::Logical(value.into())),
    });
    self.size_constraints = Some(WindowSizeConstraints {
      min_width: constraints.min_width.or(previous.min_width),
      min_height: constraints.min_height.or(previous.min_height),
      max_width: constraints.max_width.or(previous.max_width),
      max_height: constraints.max_height.or(previous.max_height),
    });
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

  fn icon(mut self, icon: Icon<'_>) -> Result<Self> {
    let expected = (icon.width as usize).checked_mul(icon.height as usize)
      .and_then(|pixels| pixels.checked_mul(4));
    if icon.width == 0 || icon.height == 0 || expected != Some(icon.rgba.len()) {
      return Err(tauri_runtime::Error::InvalidIcon(std::io::Error::new(
        std::io::ErrorKind::InvalidInput, "window icon dimensions do not match its RGBA pixels",
      ).into()));
    }
    self.icon = Some(IconData {
      rgba: icon.rgba.into_owned(),
      width: icon.width,
      height: icon.height,
    });
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
  fn owner(mut self, owner: HWND) -> Self {
    self.owner = Some(owner.0 as isize);
    self
  }

  #[cfg(windows)]
  fn parent(mut self, parent: HWND) -> Self {
    self.parent = Some(parent.0 as isize);
    self
  }

  #[cfg(target_os = "macos")]
  fn parent(mut self, parent: *mut std::ffi::c_void) -> Self {
    self.macos_parent = Some(parent as isize);
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
    let logical = position.into().to_logical::<f64>(1.0);
    self.config.traffic_light_position = Some(tauri_utils::config::LogicalPosition {
      x: logical.x,
      y: logical.y,
    });
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
    self.icon.is_some()
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
  fn icon_rejects_mismatched_dimensions() {
    let invalid = Icon { rgba: std::borrow::Cow::Borrowed(&[1, 2, 3]), width: 1, height: 1 };
    assert!(ElectronWindowBuilder::new().icon(invalid).is_err());
  }

  #[test]
  fn center_and_position_follow_the_last_setter() {
    let centered = ElectronWindowBuilder::new().position(40.0, 60.0).center();
    assert!(centered.config.center);
    assert_eq!((centered.config.x, centered.config.y), (None, None));
    let positioned = centered.position(-120.0, 90.0);
    assert!(!positioned.config.center);
    assert_eq!((positioned.config.x, positioned.config.y), (Some(-120.0), Some(90.0)));
  }

  #[test]
  fn mixed_constraints_keep_units_and_later_size_setters() {
    let constraints = WindowSizeConstraints {
      min_width: Some(PixelUnit::Physical(300.into())),
      max_height: Some(PixelUnit::Logical(600.0.into())),
      ..Default::default()
    };
    let builder = ElectronWindowBuilder::new().inner_size_constraints(constraints)
      .max_inner_size(900.0, 700.0);
    let actual = builder.size_constraints.unwrap();
    assert!(matches!(actual.min_width, Some(PixelUnit::Physical(value)) if value.0 == 300));
    assert!(matches!(actual.max_height, Some(PixelUnit::Logical(value)) if value.0 == 700.0));
  }

  #[test]
  fn with_config_keeps_the_hosts_values() {
    let config = WindowConfig {
      width: 640.0,
      height: 480.0,
      title_bar_style: tauri_utils::TitleBarStyle::Overlay,
      traffic_light_position: Some(tauri_utils::config::LogicalPosition { x: 12.0, y: 18.0 }),
      hidden_title: true,
      tabbing_identifier: Some("documents".into()),
      ..Default::default()
    };
    let builder = <ElectronWindowBuilder as WindowBuilder>::with_config(&config);
    assert_eq!(builder.config.width, 640.0);
    assert_eq!(builder.config.height, 480.0);
    assert!(matches!(builder.config.title_bar_style, tauri_utils::TitleBarStyle::Overlay));
    let position = builder.config.traffic_light_position.unwrap();
    assert_eq!((position.x, position.y), (12.0, 18.0));
    assert!(builder.config.hidden_title);
    assert_eq!(builder.config.tabbing_identifier.as_deref(), Some("documents"));
  }

  #[cfg(target_os = "macos")]
  #[test]
  fn macos_title_setters_land_in_the_config() {
    let builder = <ElectronWindowBuilder as WindowBuilder>::new()
      .title_bar_style(TitleBarStyle::Overlay)
      .traffic_light_position(tauri_runtime::dpi::LogicalPosition::new(12.0, 18.0))
      .hidden_title(true)
      .tabbing_identifier("documents");
    assert!(matches!(builder.config.title_bar_style, TitleBarStyle::Overlay));
    let position = builder.config.traffic_light_position.unwrap();
    assert_eq!((position.x, position.y), (12.0, 18.0));
    assert!(builder.config.hidden_title);
    assert_eq!(builder.config.tabbing_identifier.as_deref(), Some("documents"));
  }
}
