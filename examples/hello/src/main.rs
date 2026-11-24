// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! A Tauri application on the Electron runtime.
//!
//! The smoke test in `tests/smoke.rs` drives `tauri-runtime` directly, which
//! proves the runtime but not the stack above it. This app runs the whole path a
//! real application takes — `tauri` resolving `WebviewUrl::App` to this runtime's
//! own scheme, serving `frontendDist` over it, injecting the IPC bootstrap, and
//! routing a command back into Rust — and prints one line the harness reads.
//!
//! The exit code cannot carry the verdict: `Runtime::run` returns the way the
//! reference's does, and `App::run` exits `0` once the loop ends. So the harness
//! looks for `hello: passed`.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::time::Duration;

use tauri::AppHandle;
use tauri_runtime_electron::{Electron, SandboxPolicy};

/// The heading the page renders, and what this app asserts it read back.
const HEADING: &str = "hello from Electron";

/// Prints the runtime's own diagnostics to stderr.
struct StderrLogger;

impl log::Log for StderrLogger {
  fn enabled(&self, _metadata: &log::Metadata<'_>) -> bool {
    true
  }

  fn log(&self, record: &log::Record<'_>) {
    eprintln!("  [{}] {}", record.level(), record.args());
  }

  fn flush(&self) {}
}

fn main() {
  let _ = log::set_boxed_logger(Box::new(StderrLogger));
  log::set_max_level(log::LevelFilter::Info);

  let mut runtime = Electron::default();

  // Chromium's sandbox needs a broker a headless test machine cannot provide, so
  // the harness turns it off. A real application should leave the default alone.
  if std::env::var_os("ELECTRON_HELLO_NO_SANDBOX").is_some() {
    runtime = runtime.sandbox(SandboxPolicy::Disabled);
  }
  // Where the Chromium profile lives. The harness points it inside the checkout
  // so a run leaves nothing behind in the user's cache directory.
  if let Some(cache) = std::env::var_os("ELECTRON_HELLO_CACHE_DIR") {
    runtime = runtime.root_cache_path(cache);
  }

  tauri::Builder::default()
    // Selecting the runtime: `Electron` is this crate's `RuntimeInitAttrs`.
    .runtime(runtime)
    .invoke_handler(tauri::generate_handler![loaded, window_state])
    .setup(|app| {
      // `WebviewUrl::default()` is `App("index.html")`, which is the document a
      // `tauri dev` app opens.
      tauri::WebviewWindowBuilder::new(app, "main", tauri::WebviewUrl::default())
        .title("Electron Hello")
        .inner_size(640.0, 480.0)
        .build()?;

      // A page that never loads must fail the run rather than hang it.
      let handle = app.handle().clone();
      std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(60));
        eprintln!("hello: FAIL the page never called back");
        handle.exit(1);
      });
      Ok(())
    })
    .run(tauri::generate_context!())
    .expect("the application runs");
}

/// A window API reached from inside a Tauri command.
///
/// This is the path every `@tauri-apps/api/window` call takes: the renderer's
/// `ipc://` request becomes a command, the command asks the runtime, the runtime
/// asks Electron and waits for the answer, and the answer has to travel back out
/// as one `protocol.respond`. A runtime that blocks the run loop waiting for that
/// answer deadlocks here, because an `async` command only answers once the loop
/// turns.
#[tauri::command]
fn window_state(window: tauri::WebviewWindow) -> Result<bool, String> {
  window.is_maximized().map_err(|error| error.to_string())
}

/// The page loaded and reached the IPC bridge.
#[tauri::command]
fn loaded(app: AppHandle, url: String, heading: String, plugin: String, maximized: bool) {
  println!("hello: url={url}");
  println!("hello: heading={heading:?}");
  println!("hello: plugin:window|is_maximized => {plugin}");
  println!("hello: window.is_maximized() => {maximized}");

  // `http://tauri.localhost/` is the whole point: it is what this runtime's
  // `custom_scheme_url` answers with, so the document came from `frontendDist`
  // and `tauri` recognises the page as local.
  let served = url.starts_with("http://tauri.localhost/");
  let rendered = heading == HEADING;
  let answered = plugin.starts_with("ok ") && !maximized;

  if !served {
    eprintln!("hello: FAIL the document was not served over this runtime's scheme");
  }
  if !rendered {
    eprintln!("hello: FAIL the page did not render `{HEADING}`");
  }
  if !answered {
    eprintln!("hello: FAIL a window API did not answer");
  }
  if served && rendered && answered {
    println!("hello: passed");
  }

  app.exit(if served && rendered && answered { 0 } else { 1 });
}
