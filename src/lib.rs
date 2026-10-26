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
mod devtools;
mod environment;
mod locale;
mod platform;

mod runtime;
mod switches;
mod webview;
pub mod window;
mod window_builder;
mod window_handle;

pub use environment::DebugEnvironment;
pub use runtime::{Electron, ElectronRuntime, EventProxy, RuntimeContext, register_custom_protocol_scheme};
pub use webview::ElectronWebviewDispatcher;
pub use window::ElectronWindowDispatcher;
pub use window_builder::ElectronWindowBuilder;
pub use window_handle::ElectronRuntimeHandle;
