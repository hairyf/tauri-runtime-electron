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
    Ok(status) if code == 0 => {
      if status == ExitCode::SUCCESS {
        evidence(&["Runtime::new", "Runtime::run_return"], "smoke::main",
          "real Electron loaded the custom-protocol page, dispatched window events and returned exit code 0 after the last window closed",
          0, &[]);
      }
      status
    }
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
    // No minimum-size setter is called, so the later clamp checks this config.
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

  // The initial document can replace the window title while it loads. Wait
  // for its existing lifecycle check before exercising the title setter.
  let before = failures.len();
  expect_named(&mut failures, "page load finished", "Finished", || {
    LOADS.lock().unwrap().clone()
  });
  if let Some(webview) = webview.as_ref() {
    expect_eval(&mut failures, "created window document", "smoke-page", webview, "document.title");
  } else {
    failures.push("create_window returned no webview for the requested page".into());
  }
  evidence(&["RuntimeHandle::create_window"], "smoke::run_checks",
    "create_window returned a webview whose real custom-protocol document title is smoke-page and emitted page-load Finished",
    before, &failures);
  native_access_checks(&window, &mut failures);
  new_window_rejection_checks(&window, handle, &mut failures);
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
  let before = failures.len();
  set(&mut failures, "still connected after a minute", || {
    WindowDispatch::<()>::set_title(&window, "smoke-idle")
  });
  expect(&mut failures, "title after idle", "smoke-idle".to_string(), || {
    WindowDispatch::<()>::title(&window)
  });
  evidence(&["WindowDispatch::set_title", "WindowDispatch::title"], "smoke::run_checks",
    "title() == smoke-idle after set_title following 61 seconds of idle IPC", before, &failures);

  // Closing has to reach the event loop, otherwise the process outlives the test.
  let before = failures.len();
  set(&mut failures, "close", || WindowDispatch::<()>::close(&window));
  expect_named(&mut failures, "closed event", "destroyed", || {
    REPORT.lock().unwrap().clone()
  });
  if exit_rx.recv_timeout(Duration::from_secs(20)).is_err() {
    failures.push("the event loop did not exit after the last window closed".into());
  }
  evidence(&["WindowDispatch::close"], "smoke::run_checks",
    "close emitted Destroyed and the last window caused the event loop to request exit", before, &failures);

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

