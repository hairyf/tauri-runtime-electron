// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! Runs [`src/bin/smoke.rs`](../bin/smoke.rs) as a child process.
//!
//! It cannot be an ordinary test: macOS needs AppKit on the main thread while
//! the test harness runs every test on a worker thread, and a separate process
//! also keeps a crashing window from taking the test runner down with it. The
//! child aborts itself after two minutes, so this test cannot hang a CI job.

use std::process::Command;

#[test]
fn window_capabilities_round_trip() {
  if cfg!(any(target_os = "android", target_os = "ios")) {
    eprintln!("skipping: Electron windows are not supported on this target");
    return;
  }

  let output = Command::new(env!("CARGO_BIN_EXE_smoke"))
    .output()
    .expect("run the smoke binary");
  let stdout = String::from_utf8_lossy(&output.stdout);
  let stderr = String::from_utf8_lossy(&output.stderr);

  println!("{stdout}");
  if !stderr.is_empty() {
    eprintln!("{stderr}");
  }

  assert!(
    output.status.success(),
    "the smoke window failed:\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
  );
  assert!(
    stdout.contains("checks passed"),
    "the smoke window exited without reporting its checks:\n{stdout}"
  );
}
