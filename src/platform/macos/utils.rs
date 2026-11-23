// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! AppKit helpers.
//!
//! Three of the reference's four helpers exist because it owns the application:
//! `TransformProcessType` to move the process between foreground and UIElement,
//! `NSColor` values to hand to AppKit, and associated objects to hang the
//! traffic-light observer off an `NSWindow`. Electron's `app.dockVisibility` and
//! `window.backgroundColor` do the first two inside its own process, so what
//! remains here is the epoch the dock bookkeeping is measured from and the
//! associated-object plumbing, which is plain Objective-C runtime work.

use std::{ffi::c_void, sync::OnceLock, time::Instant};

use objc2::{
  ClassType,
  ffi::{OBJC_ASSOCIATION_RETAIN_NONATOMIC, objc_getAssociatedObject, objc_setAssociatedObject},
  rc::Retained,
  runtime::AnyObject,
};
use tauri_utils::config::Color;

/// The two process transforms the reference asks ApplicationServices for.
///
/// `objc2-application-services` is not among this crate's dependencies, so the
/// two states are named here instead of imported; nothing performs the transform
/// — Electron's `app.dockVisibility` does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessApplicationTransformState {
  /// `kProcessTransformToForegroundApplication`.
  Foreground,
  /// `kProcessTransformToUIElementApplication`.
  UiElement,
}

pub fn transform_process_type(_transform_state: ProcessApplicationTransformState) {
  crate::unsupported::report(
    "platform::macos::transform_process_type (Electron: app.dockVisibility)",
  );
}

/// The sRGB components of a Tauri color, in the 0–1 range AppKit works in.
///
/// The reference returns a `Retained<NSColor>`; `NSColor` is not among the AppKit
/// bindings this crate enables, and Electron takes the color as numbers over
/// `window.backgroundColor` / `webview.backgroundColor`, so the components are
/// what is actually needed.
pub fn ns_color_from_tauri_color(color: Color) -> (f64, f64, f64, f64) {
  let Color(red, green, blue, alpha) = color;
  let scale = u8::MAX as f64;
  (
    red as f64 / scale,
    green as f64 / scale,
    blue as f64 / scale,
    alpha as f64 / scale,
  )
}

pub fn instant_epoch() -> Instant {
  static INSTANT_EPOCH: OnceLock<Instant> = OnceLock::new();
  *INSTANT_EPOCH.get_or_init(Instant::now)
}

pub(crate) fn set_associated_object<O: ClassType>(
  object: &O,
  key: *const c_void,
  value: &AnyObject,
) {
  unsafe {
    objc_setAssociatedObject(
      object as *const O as *mut AnyObject,
      key,
      value as *const AnyObject as *mut AnyObject,
      OBJC_ASSOCIATION_RETAIN_NONATOMIC,
    );
  }
}

pub(crate) fn associated_object<O: ClassType>(
  object: &O,
  key: *const c_void,
) -> Option<Retained<AnyObject>> {
  let value = unsafe { objc_getAssociatedObject(object as *const O as *const AnyObject, key) };
  // SAFETY: the association is only ever written by `set_associated_object`,
  // which stores a valid object pointer under this key.
  unsafe { Retained::retain(value.cast_mut()) }
}
