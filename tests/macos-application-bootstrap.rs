// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

// AppKit requires the process main thread, which the standard test harness does not use.
#[cfg(target_os = "macos")]
fn main() {
  use objc2::MainThreadMarker;
  use objc2_app_kit::NSApplication;

  let mtm = MainThreadMarker::new().expect("test must run on the main thread");
  if std::env::args().nth(1).as_deref() == Some("--existing-application") {
    let _ = NSApplication::sharedApplication(mtm);
    assert!(std::panic::catch_unwind(tauri_runtime_electron::prepare_macos_application).is_err());
    return;
  }
  assert!(
    std::process::Command::new(std::env::current_exe().unwrap())
      .arg("--existing-application")
      .status()
      .unwrap()
      .success()
  );

  assert!(
    std::thread::spawn(tauri_runtime_electron::prepare_macos_application)
      .join()
      .is_err()
  );

  // The runtime owns no `NSApplication` of its own: Electron's main process does,
  // so the bootstrap only has to leave the process able to talk to it.
  tauri_runtime_electron::prepare_macos_application();
  let application = NSApplication::sharedApplication(mtm);
  tauri_runtime_electron::prepare_macos_application();
  assert!(std::ptr::eq(
    &*application,
    &*NSApplication::sharedApplication(mtm)
  ));
}

#[cfg(not(target_os = "macos"))]
fn main() {}
