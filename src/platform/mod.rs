// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! The native stand-ins for the windowing types the reference borrows from winit.
//!
//! The reference owns its event loop and its windows: winit hands it an
//! `ActiveEventLoop` to create them from, a `Window` to drive them with, and a
//! `WindowEvent` to report what the OS did to them. None of that exists here.
//! Electron owns the loop and the windows inside its own process, and this crate
//! only ever holds the raw handle Electron reports from `window.nativeHandle`.
//!
//! So the three winit types the runtime's public surface names are replaced by
//! the three types below — [`NativeWindow`], [`NativeEventLoop`] and
//! [`NativeWindowEvent`] — and the per-OS modules keep the native mechanisms the
//! reference reaches for directly (work areas, taskbar progress, dock tiles,
//! fullscreen bookkeeping) so the module tree and item names stay the reference's.
//!
//! The runtime reaches most of those mechanisms through the Electron RPCs
//! instead (`window.*`, `webview.*`, `app.*`), so several items here have no
//! caller inside this crate yet; they exist for parity and report through
//! [`crate::unsupported::report`] when the mechanism is one Electron does not
//! expose to the embedder.
#![allow(dead_code)]

#[cfg(windows)]
mod windows;

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(any(
  target_os = "linux",
  target_os = "dragonfly",
  target_os = "freebsd",
  target_os = "netbsd",
  target_os = "openbsd"
))]
pub mod linux;

use std::{ffi::c_void, fmt};

use raw_window_handle::RawWindowHandle;
use serde_json::Value;
use tauri_runtime::{
  dpi::{PhysicalPosition, PhysicalRect, PhysicalSize},
  monitor::Monitor,
};
use tauri_utils::Theme;

/// An Electron-owned native window.
///
/// The reference hands its native-event observers `Arc<dyn winit::window::Window>`:
/// a live object that can be asked for its size, its monitor and its raw handle.
/// This runtime never creates a window, so the observer gets the one thing both
/// processes actually share — the handle Electron reports for the window.
///
/// It is deliberately opaque. The pointer is only meaningful to the platform
/// module that knows how to read it, and [`Debug`] prints the platform and the
/// address rather than anything the OS would have to be asked for.
#[derive(Clone)]
pub struct NativeWindow {
  platform: &'static str,
  handle: u64,
}

impl NativeWindow {
  /// Wraps the handle Electron reports for `window.nativeHandle`.
  pub fn new(handle: u64) -> Self {
    Self {
      platform: std::env::consts::OS,
      handle,
    }
  }

  /// Wraps a `raw-window-handle` window handle.
  ///
  /// Returns `None` when the handle is not one of the window systems this crate
  /// can name, which is the same set `raw-window-handle` gives the reference.
  // `c_ulong` is `u64` on a 64-bit Linux and `u32` on Windows and a 32-bit Linux,
  // so one of the two integer casts below is always a no-op — clippy only ever
  // sees one width, so it cannot know that.
  #[allow(clippy::unnecessary_cast)]
  pub fn from_raw_handle(handle: RawWindowHandle) -> Option<Self> {
    let handle = match handle {
      RawWindowHandle::Win32(handle) => handle.hwnd.get() as u64,
      RawWindowHandle::AppKit(handle) => handle.ns_view.as_ptr() as u64,
      RawWindowHandle::Xlib(handle) => handle.window as u64,
      RawWindowHandle::Xcb(handle) => handle.window.get() as u64,
      RawWindowHandle::Wayland(handle) => handle.surface.as_ptr() as u64,
      _ => return None,
    };
    Some(Self::new(handle))
  }

  /// The raw pointer Electron reported.
  pub fn as_ptr(&self) -> *mut c_void {
    self.handle as usize as *mut c_void
  }

  /// The handle as Electron reported it.
  pub fn handle(&self) -> u64 {
    self.handle
  }

  /// The OS this handle belongs to.
  pub fn platform(&self) -> &'static str {
    self.platform
  }
}

impl fmt::Debug for NativeWindow {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("NativeWindow")
      .field("platform", &self.platform)
      .field("address", &format_args!("{:#x}", self.handle))
      .finish()
  }
}

// SAFETY: the handle is an address in the Electron process, not a pointer this
// process ever dereferences on another thread. Every operation that reads it is
// a Win32/AppKit/Xlib call the platform module performs on the thread that owns
// the window, and the value itself is immutable for the lifetime of the window.
unsafe impl Send for NativeWindow {}
unsafe impl Sync for NativeWindow {}

