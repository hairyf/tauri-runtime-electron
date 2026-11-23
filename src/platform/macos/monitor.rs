// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! Work areas, as Electron reports them.
//!
//! The reference reads `NSScreen.visibleFrame` and re-derives the work area from the
//! screen frame because winit's `MonitorHandle` only knows the full monitor rect. Electron's
//! `Display` already carries `workArea` — the same rectangle, with the Dock and the menu bar
//! subtracted — so the AppKit shim has nothing left to add.

use tauri_runtime::{dpi::PhysicalRect, monitor::Monitor};

use crate::platform::MonitorExt;

impl MonitorExt for Monitor {
  fn work_area(&self) -> PhysicalRect<i32, u32> {
    self.work_area
  }
}
