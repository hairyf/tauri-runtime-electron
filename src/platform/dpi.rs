// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! DPI awareness for the host process.
//!
//! Chromium sets its own awareness per process, so Electron is unaffected by
//! this. The processes that embed this runtime are the ones Windows virtualizes
//! when they are not aware: `SetCursorPos(2663, 1357)` is then scaled by the
//! display's factor, which puts the pointer near the corner instead of where the
//! caller asked. Every coordinate-taking Win32 call has the same problem, so the
//! awareness is declared once, as early as the runtime is started.

/// Declares per-monitor v2 awareness for the host process.
///
/// An application that already declared its own awareness (through a manifest or
/// an earlier call) keeps it: Windows rejects the second declaration, which is
/// not an error worth surfacing.
pub(crate) fn enable_per_monitor_v2() {
  #[cfg(windows)]
  imp::enable();
}

#[cfg(windows)]
mod imp {
  use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
  };

  pub(super) fn enable() {
    // SAFETY: the awareness context has no aliasing rules, and losing the race
    // with an application that declared its own awareness is the expected case.
    let _ = unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
  }
}
