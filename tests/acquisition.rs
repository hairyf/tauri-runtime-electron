//! Proves the build script staged a runnable Electron for this platform.
//!
//! The binary is what the runtime launches at startup, so it is worth failing
//! the build when acquisition silently produced nothing.

use std::path::PathBuf;
use std::process::Command;

fn staged_binary() -> Option<PathBuf> {
  option_env!("TAURI_RUNTIME_ELECTRON_ELECTRON_BINARY")
    .map(PathBuf::from)
    .filter(|path| path.exists())
}

#[test]
fn staged_electron_reports_its_version() {
  let Some(binary) = staged_binary() else {
    // A local build may legitimately run with `TAURI_RUNTIME_ELECTRON_DOWNLOAD=0`
    // or point at `node_modules/electron`, so a missing stage is not a failure.
    eprintln!("no staged Electron binary in this build");
    return;
  };

  let output = Command::new(&binary)
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
