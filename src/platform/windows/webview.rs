// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! Webview-level Win32 mechanisms.
//!
//! The reference implements these on `AppWebview` because a CEF browser is a
//! native child `HWND` that has to be parented, clipped, raised and resized by
//! hand. Electron's webviews are composited views inside the window's own
//! surface, so most of that work belongs to Electron: `webview.setBounds`,
//! `webview.visibility`, `webview.reparent` and `webview.close` are the RPCs the
//! runtime uses. The functions here keep the reference's names and take the
//! webview's native handle instead of an `AppWebview`, so the ones that are pure
//! Win32 observation still work.

use tauri_runtime::dpi::{PhysicalPosition, PhysicalSize, Rect};
use tauri_utils::config::Color;
use windows::Win32::{
  Foundation::{ERROR_SUCCESS, POINT, RECT, SetLastError},
  Graphics::Gdi::MapWindowPoints,
  UI::WindowsAndMessaging::{
    GWL_STYLE, GetParent, GetWindowLongPtrW, GetWindowRect, HWND_TOP, IsWindow, IsWindowVisible,
    SW_HIDE, SW_SHOW, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
    SetParent, SetWindowLongPtrW, SetWindowPos, ShowWindow, WINDOW_STYLE, WS_CLIPSIBLINGS,
  },
};

use crate::platform::NativeWindow;

pub(crate) fn native_parent_matches(webview: &NativeWindow, parent: &NativeWindow) -> Option<bool> {
  let hwnd = webview.hwnd();
  unsafe {
    if !IsWindow(Some(hwnd)).as_bool() {
      return None;
    }
    // `GetParent` returns NULL both for a window that has no parent and when
    // the call itself fails, and the binding maps that NULL to an `Err`
    // carrying the last error — so clear it first to tell the two apart.
    SetLastError(ERROR_SUCCESS);
    match GetParent(hwnd) {
      Ok(native_parent) => Some(native_parent == parent.hwnd()),
      // `ERROR_SUCCESS`: the window really has no parent, which is an
      // observation of the relationship and not a failure to establish it.
      Err(err) if err.code().is_ok() => Some(false),
      Err(_) => None,
    }
  }
}

pub(crate) fn native_visible(webview: &NativeWindow) -> Option<bool> {
  let hwnd = webview.hwnd();
  unsafe {
    IsWindow(Some(hwnd))
      .as_bool()
      .then(|| IsWindowVisible(hwnd).as_bool())
  }
}

/// Applies the webview background color.
///
/// Electron renders the background inside the composited view, and
/// `webview.backgroundColor` is what sets it; there is no native child window
/// whose brush could be changed from here.
pub(crate) fn set_background_color(webview: &NativeWindow, _color: Option<Color>) {
  let _ = webview;
  crate::unsupported::report(
    "AppWebview::set_background_color (Electron: webview.backgroundColor)",
  );
}

pub(crate) fn bounds(webview: &NativeWindow) -> Option<Rect> {
  let hwnd = webview.hwnd();

  let mut rect = RECT::default();
  unsafe {
    let parent = GetParent(hwnd).ok()?;
    if parent.0.is_null() {
      return None;
    }

    GetWindowRect(hwnd, &mut rect).ok()?;

    let mut points = [
      POINT {
        x: rect.left,
        y: rect.top,
      },
      POINT {
        x: rect.right,
        y: rect.bottom,
      },
    ];
    if MapWindowPoints(None, Some(parent), &mut points) == 0 {
      return None;
    }

    let x = points[0].x;
    let y = points[0].y;
    let width = (points[1].x - points[0].x).max(0) as u32;
    let height = (points[1].y - points[0].y).max(0) as u32;
    Some(Rect {
      position: PhysicalPosition::new(x, y).into(),
      size: PhysicalSize::new(width, height).into(),
    })
  }
}

