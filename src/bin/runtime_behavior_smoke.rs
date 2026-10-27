//! Real window callbacks and Win32 frame/button effects; setup APIs earn no evidence.
use std::{
  process::ExitCode,
  sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}, mpsc},
  thread::{self, ThreadId},
  time::{Duration, Instant},
};
use tauri_runtime::{
  ExitRequestedEventAction, RunEvent, Runtime, RuntimeHandle, RuntimeInitArgs,
  WebviewDispatch, WindowDispatch,
  dpi::{LogicalSize, Size},
  webview::{PendingWebview, WebviewAttributes},
  window::{PendingWindow, RawWindow, WindowBuilder, WindowEvent},
};
use tauri_runtime_electron::{
  ElectronRuntime, ElectronRuntimeHandle, ElectronWebviewDispatcher as Webview,
  ElectronWindowBuilder, ElectronWindowDispatcher as Window,
};
use tauri_utils::config::{WebviewUrl, WindowConfig};

const WAIT: Duration = Duration::from_secs(8);
type Events = Arc<Mutex<Vec<(ThreadId, WindowEvent)>>>;

fn main() -> ExitCode {
  let runtime = <ElectronRuntime as Runtime<()>>::new(RuntimeInitArgs::default()).expect("start Electron");
  let handle = <ElectronRuntime as Runtime<()>>::handle(&runtime);
  let ui_thread = thread::current().id();
  let first = create(&handle, "callback-first");
  let second = create(&handle, "callback-second");
  thread::spawn(|| {
    thread::sleep(Duration::from_secs(100));
    eprintln!("window callback/native frame regression exceeded 100s");
    std::process::abort();
  });
  let worker = thread::spawn(move || {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
      exercise(&handle, &first, &second, ui_thread);
    }));
    let _ = WindowDispatch::<()>::destroy(&first.0);
    let _ = WindowDispatch::<()>::destroy(&second.0);
    let _ = <ElectronRuntimeHandle as RuntimeHandle<()>>::request_exit(&handle, i32::from(result.is_err()));
    result
  });
  let status = <ElectronRuntime as Runtime<()>>::run_return(runtime, |event| {
    if let RunEvent::ExitRequested { code: None, tx } = event {
      let _ = tx.send(ExitRequestedEventAction::Prevent);
    }
  });
  if worker.join().expect("join window behavior worker").is_ok() && status == 0 {
    ExitCode::SUCCESS
  } else {
    ExitCode::FAILURE
  }
}

fn create(handle: &ElectronRuntimeHandle, label: &str) -> (Window, Webview) {
  let config = WindowConfig {
    title: label.into(), width: 300.0, height: 220.0, visible: true,
    decorations: true, resizable: true, maximizable: true, minimizable: true, closable: true,
    ..Default::default()
  };
  create_with_builder(handle, label, ElectronWindowBuilder::with_config(&config))
}

