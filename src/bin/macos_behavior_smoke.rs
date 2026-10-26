//! macOS system observations target Electron's PID, never the Rust process's NSApp.
#[cfg(not(target_os = "macos"))]
fn main() -> std::process::ExitCode {
  eprintln!("macOS behavior: UNKNOWN (requires macOS; no member evidence emitted)");
  std::process::ExitCode::from(77)
}

#[cfg(target_os = "macos")]
fn main() -> std::process::ExitCode {
  match macos::run() {
    Ok(()) => std::process::ExitCode::SUCCESS,
    Err(error) => {
      eprintln!("macOS behavior: {error}; no evidence for unfinished assertions");
      std::process::ExitCode::FAILURE
    }
  }
}

#[cfg(target_os = "macos")]
mod macos {
  use std::{
    fs,
    io::Read,
    path::PathBuf,
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
  };
  use serde_json::{Value, json};
  use tauri_runtime::{
    ActivationPolicy, Runtime, RuntimeHandle, RuntimeInitArgs, WebviewDispatch, WindowDispatch,
    dpi::{LogicalPosition, PhysicalPosition},
    webview::{PendingWebview, WebviewAttributes},
    window::{DetachedWindow, PendingWindow, RawWindow, WindowBuilder},
  };
  use tauri_runtime_electron::{ElectronRuntime, ElectronRuntimeHandle, ElectronWindowBuilder};
  use tauri_utils::{TitleBarStyle, config::{WebviewUrl, WindowConfig}};

  type Check<T = ()> = Result<T, String>;
  type Window = DetachedWindow<(), ElectronRuntime>;
  const WAIT: Duration = Duration::from_secs(10);

