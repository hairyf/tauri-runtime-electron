//! Launches the real Electron runtime and asserts every window capability
//! round-trips: set something, read it back, compare.
//!
//! A silent no-op cannot survive this, because the getters ask Electron rather
//! than echoing the last value the Rust side sent.
//!
//! `cargo run --bin smoke` needs a display; CI wraps it in `xvfb-run` on Linux.
//!
//! Every dispatcher call is spelled `WindowDispatch::<()>::method(window, ..)`
//! because the crate implements the traits for every `T: UserEvent`; inside a
//! non-generic test there is no `T` to infer, so each call has to pin it.

use std::{
  process::ExitCode,
  sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
    mpsc::channel,
  },
  thread,
  time::{Duration, Instant},
};

use tauri_runtime::{
  RunEvent, Runtime, RuntimeHandle, RuntimeInitArgs, WindowDispatch,
  dpi::{PhysicalPosition, PhysicalSize, Position, Size},
  window::{PendingWindow, RawWindow, WindowEvent},
};
use tauri_runtime_electron::{ElectronRuntime, ElectronRuntimeHandle, ElectronWindowBuilder};
use tauri_utils::{Theme, config::WindowConfig};

/// How long a setter may take to become visible to the getter. Every window
/// manager is asynchronous, so an immediate read would flake; a silent no-op
/// still fails, just after this window instead of instantly.
const SETTLE: Duration = Duration::from_secs(5);

/// The concrete dispatcher types, with the event type pinned to `()`.
type Window = <ElectronRuntime as Runtime<()>>::WindowDispatcher;
type Handle = <ElectronRuntime as Runtime<()>>::Handle;

