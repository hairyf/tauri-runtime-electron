// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

#![allow(clippy::arc_with_non_send_sync)]
#![allow(clippy::too_many_arguments)]
// The module tree mirrors `tauri-runtime-cef` item for item, so it keeps the
// reference's full surface even where this runtime reaches a mechanism over RPC
// instead of calling it in-process, and even where a platform's code cannot be
// compiled here. Removing an item to silence a warning would break the mirror.
#![allow(dead_code)]

mod devtools;
mod dialog;
/// The `Last Version` breadcrumb in the root cache path, and what a rollback to an older runtime
/// does with the profile a newer one left there.
mod downgrade;
/// The diagnostic environment variables Chromium reads, and the policy over them.
mod environment;
mod external_message_pump;
mod frame;
pub use dialog::{
  NativeDialogKind, NativeDialogObservation, NativeDialogSnapshot, NativeDialogToken,
};
pub use environment::DebugEnvironment;
mod frame_navigation;
/// The languages the user asked their operating system for.
mod locale;
mod macros;
mod offscreen;
pub use offscreen::{
  CursorHandler, CursorInfo, CursorType, Offscreen, OffscreenView, RenderHandler,
};
mod platform;
mod popup;
mod runtime;
// `SandboxPolicy` itself is public API and lives in `runtime`; this module holds the
// decision behind it.
mod sandbox;
/// Helpers for the Chromium command line the runtime hands to the Electron main process.
mod switches;
mod tauri_ext;
/// Reports a capability Electron has no equivalent for, once per API.
mod unsupported;
mod webview;
pub use devtools::{DevToolsMessageIdExhausted, allocate_devtools_message_id};
pub use frame::{FrameEvent, FrameEventHandler, FrameEventKind};
pub use frame_navigation::{FrameNavigationState, NativeDocumentToken};
/// The Electron main process bridge: process ownership, the event stream and RPC.
mod electron_impl;
mod window;
mod window_builder;
mod window_handle;

// The Electron-specific layer. Everything above mirrors `tauri-runtime-cef`; this
// is what the mirror cannot express, and it is additive — an application that only
// uses the Tauri API never touches it.
/// The running Electron client, for the work the CEF runtime has no equivalent of.
mod command_bridge;
pub use command_bridge::{ElectronClient, current_client, init};
pub use electron_rs::{Command, CommandChunk, OutputStream};

/// Marks the application entry point so Electron helper processes are handled.
pub use macros::electron_entry_point;
#[cfg(target_os = "macos")]
pub use platform::macos::setup_application as prepare_macos_application;
pub use runtime::*;
pub use tauri_ext::*;
pub use webview::*;
pub use window::{ElectronWindowDispatcher, NativeWindowToken};
pub use window_builder::WindowBuilderWrapper;
