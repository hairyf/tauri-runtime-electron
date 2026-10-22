//! End-to-end smoke test against a real Electron window.
//!
//! `cargo run --example smoke` exits non-zero when a getter disagrees with the
//! setter it was handed, so CI fails instead of trusting the compiler. On Linux
//! this needs a display server (CI runs it under `xvfb-run`).

use std::fmt::Debug;

use tauri_runtime::{
  Runtime, RuntimeInitArgs, WindowDispatch,
  dpi::{LogicalSize, Size},
  window::{PendingWindow, RawWindow, WindowBuilder},
};
use tauri_runtime_electron::{
  Electron, ElectronRuntime, ElectronWindowBuilder, ElectronWindowDispatcher,
};

/// Pins the event type so the dispatch methods resolve without annotations.
macro_rules! w {
  (($window:expr) $method:ident ( $($arg:expr),* )) => {
    <ElectronWindowDispatcher as WindowDispatch<()>>::$method(&$window $(, $arg)*)
  };
}

fn check<A: PartialEq + Debug>(failures: &mut usize, name: &str, actual: A, expected: A) {
  if actual == expected {
    println!("ok   {name} = {actual:?}");
  } else {
    *failures += 1;
    println!("FAIL {name} = {actual:?}, expected {expected:?}");
  }
}

fn main() {
  let runtime = <ElectronRuntime as Runtime<()>>::new(RuntimeInitArgs {
    identifier: "tauri-runtime-electron-smoke".into(),
    custom_schemes: Vec::new(),
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
    runtime_init_attrs: Electron::default(),
  })
  .expect("start the Electron runtime");

  let mut failures = 0usize;
  checks(&runtime, &mut failures);

  if failures > 0 {
    println!("\n{failures} check(s) failed");
    // Electron quits when the IPC socket closes.
    std::process::exit(1);
  }
  println!("\nall checks passed");
  std::process::exit(0);
}

fn checks(runtime: &ElectronRuntime, failures: &mut usize) {
  let size_of = |window: &ElectronWindowDispatcher| -> Option<(i64, i64)> {
    let scale = w!((window) scale_factor()).ok()?;
    let size = w!((window) inner_size()).ok()?;
    Some((
      (f64::from(size.width) / scale).round() as i64,
      (f64::from(size.height) / scale).round() as i64,
    ))
  };

  let builder = ElectronWindowBuilder::new()
    .inner_size(720.0, 520.0)
    .title("smoke")
    .decorations(false);
  let pending = PendingWindow::<(), ElectronRuntime>::new(builder, "smoke").expect("pending window");
  let window = runtime
    .create_window(pending, None::<fn(RawWindow)>)
    .expect("create window")
    .dispatcher;

  check(
    failures,
    "builder inner_size",
    size_of(&window),
    Some((720, 520)),
  );
  // The double title bar bug: a builder that drops `decorations` renders a
  // native frame behind the app's own title bar.
  check(failures, "is_decorated", w!((window) is_decorated()).ok(), Some(false));
  check(
    failures,
    "title",
    w!((window) title()).ok(),
    Some("smoke".to_string()),
  );

  w!((window) set_title("smoke 2")).expect("set title");
  check(
    failures,
    "set_title",
    w!((window) title()).ok(),
    Some("smoke 2".to_string()),
  );

  w!((window) set_resizable(false)).expect("set resizable");
  check(
    failures,
    "resizable off",
    w!((window) is_resizable()).ok(),
    Some(false),
  );
  w!((window) set_resizable(true)).expect("set resizable");
  check(
    failures,
    "resizable on",
    w!((window) is_resizable()).ok(),
    Some(true),
  );

  w!((window) set_maximizable(false)).expect("set maximizable");
  check(
    failures,
    "maximizable off",
    w!((window) is_maximizable()).ok(),
    Some(false),
  );
  w!((window) set_maximizable(true)).expect("set maximizable");

  w!((window) set_minimizable(false)).expect("set minimizable");
  check(
    failures,
    "minimizable off",
    w!((window) is_minimizable()).ok(),
    Some(false),
  );
  w!((window) set_minimizable(true)).expect("set minimizable");

  w!((window) set_closable(false)).expect("set closable");
  check(
    failures,
    "closable off",
    w!((window) is_closable()).ok(),
    Some(false),
  );
  w!((window) set_closable(true)).expect("set closable");

  w!((window) set_always_on_top(true)).expect("always on top");
  check(
    failures,
    "always_on_top",
    w!((window) is_always_on_top()).ok(),
    Some(true),
  );
  w!((window) set_always_on_top(false)).expect("always on top off");

  w!((window) set_size(Size::Logical(LogicalSize::new(640.0, 480.0)))).expect("set size");
  check(failures, "set_size", size_of(&window), Some((640, 480)));

  check(failures, "theme", w!((window) theme()).is_ok(), true);

  // Second window: same frameless guarantee, independent of the first.
  let builder = ElectronWindowBuilder::new()
    .inner_size(400.0, 300.0)
    .decorations(false);
  let pending =
    PendingWindow::<(), ElectronRuntime>::new(builder, "smoke-2").expect("pending window");
  let second = runtime
    .create_window(pending, None::<fn(RawWindow)>)
    .expect("create second window")
    .dispatcher;
  check(
    failures,
    "second window is_decorated",
    w!((second) is_decorated()).ok(),
    Some(false),
  );
}
