// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! macOS backend for the event drain.
//!
//! The reference mirrors cefclient's `main_message_loop_external_pump_mac.mm`: scheduling
//! requests are posted back onto the owning AppKit thread with `performSelector:onThread:`,
//! and delayed work is driven by an `NSTimer` installed in the common and event-tracking
//! run-loop modes so it keeps firing while AppKit spins a nested menu/tracking loop (e.g. a
//! webview context menu) that winit's callbacks never observe.
//!
//! Electron runs the AppKit loop in its own process and never asks the host to pump it, so
//! there is no owner thread to marshal to and no timer to install. The type keeps the
//! reference's four methods; the ones that would touch the timer report.

use std::sync::Weak;

use super::PumpState;

pub(super) struct PlatformPump {
  // Pending work timer. Always absent: nothing arms a timer, because nothing here pumps
  // Electron's loop.
  timer_pending: bool,
}

impl PlatformPump {
  pub(super) fn new(_state: Weak<PumpState>) -> Self {
    Self {
      timer_pending: false,
    }
  }

  /// Asks the owner thread to run `on_schedule_work(delay)`.
  ///
  /// The reference wraps the delay in an `NSNumber` and sends it to an event-handler object on
  /// the thread that built the pump. There is no such handler here, and no
  /// `OnScheduleMessagePumpWork` to receive the request from — Electron's main process writes
  /// to the socket itself.
  pub(super) fn on_schedule_message_pump_work(&mut self, _delay_ms: i64) {
    crate::unsupported::report(
      "external_message_pump::macos::on_schedule_message_pump_work (Electron owns its loop)",
    );
  }

  pub(super) fn set_timer(&mut self, delay_ms: i64) {
    debug_assert!(delay_ms > 0);
    crate::unsupported::report("external_message_pump::macos::set_timer (Electron owns its loop)");
  }

  pub(super) fn kill_timer(&mut self) {
    self.timer_pending = false;
  }

  pub(super) fn is_timer_pending(&self) -> bool {
    self.timer_pending
  }
}
