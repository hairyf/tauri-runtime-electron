// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! The running Electron client, for the Electron-specific work the CEF runtime
//! has no equivalent of.
//!
//! Everything else in this crate mirrors `tauri-runtime-cef`. This module is the
//! part that does not: an application can reach the `electron-rs` client the
//! runtime is already driving and use it directly — run a Node script, take a
//! screenshot, call an RPC the Tauri API does not wrap.
//!
//! ```no_run
//! use tauri_runtime_electron::{ElectronClient, ElectronRuntime};
//!
//! #[tauri::command]
//! async fn disk_usage(electron: tauri::State<'_, ElectronClient>) -> Result<String, String> {
//!   let output = electron
//!     .inner()
//!     .clone()
//!     .run("const fs = require('node:fs'); console.log(fs.statfs('/').bfree);")
//!     .await
//!     .map_err(|error| error.to_string())?;
//!   Ok(String::from_utf8_lossy(&output.stdout).into_owned())
//! }
//!
//! tauri::Builder::<ElectronRuntime>::new()
//!   .plugin(tauri_runtime_electron::init())
//!   .invoke_handler(tauri::generate_handler![disk_usage]);
//! ```

use std::sync::Arc;

/// The client of the Electron main process this runtime is driving.
///
/// Not a second Electron process: the same connection every `tauri-runtime` call
/// goes through, so a command it starts and a window the runtime creates share one
/// main thread.
pub type ElectronClient = Arc<electron_rs::Electron>;

thread_local! {
  /// The client of the runtime task currently running on this thread.
  static CLIENT: std::cell::RefCell<Option<ElectronClient>> = const { std::cell::RefCell::new(None) };
}

/// Runs `task` with `client` installed as the current thread's client.
///
/// The runtime wraps every task it runs inline, so a task scheduled through
/// [`RuntimeHandle::run_on_main_thread`](tauri_runtime::RuntimeHandle::run_on_main_thread)
/// can read the client without the application passing it around.
pub(crate) fn with_client<R>(client: ElectronClient, task: impl FnOnce() -> R) -> R {
  /// Puts the previous client back, on panic as well as on success.
  struct Restore(Option<ElectronClient>);
  impl Drop for Restore {
    fn drop(&mut self) {
      CLIENT.with(|slot| slot.replace(self.0.take()));
    }
  }
  let _restore = Restore(CLIENT.with(|slot| slot.replace(Some(client))));
  task()
}

/// The client of the runtime task running on this thread, when there is one.
///
/// `None` off the runtime's own thread: the client is `Send + Sync`, but which
/// application it belongs to is a per-thread fact, so guessing would hand a
/// command to the wrong process.
pub fn current_client() -> Option<ElectronClient> {
  CLIENT.with(|slot| slot.borrow().clone())
}

/// Registers this application's Electron client as Tauri managed state.
///
/// Adds only Rust state: no renderer commands and no Tauri permissions, so an
/// application that never asks for [`ElectronClient`] is unaffected. Registration
/// fails if a client is already managed rather than silently using the other one.
pub fn init() -> tauri::plugin::TauriPlugin<crate::ElectronRuntime> {
  use tauri::Manager;

  tauri::plugin::Builder::new("electron-client")
    .setup(|app, _api| {
      let (sender, receiver) = std::sync::mpsc::sync_channel(1);
      // The public app API reaches this exact runtime handle, and runtime tasks run
      // inline on the main thread, so the thread-local is populated while this runs.
      app.run_on_main_thread(move || {
        let _ = sender.send(current_client());
      })?;
      let client = receiver
        .recv_timeout(crate::runtime::IPC_TIMEOUT)
        .map_err(|error| {
          let kind = match error {
            std::sync::mpsc::RecvTimeoutError::Timeout => std::io::ErrorKind::TimedOut,
            std::sync::mpsc::RecvTimeoutError::Disconnected => std::io::ErrorKind::BrokenPipe,
          };
          std::io::Error::new(kind, format!("Electron client route failed: {error}"))
        })?
        .ok_or_else(|| std::io::Error::other("Electron client route is unavailable"))?;
      if !app.manage(client) {
        return Err(
          std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "ElectronClient is already managed by this app",
          )
          .into(),
        );
      }
      Ok(())
    })
    .build()
}