// This is an unsupported-contract regression, not popup support: both public
// creation paths must return the reason without calling a fabricated opener.
fn new_window_rejection_checks(window: &Window, handle: &Handle, failures: &mut Vec<String>) {
  use std::sync::{Arc, atomic::AtomicBool};
  use tauri_runtime::webview::NewWindowResponse;

  let called = Arc::new(AtomicBool::new(false));
  for child in [false, true] {
    let mut webview = PendingWebview::<(), ElectronRuntime>::new(
      WebviewAttributes::new(WebviewUrl::default()), "unsupported-popup",
    ).unwrap();
    let invoked = called.clone();
    webview.new_window_handler = Some(Box::new(move |_, _| {
      invoked.store(true, Ordering::SeqCst);
      NewWindowResponse::Deny
    }));
    let result = if child {
      WindowDispatch::<()>::create_webview(&mut window.clone(), webview).map(|_| ())
    } else {
      let mut pending = PendingWindow::<(), ElectronRuntime>::new(
        <ElectronWindowBuilder as tauri_runtime::window::WindowBuilder>::new(),
        "unsupported-popup-window",
      ).unwrap();
      pending.set_webview(webview);
      let after: Option<fn(RawWindow<'_>)> = None;
      <ElectronRuntimeHandle as RuntimeHandle<()>>::create_window(handle, pending, after).map(|_| ())
    };
    CHECKS.fetch_add(1, Ordering::Relaxed);
    match result {
      Err(error) if error.to_string().contains("new-window callbacks require a native opener")
        && !called.load(Ordering::SeqCst) => {
          println!("ok   new-window handler rejects {} creation explicitly (unsupported, not implemented)",
            if child { "child webview" } else { "window" });
        }
      other => failures.push(format!("new-window handler creation (child={child}): {other:?}")),
    }
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
    let before = failures.len();
    match WindowDispatch::<()>::create_webview(&mut window.clone(), pending) {
      Ok(extra) => {
        expect_eval(failures, label, want, &extra.dispatcher,
          "localStorage.getItem('smoke-session') ?? 'empty'");
        expect_eval(failures, "created child document", "smoke-page", &extra.dispatcher, "document.title");
        set(failures, "navigate session webview", || {
          WebviewDispatch::<()>::navigate(&extra.dispatcher, SECOND_PAGE.parse().unwrap())
        });
        expect_eval(failures, "session webview navigated", "smoke-second", &extra.dispatcher,
          "document.title");
        expect_named(failures, "session navigation callback", &format!("{label}:{SECOND_PAGE}"), || {
          NAVIGATIONS.lock().unwrap().clone()
        });
        evidence(&["WindowDispatch::create_webview", "WebviewDispatch::navigate"], "smoke::session_checks",
          &format!("{label} child loads smoke-page, reads localStorage={want}, then loads smoke-second and emits its own navigation callback"),
          before, failures);
        set(failures, "close session webview", || {
          WebviewDispatch::<()>::close(&extra.dispatcher)
        });
      }
      Err(error) => {
        failures.push(format!("{label}: {error}"));
        evidence(&["WindowDispatch::create_webview"], "smoke::session_checks",
          &format!("{label} child loads smoke-page, reads localStorage={want}, then loads smoke-second and emits its own navigation callback"),
          before, failures);
      }
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
  let before = failures.len();
  set(failures, "set_title", || {
    WindowDispatch::<()>::set_title(window, "smoke-1")
  });
  expect(failures, "title", "smoke-1".to_string(), || {
    WindowDispatch::<()>::title(window)
  });
  evidence(&["WindowDispatch::set_title", "WindowDispatch::title"], "smoke::title_checks",
    "live window title() == smoke-1 after set_title", before, failures);
}

fn geometry_checks(window: &Window, failures: &mut Vec<String>) {
  // A scale factor of zero would make every physical conversion divide by zero.
  let before = failures.len();
  match WindowDispatch::<()>::scale_factor(window) {
    Ok(scale) if scale > 0.0 && scale.is_finite() => println!("ok   scale_factor {scale}"),
    Ok(scale) => failures.push(format!("scale_factor is not usable: {scale}")),
    Err(error) => failures.push(format!("scale_factor: {error}")),
  }
  evidence(&["WindowDispatch::scale_factor"], "smoke::geometry_checks",
    "live Electron geometry reports a finite scale factor greater than zero", before, failures);
  let before = failures.len();
  match WindowDispatch::<()>::current_monitor(window) {
    Ok(Some(monitor)) => println!("ok   current_monitor {:?}", monitor.name),
    Ok(None) => failures.push("current_monitor: the window sits on no monitor".into()),
    Err(error) => failures.push(format!("current_monitor: {error}")),
  }
  evidence(&["WindowDispatch::current_monitor"], "smoke::geometry_checks",
    "Electron screen.getDisplayMatching for the live window returns a monitor", before, failures);
  let before = failures.len();
  match WindowDispatch::<()>::available_monitors(window) {
    Ok(monitors) if !monitors.is_empty() => println!("ok   available_monitors {}", monitors.len()),
    Ok(_) => failures.push("available_monitors is empty".into()),
    Err(error) => failures.push(format!("available_monitors: {error}")),
  }

  evidence(&["WindowDispatch::available_monitors"], "smoke::geometry_checks",
    "Electron screen.getAllDisplays returns a nonempty monitor list", before, failures);

  // The creation size travelled through a builder setter, the way Tauri's own
  // `inner_size` does; a runtime that drops it comes up at Electron's default.
  let scale = WindowDispatch::<()>::scale_factor(window).unwrap_or(1.0);
  let before = failures.len();
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

  evidence(&["WindowBuilder::inner_size", "WindowDispatch::inner_size"], "smoke::geometry_checks",
    "created content size is 320x240 logical pixels converted to physical pixels within 10px after builder inner_size overrides config 400x300",
    before, failures);

  // Stay above the 250x180 DIP minimum on high-DPI screens while exercising
  // the physical-size conversion rather than asking the OS to clamp our target.
  let resized = PhysicalSize::new((420.0 * scale).round() as u32, (320.0 * scale).round() as u32);
  let before = failures.len();
  set(failures, "set_size", || {
    WindowDispatch::<()>::set_size(window, Size::Physical(resized))
  });
  expect_size(failures, "inner_size", resized, 8, || {
    WindowDispatch::<()>::inner_size(window)
  });
  evidence(&["WindowDispatch::set_size", "WindowDispatch::inner_size"], "smoke::geometry_checks",
    "live content size matches set_size(420x320 logical pixels in physical units) within 8px", before, failures);

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
  let before = failures.len();
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

  evidence(&["WindowDispatch::outer_size"], "smoke::geometry_checks",
    "live outer frame size contains the live content size in both dimensions", before, failures);

  // The minimum size came from the creation config; a window smaller than it
  // means the runtime dropped that field on the way to Electron. Electron
  // applies it to the frame, so `outer_size` is what has to hold. Last, because
  // it leaves the window at the minimum.
  let scale = WindowDispatch::<()>::scale_factor(window).unwrap_or(1.0);
  let floor = PhysicalSize::new((250.0 * scale) as u32, (180.0 * scale) as u32);
  let before = failures.len();
  set(failures, "set_size below the minimum", || {
    WindowDispatch::<()>::set_size(window, Size::Physical(PhysicalSize::new(120, 100)))
  });
  expect_floor(failures, "minimum size holds", floor, || {
    WindowDispatch::<()>::outer_size(window)
  });
  evidence(&["WindowBuilder::with_config"], "smoke::geometry_checks",
    "with_config min_width=250/min_height=180 keeps the live outer frame above the scaled minimum after requesting 120x100 physical pixels",
    before, failures);
}

fn state_checks(window: &Window, failures: &mut Vec<String>) {
  let before = failures.len();
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
  evidence(&["WindowDispatch::set_resizable", "WindowDispatch::is_resizable"], "smoke::state_checks",
    "live isResizable reads false then true after set_resizable(false) then set_resizable(true)", before, failures);

  let before = failures.len();
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
  evidence(&["WindowDispatch::set_enabled", "WindowDispatch::is_enabled"], "smoke::state_checks",
    "live isEnabled reads false then true after set_enabled(false) then set_enabled(true)", before, failures);
  // Already-focused windows cannot prove that set_focus changed anything.
  let before = failures.len();
  expect_applied(
    failures,
    "is_focused",
    true,
    || WindowDispatch::<()>::set_focus(window),
    || WindowDispatch::<()>::is_focused(window),
  );
  evidence(&["WindowDispatch::is_focused"], "smoke::state_checks",
    "live Electron isFocused reports true", before, failures);

  // Creation config must survive before the setter changes the native state.
  let before = failures.len();
  expect(failures, "is_always_on_top at creation", true, || {
    WindowDispatch::<()>::is_always_on_top(window)
  });
  evidence(&["WindowBuilder::with_config", "WindowDispatch::is_always_on_top"], "smoke::state_checks",
    "live always-on-top state is true from creation config before the setter is called", before, failures);
  let before = failures.len();
  expect_applied(
    failures,
    "is_always_on_top off",
    false,
    || WindowDispatch::<()>::set_always_on_top(window, false),
    || WindowDispatch::<()>::is_always_on_top(window),
  );
  expect_applied(
    failures,
    "is_always_on_top",
    true,
    || WindowDispatch::<()>::set_always_on_top(window, true),
    || WindowDispatch::<()>::is_always_on_top(window),
  );
  evidence(&["WindowDispatch::set_always_on_top", "WindowDispatch::is_always_on_top"], "smoke::state_checks",
    "live always-on-top state changes false then true after set_always_on_top(false) then true", before, failures);

  // These getters mirror requested values, not native frame/button state.
  // Retain their regressions, but do not emit behavior-pass evidence for them.
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

  let before = failures.len();
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
  evidence(&["WindowDispatch::maximize", "WindowDispatch::unmaximize", "WindowDispatch::is_maximized"], "smoke::state_checks",
    "live maximized state reads true after maximize and false after unmaximize", before, failures);

  if cfg!(any(windows, target_os = "macos")) {
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

  let before = failures.len();
  set(failures, "set_theme light", || {
    WindowDispatch::<()>::set_theme(window, Some(Theme::Light))
  });
  expect(failures, "theme light", Theme::Light, || {
    WindowDispatch::<()>::theme(window)
  });
  set(failures, "set_theme", || {
    WindowDispatch::<()>::set_theme(window, Some(Theme::Dark))
  });
  expect(failures, "theme", Theme::Dark, || {
    WindowDispatch::<()>::theme(window)
  });
  evidence(&["WindowDispatch::set_theme", "WindowDispatch::theme"], "smoke::state_checks",
    "theme reads Light then Dark after set_theme(Some(Light)) then set_theme(Some(Dark))", before, failures);

  // Minimization runs last because macOS re-derives its window style.
  let before = failures.len();
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
  evidence(&["WindowDispatch::minimize", "WindowDispatch::unminimize", "WindowDispatch::is_minimized"], "smoke::state_checks",
    "live minimized state reads true after minimize and false after unminimize", before, failures);
}

fn visibility_checks(window: &Window, failures: &mut Vec<String>) {
  let before = failures.len();
  set(failures, "hide", || WindowDispatch::<()>::hide(window));
  expect(failures, "is_visible", false, || {
    WindowDispatch::<()>::is_visible(window)
  });
  set(failures, "show", || WindowDispatch::<()>::show(window));
  expect(failures, "is_visible again", true, || {
    WindowDispatch::<()>::is_visible(window)
  });
  evidence(&["WindowDispatch::hide", "WindowDispatch::show", "WindowDispatch::is_visible"], "smoke::visibility_checks",
    "live isVisible reads false after hide and true after show", before, failures);
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
  #[cfg(windows)]
  use windows::{core::PCWSTR, Win32::{
    Foundation::{POINT, RECT}, Graphics::Gdi::ClientToScreen,
    UI::WindowsAndMessaging::{FindWindowExW, GetClassNameW, GetPhysicalCursorPos, GetWindowRect,
      GetWindowThreadProcessId, IsIconic, IsWindowVisible, SetPhysicalCursorPos},
  }};
  #[cfg(windows)]
  let mut restore_title = None;
  #[cfg(windows)]
  let mut restore_pointer = None;
  let before = failures.len();
  let result = (|| -> tauri_runtime::Result<()> {
    #[cfg(windows)]
    let win32_error = |error| tauri_runtime::Error::CreateWebview(Box::new(error));
    #[cfg(windows)]
    let hwnd = {
      restore_title = Some(WindowDispatch::<()>::title(window)?);
      let mut point = POINT::default();
      unsafe { GetPhysicalCursorPos(&mut point) }.map_err(win32_error)?;
      restore_pointer = Some(point);
      let title = format!("smoke-cursor-{}", std::process::id());
      WindowDispatch::<()>::set_title(window, title.clone())?;
      let title: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
      let started = Instant::now();
      loop {
        let mut after = None;
        let mut found = Vec::new();
        // Enumerate matching top-level windows, not the first unqualified title
        // match. The hidden menu proxy belongs to Rust, not to Electron.
        while let Ok(hwnd) = unsafe { FindWindowExW(None, after, None, PCWSTR(title.as_ptr())) } {
          after = Some(hwnd);
          let mut pid = 0;
          unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)); }
          if pid != 0 && pid != std::process::id()
            && unsafe { IsWindowVisible(hwnd) }.as_bool() && !unsafe { IsIconic(hwnd) }.as_bool() {
            found.push(hwnd);
          }
        }
        if found.len() == 1 {
          let hwnd = found[0];
          let mut class = [0; 256];
          let length = unsafe { GetClassNameW(hwnd, &mut class) };
          println!("ok   cursor test HWND {hwnd:?} class {}", String::from_utf16_lossy(&class[..length.max(0) as usize]));
          break hwnd;
        }
        if started.elapsed() > SETTLE {
          failures.push(format!("cursor window lookup: expected one visible foreign-process window with unique title, found {found:?}"));
          return Ok(());
        }
        thread::sleep(Duration::from_millis(25));
      }
    };
    #[cfg(not(windows))]
    let origin = WindowDispatch::<()>::inner_position(window)?;
    let scale = WindowDispatch::<()>::scale_factor(window)?;
    let size = WindowDispatch::<()>::inner_size(window)?;
    // Two different interior points catch both a no-op and treating these
    // window-relative coordinates as desktop coordinates.
    for (divisor, logical) in [(3, false), (2, true)] {
      let offset = PhysicalPosition::new((size.width / divisor) as i32, (size.height / divisor) as i32);
      let request = if logical { Position::Logical(offset.to_logical::<f64>(scale)) }
        else { Position::Physical(offset) };
      let started = Instant::now();
      loop {
        #[cfg(windows)]
        let (target, bounds) = {
          let mut bounds = RECT::default();
          let mut point = POINT { x: offset.x, y: offset.y };
          // Show/restore can still move the window. Sample the native client
          // mapping on every retry instead of freezing a transient origin.
          unsafe { GetWindowRect(hwnd, &mut bounds) }.map_err(win32_error)?;
          unsafe { ClientToScreen(hwnd, &mut point) }.ok().map_err(win32_error)?;
          (PhysicalPosition::new(f64::from(point.x), f64::from(point.y)), bounds)
        };
        #[cfg(not(windows))]
        let target = PhysicalPosition::new(f64::from(origin.x + offset.x), f64::from(origin.y + offset.y));
        WindowDispatch::<()>::set_cursor_position(window, request)?;
        let got = <ElectronRuntimeHandle as RuntimeHandle<()>>::cursor_position(handle)?;
        #[cfg(windows)]
        let native = {
          let mut point = POINT::default();
          unsafe { GetPhysicalCursorPos(&mut point) }.map_err(win32_error)?;
          PhysicalPosition::new(f64::from(point.x), f64::from(point.y))
        };
        #[cfg(not(windows))]
        let native = got;
        if (got.x - target.x).abs() <= 2.0 && (got.y - target.y).abs() <= 2.0
          && (native.x - target.x).abs() <= 2.0 && (native.y - target.y).abs() <= 2.0 {
          CHECKS.fetch_add(1, Ordering::Relaxed);
          println!("ok   window-relative cursor_position {got:?} native={native:?} from {start:?} (logical={logical})");
          break;
        }
        if started.elapsed() > SETTLE {
          #[cfg(windows)]
          eprintln!("cursor test HWND {hwnd:?} window bounds {bounds:?}, offset {offset:?}");
          failures.push(format!("cursor_position (logical={logical}): runtime {got:?}, native {native:?}, want {target:?}"));
          break;
        }
        // A resting physical mouse may resend its position between warp/read.
        thread::sleep(Duration::from_millis(25));
      }
    }
    CHECKS.fetch_add(1, Ordering::Relaxed);
    if WindowDispatch::<()>::set_cursor_position(window,
      tauri_runtime::dpi::LogicalPosition::new(f64::NAN, 0.0)).is_ok() {
      failures.push("cursor position accepted NaN".into());
    }
    #[cfg(not(windows))]
    WindowDispatch::<()>::set_cursor_position(window, PhysicalPosition::new(
      (start.x.round() as i32).saturating_sub(origin.x),
      (start.y.round() as i32).saturating_sub(origin.y),
    ))?;
    Ok(())
  })();
  if let Err(error) = result { failures.push(format!("cursor_position: {error}")); }
  #[cfg(windows)]
  {
    if let Some(point) = restore_pointer {
      if let Err(error) = unsafe { SetPhysicalCursorPos(point.x, point.y) } {
        failures.push(format!("restore cursor: {error}"));
      }
    }
    if let Some(title) = restore_title {
      set(failures, "restore cursor test title", || WindowDispatch::<()>::set_title(window, title));
    }
  }
  evidence(&["WindowDispatch::set_cursor_position", "RuntimeHandle::cursor_position"], "smoke::cursor_checks",
    "physical and logical client offsets reach two distinct screen points and NaN is rejected; on Windows a unique visible Electron HWND supplies ClientToScreen targets and GetPhysicalCursorPos independently agrees with the runtime getter",
    before, failures);
}

/// The cursor icon and visibility show up on the page, so the page reports them.
fn cursor_style_checks(window: &Window, webview: &Webview, failures: &mut Vec<String>) {
  let before = failures.len();
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
  evidence(&["WindowDispatch::set_cursor_icon"], "smoke::cursor_style_checks",
    "page computed root cursor is pointer after set_cursor_icon(Hand)", before, failures);
  let before = failures.len();
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
  evidence(&["WindowDispatch::set_cursor_visible"], "smoke::cursor_style_checks",
    "page computed cursor changes to none when hidden and returns to pointer when shown", before, failures);
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

  let before = failures.len();
  expect_eval(failures, "eval_script_with_callback", "2", webview, "1 + 1");
  evidence(&["WebviewDispatch::eval_script_with_callback"], "smoke::webview_checks",
    "the real page evaluates 1 + 1 and invokes the callback with JSON 2", before, failures);
  let before = failures.len();
  set(failures, "eval_script", || {
    WebviewDispatch::<()>::eval_script(webview, "window.__smokeEval = 'executed'")
  });
  expect_eval(failures, "eval_script page effect", "executed", webview, "window.__smokeEval");
  evidence(&["WebviewDispatch::eval_script"], "smoke::webview_checks",
    "callback evaluation reads executed from the page global written by fire-and-forget eval_script", before, failures);
  expect_eval(
    failures,
    "loaded page",
    "smoke-page",
    webview,
    "document.title",
  );

  let before = failures.len();
  expect(failures, "url", PAGE.to_string(), || {
    WebviewDispatch::<()>::url(webview)
  });
  expect_eval(failures, "live initial URL", PAGE, webview, "location.href");
  evidence(&["WebviewDispatch::url"], "smoke::webview_checks",
    "url() and the real page location.href both equal the initial custom-protocol URL", before, failures);
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
  let before = failures.len();
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
  expect_eval(failures, "live navigated URL", SECOND_PAGE, webview, "location.href");
  evidence(&["WebviewDispatch::navigate", "WebviewDispatch::url"], "smoke::webview_checks",
    "navigate loads smoke-second, emits the matching navigation callback, and url() agrees with the page location.href", before, failures);

  let before = failures.len();
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
  evidence(&["WebviewDispatch::open_devtools", "WebviewDispatch::close_devtools", "WebviewDispatch::is_devtools_open"], "smoke::webview_checks",
    "Electron isDevToolsOpened reads true after open_devtools and false after close_devtools", before, failures);

  // A zoom factor is a style/paint decision, invisible to every getter of this
  // trait, so the page itself reports it, relative to the display scale.
  let scale = eval_once(webview, "window.devicePixelRatio")
    .and_then(|value| value.trim_matches('"').parse::<f64>().ok())
    .unwrap_or(1.0);
  let before = failures.len();
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
  evidence(&["WebviewDispatch::set_zoom"], "smoke::webview_checks",
    "page devicePixelRatio doubles after set_zoom(2) and returns to its initial value after set_zoom(1)", before, failures);

  // The runtime reports document titles to the handler an app registers.
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

/// Call only after the named behavior assertion, never after a setter alone.
fn evidence(members: &[&str], test: &str, assertion: &str, before: usize, failures: &[String]) {
  let status = if failures.len() == before { "pass" } else { "fail" };
  for member in members {
    println!("[runtime-evidence] {}", serde_json::json!({
      "member": member,
      "test": test,
      "assertion": assertion,
      "kind": "behavior",
      "status": status,
    }));
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
