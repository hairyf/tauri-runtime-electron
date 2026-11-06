// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

#![allow(dead_code)]
#![allow(missing_docs)]

//! A Tauri runtime that delegates all window and webview work to an Electron main
//! process spawned as a child of the Tauri (Rust) process.
//!
//! ```ignore
//! use tauri_runtime_electron::ElectronRuntime;
//!
//! tauri::Builder::<ElectronRuntime>::new()
//!   .run(tauri::generate_context!())
//!   .unwrap();
//! ```
//!
//! The file layout mirrors `tauri-runtime-cef`; modules that exist only because
//! CEF exposes native handles (`cef_impl`, `dialog`, `frame`, `popup`,
//! `sandbox`) have no Electron counterpart. Process communication is provided by
//! the independent `electron-rs` crate.

mod app_icon;
mod authenticated_host;
pub use authenticated_host::{authenticate_web_host, AuthenticatedWebHost};
#[cfg(feature = "tauri")]
pub use authenticated_host::bind_authenticated_web_host;
mod browser;
mod command_bridge;
mod devtools;
mod environment;
mod guest;
mod locale;
#[cfg(target_os = "macos")]
mod macos_menu;
mod load_state;
pub use load_state::WebviewLoadState;
#[cfg(feature = "tauri")]
pub use load_state::on_webview_load_state;
pub mod new_window;
mod platform;
mod rpc;
mod rpc_runtime;
mod shell_open;

mod runtime;
mod setup;
mod switches;
mod webview;
pub mod window;
mod window_builder;
mod window_handle;

#[cfg(feature = "tauri")]
pub use command_bridge::init;
pub use command_bridge::ElectronClient;
pub use electron_rs::{Command, CommandChunk, OutputStream};
pub use environment::DebugEnvironment;
pub use guest::with_guest;
pub use setup::WebviewSetup;
pub use browser::BrowserGuestReservation;
#[cfg(feature = "tauri")]
pub use browser::{acquire_browser_guest, release_browser_guest, on_browser_open_requested};
#[cfg(feature = "tauri")]
pub use new_window::on_new_window;
pub use shell_open::{open_external, open_path};
pub use new_window::{with_opener, NewWindowFeatures, NewWindowOpener, NewWindowResponse};
pub use runtime::{
  register_custom_protocol_scheme, set_app_details, Electron, ElectronRuntime, EventProxy,
  RuntimeContext,
};
pub use webview::ElectronWebviewDispatcher;
#[cfg(feature = "tauri")]
pub use webview::capture_webview;
pub use window::ElectronWindowDispatcher;
pub use window_builder::ElectronWindowBuilder;
pub use window_handle::ElectronRuntimeHandle;
