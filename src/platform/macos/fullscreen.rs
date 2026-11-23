// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! Holds fullscreen requests back while the window is animating a fullscreen transition.
//!
//! `toggleFullScreen:` is a no-op while AppKit is still animating the previous transition of
//! the window, and winit's macOS backend does not cope with that: a `set_fullscreen` call
//! that arrives during a transition is parked and replayed synchronously from inside
//! `windowDidEnterFullScreen:` / `windowDidExitFullScreen:`, where AppKit still drops the
//! toggle. Its state then disagrees with the window's — an enter is reverted by
//! `windowDidFailToEnterFullScreen:`, while a dropped exit leaves winit reporting a windowed
//! state for a window that is still fullscreen, and the next request toggles the wrong way.
//!
//! So the runtime does not hand the window a request while a transition is in flight. The
//! transitions are tracked through the window's own notifications, the latest request made
//! during one is held here, and once the transition ended it is re-issued through the message
//! channel, which lands in `handle_window_message` on a later pass of the run loop — the same
//! deferral tao gets from dispatching the toggle asynchronously.
//!
//! Electron reports the transitions as `enter-full-screen` / `leave-full-screen`, and
//! `electron-rs` already waits on them with a timeout, so the notifications are not
//! observed here: [`FullscreenTransition::transition_ended`] is what the event handler
//! calls. The type is otherwise unchanged, including the timeout that keeps a request from
//! being held forever when the `Did…` notification never arrives.

use std::cell::Cell;

use tauri_runtime::dpi::PhysicalPosition;

/// A transition that has not finished after this many seconds is considered over. AppKit's
/// animation takes well under a second; this only guards against a `Did…` notification that
/// never comes (AppKit posts none when it fails to enter fullscreen), so that a request is not
/// held forever.
const TRANSITION_TIMEOUT: f64 = 3.0;

/// The monitor a fullscreen request targets.
///
/// Kept as the request came in, rather than as a resolved monitor, so that a request held
/// back while a transition is in flight can be applied later. It mirrors the window module's
/// own target enum; it is defined here so the platform module does not depend on the window
/// dispatcher's internals.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum FullscreenTarget {
  /// The monitor the window is on (`set_fullscreen(true)`).
  CurrentMonitor,
  /// The monitor containing this physical position (`set_fullscreen_on_monitor`).
  MonitorAt(PhysicalPosition<f64>),
}

pub(crate) struct FullscreenTransition {
  /// Whether a transition is in flight.
  in_progress: Cell<bool>,
  /// The latest request made during the transition; `Some(None)` is a request to leave.
  pending: Cell<Option<Option<FullscreenTarget>>>,
  /// Called when a transition ends while a request is pending. Runs inside the event
  /// dispatch, so it must only post the request for later.
  on_transition_end: Box<dyn Fn()>,
}

impl FullscreenTransition {
  /// Starts tracking the window's fullscreen transitions.
  ///
  /// The reference observes four `NSWindow` notifications here. Electron reports the two
  /// that matter as window events, so the caller drives [`Self::transition_ended`] from
  /// them and the timeout is the caller's to schedule — see [`TRANSITION_TIMEOUT`].
  pub(crate) fn observe(on_transition_end: impl Fn() + 'static) -> Self {
    Self {
      in_progress: Cell::new(false),
      pending: Cell::new(None),
      on_transition_end: Box::new(on_transition_end),
    }
  }

  /// Marks a transition as started.
  pub(crate) fn transition_started(&self) {
    self.in_progress.set(true);
  }

  /// Marks the transition in flight as over, and reports whether a request was held.
  pub(crate) fn transition_ended(&self) {
    self.in_progress.set(false);
    if self.pending.get().is_some() {
      (self.on_transition_end)();
    }
  }

  /// Whether the window is in the middle of entering or leaving fullscreen.
  pub(crate) fn in_progress(&self) -> bool {
    self.in_progress.get()
  }

  /// Keeps `target` until the transition in progress ends, replacing any earlier request.
  pub(crate) fn hold(&self, target: Option<FullscreenTarget>) {
    self.pending.set(Some(target));
  }

  /// The request held during the transition in progress, if any, without taking it.
  pub(crate) fn pending(&self) -> Option<Option<FullscreenTarget>> {
    self.pending.get()
  }

  /// The request held during the last transition, if any.
  pub(crate) fn take_pending(&self) -> Option<Option<FullscreenTarget>> {
    self.pending.take()
  }
}
