// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! Work areas, as Electron reports them.
//!
//! The reference parses `_NET_WORKAREA` off the root window and intersects it with the
//! monitor rect, because winit's `MonitorHandle` only knows the full monitor rect. Electron's
//! `Display` already carries `workArea`, so the EWMH parsing has nothing left to add — but
//! the intersection helpers stay, because they are the part that has no X11 dependency and
//! they are what a caller with a work area from another source would need.

use tauri_runtime::{
  dpi::{PhysicalPosition, PhysicalRect, PhysicalSize},
  monitor::Monitor,
};

use crate::platform::MonitorExt;

impl MonitorExt for Monitor {
  fn work_area(&self) -> PhysicalRect<i32, u32> {
    self.work_area
  }
}

/// The work area the X server would report for `monitor_bounds`.
///
/// `_NET_WORKAREA` is a root-window property describing the whole desktop, so the reference
/// reads it and intersects it with the monitor. This crate opens no X connection, and
/// Electron's `Display.workArea` is the same rectangle already resolved per monitor, so there
/// is no X11 answer to give here — the caller falls back to the monitor rect, exactly as the
/// reference does when the property is missing.
pub(crate) fn x11_work_area(
  _monitor_bounds: PhysicalRect<i32, u32>,
) -> Option<PhysicalRect<i32, u32>> {
  crate::unsupported::report("platform::linux::x11_work_area (Electron: Display.workArea)");
  None
}

fn intersect_rects(
  a: PhysicalRect<i32, u32>,
  b: PhysicalRect<i32, u32>,
) -> Option<PhysicalRect<i32, u32>> {
  let left = a.position.x.max(b.position.x);
  let top = a.position.y.max(b.position.y);
  let right = rect_right(a).min(rect_right(b));
  let bottom = rect_bottom(a).min(rect_bottom(b));

  if right <= left || bottom <= top {
    return None;
  }

  Some(PhysicalRect {
    position: PhysicalPosition::new(left, top),
    size: PhysicalSize::new((right - left) as u32, (bottom - top) as u32),
  })
}

fn rect_right(rect: PhysicalRect<i32, u32>) -> i32 {
  rect
    .position
    .x
    .saturating_add(i32::try_from(rect.size.width).unwrap_or(i32::MAX))
}

fn rect_bottom(rect: PhysicalRect<i32, u32>) -> i32 {
  rect
    .position
    .y
    .saturating_add(i32::try_from(rect.size.height).unwrap_or(i32::MAX))
}
