// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! Win32 mechanisms the reference reaches for directly.
//!
//! The reference needs them because it owns the window: `EnableWindow` to grey
//! out a window it created, `DwmGetWindowAttribute` to centre it, `SetParent` to
//! move a CEF child window between hosts. Electron owns the window here, so most
//! of these are reached through `window.*` RPCs instead; what remains is the
//! handful of Win32 calls that are equivalent to what Electron does internally
//! and that need nothing but the `HWND` Electron reports.

mod event_loop;
mod icon;
mod monitor;
mod webview;
mod window;
