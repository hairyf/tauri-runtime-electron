// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! The event drain, the port of the reference's external message pump.
//!
//! The reference runs CEF with `external_message_pump` enabled: CEF does not run its own
//! message loop, and instead asks the host to call `cef::do_message_loop_work()` by invoking
//! `OnScheduleMessagePumpWork(delay)` whenever it has work pending. The module is a port of
//! upstream cefclient's external pump — same scheduling and reentrancy logic — with a
//! [`PlatformPump`] backend per OS that drives a timer on the loop winit already runs:
//!
//! - Windows: a `WM_TIMER` on a message-only window.
//! - macOS: an `NSTimer` in the common and event-tracking run-loop modes.
//! - Linux/BSD: a custom low-priority GLib source with a wakeup pipe.
//!
//! None of that applies here. Electron owns its message loop — Chromium's `MessageLoop` plus
//! Node's `uv` loop — inside its own process, and there is no embedder hook equivalent to
//! `OnScheduleMessagePumpWork`: no `external_message_pump` setting, no `do_message_loop_work`
//! to call, no timer to own and no reentrancy to detect. The reason the module exists — winit
//! and CEF both wanting to own the same native loop — does not arise, because the Rust side
//! and the Electron side are separate processes talking over a socket.
//!
//! What survives is the other half of the job: something has to drain the events Electron
//! pushed. That is what [`ElectronExternalPump::pump`] does, over the `electron-rs` event
//! receiver the runtime already owns. The scheduling entry points are kept so the shape and
//! the reasoning stay recognisable, and they report rather than pretend to arm a timer.
#![allow(dead_code)]

use std::sync::{Arc, Mutex, mpsc::Receiver};

#[cfg(any(
  target_os = "linux",
  target_os = "dragonfly",
  target_os = "freebsd",
  target_os = "netbsd",
  target_os = "openbsd"
))]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

#[cfg(any(
  target_os = "linux",
  target_os = "dragonfly",
  target_os = "freebsd",
  target_os = "netbsd",
  target_os = "openbsd"
))]
use linux::PlatformPump;
#[cfg(target_os = "macos")]
use macos::PlatformPump;
#[cfg(windows)]
use windows::PlatformPump;

// Special timer delay placeholder value. Intentionally 32-bit for Windows and
// OS X platform API compatibility.
const K_TIMER_DELAY_PLACEHOLDER: i64 = i32::MAX as i64;

// The maximum number of milliseconds we're willing to wait between calls to
// DoWork().
const K_MAX_TIMER_DELAY: i64 = 1000 / 30; // 30fps

/// Handle to the event drain. Cloning shares the same underlying state; the
/// backing platform resources are released when the last clone drops.
#[derive(Clone)]
pub(crate) struct ElectronExternalPump {
  state: Arc<PumpState>,
}

impl ElectronExternalPump {
  /// Builds a pump over the runtime's `electron-rs` event receiver.
  ///
  /// The receiver is the one `Ipc::launch` returned and the runtime keeps behind a mutex; the
  /// pump borrows it rather than taking it, so the run loop and the pump see the same stream.
  pub(crate) fn new(events: Arc<Mutex<Receiver<electron_rs::Event>>>) -> Self {
    let state = Arc::new_cyclic(|weak| PumpState {
      events,
      scheduled: Mutex::new(None),
      platform: Mutex::new(PlatformPump::new(weak.clone())),
    });

    Self { state }
  }

  /// Called when Electron reports it has work. May run on any thread.
  ///
  /// The reference receives this from CEF's `OnScheduleMessagePumpWork`. Electron never calls
  /// it — it pushes events itself — so this only records the request and hands it to the
  /// platform backend, which reports that there is no timer to arm.
  pub(crate) fn on_schedule_message_pump_work(&self, delay_ms: i64) {
    self.state.on_schedule_message_pump_work(delay_ms);
  }