  // No linked/private AppKit pointers: this separate executable reads the target
  // process via public NSRunningApplication, CGWindowList and Accessibility APIs.
  const OBSERVER: &str = r#"
#import <AppKit/AppKit.h>
#import <ApplicationServices/ApplicationServices.h>
#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

static int unknown(NSString *reason) {
  NSData *data = [NSJSONSerialization dataWithJSONObject:@{@"error":reason} options:0 error:NULL];
  fwrite(data.bytes, 1, data.length, stdout);
  return 77;
}
static id ax(AXUIElementRef element, CFStringRef key) {
  CFTypeRef value = NULL;
  if (!element || AXUIElementCopyAttributeValue(element, key, &value) != kAXErrorSuccess) return nil;
  return CFBridgingRelease(value);
}
static NSArray *frame(AXUIElementRef element) {
  id p = ax(element, kAXPositionAttribute), s = ax(element, kAXSizeAttribute);
  if (!p || !s || CFGetTypeID((__bridge CFTypeRef)p) != AXValueGetTypeID() || CFGetTypeID((__bridge CFTypeRef)s) != AXValueGetTypeID()) return nil;
  CGPoint point; CGSize size;
  if (!AXValueGetValue((__bridge AXValueRef)p, kAXValueCGPointType, &point) || !AXValueGetValue((__bridge AXValueRef)s, kAXValueCGSizeType, &size)) return nil;
  return @[@(point.x), @(point.y), @(size.width), @(size.height)];
}
static AXUIElementRef webArea(AXUIElementRef element, int depth, int *budget) {
  if (depth > 12 || --*budget <= 0) return NULL;
  if ([ax(element, kAXRoleAttribute) isEqual:@"AXWebArea"]) return (AXUIElementRef)CFRetain(element);
  id children = ax(element, kAXChildrenAttribute);
  if ([children isKindOfClass:[NSArray class]]) for (id child in children) {
    AXUIElementRef found = webArea((__bridge AXUIElementRef)child, depth + 1, budget);
    if (found) return found;
  }
  return NULL;
}
static void dockItems(AXUIElementRef element, NSString *bundle, int depth, int *budget, int *urls, int *matches) {
  if (depth > 8 || --*budget <= 0) return;
  id url = ax(element, CFSTR("AXURL"));
  NSString *path = nil;
  if ([url isKindOfClass:[NSURL class]]) path = [url path];
  else if ([url isKindOfClass:[NSString class]]) path = [[NSURL URLWithString:url] path];
  if (path) {
    ++*urls;
    if ([[path stringByResolvingSymlinksInPath] isEqual:bundle]) ++*matches;
  }
  id children = ax(element, kAXChildrenAttribute);
  if ([children isKindOfClass:[NSArray class]]) for (id child in children)
    dockItems((__bridge AXUIElementRef)child, bundle, depth + 1, budget, urls, matches);
}
int main(int argc, const char **argv) {
  @autoreleasepool {
    if (argc < 3) return unknown(@"observer needs mode and Electron PID");
    pid_t pid = (pid_t)atoi(argv[2]);
    NSRunningApplication *app = [NSRunningApplication runningApplicationWithProcessIdentifier:pid];
    if (!app || app.terminated || pid == getpid()) return unknown(@"target is not an external running application");
    if (!strcmp(argv[1], "activate")) {
      BOOL accepted = [app activateWithOptions:NSApplicationActivateIgnoringOtherApps];
      return accepted ? 0 : unknown(@"system refused foreground activation");
    }
    if (!AXIsProcessTrusted()) return unknown(@"Accessibility permission is unavailable for the native observer");
    NSRunningApplication *dock = [NSRunningApplication runningApplicationsWithBundleIdentifier:@"com.apple.dock"].firstObject;
    if (!dock) return unknown(@"no login-session Dock process");
    AXUIElementRef dockAX = AXUIElementCreateApplication(dock.processIdentifier);
    AXUIElementSetMessagingTimeout(dockAX, 2.0);
    NSString *bundle = app.bundleURL.path.stringByResolvingSymlinksInPath;
    int budget = 1500, urls = 0, matches = 0;
    dockItems(dockAX, bundle, 0, &budget, &urls, &matches);
    CFRelease(dockAX);
    if (!urls || !bundle) return unknown(@"Dock Accessibility URLs cannot be inspected");
    NSArray *cg = CFBridgingRelease(CGWindowListCopyWindowInfo(kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements, kCGNullWindowID));
    if (!cg) return unknown(@"CGWindowList is unavailable");
    NSMutableArray *rectangles = [NSMutableArray array];
    for (NSDictionary *entry in cg) {
      if ([entry[(__bridge NSString *)kCGWindowOwnerPID] intValue] != pid || [entry[(__bridge NSString *)kCGWindowLayer] intValue] != 0) continue;
      CGRect rect;
      if (CGRectMakeWithDictionaryRepresentation((__bridge CFDictionaryRef)entry[(__bridge NSString *)kCGWindowBounds], &rect) && rect.size.width > 20 && rect.size.height > 20)
        [rectangles addObject:@[@(rect.origin.x), @(rect.origin.y), @(rect.size.width), @(rect.size.height)]];
    }
    AXUIElementRef application = AXUIElementCreateApplication(pid);
    AXUIElementSetMessagingTimeout(application, 2.0);
    id windows = ax(application, kAXWindowsAttribute);
    NSMutableArray *observed = [NSMutableArray array];
    if ([windows isKindOfClass:[NSArray class]]) for (id window in windows) {
      AXUIElementRef w = (__bridge AXUIElementRef)window;
      NSString *title = ax(w, kAXTitleAttribute);
      NSArray *rect = frame(w);
      if (!title || !rect) continue;
      NSMutableArray *buttons = [NSMutableArray array];
      CFStringRef keys[] = { kAXCloseButtonAttribute, kAXMinimizeButtonAttribute, kAXZoomButtonAttribute };
      for (int i = 0; i < 3; ++i) {
        id button = ax(w, keys[i]);
        [buttons addObject:button ? (frame((__bridge AXUIElementRef)button) ?: (id)[NSNull null]) : (id)[NSNull null]];
      }
      id titleElement = ax(w, kAXTitleUIElementAttribute);
      NSArray *titleRect = titleElement ? frame((__bridge AXUIElementRef)titleElement) : nil;
      NSString *titleText = titleElement ? (ax((__bridge AXUIElementRef)titleElement, kAXValueAttribute) ?: ax((__bridge AXUIElementRef)titleElement, kAXTitleAttribute)) : nil;
      budget = 1500;
      AXUIElementRef area = webArea(w, 0, &budget);
      NSArray *content = area ? frame(area) : nil;
      if (area) CFRelease(area);
      BOOL onScreen = NO;
      for (NSArray *candidate in rectangles) {
        BOOL equal = YES;
        for (int i = 0; i < 4; ++i) if (fabs([rect[i] doubleValue] - [candidate[i] doubleValue]) > 2.0) equal = NO;
        if (equal) onScreen = YES;
      }
      [observed addObject:@{@"title":title, @"frame":rect, @"buttons":buttons,
        @"content":content ?: (id)[NSNull null], @"onScreen":@(onScreen),
        @"titleFrame":titleRect ?: (id)[NSNull null], @"titleText":titleText ?: (id)[NSNull null]}];
    }
    CFRelease(application);
    NSRunningApplication *front = NSWorkspace.sharedWorkspace.frontmostApplication;
    NSDictionary *result = @{@"pid":@(app.processIdentifier), @"executable":app.executableURL.path ?: @"",
      @"policy":@(app.activationPolicy), @"hidden":@(app.hidden), @"active":@(app.active),
      @"frontPID":@(front.processIdentifier), @"dockMatches":@(matches), @"cgWindowCount":@(rectangles.count), @"windows":observed};
    NSData *data = [NSJSONSerialization dataWithJSONObject:result options:0 error:NULL];
    if (!data) return unknown(@"native observations were not JSON serializable");
    fwrite(data.bytes, 1, data.length, stdout);
    return 0;
  }
}
"#;

