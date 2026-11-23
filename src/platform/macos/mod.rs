// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! macOS mechanisms the reference reaches for directly.
//!
//! The reference owns the `NSApplication`, its windows and its webviews, so it subclasses
//! AppKit to install a delegate, drives the Dock tile, tracks fullscreen transitions through
//! window notifications and wakes its own run loop from other threads. Electron owns all of
//! that inside its own process here, and the runtime reaches it through the `app.*` and
//! `window.*` RPCs, so the modules below keep the reference's names and shapes and report
//! where the mechanism is no longer reachable from this side.
//!
//! The AppKit and ApplicationServices bindings the reference uses (`NSWindow`, `NSView`,
//! `NSDockTile`, `NSProgressIndicator`, `NSScreen`, `TransformProcessType`, CoreFoundation
//! run loops) are not among this crate's dependencies; only `NSApplication` and
//! `NSRunningApplication` are, which is what the application-level operations below need.

mod application;
mod dock;
mod event_loop;
mod fullscreen;
mod monitor;
mod progress;
mod utils;
mod wake;
mod webview;
mod window;

pub use application::setup_application;
pub(crate) use application::{
  AppDelegate, AppDelegateEvent, activate_application, set_application_event_handler,
};
pub(crate) use fullscreen::FullscreenTransition;
pub(crate) use wake::MainThreadWake;
pub(crate) use window::nswindow;
