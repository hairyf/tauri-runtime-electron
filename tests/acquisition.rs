//! Proves the build script staged a runnable Electron for this platform.

use std::time::Duration;
use tauri_runtime_electron::Electron;
use tokio::io::AsyncReadExt as _;

fn assert_node_stderr(stderr: &[u8], expected: &[u8]) {
  let prefix = stderr.strip_suffix(expected).expect("Node stderr preserves the requested raw bytes");
  #[cfg(windows)]
  if !prefix.is_empty() {
    let diagnostic = std::str::from_utf8(prefix).expect("Electron diagnostic is UTF-8");
    let lines: Vec<_> = diagnostic.lines().collect();
    assert!(!lines.is_empty());
    for line in lines {
      assert!(line.starts_with('['));
      assert!(line.contains(r"ERROR:third_party\crashpad\crashpad\util\win\registration_protocol_win.cc:108] CreateFile:"));
      assert!(line.ends_with("(0x2)"));
    }
  }
  #[cfg(not(windows))]
  assert!(prefix.is_empty(), "unexpected Node stderr prefix: {prefix:?}");
}

#[test]
fn staged_electron_reports_its_version() {
  let binary = env!("ELECTRON_RS_STAGED_BINARY");
  assert!(std::path::Path::new(binary).is_file(), "build did not stage Electron: {binary}");
  let output = tokio::runtime::Builder::new_current_thread()
    .enable_all()
    .build()
    .expect("Node child runtime")
    .block_on(async {
      let child: tokio::process::Child = Electron::new()
        .binary(binary)
        .node()
        .expect("build staged Electron command")
        .args(["--version"])
        .spawn()
        .expect("spawn staged Electron");
      tokio::time::timeout(Duration::from_secs(8), child.wait_with_output())
        .await
        .expect("Node version timed out")
        .expect("staged Electron failed to finish")
    });

  let version = String::from_utf8_lossy(&output.stdout);
  assert!(
    output.status.success(),
    "staged Electron exited with {:?}: {}",
    output.status,
    String::from_utf8_lossy(&output.stderr)
  );
  assert!(version.trim().starts_with('v'), "unexpected version output: {version:?}");
}

#[test]
fn node_preserves_raw_output_and_nonzero_exit() {
  tokio::runtime::Builder::new_current_thread()
    .enable_all()
    .build()
    .expect("Node output runtime")
    .block_on(async {
      let child = Electron::new()
        .binary(env!("ELECTRON_RS_STAGED_BINARY"))
        .node()
        .expect("build native Node command")
        .args([
          "-e",
          "process.stdout.write(Buffer.from([0,255,195]));process.stderr.write(Buffer.from([128,10]));process.exitCode=7;",
        ])
        .spawn()
        .expect("spawn native Node child");
      let output = tokio::time::timeout(Duration::from_secs(8), child.wait_with_output())
        .await
        .expect("Node output timed out")
        .expect("collect Node output");
      assert_eq!(output.status.code(), Some(7));
      assert_eq!(output.stdout, [0, 255, 195]);
      assert_node_stderr(&output.stderr, &[128, 10]);
    });
}

#[test]
fn staged_electron_reports_the_heap_limit_it_enforces() {
  let electron = Electron::new().binary(env!("ELECTRON_RS_STAGED_BINARY"));
  let (honoured, clamped) = tokio::runtime::Builder::new_current_thread()
    .enable_all()
    .build()
    .expect("heap limit runtime")
    .block_on(async {
      tokio::time::timeout(Duration::from_secs(30), async {
        (
          electron.heap_size_limit(1024).await.expect("measure 1024 MiB"),
          electron.heap_size_limit(32768).await.expect("measure 32768 MiB"),
        )
      })
      .await
      .expect("heap limit probe timed out")
    });
  println!("staged Electron: {honoured}; {clamped}");
  // V8 adds its young generation on top of the old space the switch names, so an
  // honoured request reports more than it asked for: the number comes from V8,
  // not from the request echoed back.
  assert!(honoured.limit_mib() > 1024, "{honoured}");
  assert!(!honoured.is_clamped(), "{honoured}");
  // The bundled distribution is built with V8's pointer-compression cage, which is
  // why the probe exists: a request above it is clamped instead of honoured.
  assert!(clamped.is_clamped(), "{clamped}");
  assert!(clamped.limit_bytes() >= honoured.limit_bytes(), "{honoured}; {clamped}");
}

#[test]
fn node_streams_before_exit_and_supports_native_kill() {
  tokio::runtime::Builder::new_current_thread()
    .enable_all()
    .build()
    .expect("Node streaming runtime")
    .block_on(async {
      let mut child = Electron::new()
        .binary(env!("ELECTRON_RS_STAGED_BINARY"))
        .node()
        .expect("build live Node command")
        .args([
          "-e",
          "process.stdout.write(Buffer.from([0,255,195]));setInterval(()=>{},1000);",
        ])
        .spawn()
        .expect("spawn live Node child");
      let mut stdout = child.stdout.take().expect("piped Node stdout");
      let mut first = [0; 3];
      tokio::time::timeout(Duration::from_secs(8), stdout.read_exact(&mut first))
        .await
        .expect("live Node output timed out")
        .expect("read live Node bytes");
      assert_eq!(first, [0, 255, 195]);
      assert!(child.try_wait().expect("query live Node child").is_none());
      tokio::time::timeout(Duration::from_secs(8), child.kill())
        .await
        .expect("Node kill timed out")
        .expect("kill native Node child");
      assert!(!child.wait().await.expect("reap killed Node child").success());
    });
}
