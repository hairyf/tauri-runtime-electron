//! Launches the real Electron runtime and checks a named desktop subset:
//! set a value, read it back, compare, and observe events and page state.
//!
//! These checks catch no-ops on their exercised paths. Platform skips are
//! reported; native effects and the complete official examples need more evidence.
//!
//! `cargo run --bin smoke` needs a display; CI wraps it in `xvfb-run` on Linux.
//!
//! Every dispatcher call is spelled `WindowDispatch::<()>::method(window, ..)`
//! because the crate implements the traits for every `T: UserEvent`; inside a
//! non-generic test there is no `T` to infer, so each call has to pin it.

use std::{
  fmt::Debug,
  path::Path,
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
const PAGE: &str = "tauri://localhost/index.html";

/// The page `navigate` sends the window to; its title proves the load happened.
const SECOND_PAGE: &str = "tauri://localhost/second.html";

fn main() -> ExitCode {
  let runtime = match <ElectronRuntime as Runtime<()>>::new(RuntimeInitArgs::default()) {
    Ok(runtime) => runtime,
    Err(error) => {
      eprintln!("[smoke] cannot start Electron: {error}");
      return ExitCode::FAILURE;
    }
  };

  let handle: Handle = <ElectronRuntime as Runtime<()>>::handle(&runtime);
  let (exit_tx, exit_rx) = channel::<()>();

  // A hung window must fail the job instead of blocking CI until its timeout.
  thread::spawn(|| {
    thread::sleep(Duration::from_secs(120));
    eprintln!("[smoke] watchdog: the runtime did not exit after 120s");
    std::process::abort();
  });

  // The runtime's UI state belongs to its creating thread. Checks use the
  // handle from a worker so synchronous dispatch can be serviced by this loop.
  let checks = thread::spawn(move || {
    let status = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
      run_checks(&handle, exit_rx)
    })).unwrap_or(ExitCode::FAILURE);
    // Setup can fail before any window exists to trigger automatic exit.
    let code = if status == ExitCode::SUCCESS { 0 } else { 1 };
    let _ = <ElectronRuntimeHandle as RuntimeHandle<()>>::request_exit(&handle, code);
    status
  });
  let code = <ElectronRuntime as Runtime<()>>::run_return(runtime, move |event| {
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
  match checks.join() {
    Ok(status) if code == 0 => status,
    Ok(_) => {
      eprintln!("[smoke] runtime exited with code {code}");
      ExitCode::FAILURE
    }
    Err(_) => {
      eprintln!("[smoke] checks panicked");
      ExitCode::FAILURE
    }
  }
}

fn run_checks(handle: &Handle, exit_rx: std::sync::mpsc::Receiver<()>) -> ExitCode {
  let mut failures = Vec::new();
  let session_root = std::env::temp_dir().join(format!(
    "tauri-runtime-electron-smoke-{}-{}",
    std::process::id(),
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos(),
  ));
  let session_a = session_root.join("a");
  let session_b = session_root.join("b");
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

  // The size travels through the builder *setter*, not through the config:
  // Tauri's own `inner_size` does the same, and a setter that forgets to store
  // the value is a window that comes up the wrong size.
  let builder = <ElectronWindowBuilder as tauri_runtime::window::WindowBuilder>::with_config(&config);
  let builder =
    <ElectronWindowBuilder as tauri_runtime::window::WindowBuilder>::inner_size(builder, 320.0, 240.0);
  let mut pending = match PendingWindow::<(), ElectronRuntime>::new(builder, "smoke") {
    Ok(pending) => pending,
    Err(error) => {
      eprintln!("[smoke] cannot build the window: {error}");
      return ExitCode::FAILURE;
    }
  };
  match PendingWebview::<(), ElectronRuntime>::new(
    WebviewAttributes::new(WebviewUrl::App("index.html".into()))
      .data_directory(session_a.clone())
      .initialization_script("window.__smokeMain = 'main'")
      .initialization_script_on_all_frames("window.__smokeAll = 'all'"),
    "smoke-page",
  ) {
    Ok(mut webview) => {
      // The window loads this page itself, so the checks below can eval against
      // a document that really exists.
      webview.url = PAGE.to_string();
      webview.register_uri_scheme_protocol("tauri", |_, request, respond| {
        let title = match request.uri().path() {
          "/second.html" => "smoke-second",
          "/blocked.html" => "smoke-blocked",
          "/sibling.html" => "smoke-sibling",
          _ => "smoke-page",
        };
        let frame = if request.uri().path() == "/index.html" {
          "<iframe id='smoke-frame' src='/frame.html'></iframe>"
        } else {
          ""
        };
        let response = http::Response::builder()
          .header(http::header::CONTENT_TYPE, "text/html")
          .body(std::borrow::Cow::Owned(format!("<title>{title}</title><p>smoke</p>{frame}").into_bytes()))
          .unwrap();
        if request.uri().path() == "/sibling.html" {
          // Keep an unrelated frame loading while the other frame is vetoed:
          // stopping the whole WebContents must fail this allowed navigation.
          thread::spawn(move || {
            thread::sleep(Duration::from_millis(300));
            respond(response);
          });
        } else {
          respond(response);
        }
      });
      // The page posts through `window.ipc.postMessage`, which is the bridge
      // Tauri's own IPC script uses; the handler is the other end of it.
      webview.ipc_handler = Some(Box::new(|_webview, request| {
        IPC.lock().unwrap().push(request.body().clone());
      }));
      webview.navigation_handler = Some(Box::new(|url| {
        NAVIGATIONS.lock().unwrap().push(url.to_string());
        url.path() != "/blocked.html"
      }));
      webview.on_page_load_handler = Some(Box::new(|_url, event| {
        LOADS.lock().unwrap().push(format!("{event:?}"));
      }));
      webview.document_title_changed_handler =
        Some(Box::new(|title| TITLES.lock().unwrap().push(title)));
      pending.set_webview(webview);
    }
    Err(error) => {
      eprintln!("[smoke] cannot build the webview: {error}");
      return ExitCode::FAILURE;
    }
  }
  // A fn pointer is higher-ranked over the handle lifetime, so it satisfies `Fn(RawWindow)`.
  #[cfg(windows)]
  let after: Option<fn(RawWindow<'_>)> = Some(record_window_creation);
  #[cfg(not(windows))]
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

  #[cfg(windows)]
  expect(&mut failures, "creation callback once", 1, || {
    Ok(CREATIONS.load(Ordering::SeqCst))
  });
  #[cfg(windows)]
  {
    CHECKS.fetch_add(1, Ordering::Relaxed);
    match WindowDispatch::<()>::window_handle(&window).map(|handle| handle.as_raw()) {
      Ok(raw_window_handle::RawWindowHandle::Win32(raw))
        if *CREATED_HWND.lock().unwrap() == Some(raw.hwnd.get()) =>
      {
        println!("ok   creation callback HWND matches window handle");
      }
      other => failures.push(format!(
        "creation callback HWND {:?} does not match {other:?}",
        *CREATED_HWND.lock().unwrap(),
      )),
    }
  }

  native_access_checks(&window, &mut failures);
  window_checks(&window, &handle, &mut failures);
  if let Some(webview) = webview.as_ref() {
    session_checks(&window, webview, &session_a, &session_b, &mut failures);
  }
  webview_checks(webview.as_ref(), &mut failures);
  if let Some(webview) = webview.as_ref() {
    cursor_style_checks(&window, webview, &mut failures);
  }

  // An idle app must stay connected: the handshake read used to leave a timeout
  // on the socket, and a minute of quiet then looked like a dead Electron.
  thread::sleep(Duration::from_secs(61));
  set(&mut failures, "still connected after a minute", || {
    WindowDispatch::<()>::set_title(&window, "smoke-idle")
  });

  // Closing has to reach the event loop, otherwise the process outlives the test.
  set(&mut failures, "close", || WindowDispatch::<()>::close(&window));
  expect_named(&mut failures, "closed event", "destroyed", || {
    REPORT.lock().unwrap().clone()
  });
  if exit_rx.recv_timeout(Duration::from_secs(20)).is_err() {
    failures.push("the event loop did not exit after the last window closed".into());
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

fn native_access_checks(_window: &Window, _failures: &mut Vec<String>) {
  #[cfg(target_os = "macos")]
  expect(_failures, "AppKit handle unavailable", true, || {
    Ok(matches!(
      WindowDispatch::<()>::window_handle(_window),
      Err(raw_window_handle::HandleError::Unavailable)
    ))
  });
  #[cfg(any(
    target_os = "linux", target_os = "dragonfly", target_os = "freebsd",
    target_os = "netbsd", target_os = "openbsd"
  ))]
  {
    expect(_failures, "GTK window unavailable", true, || {
      Ok(WindowDispatch::<()>::gtk_window(_window).is_err())
    });
    expect(_failures, "GTK widget tree unavailable", true, || {
      Ok(WindowDispatch::<()>::default_vbox(_window).is_err())
    });
  }
}

fn session_checks(
  window: &Window,
  webview: &Webview,
  session_a: &Path,
  session_b: &Path,
  failures: &mut Vec<String>,
) {
  expect_eval(failures, "session seed", "a", webview,
    "localStorage.setItem('smoke-session', 'a'); localStorage.getItem('smoke-session')");
  for (label, directory, want) in [
    ("same-session", session_a, "a"),
    ("other-session", session_b, "empty"),
  ] {
    let mut pending = PendingWebview::<(), ElectronRuntime>::new(
      WebviewAttributes::new(WebviewUrl::App("index.html".into()))
        .data_directory(directory.to_path_buf()),
      label,
    ).unwrap();
    pending.url = PAGE.to_string();
    pending.navigation_handler = Some(Box::new(move |url| {
      NAVIGATIONS.lock().unwrap().push(format!("{label}:{}", url));
      true
    }));
    match WindowDispatch::<()>::create_webview(&mut window.clone(), pending) {
      Ok(extra) => {
        expect_eval(failures, label, want, &extra.dispatcher,
          "localStorage.getItem('smoke-session') ?? 'empty'");
        set(failures, "navigate session webview", || {
          WebviewDispatch::<()>::navigate(&extra.dispatcher, SECOND_PAGE.parse().unwrap())
        });
        expect_eval(failures, "session webview navigated", "smoke-second", &extra.dispatcher,
          "document.title");
        expect_named(failures, "session navigation callback", &format!("{label}:{SECOND_PAGE}"), || {
          NAVIGATIONS.lock().unwrap().clone()
        });
        set(failures, "close session webview", || {
          WebviewDispatch::<()>::close(&extra.dispatcher)
        });
      }
      Err(error) => failures.push(format!("{label}: {error}")),
    }
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

  // The creation size travelled through a builder setter, the way Tauri's own
  // `inner_size` does; a runtime that drops it comes up at Electron's default.
  let scale = WindowDispatch::<()>::scale_factor(window).unwrap_or(1.0);
  expect_size(
    failures,
    "inner_size from the builder",
    PhysicalSize::new(
      (320.0 * scale).round() as u32,
      (240.0 * scale).round() as u32,
    ),
    10,
    || WindowDispatch::<()>::inner_size(window),
  );

  // Stay above the 250x180 DIP minimum on high-DPI screens while exercising
  // the physical-size conversion rather than asking the OS to clamp our target.
  let resized = PhysicalSize::new((420.0 * scale).round() as u32, (320.0 * scale).round() as u32);
  set(failures, "set_size", || {
    WindowDispatch::<()>::set_size(window, Size::Physical(resized))
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
  expect_size(failures, "inner_size", resized, 8, || {
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
  if !cfg!(any(windows, target_os = "macos")) {
    // Warping needs a native call the runtime can make: Win32 on Windows,
    // CoreGraphics on macOS. A Linux window has neither.
    println!("skip cursor_position: only Windows and macOS can warp the cursor");
    return;
  }
  let Ok(start) = <ElectronRuntimeHandle as RuntimeHandle<()>>::cursor_position(handle) else {
    failures.push("cursor_position: cannot read the starting position".into());
    return;
  };
  let monitor = match WindowDispatch::<()>::current_monitor(window) {
    Ok(Some(monitor)) => monitor,
    other => {
      failures.push(format!("cursor_position: cannot locate the window's monitor: {other:?}"));
      return;
    }
  };
  // A relative move can leave the monitor or land in a multi-display gap, where
  // the OS clamps it. Pick an interior point on the window's actual monitor.
  let area = monitor.work_area;
  let mut target = PhysicalPosition::new(
    f64::from(area.position.x) + f64::from(area.size.width) / 2.0,
    f64::from(area.position.y) + f64::from(area.size.height) / 2.0,
  );
  if (target.x - start.x).abs() <= 2.0 && (target.y - start.y).abs() <= 2.0 {
    target.x = f64::from(area.position.x) + f64::from(area.size.width) / 4.0;
  }
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
    "pointer",
    webview,
    "getComputedStyle(document.documentElement).cursor",
  );
}
fn webview_checks(webview: Option<&Webview>, failures: &mut Vec<String>) {
  let Some(webview) = webview else {
    failures.push("webview: the window was created without its webview".into());
    return;
  };

  let called = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
  let invoked = called.clone();
  let result = WebviewDispatch::<()>::with_webview(webview, move |_| {
    invoked.store(true, Ordering::SeqCst);
  });
  expect(failures, "native webview unavailable without callback", true, || {
    Ok(result.is_err() && !called.load(Ordering::SeqCst))
  });
  expect_eval(failures, "main-frame initialization", "main/all", webview,
    "window.__smokeMain + '/' + window.__smokeAll");
  expect_eval(failures, "subframe initialization scope", "undefined/all/undefined", webview,
    "(() => { const frame = document.querySelector('#smoke-frame')?.contentWindow; return typeof frame?.__smokeMain + '/' + frame?.__smokeAll + '/' + typeof frame?.ipc })()");
  expect_eval(failures, "page has no Node globals", "undefined/undefined/undefined", webview,
    "typeof window.require + '/' + typeof window.process + '/' + typeof window.Buffer");
  expect_eval(failures, "subframe has no Node globals", "undefined/undefined/undefined", webview,
    "(() => { const frame = document.querySelector('#smoke-frame')?.contentWindow; return typeof frame?.require + '/' + typeof frame?.process + '/' + typeof frame?.Buffer })()");
  expect_named(failures, "initial navigation callback", PAGE, || {
    NAVIGATIONS.lock().unwrap().clone()
  });
  expect_named(failures, "subframe navigation callback", "tauri://localhost/frame.html", || {
    NAVIGATIONS.lock().unwrap().clone()
  });

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
  let _ = eval_once(webview,
    "(() => { const sibling = document.createElement('iframe'); sibling.id = 'smoke-sibling'; sibling.src = 'tauri://localhost/sibling.html'; document.body.append(sibling); return 'requested' })()");
  expect_named(failures, "sibling navigation callback", "tauri://localhost/sibling.html", || {
    NAVIGATIONS.lock().unwrap().clone()
  });
  let _ = eval_once(webview,
    "(() => { document.querySelector('#smoke-frame').contentWindow.location.href = 'tauri://localhost/blocked.html?frame=smoke'; return 'requested' })()");
  expect_named(failures, "vetoed subframe navigation callback", "tauri://localhost/blocked.html?frame=smoke", || {
    NAVIGATIONS.lock().unwrap().clone()
  });
  expect_eval(failures, "subframe veto preserves main document", "tauri://localhost/index.html/smoke-page", webview,
    "location.href + '/' + document.title");
  // Electron can replace a cancelled subframe with an inaccessible error page;
  // request veto guarantees the target cannot load, not preservation of old DOM.
  expect_eval(failures, "sibling loads despite subframe veto", "smoke-sibling", webview,
    "document.querySelector('#smoke-sibling').contentDocument?.title");
  expect_eval(failures, "subframe veto blocks target document", "true", webview,
    "(() => { try { return document.querySelector('#smoke-frame').contentDocument?.title !== 'smoke-blocked' } catch { return true } })()");
  let _ = eval_once(webview,
    "(() => { const link = document.createElement('a'); link.href = 'tauri://localhost/blocked.html'; document.body.append(link); link.click(); return 'clicked' })()");
  expect_named(failures, "vetoed navigation callback", "tauri://localhost/blocked.html", || {
    NAVIGATIONS.lock().unwrap().clone()
  });
  expect_eval(failures, "veto keeps current document", "tauri://localhost/index.html/smoke-page", webview,
    "location.href + '/' + document.title");
  set(failures, "navigate", || {
    WebviewDispatch::<()>::navigate(webview, SECOND_PAGE.parse().unwrap())
  });
  expect(failures, "url after navigate", SECOND_PAGE.to_string(), || {
    WebviewDispatch::<()>::url(webview)
  });
  // The mirrored url above could be a lie; this reads the document instead.
  expect_eval(failures, "navigated", "smoke-second", webview, "document.title");
  expect_named(failures, "main navigation callback", SECOND_PAGE, || {
    NAVIGATIONS.lock().unwrap().clone()
  });

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

  // The runtime reports the page's lifecycle to the handlers an app registers.
  expect_named(failures, "page load finished", "Finished", || {
    LOADS.lock().unwrap().clone()
  });
  expect_named(failures, "document title", "smoke-page", || {
    TITLES.lock().unwrap().clone()
  });

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

#[cfg(windows)]
fn record_window_creation(_raw: RawWindow<'_>) {
  CREATIONS.fetch_add(1, Ordering::SeqCst);
  #[cfg(windows)]
  {
    *CREATED_HWND.lock().unwrap() = Some(_raw.hwnd);
  }
}

#[cfg(windows)]
static CREATIONS: AtomicUsize = AtomicUsize::new(0);
#[cfg(windows)]
static CREATED_HWND: Mutex<Option<isize>> = Mutex::new(None);

static CHECKS: AtomicUsize = AtomicUsize::new(0);
static REPORT: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// What the page posted through the IPC bridge, as the runtime received it.
static IPC: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Navigation policy calls observed through Rust, including child views/frames.
static NAVIGATIONS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// The page load events the runtime reported to the handler.
static LOADS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// The document titles the runtime reported to the handler.
static TITLES: Mutex<Vec<String>> = Mutex::new(Vec::new());
