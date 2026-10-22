//! End-to-end smoke test against a real Electron window.
//!
//! `cargo run --example smoke` exits non-zero when a getter disagrees with the
//! setter it was handed, so CI fails instead of trusting the compiler. On Linux
//! this needs a display server (CI runs it under `xvfb-run`).

use std::fmt::Debug;

use tauri_runtime::{
  Icon, ProgressBarState, ProgressBarStatus, Runtime, RuntimeInitArgs, UserAttentionType,
  WebviewDispatch, WindowDispatch,
  dpi::{LogicalPosition, LogicalSize, LogicalUnit, PhysicalPosition, PixelUnit, Position, Size},
  webview::{PendingWebview, WebviewAttributes},
  window::{CursorIcon, PendingWindow, RawWindow, WindowBuilder, WindowSizeConstraints},
};
use tauri_runtime_electron::{
  Electron, ElectronRuntime, ElectronWebviewDispatcher, ElectronWindowBuilder,
  ElectronWindowDispatcher,
};
use tauri_utils::{
  Theme,
  config::{Color, WebviewUrl},
};

/// Pins the event type so the dispatch methods resolve without annotations.
macro_rules! w {
  (($window:expr) $method:ident ( $($arg:expr),* )) => {
    <ElectronWindowDispatcher as WindowDispatch<()>>::$method(&$window $(, $arg)*)
  };
}

