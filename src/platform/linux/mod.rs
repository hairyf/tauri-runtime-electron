// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! X11 and GTK mechanisms the reference reaches for directly.
//!
//! The reference needs them because it owns the window and because CEF creates native X11
//! child windows: it builds a 24-bit X11 host over GTK's content box, drives EWMH client
//! messages for activation and workspace state, and loads `libunity` itself for the launcher
//! badge. Electron owns the window, its webviews are composited views rather than X11
//! children, and it links `libunity` in its own process, so the runtime reaches these through
//! `window.*` RPCs instead.
//!
//! The `x11-dl`, `dlopen2` and `libc` dependencies the reference uses are not among this
//! crate's, so the modules below keep the reference's names and report where the mechanism
//! would have been reached over the X11 connection.

mod event_loop;
mod monitor;
mod taskbar;
mod utils;
mod webview;
mod window;

pub(crate) use window::CefX11Host;
