//! Proves the build script staged a runnable Electron for this platform.

use std::process::Command;

#[test]
fn staged_electron_reports_its_version() {
  let binary = env!("ELECTRON_RS_STAGED_BINARY");
  assert!(std::path::Path::new(binary).is_file(), "build did not stage Electron: {binary}");
  let output = Command::new(binary)
    .arg("--version")
    .env("ELECTRON_RUN_AS_NODE", "1")
    .output()
    .expect("staged Electron failed to launch");

  let version = String::from_utf8_lossy(&output.stdout);
  assert!(
    output.status.success(),
    "staged Electron exited with {:?}: {}",
    output.status,
    String::from_utf8_lossy(&output.stderr)
  );
  assert!(version.trim().starts_with('v'), "unexpected version output: {version:?}");
}
