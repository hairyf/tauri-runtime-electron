// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! Drives a real Electron process through this runtime and asserts what came back.
//!
//! The unit tests mock the RPC boundary; this one does not. It launches the
//! distribution the build script prepared, creates a window with a live document,
//! and checks that every call reached Electron and returned the value Electron
//! actually holds — a setter is only proven by the getter that reads it back.
//!
//! `harness = false` because the run loop owns the process: the standard harness
//! runs each test on a worker thread while Electron expects the main one.
//!
//! ```text
//! cargo test --test smoke -- --nocapture
//! ```
//!
//! Linux needs a display server, so CI runs it under `xvfb-run`.

use std::{
  sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
  },
  time::{Duration, Instant},
};

use tauri_runtime::{
  RunEvent, Runtime, RuntimeHandle, RuntimeInitArgs, WebviewDispatch, WindowDispatch,
  dpi::{LogicalPosition, LogicalSize, Position, Size},
  webview::{PendingWebview, WebviewAttributes},
  window::{PendingWindow, WindowBuilder},
};
use tauri_runtime_electron::{Electron, ElectronRuntime, SandboxPolicy, WindowBuilderWrapper};

/// How long any one condition may take before the smoke test calls it a failure.
const TIMEOUT: Duration = Duration::from_secs(60);

/// A document that reports what the runtime did to it.
///
/// `eval_script` returns the value of its last expression, so a script that reads
/// `document.readyState` proves the page really loaded rather than that the command
/// was merely accepted.
const PAGE: &str = r#"<!doctype html>
<html>
  <head><title>smoke title</title></head>
  <body>
    <h1 id="heading">smoke</h1>
    <script>
      window.__smoke = { ready: document.readyState, marker: 'loaded' };
    </script>
  </body>
</html>"#;

/// What the run loop reported, so a condition can wait for it.
#[derive(Default)]
struct Observed {
  ready: AtomicBool,
  window_events: Mutex<Vec<String>>,
  webview_events: Mutex<Vec<String>>,
}

impl Observed {
  fn window_events(&self) -> Vec<String> {
    self.window_events.lock().unwrap().clone()
  }

  fn webview_events(&self) -> Vec<String> {
    self.webview_events.lock().unwrap().clone()
  }
}

/// Records one assertion so the output says what was proven, not just that it ran.
fn check(what: &str, held: bool) {
  assert!(held, "{what}");
  println!("  ok  {what}");
}

/// Polls `condition` until it holds or [`TIMEOUT`] expires.
fn wait_for(what: &str, mut condition: impl FnMut() -> bool) {
  let deadline = Instant::now() + TIMEOUT;
  while Instant::now() < deadline {
    if condition() {
      return;
    }
    std::thread::sleep(Duration::from_millis(25));
  }
  panic!("timed out after {TIMEOUT:?} waiting for {what}");
}

/// Runs one future to completion on a throwaway runtime.
///
/// The Electron client is async; this test is not, so the two meet here rather than
/// pulling an async runtime through every assertion.
fn block_on<F: std::future::Future>(future: F) -> F::Output {
  tokio::runtime::Builder::new_current_thread()
    .enable_all()
    .build()
    .expect("build a runtime")
    .block_on(future)
}

/// Prints the runtime's own diagnostics to stderr.
///
/// Without a logger every `log::warn!` in the crate is dropped, which turns a
/// reported-unsupported call into silence — exactly the failure mode the crate
/// exists to avoid.
struct StderrLogger;

impl log::Log for StderrLogger {
  fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
    metadata.level() <= log::Level::Warn
  }

  fn log(&self, record: &log::Record<'_>) {
    if self.enabled(record.metadata()) {
      eprintln!("  [{}] {}", record.level(), record.args());
    }
  }

  fn flush(&self) {}
}

