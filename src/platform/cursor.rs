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

/// Moves the cursor to a screen position in physical pixels. The window
/// dispatcher translates client offsets first. `scale` converts macOS points.
/// Native failures and unsupported platforms are returned to the caller.
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

#[cfg(target_os = "linux")]
mod imp {
  use std::{
    ffi::{c_char, c_int, c_uint, c_ulong, c_void},
    sync::Mutex,
  };
  use tauri_runtime::dpi::PhysicalPosition;

  #[link(name = "X11")]
  extern "C" {
    fn XOpenDisplay(name: *const c_char) -> *mut c_void;
    fn XDefaultRootWindow(display: *mut c_void) -> c_ulong;
    fn XWarpPointer(
      display: *mut c_void,
      src_window: c_ulong,
      dest_window: c_ulong,
      src_x: c_int,
      src_y: c_int,
      src_width: c_uint,
      src_height: c_uint,
      dest_x: c_int,
      dest_y: c_int,
    ) -> c_int;
    fn XSync(display: *mut c_void, discard: c_int) -> c_int;
    fn XCloseDisplay(display: *mut c_void) -> c_int;
  }

  // Xlib needs serialization unless XInitThreads ran before any Xlib calls.
  // ponytail: serialize short-lived connections; reuse one if warp throughput matters.
  static XLIB: Mutex<()> = Mutex::new(());

  pub(super) fn warp(position: PhysicalPosition<i32>, _scale: f64) -> Result<(), String> {
    if std::env::var_os("WAYLAND_DISPLAY").is_some()
      || std::env::var("XDG_SESSION_TYPE")
        .is_ok_and(|session| session.eq_ignore_ascii_case("wayland"))
    {
      return Err("global cursor warping is not supported on Wayland".into());
    }
    // XWarpPointer takes C ints, but the X11 protocol encodes coordinates as INT16.
    if i16::try_from(position.x).is_err() || i16::try_from(position.y).is_err() {
      return Err("X11 cursor position exceeds signed 16-bit screen coordinates".into());
    }
    let _guard = XLIB
      .lock()
      .map_err(|_| "X11 cursor lock poisoned".to_string())?;
    // SAFETY: null selects DISPLAY. This call owns the returned connection and
    // never shares it with another thread or a toolkit.
    let display = unsafe { XOpenDisplay(std::ptr::null()) };
    if display.is_null() {
      return Err("XOpenDisplay could not open DISPLAY; an X11 display is required".into());
    }
    // SAFETY: display is live and exclusively owned, and the root belongs to it.
    // A zero source window is X11 None, so the destination is an absolute root
    // position. No fallible Rust operations intervene before closing display.
    unsafe {
      let root = XDefaultRootWindow(display);
      XWarpPointer(display, 0, root, 0, 0, 0, 0, position.x, position.y);
      // XWarpPointer's return value is not a server status; wait for processing.
      XSync(display, 0);
      XCloseDisplay(display);
    }
    Ok(())
  }

  #[test]
  #[ignore = "run with DISPLAY, WAYLAND_DISPLAY and XDG_SESSION_TYPE unset"]
  fn warp_without_display_returns_error() {
    assert!(std::env::var_os("DISPLAY").is_none());
    assert!(warp(PhysicalPosition::new(0, 0), 1.0)
      .unwrap_err()
      .contains("XOpenDisplay"));
  }
}

#[cfg(all(
  unix,
  not(target_os = "linux"),
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