  fn output(command: &mut Command, timeout: Duration) -> Check<Vec<u8>> {
    let mut child = command.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()
      .map_err(|error| format!("UNKNOWN: start {command:?}: {error}"))?;
    let mut stdout = child.stdout.take().ok_or("UNKNOWN: missing observer stdout")?;
    let mut stderr = child.stderr.take().ok_or("UNKNOWN: missing observer stderr")?;
    let stdout = thread::spawn(move || { let mut bytes = Vec::new(); stdout.read_to_end(&mut bytes).map(|_| bytes) });
    let stderr = thread::spawn(move || { let mut bytes = Vec::new(); stderr.read_to_end(&mut bytes).map(|_| bytes) });
    let started = Instant::now();
    let status = loop {
      match child.try_wait() {
        Ok(Some(status)) => break status,
        Ok(None) if started.elapsed() < timeout => thread::sleep(Duration::from_millis(30)),
        state => {
          let _ = child.kill();
          let _ = child.wait();
          return Err(format!("UNKNOWN: native observer command did not finish: {state:?}"));
        }
      }
    };
    let stdout = stdout.join().map_err(|_| "UNKNOWN: observer stdout reader panicked")?.map_err(|error| error.to_string())?;
    let stderr = stderr.join().map_err(|_| "UNKNOWN: observer stderr reader panicked")?.map_err(|error| error.to_string())?;
    if !status.success() {
      return Err(format!("UNKNOWN: native command {status}: {} {}",
        String::from_utf8_lossy(&stdout), String::from_utf8_lossy(&stderr)));
    }
    Ok(stdout)
  }

