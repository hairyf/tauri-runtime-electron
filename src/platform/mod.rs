// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! Platform helpers.
//!
//! CEF's `platform` module exists because Tauri reports monitors through winit
//! while the CEF runtime gets them from Chromium, and the two disagree about
//! work areas on every OS — hence the native `MonitorExt` shims.
//!
//! Electron hands the runtime display geometry, including the work area, so the
//! shims have nothing to answer here: [`monitor_from_value`] is the only piece
//! left, and it is shared by the runtime and the window dispatchers.

use serde_json::Value;
use tauri_runtime::{dpi::PhysicalRect, monitor::Monitor};

use crate::runtime::{rect_position, rect_size};

pub(crate) mod cursor;
pub(crate) mod dpi;

/// Maps the monitor JSON shape produced by `electron/main.cjs` onto Tauri's
/// physical-pixel `Monitor`.
pub(crate) fn monitor_from_value(value: &Value) -> Option<Monitor> {
  if !value.is_object() {
    return None;
  }
  let work_area = &value["workArea"];
  Some(Monitor {
    name: value["name"].as_str().map(str::to_string),
    size: rect_size(&value["size"]),
    position: rect_position(&value["position"]),
    work_area: PhysicalRect {
      position: rect_position(&work_area["position"]),
      size: rect_size(&work_area["size"]),
    },
    scale_factor: value["scaleFactor"].as_f64().unwrap_or(1.0),
  })
}
