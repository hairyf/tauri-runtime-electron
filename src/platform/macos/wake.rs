// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! Wakes the event loop from any thread without re-entering its event handler.
//!
//! The reference does not signal winit's proxy itself. It pokes the main run loop and lets a
//! run-loop observer, registered in the common modes for the start of every pass, run a
//! callback on the main thread; the runtime signals the proxy from there once no
//! `ApplicationHandler` callback is running (see `RuntimeContext::wake_event_loop`).
//!
//! Signalling from that observer is what keeps winit's handler from being re-entered:
//! CoreFoundation collects the sources to perform right after the "before sources" observers
//! ran, so a proxy signalled there is performed in the same pass, before the loop polls for
//! AppKit events and dispatches one to a winit callback that might spin a nested loop.
//!
//! None of that applies here. This crate does not run a loop and does not link
//! CoreFoundation, so no observer is registered: the runtime's own `EventProxy` — an
//! `mpsc::Sender` the run loop blocks on — is what wakes it, and it can be signalled from any
//! thread without re-entering anything. The type is kept so the call sites and the reasoning
//! stay recognisable.

use std::sync::Arc;

/// Runs on the main thread at the start of every run-loop pass.
pub(crate) type OnPass = dyn Fn() + Send + Sync;

/// Pokes the main run loop from any thread and runs `on_pass` on the main thread at the start
/// of each of its passes, including those of nested AppKit loops.
#[derive(Clone)]
pub(crate) struct MainThreadWake {
  inner: Arc<Inner>,
}

struct Inner {
  on_pass: Arc<OnPass>,
}

impl MainThreadWake {
  /// Registers the pass callback. Main thread only.
  pub(crate) fn new(on_pass: impl Fn() + Send + Sync + 'static) -> Self {
    debug_assert!(
      objc2::MainThreadMarker::new().is_some(),
      "MainThreadWake must be created on the main thread"
    );

    Self {
      inner: Arc::new(Inner {
        on_pass: Arc::new(on_pass),
      }),
    }
  }

  /// Makes the main run loop start a new pass soon, so that `on_pass` runs. Any thread.
  ///
  /// Calls coalesce: the loop wakes once however many times this ran before it did.
  pub(crate) fn wake_up(&self) {
    crate::unsupported::report("platform::macos::MainThreadWake::wake_up (no run-loop observer)");
  }
}
