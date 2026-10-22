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
  fmt::Debug,
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
  RunEvent, Runtime, RuntimeHandle, RuntimeInitArgs, WebviewDispatch, WindowDispatch,
  dpi::{PhysicalPosition, PhysicalSize, Position, Size},
  webview::{PendingWebview, WebviewAttributes},
  window::{CursorIcon, DetachedWindow, PendingWindow, RawWindow, WindowEvent},
};
use tauri_runtime_electron::{ElectronRuntime, ElectronRuntimeHandle, ElectronWindowBuilder};
use tauri_utils::{
  Theme,
  config::{WebviewUrl, WindowConfig},
};

/// How long a setter may take to become visible to the getter. Every window
/// manager is asynchronous, so an immediate read would flake; a silent no-op
/// still fails, just after this window instead of instantly.
const SETTLE: Duration = Duration::from_secs(5);

/// The concrete dispatcher types, with the event type pinned to `()`.
 type Window = <ElectronRuntime as Runtime<()>>::WindowDispatcher;
 type Webview = <ElectronRuntime as Runtime<()>>::WebviewDispatcher;
 type Handle = <ElectronRuntime as Runtime<()>>::Handle;

/// The first page the window loads.
const PAGE: &str = "data:text/html,<title>smoke-page</title><p>smoke</p>";