fn create_with_builder(handle: &ElectronRuntimeHandle, label: &str, builder: ElectronWindowBuilder) -> (Window, Webview) {
  let mut pending = PendingWindow::<(), ElectronRuntime>::new(builder, label).unwrap();
  let url = format!("data:text/html,<title>{label}</title><p>real callback page</p>");
  let mut webview = PendingWebview::new(WebviewAttributes::new(WebviewUrl::External(url.parse().unwrap())), label).unwrap();
  webview.url = url;
  pending.set_webview(webview);
  let window = <ElectronRuntimeHandle as RuntimeHandle<()>>::create_window(handle, pending, None::<fn(RawWindow<'_>)>).unwrap();
  (window.dispatcher, window.webview.unwrap().webview.dispatcher)
}

fn wait(description: &str, mut check: impl FnMut() -> bool) {
  let start = Instant::now();
  while !check() {
    assert!(start.elapsed() < WAIT, "{description}");
    thread::sleep(Duration::from_millis(25));
  }
}

fn eval(webview: &Webview, script: &str) -> serde_json::Value {
  let (tx, rx) = mpsc::channel();
  WebviewDispatch::<()>::eval_script_with_callback(webview, script, move |value| { let _ = tx.send(value); }).unwrap();
  serde_json::from_str(&rx.recv_timeout(WAIT).expect("page evaluation timed out")).unwrap()
}

fn evidence(member: &str, assertion: &str) {
  report(member, assertion, "pass");
}

fn report(member: &str, assertion: &str, status: &str) {
  println!("[runtime-evidence] {}", serde_json::json!({
    "member": member, "test": "runtime_behavior_smoke::exercise", "assertion": assertion,
    "kind": "behavior", "status": status,
  }));
}

fn exercise(handle: &ElectronRuntimeHandle, first: &(Window, Webview), second: &(Window, Webview), ui_thread: ThreadId) {
  assert_ne!(thread::current().id(), ui_thread);
  wait("first page must load", || eval(&first.1, "document.title") == "callback-first");
  wait("second page must load", || eval(&second.1, "document.title") == "callback-second");

  let (tx, rx) = mpsc::channel();
  let target = first.0.clone();
  WindowDispatch::<()>::run_on_main_thread(&first.0, move || {
    let result = WindowDispatch::<()>::set_title(&target, "callback-ui-task")
      .and_then(|()| WindowDispatch::<()>::title(&target));
    tx.send((thread::current().id(), result)).unwrap();
  }).unwrap();
  let (actual_thread, title) = rx.recv_timeout(WAIT).expect("window UI task did not run");
  assert_eq!(actual_thread, ui_thread);
  assert_eq!(title.unwrap(), "callback-ui-task");
  assert_eq!(WindowDispatch::<()>::title(&second.0).unwrap(), "callback-second");
  evidence("WindowDispatch::run_on_main_thread", "a worker-scheduled closure ran on the runtime UI ThreadId, changed/read the first real window title, and left the second window unchanged");

  let first_events: Events = Default::default();
  let second_events: Events = Default::default();
  let veto = Arc::new(AtomicBool::new(true));
  let observed = first_events.clone();
  let prevent = veto.clone();
  WindowDispatch::<()>::on_window_event(&first.0, move |event| {
    if let WindowEvent::CloseRequested { signal_tx } = event {
      if prevent.load(Ordering::SeqCst) { signal_tx.send(true).unwrap(); }
    }
    observed.lock().unwrap().push((thread::current().id(), event.clone()));
  });
  let observed = second_events.clone();
  WindowDispatch::<()>::on_window_event(&second.0, move |event| {
    observed.lock().unwrap().push((thread::current().id(), event.clone()));
  });

  // Resize is only the stimulus; no geometry/setter member is claimed here.
  for (width, height) in [(420.0, 310.0), (350.0, 260.0)] {
    let scale = WindowDispatch::<()>::scale_factor(&first.0).unwrap();
    let wanted = LogicalSize::new(width, height).to_physical::<u32>(scale);
    let from = first_events.lock().unwrap().len();
    let peer_from = second_events.lock().unwrap().len();
    WindowDispatch::<()>::set_size(&first.0, Size::Logical(LogicalSize::new(width, height))).unwrap();
    wait("registered resize callback must contain the requested physical client size", || {
      first_events.lock().unwrap()[from..].iter().any(|(_, event)| matches!(event,
        WindowEvent::Resized(size) if size.width.abs_diff(wanted.width) <= 8 && size.height.abs_diff(wanted.height) <= 8))
    });
    let size = WindowDispatch::<()>::inner_size(&first.0).unwrap();
    assert!(size.width.abs_diff(wanted.width) <= 8 && size.height.abs_diff(wanted.height) <= 8);
    assert!(!second_events.lock().unwrap()[peer_from..].iter().any(|(_, event)| matches!(event,
      WindowEvent::Resized(size) if size.width.abs_diff(wanted.width) <= 8 && size.height.abs_diff(wanted.height) <= 8)),
      "first-window resize must not be delivered to the second window's listener");
  }

  builder_frame_checks(handle);
  #[cfg(windows)]
  native_frame_checks(&first.0);
  #[cfg(any(target_os = "macos", target_os = "linux"))]
  unix_frame_checks(&first.0);

  WindowDispatch::<()>::close(&first.0).unwrap();
  wait("CloseRequested callback must reach its registered window", || first_events.lock().unwrap().iter()
    .any(|(_, event)| matches!(event, WindowEvent::CloseRequested { .. })));
  thread::sleep(Duration::from_millis(250));
  assert_eq!(eval(&first.1, "document.title"), "callback-first", "veto must keep the real page alive");
  assert!(!first_events.lock().unwrap().iter().any(|(_, event)| matches!(event, WindowEvent::Destroyed)));
  assert!(!second_events.lock().unwrap().iter().any(|(_, event)| matches!(event, WindowEvent::CloseRequested { .. } | WindowEvent::Destroyed)));
  veto.store(false, Ordering::SeqCst);
  WindowDispatch::<()>::close(&first.0).unwrap();
  wait("allowing close must deliver Destroyed", || first_events.lock().unwrap().iter()
    .any(|(_, event)| matches!(event, WindowEvent::Destroyed)));
  assert_eq!(first_events.lock().unwrap().iter().filter(|(_, event)| matches!(event, WindowEvent::Destroyed)).count(), 1);
  assert!(first_events.lock().unwrap().iter().all(|(thread, _)| *thread == ui_thread), "window events must run on the UI thread");
  assert_eq!(eval(&second.1, "document.title"), "callback-second", "the peer must survive another window's close");
  assert!(!second_events.lock().unwrap().iter().any(|(_, event)| matches!(event, WindowEvent::Destroyed)));
  evidence("WindowDispatch::on_window_event", "registered listener receives two actual resize payloads and CloseRequested/Destroyed on the UI thread; veto preserves the real page, final Destroyed occurs once, and the peer listener/page stays isolated");
}

/// Independent native observations; unavailable tools/AX permission never earn pass.
fn settled_native_flag(window: &Window, flag: &str, wanted: bool) -> Result<bool, String> {
  let start = Instant::now();
  loop {
    let actual = native_flag(window, flag)?;
    if actual == wanted || start.elapsed() >= WAIT { return Ok(actual); }
    thread::sleep(Duration::from_millis(100));
  }
}

fn flag_getter(window: &Window, flag: &str) -> bool {
  match flag {
    "decorations" => WindowDispatch::<()>::is_decorated(window),
    "maximizable" => WindowDispatch::<()>::is_maximizable(window),
    "minimizable" => WindowDispatch::<()>::is_minimizable(window),
    "closable" => WindowDispatch::<()>::is_closable(window),
    _ => unreachable!(),
  }.unwrap()
}

fn builder_frame_checks(handle: &ElectronRuntimeHandle) {
  for (flag, member, getter) in [
    ("decorations", "WindowBuilder::decorations", "WindowDispatch::is_decorated"),
    ("maximizable", "WindowBuilder::maximizable", "WindowDispatch::is_maximizable"),
    ("minimizable", "WindowBuilder::minimizable", "WindowDispatch::is_minimizable"),
    ("closable", "WindowBuilder::closable", "WindowDispatch::is_closable"),
  ] {
    if cfg!(target_os = "linux") && matches!(flag, "maximizable" | "minimizable") {
      println!("skip {member}: official Linux contract is unsupported");
      continue;
    }
    let mut unknown = None;
    for wanted in [false, true] {
      let title = format!("builder-{flag}-{wanted}-{}", std::process::id());
      let mut config = WindowConfig {
        title: title.clone(), width: 300.0, height: 220.0, visible: true,
        decorations: true, resizable: true, maximizable: true, minimizable: true, closable: true,
        ..Default::default()
      };
      // Opposite config values catch a builder setter that silently drops its value.
      match flag {
        "decorations" => config.decorations = !wanted,
        "maximizable" => config.maximizable = !wanted,
        "minimizable" => config.minimizable = !wanted,
        "closable" => config.closable = !wanted,
        _ => unreachable!(),
      }
      let builder = ElectronWindowBuilder::with_config(&config);
      let builder = match flag {
        "decorations" => builder.decorations(wanted),
        "maximizable" => builder.maximizable(wanted),
        "minimizable" => builder.minimizable(wanted),
        "closable" => builder.closable(wanted),
        _ => unreachable!(),
      };
      let (window, webview) = create_with_builder(handle, &title, builder);
      wait("builder comparison page must load", || eval(&webview, "document.title") == title);
      match settled_native_flag(&window, flag, wanted) {
        Ok(actual) if actual == wanted => {
          assert_eq!(flag_getter(&window, flag), actual, "{getter} must agree with native {flag}");
        }
        Ok(actual) => {
          let reason = format!("{flag}({wanted}) produced native {actual}; platform behavior is not verified");
          panic!("{member}: {reason}");
        }
        Err(reason) => { unknown = Some(reason); }
      }
      WindowDispatch::<()>::destroy(&window).unwrap();
    }
    let status = if unknown.is_some() { "unknown" } else { "pass" };
    let assertion = match unknown {
      Some(reason) => format!("builder false/true native comparison unavailable: {reason}"),
      None => format!("fresh windows built with {flag}(false) and {flag}(true) override opposite config values; native frame/button state and getter both match"),
    };
    report(member, &assertion, status);
    report(getter, &format!("builder native comparison: {assertion}"), status);
  }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn unix_frame_checks(window: &Window) {
  let old_title = WindowDispatch::<()>::title(window).unwrap();
  WindowDispatch::<()>::set_title(window, format!("native-buttons-{}", std::process::id())).unwrap();
  for (flag, setter, getter) in [
    ("decorations", "WindowDispatch::set_decorations", "WindowDispatch::is_decorated"),
    ("maximizable", "WindowDispatch::set_maximizable", "WindowDispatch::is_maximizable"),
    ("minimizable", "WindowDispatch::set_minimizable", "WindowDispatch::is_minimizable"),
    ("closable", "WindowDispatch::set_closable", "WindowDispatch::is_closable"),
  ] {
    if cfg!(target_os = "linux") && matches!(flag, "maximizable" | "minimizable") {
      println!("skip {setter}/{getter}: official Linux contract is unsupported");
      continue;
    }
    let mut unknown = None;
    for wanted in [false, true] {
      match flag {
        "decorations" => WindowDispatch::<()>::set_decorations(window, wanted),
        "maximizable" => WindowDispatch::<()>::set_maximizable(window, wanted),
        "minimizable" => WindowDispatch::<()>::set_minimizable(window, wanted),
        "closable" => WindowDispatch::<()>::set_closable(window, wanted),
        _ => unreachable!(),
      }.unwrap();
      match settled_native_flag(window, flag, wanted) {
        Ok(actual) => {
          assert_eq!(actual, wanted, "{setter} must change the actual native button");
          assert_eq!(flag_getter(window, flag), actual, "{getter} must match the native button");
        }
        Err(reason) => { unknown = Some(reason); }
      }
    }
    if let Some(reason) = unknown {
      report(setter, &format!("native button observation unavailable: {reason}"), "unknown");
      report(getter, &format!("native button observation unavailable: {reason}"), "unknown");
    } else {
      evidence(setter, &format!("false/true requests disable/enable the real Electron window's native {flag} button, observed outside Electron"));
      evidence(getter, &format!("false/true reads agree with the independently observed native {flag} button"));
    }
  }
  WindowDispatch::<()>::set_title(window, old_title).unwrap();
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn probe(program: &str, args: &[&str]) -> Result<String, String> {
  use std::process::{Command, Stdio};
  let mut child = Command::new(program).args(args).stdin(Stdio::null())
    .stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()
    .map_err(|error| format!("{program}: {error}"))?;
  let start = Instant::now();
  loop {
    if child.try_wait().map_err(|error| error.to_string())?.is_some() { break; }
    if start.elapsed() > WAIT {
      let _ = child.kill();
      let _ = child.wait();
      return Err(format!("{program}: native probe timed out"));
    }
    thread::sleep(Duration::from_millis(25));
  }
  let output = child.wait_with_output().map_err(|error| error.to_string())?;
  if !output.status.success() {
    return Err(format!("{program}: {}", String::from_utf8_lossy(&output.stderr).trim()));
  }
  Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

#[cfg(target_os = "macos")]
fn native_flag(window: &Window, flag: &str) -> Result<bool, String> {
  let title = WindowDispatch::<()>::title(window).map_err(|error| error.to_string())?;
  let attribute = match flag {
    "decorations" => "native-caption-controls",
    "maximizable" => "AXZoomButton",
    "minimizable" => "AXMinimizeButton",
    "closable" => "AXCloseButton",
    _ => unreachable!(),
  };
  // Ask System Events about the Electron process, never this Rust process's NSApp.
  let source = r#"on run argv
    set titleText to item 1 of argv
    set attributeName to item 2 of argv
    tell application "System Events"
      if not UI elements enabled then error "Accessibility permission is unavailable"
      set matches to {}
      repeat with p in (application processes whose background only is false)
        repeat with w in (windows of p whose name is titleText)
          set end of matches to w
        end repeat
      end repeat
      if (count of matches) is not 1 then error "Expected one real Electron AX window"
      set w to item 1 of matches
      if attributeName is "native-caption-controls" then
        -- AXTitleUIElement is optional even on a normal titled window.
        -- Observe the real caption controls and their nonzero native frames.
        repeat with buttonName in {"AXCloseButton", "AXMinimizeButton", "AXZoomButton"}
          if not (exists attribute buttonName of w) then error "Native caption controls are unavailable"
          set captionButton to (get value of attribute buttonName of w)
          if captionButton is missing value then return false
          set buttonSize to (get value of attribute "AXSize" of captionButton)
          if (item 1 of buttonSize) <= 0 or (item 2 of buttonSize) <= 0 then return false
          if exists attribute "AXHidden" of captionButton then
            if (get value of attribute "AXHidden" of captionButton) then return false
          end if
        end repeat
        return true
      end if
      if not (exists attribute attributeName of w) then error "Native AX attribute is unavailable"
      set axElement to (get value of attribute attributeName of w)
      if axElement is missing value then error "Native button is absent; enabled state is unavailable"
      return (get value of attribute "AXEnabled" of axElement)
    end tell
  end run"#;
  match probe("osascript", &["-e", source, &title, attribute])?.as_str() {
    "true" => Ok(true),
    "false" => Ok(false),
    other => Err(format!("unrecognized AX state: {other}")),
  }
}

#[cfg(target_os = "linux")]
fn native_flag(window: &Window, flag: &str) -> Result<bool, String> {
  let title = WindowDispatch::<()>::title(window).map_err(|error| error.to_string())?;
  let clients = probe("xprop", &["-root", "_NET_CLIENT_LIST"])?;
  let mut found = Vec::new();
  for id in clients.split(|c: char| c == ',' || c.is_whitespace()).filter(|word| word.starts_with("0x")) {
    let name = probe("xprop", &["-id", id, "_NET_WM_NAME", "WM_NAME"])?;
    if name.lines().any(|line| line.ends_with(&format!("\"{title}\""))) { found.push(id.to_owned()); }
  }
  if found.len() != 1 { return Err(format!("expected one X11 WM-managed window named {title}, found {found:?}")); }
  let property = match flag {
    "decorations" => "_NET_FRAME_EXTENTS",
    "closable" => "_NET_WM_ALLOWED_ACTIONS",
    _ => return Err("official Linux maximize/minimize button API is unsupported".into()),
  };
  let actual = probe("xprop", &["-id", &found[0], property])?;
  let (_, value) = actual.split_once('=').ok_or_else(|| format!("WM did not supply {property}: {actual}"))?;
  if flag == "closable" { return Ok(value.split(',').any(|action| action.trim() == "_NET_WM_ACTION_CLOSE")); }
  let extents = value.split(',').map(|axis| axis.trim().parse::<u32>()).collect::<Result<Vec<_>, _>>()
    .map_err(|error| format!("invalid WM frame extents: {error}"))?;
  if extents.len() != 4 { return Err(format!("invalid WM frame extents: {actual}")); }
  Ok(extents.iter().any(|&axis| axis != 0))
}

#[cfg(windows)]
fn native_hwnd(window: &Window) -> Result<windows::Win32::Foundation::HWND, String> {
  use windows::{core::PCWSTR, Win32::UI::WindowsAndMessaging::{FindWindowExW, GetWindowThreadProcessId, IsWindowVisible}};
  let title = WindowDispatch::<()>::title(window).map_err(|error| error.to_string())?;
  let title: Vec<_> = title.encode_utf16().chain(std::iter::once(0)).collect();
  let start = Instant::now();
  loop {
    let mut after = None;
    let mut found = Vec::new();
    while let Ok(hwnd) = unsafe { FindWindowExW(None, after, None, PCWSTR(title.as_ptr())) } {
      after = Some(hwnd);
      let mut pid = 0;
      unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)); }
      if pid != 0 && pid != std::process::id() && unsafe { IsWindowVisible(hwnd) }.as_bool() { found.push(hwnd); }
    }
    if found.len() == 1 { return Ok(found[0]); }
    if start.elapsed() >= WAIT { return Err(format!("expected one visible Electron HWND, found {found:?}")); }
    thread::sleep(Duration::from_millis(25));
  }
}

#[cfg(windows)]
fn native_flag(window: &Window, flag: &str) -> Result<bool, String> {
  use windows::Win32::{Foundation::{POINT, RECT}, Graphics::Gdi::ClientToScreen,
    UI::WindowsAndMessaging::{GetMenuState, GetSystemMenu, GetWindowLongPtrW, GetWindowRect,
      GWL_STYLE, MF_BYCOMMAND, MF_DISABLED, MF_GRAYED, SC_CLOSE, WS_MAXIMIZEBOX, WS_MINIMIZEBOX}};
  let hwnd = native_hwnd(window)?;
  if flag == "decorations" {
    let mut outer = RECT::default();
    let mut client_origin = POINT::default();
    unsafe { GetWindowRect(hwnd, &mut outer) }.map_err(|error| error.to_string())?;
    if !unsafe { ClientToScreen(hwnd, &mut client_origin) }.as_bool() {
      return Err("could not read native client origin".into());
    }
    // Electron deliberately keeps WS_CAPTION on frameless windows for animations.
    // Observe the actual non-client caption inset, not that implementation detail.
    return Ok(client_origin.y - outer.top > client_origin.x - outer.left);
  }
  if flag == "closable" {
    let menu = unsafe { GetSystemMenu(hwnd, false) };
    if menu.0.is_null() { return Err("native system menu is absent".into()); }
    let state = unsafe { GetMenuState(menu, SC_CLOSE, MF_BYCOMMAND) };
    if state == u32::MAX { return Err("native SC_CLOSE is absent".into()); }
    return Ok(state & (MF_DISABLED | MF_GRAYED).0 == 0);
  }
  let mask = match flag {
    "maximizable" => WS_MAXIMIZEBOX.0,
    "minimizable" => WS_MINIMIZEBOX.0,
    _ => unreachable!(),
  };
  let style = unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) } as u32;
  Ok(style & mask == mask)
}

