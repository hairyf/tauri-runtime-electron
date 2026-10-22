// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! Runs [`src/bin/smoke.rs`](../bin/smoke.rs) as a child process.
//!
//! It cannot be an ordinary test: macOS needs AppKit on the main thread while
//! the test harness runs every test on a worker thread, and a separate process
//! also keeps a crashing window from taking the test runner down with it.
//!
//! The child's output is inherited rather than piped: Electron leaves renderer
//! and zygote processes behind on Linux, they keep the pipe open, and reading it
//! to the end would then wait forever. The child aborts itself after two minutes
//! and this test kills it after three, so a hung window fails instead of
//! blocking the job.

use std::{
  process::{Command, Stdio},
  thread,
  time::{Duration, Instant},
};

#[test]
fn window_capabilities_round_trip() {
  if cfg!(any(target_os = "android", target_os = "ios")) {
    eprintln!("skipping: Electron windows are not supported on this target");
    return;
  }

  let mut child = Command::new(env!("CARGO_BIN_EXE_smoke"))
    .stdout(Stdio::inherit())
    .stderr(Stdio::inherit())
    .spawn()
    .expect("run the smoke binary");

  let deadline = Instant::now() + Duration::from_secs(180);
  loop {
    match child.try_wait().expect("wait for the smoke binary") {
      Some(status) => {
        assert!(status.success(), "the smoke window failed with {status}");
        return;
      }
      None if Instant::now() >= deadline => {
        let _ = child.kill();
        let _ = child.wait();
        panic!("the smoke window did not exit within 180s");
      }
      None => thread::sleep(Duration::from_millis(500)),
    }
  }
}