/// The page `navigate` sends the window to; its title proves the load happened.
const SECOND_PAGE: &str = "data:text/html,<title>smoke-second</title><p>second</p>";

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
    always_on_top: true,
    // Creation-time only: nothing can set the minimum size afterwards.
    min_width: Some(250.0),
    min_height: Some(180.0),
    ..Default::default()
  };

  let mut pending = match PendingWindow::<(), ElectronRuntime>::new(
    <ElectronWindowBuilder as tauri_runtime::window::WindowBuilder>::with_config(&config),
    "smoke",
  ) {
    Ok(pending) => pending,
    Err(error) => {
      eprintln!("[smoke] cannot build the window: {error}");
      return ExitCode::FAILURE;
    }
  };
  match PendingWebview::<(), ElectronRuntime>::new(
    WebviewAttributes::new(WebviewUrl::App("index.html".into())),
    "smoke-page",
  ) {
    Ok(mut webview) => {
      // The window loads this page itself, so the checks below can eval against
      // a document that really exists.
      webview.url = PAGE.to_string();
      // The page posts through `window.ipc.postMessage`, which is the bridge
      // Tauri's own IPC script uses; the handler is the other end of it.
      webview.ipc_handler = Some(Box::new(|_webview, request| {
        IPC.lock().unwrap().push(request.body().clone());
      }));
      pending.set_webview(webview);
    }
    Err(error) => {
      eprintln!("[smoke] cannot build the webview: {error}");
      return ExitCode::FAILURE;
    }
  }
  // A fn pointer is higher-ranked over the handle lifetime, so it satisfies `Fn(RawWindow)`.
  let after: Option<fn(RawWindow<'_>)> = None;
  let detached = match <ElectronRuntimeHandle as RuntimeHandle<()>>::create_window(
    &handle, pending, after,
  ) {
    Ok(window) => window,
    Err(error) => {
      eprintln!("[smoke] cannot create the window: {error}");
      return ExitCode::FAILURE;
    }
  };
  let DetachedWindow {
    dispatcher: window,
    webview,
    ..
  } = detached;
  let webview = webview.map(|detached| detached.webview.dispatcher);

  window_checks(&window, &handle, &mut failures);
  webview_checks(webview.as_ref(), &mut failures);
  if let Some(webview) = webview.as_ref() {
    cursor_style_checks(&window, webview, &mut failures);
  }

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
  // A scale factor of zero would make every physical conversion divide by zero.
  match WindowDispatch::<()>::scale_factor(window) {
    Ok(scale) if scale > 0.0 && scale.is_finite() => println!("ok   scale_factor {scale}"),
    Ok(scale) => failures.push(format!("scale_factor is not usable: {scale}")),
    Err(error) => failures.push(format!("scale_factor: {error}")),
  }
  match WindowDispatch::<()>::current_monitor(window) {
    Ok(Some(monitor)) => println!("ok   current_monitor {:?}", monitor.name),
    Ok(None) => failures.push("current_monitor: the window sits on no monitor".into()),
    Err(error) => failures.push(format!("current_monitor: {error}")),
  }
  match WindowDispatch::<()>::available_monitors(window) {
    Ok(monitors) if !monitors.is_empty() => println!("ok   available_monitors {}", monitors.len()),
    Ok(_) => failures.push("available_monitors is empty".into()),
    Err(error) => failures.push(format!("available_monitors: {error}")),
  }

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

  // The minimum size came from the creation config; a window smaller than it
  // means the runtime dropped that field on the way to Electron. Electron
  // applies it to the frame, so `outer_size` is what has to hold. Last, because
  // it leaves the window at the minimum.
  let scale = WindowDispatch::<()>::scale_factor(window).unwrap_or(1.0);
  let floor = PhysicalSize::new((250.0 * scale) as u32, (180.0 * scale) as u32);
  set(failures, "set_size below the minimum", || {
    WindowDispatch::<()>::set_size(window, Size::Physical(PhysicalSize::new(120, 100)))
  });
  expect_floor(failures, "minimum size holds", floor, || {
    WindowDispatch::<()>::outer_size(window)
  });
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

  expect_applied(
    failures,
    "is_enabled",
    false,
    || WindowDispatch::<()>::set_enabled(window, false),
    || WindowDispatch::<()>::is_enabled(window),
  );
  expect_applied(
    failures,
    "is_enabled back",
    true,
    || WindowDispatch::<()>::set_enabled(window, true),
    || WindowDispatch::<()>::is_enabled(window),
  );
  // Focus and minimization both animate, so they are polled like the rest.
  expect_applied(
    failures,
    "is_focused",
    true,
    || WindowDispatch::<()>::set_focus(window),
    || WindowDispatch::<()>::is_focused(window),
  );

  // The window asks for always-on-top while it is created, which is how an app
  // usually does it; the setter then has to turn it off and back on.
  expect(failures, "is_always_on_top at creation", true, || {
    WindowDispatch::<()>::is_always_on_top(window)
  });
  expect_applied(
    failures,
    "is_always_on_top",
    true,
    || WindowDispatch::<()>::set_always_on_top(window, true),
    || WindowDispatch::<()>::is_always_on_top(window),
  );
  expect_applied(
    failures,
    "is_always_on_top off",
    false,
    || WindowDispatch::<()>::set_always_on_top(window, false),
    || WindowDispatch::<()>::is_always_on_top(window),
  );

  // Electron can only change the frame through Win32, and it documents the
  // three "izable" getters for macOS and Windows only.
  if cfg!(windows) {
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
  } else {
    println!("skip decorations: Electron cannot change the frame after creation");
  }

  expect_applied(
    failures,
    "is_maximized",
    true,
    || WindowDispatch::<()>::maximize(window),
    || WindowDispatch::<()>::is_maximized(window),
  );
  expect_applied(
    failures,
    "is_maximized off",
    false,
    || WindowDispatch::<()>::unmaximize(window),
    || WindowDispatch::<()>::is_maximized(window),
  );

  if cfg!(any(windows, target_os = "macos")) {
    // Electron's macOS zoom button is enabled from the window style, not from
    // what set_maximizable asked for (native_window_mac.mm: UpdateZoomButton
    // only looks at the resizable mask), so only Windows can report it back.
    if cfg!(windows) {
      set(failures, "set_maximizable", || {
        WindowDispatch::<()>::set_maximizable(window, false)
      });
      expect(failures, "is_maximizable", false, || {
        WindowDispatch::<()>::is_maximizable(window)
      });
    } else {
      println!("skip is_maximizable: Electron derives the macOS zoom button from the resizable style");
    }
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
  } else {
    println!("skip maximizable/minimizable/closable: macOS and Windows only");
  }

  set(failures, "set_theme", || {
    WindowDispatch::<()>::set_theme(window, Some(Theme::Dark))
  });
  expect(failures, "theme", Theme::Dark, || {
    WindowDispatch::<()>::theme(window)
  });

  // Minimization runs last: on macOS it re-derives the window style, and the
  // getters above read that style.
  expect_applied(
    failures,
    "is_minimized",
    true,
    || WindowDispatch::<()>::minimize(window),
    || WindowDispatch::<()>::is_minimized(window),
  );
  expect_applied(
    failures,
    "is_minimized back",
    false,
    || WindowDispatch::<()>::unminimize(window),
    || WindowDispatch::<()>::is_minimized(window),
  );
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

/// The cursor icon and visibility show up on the page, so the page reports them.
fn cursor_style_checks(window: &Window, webview: &Webview, failures: &mut Vec<String>) {
  set(failures, "set_cursor_icon", || {
    WindowDispatch::<()>::set_cursor_icon(window, CursorIcon::Hand)
  });
  expect_eval(
    failures,
    "cursor icon",
    "pointer",
    webview,
    "getComputedStyle(document.documentElement).cursor",
  );
  set(failures, "set_cursor_visible", || {
    WindowDispatch::<()>::set_cursor_visible(window, false)
  });
  expect_eval(
    failures,
    "cursor hidden",
    "none",
    webview,
    "getComputedStyle(document.documentElement).cursor",
  );
  set(failures, "set_cursor_visible back", || {
    WindowDispatch::<()>::set_cursor_visible(window, true)
  });
  expect_eval(
    failures,
    "cursor shown",
    "default",
    webview,
    "getComputedStyle(document.documentElement).cursor",
  );
}
fn webview_checks(webview: Option<&Webview>, failures: &mut Vec<String>) {
  let Some(webview) = webview else {
    failures.push("webview: the window was created without its webview".into());
    return;
  };

  expect_eval(failures, "eval_script", "2", webview, "1 + 1");
  expect_eval(
    failures,
    "loaded page",
    "smoke-page",
    webview,
    "document.title",
  );

  expect(failures, "url", PAGE.to_string(), || {
    WebviewDispatch::<()>::url(webview)
  });
  set(failures, "navigate", || {
    WebviewDispatch::<()>::navigate(webview, SECOND_PAGE.parse().unwrap())
  });
  expect(failures, "url after navigate", SECOND_PAGE.to_string(), || {
    WebviewDispatch::<()>::url(webview)
  });
  // The mirrored url above could be a lie; this reads the document instead.
  expect_eval(failures, "navigated", "smoke-second", webview, "document.title");

  expect_applied(
    failures,
    "devtools open",
    true,
    || {
      WebviewDispatch::<()>::open_devtools(webview);
      Ok(())
    },
    || WebviewDispatch::<()>::is_devtools_open(webview),
  );
  WebviewDispatch::<()>::close_devtools(webview);
  expect(failures, "devtools closed", false, || {
    WebviewDispatch::<()>::is_devtools_open(webview)
  });

  // A zoom factor is a style/paint decision, invisible to every getter of this
  // trait, so the page itself reports it, relative to the display scale.
  let scale = eval_once(webview, "window.devicePixelRatio")
    .and_then(|value| value.trim_matches('"').parse::<f64>().ok())
    .unwrap_or(1.0);
  set(failures, "set_zoom", || {
    WebviewDispatch::<()>::set_zoom(webview, 2.0)
  });
  expect_eval(
    failures,
    "zoom",
    &format!("{}", scale * 2.0),
    webview,
    "String(window.devicePixelRatio)",
  );
  set(failures, "set_zoom back", || {
    WebviewDispatch::<()>::set_zoom(webview, 1.0)
  });
  expect_eval(
    failures,
    "zoom back",
    &format!("{scale}"),
    webview,
    "String(window.devicePixelRatio)",
  );

  // The page talks to the runtime the way a Tauri app's frontend does.
  let _ = eval_once(webview, "window.ipc.postMessage('ping')");
  let started = Instant::now();
  loop {
    if IPC.lock().is_ok_and(|messages| messages.iter().any(|body| body == "ping")) {
      CHECKS.fetch_add(1, Ordering::Relaxed);
      println!("ok   ipc round trip ping in {:?}", started.elapsed());
      return;
    }
    if started.elapsed() > SETTLE {
      failures.push("ipc round trip: the page's postMessage never reached the handler".into());
      return;
    }
    thread::sleep(Duration::from_millis(25));
  }
}

/// Evaluates `script` once, for a value another check has to compare against.
fn eval_once(webview: &Webview, script: &str) -> Option<String> {
  let (tx, rx) = channel::<String>();
  WebviewDispatch::<()>::eval_script_with_callback(webview, script, move |value| {
    let _ = tx.send(value);
  })
  .ok()?;
  rx.recv_timeout(Duration::from_secs(2)).ok()
}

/// Evaluates `script` in the page until it returns `want`, so a page that is
/// still loading does not fail the check.
fn expect_eval(
  failures: &mut Vec<String>,
  name: &str,
  want: &str,
  webview: &Webview,
  script: &str,
) {
  CHECKS.fetch_add(1, Ordering::Relaxed);
  let start = Instant::now();
  let mut last = String::from("<no result>");
  let (tx, rx) = channel::<String>();
  while start.elapsed() < SETTLE {
    let tx = tx.clone();
    let called = WebviewDispatch::<()>::eval_script_with_callback(webview, script, move |value| {
      let _ = tx.send(value);
    });
    if let Err(error) = called {
      failures.push(format!("{name}: {error}"));
      return;
    }
    if let Ok(value) = rx.recv_timeout(Duration::from_millis(500)) {
      // The callback stringifies JSON, so a text result keeps its quotes.
      last = value.trim_matches('"').to_string();
      if last == want {
        println!("ok   {name} `{script}` = {last} in {:?}", start.elapsed());
        return;
      }
    }
    thread::sleep(Duration::from_millis(250));
  }
  failures.push(format!("{name}: `{script}` returned {last:?}, want {want:?}"));
}

/// Repeats a setter until its getter agrees, for state Electron only applies
/// once the window has finished being shown.
fn expect_applied<T: PartialEq + Debug>(
  failures: &mut Vec<String>,
  name: &str,
  want: T,
  mut apply: impl FnMut() -> tauri_runtime::Result<()>,
  mut read: impl FnMut() -> tauri_runtime::Result<T>,
) {
  let started = Instant::now();
  loop {
    let applied = apply();
    let got = read();
    let settled = matches!(&got, Ok(got) if *got == want) || started.elapsed() >= SETTLE;
    if settled {
      match (applied, got) {
        (Err(error), _) => failures.push(format!("{name}: {error}")),
        (_, Ok(got)) if got == want => {
          CHECKS.fetch_add(1, Ordering::Relaxed);
          println!("ok   {name} {got:?} in {:?}", started.elapsed());
        }
        (_, Ok(got)) => failures.push(format!("{name}: got {got:?}, want {want:?}")),
        (_, Err(error)) => failures.push(format!("{name}: {error}")),
      }
      return;
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

/// Retries a getter until the size is at least `floor` in both dimensions.
fn expect_floor(
  failures: &mut Vec<String>,
  name: &str,
  floor: PhysicalSize<u32>,
  mut read: impl FnMut() -> tauri_runtime::Result<PhysicalSize<u32>>,
) {
  let started = Instant::now();
  loop {
    match read() {
      Ok(got) if got.width >= floor.width && got.height >= floor.height => {
        CHECKS.fetch_add(1, Ordering::Relaxed);
        println!("ok   {name} {:?} in {:?}", got, started.elapsed());
        return;
      }
      Ok(got) if started.elapsed() > SETTLE => {
        failures.push(format!("{name}: got {got:?}, want at least {floor:?}"));
        return;
      }
      Err(error) if started.elapsed() > SETTLE => {
        failures.push(format!("{name}: {error}"));
        return;
      }
      _ => thread::sleep(Duration::from_millis(25)),
    }
  }
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

/// What the page posted through the IPC bridge, as the runtime received it.
static IPC: Mutex<Vec<String>> = Mutex::new(Vec::new());
