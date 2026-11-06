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

fn main() {
  if let Ok(mode) = std::env::var("TAURI_ELECTRON_LIFECYCLE_CHILD") {
    exercise(&mode);
    return;
  }

  for (mode, expected) in [("return", 0), ("restart", 0), ("run", 23)] {
    let mut child = Command::new(std::env::current_exe().unwrap())
      .env("TAURI_ELECTRON_LIFECYCLE_CHILD", mode)
      .stdout(Stdio::inherit())
      .stderr(Stdio::inherit())
      .spawn()
      .expect("run lifecycle child");
    eprintln!("[lifecycle] mode={mode} child_pid={} started", child.id());
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
      match child.try_wait().expect("wait for lifecycle child") {
        Some(status) => {
          eprintln!("[lifecycle] mode={mode} child_pid={} exited={status}", child.id());
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

#[cfg(not(windows))]
fn startup_snapshot(mode: &str, stage: &str, _failed: bool) {
  eprintln!("[lifecycle] mode={mode} stage={stage} rust_pid={}", std::process::id());
}

#[cfg(windows)]
fn startup_snapshot(mode: &str, stage: &str, failed: bool) {
  use std::{fs, io::Read, path::PathBuf};
  let owner = std::env::var_os("TAURI_RUNTIME_ELECTRON_OWNER").or_else(|| {
    std::env::current_exe().ok().and_then(|exe| exe.file_stem().map(ToOwned::to_owned))
  });
  let profile = std::env::var_os("APPDATA").zip(owner).map(|(root, owner)| PathBuf::from(root).join(owner));
  eprintln!("[lifecycle] mode={mode} stage={stage} rust_pid={} profile={profile:?}", std::process::id());
  if !failed { return; }
  if failed {
    if let Some(profile) = &profile {
      for directory in [profile.parent().unwrap_or(profile).to_path_buf(), profile.clone(), profile.join("GPUCache"), profile.join("ShaderCache")] {
        eprintln!("[lifecycle] metadata {directory:?}: {:?}", fs::metadata(&directory).map(|meta| (meta.is_dir(), meta.len(), meta.modified().ok())));
        if directory == *profile || directory.starts_with(profile) {
          let entries = fs::read_dir(&directory).into_iter().flatten();
          for entry in entries.take(20).flatten() {
            eprintln!("[lifecycle] entry {:?}: {:?}", entry.file_name(), entry.metadata().map(|meta| (meta.is_dir(), meta.len(), meta.modified().ok())));
          }
        }
      }
    }
  }
  let path = std::env::temp_dir().join(format!("runtime-lifecycle-{}-{stage}.txt", std::process::id()));
  let output = match fs::File::create(&path) { Ok(output) => output, Err(error) => { eprintln!("[lifecycle] process snapshot file: {error}"); return; } };
  let binary = env!("ELECTRON_RS_STAGED_BINARY").replace("'", "''");
  let fields = if failed { "ProcessId,ParentProcessId,CommandLine" } else { "ProcessId,ParentProcessId" };
  let script = format!("$ErrorActionPreference='Stop'; Get-CimInstance Win32_Process -Filter \"Name='electron.exe'\" | Where-Object {{ $_.ExecutablePath -eq '{binary}' }} | Select-Object -First 20 {fields} | ConvertTo-Json -Compress");
  let result = Command::new("powershell.exe").args(["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", &script])
    .stdin(Stdio::null()).stdout(Stdio::from(output)).stderr(Stdio::null()).spawn();
  match result {
    Ok(mut child) => {
      let deadline = Instant::now() + Duration::from_secs(3);
      loop {
        match child.try_wait() {
          Ok(Some(status)) => { eprintln!("[lifecycle] process snapshot status={status}"); break; }
          Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
          result => { let _ = child.kill(); let _ = child.wait(); eprintln!("[lifecycle] process snapshot bounded stop: {result:?}"); break; }
        }
      }
      let mut text = String::new();
      if let Ok(file) = fs::File::open(&path) {
        let _ = file.take(16_384).read_to_string(&mut text);
      }
      eprintln!("[lifecycle] Electron processes: {}", text.replace('\0', ""));
    }
    Err(error) => eprintln!("[lifecycle] process snapshot unavailable: {error}"),
  }
  let _ = fs::remove_file(path);
}

fn exercise(mode: &str) {
  startup_snapshot(mode, "before", false);
  let runtime = <ElectronRuntime as Runtime<()>>::new(RuntimeInitArgs::default())
    .unwrap_or_else(|error| {
      startup_snapshot(mode, "failed", true);
      panic!("start Electron for lifecycle regression: {error:?}");
    });
  startup_snapshot(mode, "ready", false);
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
