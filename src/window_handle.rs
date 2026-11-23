// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! Raw-handle wrappers.
//!
//! `raw_window_handle`'s handles are not `Send`, but a window's handle has to
//! travel between the thread that asks for it and the one that answers. The
//! newtypes below make that trip explicit; the handle is only borrowed where it
//! is used, exactly as the reference does for winit's handles.
//!
//! Electron hands the runtime a *number* (`window.nativeHandle`), not a live
//! object, so nothing here owns anything: the value is a snapshot of a handle
//! the Electron main process owns, and it is only as valid as that window.

pub(crate) struct SendRawWindowHandle(pub raw_window_handle::RawWindowHandle);
unsafe impl Send for SendRawWindowHandle {}

pub(crate) struct SendRawDisplayHandle(pub raw_window_handle::RawDisplayHandle);
unsafe impl Send for SendRawDisplayHandle {}

impl SendRawWindowHandle {
  /// Wraps the native handle Electron reported for a window.
  ///
  /// `window.nativeHandle` gives the runtime one integer: an `HWND` on Windows,
  /// the content `NSView*` on macOS, the X11 `Window` on Linux. A zero handle
  /// means Electron could not name one, and `None` is returned rather than
  /// borrowing a handle that names nothing.
  pub(crate) fn from_native(handle: isize) -> Option<Self> {
    if handle == 0 {
      return None;
    }

    #[cfg(windows)]
    let raw = {
      let hwnd = std::num::NonZeroIsize::new(handle)?;
      raw_window_handle::RawWindowHandle::Win32(raw_window_handle::Win32WindowHandle::new(hwnd))
    };

    #[cfg(target_os = "macos")]
    let raw = {
      let ns_view = std::ptr::NonNull::new(handle as *mut std::ffi::c_void)?;
      raw_window_handle::RawWindowHandle::AppKit(raw_window_handle::AppKitWindowHandle::new(
        ns_view,
      ))
    };

    #[cfg(any(
      target_os = "linux",
      target_os = "dragonfly",
      target_os = "freebsd",
      target_os = "netbsd",
      target_os = "openbsd"
    ))]
    let raw = raw_window_handle::RawWindowHandle::Xlib(raw_window_handle::XlibWindowHandle::new(
      handle as std::ffi::c_ulong,
    ));

    #[cfg(not(any(
      windows,
      target_os = "macos",
      target_os = "linux",
      target_os = "dragonfly",
      target_os = "freebsd",
      target_os = "netbsd",
      target_os = "openbsd"
    )))]
    let raw = {
      let _ = handle;
      return None;
    };

    Some(Self(raw))
  }
}

impl SendRawDisplayHandle {
  /// A display handle for a runtime that owns no display connection.
  ///
  /// Electron owns the display server connection inside its own process, so this
  /// runtime has none of its own to lend. The value is the platform's empty
  /// handle: it names no connection, and dereferencing it is meaningless.
  pub(crate) fn none() -> Self {
    #[cfg(windows)]
    let raw =
      raw_window_handle::RawDisplayHandle::Windows(raw_window_handle::WindowsDisplayHandle::new());

    #[cfg(target_os = "macos")]
    let raw =
      raw_window_handle::RawDisplayHandle::AppKit(raw_window_handle::AppKitDisplayHandle::new());

    #[cfg(any(
      target_os = "linux",
      target_os = "dragonfly",
      target_os = "freebsd",
      target_os = "netbsd",
      target_os = "openbsd"
    ))]
    let raw =
      raw_window_handle::RawDisplayHandle::Xlib(raw_window_handle::XlibDisplayHandle::new(None, 0));

    #[cfg(not(any(
      windows,
      target_os = "macos",
      target_os = "linux",
      target_os = "dragonfly",
      target_os = "freebsd",
      target_os = "netbsd",
      target_os = "openbsd"
    )))]
    let raw = raw_window_handle::RawDisplayHandle::Web(raw_window_handle::WebDisplayHandle::new());

    Self(raw)
  }

  pub(crate) fn as_raw(&self) -> raw_window_handle::RawDisplayHandle {
    self.0
  }
}

/// Owned copy of a window's raw handles, for a surface that outlives the borrow.
///
/// The reference keeps this for `softbuffer`, which paints the window background
/// under a CEF child that draws with per-pixel alpha. Electron paints its own
/// window background (the root view and the compositor's background), so nothing
/// in this runtime constructs a software surface; the type is kept, with the
/// reference's contract, so the module keeps its shape.
#[cfg(windows)]
#[allow(dead_code)]
#[derive(Clone, Copy, Debug)]
pub(crate) struct SoftbufferWindowHandle {
  display: raw_window_handle::RawDisplayHandle,
  window: raw_window_handle::RawWindowHandle,
}

#[cfg(windows)]
#[allow(dead_code)]
impl SoftbufferWindowHandle {
  /// Captures both raw handles of the window Electron reported, `None` when the
  /// handle names nothing.
  pub(crate) fn new(handle: isize) -> Option<Self> {
    Some(Self {
      display: SendRawDisplayHandle::none().as_raw(),
      window: SendRawWindowHandle::from_native(handle)?.0,
    })
  }
}

#[cfg(windows)]
impl raw_window_handle::HasDisplayHandle for SoftbufferWindowHandle {
  fn display_handle(
    &self,
  ) -> std::result::Result<raw_window_handle::DisplayHandle<'_>, raw_window_handle::HandleError> {
    Ok(unsafe { raw_window_handle::DisplayHandle::borrow_raw(self.display) })
  }
}

#[cfg(windows)]
impl raw_window_handle::HasWindowHandle for SoftbufferWindowHandle {
  fn window_handle(
    &self,
  ) -> std::result::Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
    Ok(unsafe { raw_window_handle::WindowHandle::borrow_raw(self.window) })
  }
}
