// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! Platform helpers and conversion of Electron's physical display geometry.

use electron_rs::rpc::Display;
use tauri_runtime::{dpi::{PhysicalPosition, PhysicalRect, PhysicalSize}, monitor::Monitor};

pub(crate) mod cursor;
pub(crate) mod dpi;
#[cfg(target_os = "linux")]
pub(crate) mod linux_window;

pub(crate) fn monitor_from_display(display: &Display) -> Monitor {
  Monitor {
    name: Some(display.name.clone()),
    size: PhysicalSize::new(display.size.width, display.size.height),
    position: PhysicalPosition::new(display.position.x, display.position.y),
    work_area: PhysicalRect {
      position: PhysicalPosition::new(display.work_area.position.x, display.work_area.position.y),
      size: PhysicalSize::new(display.work_area.size.width, display.work_area.size.height),
    },
    scale_factor: display.scale_factor,
  }
}