pub(crate) fn reparent(webview: &NativeWindow, parent: &NativeWindow) {
  let parent = parent.hwnd();
  let _ = unsafe { SetParent(webview.hwnd(), Some(parent)) };
  // The new parent brings a new set of siblings to blend against.
  unclip_siblings(webview);
}

/// Lets this webview blend with the webviews stacked beneath it.
///
/// Views puts WS_CLIPSIBLINGS on every widget it creates, unconditionally and
/// with no opt-out reachable through the CEF API (see
/// `CalculateWindowStylesFromInitParams` in
/// `ui/views/widget/widget_hwnd_utils.cc`). The style makes overlapping siblings
/// mutually exclusive rectangles: only the topmost one paints there, so a
/// translucent webview laid over another one has nothing left to composite
/// against except the window itself, and punches a hole straight through the
/// webview below it. Dropping the style is what puts the lower webview's pixels
/// back under the upper one's alpha.
///
/// Electron's webviews are composited views rather than `HWND`s, so the style is
/// never set in the first place; the call is kept because it is harmless and
/// because a native child window reparented from here would need it.
pub(crate) fn unclip_siblings(webview: &NativeWindow) {
  let hwnd = webview.hwnd();
  unsafe {
    let style = WINDOW_STYLE(GetWindowLongPtrW(hwnd, GWL_STYLE) as u32);
    if (style & WS_CLIPSIBLINGS).0 == 0 {
      return;
    }
    SetWindowLongPtrW(hwnd, GWL_STYLE, (style & !WS_CLIPSIBLINGS).0 as isize);

    // Styles changed after creation do not reach the frame until the window is
    // told to recalculate it.
    let _ = SetWindowPos(
      hwnd,
      None,
      0,
      0,
      0,
      0,
      SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
    );
  }
}

pub(crate) fn apply_visible(webview: &NativeWindow, visible: bool) {
  let _ = unsafe { ShowWindow(webview.hwnd(), if visible { SW_SHOW } else { SW_HIDE }) };
}

/// Destroys the native window hosting this webview.
///
/// The reference has to do this itself because CEF creates the browser's window
/// and only reports `WindowDestroyed` when it goes away. Electron closes the
/// webview through `webview.close`, which tears down the `WebContentsView` in
/// its own process.
pub(crate) fn destroy_host_window(webview: &NativeWindow) {
  let _ = webview;
  crate::unsupported::report("AppWebview::destroy_host_window (Electron: webview.close)");
}

/// Engages or disengages the z-order pin.
///
/// The pin is a window subclass that refuses every `WM_WINDOWPOSCHANGING` while
/// engaged, so nothing but this runtime can move the webview again.
/// `SetWindowSubclass` lives in `Win32_UI_Shell`, which this crate does not
/// enable, and Electron's composited views have no `HWND` to subclass.
pub(crate) fn set_z_order_pinned(webview: &NativeWindow, _pinned: bool) {
  let _ = webview;
  crate::unsupported::report("AppWebview::set_z_order_pinned (no native child window to subclass)");
}

/// Raises this webview above its siblings.
///
/// The raise itself is a plain `SetWindowPos`, but it cannot be pinned here — see
/// [`set_z_order_pinned`] — so a later z-order change from the OS wins, which is
/// what the reference's pin exists to prevent.
pub(crate) fn raise_to_top(webview: &NativeWindow) {
  set_z_order_pinned(webview, false);

  let _ = unsafe {
    SetWindowPos(
      webview.hwnd(),
      Some(HWND_TOP),
      0,
      0,
      0,
      0,
      SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
    )
  };

  set_z_order_pinned(webview, true);
}

pub(crate) fn apply_physical_bounds(
  webview: &NativeWindow,
  _scale: f64,
  x: i32,
  y: i32,
  width: i32,
  height: i32,
) {
  unsafe {
    let _ = SetWindowPos(
      webview.hwnd(),
      None,
      x,
      y,
      width,
      height,
      SWP_NOZORDER | SWP_NOACTIVATE,
    );
  }
}