  /// Drains every event Electron pushed since the last call. Must run on the owner thread.
  ///
  /// This is the reference's `do_work` in its Electron form: instead of letting a message
  /// loop run, it hands the caller the batch of events to dispatch. An empty batch means
  /// there was nothing to do, which is the answer `perform_message_loop_work` gives when CEF
  /// had no work pending.
  pub(crate) fn pump(&self) -> Vec<electron_rs::Event> {
    self.state.pump()
  }

  /// The wake signal the reference's platform backends raise when Electron reports work.
  ///
  /// Electron has no `app.wakeup` hook: its main process writes to the socket itself, and the
  /// runtime's `EventProxy` — an `mpsc::Sender` the run loop blocks on — is what wakes this
  /// side. Nothing is left for this method to do.
  pub(crate) fn wake(&self) {
    self.state.wake();
  }
}

/// Platform-independent pump state, shared with the [`PlatformPump`] backend.
struct PumpState {
  /// The `electron-rs` event stream, drained by [`Self::pump`].
  events: Arc<Mutex<Receiver<electron_rs::Event>>>,
  /// The delay of the last scheduling request, kept so a caller can tell whether Electron
  /// asked for immediate work.
  scheduled: Mutex<Option<i64>>,
  platform: Mutex<PlatformPump>,
}

impl PumpState {
  /// Post a scheduling request onto the owner thread. The platform backend is
  /// responsible for delivering it there, where it lands back in
  /// [`Self::on_schedule_work`]. Mirrors the platform `OnScheduleMessagePumpWork`.
  fn on_schedule_message_pump_work(&self, delay_ms: i64) {
    if let Ok(mut platform) = self.platform.lock() {
      platform.on_schedule_message_pump_work(delay_ms);
    }
  }

  /// Runs on the owner thread once a scheduling request is delivered. Mirrors
  /// cefclient's `OnScheduleWork`.
  fn on_schedule_work(&self, delay_ms: i64) {
    if let Ok(mut scheduled) = self.scheduled.lock() {
      *scheduled = Some(delay_ms);
    }

    if delay_ms == K_TIMER_DELAY_PLACEHOLDER && self.is_timer_pending() {
      // Don't set the maximum timer requested from DoWork() if a timer event is
      // currently pending.
      return;
    }

    if let Ok(mut platform) = self.platform.lock() {
      platform.kill_timer();
    }

    if delay_ms <= 0 {
      // Execute the work immediately.
      self.perform_message_loop_work();
    } else if let Ok(mut platform) = self.platform.lock() {
      // Never wait longer than the maximum allowed time.
      let delay_ms = delay_ms.min(K_MAX_TIMER_DELAY);
      // Results in call to OnTimerTimeout() after the specified delay.
      platform.set_timer(delay_ms);
    }
  }

  /// Runs on the owner thread when the platform timer fires. Mirrors cefclient's
  /// `OnTimerTimeout`.
  fn on_timer_timeout(&self) {
    if let Ok(mut platform) = self.platform.lock() {
      platform.kill_timer();
    }
    self.perform_message_loop_work();
  }

  /// Drains the events Electron pushed. Mirrors cefclient's `DoWork`.
  fn pump(&self) -> Vec<electron_rs::Event> {
    let receiver = self
      .events
      .lock()
      .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut batch = Vec::new();
    while let Ok(event) = receiver.try_recv() {
      batch.push(event);
    }
    batch
  }

  fn is_timer_pending(&self) -> bool {
    self
      .platform
      .lock()
      .map(|platform| platform.is_timer_pending())
      .unwrap_or(true)
  }

  /// Mirrors cefclient's `PerformMessageLoopWork`.
  ///
  /// The reference guards this against re-entry because `CefDoMessageLoopWork` can run paint
  /// and IPC callbacks that call back into it. Draining a channel cannot re-enter itself, so
  /// the guard has no counterpart; what it returns is whether there was any work at all.
  fn perform_message_loop_work(&self) -> bool {
    !self.pump().is_empty()
  }

  /// Reports that the wake signal has no Electron counterpart.
  fn wake(&self) {
    crate::unsupported::report("ElectronExternalPump::wake (Electron pushes its own events)");
  }
}
