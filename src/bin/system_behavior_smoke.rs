//! Real system cursor and application-theme behavior. No cached options certify effects.
//! Run on a desktop; unavailable OS pointer observation is reported as unknown.
use serde_json::{json, Value};
use std::{
  panic::{catch_unwind, resume_unwind, AssertUnwindSafe},
  process::ExitCode,
  sync::mpsc,
  thread,
  time::{Duration, Instant},
};
use tauri_runtime::{
  dpi::{LogicalPosition, PhysicalPosition, Position},
  webview::{PendingWebview, WebviewAttributes},
  window::{PendingWindow, RawWindow, WindowBuilder},
  ExitRequestedEventAction, RunEvent, Runtime, RuntimeHandle, RuntimeInitArgs,
  WebviewDispatch, WindowDispatch,
};
use tauri_runtime_electron::{
  ElectronRuntime, ElectronRuntimeHandle, ElectronWebviewDispatcher as Webview,
  ElectronWindowBuilder, ElectronWindowDispatcher as Window,
};
use tauri_utils::{config::{WebviewUrl, WindowConfig}, Theme};

const WAIT: Duration = Duration::from_secs(8);
type Point = (f64, f64);
type Page = (Window, Webview);

// The worker observes pages while the owning thread calls the actual Runtime
// methods. Callback evaluation needs a live UI pump, including before run_return.
enum DirectRequest {
  Cursor(mpsc::SyncSender<tauri_runtime::Result<PhysicalPosition<f64>>>),
  Theme(Option<Theme>, mpsc::SyncSender<()>),
  Done,
}

fn main() -> ExitCode {
  thread::spawn(|| {
    thread::sleep(Duration::from_secs(120));
    eprintln!("system behavior smoke exceeded 120s");
    std::process::abort();
  });
  let mut runtime = <ElectronRuntime as Runtime<()>>::new(RuntimeInitArgs::default()).expect("start real Electron");
  let handle = <ElectronRuntime as Runtime<()>>::handle(&runtime);
  let first = create(&handle, "system-first", 60.0);
  let second = create(&handle, "system-second", 430.0);
  let (direct_tx, direct_rx) = mpsc::channel();
  let (continue_tx, continue_rx) = mpsc::sync_channel(1);
  let worker = thread::spawn(move || {
    let direct = catch_unwind(AssertUnwindSafe(|| {
      ready(&first.1, "system-first");
      ready(&second.1, "system-second");
      cursor_checks("Runtime::cursor_position", &first.0, || {
        let (tx, rx) = mpsc::sync_channel(1);
        direct_tx.send(DirectRequest::Cursor(tx)).unwrap();
        rx.recv_timeout(WAIT).expect("owning thread must query Runtime cursor")
      });
      theme_checks("Runtime::set_theme", &first.1, &second.1, |theme| {
        let (tx, rx) = mpsc::sync_channel(1);
        direct_tx.send(DirectRequest::Theme(theme, tx)).unwrap();
        rx.recv_timeout(WAIT).expect("owning thread must set Runtime theme");
      });
    }));
    direct_tx.send(DirectRequest::Done).unwrap();
    continue_rx.recv_timeout(WAIT).expect("start run_return for handle checks");
    let result = direct.and_then(|()| catch_unwind(AssertUnwindSafe(|| {
      theme_checks("RuntimeHandle::set_theme", &first.1, &second.1,
        |theme| <ElectronRuntimeHandle as RuntimeHandle<()>>::set_theme(&handle, theme));
      cursor_checks("RuntimeHandle::cursor_position", &first.0,
        || <ElectronRuntimeHandle as RuntimeHandle<()>>::cursor_position(&handle));
      window_cursor_checks(&first.0);
    })));
    // Fresh runtime theme starts at system; restore it even after a failed assertion.
    <ElectronRuntimeHandle as RuntimeHandle<()>>::set_theme(&handle, None);
    let _ = WindowDispatch::<()>::destroy(&first.0);
    let _ = WindowDispatch::<()>::destroy(&second.0);
    // Automatic last-window exit can race this send. Keep the assertion result.
    let _ = <ElectronRuntimeHandle as RuntimeHandle<()>>::request_exit(&handle, i32::from(result.is_err()));
    result
  });
  loop {
    match direct_rx.try_recv() {
      Ok(DirectRequest::Cursor(tx)) => {
        let _ = tx.send(<ElectronRuntime as Runtime<()>>::cursor_position(&runtime));
      }
      Ok(DirectRequest::Theme(theme, tx)) => {
        <ElectronRuntime as Runtime<()>>::set_theme(&runtime, theme);
        let _ = tx.send(());
      }
      Ok(DirectRequest::Done) => break,
      Err(mpsc::TryRecvError::Empty) => {}
      Err(mpsc::TryRecvError::Disconnected) => panic!("direct runtime worker disconnected"),
    }
    <ElectronRuntime as Runtime<()>>::run_iteration(&mut runtime, |_| {});
    thread::sleep(Duration::from_millis(3));
  }
  continue_tx.send(()).expect("continue system checks");
  let code = <ElectronRuntime as Runtime<()>>::run_return(runtime, |event| {
    if let RunEvent::ExitRequested { code: None, tx } = event {
      let _ = tx.send(ExitRequestedEventAction::Prevent);
    }
  });
  if worker.join().expect("join system behavior worker").is_ok() && code == 0 {
    ExitCode::SUCCESS
  } else {
    ExitCode::FAILURE
  }
}

