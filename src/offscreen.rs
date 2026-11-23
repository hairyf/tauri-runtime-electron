// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

use std::sync::{
  Arc, Mutex, Weak,
  atomic::{AtomicBool, Ordering},
};

use tauri_runtime::dpi::Rect;

use crate::platform::NativeWindow;

/// The cursor kinds an offscreen surface can report.
///
/// The reference names CEF's `CursorType` here. Electron's windowless surface
/// keeps the CSS cursor inside the renderer and never notifies the host, so this
/// enum exists so [`CursorHandler`] keeps its shape; no value is ever delivered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CursorType {
  /// The platform's default cursor.
  Pointer,
  /// A crosshair.
  Cross,
  /// A hand, usually over a link.
  Hand,
  /// A text insertion caret.
  IBeam,
  /// A busy indicator.
  Wait,
  /// A help indicator.
  Help,
  /// An edge or corner resize cursor.
  Resize,
  /// Something is to be moved.
  Move,
  /// Vertical text selection.
  VerticalText,
  /// A context menu is available.
  ContextMenu,
  /// Something cannot be done.
  NotAllowed,
  /// Something can be grabbed.
  Grab,
  /// Something is grabbed.
  Grabbing,
  /// The application supplied its own cursor image.
  Custom,
}

/// Extra data for a cursor notification.
///
/// The reference carries CEF's `CursorInfo`, whose custom-cursor buffer is only
/// valid during the callback. Electron reports no cursor at all, so the mirror
/// keeps the kind and leaves the image out: there is no buffer whose lifetime
/// could be documented.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct CursorInfo {
  /// The cursor the page asked for.
  pub cursor_type: CursorType,
}

/// A cursor notification on the offscreen surface. Any custom cursor buffer is
/// only valid during the callback.
pub type CursorHandler = dyn Fn(CursorType, Option<&CursorInfo>) + Send + Sync;

/// One webview's paint target.
///
/// The callback one offscreen paint feeds: the view, its BGRA pixels, and their size.
type PaintCallback = dyn Fn(&OffscreenView, &[u8], u32, u32) + Send + Sync;

/// The reference hands CEF a `RenderHandler` per webview. Electron's offscreen
/// mode is a `webPreferences.offscreen` flag on an ordinary `webContents`, whose
/// frames arrive as `paint` events, so the mirror keeps the per-webview factory
/// and names the callback those events would feed.
#[derive(Clone)]
pub struct RenderHandler {
  paint: Arc<PaintCallback>,
}

impl RenderHandler {
  /// Builds a handler from its paint callback.
  ///
  /// `pixels` are BGRA in physical pixels, `width` and `height` their size; the
  /// view carries the logical bounds and the requested visibility.
  pub fn new<F: Fn(&OffscreenView, &[u8], u32, u32) + Send + Sync + 'static>(paint: F) -> Self {
    Self {
      paint: Arc::new(paint),
    }
  }

  /// Delivers one software frame to the application.
  #[allow(dead_code)]
  pub fn paint(&self, view: &OffscreenView, pixels: &[u8], width: u32, height: u32) {
    (self.paint)(view, pixels, width, height);
  }
}

/// How the browser renders its own UI.
///
/// The reference selects CEF's `ALLOY` runtime style for windowless browsers.
/// Electron has no runtime style to select — the popup's window is an ordinary
/// `BaseWindow` — so the value is recorded and reported, never applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum RuntimeStyle {
  /// Alloy: the application owns every pixel of the window.
  #[default]
  Alloy,
  /// Chrome: the browser draws its own UI.
  Chrome,
}

/// What an offscreen webview would be created with.
///
/// The reference builds a `cef::WindowInfo` here. Electron has no window info to
/// build — the mode is a webview preference — so the mirror keeps the four fields
/// the reference sets, which is everything an application could observe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct OffscreenInfo {
  pub(crate) windowless_rendering_enabled: bool,
  pub(crate) shared_texture_enabled: bool,
  pub(crate) external_begin_frame_enabled: bool,
  pub(crate) runtime_style: RuntimeStyle,
}

impl OffscreenInfo {
  /// Windowless rendering on, Alloy style, no paint options.
  fn windowless() -> Self {
    Self {
      windowless_rendering_enabled: true,
      shared_texture_enabled: false,
      external_begin_frame_enabled: false,
      runtime_style: RuntimeStyle::Alloy,
    }
  }
}

