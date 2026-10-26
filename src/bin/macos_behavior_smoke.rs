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
static id axRead(AXUIElementRef element, CFStringRef key, AXError *error) {
  CFTypeRef value = NULL;
  *error = element ? AXUIElementCopyAttributeValue(element, key, &value) : kAXErrorInvalidUIElement;
  return *error == kAXErrorSuccess ? CFBridgingRelease(value) : nil;
}
static id ax(AXUIElementRef element, CFStringRef key) {
  AXError error;
  return axRead(element, key, &error);
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
static BOOL dockItems(AXUIElementRef element, NSString *bundle, int depth, int *budget, int *urls, int *matches) {
  if (depth > 8 || --*budget <= 0) return NO;
  AXError error;
  id url = axRead(element, CFSTR("AXURL"), &error);
  if (error != kAXErrorSuccess && error != kAXErrorNoValue && error != kAXErrorAttributeUnsupported) return NO;
  NSString *path = nil;
  if ([url isKindOfClass:[NSURL class]]) path = [url path];
  else if ([url isKindOfClass:[NSString class]]) path = [[NSURL URLWithString:url] path];
  if (path) {
    ++*urls;
    if ([[path stringByResolvingSymlinksInPath] isEqual:bundle]) ++*matches;
  }
  id children = axRead(element, kAXChildrenAttribute, &error);
  if (error != kAXErrorSuccess && error != kAXErrorNoValue && error != kAXErrorAttributeUnsupported) return NO;
  if (children && ![children isKindOfClass:[NSArray class]]) return NO;
  for (id child in children) if (!dockItems((__bridge AXUIElementRef)child, bundle, depth + 1, budget, urls, matches)) return NO;
  return YES;
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
    AXUIElementRef application = AXUIElementCreateApplication(pid);
    AXUIElementSetMessagingTimeout(application, 2.0);
    if (!strcmp(argv[1], "manual")) {
      AXError error;
      id previous = axRead(application, CFSTR("AXManualAccessibility"), &error);
      AXError enabled = AXUIElementSetAttributeValue(application, CFSTR("AXManualAccessibility"), kCFBooleanTrue);
      NSDictionary *result = @{@"previous":error == kAXErrorSuccess && [previous isKindOfClass:[NSNumber class]] ? previous : (id)[NSNull null], @"setError":@(enabled)};
      NSData *data = [NSJSONSerialization dataWithJSONObject:result options:0 error:NULL];
      fwrite(data.bytes, 1, data.length, stdout);
      CFRelease(application);
      return 0;
    }
    if (!strcmp(argv[1], "restore-manual") && argc == 4) {
      AXError error = AXUIElementSetAttributeValue(application, CFSTR("AXManualAccessibility"), atoi(argv[3]) ? kCFBooleanTrue : kCFBooleanFalse);
      CFRelease(application);
      return error == kAXErrorSuccess ? 0 : unknown(@"could not restore AXManualAccessibility");
    }
    NSRunningApplication *front = NSWorkspace.sharedWorkspace.frontmostApplication;
    NSMutableDictionary *result = [@{@"pid":@(app.processIdentifier), @"executable":app.executableURL.path ?: @"",
      @"policy":@(app.activationPolicy), @"hidden":@(app.hidden), @"active":@(app.active),
      @"frontPID":front ? @(front.processIdentifier) : (id)[NSNull null],
      @"cgWindows":[NSNull null], @"cgError":[NSNull null], @"windows":[NSNull null], @"axError":[NSNull null],
      @"dockMatches":[NSNull null], @"dockError":[NSNull null]} mutableCopy];
    NSArray *cg = CFBridgingRelease(CGWindowListCopyWindowInfo(kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements, kCGNullWindowID));
    NSMutableArray *rectangles = [NSMutableArray array];
    if (!cg) result[@"cgError"] = @"CGWindowList is unavailable";
    else {
      for (NSDictionary *entry in cg) {
        if ([entry[(__bridge NSString *)kCGWindowOwnerPID] intValue] != pid || [entry[(__bridge NSString *)kCGWindowLayer] intValue] != 0) continue;
        CGRect rect;
        NSNumber *number = entry[(__bridge NSString *)kCGWindowNumber];
        if (!number || !CGRectMakeWithDictionaryRepresentation((__bridge CFDictionaryRef)entry[(__bridge NSString *)kCGWindowBounds], &rect)) {
          result[@"cgError"] = @"CG window identity or bounds unavailable";
          continue;
        }
        if (rect.size.width > 20 && rect.size.height > 20)
          [rectangles addObject:@{@"id":number, @"frame":@[@(rect.origin.x), @(rect.origin.y), @(rect.size.width), @(rect.size.height)]}];
      }
      if (result[@"cgError"] == [NSNull null]) result[@"cgWindows"] = rectangles;
    }
    if (!AXIsProcessTrusted()) {
      result[@"axError"] = @"Accessibility permission is unavailable for the native observer";
      result[@"dockError"] = result[@"axError"];
    } else {
      NSRunningApplication *dock = [NSRunningApplication runningApplicationsWithBundleIdentifier:@"com.apple.dock"].firstObject;
      NSString *bundle = app.bundleURL.path.stringByResolvingSymlinksInPath;
      if (!dock || !bundle) result[@"dockError"] = @"Dock process or target bundle URL unavailable";
      else {
        AXUIElementRef dockAX = AXUIElementCreateApplication(dock.processIdentifier);
        AXUIElementSetMessagingTimeout(dockAX, 2.0);
        int budget = 1500, urls = 0, matches = 0;
        BOOL complete = dockItems(dockAX, bundle, 0, &budget, &urls, &matches);
        CFRelease(dockAX);
        if (complete && urls) result[@"dockMatches"] = @(matches);
        else result[@"dockError"] = @"Dock AX traversal incomplete, failed or URL attributes unavailable";
      }
      AXError windowsError;
      id windows = axRead(application, kAXWindowsAttribute, &windowsError);
      if (windowsError != kAXErrorSuccess || ![windows isKindOfClass:[NSArray class]]) {
        result[@"axError"] = [NSString stringWithFormat:@"AXWindows unavailable (%d)", windowsError];
      } else {
        NSMutableArray *observed = [NSMutableArray array];
        for (id window in windows) {
          AXUIElementRef w = (__bridge AXUIElementRef)window;
          NSString *title = ax(w, kAXTitleAttribute);
          NSArray *rect = frame(w);
          if (![title isKindOfClass:[NSString class]] || !rect) {
            result[@"axError"] = @"AX window title or frame unavailable";
            continue;
          }
          NSMutableArray *buttons = [NSMutableArray array];
          CFStringRef keys[] = { kAXCloseButtonAttribute, kAXMinimizeButtonAttribute, kAXZoomButtonAttribute };
          for (int i = 0; i < 3; ++i) {
            id button = ax(w, keys[i]);
            [buttons addObject:button ? (frame((__bridge AXUIElementRef)button) ?: (id)[NSNull null]) : (id)[NSNull null]];
          }
          AXError titleError;
          id titleElement = axRead(w, kAXTitleUIElementAttribute, &titleError);
          NSArray *titleRect = titleElement ? frame((__bridge AXUIElementRef)titleElement) : nil;
          NSString *titleText = titleElement ? (ax((__bridge AXUIElementRef)titleElement, kAXValueAttribute) ?: ax((__bridge AXUIElementRef)titleElement, kAXTitleAttribute)) : nil;
          NSString *titleStatus = titleElement ? @"present" : ((titleError == kAXErrorNoValue || titleError == kAXErrorSuccess) ? @"absent" : @"unavailable");
          int budget = 1500;
          AXUIElementRef area = webArea(w, 0, &budget);
          NSArray *content = area ? frame(area) : nil;
          if (area) CFRelease(area);
          id onScreen = [NSNull null];
          if (result[@"cgError"] == [NSNull null]) {
            onScreen = @NO;
            for (NSDictionary *candidate in rectangles) {
              BOOL equal = YES;
              for (int i = 0; i < 4; ++i) if (fabs([rect[i] doubleValue] - [candidate[@"frame"][i] doubleValue]) > 2.0) equal = NO;
              if (equal) onScreen = @YES;
            }
          }
          [observed addObject:@{@"title":title, @"frame":rect, @"buttons":buttons,
            @"content":content ?: (id)[NSNull null], @"onScreen":onScreen,
            @"titleFrame":titleRect ?: (id)[NSNull null], @"titleText":titleText ?: (id)[NSNull null],
            @"titleStatus":titleStatus, @"titleError":@(titleError)}];
        }
        result[@"windows"] = observed;
      }
    }
    CFRelease(application);
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

  struct Observer { root: PathBuf, binary: PathBuf, pid: u32, manual_previous: Option<bool> }
  impl Drop for Observer {
    fn drop(&mut self) {
      if let Err(error) = self.restore_manual() { eprintln!("{error}"); }
      let _ = fs::remove_dir_all(&self.root);
    }
  }
  impl Observer {
    fn new() -> Check<Self> {
      let root = std::env::temp_dir().join(format!("electron-macos-observer-{}-{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
      fs::create_dir(&root).map_err(|error| format!("UNKNOWN: create observer temp directory: {error}"))?;
      let observer = Self { binary: root.join("observer"), root, pid: 0, manual_previous: None };
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
      match output(Command::new(&self.binary).arg("manual").arg(self.pid.to_string()), WAIT) {
        Ok(bytes) => {
          let value: Value = serde_json::from_slice(&bytes).map_err(|error| format!("UNKNOWN: AXManualAccessibility JSON: {error}"))?;
          self.manual_previous = value["previous"].as_bool();
          eprintln!("native AXManualAccessibility setup: {value}");
        }
        Err(error) => eprintln!("{error}"),
      }
      Ok(())
    }
    fn restore_manual(&mut self) -> Check {
      if let Some(previous) = self.manual_previous.take() {
        output(Command::new(&self.binary).arg("restore-manual").arg(self.pid.to_string()).arg(if previous { "1" } else { "0" }), WAIT)?;
      }
      Ok(())
    }
    fn sample(&self) -> Check<Value> {
      let bytes = output(Command::new(&self.binary).arg("snapshot").arg(self.pid.to_string()), WAIT)?;
      let value: Value = serde_json::from_slice(&bytes).map_err(|error| format!("UNKNOWN: native JSON: {error}"))?;
      if value["pid"].as_u64() != Some(u64::from(self.pid)) { return Err(format!("UNKNOWN: observation PID mismatch: {value}")) }
      Ok(value)
    }
    fn wait(&self, description: &str, check: impl Fn(&Value) -> bool) -> Check<Value> {
      self.wait_for(description, &[], check)
    }
    fn wait_for(&self, description: &str, fields: &[&str], check: impl Fn(&Value) -> bool) -> Check<Value> {
      let started = Instant::now();
      loop {
        let got = self.sample()?;
        for field in fields {
          if got[*field].is_null() {
            return Err(format!("UNKNOWN: {description}: {field} unavailable; external observation: {got}"));
          }
        }
        if check(&got) { return Ok(got) }
        if fields.contains(&"windows") && !got["axError"].is_null() {
          return Err(format!("UNKNOWN: {description}: incomplete AX observation: {got}"));
        }
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
  fn report(member: &str, test: &str, assertion: &str, status: &str) {
    println!("[runtime-evidence] {}", json!({"member":member, "test":format!("macos_behavior_smoke::{test}"), "assertion":assertion, "kind":"behavior", "status":status}));
  }
  fn record(member: &str, test: &str, assertion: &str, result: Check, failures: &mut Vec<String>) {
    match result {
      Ok(()) => report(member, test, assertion, "pass"),
      Err(error) => {
        let status = if error.starts_with("UNKNOWN:") { "unknown" } else { "fail" };
        report(member, test, &error, status);
        if status == "fail" { failures.push(format!("{member}: {error}")); }
      }
    }
  }
  fn native_window<'a>(snapshot: &'a Value, title: &str) -> Option<&'a Value> {
    snapshot["windows"].as_array()?.iter().find(|window| window["title"] == title)
  }
  fn cg_ids(snapshot: &Value) -> Check<Vec<u64>> {
    let windows = snapshot["cgWindows"].as_array().ok_or_else(|| format!("UNKNOWN: CGWindowList unavailable: {snapshot}"))?;
    let mut ids = windows.iter().map(|window| window["id"].as_u64().ok_or("UNKNOWN: CGWindowNumber unavailable".to_owned())).collect::<Check<Vec<_>>>()?;
    ids.sort_unstable();
    require(ids.windows(2).all(|pair| pair[0] != pair[1]), "CGWindowNumbers must identify distinct native windows")?;
    Ok(ids)
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
    let ready = (|| {
      WebviewDispatch::<()>::eval_script_with_callback(&window.webview.as_ref().unwrap().webview.dispatcher,
        "new Promise(resolve => { const ready = () => { document.body.textContent = 'native AX layout proof'; document.body.style.margin = '0'; resolve(true); }; if (document.body) ready(); else document.addEventListener('DOMContentLoaded', ready, {once:true}); })",
        move |value| { let _ = tx.send(value); }).map_err(|error| error.to_string())?;
      require(rx.recv_timeout(WAIT).map_err(|error| error.to_string())? == "true", "real page must initialize before AX layout inspection")
    })();
    if let Err(error) = ready {
      let _ = WindowDispatch::<()>::destroy(&window.dispatcher);
      return Err(error);
    }
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
    let _ = member;
    Ok(())
  }
  fn dock(observer: &Observer, member: &str, mut set: impl FnMut(bool) -> Check) -> Check {
    observer.wait_for("baseline Electron Dock item", &["dockMatches"], |state| state["dockMatches"].as_u64().is_some_and(|count| count > 0))?;
    // Electron and Tao both ignore Dock hide within one second of Dock show.
    thread::sleep(Duration::from_millis(1100));
    observer.wait_for("stable Electron Dock item before hide", &["dockMatches"], |state| state["dockMatches"].as_u64().is_some_and(|count| count > 0))?;
    set(false)?;
    observer.wait_for("Electron's bundle URL must disappear from the real Dock AX tree", &["dockMatches"], |state| state["dockMatches"] == 0)?;
    set(true)?;
    observer.wait_for("Electron's bundle URL must return to the real Dock AX tree", &["dockMatches"], |state| state["dockMatches"].as_u64().is_some_and(|count| count > 0))?;
    let _ = member;
    Ok(())
  }
  fn hide_show(observer: &Observer, first: &Window, hide_member: &str, show_member: &str, hide: impl FnOnce() -> Check, mut show: impl FnMut() -> Check, failures: &mut Vec<String>) {
    let mut ids = Vec::new();
    let hidden = (|| {
      show()?;
      WindowDispatch::<()>::set_focus(&first.dispatcher).map_err(|error| error.to_string())?;
      let baseline = observer.wait_for("two distinct Electron CG windows visible and app foreground before hide", &["cgWindows"], |state| state["active"] == true && cg_ids(state).is_ok_and(|ids| ids.len() == 2))?;
      ids = cg_ids(&baseline)?;
      hide()?;
      observer.wait_for("all-app hide must remove both baseline CG window IDs and foreground ownership", &["cgWindows"], |state| state["hidden"] == true && state["active"] == false && cg_ids(state).is_ok_and(|now| ids.iter().all(|id| !now.contains(id))))?;
      Ok(())
    })();
    let can_show = hidden.is_ok();
    record(hide_member, "app_hide", "NSRunningApplication becomes hidden/inactive and both positively observed Electron CGWindowNumbers disappear", hidden, failures);
    let shown = (|| {
      if !can_show { return Err("UNKNOWN: app.show requires an independently observed hidden baseline; app.hide result is reported separately".into()) }
      let finder = observer.finder()?;
      show()?;
      observer.wait_for("all-app show restores exactly the same two CG window IDs without stealing Finder focus", &["cgWindows", "frontPID"], |state| state["hidden"] == false && state["active"] == false && state["frontPID"].as_u64() == Some(u64::from(finder)) && cg_ids(state).is_ok_and(|now| now == ids))?;
      Ok(())
    })();
    record(show_member, "app_show", "the same two Electron CGWindowNumbers return, isHidden clears and Finder retains foreground ownership", shown, failures);
  }
  fn restore_app(handle: &ElectronRuntimeHandle, failures: &mut Vec<String>) {
    let result = <ElectronRuntimeHandle as RuntimeHandle<()>>::set_activation_policy(handle, ActivationPolicy::Regular)
      .and_then(|_| <ElectronRuntimeHandle as RuntimeHandle<()>>::set_dock_visibility(handle, true))
      .and_then(|_| <ElectronRuntimeHandle as RuntimeHandle<()>>::show(handle));
    if let Err(error) = result { failures.push(format!("FAIL: restore app prerequisites: {error}")); }
  }
  fn runtime_checks(runtime: &mut ElectronRuntime, handle: &ElectronRuntimeHandle, observer: &Observer, first: &Window, failures: &mut Vec<String>) {
    let result = policy(observer, "Runtime::set_activation_policy", |value| { <ElectronRuntime as Runtime<()>>::set_activation_policy(runtime, value); Ok(()) });
    record("Runtime::set_activation_policy", "activation_policy", "external NSRunningApplication reports Accessory, Prohibited and Regular after each mutation", result, failures);
    restore_app(handle, failures);
    let result = dock(observer, "Runtime::set_dock_visibility", |value| { <ElectronRuntime as Runtime<()>>::set_dock_visibility(runtime, value); Ok(()) });
    record("Runtime::set_dock_visibility", "dock_visibility", "complete external Dock AX traversal observes the Electron bundle disappear and reappear after a stable positive baseline", result, failures);
    restore_app(handle, failures);
    hide_show(observer, first, "Runtime::hide", "Runtime::show",
      || { <ElectronRuntime as Runtime<()>>::hide(runtime); Ok(()) },
      || { <ElectronRuntime as Runtime<()>>::show(runtime); Ok(()) }, failures);
    restore_app(handle, failures);
  }
  fn handle_checks(handle: &ElectronRuntimeHandle, observer: &Observer, first: &Window, failures: &mut Vec<String>) {
    let result = policy(observer, "RuntimeHandle::set_activation_policy", |value| <ElectronRuntimeHandle as RuntimeHandle<()>>::set_activation_policy(handle, value).map_err(|error| error.to_string()));
    record("RuntimeHandle::set_activation_policy", "activation_policy", "external NSRunningApplication reports Accessory, Prohibited and Regular after each mutation", result, failures);
    restore_app(handle, failures);
    let result = dock(observer, "RuntimeHandle::set_dock_visibility", |value| <ElectronRuntimeHandle as RuntimeHandle<()>>::set_dock_visibility(handle, value).map_err(|error| error.to_string()));
    record("RuntimeHandle::set_dock_visibility", "dock_visibility", "complete external Dock AX traversal observes the Electron bundle disappear and reappear after a stable positive baseline", result, failures);
    restore_app(handle, failures);
    hide_show(observer, first, "RuntimeHandle::hide", "RuntimeHandle::show",
      || <ElectronRuntimeHandle as RuntimeHandle<()>>::hide(handle).map_err(|error| error.to_string()),
      || <ElectronRuntimeHandle as RuntimeHandle<()>>::show(handle).map_err(|error| error.to_string()), failures);
    restore_app(handle, failures);
  }

  fn numbers(value: &Value) -> Check<[f64;4]> {
    let values = value.as_array().filter(|values| values.len() == 4).ok_or_else(|| format!("UNKNOWN: AX frame unavailable: {value}"))?;
    let mut result = [0.0;4];
    for (index, value) in values.iter().enumerate() { result[index] = value.as_f64().ok_or("UNKNOWN: nonnumeric AX frame")?; }
    Ok(result)
  }
  fn caption(observer: &Observer, title: &str) -> Check<Value> {
    let snapshot = observer.wait_for("new titlebar window must exist in the external AX/CG tree", &["windows", "cgWindows"], |state| native_window(state, title).is_some_and(|window| window["onScreen"] == true))?;
    Ok(native_window(&snapshot, title).unwrap().clone())
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
  fn titlebars(handle: &ElectronRuntimeHandle, observer: &Observer, failures: &mut Vec<String>) {
    for (case, member, assertion) in [
      ("title_bar_style", "WindowBuilder::title_bar_style", "Visible/Transparent retain native AXWebArea caption inset; Overlay extends beneath the caption, with all three native controls retained"),
      ("hidden_title", "WindowBuilder::hidden_title", "a readable visible-title baseline changes to confirmed absent/hidden native title text without moving controls or changing content inset"),
      ("builder_traffic_light_position", "WindowBuilder::traffic_light_position", "two fresh Overlay windows' three native AX control frames differ by the requested 26x16 logical delta"),
      ("dispatch_traffic_light_position", "WindowDispatch::set_traffic_light_position", "logical and physical requests move all three native AX control frames by distinct correct logical deltas"),
    ] {
      let mut cleanup = Vec::new();
      let mut make = |title: &str, builder| -> Check<Window> {
        let window = create(handle, title, builder)?;
        cleanup.push(window.dispatcher.clone());
        Ok(window)
      };
      let mut result = (|| {
        match case {
          "title_bar_style" => {
            let _normal = make("title-visible", builder("title-visible").title_bar_style(TitleBarStyle::Visible).hidden_title(false))?;
            let normal = caption(observer, "title-visible")?;
            let _transparent = make("title-transparent", builder("title-transparent").title_bar_style(TitleBarStyle::Transparent).hidden_title(false))?;
            let transparent = caption(observer, "title-transparent")?;
            let _overlay = make("title-overlay", builder("title-overlay").title_bar_style(TitleBarStyle::Overlay).hidden_title(false))?;
            let overlay = caption(observer, "title-overlay")?;
            for state in [&normal, &transparent, &overlay] {
              for index in 0..3 { numbers(&state["buttons"][index])?; }
            }
            require(inset(&normal)? >= 16.0, "Visible titlebar must keep real web content below the titlebar")?;
            require((inset(&transparent)? - inset(&normal)?).abs() <= 2.0, "Transparent titlebar must retain normal content inset, unlike Overlay")?;
            require(inset(&overlay)?.abs() <= 2.0, "Overlay must place the actual AXWebArea underneath the titlebar")?;
          }
          "hidden_title" => {
            let _visible = make("title-visible", builder("title-visible").title_bar_style(TitleBarStyle::Visible).hidden_title(false))?;
            let normal = caption(observer, "title-visible")?;
            if normal["titleStatus"] != "present" || normal["titleText"].is_null() || normal["titleFrame"].is_null() {
              return Err(format!("UNKNOWN: visible native title baseline unavailable: {normal}"));
            }
            require(normal["titleText"] == "title-visible", "visible native title must match the requested title")?;
            let _hidden = make("title-hidden", builder("title-hidden").title_bar_style(TitleBarStyle::Visible).hidden_title(true))?;
            let hidden = caption(observer, "title-hidden")?;
            if hidden["titleStatus"] == "unavailable" || (hidden["titleStatus"] == "present" && (hidden["titleText"].is_null() || hidden["titleFrame"].is_null())) {
              return Err(format!("UNKNOWN: hidden-title AX query unavailable, not proof of absence: {hidden}"));
            }
            require(hidden["titleStatus"] == "absent" || hidden["titleText"] == "", "hidden_title(true) must remove the actual native title text")?;
            require((inset(&hidden)? - inset(&normal)?).abs() <= 2.0, "hiding title text must preserve Visible content layout")?;
            controls_shift(&normal, &hidden, 0.0, 0.0)?;
          }
          "builder_traffic_light_position" => {
            let _a = make("buttons-a", builder("buttons-a").title_bar_style(TitleBarStyle::Overlay).traffic_light_position(LogicalPosition::new(12.0,14.0)))?;
            let a = caption(observer, "buttons-a")?;
            let _b = make("buttons-b", builder("buttons-b").title_bar_style(TitleBarStyle::Overlay).traffic_light_position(LogicalPosition::new(38.0,30.0)))?;
            let b = caption(observer, "buttons-b")?;
            controls_shift(&a, &b, 26.0, 16.0)?;
          }
          "dispatch_traffic_light_position" => {
            let moved = make("buttons-dispatch", builder("buttons-dispatch").title_bar_style(TitleBarStyle::Overlay).traffic_light_position(LogicalPosition::new(12.0,14.0)))?;
            let initial = caption(observer, "buttons-dispatch")?;
            WindowDispatch::<()>::set_traffic_light_position(&moved.dispatcher, LogicalPosition::new(42.0,34.0).into()).map_err(|error| error.to_string())?;
            let logical = caption(observer, "buttons-dispatch")?;
            controls_shift(&initial, &logical, 30.0, 20.0)?;
            let scale = WindowDispatch::<()>::scale_factor(&moved.dispatcher).map_err(|error| error.to_string())?;
            let point: PhysicalPosition<i32> = LogicalPosition::new(26.0,22.0).to_physical(scale);
            WindowDispatch::<()>::set_traffic_light_position(&moved.dispatcher, point.into()).map_err(|error| error.to_string())?;
            let physical = caption(observer, "buttons-dispatch")?;
            controls_shift(&logical, &physical, -16.0, -12.0)?;
          }
          _ => unreachable!(),
        }
        Ok(())
      })();
      drop(make);
      for window in cleanup {
        if let Err(error) = WindowDispatch::<()>::destroy(&window) {
          failures.push(format!("FAIL: {case} cleanup: {error}"));
          if result.is_ok() { result = Err(format!("FAIL: {case} cleanup: {error}")); }
        }
      }
      record(member, case, assertion, result, failures);
    }
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
    let mut failures = Vec::new();
    runtime_checks(&mut runtime, &handle, &observer, &first, &mut failures);
    let worker_handle = handle.clone();
    let worker = thread::spawn(move || {
      handle_checks(&worker_handle, &observer, &first, &mut failures);
      titlebars(&worker_handle, &observer, &mut failures);
      restore_app(&worker_handle, &mut failures);
      if initial_front != 0 && initial_front != observer.pid {
        if let Err(error) = observer.activate(initial_front) { eprintln!("{error}"); }
      }
      if let Err(error) = observer.restore_manual() { eprintln!("{error}"); }
      for window in [&first, &second] {
        if let Err(error) = WindowDispatch::<()>::destroy(&window.dispatcher) {
          failures.push(format!("FAIL: app window cleanup: {error}"));
        }
      }
      let result = if failures.is_empty() { Ok(()) } else { Err(failures.join("\n")) };
      let _ = <ElectronRuntimeHandle as RuntimeHandle<()>>::request_exit(&worker_handle, i32::from(result.is_err()));
      result
    });
    let status = <ElectronRuntime as Runtime<()>>::run_return(runtime, |_| {});
    let result = worker.join().map_err(|_| "FAIL: macOS behavior worker panicked".to_string())?;
    let _ = done_tx.send(());
    result?;
    require(status == 0, "real Electron runtime must exit zero")?;
    println!("macOS external system cases completed; member markers retain unavailable observations as unknown");
    Ok(())
  }
}
