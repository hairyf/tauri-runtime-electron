//! Standalone real Electron checks for runtime/proxy, creation and desktop geometry.
//! Run on the desktop (Linux: xvfb + a window manager). No mocks or cached options.
//! Unobserved native effects remain unknown; a one-screen run never claims two-screen coverage.
use std::{
  borrow::Cow,
  process::{Command, ExitCode, Stdio},
  sync::{Arc, Mutex, atomic::{AtomicBool, AtomicUsize, Ordering}, mpsc},
  thread,
  time::{Duration, Instant},
};
use tauri_runtime::{
  EventLoopProxy, Icon, RunEvent, Runtime, RuntimeHandle, RuntimeInitArgs,
  WebviewDispatch, WindowDispatch,
  dpi::{LogicalPosition, LogicalSize, PhysicalPosition, PhysicalSize, PixelUnit, Position, Size},
  monitor::Monitor,
  webview::{PendingWebview, WebviewAttributes},
  window::{DetachedWindow, PendingWindow, RawWindow, WindowBuilder, WindowEvent, WindowSizeConstraints},
};
use tauri_runtime_electron::{ElectronRuntime, ElectronRuntimeHandle, ElectronWebviewDispatcher, ElectronWindowBuilder, ElectronWindowDispatcher};
use tauri_utils::{Theme, config::WebviewUrl};

const WAIT: Duration = Duration::from_secs(8);
type Window = ElectronWindowDispatcher;
type Webview = ElectronWebviewDispatcher;
type Detached = DetachedWindow<u32, ElectronRuntime>;
static SERIAL: AtomicUsize = AtomicUsize::new(0);

