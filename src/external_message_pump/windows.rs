// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! Windows backend for the event drain.
//!
//! The reference mirrors cefclient's `main_message_loop_external_pump_win.cc`: a message-only
//! window owns a `WM_TIMER`, and `OnScheduleMessagePumpWork` posts a private `WM_HAVE_WORK`
//! message to it. winit runs the thread's own `GetMessage`/`DispatchMessage` loop, which
//! delivers both messages to the window procedure — so CEF is pumped from the same loop,
//! including while Windows runs a modal move/resize loop that winit's `ApplicationHandler`
//! callbacks never observe.
//!
//! Electron runs that loop in its own process and never asks the host to pump it, so there is
//! no window to create and no timer to arm. The type keeps the reference's four methods; the
//! two that would touch the timer report.

use std::sync::Weak;

use super::PumpState;

pub(super) struct PlatformPump {
  // True if a timer event is currently pending. Always false: nothing arms a timer, because
  // nothing here pumps Electron's loop.
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
  /// The reference posts `WM_HAVE_WORK` to a message-only window it created for the purpose.
  /// There is no such window here, and no `OnScheduleMessagePumpWork` to receive the request
  /// from — Electron's main process writes to the socket itself.
  pub(super) fn on_schedule_message_pump_work(&mut self, _delay_ms: i64) {
    crate::unsupported::report(
      "external_message_pump::windows::on_schedule_message_pump_work (Electron owns its loop)",
    );
  }

  pub(super) fn set_timer(&mut self, delay_ms: i64) {
    debug_assert!(delay_ms > 0);
    crate::unsupported::report(
      "external_message_pump::windows::set_timer (Electron owns its loop)",
    );
  }

  pub(super) fn kill_timer(&mut self) {
    self.timer_pending = false;
  }

  pub(super) fn is_timer_pending(&self) -> bool {
    self.timer_pending
  }
}