/// The reference's `dyn ActiveEventLoop` slot.
///
/// winit hands its `ApplicationHandler` a borrowed event loop that can create
/// windows, read monitors and exit the process. Electron owns that loop inside
/// its own process, so there is nothing on this side to hand out. The trait stays
/// so the observer signature is the reference's, and [`DetachedEventLoop`] is the
/// value the runtime passes.
pub trait NativeEventLoop {
  /// The OS whose loop this stands in for.
  fn platform(&self) -> &'static str;
}

/// A [`NativeEventLoop`] that is not attached to any loop.
///
/// This is what the runtime hands to a [`crate::NativeWindowEventHandler`]: the
/// loop it would name is Electron's, and every operation the reference performs
/// on it is an `app.*` RPC here.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct DetachedEventLoop;

impl NativeEventLoop for DetachedEventLoop {
  fn platform(&self) -> &'static str {
    std::env::consts::OS
  }
}

/// The native window events the reference observes.
///
/// winit's `WindowEvent` also carries input, drag-and-drop and IME variants;
/// this runtime never sees those, because the renderer owns them and Electron
/// reports the ones it exposes as webview events. What is left is the set the
/// reference actually matches on.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum NativeWindowEvent {
  /// The user asked to close the window.
  CloseRequested,
  /// The native window is gone.
  Destroyed,
  /// The window gained or lost focus.
  Focused(bool),
  /// The window moved.
  Moved(PhysicalPosition<i32>),
  /// The window's surface was resized.
  Resized(PhysicalSize<u32>),
  /// The window moved to a display with another scale factor.
  ScaleFactorChanged {
    /// The new scale factor.
    scale_factor: f64,
    /// The new surface size.
    ///
    /// winit hands the handler an `InnerSizeWriter` here so it can veto the
    /// resize; Electron has already resized the window by the time it reports
    /// the change, so the event carries the size instead.
    new_inner_size: PhysicalSize<u32>,
  },
  /// The system theme changed.
  ThemeChanged(Theme),
  /// The window was occluded or unoccluded by another window.
  Occluded(bool),
  /// The window needs a redraw.
  RedrawRequested,
}

pub(crate) trait MonitorExt {
  /// Get the work area of this monitor.
  ///
  /// The reference needs a native shim per OS — `GetMonitorInfoW`,
  /// `NSScreen.visibleFrame`, `_NET_WORKAREA` — because winit has no work-area
  /// API. Electron reports `Display.workArea` itself, so the trait keeps its
  /// shape and the answer comes from the monitor record.
  fn work_area(&self) -> PhysicalRect<i32, u32>;
}

fn monitor_bounds(monitor: &Monitor) -> PhysicalRect<i32, u32> {
  PhysicalRect {
    position: monitor.position,
    size: monitor.size,
  }
}

/// Maps the monitor JSON shape produced by the Electron main process onto
/// Tauri's physical-pixel `Monitor`.
pub(crate) fn monitor_from_value(value: &Value) -> Option<Monitor> {
  if !value.is_object() {
    return None;
  }
  let rect_position = |value: &Value| {
    PhysicalPosition::new(
      value["x"].as_i64().unwrap_or_default() as i32,
      value["y"].as_i64().unwrap_or_default() as i32,
    )
  };
  let rect_size = |value: &Value| {
    PhysicalSize::new(
      value["width"].as_u64().unwrap_or_default() as u32,
      value["height"].as_u64().unwrap_or_default() as u32,
    )
  };
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

pub trait EventLoopExt {
  #[cfg(target_os = "macos")]
  fn set_activation_policy(&self, policy: tauri_runtime::ActivationPolicy);
  #[cfg(target_os = "macos")]
  fn set_dock_visibility(&self, visible: bool);
  #[cfg(target_os = "macos")]
  fn show_application(&self);
  #[cfg(target_os = "macos")]
  fn hide_application(&self);
  #[cfg(target_os = "macos")]
  fn set_progress_bar(&self, state: tauri_runtime::ProgressBarState);
  fn set_badge_count(&self, count: Option<i64>, desktop_filename: Option<String>);
  fn set_badge_label(&self, label: Option<String>);
  fn cursor_position(&self) -> tauri_runtime::Result<tauri_runtime::dpi::PhysicalPosition<f64>>;
}