fn create(handle: &ElectronRuntimeHandle, label: &str, x: f64) -> Page {
  let config = WindowConfig {
    title: label.into(), width: 320.0, height: 240.0, x: Some(x), y: Some(80.0),
    visible: true, always_on_top: cfg!(windows), theme: None,
    ..Default::default()
  };
  let mut pending = PendingWindow::<(), ElectronRuntime>::new(ElectronWindowBuilder::with_config(&config), label).unwrap();
  let url = format!(r#"data:text/html,<!doctype html><title>{label}</title><body data-proof='{label}'><script>window.themeChanges=[];matchMedia('(prefers-color-scheme: dark)').addEventListener('change',e=>themeChanges.push(e.matches));</script>independent real theme page</body>"#);
  let mut page = PendingWebview::new(WebviewAttributes::new(WebviewUrl::External(url.parse().unwrap())), label).unwrap();
  page.url = url;
  pending.set_webview(page);
  let window = <ElectronRuntimeHandle as RuntimeHandle<()>>::create_window(handle, pending, None::<fn(RawWindow<'_>)>).unwrap();
  (window.dispatcher, window.webview.unwrap().webview.dispatcher)
}

fn eval(page: &Webview, script: &str) -> Value {
  let (tx, rx) = mpsc::channel();
  WebviewDispatch::<()>::eval_script_with_callback(page, script, move |value| { let _ = tx.send(value); }).expect("evaluate real page");
  serde_json::from_str(&rx.recv_timeout(WAIT).expect("real page evaluation timed out")).expect("JSON page result")
}

fn wait(description: &str, mut check: impl FnMut() -> bool) {
  let start = Instant::now();
  while !check() {
    assert!(start.elapsed() < WAIT, "{description}");
    thread::sleep(Duration::from_millis(30));
  }
}

fn ready(page: &Webview, proof: &str) {
  wait("independent page must load its own proof", || eval(page, "document.body?.dataset.proof") == proof);
}

fn evidence(member: &str, assertion: &str, status: &str) {
  println!("[runtime-evidence] {}", json!({
    "member": member, "test": "system_behavior_smoke", "assertion": assertion,
    "kind": "behavior", "status": status,
  }));
}

fn media(page: &Webview) -> Value {
  eval(page, "({dark:matchMedia('(prefers-color-scheme: dark)').matches,light:matchMedia('(prefers-color-scheme: light)').matches,changes:themeChanges})")
}

fn theme_checks(member: &str, first: &Webview, second: &Webview, set: impl Fn(Option<Theme>)) {
  let baseline = media(first)["dark"].as_bool().expect("real dark media state");
  assert_eq!(media(second)["dark"], baseline, "fresh pages must observe the same system scheme");
  let result = catch_unwind(AssertUnwindSafe(|| {
    for dark in [!baseline, baseline] {
      let from_first = media(first)["changes"].as_array().unwrap().len();
      let from_second = media(second)["changes"].as_array().unwrap().len();
      set(Some(if dark { Theme::Dark } else { Theme::Light }));
      for (page, from) in [(first, from_first), (second, from_second)] {
        wait("both real pages must update dark/light media queries and dispatch a change event", || {
          let state = media(page);
          state["dark"] == dark && state["light"] == !dark
            && state["changes"].as_array().unwrap()[from..].iter().any(|value| *value == dark)
        });
      }
    }
  }));
  // Only the isolated Electron process's app override changed, never OS settings.
  set(None);
  for page in [first, second] {
    wait("return both pages to their original system color scheme", || media(page)["dark"] == baseline);
  }
  if let Err(error) = result { resume_unwind(error); }
  evidence(member, "two independently loaded pages both switch dark/light matchMedia values and receive change events for opposite app-theme requests; system theme selection is restored", "pass");
}

fn distance(a: Point, b: Point) -> f64 { (a.0 - b.0).abs().max((a.1 - b.1).abs()) }

fn cursor_targets(window: &Window) -> (f64, [(Position, Point); 2]) {
  let scale = WindowDispatch::<()>::scale_factor(window).unwrap();
  let origin = WindowDispatch::<()>::inner_position(window).unwrap();
  let size = WindowDispatch::<()>::inner_size(window).unwrap();
  assert!(scale.is_finite() && scale > 0.0 && size.width > 80 && size.height > 80);
  let first = PhysicalPosition::new((size.width / 3) as i32, (size.height / 3) as i32);
  let second = PhysicalPosition::new((size.width * 2 / 3) as i32, (size.height * 2 / 3) as i32);
  let screen = |offset: PhysicalPosition<i32>| (f64::from(origin.x + offset.x), f64::from(origin.y + offset.y));
  (scale, [(Position::Physical(first), screen(first)),
    (Position::Logical(LogicalPosition::new(f64::from(second.x) / scale, f64::from(second.y) / scale)), screen(second))])
}

fn raw_point(physical: Point, scale: f64) -> Point {
  if cfg!(target_os = "macos") { (physical.0 / scale, physical.1 / scale) } else { physical }
}
fn physical_point(raw: Point, scale: f64) -> Point {
  if cfg!(target_os = "macos") { (raw.0 * scale, raw.1 * scale) } else { raw }
}

fn observe_pointer(target: Point, scale: f64) -> Result<Point, String> {
  let start = Instant::now();
  loop {
    let observed = physical_point(os_pointer::read()?, scale);
    if distance(observed, target) <= 2.0 { return Ok(observed) }
    if start.elapsed() > WAIT { return Err(format!("OS pointer did not reach {target:?}; independently read {observed:?} (permission, desktop or human input interference)")) }
    thread::sleep(Duration::from_millis(25));
  }
}

fn restore_pointer(original: Point) {
  // A denied warp may have left the pointer untouched; do not turn that
  // unavailable stimulus into a false restoration failure.
  if os_pointer::read().is_ok_and(|actual| distance(actual, original) <= 2.0) { return }
  os_pointer::move_to(original).expect("restore original OS pointer position");
  wait("independent OS reader must observe restored cursor", || os_pointer::read().is_ok_and(|actual| distance(actual, original) <= 2.0));
}

fn cursor_checks(member: &str, window: &Window, query: impl Fn() -> tauri_runtime::Result<PhysicalPosition<f64>>) {
  let original = match os_pointer::read() {
    Ok(point) => point,
    Err(error) => { evidence(member, &format!("independent OS pointer observation unavailable: {error}"), "unknown"); return }
  };
  let (scale, targets) = cursor_targets(window);
  let result = catch_unwind(AssertUnwindSafe(|| -> Result<(), String> {
    for (_, target) in targets {
      // Independent native stimulus, not another runtime IPC getter/setter.
      os_pointer::move_to(raw_point(target, scale))?;
      let before = observe_pointer(target, scale)?;
      let actual = query().expect("runtime cursor query must succeed after native movement");
      let after = physical_point(os_pointer::read()?, scale);
      if distance(before, after) > 2.0 { return Err("pointer moved during comparison; no stable independent sample".into()) }
      assert!(distance((actual.x, actual.y), after) <= 2.0,
        "{member}: runtime={actual:?}, independent OS={after:?}, target={target:?}");
      println!("system pointer {member}: runtime={actual:?}, independent OS={after:?}");
    }
    Ok(())
  }));
  restore_pointer(original);
  match result {
    Ok(Ok(())) => evidence(member, "two distinct real OS pointer movements are independently read by Win32/CoreGraphics/X11 and agree with this API's physical screen coordinates; original cursor restored", "pass"),
    Ok(Err(error)) => evidence(member, &error, "unknown"),
    Err(error) => resume_unwind(error),
  }
}

fn window_cursor_checks(window: &Window) {
  let member = "WindowDispatch::set_cursor_position";
  let original = match os_pointer::read() {
    Ok(point) => point,
    Err(error) => { evidence(member, &format!("independent OS pointer observation unavailable: {error}"), "unknown"); return }
  };
  let (scale, targets) = cursor_targets(window);
  let result = catch_unwind(AssertUnwindSafe(|| -> Result<(), String> {
    // Put the pointer elsewhere first so a no-op cannot satisfy either target.
    os_pointer::move_to(raw_point((targets[0].1.0 + 15.0, targets[0].1.1 + 15.0), scale))?;
    for (request, target) in targets {
      if let Err(error) = WindowDispatch::<()>::set_cursor_position(window, request) {
        return Err(format!("window cursor movement unavailable on this desktop: {error}"));
      }
      let actual = observe_pointer(target, scale)
        .unwrap_or_else(|error| panic!("successful cursor setter did not move the actual OS pointer: {error}"));
      println!("system window-relative pointer: request={request:?}, independent OS={actual:?}");
    }
    Ok(())
  }));
  restore_pointer(original);
  match result {
    Ok(Ok(())) => evidence(member, "physical and logical client offsets move the real OS pointer to two distinct expected screen points, confirmed independently rather than through IPC; original cursor restored", "pass"),
    Ok(Err(error)) => evidence(member, &error, "unknown"),
    Err(error) => resume_unwind(error),
  }
}

#[cfg(windows)]
mod os_pointer {
  use super::Point;
  use windows::Win32::{Foundation::POINT, UI::WindowsAndMessaging::{GetPhysicalCursorPos, SetPhysicalCursorPos}};

  // Physical variants avoid DPI virtualization in the Rust observer process.
  pub fn read() -> Result<Point, String> {
    let mut point = POINT::default();
    unsafe { GetPhysicalCursorPos(&mut point) }.map_err(|error| error.to_string())?;
    Ok((f64::from(point.x), f64::from(point.y)))
  }
  pub fn move_to(point: Point) -> Result<(), String> {
    unsafe { SetPhysicalCursorPos(point.0.round() as i32, point.1.round() as i32) }.map_err(|error| error.to_string())
  }
}

#[cfg(target_os = "macos")]
mod os_pointer {
  use super::Point;
  use std::ffi::c_void;
  #[repr(C)]
  struct CGPoint { x: f64, y: f64 }
  #[link(name = "CoreGraphics", kind = "framework")]
  extern "C" {
    fn CGEventCreate(source: *const c_void) -> *const c_void;
    fn CGEventGetLocation(event: *const c_void) -> CGPoint;
    fn CGWarpMouseCursorPosition(point: CGPoint) -> i32;
    fn CGAssociateMouseAndMouseCursorPosition(connected: bool) -> i32;
  }
  #[link(name = "CoreFoundation", kind = "framework")]
  extern "C" { fn CFRelease(object: *const c_void); }

  pub fn read() -> Result<Point, String> {
    // This is an actual event allocated by CoreGraphics, never an Electron pointer.
    let event = unsafe { CGEventCreate(std::ptr::null()) };
    if event.is_null() { return Err("CGEventCreate unavailable or permission denied".into()) }
    let point = unsafe { CGEventGetLocation(event) };
    unsafe { CFRelease(event); }
    Ok((point.x, point.y))
  }
  pub fn move_to(point: Point) -> Result<(), String> {
    let result = unsafe { CGWarpMouseCursorPosition(CGPoint { x: point.0, y: point.1 }) };
    if result != 0 { return Err(format!("CGWarpMouseCursorPosition: {result}")) }
    let result = unsafe { CGAssociateMouseAndMouseCursorPosition(true) };
    if result != 0 { return Err(format!("CGAssociateMouseAndMouseCursorPosition: {result}")) }
    Ok(())
  }
}

#[cfg(target_os = "linux")]
mod os_pointer {
  use super::Point;
  use std::process::Command;

  fn run(args: &[&str]) -> Result<String, String> {
    if std::env::var_os("DISPLAY").is_none() || std::env::var_os("WAYLAND_DISPLAY").is_some() {
      return Err("requires an X11 desktop; Wayland/Xwayland cannot independently certify the system pointer".into())
    }
    // xdotool reads via XQueryPointer and moves via XWarpPointer, independent of Electron.
    let output = Command::new("xdotool").args(args).output().map_err(|error| format!("xdotool unavailable: {error}"))?;
    if !output.status.success() { return Err(format!("xdotool {args:?}: {}", String::from_utf8_lossy(&output.stderr))) }
    String::from_utf8(output.stdout).map_err(|error| error.to_string())
  }
  pub fn read() -> Result<Point, String> {
    let output = run(&["getmouselocation", "--shell"])?;
    let axis = |key: &str| output.lines().find_map(|line| line.strip_prefix(key))
      .and_then(|value| value.parse::<f64>().ok()).ok_or_else(|| format!("missing {key} in xdotool pointer output"));
    Ok((axis("X=")?, axis("Y=")?))
  }
  pub fn move_to(point: Point) -> Result<(), String> {
    run(&["mousemove", &format!("{:.0}", point.0), &format!("{:.0}", point.1)]).map(|_| ())
  }
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
mod os_pointer {
  use super::Point;
  pub fn read() -> Result<Point, String> { Err("no independent native pointer observer for this target".into()) }
  pub fn move_to(_: Point) -> Result<(), String> { Err("no native pointer mover for this target".into()) }
}
