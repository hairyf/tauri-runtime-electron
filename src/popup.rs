// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! Window requests a page makes, and the window features it asks for.
//!
//! CEF's `popup.rs` drives `CefBrowserHost` popups and keeps a native browser
//! per popup. Electron routes the same request through
//! `webContents.setWindowOpenHandler`, which can only answer synchronously — so
//! the runtime denies in Electron and lets Tauri's `new_window_handler` decide,
//! creating the window through the normal runtime path. This module only turns
//! the feature string into the size and position Tauri expects.

use tauri_runtime::dpi::{LogicalPosition, LogicalSize};

/// Parses a Chromium window feature string (`width=800,height=600,left=10`).
///
/// Chromium reports features as `key=value` pairs separated by commas, and a
/// bare `key` means "true"; only the geometry keys are read here because that is
/// all [`NewWindowFeatures`](tauri_runtime::webview::NewWindowFeatures) carries.
pub(crate) fn window_features(
  features: &str,
) -> (Option<LogicalSize<f64>>, Option<LogicalPosition<f64>>) {
  let mut width = None;
  let mut height = None;
  let mut left = None;
  let mut top = None;
  for part in features.split(',') {
    let Some((key, value)) = part.split_once('=') else {
      continue;
    };
    let Ok(value) = value.trim().parse::<f64>() else {
      continue;
    };
    match key.trim() {
      "width" | "innerWidth" => width = Some(value),
      "height" | "innerHeight" => height = Some(value),
      "left" | "screenX" | "x" => left = Some(value),
      "top" | "screenY" | "y" => top = Some(value),
      _ => {}
    }
  }
  let size = match (width, height) {
    (Some(width), Some(height)) => Some(LogicalSize::new(width, height)),
    _ => None,
  };
  let position = match (left, top) {
    (Some(x), Some(y)) => Some(LogicalPosition::new(x, y)),
    _ => None,
  };
  (size, position)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn parses_geometry() {
    let (size, position) = window_features("width=800,height=600,left=10,top=20");
    assert_eq!(size, Some(LogicalSize::new(800.0, 600.0)));
    assert_eq!(position, Some(LogicalPosition::new(10.0, 20.0)));
  }

  #[test]
  fn ignores_incomplete_geometry() {
    let (size, position) = window_features("width=800,popup");
    assert_eq!(size, None);
    assert_eq!(position, None);
  }
}
