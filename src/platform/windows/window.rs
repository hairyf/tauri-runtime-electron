// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! Window-level Win32 mechanisms.
//!
//! The reference implements these on the window it created, reading the `HWND`
//! out of winit's raw handle. Here the `HWND` is what Electron reports for
//! `window.nativeHandle`, so the same calls are available — but Electron already
//! performs most of them inside its own process, and the runtime reaches those
//! through the `window.*` RPCs. Only the calls that are pure Win32 and need no
//! state on the Electron side are made from here.

use std::ffi::c_void;

use tauri_runtime::{Icon, ProgressBarState};
use tauri_utils::config::Color;
use windows::Win32::{
  Foundation::{HWND, RECT},
  Graphics::Dwm::{DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute},
  UI::Input::KeyboardAndMouse::{EnableWindow, IsWindowEnabled},
};

use crate::platform::NativeWindow;

impl NativeWindow {
  /// The handle handed to native APIs.
  ///
  /// CEF wraps the window handle in its own type; Electron reports the `HWND`
  /// itself, so the indirection collapses to the same value as [`Self::hwnd`].
  pub(crate) fn cef_host_handle(&self) -> HWND {
    self.hwnd()
  }

  pub(crate) fn hwnd(&self) -> HWND {
    HWND(self.handle() as usize as *mut c_void)
  }

  pub(crate) fn is_enabled(&self) -> bool {
    unsafe { IsWindowEnabled(self.hwnd()) }.as_bool()
  }

  /// Enables or disables the window.
  ///
  /// Electron's `window.enabled` is the same `EnableWindow` call on the same
  /// `HWND`, so this is equivalent rather than emulated.
  pub(crate) fn set_enabled(&self, enabled: bool) {
    let _ = unsafe { EnableWindow(self.hwnd(), enabled) };
  }

  /// Sets the taskbar overlay icon.
  ///
  /// Electron owns the overlay: `window.overlayIcon` hands it the RGBA bytes and
  /// it drives `ITaskbarList3` from its own process. `ITaskbarList3` lives behind
  /// the `Win32_System_Com` and `Win32_UI_Shell` features this crate does not
  /// enable, so there is no `HICON` path to take here.
  pub(crate) fn set_overlay_icon(&self, _icon: Option<Icon<'static>>) {
    crate::unsupported::report("AppWindow::set_overlay_icon (Electron: window.overlayIcon)");
  }

  /// Sets the taskbar progress bar.
  ///
  /// Same split as [`Self::set_overlay_icon`]: `window.progressBar` is the
  /// Electron-side implementation of exactly this `ITaskbarList3` state machine.
  pub(crate) fn set_progress_bar(&self, _state: ProgressBarState) {
    crate::unsupported::report("AppWindow::set_progress_bar (Electron: window.progressBar)");
  }

  /// Applies the window background color.
  ///
  /// The reference only records the color and asks for a redraw, because the
  /// paint itself happens in [`Self::draw_background_surface`]. Electron paints
  /// the window background itself (`window.backgroundColor`), so there is no
  /// surface on this side to invalidate.
  pub(crate) fn set_background_color(&mut self, _color: Option<Color>) {
    crate::unsupported::report(
      "AppWindow::set_background_color (Electron: window.backgroundColor)",
    );
  }

  /// Paints the window's own background over everything its webviews do not
  /// cover.
  ///
  /// The reference needs this because winit registers its window class without a
  /// background brush, and Windows leaves the pixels a destroyed child window
  /// drew last sitting in the parent's client area. Electron's window is a
  /// compositor surface that repaints itself, and its webviews are composited
  /// views rather than child `HWND`s, so nothing is left behind to clear.
  pub(crate) fn draw_background_surface(&mut self) {
    crate::unsupported::report(
      "AppWindow::draw_background_surface (Electron paints its own surface)",
    );
  }

  /// The visible frame height reported by DWM (`DWMWA_EXTENDED_FRAME_BOUNDS`).
  ///
  /// winit's `outer_size` includes the invisible resize/shadow border, which
  /// throws off vertical centering for decorated windows; the DWM extended frame
  /// bounds describe the actually-visible window rectangle. Electron's
  /// `window.center` already centres against the visible frame, so this is no
  /// longer on the centering path — it stays because it is the only way to read
  /// the visible frame from this side. Returns `None` on failure.
  pub(crate) fn dwm_visible_frame_height(&self) -> Option<u32> {
    let mut rect = RECT::default();
    let result = unsafe {
      DwmGetWindowAttribute(
        self.hwnd(),
        DWMWA_EXTENDED_FRAME_BOUNDS,
        &mut rect as *mut _ as *mut _,
        std::mem::size_of::<RECT>() as u32,
      )
    };
    result.ok()?;
    Some((rect.bottom - rect.top) as u32)
  }
}