fn main() -> ExitCode {
  let runtime = match <ElectronRuntime as Runtime<()>>::new(RuntimeInitArgs::default()) {
    Ok(runtime) => runtime,
    Err(error) => {
      eprintln!("[smoke] cannot start Electron: {error}");
      return ExitCode::FAILURE;
    }
  };

  // The event loop lives on its own thread so the checks below can call the
  // dispatchers like an application would.
  let handle: Handle = <ElectronRuntime as Runtime<()>>::handle(&runtime);
  let (exit_tx, exit_rx) = channel::<()>();
  let loop_thread = thread::spawn(move || {
    <ElectronRuntime as Runtime<()>>::run(runtime, move |event| {
      if let RunEvent::WindowEvent { event, .. } = &event {
        let name = match event {
          WindowEvent::Destroyed => "destroyed",
          WindowEvent::CloseRequested { .. } => "closeRequested",
          WindowEvent::Resized(..) => "resized",
          WindowEvent::Moved(..) => "moved",
          WindowEvent::Focused(..) => "focused",
          _ => "other",
        };
        REPORT.lock().unwrap().push(name.to_string());
      }
      if matches!(event, RunEvent::Exit | RunEvent::ExitRequested { .. }) {
        let _ = exit_tx.send(());
      }
    });
  });

  // A hung window must fail the job instead of blocking CI until its timeout.
  thread::spawn(|| {
    thread::sleep(Duration::from_secs(120));
    eprintln!("[smoke] watchdog: the runtime did not exit after 120s");
    std::process::abort();
  });

  let mut failures = Vec::new();
  let config = WindowConfig {
    title: "smoke".into(),
    width: 400.0,
    height: 300.0,
    visible: true,
    decorations: true,
    ..Default::default()
  };

  let pending = match PendingWindow::<(), ElectronRuntime>::new(
    <ElectronWindowBuilder as tauri_runtime::window::WindowBuilder>::with_config(&config),
    "smoke",
  ) {
    Ok(pending) => pending,
    Err(error) => {
      eprintln!("[smoke] cannot build the window: {error}");
      return ExitCode::FAILURE;
    }
  };
  // A fn pointer is higher-ranked over the handle lifetime, so it satisfies `Fn(RawWindow)`.
  let after: Option<fn(RawWindow<'_>)> = None;
  let window = match <ElectronRuntimeHandle as RuntimeHandle<()>>::create_window(
    &handle,
    pending,
    after,
  ) {
    Ok(window) => window.dispatcher,
    Err(error) => {
      eprintln!("[smoke] cannot create the window: {error}");
      return ExitCode::FAILURE;
    }
  };

  window_checks(&window, &handle, &mut failures);

  // Closing has to reach the event loop, otherwise the process outlives the test.
  set(&mut failures, "close", || WindowDispatch::<()>::close(&window));
  expect_named(&mut failures, "closed event", "destroyed", || {
    REPORT.lock().unwrap().clone()
  });
  match exit_rx.recv_timeout(Duration::from_secs(20)) {
    Ok(()) => {
      let _ = loop_thread.join();
    }
    Err(_) => failures.push("the event loop did not exit after the last window closed".into()),
  }

  if failures.is_empty() {
    println!("\n[smoke] {} checks passed", CHECKS.load(Ordering::Relaxed));
    ExitCode::SUCCESS
  } else {
    eprintln!("\n[smoke] {} failed:", failures.len());
    for failure in &failures {
      eprintln!("  - {failure}");
    }
    ExitCode::FAILURE
  }
}

fn window_checks(window: &Window, handle: &Handle, failures: &mut Vec<String>) {
  title_checks(window, failures);
  geometry_checks(window, failures);
  state_checks(window, failures);
  visibility_checks(window, failures);
  cursor_checks(window, handle, failures);
}

fn title_checks(window: &Window, failures: &mut Vec<String>) {
  set(failures, "set_title", || {
    WindowDispatch::<()>::set_title(window, "smoke-1")
  });
  expect(failures, "title", "smoke-1".to_string(), || {
    WindowDispatch::<()>::title(window)
  });
}

fn geometry_checks(window: &Window, failures: &mut Vec<String>) {
  set(failures, "set_size", || {
    WindowDispatch::<()>::set_size(window, Size::Physical(PhysicalSize::new(420, 320)))
  });

  set(failures, "set_position", || {
    WindowDispatch::<()>::set_position(
      window,
      Position::Physical(PhysicalPosition::new(160, 140)),
    )
  });
  // Window managers may clamp the position to the work area, so only require
  // that the request arrives instead of landing exactly.
  match WindowDispatch::<()>::outer_position(window) {
    Ok(position) if position.x != 0 || position.y != 0 => {
      println!("ok   outer_position {position:?}")
    }
    Ok(position) => failures.push(format!("outer_position did not move: {position:?}")),
    Err(error) => failures.push(format!("outer_position: {error}")),
  }

  // `set_size` sets the content size, so the content is what has to arrive;
  // the frame around it makes `outer_size` larger by an OS-specific amount.
  expect_size(failures, "inner_size", PhysicalSize::new(420, 320), 8, || {
    WindowDispatch::<()>::inner_size(window)
  });
  match (
    WindowDispatch::<()>::inner_size(window),
    WindowDispatch::<()>::outer_size(window),
  ) {
    (Ok(inner), Ok(outer)) if outer.width >= inner.width && outer.height >= inner.height => {
      println!("ok   outer_size {outer:?} contains {inner:?}")
    }
    (Ok(inner), Ok(outer)) => failures.push(format!("outer_size {outer:?} is smaller than {inner:?}")),
    (_, Err(error)) => failures.push(format!("outer_size: {error}")),
    (Err(error), _) => failures.push(format!("inner_size: {error}")),
  }
}

fn state_checks(window: &Window, failures: &mut Vec<String>) {
  set(failures, "set_resizable", || {
    WindowDispatch::<()>::set_resizable(window, false)
  });
  expect(failures, "is_resizable", false, || {
    WindowDispatch::<()>::is_resizable(window)
  });
  set(failures, "set_resizable back", || {
    WindowDispatch::<()>::set_resizable(window, true)
  });
  expect(failures, "is_resizable back", true, || {
    WindowDispatch::<()>::is_resizable(window)
  });

  set(failures, "set_always_on_top", || {
    WindowDispatch::<()>::set_always_on_top(window, true)
  });
  expect(failures, "is_always_on_top", true, || {
    WindowDispatch::<()>::is_always_on_top(window)
  });
  set(failures, "set_always_on_top off", || {
    WindowDispatch::<()>::set_always_on_top(window, false)
  });
  expect(failures, "is_always_on_top off", false, || {
    WindowDispatch::<()>::is_always_on_top(window)
  });

  set(failures, "set_decorations", || {
    WindowDispatch::<()>::set_decorations(window, false)
  });
  expect(failures, "is_decorated", false, || {
    WindowDispatch::<()>::is_decorated(window)
  });
  set(failures, "set_decorations back", || {
    WindowDispatch::<()>::set_decorations(window, true)
  });
  expect(failures, "is_decorated back", true, || {
    WindowDispatch::<()>::is_decorated(window)
  });

  set(failures, "maximize", || WindowDispatch::<()>::maximize(window));
  expect(failures, "is_maximized", true, || {
    WindowDispatch::<()>::is_maximized(window)
  });
  set(failures, "unmaximize", || {
    WindowDispatch::<()>::unmaximize(window)
  });
  expect(failures, "is_maximized off", false, || {
    WindowDispatch::<()>::is_maximized(window)
  });

  set(failures, "set_maximizable", || {
    WindowDispatch::<()>::set_maximizable(window, false)
  });
  expect(failures, "is_maximizable", false, || {
    WindowDispatch::<()>::is_maximizable(window)
  });
  set(failures, "set_minimizable", || {
    WindowDispatch::<()>::set_minimizable(window, false)
  });
  expect(failures, "is_minimizable", false, || {
    WindowDispatch::<()>::is_minimizable(window)
  });
  set(failures, "set_closable", || {
    WindowDispatch::<()>::set_closable(window, false)
  });
  expect(failures, "is_closable", false, || {
    WindowDispatch::<()>::is_closable(window)
  });
  // Restore, otherwise the window cannot be closed by the checks below.
  set(failures, "set_closable back", || {
    WindowDispatch::<()>::set_closable(window, true)
  });

  set(failures, "set_theme", || {
    WindowDispatch::<()>::set_theme(window, Some(Theme::Dark))
  });
  expect(failures, "theme", Theme::Dark, || {
    WindowDispatch::<()>::theme(window)
  });
}

fn visibility_checks(window: &Window, failures: &mut Vec<String>) {
  set(failures, "hide", || WindowDispatch::<()>::hide(window));
  expect(failures, "is_visible", false, || {
    WindowDispatch::<()>::is_visible(window)
  });
  set(failures, "show", || WindowDispatch::<()>::show(window));
  expect(failures, "is_visible again", true, || {
    WindowDispatch::<()>::is_visible(window)
  });
}

fn cursor_checks(window: &Window, handle: &Handle, failures: &mut Vec<String>) {
  let Ok(start) = <ElectronRuntimeHandle as RuntimeHandle<()>>::cursor_position(handle) else {
    failures.push("cursor_position: cannot read the starting position".into());
    return;
  };
  // Move relative to where the pointer is: this proves the setter and the
  // getter agree without depending on how each side labels screen coordinates.
  let target = PhysicalPosition::new(start.x + 60.0, start.y + 40.0);
  let request = Position::Physical(PhysicalPosition::new(
    target.x.round() as i32,
    target.y.round() as i32,
  ));
  let started = Instant::now();
  loop {
    if let Err(error) = WindowDispatch::<()>::set_cursor_position(window, request) {
      failures.push(format!("set_cursor_position: {error}"));
      return;
    }
    // A resting mouse keeps resending its position, so the warp has to be
    // repeated until a read catches it instead of being applied once.
    match <ElectronRuntimeHandle as RuntimeHandle<()>>::cursor_position(handle) {
      Ok(got) => {
        if (got.x - target.x).abs() <= 2.0 && (got.y - target.y).abs() <= 2.0 {
          CHECKS.fetch_add(1, Ordering::Relaxed);
          println!("ok   cursor_position {got:?} from {start:?} in {:?}", started.elapsed());
          return;
        }
        if started.elapsed() > SETTLE {
          failures.push(format!("cursor_position: got {got:?}, want {target:?}"));
          return;
        }
      }
      Err(error) => {
        failures.push(format!("cursor_position: {error}"));
        return;
      }
    }
    thread::sleep(Duration::from_millis(25));
  }
}

/// Records a setter failure; setters have no return value to compare.
fn set(failures: &mut Vec<String>, name: &str, call: impl FnOnce() -> tauri_runtime::Result<()>) {
  match call() {
    Ok(()) => {
      CHECKS.fetch_add(1, Ordering::Relaxed);
      println!("ok   {name}")
    }
    Err(error) => failures.push(format!("{name}: {error}")),
  }
}

/// Retries a getter until it reports the value the setter asked for.
fn expect<T: PartialEq + std::fmt::Debug>(
  failures: &mut Vec<String>,
  name: &str,
  want: T,
  mut read: impl FnMut() -> tauri_runtime::Result<T>,
) {
  let started = Instant::now();
  loop {
    match read() {
      Ok(got) if got == want => {
        CHECKS.fetch_add(1, Ordering::Relaxed);
        println!("ok   {name} ({:.0}ms)", started.elapsed().as_millis());
        return;
      }
      Ok(got) => {
        if started.elapsed() > SETTLE {
          failures.push(format!("{name}: got {got:?}, want {want:?}"));
          return;
        }
      }
      Err(error) => {
        if started.elapsed() > SETTLE {
          failures.push(format!("{name}: {error}"));
          return;
        }
      }
    }
    thread::sleep(Duration::from_millis(25));
  }
}

/// Retries a size getter, allowing `tolerance` physical pixels of rounding.
/// Electron converts to DIPs and back, and 1.5x scaling cannot round-trip exactly.
fn expect_size(
  failures: &mut Vec<String>,
  name: &str,
  want: PhysicalSize<u32>,
  tolerance: u32,
  mut read: impl FnMut() -> tauri_runtime::Result<PhysicalSize<u32>>,
) {
  expect(
    failures,
    name,
    want,
    move || {
      read().map(|got| {
        let fits = got.width.abs_diff(want.width) <= tolerance
          && got.height.abs_diff(want.height) <= tolerance;
        if fits { want } else { got }
      })
    },
  );
}

/// Requires the observed window events to contain `want`.
fn expect_named(
  failures: &mut Vec<String>,
  name: &str,
  want: &str,
  read: impl Fn() -> Vec<String>,
) {
  let started = Instant::now();
  loop {
    if read().iter().any(|event| event == want) {
      CHECKS.fetch_add(1, Ordering::Relaxed);
      println!("ok   {name}");
      return;
    }
    if started.elapsed() > SETTLE {
      let seen = read();
      failures.push(format!("{name}: saw {seen:?}"));
      return;
    }
    thread::sleep(Duration::from_millis(25));
  }
}

static CHECKS: AtomicUsize = AtomicUsize::new(0);
static REPORT: Mutex<Vec<String>> = Mutex::new(Vec::new());