#[cfg(windows)]
fn native_frame_checks(window: &Window) {
  use windows::Win32::{
    Foundation::HWND,
    UI::WindowsAndMessaging::{GetMenuState, GetSystemMenu, GetWindowLongPtrW,
      GWL_STYLE, MF_BYCOMMAND, MF_DISABLED, MF_GRAYED,
      SC_CLOSE, WS_CAPTION, WS_MAXIMIZEBOX, WS_MINIMIZEBOX, WS_SYSMENU, WS_THICKFRAME},
  };
  let old_title = WindowDispatch::<()>::title(window).unwrap();
  let title = format!("runtime-native-frame-{}", std::process::id());
  WindowDispatch::<()>::set_title(window, title).unwrap();
  let hwnd = native_hwnd(window).expect("find one visible Electron window, never the local menu proxy");
  let style = || unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) } as u32;
  let close_enabled = || {
    let menu = unsafe { GetSystemMenu(hwnd, false) };
    assert_ne!(menu.0, std::ptr::null_mut(), "real window must have a system menu");
    let state = unsafe { GetMenuState(menu, SC_CLOSE, MF_BYCOMMAND) };
    assert_ne!(state, u32::MAX, "SC_CLOSE must exist");
    state & (MF_DISABLED | MF_GRAYED).0 == 0
  };
  assert_ne!(hwnd, HWND::default());
  for wanted in [false, true] {
    WindowDispatch::<()>::set_maximizable(window, wanted).unwrap();
    wait("native WS_MAXIMIZEBOX must change", || (style() & WS_MAXIMIZEBOX.0 != 0) == wanted);
    assert_eq!(WindowDispatch::<()>::is_maximizable(window).unwrap(), wanted);
  }
  evidence("WindowDispatch::set_maximizable", "real Electron HWND WS_MAXIMIZEBOX clears then sets after false/true requests");
  evidence("WindowDispatch::is_maximizable", "getter reports false then true matching independently read WS_MAXIMIZEBOX on the real Electron HWND");
  for wanted in [false, true] {
    WindowDispatch::<()>::set_minimizable(window, wanted).unwrap();
    wait("native WS_MINIMIZEBOX must change", || (style() & WS_MINIMIZEBOX.0 != 0) == wanted);
    assert_eq!(WindowDispatch::<()>::is_minimizable(window).unwrap(), wanted);
  }
  evidence("WindowDispatch::set_minimizable", "real Electron HWND WS_MINIMIZEBOX clears then sets after false/true requests");
  evidence("WindowDispatch::is_minimizable", "getter reports false then true matching independently read WS_MINIMIZEBOX on the real Electron HWND");
  for wanted in [false, true] {
    WindowDispatch::<()>::set_closable(window, wanted).unwrap();
    wait("native system-menu SC_CLOSE must disable/enable", || close_enabled() == wanted);
    assert_eq!(WindowDispatch::<()>::is_closable(window).unwrap(), wanted);
  }
  evidence("WindowDispatch::set_closable", "real Electron HWND system-menu SC_CLOSE disables then enables after false/true requests");
  evidence("WindowDispatch::is_closable", "getter reports false then true matching independently read system-menu SC_CLOSE state");

  let independent = (WS_THICKFRAME | WS_MAXIMIZEBOX | WS_MINIMIZEBOX | WS_SYSMENU).0;
  for capabilities in [false, true] {
    WindowDispatch::<()>::set_resizable(window, capabilities).unwrap();
    WindowDispatch::<()>::set_maximizable(window, capabilities).unwrap();
    WindowDispatch::<()>::set_minimizable(window, capabilities).unwrap();
    WindowDispatch::<()>::set_closable(window, capabilities).unwrap();
    wait("prepare actual native capabilities before changing decorations", || {
      let actual = style();
      (actual & WS_THICKFRAME.0 != 0) == capabilities
        && (actual & WS_MAXIMIZEBOX.0 != 0) == capabilities
        && (actual & WS_MINIMIZEBOX.0 != 0) == capabilities
        && close_enabled() == capabilities
    });
    let preserved = style() & independent;
    let close_before = close_enabled();
    for decorated in [false, true] {
      WindowDispatch::<()>::set_decorations(window, decorated).unwrap();
      wait("native WS_CAPTION must change without rewriting independent capabilities", || {
        let actual = style();
        actual & WS_CAPTION.0 == if decorated { WS_CAPTION.0 } else { 0 }
          && actual & independent == preserved && close_enabled() == close_before
      });
      assert_eq!(WindowDispatch::<()>::is_decorated(window).unwrap(), decorated);
      assert_eq!(settled_native_flag(window, "decorations", decorated).unwrap(), decorated,
        "native client insets must reflect the visible caption, not only its style bit");
    }
  }
  evidence("WindowDispatch::set_decorations", "real Electron HWND caption style and actual client caption inset clear/restore while WS_THICKFRAME, WS_MAXIMIZEBOX, WS_MINIMIZEBOX, WS_SYSMENU and SC_CLOSE stay unchanged with both enabled and disabled capabilities");
  evidence("WindowDispatch::is_decorated", "getter reports false/true matching independently read native caption insets on the real Electron HWND");
  WindowDispatch::<()>::set_title(window, old_title).unwrap();
}
