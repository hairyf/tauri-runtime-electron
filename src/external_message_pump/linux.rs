// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! Linux/BSD backend for the event drain.
//!
//! The reference mirrors cefclient's `main_message_loop_external_pump_linux.cc`: a custom
//! low-priority `GSource` is attached to the default GLib context, delayed work is tracked as
//! an absolute deadline, and cross-thread scheduling wakes the GLib poll through a pipe.
//!
//! Electron runs the GLib loop in its own process and never asks the host to pump it, so there
//! is no source to attach, no deadline to track and no pipe to write. The type keeps the
//! reference's four methods; the ones that would touch the source report.

use std::sync::Weak;

use super::PumpState;

pub(super) struct PlatformPump {
  // The time when we need to do delayed work. Always absent: nothing arms a deadline, because
  // nothing here pumps Electron's loop.
  delayed_work_time: Option<std::time::Instant>,
}

impl PlatformPump {
  pub(super) fn new(_state: Weak<PumpState>) -> Self {
    Self {
      delayed_work_time: None,
    }
  }

  /// Asks the owner thread to run `on_schedule_work(delay)`.
  ///
  /// The reference writes the delay into a wakeup pipe so the GLib poll returns and the
  /// attached source's `check` callback picks it up. There is no source and no pipe here, and
  /// no `OnScheduleMessagePumpWork` to receive the request from — Electron's main process
  /// writes to the socket itself.
  pub(super) fn on_schedule_message_pump_work(&mut self, _delay_ms: i64) {
    crate::unsupported::report(
      "external_message_pump::linux::on_schedule_message_pump_work (Electron owns its loop)",
    );
  }

  pub(super) fn set_timer(&mut self, delay_ms: i64) {
    debug_assert!(delay_ms > 0);
    crate::unsupported::report("external_message_pump::linux::set_timer (Electron owns its loop)");
  }

  pub(super) fn kill_timer(&mut self) {
    self.delayed_work_time = None;
  }

  /// Whether a deadline is still in the future.
  ///
  /// The reference rounds the remaining time up, so delayed work never runs early. Nothing
  /// sets a deadline here, so the answer is always "no".
  pub(super) fn is_timer_pending(&self) -> bool {
    self
      .delayed_work_time
      .is_some_and(|deadline| deadline > std::time::Instant::now())
  }
}