/// Same, for the webview dispatcher.
macro_rules! v {
  (($webview:expr) $method:ident ( $($arg:expr),* )) => {
    <ElectronWebviewDispatcher as WebviewDispatch<()>>::$method(&$webview $(, $arg)*)
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

/// A setter without a getter still proves its IPC round trip reached Electron:
/// a missing handler answers with an error.
macro_rules! accepted {
  ($failures:expr, $name:expr, $call:expr) => {
    match $call {
      Ok(()) => check($failures, $name, true, true),
      Err(error) => {
        *$failures += 1;
        println!("FAIL {} = {error}", $name);
      }
    }
  };
}

/// macOS animates minimize and fullscreen, so the state settles a moment after
/// the call returns; poll instead of racing the window manager.
#[cfg(not(target_os = "linux"))]
fn eventually<A: Clone + Debug + PartialEq>(
  failures: &mut usize,
  name: &str,
  expected: A,
  mut read: impl FnMut() -> Option<A>,
) {
  let mut actual = read();
  for _ in 0..200 {
    if actual.as_ref() == Some(&expected) {
      break;
    }
    std::thread::sleep(std::time::Duration::from_millis(50));
    actual = read();
  }
  check(failures, name, actual, Some(expected));
}

/// Runtime attributes for the smoke run.
///
/// Chromium's SUID sandbox helper is not usable on CI runners (Ubuntu 24.04
/// restricts unprivileged user namespaces), and the window behaviour is what
/// this example is here to verify.
fn runtime_attrs() -> Electron {
  #[cfg(target_os = "linux")]
  return Electron::default().arg("--no-sandbox");
  #[cfg(not(target_os = "linux"))]
  Electron::default()
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
    runtime_init_attrs: runtime_attrs(),
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
  let position_of = |window: &ElectronWindowDispatcher| -> Option<(i64, i64)> {
    let scale = w!((window) scale_factor()).ok()?;
    let position: PhysicalPosition<i32> = w!((window) outer_position()).ok()?;
    Some((
      (f64::from(position.x) / scale).round() as i64,
      (f64::from(position.y) / scale).round() as i64,
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

  // --- what the builder was asked for ------------------------------------
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

  // --- geometry ----------------------------------------------------------
  w!((window) set_position(Position::Logical(LogicalPosition::new(120.0, 90.0))))
    .expect("set position");
  check(failures, "set_position", position_of(&window), Some((120, 90)));

  w!((window) set_size(Size::Logical(LogicalSize::new(640.0, 480.0)))).expect("set size");
  check(failures, "set_size", size_of(&window), Some((640, 480)));

  accepted!(failures, "center", w!((window) center()));
  accepted!(
    failures,
    "set_min_size",
    w!((window) set_min_size(Some(Size::Logical(LogicalSize::new(320.0, 240.0)))))
  );
  accepted!(
    failures,
    "set_max_size",
    w!((window) set_max_size(Some(Size::Logical(LogicalSize::new(1920.0, 1080.0)))))
  );
  accepted!(
    failures,
    "set_size_constraints",
    w!((window) set_size_constraints(WindowSizeConstraints {
      min_width: Some(PixelUnit::Logical(LogicalUnit(200.0))),
      min_height: Some(PixelUnit::Logical(LogicalUnit(150.0))),
      max_width: Some(PixelUnit::Logical(LogicalUnit(2000.0))),
      max_height: Some(PixelUnit::Logical(LogicalUnit(1500.0))),
    }))
  );
  let outer = w!((window) outer_size()).ok();
  let inner = w!((window) inner_size()).ok();
  check(
    failures,
    "outer_size >= inner_size",
    outer.zip(inner).map(|(o, i)| o.width >= i.width && o.height >= i.height),
    Some(true),
  );

  // --- state -------------------------------------------------------------
  w!((window) set_title("smoke 2")).expect("set title");
  check(
    failures,
    "set_title",
    w!((window) title()).ok(),
    Some("smoke 2".to_string()),
  );

  for (name, expected) in [("resizable off", false), ("resizable on", true)] {
    w!((window) set_resizable(expected)).expect("set resizable");
    check(failures, name, w!((window) is_resizable()).ok(), Some(expected));
  }

  // Electron implements these two on macOS and Windows only.
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

  w!((window) set_enabled(false)).expect("set enabled");
  check(
    failures,
    "set_enabled off",
    w!((window) is_enabled()).ok(),
    Some(false),
  );
  w!((window) set_enabled(true)).expect("set enabled");
  check(
    failures,
    "set_enabled on",
    w!((window) is_enabled()).ok(),
    Some(true),
  );

  check(
    failures,
    "theme before",
    w!((window) theme()).is_ok(),
    true,
  );
  w!((window) set_theme(Some(Theme::Dark))).expect("set theme");
  check(
    failures,
    "set_theme dark",
    w!((window) theme()).ok(),
    Some(Theme::Dark),
  );
  w!((window) set_theme(Some(Theme::Light))).expect("set theme");
  check(
    failures,
    "set_theme light",
    w!((window) theme()).ok(),
    Some(Theme::Light),
  );
  w!((window) set_theme(None)).expect("clear theme");

  w!((window) hide()).expect("hide");
  check(
    failures,
    "hide",
    w!((window) is_visible()).ok(),
    Some(false),
  );
  w!((window) show()).expect("show");
  check(
    failures,
    "show",
    w!((window) is_visible()).ok(),
    Some(true),
  );

  // A window manager is required for these, and xvfb has none.
  #[cfg(not(target_os = "linux"))]
  {
    w!((window) maximize()).expect("maximize");
    eventually(failures, "maximize", true, || {
      w!((window) is_maximized()).ok()
    });
    w!((window) unmaximize()).expect("unmaximize");
    eventually(failures, "unmaximize", false, || {
      w!((window) is_maximized()).ok()
    });
    w!((window) minimize()).expect("minimize");
    eventually(failures, "minimize", true, || {
      w!((window) is_minimized()).ok()
    });
    w!((window) unminimize()).expect("unminimize");
    eventually(failures, "unminimize", false, || {
      w!((window) is_minimized()).ok()
    });

    w!((window) set_fullscreen(true)).expect("fullscreen");
    eventually(failures, "set_fullscreen true", true, || {
      w!((window) is_fullscreen()).ok()
    });
    w!((window) set_fullscreen(false)).expect("windowed");
    eventually(failures, "set_fullscreen false", false, || {
      w!((window) is_fullscreen()).ok()
    });
  }
  accepted!(
    failures,
    "set_fullscreen_on_monitor",
    w!((window) set_fullscreen_on_monitor(PhysicalPosition::new(0.0, 0.0)))
  );
  accepted!(failures, "set_focus", w!((window) set_focus()));

  #[cfg(windows)]
  {
    w!((window) set_decorations(true)).expect("decorate");
    check(
      failures,
      "set_decorations true",
      w!((window) is_decorated()).ok(),
      Some(true),
    );
    w!((window) set_decorations(false)).expect("undecorate");
    check(
      failures,
      "set_decorations false",
      w!((window) is_decorated()).ok(),
      Some(false),
    );
  }

  // --- setters whose state Electron does not report back -----------------
  accepted!(failures, "set_skip_taskbar", w!((window) set_skip_taskbar(false)));
  accepted!(failures, "set_shadow", w!((window) set_shadow(true)));
  accepted!(
    failures,
    "set_background_color",
    w!((window) set_background_color(Some(Color(20, 20, 20, 255))))
  );
  accepted!(
    failures,
    "set_content_protected",
    w!((window) set_content_protected(false))
  );
  accepted!(
    failures,
    "set_always_on_bottom",
    w!((window) set_always_on_bottom(false))
  );
  accepted!(
    failures,
    "set_visible_on_all_workspaces",
    w!((window) set_visible_on_all_workspaces(false))
  );
  accepted!(failures, "set_focusable", w!((window) set_focusable(true)));
  accepted!(
    failures,
    "set_icon",
    w!((window) set_icon(Icon {
      rgba: std::borrow::Cow::Borrowed(&[255u8, 0, 0, 255]),
      width: 1,
      height: 1,
    }))
  );
  accepted!(
    failures,
    "set_badge_count",
    w!((window) set_badge_count(Some(1), None))
  );
  accepted!(
    failures,
    "set_badge_label",
    w!((window) set_badge_label(Some("1".to_string())))
  );
  accepted!(failures, "set_overlay_icon", w!((window) set_overlay_icon(None)));
  accepted!(
    failures,
    "set_progress_bar",
    w!((window) set_progress_bar(ProgressBarState {
      status: Some(ProgressBarStatus::Normal),
      progress: Some(50),
      desktop_filename: None,
    }))
  );
  accepted!(
    failures,
    "request_user_attention",
    w!((window) request_user_attention(Some(UserAttentionType::Informational)))
  );
  accepted!(
    failures,
    "set_traffic_light_position",
    w!((window) set_traffic_light_position(Position::Logical(LogicalPosition::new(20.0, 20.0))))
  );

  // --- cursor ------------------------------------------------------------
  accepted!(failures, "set_cursor_visible", w!((window) set_cursor_visible(true)));
  accepted!(
    failures,
    "set_cursor_icon",
    w!((window) set_cursor_icon(CursorIcon::Hand))
  );
  accepted!(
    failures,
    "set_cursor_position",
    w!((window) set_cursor_position(Position::Logical(LogicalPosition::new(300.0, 300.0))))
  );
  accepted!(
    failures,
    "set_cursor_grab off",
    w!((window) set_cursor_grab(false))
  );
  accepted!(
    failures,
    "set_ignore_cursor_events",
    w!((window) set_ignore_cursor_events(false))
  );

  // --- monitors ----------------------------------------------------------
  check(
    failures,
    "current_monitor",
    w!((window) current_monitor()).ok().flatten().is_some(),
    true,
  );
  check(
    failures,
    "primary_monitor",
    w!((window) primary_monitor()).ok().flatten().is_some(),
    true,
  );
  check(
    failures,
    "available_monitors",
    w!((window) available_monitors()).ok().is_some_and(|list| !list.is_empty()),
    true,
  );
  check(
    failures,
    "monitor_from_point",
    w!((window) monitor_from_point(10.0, 10.0)).ok().flatten().is_some(),
    true,
  );

  // --- second window -----------------------------------------------------
  // Same frameless guarantee, independent state, and a default-sized window so
  // the builder path is exercised without an explicit size.
  let builder = ElectronWindowBuilder::new()
    .inner_size(400.0, 300.0)
    .title("smoke-2")
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
  check(
    failures,
    "second window title",
    w!((second) title()).ok(),
    Some("smoke-2".to_string()),
  );
  check(
    failures,
    "second window size",
    size_of(&second),
    Some((400, 300)),
  );

  // --- guest webview -----------------------------------------------------
  // A second webview on the same window, i.e. a `<webview>` in Electron terms.
  let mut owner = window.clone();
  let attrs = WebviewAttributes::new(WebviewUrl::External(
    "about:blank".parse().expect("about:blank"),
  ));
  let pending =
    PendingWebview::<(), ElectronRuntime>::new(attrs, (), "guest").expect("pending webview");
  match owner.create_webview(pending) {
    Ok(created) => {
      let webview = created.dispatcher;
      check(
        failures,
        "webview url",
        v!((webview) url()).ok(),
        Some("about:blank".to_string()),
      );
      check(failures, "webview size", v!((webview) size()).is_ok(), true);
      check(
        failures,
        "webview position",
        v!((webview) position()).is_ok(),
        true,
      );
      check(
        failures,
        "webview can_go_back",
        v!((webview) can_go_back()).ok(),
        Some(false),
      );
      check(
        failures,
        "webview can_go_forward",
        v!((webview) can_go_forward()).ok(),
        Some(false),
      );
      check(failures, "webview cookies", v!((webview) cookies()).is_ok(), true);
      accepted!(
        failures,
        "webview eval_script",
        v!((webview) eval_script("document.title = 'guest'"))
      );
      accepted!(
        failures,
        "webview set_size",
        v!((webview) set_size(Size::Logical(LogicalSize::new(200.0, 150.0))))
      );
      accepted!(
        failures,
        "webview set_position",
        v!((webview) set_position(Position::Logical(LogicalPosition::new(10.0, 10.0))))
      );
      accepted!(failures, "webview reload", v!((webview) reload()));
      accepted!(failures, "webview show", v!((webview) show()));
      accepted!(failures, "webview hide", v!((webview) hide()));
      accepted!(failures, "webview set_focus", v!((webview) set_focus()));
      accepted!(failures, "webview set_zoom", v!((webview) set_zoom(1.5)));
      accepted!(
        failures,
        "webview set_background_color",
        v!((webview) set_background_color(Some(Color(0, 0, 0, 255))))
      );
      accepted!(failures, "webview close", v!((webview) close()));
    }
    Err(error) => {
      *failures += 1;
      println!("FAIL create webview = {error}");
    }
  }
}