fn main() -> ExitCode {
  thread::spawn(|| {
    thread::sleep(Duration::from_secs(240));
    eprintln!("desktop behavior smoke exceeded 240s");
    std::process::abort();
  });
  match std::env::args().nth(1).as_deref() {
    Some("--run-child") => return exit_child(),
    Some("--any-thread-child") => return any_thread_child(),
    _ => {}
  }
  for (mode, code) in [("--run-child", 29), ("--any-thread-child", 0)] {
    if cfg!(target_os = "macos") && mode == "--any-thread-child" { continue }
    let mut child = Command::new(std::env::current_exe().unwrap()).arg(mode)
      .stdout(Stdio::inherit()).stderr(Stdio::inherit()).spawn().expect("launch isolated runtime");
    let start = Instant::now();
    let status = loop {
      if let Some(status) = child.try_wait().unwrap() { break status }
      if start.elapsed() > Duration::from_secs(60) {
        let _ = child.kill();
        let _ = child.wait();
        panic!("{mode} did not exit in 60s");
      }
      thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(status.code(), Some(code), "{mode}");
    if mode == "--run-child" {
      evidence(&["Runtime::run", "RuntimeHandle::request_exit"], "run exits its isolated process with requested code 29 after Ready/ExitRequested/Exit", "pass");
    } else {
      evidence(&["Runtime::new_any_thread"], "a non-main thread initializes real Electron, receives its user event and returns the requested exit code", "pass");
    }
  }
  let mut runtime = <ElectronRuntime as Runtime<u32>>::new(RuntimeInitArgs::default()).unwrap();
  let ui_thread = thread::current().id();
  let handle = <ElectronRuntime as Runtime<u32>>::handle(&runtime);
  let from_runtime = <ElectronRuntime as Runtime<u32>>::create_proxy(&runtime);
  let from_handle = <ElectronRuntimeHandle as RuntimeHandle<u32>>::create_proxy(&handle);
  EventLoopProxy::<u32>::send_event(&from_runtime, 401).unwrap();
  EventLoopProxy::<u32>::send_event(&from_handle, 402).unwrap();
  let observed = Arc::new(Mutex::new(Vec::new()));
  let events = observed.clone();
  <ElectronRuntime as Runtime<u32>>::run_iteration(&mut runtime, move |event| {
    if let RunEvent::UserEvent(nonce) = event { events.lock().unwrap().push((nonce, thread::current().id())); }
  });
  assert_eq!(*observed.lock().unwrap(), vec![(401, ui_thread), (402, ui_thread)]);
  evidence(&["Runtime::create_proxy", "RuntimeHandle::create_proxy", "EventLoopProxy::send_event", "Runtime::run_iteration"],
    "one iteration returns to its caller after delivering two distinct proxy nonces exactly once, in order, on the runtime thread", "pass");

  let displays = <ElectronRuntime as Runtime<u32>>::available_monitors(&runtime);
  monitors(&displays);
  let primary = <ElectronRuntime as Runtime<u32>>::primary_monitor(&runtime).expect("primary display");
  assert!(displays.iter().any(|monitor| same_monitor(monitor, &primary)));
  for monitor in &displays {
    let point = query_point(monitor);
    assert!(same_monitor(&<ElectronRuntime as Runtime<u32>>::monitor_from_point(&runtime, point.0, point.1).unwrap(), monitor));
  }
  let outside = outside_point(&displays);
  assert!(<ElectronRuntime as Runtime<u32>>::monitor_from_point(&runtime, outside.0, outside.1).is_none());
  let runtime_cursor = <ElectronRuntime as Runtime<u32>>::cursor_position(&runtime).unwrap();
  assert!(runtime_cursor.x.is_finite() && runtime_cursor.y.is_finite());
  evidence(&["Runtime::available_monitors", "Runtime::primary_monitor", "Runtime::monitor_from_point"],
    &format!("{} actual displays have usable physical geometry; primary belongs to the list, each interior query returns its display and outside all displays returns None", displays.len()), "pass");

  let root = <ElectronRuntime as Runtime<u32>>::create_window(&runtime,
    pending(ElectronWindowBuilder::new().inner_size(420.0, 320.0).always_on_top(cfg!(windows)), "runtime-create", true), None::<fn(RawWindow<'_>)>).unwrap();
  let runtime_child = <ElectronRuntime as Runtime<u32>>::create_webview(&runtime, root.id, page("runtime-child")).unwrap().dispatcher;
  let root_page = root.webview.as_ref().unwrap().webview.dispatcher.clone();
  let worker = thread::spawn(move || {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
      expect_eval(&root_page, "document.body.dataset.proof", serde_json::json!("runtime-create"));
      expect_eval(&runtime_child, "document.body.dataset.proof", serde_json::json!("runtime-child"));
      evidence(&["Runtime::create_window", "Runtime::create_webview", "WindowBuilder::new"],
        "both direct Runtime creation entries produce independently evaluable native pages with their own proof values", "pass");
      let (tx, rx) = mpsc::channel();
      <ElectronRuntimeHandle as RuntimeHandle<u32>>::run_on_main_thread(&handle, move || {
        tx.send(thread::current().id()).unwrap();
      }).unwrap();
      assert_eq!(rx.recv_timeout(WAIT).unwrap(), ui_thread);
      assert_ne!(thread::current().id(), ui_thread);
      evidence(&["Runtime::handle", "RuntimeHandle::run_on_main_thread"],
        "a handle moved to a distinct worker schedules a callback on the recorded runtime thread", "pass");
      let (tx, rx) = mpsc::channel();
      WebviewDispatch::<u32>::run_on_main_thread(&root_page, move || { tx.send(thread::current().id()).unwrap(); }).unwrap();
      assert_eq!(rx.recv_timeout(WAIT).unwrap(), ui_thread);
      evidence(&["WebviewDispatch::run_on_main_thread"], "a webview scheduled from a worker invokes its closure on the runtime thread", "pass");
      creation_checks(&handle, &root, &root_page);
      // The full-size creation probe covers the main view. A hidden Blink page
      // can defer layout/ResizeObserver; geometry checks require a visible page.
      WebviewDispatch::<u32>::close(&runtime_child).unwrap();
      focus(&root.dispatcher);
      expect_eval(&root_page, "document.visibilityState", serde_json::json!("visible"));
      monitor_checks(&handle, &root.dispatcher, &displays, &primary);
      geometry_checks(&root.dispatcher, &root_page, &displays);
      builder_checks(&handle, &root.dispatcher, &displays);
    }));
    <ElectronRuntimeHandle as RuntimeHandle<u32>>::request_exit(&handle, i32::from(result.is_err())).unwrap();
    result.is_ok()
  });
  let code = <ElectronRuntime as Runtime<u32>>::run_return(runtime, |_| {});
  if worker.join().unwrap() && code == 0 { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}

fn exit_child() -> ExitCode {
  let runtime = <ElectronRuntime as Runtime<u32>>::new(RuntimeInitArgs::default()).unwrap();
  let handle = <ElectronRuntime as Runtime<u32>>::handle(&runtime);
  <ElectronRuntimeHandle as RuntimeHandle<u32>>::request_exit(&handle, 29).unwrap();
  let mut order = Vec::new();
  <ElectronRuntime as Runtime<u32>>::run(runtime, move |event| match event {
    RunEvent::Ready => order.push("ready"),
    RunEvent::ExitRequested { code, .. } => { assert_eq!(code, Some(29)); assert_eq!(order, ["ready"]); order.push("accepted"); }
    RunEvent::Exit => assert_eq!(order, ["ready", "accepted"]),
    _ => {}
  });
  panic!("Runtime::run returned instead of exiting");
}

fn any_thread_child() -> ExitCode {
  #[cfg(any(windows, target_os = "linux"))]
  {
    let parent = thread::current().id();
    thread::spawn(move || {
      assert_ne!(thread::current().id(), parent);
      let runtime = <ElectronRuntime as Runtime<u32>>::new_any_thread(RuntimeInitArgs::default()).unwrap();
      let handle = <ElectronRuntime as Runtime<u32>>::handle(&runtime);
      let proxy = <ElectronRuntime as Runtime<u32>>::create_proxy(&runtime);
      EventLoopProxy::<u32>::send_event(&proxy, 703).unwrap();
      let seen = Arc::new(AtomicUsize::new(0));
      let count = seen.clone();
      let code = <ElectronRuntime as Runtime<u32>>::run_return(runtime, move |event| {
        if let RunEvent::UserEvent(nonce) = event {
          assert_eq!(nonce, 703);
          count.fetch_add(1, Ordering::SeqCst);
          <ElectronRuntimeHandle as RuntimeHandle<u32>>::request_exit(&handle, 13).unwrap();
        }
      });
      assert_eq!(code, 13);
      assert_eq!(seen.load(Ordering::SeqCst), 1);
    }).join().unwrap();
  }
  ExitCode::SUCCESS
}

fn evidence(members: &[&str], assertion: &str, status: &str) {
  for member in members {
    println!("[runtime-evidence] {}", serde_json::json!({
      "member": member, "test": "desktop_behavior_smoke", "assertion": assertion,
      "kind": "behavior", "status": status,
    }));
  }
}

fn wait(mut condition: impl FnMut() -> bool, why: &str) {
  let start = Instant::now();
  while !condition() {
    assert!(start.elapsed() < WAIT, "timed out: {why}");
    thread::sleep(Duration::from_millis(40));
  }
}
fn eval(webview: &Webview, script: &str) -> Option<serde_json::Value> {
  let (tx, rx) = mpsc::channel();
  WebviewDispatch::<u32>::eval_script_with_callback(webview, script, move |value| { let _ = tx.send(value); }).ok()?;
  serde_json::from_str(&rx.recv_timeout(WAIT).ok()?).ok()
}
fn expect_eval(webview: &Webview, script: &str, expected: serde_json::Value) {
  wait(|| {
    let actual = eval(webview, script);
    if actual == Some(expected.clone()) { return true }
    eprintln!("desktop renderer {script}: {actual:?}, expected {expected}");
    false
  }, script);
}
fn page(label: &str) -> PendingWebview<u32, ElectronRuntime> {
  let url = format!("data:text/html,<body data-proof='{label}'>desktop behavior</body>");
  let mut page = PendingWebview::new(WebviewAttributes::new(WebviewUrl::External(url.parse().unwrap())), label).unwrap();
  page.url = url;
  page
}
fn pending(builder: ElectronWindowBuilder, label: &str, with_page: bool) -> PendingWindow<u32, ElectronRuntime> {
  let mut pending = PendingWindow::new(builder, label).unwrap();
  if with_page { pending.set_webview(page(label)); }
  pending
}
fn create(handle: &ElectronRuntimeHandle, builder: ElectronWindowBuilder, with_page: bool) -> Detached {
  let label = format!("desktop-{}", SERIAL.fetch_add(1, Ordering::SeqCst));
  <ElectronRuntimeHandle as RuntimeHandle<u32>>::create_window(handle, pending(builder, &label, with_page), None::<fn(RawWindow<'_>)>).unwrap()
}
fn discard(window: &Window) {
  WindowDispatch::<u32>::destroy(window).unwrap();
  wait(|| WindowDispatch::<u32>::title(window).is_err(), "destroy native window");
}

fn creation_checks(handle: &ElectronRuntimeHandle, root: &Detached, root_page: &Webview) {
  let child = <ElectronRuntimeHandle as RuntimeHandle<u32>>::create_webview(handle, root.id, page("handle-child")).unwrap().dispatcher;
  expect_eval(&child, "document.body.dataset.proof", serde_json::json!("handle-child"));
  evidence(&["RuntimeHandle::create_webview"], "handle-created native child loads its own unique proof page", "pass");
  WebviewDispatch::<u32>::close(&child).unwrap();
  wait(|| eval(&child, "1").is_none(), "child actually closes");
  expect_eval(root_page, "document.body.dataset.proof", serde_json::json!("runtime-create"));
  evidence(&["WebviewDispatch::close"], "closing the handle-created child makes it unevaluable while the independent root page remains alive", "pass");
  let mut route = root.dispatcher.clone();
  let extra = WindowDispatch::<u32>::create_window(&mut route,
    pending(ElectronWindowBuilder::new().inner_size(340.0, 240.0), "window-route", true), None::<fn(RawWindow<'_>)>).unwrap();
  expect_eval(&extra.webview.as_ref().unwrap().webview.dispatcher, "document.body.dataset.proof", serde_json::json!("window-route"));
  evidence(&["WindowDispatch::create_window"], "window-dispatch creation makes a separate native window and live proof page", "pass");
  let vetoes = Arc::new(AtomicUsize::new(0));
  let destroyed = Arc::new(AtomicBool::new(false));
  let count = vetoes.clone();
  let gone = destroyed.clone();
  WindowDispatch::<u32>::on_window_event(&extra.dispatcher, move |event| match event {
    WindowEvent::CloseRequested { signal_tx } => { count.fetch_add(1, Ordering::SeqCst); signal_tx.send(true).unwrap(); }
    WindowEvent::Destroyed => { gone.store(true, Ordering::SeqCst); }
    _ => {}
  });
  WindowDispatch::<u32>::close(&extra.dispatcher).unwrap();
  wait(|| vetoes.load(Ordering::SeqCst) > 0, "real close request reaches veto");
  assert!(WindowDispatch::<u32>::title(&extra.dispatcher).is_ok());
  thread::sleep(Duration::from_millis(150));
  let before = vetoes.load(Ordering::SeqCst);
  discard(&extra.dispatcher);
  wait(|| destroyed.load(Ordering::SeqCst), "destroy event");
  assert_eq!(vetoes.load(Ordering::SeqCst), before);
  evidence(&["WindowDispatch::destroy"], "destroy removes a live window and emits Destroyed despite an active close veto, without another CloseRequested", "pass");
}

fn same_monitor(a: &Monitor, b: &Monitor) -> bool { a.position == b.position && a.size == b.size }
fn monitors(displays: &[Monitor]) {
  assert!(!displays.is_empty());
  for monitor in displays {
    assert!(monitor.size.width > 0 && monitor.size.height > 0);
    assert!(monitor.scale_factor.is_finite() && monitor.scale_factor > 0.0);
  }
}
fn query_point(monitor: &Monitor) -> (f64, f64) {
  let x = f64::from(monitor.position.x) + f64::from(monitor.size.width) / 2.0;
  let y = f64::from(monitor.position.y) + f64::from(monitor.size.height) / 2.0;
  if cfg!(windows) { (x, y) } else { (x / monitor.scale_factor, y / monitor.scale_factor) }
}
fn outside_point(displays: &[Monitor]) -> (f64, f64) {
  let right = displays.iter().map(|monitor| f64::from(monitor.position.x) + f64::from(monitor.size.width)).fold(0.0, f64::max);
  let bottom = displays.iter().map(|monitor| f64::from(monitor.position.y) + f64::from(monitor.size.height)).fold(0.0, f64::max);
  (right + 100_000.0, bottom + 100_000.0)
}
fn monitor_checks(handle: &ElectronRuntimeHandle, window: &Window, displays: &[Monitor], primary: &Monitor) {
  let list = <ElectronRuntimeHandle as RuntimeHandle<u32>>::available_monitors(handle).unwrap();
  monitors(&list);
  assert_eq!(list.len(), displays.len());
  assert!(same_monitor(&<ElectronRuntimeHandle as RuntimeHandle<u32>>::primary_monitor(handle).unwrap().unwrap(), primary));
  assert!(same_monitor(&WindowDispatch::<u32>::primary_monitor(window).unwrap().unwrap(), primary));
  for monitor in displays {
    let point = query_point(monitor);
    assert!(same_monitor(&<ElectronRuntimeHandle as RuntimeHandle<u32>>::monitor_from_point(handle, point.0, point.1).unwrap().unwrap(), monitor));
    assert!(same_monitor(&WindowDispatch::<u32>::monitor_from_point(window, point.0, point.1).unwrap().unwrap(), monitor));
  }
  let outside = outside_point(displays);
  assert!(<ElectronRuntimeHandle as RuntimeHandle<u32>>::monitor_from_point(handle, outside.0, outside.1).unwrap().is_none());
  assert!(WindowDispatch::<u32>::monitor_from_point(window, outside.0, outside.1).unwrap().is_none());
  evidence(&["RuntimeHandle::available_monitors", "RuntimeHandle::primary_monitor", "RuntimeHandle::monitor_from_point", "WindowDispatch::primary_monitor", "WindowDispatch::monitor_from_point"],
    &format!("handle/window queries observe all {} actual displays; interior points select their containing displays and exterior points return None", displays.len()), "pass");
  println!("unverified Runtime::cursor_position: finite coordinates alone do not prove the OS pointer position");
}

fn physical_size(window: &Window, width: f64, height: f64) -> PhysicalSize<u32> {
  let scale = WindowDispatch::<u32>::scale_factor(window).unwrap();
  PhysicalSize::new((width * scale).round() as u32, (height * scale).round() as u32)
}
fn size_is(window: &Window, target: PhysicalSize<u32>) -> bool {
  WindowDispatch::<u32>::inner_size(window).is_ok_and(|got| got.width.abs_diff(target.width) <= 6 && got.height.abs_diff(target.height) <= 6)
}
fn resize(window: &Window, width: f64, height: f64) {
  WindowDispatch::<u32>::set_size(window, Size::Logical(LogicalSize::new(width, height))).unwrap();
}
fn focus(window: &Window) {
  WindowDispatch::<u32>::set_focus(window).unwrap();
  wait(|| WindowDispatch::<u32>::is_focused(window).unwrap(), "focus transitions to target window");
}
fn geometry_checks(window: &Window, page: &Webview, displays: &[Monitor]) {
  let monitor = WindowDispatch::<u32>::current_monitor(window).unwrap().unwrap();
  let area = monitor.work_area;
  let scale = monitor.scale_factor;
  for (dx, dy) in [(45, 50), (180, 160)] {
    let point = PhysicalPosition::new(area.position.x + dx, area.position.y + dy);
    WindowDispatch::<u32>::set_position(window, Position::Physical(point)).unwrap();
    wait(|| WindowDispatch::<u32>::outer_position(window).is_ok_and(|got| got.x.abs_diff(point.x) <= 8 && got.y.abs_diff(point.y) <= 8), "window reaches each physical screen position");
    let outer = WindowDispatch::<u32>::outer_position(window).unwrap();
    let inner = WindowDispatch::<u32>::inner_position(window).unwrap();
    let frame = WindowDispatch::<u32>::outer_size(window).unwrap();
    let content = WindowDispatch::<u32>::inner_size(window).unwrap();
    assert!(inner.x >= outer.x && inner.y >= outer.y);
    assert!(i64::from(inner.x) + i64::from(content.width) <= i64::from(outer.x) + i64::from(frame.width) + 6);
    assert!(i64::from(inner.y) + i64::from(content.height) <= i64::from(outer.y) + i64::from(frame.height) + 6);
  }
  evidence(&["WindowDispatch::set_position", "WindowDispatch::outer_position", "WindowDispatch::inner_position"],
    "two distinct physical positions are reached within 8px and the live client rectangle fits inside the actual outer frame at both positions", "pass");
  WindowDispatch::<u32>::center(window).unwrap();
  wait(|| {
    let position = WindowDispatch::<u32>::outer_position(window).unwrap();
    let size = WindowDispatch::<u32>::outer_size(window).unwrap();
    (f64::from(position.x) + f64::from(size.width) / 2.0 - f64::from(area.position.x) - f64::from(area.size.width) / 2.0).abs() < 15.0 * scale
      && (f64::from(position.y) + f64::from(size.height) / 2.0 - f64::from(area.position.y) - f64::from(area.size.height) / 2.0).abs() < 15.0 * scale
  }, "native window is centered in its monitor work area");
  evidence(&["WindowDispatch::center"], "the actual outer-frame center matches the current monitor work-area center within 15 logical pixels", "pass");
  WindowDispatch::<u32>::set_min_size(window, Some(Size::Logical(LogicalSize::new(280.0, 200.0)))).unwrap();
  resize(window, 160.0, 120.0);
  wait(|| size_is(window, physical_size(window, 280.0, 200.0)), "minimum clamps content size");
  WindowDispatch::<u32>::set_min_size(window, None).unwrap();
  resize(window, 190.0, 150.0);
  wait(|| size_is(window, physical_size(window, 190.0, 150.0)), "cleared minimum permits smaller content");
  evidence(&["WindowDispatch::set_min_size"], "setting the minimum clamps real content to 280x200; clearing it permits 190x150", "pass");
  WindowDispatch::<u32>::set_max_size(window, Some(Size::Logical(LogicalSize::new(470.0, 350.0)))).unwrap();
  resize(window, 650.0, 490.0);
  wait(|| size_is(window, physical_size(window, 470.0, 350.0)), "maximum clamps content size");
  WindowDispatch::<u32>::set_max_size(window, None).unwrap();
  resize(window, 520.0, 390.0);
  wait(|| size_is(window, physical_size(window, 520.0, 390.0)), "cleared maximum permits larger content");
  evidence(&["WindowDispatch::set_max_size"], "setting maximum clamps actual content to 470x350; clearing it permits 520x390", "pass");
  let constraints = WindowSizeConstraints { min_width: Some(PixelUnit::Logical(300.0.into())), max_height: Some(PixelUnit::Physical(physical_size(window, 0.0, 300.0).height.into())), ..Default::default() };
  WindowDispatch::<u32>::set_size_constraints(window, constraints).unwrap();
  resize(window, 180.0, 500.0);
  wait(|| size_is(window, physical_size(window, 300.0, 300.0)), "mixed physical/logical constraints apply");
  WindowDispatch::<u32>::set_size_constraints(window, WindowSizeConstraints::default()).unwrap();
  resize(window, 420.0, 320.0);
  wait(|| size_is(window, physical_size(window, 420.0, 320.0)), "constraints reset");
  expect_eval(page, "[innerWidth, innerHeight]", serde_json::json!([420, 320]));
  evidence(&["WindowDispatch::set_size_constraints"], "mixed-axis logical minimum width and physical maximum height constrain actual content; clearing restores a 420x320 page viewport", "pass");
  let normal = frame(window);
  WindowDispatch::<u32>::set_fullscreen(window, true).unwrap();
  wait(|| WindowDispatch::<u32>::is_fullscreen(window).unwrap(), "enter fullscreen");
  assert_fullscreen_size(window);
  WindowDispatch::<u32>::set_fullscreen(window, false).unwrap();
  restored(window, normal);
  evidence(&["WindowDispatch::set_fullscreen", "WindowDispatch::is_fullscreen"], "enter/leave fullscreen changes native state, spans the actual monitor and restores the prior normal position and size", "pass");
  for monitor in displays {
    let normal = frame(window);
    let target = PhysicalPosition::new(f64::from(monitor.position.x) + f64::from(monitor.size.width) / 2.0, f64::from(monitor.position.y) + f64::from(monitor.size.height) / 2.0);
    WindowDispatch::<u32>::set_fullscreen_on_monitor(window, target).unwrap();
    wait(|| WindowDispatch::<u32>::is_fullscreen(window).unwrap() && WindowDispatch::<u32>::current_monitor(window).unwrap().is_some_and(|got| same_monitor(&got, monitor)), "fullscreen reaches requested physical monitor");
    assert_fullscreen_size(window);
    WindowDispatch::<u32>::set_fullscreen(window, false).unwrap();
    restored(window, normal);
  }
  let normal = frame(window);
  let outside = outside_point(displays);
  WindowDispatch::<u32>::set_fullscreen_on_monitor(window, PhysicalPosition::new(outside.0, outside.1)).unwrap();
  thread::sleep(Duration::from_millis(750));
  restored(window, normal);
  evidence(&["WindowDispatch::set_fullscreen_on_monitor"], &format!("fullscreen physically selects each of {} available monitors and restores the prior normal frame on exit; outside all monitors changes neither state nor frame; cross-screen selection only exercised when more than one is present", displays.len()), "pass");
  #[cfg(target_os = "macos")]
  {
    let normal = frame(window);
    WindowDispatch::<u32>::set_simple_fullscreen(window, true).unwrap();
    wait(|| WindowDispatch::<u32>::is_fullscreen(window).unwrap(), "simple fullscreen enters");
    assert_fullscreen_size(window);
    WindowDispatch::<u32>::set_simple_fullscreen(window, false).unwrap();
    restored(window, normal);
    evidence(&["WindowDispatch::set_simple_fullscreen"], "simple fullscreen spans the monitor and restores the previous normal position and size on exit", "pass");
  }
}
fn frame(window: &Window) -> (PhysicalPosition<i32>, PhysicalSize<u32>) {
  (WindowDispatch::<u32>::outer_position(window).unwrap(), WindowDispatch::<u32>::outer_size(window).unwrap())
}
fn restored(window: &Window, normal: (PhysicalPosition<i32>, PhysicalSize<u32>)) {
  wait(|| {
    if WindowDispatch::<u32>::is_fullscreen(window).unwrap() { return false }
    let (position, size) = frame(window);
    position.x.abs_diff(normal.0.x) <= 8 && position.y.abs_diff(normal.0.y) <= 8
      && size.width.abs_diff(normal.1.width) <= 8 && size.height.abs_diff(normal.1.height) <= 8
  }, "leaving fullscreen restores its previous normal physical frame");
}
fn assert_fullscreen_size(window: &Window) {
  wait(|| {
    let monitor = WindowDispatch::<u32>::current_monitor(window).unwrap().unwrap();
    let size = WindowDispatch::<u32>::outer_size(window).unwrap();
    size.width.abs_diff(monitor.size.width) <= 12 && size.height.abs_diff(monitor.size.height) <= 12
  }, "fullscreen native frame spans display");
}

fn builder_checks(handle: &ElectronRuntimeHandle, anchor: &Window, displays: &[Monitor]) {
  let monitor = WindowDispatch::<u32>::current_monitor(anchor).unwrap().unwrap();
  let area = monitor.work_area;
  let scale = monitor.scale_factor;
  let centered = create(handle, ElectronWindowBuilder::new().inner_size(310.0, 230.0).center(), false);
  let pos = WindowDispatch::<u32>::outer_position(&centered.dispatcher).unwrap();
  let size = WindowDispatch::<u32>::outer_size(&centered.dispatcher).unwrap();
  assert!((f64::from(pos.x) + f64::from(size.width) / 2.0 - f64::from(area.position.x) - f64::from(area.size.width) / 2.0).abs() < 15.0 * scale);
  assert!((f64::from(pos.y) + f64::from(size.height) / 2.0 - f64::from(area.position.y) - f64::from(area.size.height) / 2.0).abs() < 15.0 * scale);
  evidence(&["WindowBuilder::center"], "a window created by center() has its live frame centered in its monitor work area", "pass");
  discard(&centered.dispatcher);
  let point = LogicalPosition::new(f64::from(area.position.x) / scale + 70.0, f64::from(area.position.y) / scale + 80.0);
  let target = PhysicalPosition::new((point.x * scale).round() as i32, (point.y * scale).round() as i32);
  let positioned = create(handle, ElectronWindowBuilder::new().inner_size(300.0, 220.0).position(point.x, point.y), false);
  wait(|| WindowDispatch::<u32>::outer_position(&positioned.dispatcher).is_ok_and(|got| got.x.abs_diff(target.x) <= 8 && got.y.abs_diff(target.y) <= 8), "builder initial position");
  evidence(&["WindowBuilder::position"], "an explicitly positioned new window starts at its requested logical-pixel location within 8 physical pixels", "pass");
  discard(&positioned.dispatcher);

  for (name, builder, requested, expected) in [
    ("WindowBuilder::min_inner_size", ElectronWindowBuilder::new().inner_size(320.0, 240.0).min_inner_size(260.0, 190.0), (120.0, 90.0), (260.0, 190.0)),
    ("WindowBuilder::max_inner_size", ElectronWindowBuilder::new().inner_size(320.0, 240.0).max_inner_size(450.0, 340.0), (700.0, 500.0), (450.0, 340.0)),
    ("WindowBuilder::inner_size_constraints", ElectronWindowBuilder::new().inner_size(320.0, 240.0).inner_size_constraints(WindowSizeConstraints {
      min_width: Some(PixelUnit::Logical(270.0.into())), max_height: Some(PixelUnit::Logical(300.0.into())), ..Default::default()
    }), (160.0, 500.0), (270.0, 300.0)),
  ] {
    let target = create(handle, builder, false);
    resize(&target.dispatcher, requested.0, requested.1);
    wait(|| size_is(&target.dispatcher, physical_size(&target.dispatcher, expected.0, expected.1)), "builder constraints affect real resizing");
    evidence(&[name], "the explicitly called builder constraint changes the actual content size when a later resize exceeds its limit", "pass");
    discard(&target.dispatcher);
  }
  for margin in [false, true] {
    let builder = ElectronWindowBuilder::new().inner_size(f64::from(area.size.width) / scale + 300.0, f64::from(area.size.height) / scale + 200.0);
    let builder = if margin { builder.prevent_overflow_with_margin(Size::Physical(PhysicalSize::new(100, 80))) } else { builder.prevent_overflow() };
    let target = create(handle, builder, false);
    let pos = WindowDispatch::<u32>::outer_position(&target.dispatcher).unwrap();
    let size = WindowDispatch::<u32>::outer_size(&target.dispatcher).unwrap();
    let actual = WindowDispatch::<u32>::current_monitor(&target.dispatcher).unwrap().unwrap().work_area;
    assert!(pos.x >= actual.position.x - 4 && pos.y >= actual.position.y - 4);
    assert!(i64::from(pos.x) + i64::from(size.width) <= i64::from(actual.position.x) + i64::from(actual.size.width) + 4);
    assert!(i64::from(pos.y) + i64::from(size.height) <= i64::from(actual.position.y) + i64::from(actual.size.height) + 4);
    if margin { assert!(size.width + 90 <= actual.size.width && size.height + 70 <= actual.size.height); }
    evidence(&[if margin { "WindowBuilder::prevent_overflow_with_margin" } else { "WindowBuilder::prevent_overflow" }],
      "an oversized initial window is actually clamped inside the monitor work area, with the requested margin when enabled", "pass");
    discard(&target.dispatcher);
  }
  let target = create(handle, ElectronWindowBuilder::new().resizable(false).title("builder-native-title").visible(false).inner_size(310.0, 220.0), false);
  assert!(!WindowDispatch::<u32>::is_resizable(&target.dispatcher).unwrap());
  assert!(!WindowDispatch::<u32>::is_visible(&target.dispatcher).unwrap());
  assert_eq!(WindowDispatch::<u32>::title(&target.dispatcher).unwrap(), "builder-native-title");
  evidence(&["WindowBuilder::resizable", "WindowBuilder::title", "WindowBuilder::visible"], "explicit builder setters create a hidden, non-resizable native window with its requested title", "pass");
  discard(&target.dispatcher);
  let top = create(handle, ElectronWindowBuilder::new().always_on_top(true).inner_size(320.0, 230.0), false);
  wait(|| WindowDispatch::<u32>::is_always_on_top(&top.dispatcher).unwrap(), "builder topmost native state");
  evidence(&["WindowBuilder::always_on_top"], "the live new window is already topmost before calling a topmost dispatcher setter", "pass");
  discard(&top.dispatcher);
  let maximum = create(handle, ElectronWindowBuilder::new().maximized(true), false);
  wait(|| WindowDispatch::<u32>::is_maximized(&maximum.dispatcher).unwrap(), "builder maximized native state");
  evidence(&["WindowBuilder::maximized"], "the newly created native window is maximized before any maximize dispatcher call", "pass");
  discard(&maximum.dispatcher);
  let full = create(handle, ElectronWindowBuilder::new().fullscreen(true), false);
  wait(|| WindowDispatch::<u32>::is_fullscreen(&full.dispatcher).unwrap(), "builder fullscreen native state");
  assert_fullscreen_size(&full.dispatcher);
  evidence(&["WindowBuilder::fullscreen"], "the newly created native window is fullscreen and its actual outer frame spans the current monitor", "pass");
  discard(&full.dispatcher);

  focus(anchor);
  let inactive = create(handle, ElectronWindowBuilder::new().focused(false).inner_size(300.0, 220.0), false);
  thread::sleep(Duration::from_millis(200));
  assert!(!WindowDispatch::<u32>::is_focused(&inactive.dispatcher).unwrap());
  let active = create(handle, ElectronWindowBuilder::new().focused(true).inner_size(300.0, 220.0), false);
  wait(|| WindowDispatch::<u32>::is_focused(&active.dispatcher).unwrap() && !WindowDispatch::<u32>::is_focused(&inactive.dispatcher).unwrap(), "builder focus true transfers focus");
  evidence(&["WindowBuilder::focused"], "a focused(false) new window stays unfocused while a separate focused(true) new window acquires native focus", "pass");
  focus(anchor);
  let unfocusable = create(handle, ElectronWindowBuilder::new().focusable(false).focused(false).inner_size(310.0, 220.0), false);
  WindowDispatch::<u32>::set_focus(&unfocusable.dispatcher).unwrap();
  thread::sleep(Duration::from_millis(250));
  assert!(!WindowDispatch::<u32>::is_focused(&unfocusable.dispatcher).unwrap());
  WindowDispatch::<u32>::set_focusable(&unfocusable.dispatcher, true).unwrap();
  focus(&unfocusable.dispatcher);
  assert!(!WindowDispatch::<u32>::is_focused(anchor).unwrap());
  focus(anchor);
  WindowDispatch::<u32>::set_focusable(&unfocusable.dispatcher, false).unwrap();
  WindowDispatch::<u32>::set_focus(&unfocusable.dispatcher).unwrap();
  thread::sleep(Duration::from_millis(250));
  assert!(!WindowDispatch::<u32>::is_focused(&unfocusable.dispatcher).unwrap());
  evidence(&["WindowBuilder::focusable", "WindowDispatch::set_focusable", "WindowDispatch::set_focus"],
    "focusable(false) prevents native focus; enabling it permits a real transfer from another window, disabling prevents focus again", "pass");
  for window in [&inactive.dispatcher, &active.dispatcher, &unfocusable.dispatcher] { discard(window); }

  let light = create(handle, ElectronWindowBuilder::new().theme(Some(Theme::Light)), true);
  expect_eval(&light.webview.as_ref().unwrap().webview.dispatcher, "matchMedia('(prefers-color-scheme: dark)').matches", serde_json::json!(false));
  discard(&light.dispatcher);
  let builder = ElectronWindowBuilder::new();
  assert_eq!(builder.get_theme(), None);
  let builder = builder.theme(Some(Theme::Light));
  assert_eq!(builder.get_theme(), Some(Theme::Light));
  let builder = builder.theme(Some(Theme::Dark));
  assert_eq!(builder.get_theme(), Some(Theme::Dark));
  assert!(!builder.has_icon());
  let builder = builder.icon(Icon { rgba: Cow::Owned(vec![220, 30, 10, 255].repeat(4)), width: 2, height: 2 }).unwrap();
  assert!(builder.has_icon());
  let themed = create(handle, builder, true);
  let themed_page = &themed.webview.as_ref().unwrap().webview.dispatcher;
  expect_eval(themed_page, "matchMedia('(prefers-color-scheme: dark)').matches", serde_json::json!(true));
  assert_eq!(WindowDispatch::<u32>::theme(&themed.dispatcher).unwrap(), Theme::Dark);
  evidence(&["WindowBuilder::theme"], "two separately constructed theme(Light)/theme(Dark) windows produce false/true from the live page matchMedia prefers-color-scheme query", "pass");
  evidence(&["WindowBuilder::has_icon", "WindowBuilder::get_theme"], "within real window creation has_icon changes false to true and get_theme changes None to Light to Dark; this does not assert native icon rendering", "pass");
  discard(&themed.dispatcher);
  println!("unverified WindowBuilder::background_color: native backing pixels need an independent compositor probe");
  println!("desktop behavior assertions completed on {} actual displays", displays.len());
}
