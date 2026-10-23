// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! A real runtime, without windows. Like runtime_smoke, isolate process exit and
//! bound hangs in a child so a lifecycle regression cannot stop the test runner.

use std::{
  process::{Command, Stdio},
  sync::{Arc, Mutex},
  thread,
  time::{Duration, Instant},
};

use tauri_runtime::{ExitRequestedEventAction, RunEvent, Runtime, RuntimeHandle, RuntimeInitArgs};
use tauri_runtime_electron::{ElectronRuntime, ElectronRuntimeHandle};

#[test]
fn explicit_exit_lifecycle() {
  if let Ok(mode) = std::env::var("TAURI_ELECTRON_LIFECYCLE_CHILD") {
    exercise(&mode);
    return;
  }

  for (mode, expected) in [("return", 0), ("restart", 0), ("run", 23)] {
    let mut child = Command::new(std::env::current_exe().unwrap())
      .args(["--exact", "explicit_exit_lifecycle", "--nocapture"])
      .env("TAURI_ELECTRON_LIFECYCLE_CHILD", mode)
      .stdout(Stdio::inherit())
      .stderr(Stdio::inherit())
      .spawn()
      .expect("run lifecycle child");
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
      match child.try_wait().expect("wait for lifecycle child") {
        Some(status) => {
          assert_eq!(status.code(), Some(expected), "{mode}: {status}");
          break;
        }
        None if Instant::now() >= deadline => {
          let _ = child.kill();
          let _ = child.wait();
          panic!("{mode}: runtime did not exit within 60s");
        }
        None => thread::sleep(Duration::from_millis(100)),
      }
    }
  }
}

fn exercise(mode: &str) {
  let runtime = <ElectronRuntime as Runtime<()>>::new(RuntimeInitArgs::default())
    .expect("start Electron for lifecycle regression");
  let handle = <ElectronRuntime as Runtime<()>>::handle(&runtime);
  // Setup-time exit must queue, not disappear before run starts.
  <ElectronRuntimeHandle as RuntimeHandle<()>>::request_exit(&handle, 17).unwrap();
  let accepted = match mode {
    "return" => -7,
    "restart" => i32::MAX,
    "run" => 23,
    _ => panic!("unknown lifecycle mode"),
  };
  let seen = Arc::new(Mutex::new(Vec::new()));
  let events = seen.clone();
  let callback = move |event| {
    let mut events = events.lock().unwrap();
    match event {
      RunEvent::Ready => events.push("ready"),
      RunEvent::ExitRequested { code, tx } => {
        if events.len() == 1 {
          assert_eq!(code, Some(17));
          events.push("veto");
          tx.send(ExitRequestedEventAction::Prevent).unwrap();
          <ElectronRuntimeHandle as RuntimeHandle<()>>::request_exit(&handle, accepted).unwrap();
        } else {
          assert_eq!(events.as_slice(), ["ready", "veto"]);
          assert_eq!(code, Some(accepted));
          events.push("accepted");
        }
      }
      RunEvent::Exit => {
        assert_eq!(events.as_slice(), ["ready", "veto", "accepted"]);
        events.push("exit");
      }
      _ => {}
    }
  };
  if mode == "run" {
    <ElectronRuntime as Runtime<()>>::run(runtime, callback);
    panic!("run must exit the process, not return");
  }
  assert_eq!(
    <ElectronRuntime as Runtime<()>>::run_return(runtime, callback),
    accepted
  );
  assert_eq!(
    seen.lock().unwrap().as_slice(),
    ["ready", "veto", "accepted", "exit"]
  );
}