/// Windowless browser configuration. The application owns painting, input/IME
/// forwarding and composition; Tauri keeps its IPC, protocols and browser
/// lifecycle.
///
/// The reference enables CEF's windowless rendering through
/// [`crate::Cef::with_settings`] and reaches the browser host through
/// [`crate::Webview::browser`]. Electron's offscreen mode is a `webPreferences`
/// flag on a webview that still lives in a `BaseWindow`, and this runtime's
/// bridge exposes neither the flag nor a `paint` event, so every entry point
/// below reports that it cannot be honoured instead of pretending to paint.
#[derive(Clone)]
pub struct Offscreen {
  /// Delivers shared GPU resources through the accelerated paint callback.
  /// Open and copy them during the callback, respecting their lifetime.
  pub shared_texture_enabled: bool,
  /// Requires the application to drive every frame itself.
  pub external_begin_frame_enabled: bool,
  /// Handles cursor changes; other display notifications remain managed by Tauri.
  pub cursor_handler: Option<Arc<CursorHandler>>,
  pub(crate) render_handler: Arc<dyn Fn(OffscreenView) -> RenderHandler + Send + Sync>,
}

impl Offscreen {
  /// Creates a render handler for each webview.
  ///
  /// The handler must read the current bounds and visibility from `view` and
  /// call [`OffscreenView::request_redraw`] after painting. View rectangles use
  /// logical pixels, while paint buffers use physical pixels.
  pub fn new<F: Fn(OffscreenView) -> RenderHandler + Send + Sync + 'static>(handler: F) -> Self {
    Self {
      shared_texture_enabled: false,
      external_begin_frame_enabled: false,
      cursor_handler: None,
      render_handler: Arc::new(handler),
    }
  }

  /// The window info this configuration asks for.
  ///
  /// The reference turns this into a `cef::WindowInfo` and creates a windowless
  /// browser with it. Electron's bridge has no `webPreferences.offscreen` in its
  /// `webview.create` payload, so the runtime cannot ask for the mode; the value
  /// is built and reported so a caller learns the request was not honoured.
  #[allow(dead_code)]
  pub(crate) fn window_info(&self, _parent: isize) -> OffscreenInfo {
    crate::unsupported::report("Offscreen::window_info");
    OffscreenInfo {
      shared_texture_enabled: self.shared_texture_enabled,
      external_begin_frame_enabled: self.external_begin_frame_enabled,
      ..OffscreenInfo::windowless()
    }
  }
}

/// Live layout of one offscreen webview, shared with its render handler.
#[derive(Clone)]
pub struct OffscreenView {
  /// Weak, so a render handler that outlives the window does not keep it alive.
  pub(crate) window: Weak<NativeWindow>,
  pub(crate) bounds: Arc<Mutex<Rect>>,
  pub(crate) visible: Arc<AtomicBool>,
}

impl OffscreenView {
  /// Native host used for scale, screen coordinates, IME and redraw requests.
  ///
  /// Electron's offscreen surface is a webview preference rather than a separate
  /// browser, so there is no host window to hand out and this always reports
  /// `None`; the scale and screen coordinates come from the window record.
  pub fn window(&self) -> Option<Arc<NativeWindow>> {
    crate::unsupported::report("OffscreenView::window");
    self.window.upgrade()
  }

  /// Asks the host window to redraw, if it still exists.
  ///
  /// The reference reaches the host window directly. The mirror's equivalent is
  /// `webContents.invalidate()`, which the bridge does not expose, so the request
  /// is reported and dropped.
  pub fn request_redraw(&self) {
    crate::unsupported::report("OffscreenView::request_redraw");
  }

  /// Bounds relative to the host, in logical pixels.
  pub fn bounds(&self) -> Rect {
    *self.bounds.lock().unwrap_or_else(|e| e.into_inner())
  }

  /// Requested webview visibility, independent of native window occlusion.
  pub fn is_visible(&self) -> bool {
    self.visible.load(Ordering::Relaxed)
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn offscreen_options_select_alloy_and_the_requested_paint_mode() {
    let mut options = Offscreen::new(|_| RenderHandler::new(|_, _, _, _| {}));
    let software = options.window_info(0);
    assert!(software.windowless_rendering_enabled);
    assert_eq!(software.runtime_style, RuntimeStyle::Alloy);
    assert!(!software.shared_texture_enabled);
    assert!(!software.external_begin_frame_enabled);

    options.shared_texture_enabled = true;
    options.external_begin_frame_enabled = true;
    let accelerated = options.window_info(0);
    assert!(accelerated.shared_texture_enabled);
    assert!(accelerated.external_begin_frame_enabled);
  }
}