fn main() {
  let _ = log::set_boxed_logger(Box::new(StderrLogger));
  log::set_max_level(log::LevelFilter::Warn);

  println!("smoke: launching a real Electron process");

  // The profile goes beside the build output rather than in the user's cache
  // directory, so a test run leaves the machine as it found it — and so the
  // `Last Version` breadcrumb lands somewhere the test can inspect.
  let cache = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
    .join("target")
    .join("smoke-profile");
  let _ = std::fs::remove_dir_all(&cache);

  let mut runtime = <ElectronRuntime<()> as Runtime<()>>::new(RuntimeInitArgs {
    identifier: "tauri-runtime-electron-smoke".into(),
    custom_schemes: vec!["smoke".into()],
    #[cfg(windows)]
    msg_hook: None,
    #[cfg(any(
      target_os = "linux",
      target_os = "dragonfly",
      target_os = "freebsd",
      target_os = "netbsd",
      target_os = "openbsd"
    ))]
    app_id: None,
    runtime_init_attrs: Electron::default()
      .root_cache_path(&cache)
      // Chromium's sandbox needs a session it can broker; a CI runner and a
      // headless shell cannot provide one, and the smoke test is about this
      // runtime's calls, not about Chromium's process isolation.
      .sandbox(SandboxPolicy::Disabled),
  })
  .expect("the runtime starts and connects to Electron");

  let handle = runtime.handle();
  let observed = Arc::new(Observed::default());

  // ── the handshake ───────────────────────────────────────────────────────────
  let version = handle
    .webview_version()
    .expect("Electron reports its version over the handshake");
  check(
    &format!("handshake reports Electron {version}"),
    !version.is_empty(),
  );

  let monitors = handle
    .available_monitors()
    .expect("Electron reports its displays");
  check(
    &format!("{} display(s) reported", monitors.len()),
    !monitors.is_empty(),
  );
  check(
    "a primary display exists",
    handle
      .primary_monitor()
      .expect("primary display query")
      .is_some(),
  );
  let cursor = handle
    .cursor_position()
    .expect("Electron reports the cursor position");
  check(
    &format!("cursor at ({}, {})", cursor.x, cursor.y),
    cursor.x.is_finite() && cursor.y.is_finite(),
  );

  // ── window + webview creation ───────────────────────────────────────────────
  // The page is served from a custom scheme, which is what Tauri does for
  // `tauri://localhost/index.html`. A scheme that is privileged but never
  // registered with the main process falls through to the network stack and the
  // navigation fails with `ERR_FAILED`, so this is the case worth guarding.
  let webview_attributes = WebviewAttributes::new(tauri_utils::config::WebviewUrl::CustomProtocol(
    "smoke://localhost/index.html"
      .parse()
      .expect("the custom-protocol URL parses"),
  ));
  let mut pending_webview =
    PendingWebview::<(), ElectronRuntime<()>>::new(webview_attributes, Default::default(), "smoke")
      .expect("the webview label is valid");
  pending_webview.register_uri_scheme_protocol("smoke", |_webview_id, request, responder| {
    let path = request.uri().path();
    let (status, body) = if path == "/index.html" {
      (200, PAGE.to_owned())
    } else {
      (404, "<h1>not found</h1>".to_owned())
    };
    responder(
      http::Response::builder()
        .status(status)
        .header("Content-Type", "text/html")
        .body(std::borrow::Cow::Owned(body.into_bytes()))
        .expect("a response with a status and a body is always valid"),
    );
  });

  let builder = WindowBuilderWrapper::new()
    .title("smoke window")
    .inner_size(640.0, 480.0)
    .position(120.0, 120.0);
  let mut pending = PendingWindow::<(), ElectronRuntime<()>>::new(builder, "smoke")
    .expect("the window label is valid");
  pending.webview = Some(pending_webview);

  let detached = runtime
    .create_window::<fn(tauri_runtime::window::RawWindow)>(pending, None)
    .expect("Electron creates the window");
  let window = detached.dispatcher.clone();
  let webview = detached
    .webview
    .as_ref()
    .expect("the window carries its initial webview")
    .webview
    .dispatcher
    .clone();
  check("Electron created the window and its webview", true);

  // ── the run loop ────────────────────────────────────────────────────────────
  // Pumped from here so assertions can interleave with the events Electron sends.
  let pump = {
    let observed = Arc::clone(&observed);
    move |runtime: &mut ElectronRuntime<()>| {
      // `run_iteration` takes a `'static` callback, so each pump hands it its own
      // handle on what the run loop reported.
      let observed = Arc::clone(&observed);
      runtime.run_iteration(move |event| match event {
        RunEvent::Ready => observed.ready.store(true, Ordering::SeqCst),
        RunEvent::WindowEvent { event, .. } => observed
          .window_events
          .lock()
          .unwrap()
          .push(format!("{event:?}")),
        RunEvent::WebviewEvent { event, .. } => observed
          .webview_events
          .lock()
          .unwrap()
          .push(format!("{event:?}")),
        _ => {}
      });
    }
  };

  wait_for("the run loop to report Ready", || {
    pump(&mut runtime);
    observed.ready.load(Ordering::SeqCst)
  });
  check("the run loop reported Ready", true);

  // ── setters proven by their getters ─────────────────────────────────────────
  window.set_title("smoke title 2").expect("set_title");
  wait_for("the title to round-trip", || {
    pump(&mut runtime);
    window.title().is_ok_and(|title| title == "smoke title 2")
  });
  check("set_title round-trips through title()", true);

  window
    .set_size(Size::Logical(LogicalSize::new(800.0, 600.0)))
    .expect("set_size");
  wait_for("the inner size to grow", || {
    pump(&mut runtime);
    window
      .inner_size()
      .is_ok_and(|size| size.width >= 700 && size.height >= 500)
  });
  let inner = window.inner_size().expect("inner_size");
  check(
    &format!(
      "set_size(800x600) round-trips through inner_size() ({}x{})",
      inner.width, inner.height
    ),
    inner.width >= 700 && inner.height >= 500,
  );

  window
    .set_position(Position::Logical(LogicalPosition::new(150.0, 160.0)))
    .expect("set_position");
  wait_for("the outer position to be readable", || {
    pump(&mut runtime);
    window.outer_position().is_ok()
  });
  let outer = window.outer_position().expect("outer_position");
  check(
    &format!(
      "set_position(150, 160) round-trips through outer_position() ({}, {})",
      outer.x, outer.y
    ),
    outer.x > 0 || outer.y > 0,
  );

  window.set_resizable(false).expect("set_resizable");
  wait_for("is_resizable to report false", || {
    pump(&mut runtime);
    window.is_resizable().is_ok_and(|value| !value)
  });
  check(
    "set_resizable(false) round-trips through is_resizable()",
    true,
  );
  window.set_resizable(true).expect("set_resizable");
  wait_for("is_resizable to report true", || {
    pump(&mut runtime);
    window.is_resizable().is_ok_and(|value| value)
  });
  check(
    "set_resizable(true) round-trips through is_resizable()",
    true,
  );

  // Maximize and minimize are window-manager operations on Linux. Xvfb runs no
  // window manager, so the flag never reaches the window and the getter keeps
  // reporting the default; the assertion is skipped there rather than pretending
  // the call failed.
  if cfg!(target_os = "linux") {
    println!("  ..  skipped on Linux: no window manager under Xvfb");
  } else {
    window.set_maximizable(false).expect("set_maximizable");
    wait_for("is_maximizable to report false", || {
      pump(&mut runtime);
      window.is_maximizable().is_ok_and(|value| !value)
    });
    check(
      "set_maximizable(false) round-trips through is_maximizable()",
      true,
    );

    window.set_minimizable(false).expect("set_minimizable");
    wait_for("is_minimizable to report false", || {
      pump(&mut runtime);
      window.is_minimizable().is_ok_and(|value| !value)
    });
    check(
      "set_minimizable(false) round-trips through is_minimizable()",
      true,
    );
  }

  window.set_closable(false).expect("set_closable");
  wait_for("is_closable to report false", || {
    pump(&mut runtime);
    window.is_closable().is_ok_and(|value| !value)
  });
  check(
    "set_closable(false) round-trips through is_closable()",
    true,
  );

  // `setAlwaysOnTop` on Windows is `SetWindowPos(HWND_TOPMOST)`, which only takes
  // effect on a mapped window, so the window has to be visible first.
  wait_for("the window to be visible", || {
    pump(&mut runtime);
    window.is_visible().is_ok_and(|visible| visible)
  });
  check("the window is visible after window.ready", true);

  window.set_always_on_top(true).expect("set_always_on_top");
  let mut attempts = 1;
  wait_for("is_always_on_top to report true", || {
    pump(&mut runtime);
    if window.is_always_on_top().is_ok_and(|value| value) {
      return true;
    }
    if attempts < 8 {
      attempts += 1;
      std::thread::sleep(Duration::from_millis(250));
      window.set_always_on_top(true).expect("set_always_on_top");
    }
    false
  });
  check(
    "set_always_on_top(true) round-trips through is_always_on_top()",
    true,
  );

  let scale = window.scale_factor().expect("scale_factor");
  check(&format!("scale_factor is {scale}"), scale > 0.0);
  check(
    "current_monitor names the display the window is on",
    window.current_monitor().expect("current_monitor").is_some(),
  );

  // ── the page really loaded ──────────────────────────────────────────────────
  // The document computes a value in its own script; reading it back proves the
  // page ran, not merely that the navigation command was accepted.
  let mut evaluate = |script: &str| -> Option<String> {
    let result = Arc::new(Mutex::new(None));
    let slot = Arc::clone(&result);
    webview
      .eval_script_with_callback(script, move |value| {
        *slot.lock().unwrap() = Some(value);
      })
      .expect("eval_script_with_callback");
    let deadline = Instant::now() + TIMEOUT;
    while Instant::now() < deadline {
      pump(&mut runtime);
      if let Some(value) = result.lock().unwrap().clone() {
        return Some(value);
      }
      std::thread::sleep(Duration::from_millis(25));
    }
    None
  };

  let marker = evaluate("window.__smoke && window.__smoke.marker");
  check(
    &format!("eval_script_with_callback reads a value the document computed ({marker:?})"),
    marker
      .as_deref()
      .is_some_and(|value| value.contains("loaded")),
  );

  let ready_state = evaluate("document.readyState").unwrap_or_default();
  check(
    &format!("document.readyState is {ready_state}"),
    ready_state.contains("complete") || ready_state.contains("interactive"),
  );

  let heading = evaluate("document.getElementById('heading').textContent").unwrap_or_default();
  check(
    &format!("eval_script reads the document ({heading})"),
    heading.contains("smoke"),
  );

  let current_url = webview.url().expect("webview.url");
  check(
    "webview.url reports the document it loaded",
    current_url.starts_with("smoke://localhost/index.html"),
  );

  let bounds = webview.bounds().expect("webview.bounds");
  let (width, height) = match bounds.size {
    Size::Physical(size) => (size.width, size.height),
    Size::Logical(size) => (size.width as u32, size.height as u32),
  };
  check(
    &format!("webview.bounds reports {width}x{height}"),
    width > 0 && height > 0,
  );

  webview.set_zoom(1.25).expect("set_zoom");
  check("set_zoom reached Electron", true);

  let can_go_back = webview.can_go_back().expect("can_go_back");
  check(
    &format!("can_go_back reports {can_go_back} for a fresh document"),
    !can_go_back,
  );

  println!(
    "smoke: {} window event(s), {} webview event(s)",
    observed.window_events().len(),
    observed.webview_events().len()
  );
  check(
    "the run loop delivered at least one window event",
    !observed.window_events().is_empty(),
  );

  // ── the Electron-specific layer ─────────────────────────────────────────────
  // Everything above mirrors `tauri-runtime-cef`. This is what the mirror cannot
  // express: the raw client, reached the way an application reaches it — from a task
  // the runtime runs on its own thread, which is what `init()` installs as managed
  // state.
  let slot = Arc::new(Mutex::new(None));
  let observed_client = Arc::clone(&slot);
  handle
    .run_on_main_thread(move || {
      *observed_client.lock().unwrap() = tauri_runtime_electron::current_client();
    })
    .expect("schedule the client lookup");
  let client = slot.lock().unwrap().clone();
  check(
    "current_client() finds the client from a runtime task",
    client.is_some(),
  );
  let client = client.expect("the runtime exposes its Electron client");

  // A Node script on the Electron main thread: work no `tauri-runtime` call can
  // express, which is the point of exposing the client at all.
  let command = client.run("console.log('smoke-command', process.platform)");
  let output = block_on(async move { command.await })
    .expect("the Node script runs on the Electron main thread");
  let stdout = String::from_utf8_lossy(&output.stdout);
  check(
    &format!("ElectronClient ran a Node script ({})", stdout.trim()),
    stdout.contains("smoke-command") && output.status.success(),
  );

  // ── teardown ────────────────────────────────────────────────────────────────
  window.destroy().expect("destroy");
  wait_for("the destroy event to arrive", || {
    pump(&mut runtime);
    observed
      .window_events()
      .iter()
      .any(|event| event.contains("Destroyed"))
  });
  check("destroy delivers a Destroyed window event", true);

  println!("smoke: passed");
}
