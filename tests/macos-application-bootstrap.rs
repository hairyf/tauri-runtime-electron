// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

// AppKit requires the process main thread, which the standard test harness does not use.
#[cfg(target_os = "macos")]
fn main() {
  use objc2::MainThreadMarker;
  use objc2_app_kit::NSApplication;

  let mtm = MainThreadMarker::new().expect("test must run on the main thread");

  // The child starts from a process whose `NSApplication` already exists.
  if std::env::args().nth(1).as_deref() == Some("--existing-application") {
    let _ = NSApplication::sharedApplication(mtm);
    tauri_runtime_electron::prepare_macos_application();
    return;
  }

  // Off the main thread there is no `MainThreadMarker`, so the bootstrap has to
  // refuse rather than touch AppKit from a worker.
  assert!(
    std::thread::spawn(tauri_runtime_electron::prepare_macos_application)
      .join()
      .is_err(),
    "the bootstrap must refuse to run off the main thread"
  );

  // The reference installs an `NSApplication` subclass of its own and panics if one
  // already exists. Electron owns the application object in its own process, so
  // this bootstrap only validates the call and leaves the singleton alone — which
  // makes it idempotent, and is what the assertions below pin down.
  tauri_runtime_electron::prepare_macos_application();
  let application = NSApplication::sharedApplication(mtm);
  tauri_runtime_electron::prepare_macos_application();
  assert!(
    std::ptr::eq(&*application, &*NSApplication::sharedApplication(mtm)),
    "the bootstrap must not replace the process application"
  );

  // And it must still work when an application already exists.
  assert!(
    std::process::Command::new(std::env::current_exe().unwrap())
      .arg("--existing-application")
      .status()
      .unwrap()
      .success(),
    "the bootstrap must accept an existing application"
  );
}

#[cfg(not(target_os = "macos"))]
fn main() {}