  struct Observer { root: PathBuf, binary: PathBuf, pid: u32 }
  impl Drop for Observer {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.root); }
  }
  impl Observer {
    fn new() -> Check<Self> {
      let root = std::env::temp_dir().join(format!("electron-macos-observer-{}-{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
      fs::create_dir(&root).map_err(|error| format!("UNKNOWN: create observer temp directory: {error}"))?;
      let observer = Self { binary: root.join("observer"), root, pid: 0 };
      let source = observer.root.join("observer.m");
      fs::write(&source, OBSERVER).map_err(|error| format!("UNKNOWN: write observer: {error}"))?;
      output(Command::new("/usr/bin/clang").args(["-fobjc-arc", "-framework", "AppKit", "-framework", "ApplicationServices"])
        .arg(&source).arg("-o").arg(&observer.binary), Duration::from_secs(45))?;
      Ok(observer)
    }
    fn discover(&mut self) -> Check {
      let ps = output(Command::new("/bin/ps").args(["-axo", "pid=,ppid=,comm="]), WAIT)?;
      let ps = String::from_utf8_lossy(&ps);
      let mut candidates = Vec::new();
      for line in ps.lines() {
        let mut fields = line.split_whitespace();
        let pid = fields.next().and_then(|value| value.parse::<u32>().ok());
        let parent = fields.next().and_then(|value| value.parse::<u32>().ok());
        let executable = fields.collect::<Vec<_>>().join(" ");
        if parent == Some(std::process::id()) && executable.contains(".app/Contents/MacOS/") {
          if let Some(pid) = pid { candidates.push(pid); }
        }
      }
      if candidates.len() != 1 { return Err(format!("UNKNOWN: expected one direct Electron app child, observed {candidates:?}")) }
      self.pid = candidates[0];
      if self.pid == std::process::id() { return Err("UNKNOWN: observer selected the Rust process".into()) }
      Ok(())
    }
    fn sample(&self) -> Check<Value> {
      let bytes = output(Command::new(&self.binary).arg("snapshot").arg(self.pid.to_string()), WAIT)?;
      let value: Value = serde_json::from_slice(&bytes).map_err(|error| format!("UNKNOWN: native JSON: {error}"))?;
      if value["pid"].as_u64() != Some(u64::from(self.pid)) { return Err(format!("UNKNOWN: observation PID mismatch: {value}")) }
      Ok(value)
    }
    fn wait(&self, description: &str, check: impl Fn(&Value) -> bool) -> Check<Value> {
      let started = Instant::now();
      loop {
        let got = self.sample()?;
        if check(&got) { return Ok(got) }
        if started.elapsed() >= WAIT { return Err(format!("FAIL: {description}; actual external system observation: {got}")) }
        thread::sleep(Duration::from_millis(100));
      }
    }
    fn activate(&self, pid: u32) -> Check {
      output(Command::new(&self.binary).arg("activate").arg(pid.to_string()), WAIT).map(|_| ())
    }
    fn finder(&self) -> Check<u32> {
      let bytes = output(Command::new("/usr/bin/pgrep").args(["-x", "Finder"]), WAIT)?;
      let pid = String::from_utf8_lossy(&bytes).trim().parse().map_err(|_| "UNKNOWN: Finder PID unavailable")?;
      self.activate(pid)?;
      self.wait("Finder must own the foreground before app.show", |value| value["frontPID"].as_u64() == Some(u64::from(pid)))?;
      Ok(pid)
    }
  }

  fn require(condition: bool, message: &str) -> Check {
    if condition { Ok(()) } else { Err(format!("FAIL: {message}")) }
  }
  fn evidence(member: &str, test: &str, assertion: &str) {
    println!("[runtime-evidence] {}", json!({"member":member, "test":format!("macos_behavior_smoke::{test}"), "assertion":assertion, "kind":"behavior", "status":"pass"}));
  }
  fn native_window<'a>(snapshot: &'a Value, title: &str) -> Option<&'a Value> {
    snapshot["windows"].as_array()?.iter().find(|window| window["title"] == title)
  }
  fn visible_pair(snapshot: &Value) -> bool {
    ["macos-app-one", "macos-app-two"].iter().all(|title| native_window(snapshot, title).is_some_and(|window| window["onScreen"] == true))
  }
  // Native-only creation is safe before the Rust event pump starts. Renderer
  // callbacks are queued on that pump and must be awaited only from the worker.
  fn create_native(handle: &ElectronRuntimeHandle, label: &str, builder: ElectronWindowBuilder) -> Check<Window> {
    let mut pending = PendingWindow::<(), ElectronRuntime>::new(builder, label).map_err(|error| error.to_string())?;
    let mut page = PendingWebview::new(WebviewAttributes::new(WebviewUrl::External("about:blank".parse().unwrap())), label).map_err(|error| error.to_string())?;
    page.url = "about:blank".into();
    pending.set_webview(page);
    <ElectronRuntimeHandle as RuntimeHandle<()>>::create_window(handle, pending, None::<fn(RawWindow<'_>)>).map_err(|error| error.to_string())
  }
  // Only titlebars() uses this renderer-ready path, from the run_return worker.
  fn create(handle: &ElectronRuntimeHandle, label: &str, builder: ElectronWindowBuilder) -> Check<Window> {
    let window = create_native(handle, label, builder)?;
    let (tx, rx) = mpsc::channel();
    WebviewDispatch::<()>::eval_script_with_callback(&window.webview.as_ref().unwrap().webview.dispatcher,
      "new Promise(resolve => { const ready = () => { document.body.textContent = 'native AX layout proof'; document.body.style.margin = '0'; resolve(true); }; if (document.body) ready(); else document.addEventListener('DOMContentLoaded', ready, {once:true}); })",
      move |value| { let _ = tx.send(value); }).map_err(|error| error.to_string())?;
    require(rx.recv_timeout(WAIT).map_err(|error| error.to_string())? == "true", "real page must initialize before AX layout inspection")?;
    Ok(window)
  }
  fn builder(title: &str) -> ElectronWindowBuilder {
    ElectronWindowBuilder::with_config(&WindowConfig {
      title:title.into(), width:420.0, height:280.0, x:Some(160.0), y:Some(160.0), visible:true,
      ..Default::default()
    })
  }

  fn policy(observer: &Observer, member: &str, mut set: impl FnMut(ActivationPolicy) -> Check) -> Check {
    for (policy, value) in [(ActivationPolicy::Accessory,1), (ActivationPolicy::Prohibited,2), (ActivationPolicy::Regular,0)] {
      set(policy)?;
      observer.wait("activation policy must change in the external Electron process", |state| state["policy"] == value)?;
    }
    evidence(member, "activation_policy", "external NSRunningApplication for the verified Electron PID reports Accessory, Prohibited and Regular after each distinct mutation");
    Ok(())
  }
  fn dock(observer: &Observer, member: &str, mut set: impl FnMut(bool) -> Check) -> Check {
    observer.wait("baseline Electron Dock item", |state| state["dockMatches"].as_u64().is_some_and(|count| count > 0))?;
    set(false)?;
    observer.wait("Electron's bundle URL must disappear from the real Dock AX tree", |state| state["dockMatches"] == 0)?;
    set(true)?;
    observer.wait("Electron's bundle URL must return to the real Dock AX tree", |state| state["dockMatches"].as_u64().is_some_and(|count| count > 0))?;
    evidence(member, "dock_visibility", "external Dock Accessibility items matched by Electron bundle URL disappear when hidden and reappear when shown");
    Ok(())
  }
  fn hide_show(observer: &Observer, first: &Window, hide_member: &str, show_member: &str, hide: impl FnOnce() -> Check, mut show: impl FnMut() -> Check) -> Check {
    show()?;
    WindowDispatch::<()>::set_focus(&first.dispatcher).map_err(|error| error.to_string())?;
    observer.wait("both app windows visible and Electron foreground before hide", |state| state["active"] == true && visible_pair(state))?;
    hide()?;
    observer.wait("all-app hide must remove both real windows and foreground ownership", |state| state["hidden"] == true && state["active"] == false && state["cgWindowCount"] == 0)?;
    evidence(hide_member, "app_hide", "NSRunningApplication isHidden becomes true and isActive false, and CGWindowList no longer lists either of the two real Electron windows");
    let finder = observer.finder()?;
    show()?;
    observer.wait("all-app show must restore both windows without stealing focus", |state| state["hidden"] == false && state["active"] == false && state["frontPID"].as_u64() == Some(u64::from(finder)) && visible_pair(state))?;
    evidence(show_member, "app_show", "both Electron windows return in external AX/CG observations and isHidden becomes false while Finder retains foreground ownership");
    Ok(())
  }
  fn runtime_checks(runtime: &mut ElectronRuntime, handle: &ElectronRuntimeHandle, observer: &Observer, first: &Window) -> Check {
    policy(observer, "Runtime::set_activation_policy", |value| { <ElectronRuntime as Runtime<()>>::set_activation_policy(runtime, value); Ok(()) })?;
    dock(observer, "Runtime::set_dock_visibility", |value| { <ElectronRuntime as Runtime<()>>::set_dock_visibility(runtime, value); Ok(()) })?;
    hide_show(observer, first, "Runtime::hide", "Runtime::show",
      || { <ElectronRuntime as Runtime<()>>::hide(runtime); Ok(()) },
      || { <ElectronRuntime as Runtime<()>>::show(runtime); Ok(()) })?;
    <ElectronRuntimeHandle as RuntimeHandle<()>>::set_activation_policy(handle, ActivationPolicy::Regular).map_err(|error| error.to_string())
  }
  fn handle_checks(handle: &ElectronRuntimeHandle, observer: &Observer, first: &Window) -> Check {
    policy(observer, "RuntimeHandle::set_activation_policy", |value| <ElectronRuntimeHandle as RuntimeHandle<()>>::set_activation_policy(handle, value).map_err(|error| error.to_string()))?;
    dock(observer, "RuntimeHandle::set_dock_visibility", |value| <ElectronRuntimeHandle as RuntimeHandle<()>>::set_dock_visibility(handle, value).map_err(|error| error.to_string()))?;
    hide_show(observer, first, "RuntimeHandle::hide", "RuntimeHandle::show",
      || <ElectronRuntimeHandle as RuntimeHandle<()>>::hide(handle).map_err(|error| error.to_string()),
      || <ElectronRuntimeHandle as RuntimeHandle<()>>::show(handle).map_err(|error| error.to_string()))
  }

  fn numbers(value: &Value) -> Check<[f64;4]> {
    let values = value.as_array().filter(|values| values.len() == 4).ok_or_else(|| format!("UNKNOWN: AX frame unavailable: {value}"))?;
    let mut result = [0.0;4];
    for (index, value) in values.iter().enumerate() { result[index] = value.as_f64().ok_or("UNKNOWN: nonnumeric AX frame")?; }
    Ok(result)
  }
  fn caption(observer: &Observer, title: &str) -> Check<Value> {
    let snapshot = observer.wait("new titlebar window must exist in the external AX/CG tree", |state| native_window(state, title).is_some_and(|window| window["onScreen"] == true))?;
    let window = native_window(&snapshot, title).unwrap().clone();
    numbers(&window["frame"])?;
    numbers(&window["content"])?;
    for index in 0..3 { numbers(&window["buttons"][index])?; }
    Ok(window)
  }
  fn inset(window: &Value) -> Check<f64> { Ok(numbers(&window["content"])?[1] - numbers(&window["frame"])?[1]) }
  fn controls_shift(before: &Value, after: &Value, x: f64, y: f64) -> Check {
    let a = numbers(&before["frame"])?;
    let b = numbers(&after["frame"])?;
    for index in 0..3 {
      let first = numbers(&before["buttons"][index])?;
      let next = numbers(&after["buttons"][index])?;
      require((next[0]-b[0] - (first[0]-a[0]) - x).abs() <= 2.0 && (next[1]-b[1] - (first[1]-a[1]) - y).abs() <= 2.0,
        "actual AX traffic-light controls must move by the requested logical delta")?;
      require((first[2]-next[2]).abs() <= 1.0 && (first[3]-next[3]).abs() <= 1.0, "traffic-light relocation must preserve control sizes")?;
    }
    Ok(())
  }
  fn titlebars(handle: &ElectronRuntimeHandle, observer: &Observer) -> Check {
    let visible = create(handle, "title-visible", builder("title-visible").title_bar_style(TitleBarStyle::Visible).hidden_title(false))?;
    let normal = caption(observer, "title-visible")?;
    if normal["titleText"] != "title-visible" || normal["titleFrame"].is_null() {
      return Err(format!("UNKNOWN: standard native window exposes no observable title UI element: {normal}"))
    }
    let transparent = create(handle, "title-transparent", builder("title-transparent").title_bar_style(TitleBarStyle::Transparent).hidden_title(false))?;
    let transparent_state = caption(observer, "title-transparent")?;
    let overlay = create(handle, "title-overlay", builder("title-overlay").title_bar_style(TitleBarStyle::Overlay).hidden_title(false))?;
    let overlay_state = caption(observer, "title-overlay")?;
    require(inset(&normal)? >= 16.0, "Visible titlebar must keep real web content below the titlebar")?;
    require((inset(&transparent_state)? - inset(&normal)?).abs() <= 2.0, "Transparent titlebar must retain the normal content inset, unlike Overlay")?;
    require(inset(&overlay_state)?.abs() <= 2.0, "Overlay must place the actual AXWebArea underneath the titlebar")?;
    evidence("WindowBuilder::title_bar_style", "title_bar_style", "fresh Visible, Transparent and Overlay windows retain three real AX titlebar controls; AXWebArea is inset for Visible/Transparent and extends beneath the Overlay caption");

    let hidden = create(handle, "title-hidden", builder("title-hidden").title_bar_style(TitleBarStyle::Visible).hidden_title(true))?;
    let hidden_state = caption(observer, "title-hidden")?;
    require(hidden_state["titleFrame"].is_null() || hidden_state["titleText"] != "title-hidden", "hidden_title(true) must remove the actual title text UI element")?;
    require((inset(&hidden_state)? - inset(&normal)?).abs() <= 2.0, "hiding only title text must not convert a Visible titlebar to Overlay content layout")?;
    controls_shift(&normal, &hidden_state, 0.0, 0.0)?;
    evidence("WindowBuilder::hidden_title", "hidden_title", "fresh hidden_title(false/true) windows differ in the real AX title text element while traffic lights and normal AXWebArea titlebar inset remain unchanged");

    let positioned_a = create(handle, "buttons-a", builder("buttons-a").title_bar_style(TitleBarStyle::Overlay).traffic_light_position(LogicalPosition::new(12.0,14.0)))?;
    let a = caption(observer, "buttons-a")?;
    let positioned_b = create(handle, "buttons-b", builder("buttons-b").title_bar_style(TitleBarStyle::Overlay).traffic_light_position(LogicalPosition::new(38.0,30.0)))?;
    let b = caption(observer, "buttons-b")?;
    controls_shift(&a, &b, 26.0, 16.0)?;
    evidence("WindowBuilder::traffic_light_position", "builder_traffic_light_position", "two fresh Overlay windows with different builder control positions have all three real AX traffic-light frames shifted by the requested 26x16 logical delta");

    let moved = create(handle, "buttons-dispatch", builder("buttons-dispatch").title_bar_style(TitleBarStyle::Overlay).traffic_light_position(LogicalPosition::new(12.0,14.0)))?;
    let initial = caption(observer, "buttons-dispatch")?;
    WindowDispatch::<()>::set_traffic_light_position(&moved.dispatcher, LogicalPosition::new(42.0,34.0).into()).map_err(|error| error.to_string())?;
    let logical = caption(observer, "buttons-dispatch")?;
    controls_shift(&initial, &logical, 30.0, 20.0)?;
    let scale = WindowDispatch::<()>::scale_factor(&moved.dispatcher).map_err(|error| error.to_string())?;
    let point: PhysicalPosition<i32> = LogicalPosition::new(26.0,22.0).to_physical(scale);
    WindowDispatch::<()>::set_traffic_light_position(&moved.dispatcher, point.into()).map_err(|error| error.to_string())?;
    let physical = caption(observer, "buttons-dispatch")?;
    controls_shift(&logical, &physical, -16.0, -12.0)?;
    evidence("WindowDispatch::set_traffic_light_position", "dispatch_traffic_light_position", "logical and physical coordinate dispatches move all three externally observed AX traffic-light frames by distinct correct logical deltas in the same native window");
    for window in [visible,transparent,overlay,hidden,positioned_a,positioned_b,moved] {
      WindowDispatch::<()>::destroy(&window.dispatcher).map_err(|error| error.to_string())?;
    }
    Ok(())
  }

  pub fn run() -> Check {
    let mut observer = Observer::new()?;
    let root = observer.root.clone();
    let (done_tx, done_rx) = mpsc::channel();
    thread::spawn(move || {
      if matches!(done_rx.recv_timeout(Duration::from_secs(160)), Err(mpsc::RecvTimeoutError::Timeout)) {
        eprintln!("macOS behavior: UNKNOWN (watchdog expired)");
        let _ = Command::new("/usr/bin/pkill").args(["-TERM", "-P", &std::process::id().to_string()]).status();
        let _ = fs::remove_dir_all(root);
        std::process::exit(1);
      }
    });
    let mut runtime = <ElectronRuntime as Runtime<()>>::new(RuntimeInitArgs::default()).map_err(|error| error.to_string())?;
    let handle = <ElectronRuntime as Runtime<()>>::handle(&runtime);
    observer.discover()?;
    let initial_front = observer.sample()?["frontPID"].as_u64().unwrap_or(0) as u32;
    let first = create_native(&handle, "macos-app-one", builder("macos-app-one").position(100.0,100.0))?;
    let second = create_native(&handle, "macos-app-two", builder("macos-app-two").position(560.0,100.0))?;
    observer.wait("preflight: actual Electron app, Dock item and two native windows", |state| state["policy"] == 0 && state["dockMatches"].as_u64().is_some_and(|count| count > 0) && visible_pair(state))?;
    if let Err(error) = runtime_checks(&mut runtime, &handle, &observer, &first) {
      let _ = <ElectronRuntimeHandle as RuntimeHandle<()>>::request_exit(&handle, 1);
      let _ = <ElectronRuntime as Runtime<()>>::run_return(runtime, |_| {});
      let _ = done_tx.send(());
      return Err(error)
    }
    let worker_handle = handle.clone();
    let worker = thread::spawn(move || {
      let result = handle_checks(&worker_handle, &observer, &first).and_then(|_| titlebars(&worker_handle, &observer));
      let _ = <ElectronRuntimeHandle as RuntimeHandle<()>>::set_activation_policy(&worker_handle, ActivationPolicy::Regular);
      let _ = <ElectronRuntimeHandle as RuntimeHandle<()>>::set_dock_visibility(&worker_handle, true);
      let _ = <ElectronRuntimeHandle as RuntimeHandle<()>>::show(&worker_handle);
      if initial_front != 0 && initial_front != observer.pid { let _ = observer.activate(initial_front); }
      let _ = WindowDispatch::<()>::destroy(&first.dispatcher);
      let _ = WindowDispatch::<()>::destroy(&second.dispatcher);
      let _ = <ElectronRuntimeHandle as RuntimeHandle<()>>::request_exit(&worker_handle, i32::from(result.is_err()));
      result
    });
    let status = <ElectronRuntime as Runtime<()>>::run_return(runtime, |_| {});
    let result = worker.join().map_err(|_| "FAIL: macOS behavior worker panicked".to_string())?;
    let _ = done_tx.send(());
    result?;
    require(status == 0, "real Electron runtime must exit zero")?;
    println!("macOS external system behavior regressions passed");
    Ok(())
  }
}
