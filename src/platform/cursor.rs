// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! Moving the OS cursor.
//!
//! Electron exposes `screen.getCursorScreenPoint()` but no way to set it, so
//! [`WindowDispatch::set_cursor_position`] falls through to the platform. This is
//! the one capability where the renderer cannot do the work: the cursor has to
//! move even while the pointer is outside the page.

use tauri_runtime::dpi::PhysicalPosition;

/// Moves the cursor to `position`, given in physical pixels like the rest of
/// Tauri. `scale` is the physical-pixels-per-point ratio (macOS warps in points).
///
/// A failure is reported as a message instead of an error so the caller can log
/// it once through [`crate::runtime::unsupported`] and stay silent afterwards.
pub(crate) fn warp(position: PhysicalPosition<i32>, scale: f64) -> Result<(), String> {
  imp::warp(position, scale)
}

#[cfg(windows)]
mod imp {
  use tauri_runtime::dpi::PhysicalPosition;
  use windows::Win32::UI::WindowsAndMessaging::SetCursorPos;

  pub(super) fn warp(position: PhysicalPosition<i32>, _scale: f64) -> Result<(), String> {
    // SAFETY: SetCursorPos takes screen coordinates and has no aliasing rules.
    unsafe { SetCursorPos(position.x, position.y) }
      .map_err(|error| format!("SetCursorPos({}, {}): {error}", position.x, position.y))
  }
}

#[cfg(target_os = "macos")]
mod imp {
  use tauri_runtime::dpi::PhysicalPosition;

  #[repr(C)]
  struct CGPoint {
    x: f64,
    y: f64,
  }

  #[link(name = "CoreGraphics", kind = "framework")]
  extern "C" {
    fn CGWarpMouseCursorPosition(new_cursor_position: CGPoint) -> i32;
    fn CGAssociateMouseAndMouseCursorPosition(connected: bool) -> i32;
  }

  pub(super) fn warp(position: PhysicalPosition<i32>, scale: f64) -> Result<(), String> {
    let scale = if scale > 0.0 { scale } else { 1.0 };
    let point = CGPoint {
      x: f64::from(position.x) / scale,
      y: f64::from(position.y) / scale,
    };
    // SAFETY: both functions only touch the window server cursor state.
    let status = unsafe { CGWarpMouseCursorPosition(point) };
    if status != 0 {
      return Err(format!("CGWarpMouseCursorPosition failed with {status}"));
    }
    // Warping suppresses the cursor until it is re-associated; without this the
    // pointer freezes after the first warp.
    unsafe { CGAssociateMouseAndMouseCursorPosition(true) };
    Ok(())
  }
}

#[cfg(all(
  unix,
  not(target_os = "macos"),
  not(target_os = "ios"),
  not(target_os = "android")
))]
mod imp {
  use tauri_runtime::dpi::PhysicalPosition;

  pub(super) fn warp(_position: PhysicalPosition<i32>, _scale: f64) -> Result<(), String> {
    // GDK's display is initialized only by a GTK main loop, and this runtime
    // never runs one: gdk::Display::default() panics with "GDK has not been
    // initialized" instead of returning None. Opening a second X connection
    // behind GTK's back would fight the toolkit for the pointer, and Wayland
    // has no warp request at all, so this stays a logged gap.
    Err("an Electron window has no GTK display to warp the cursor on".to_string())
  }
}
