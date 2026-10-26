//! Real Chromium window.open regression: no page mocks or replacement navigations.
use std::{
  collections::HashMap,
  io::{Read, Write},
  net::{TcpListener, TcpStream},
  process::ExitCode,
  sync::{Arc, Mutex, atomic::{AtomicUsize, Ordering}, mpsc},
  thread,
  time::{Duration, Instant},
};
use tauri_runtime::{
  Runtime, RuntimeHandle, RuntimeInitArgs, WebviewDispatch, WindowDispatch,
  window::{PendingWindow, RawWindow, WindowBuilder, WindowEvent},
  webview::{PendingWebview, WebviewAttributes},
};
use tauri_runtime_electron::{
  ElectronRuntime, ElectronRuntimeHandle, ElectronWebviewDispatcher,
  ElectronWindowBuilder, ElectronWindowDispatcher,
  new_window::{with_opener, NewWindowOpener, NewWindowResponse},
};
use tauri_utils::config::{WebviewUrl, WindowConfig};

type Target = (ElectronWindowDispatcher, ElectronWebviewDispatcher);
type Requests = Arc<Mutex<Vec<(String, String, HashMap<String, String>, Vec<u8>)>>>;
const WAIT: Duration = Duration::from_secs(8);

fn main() -> ExitCode {
  let runtime = <ElectronRuntime as Runtime<()>>::new(RuntimeInitArgs::default()).expect("start Electron");
  let handle = <ElectronRuntime as Runtime<()>>::handle(&runtime);
  thread::spawn(|| {
    thread::sleep(Duration::from_secs(100));
    eprintln!("native popup regression exceeded 100s");
    std::process::abort();
  });
  let worker = thread::spawn(move || {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| exercise(&handle)));
    // Closing the last native popup can finish run_return before this worker
    // sends its explicit exit request. The exercise result still controls failure.
    if let Err(error) = <ElectronRuntimeHandle as RuntimeHandle<()>>::request_exit(&handle, i32::from(result.is_err())) {
      eprintln!("runtime exited before the smoke worker's exit request: {error}");
    }
    result
  });
  let status = <ElectronRuntime as Runtime<()>>::run_return(runtime, |_| {});
  if worker.join().expect("popup test thread").is_ok() && status == 0 { ExitCode::SUCCESS }
  else { ExitCode::FAILURE }
}

fn create(handle: &ElectronRuntimeHandle, label: &str, url: &str, init: &str) -> tauri_runtime::Result<Target> {
  create_with_attributes(handle, label, url, WebviewAttributes::new(WebviewUrl::External(url.parse().unwrap())).initialization_script(init))
}

fn create_with_attributes(handle: &ElectronRuntimeHandle, label: &str, url: &str, attributes: WebviewAttributes) -> tauri_runtime::Result<Target> {
  let config = WindowConfig { width: 420.0, height: 300.0, visible: true, ..Default::default() };
  let mut pending = PendingWindow::<(), ElectronRuntime>::new(ElectronWindowBuilder::with_config(&config), label)?;
  let mut webview = PendingWebview::new(attributes, label)?;
  webview.url = url.into();
  pending.set_webview(webview);
  let detached = <ElectronRuntimeHandle as RuntimeHandle<()>>::create_window(handle, pending, None::<fn(RawWindow<'_>)>)?;
  Ok((detached.dispatcher, detached.webview.unwrap().webview.dispatcher))
}

fn eval(webview: &ElectronWebviewDispatcher, script: &str) -> serde_json::Value {
  let (tx, rx) = mpsc::channel();
  WebviewDispatch::<()>::eval_script_with_callback(webview, script, move |value| { let _ = tx.send(value); }).unwrap();
  serde_json::from_str(&rx.recv_timeout(WAIT).expect("renderer evaluation timed out")).expect("JSON result")
}

fn expect(webview: &ElectronWebviewDispatcher, script: &str, want: serde_json::Value) {
  let start = Instant::now();
  loop {
    let got = eval(webview, script);
    if got == want { return }
    assert!(start.elapsed() < WAIT, "{script}: got {got}, want {want}");
    thread::sleep(Duration::from_millis(40));
  }
}

fn evidence(member: &str, assertion: &str) {
  println!("[runtime-evidence] {}", serde_json::json!({
    "member": member, "test": "new_window_smoke::exercise", "assertion": assertion,
    "kind": "behavior", "status": "pass",
  }));
}

fn exercise(handle: &ElectronRuntimeHandle) {
  use serde_json::json;
  let requests: Requests = Default::default();
  let origin = server(requests.clone());
  let other_origin = server(requests.clone());
  let (main_window, main) = create(handle, "popup-opener", &format!("{origin}/opener"), "window.openerInit = true").unwrap();
  expect(&main, "document.title", json!("/opener"));
  // Default behavior is allow, not the former unconditional deny.
  assert_eq!(eval(&main, &format!("window.defaultPopup = window.open('{origin}/default'); !!defaultPopup")), true);
  expect(&main, "defaultPopup.document.title", json!("/default"));
  assert_eq!(eval(&main, "defaultPopup.opener === window"), true);
  eval(&main, "defaultPopup.close(); true");
  expect(&main, "defaultPopup.closed", json!(true));

  let targets: Arc<Mutex<HashMap<String, Target>>> = Default::default();
  let stale: Arc<Mutex<Option<NewWindowOpener>>> = Default::default();
  let geometries = Arc::new(Mutex::new(Vec::new()));
  let count = Arc::new(AtomicUsize::new(0));
  let destroyed: Arc<Mutex<Vec<String>>> = Default::default();
  let destroyed_in_handler = destroyed.clone();
  let rejected_sessions = Arc::new(AtomicUsize::new(0));
  let rejected_in_handler = rejected_sessions.clone();
  let created = targets.clone();
  let saved = stale.clone();
  let observed_geometry = geometries.clone();
  let calls = count.clone();
  let runtime_handle = handle.clone();
  let invalid = main_window.clone();
  main.on_new_window(move |url, features| {
    calls.fetch_add(1, Ordering::SeqCst);
    *saved.lock().unwrap() = Some(features.opener());
    observed_geometry.lock().unwrap().push((features.frame_name.clone(), features.size, features.position));
    match url.path() {
      "/deny" => NewWindowResponse::Deny,
      "/allow" => NewWindowResponse::Allow,
      "/invalid" => NewWindowResponse::Create { window: invalid.clone() },
      "/incompatible-session" => {
        let result = with_opener(features.opener(), || create_with_attributes(
          &runtime_handle, "private-target", "about:blank",
          WebviewAttributes::new(WebviewUrl::External("about:blank".parse().unwrap())).incognito(true),
        ));
        assert!(result.is_err(), "an incognito target must not silently use its opener's persistent session");
        rejected_in_handler.fetch_add(1, Ordering::SeqCst);
        NewWindowResponse::Deny
      }
      _ => {
        let label = format!("native-{}", calls.load(Ordering::SeqCst));
        // If this URL were loaded instead of Chromium's guest, tests below would
        // see the wrong title, lose POST/referrer, and return a null WindowProxy.
        let target = with_opener(features.opener(), || create(
          &runtime_handle, &label, "about:blank#must-not-navigate", "window.createdInit = 'before-page'",
        )).expect("create native popup destination");
        let destroyed = destroyed_in_handler.clone();
        let path = url.path().to_owned();
        WindowDispatch::<()>::on_window_event(&target.0, move |event| {
          if matches!(event, WindowEvent::Destroyed) { destroyed.lock().unwrap().push(path.clone()); }
        });
        let response = match url.path() {
          "/unused-deny" => NewWindowResponse::Deny,
          "/unused-allow" => NewWindowResponse::Allow,
          "/unused-invalid" => NewWindowResponse::Create { window: invalid.clone() },
          _ => NewWindowResponse::Create { window: target.0.clone() },
        };
        created.lock().unwrap().insert(url.path().into(), target);
        response
      }
    }
  }).unwrap();
  assert_eq!(eval(&main, &format!("window.open('{origin}/deny') === null")), true);
  assert_eq!(eval(&main, &format!("window.open('{origin}/invalid') === null")), true);
  for (path, allowed) in [("/unused-deny", false), ("/unused-allow", true), ("/unused-invalid", false)] {
    assert_eq!(eval(&main, &format!("window.discarded = window.open('{origin}{path}'); !!discarded")), allowed);
    if allowed {
      expect(&main, "discarded.document.title", json!(path));
      assert_eq!(eval(&main, "discarded.opener === window && discarded.createdInit === undefined"), true);
      eval(&main, "discarded.close(); true");
      expect(&main, "discarded.closed", json!(true));
    }
    let start = Instant::now();
    while !destroyed.lock().unwrap().iter().any(|closed| closed == path) {
      assert!(start.elapsed() < WAIT, "discarded {path} must deliver Destroyed instead of silently forgetting its registry entry");
      thread::sleep(Duration::from_millis(20));
    }
    let abandoned = targets.lock().unwrap().get(path).unwrap().0.clone();
    assert!(WindowDispatch::<()>::title(&abandoned).is_err(), "discarded {path} must leave no native shell");
  }
  assert_eq!(eval(&main, &format!("window.open('{origin}/incompatible-session') === null")), true);
  assert_eq!(rejected_sessions.load(Ordering::SeqCst), 1);
  assert_eq!(eval(&main, "document.title"), "/opener", "a rejected private target must leave its opener alive");
  assert_eq!(eval(&main, &format!("window.allowed = window.open('{origin}/allow'); !!allowed")), true);
  expect(&main, "allowed.document.title", json!("/allow"));
  assert_eq!(eval(&main, "allowed.opener === window"), true);
  eval(&main, "allowed.close(); true");
  expect(&main, "allowed.closed", json!(true));

  assert_eq!(eval(&main, &format!("window.created = window.open('{origin}/create', 'native-named', 'width=333,height=222,left=40,top=50'); !!created")), true);
  expect(&main, "created.document.title", json!("/create"));
  assert_eq!(eval(&main, "[created.opener === window, created.createdInit, typeof created.require, typeof created.process, created.document.referrer]"),
    json!([true, "before-page", "undefined", "undefined", format!("{origin}/opener")]));
  let (popup_window, popup) = targets.lock().unwrap().get("/create").unwrap().clone();
  assert_eq!(WebviewDispatch::<()>::url(&popup).unwrap(), eval(&popup, "location.href").as_str().unwrap(),
    "adopted popup URL must be the native target, not the builder's placeholder");
  let cookie = tauri_runtime::Cookie::build(("nativePopupCookie", "actual-url")).path("/").build();
  assert!(cookie.domain().is_none());
  WebviewDispatch::<()>::set_cookie(&popup, cookie).unwrap();
  expect(&popup, "document.cookie.includes('nativePopupCookie=actual-url')", json!(true));
  assert_eq!(eval(&popup, "window.opener.document.title"), "/opener");
  assert_eq!(eval(&main, "created.document.body.dataset.fromOpener = 'real'; created.document.body.dataset.fromOpener"), "real");
  assert_eq!(eval(&popup, "document.body.dataset.fromOpener"), "real");
  eval(&popup, "opener.postMessage('from-native-child', location.origin); true");
  // Install both sides before sending, to prove native WindowProxy message routing.
  eval(&main, "window.messages = []; onmessage = e => messages.push([e.data, e.source === created]); true");
  eval(&popup, "opener.postMessage('native-message', location.origin); true");
  expect(&main, "messages.some(([data, source]) => data === 'native-message' && source)", json!(true));
  let before_reuse = count.load(Ordering::SeqCst);
  assert_eq!(eval(&main, &format!("window.open('{origin}/create-reused', 'native-named') === created")), true);
  expect(&main, "created.document.title", json!("/create-reused"));
  assert_eq!(count.load(Ordering::SeqCst), before_reuse, "named window reuse must not create another guest");
  assert_eq!(WebviewDispatch::<()>::url(&popup).unwrap(), eval(&popup, "location.href").as_str().unwrap());
  eval(&popup, "setTimeout(() => { location.href = '/popup-self-navigation' }, 0); true");
  expect(&popup, "document.title", json!("/popup-self-navigation"));
  assert_eq!(WebviewDispatch::<()>::url(&popup).unwrap(), eval(&popup, "location.href").as_str().unwrap(),
    "page-initiated navigation must update the native dispatcher URL");
  eval(&popup, "location.hash = 'native-popup-hash'; true");
  assert_eq!(WebviewDispatch::<()>::url(&popup).unwrap(), eval(&popup, "location.href").as_str().unwrap(),
    "same-document hash changes must remain visible through the native URL API");
  WebviewDispatch::<()>::delete_cookie(&popup, tauri_runtime::Cookie::new("nativePopupCookie", "actual-url")).unwrap();
  expect(&popup, "document.cookie.includes('nativePopupCookie=actual-url')", json!(false));
  let geometry = geometries.lock().unwrap().iter().find(|(name, _, _)| name == "native-named").unwrap().clone();
  assert_eq!(geometry.1.unwrap().width, 333.0);
  assert_eq!(geometry.1.unwrap().height, 222.0);
  assert_eq!(geometry.2.unwrap().x, 40.0);
  assert_eq!(geometry.2.unwrap().y, 50.0);
  WindowDispatch::<()>::set_title(&popup_window, "managed-native").unwrap();
  assert_eq!(WindowDispatch::<()>::title(&popup_window).unwrap(), "managed-native");

  // Native browser semantics: cross-origin DOM access denied, messages allowed.
  assert_eq!(eval(&main, &format!("window.cross = window.open('{other_origin}/cross'); !!cross")), true);
  let cross = targets.lock().unwrap().get("/cross").unwrap().1.clone();
  expect(&cross, "document.title", json!("/cross"));
  assert_eq!(eval(&main, "(() => { try { return cross.document.title } catch(e) { return e.name } })()"), "SecurityError");
  eval(&cross, "window.fromParent = null; onmessage = e => fromParent = [e.data, e.origin, e.source === opener]; true");
  eval(&main, &format!("cross.postMessage('cross-message', '{other_origin}'); true"));
  expect(&cross, "fromParent", json!(["cross-message", origin, true]));

  assert_eq!(eval(&main, &format!("window.open('{origin}/noopener', '_blank', 'noopener') === null")), true);
  let no_opener = targets.lock().unwrap().get("/noopener").unwrap().1.clone();
  expect(&no_opener, "document.title", json!("/noopener"));
  assert_eq!(eval(&no_opener, "opener === null"), true);
  assert_eq!(eval(&main, &format!("window.open('{origin}/noreferrer', '_blank', 'noreferrer') === null")), true);
  let no_referrer = targets.lock().unwrap().get("/noreferrer").unwrap().1.clone();
  expect(&no_referrer, "document.title", json!("/noreferrer"));
  assert_eq!(eval(&no_referrer, "[opener === null, document.referrer]"), json!([true, ""]));

  // Target form navigation must remain POST with the browser's original body.
  eval(&main, &format!("(() => {{ const f = document.createElement('form'); f.action = '{origin}/post'; f.method = 'POST'; f.target = '_blank'; f.innerHTML = '<input name=proof value=native-post>'; document.body.append(f); f.submit(); return true }})()"));
  let start = Instant::now();
  loop {
    if requests.lock().unwrap().iter().any(|(method, path, headers, body)| method == "POST" && path == "/post"
      && headers.get("referer") == Some(&format!("{origin}/opener")) && body == b"proof=native-post") { break }
    assert!(start.elapsed() < WAIT, "native target form lost POST body or referrer: {:?}", requests.lock().unwrap());
    thread::sleep(Duration::from_millis(40));
  }
  // An explicitly compatible persistent session must still adopt the native guest.
  let directory = std::env::temp_dir().join(format!("electron-popup-session-{}", std::process::id()));
  let session_url = format!("{origin}/session-opener");
  let (session_window, session_opener) = create_with_attributes(handle, "session-opener", &session_url,
    WebviewAttributes::new(WebviewUrl::External(session_url.parse().unwrap())).data_directory(directory.clone()),
  ).unwrap();
  expect(&session_opener, "document.title", json!("/session-opener"));
  eval(&session_opener, "document.cookie = 'nativeSession=same; path=/'; true");
  let session_handle = handle.clone();
  session_opener.on_new_window(move |_url, features| {
    let target = with_opener(features.opener(), || create_with_attributes(
      &session_handle, "session-target", "about:blank",
      WebviewAttributes::new(WebviewUrl::External("about:blank".parse().unwrap())).data_directory(directory.join(".")),
    )).expect("compatible data directory must keep its native opener");
    NewWindowResponse::Create { window: target.0 }
  }).unwrap();
  assert_eq!(eval(&session_opener, &format!("window.sameSession = window.open('{origin}/same-session'); !!sameSession")), true);
  expect(&session_opener, "sameSession.document.title", json!("/same-session"));
  assert_eq!(eval(&session_opener, "sameSession.opener === window && sameSession.document.cookie.includes('nativeSession=same')"), true);
  eval(&session_opener, "sameSession.close(); true");
  expect(&session_opener, "sameSession.closed", json!(true));
  WindowDispatch::<()>::destroy(&session_window).unwrap();

  // Opener tokens must expire instead of attaching to another request later.
  let expired = stale.lock().unwrap().clone().unwrap();
  assert!(with_opener(expired, || create(handle, "expired-opener", "about:blank", "")).is_err());

  eval(&main, "created.close(); true");
  expect(&main, "created.closed", json!(true));
  assert!(WindowDispatch::<()>::title(&popup_window).is_err(), "native close must forget the managed destination");
  // The child must survive its opener navigating to a different document.
  WebviewDispatch::<()>::navigate(&main, format!("{origin}/opener-after").parse().unwrap()).unwrap();
  expect(&main, "document.title", json!("/opener-after"));
  assert_eq!(eval(&cross, "[document.title, opener !== null]"), json!(["/cross", true]));
  WindowDispatch::<()>::destroy(&main_window).unwrap();
  expect(&cross, "document.title", json!("/cross"));
  for (window, _) in targets.lock().unwrap().values() { let _ = WindowDispatch::<()>::destroy(window); }
  evidence("ElectronWebviewDispatcher::on_new_window", "默认 Allow、自定义 Allow/Create/Deny、真实 WindowProxy/opener、命名复用、跨源隔离与消息、noopener/noreferrer、POST/referrer、关闭及 opener 生命周期");
  evidence("tauri_runtime_electron::with_opener", "同请求接管 Chromium guest；过期 opener 拒绝，未重新导航目标页");
  evidence("NewWindowFeatures::opener", "原生请求一次性令牌用于构建目标；尺寸333×222和位置40,50保持");
  println!("native Chromium popup regressions passed");
}

fn server(requests: Requests) -> String {
  let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
  let origin = format!("http://{}", listener.local_addr().unwrap());
  thread::spawn(move || {
    for stream in listener.incoming() {
      let requests = requests.clone();
      thread::spawn(move || serve(stream.unwrap(), requests));
    }
  });
  origin
}

fn serve(mut stream: TcpStream, requests: Requests) {
  stream.set_read_timeout(Some(WAIT)).unwrap();
  let mut bytes = Vec::new();
  let mut chunk = [0; 4096];
  let end = loop {
    let n = match stream.read(&mut chunk) { Ok(0) | Err(_) => return, Ok(n) => n };
    bytes.extend_from_slice(&chunk[..n]);
    if let Some(index) = bytes.windows(4).position(|window| window == b"\r\n\r\n") { break index + 4 }
    assert!(bytes.len() < 64 * 1024, "HTTP test header too large");
  };
  let head = String::from_utf8_lossy(&bytes[..end]);
  let mut lines = head.split("\r\n");
  let mut request = lines.next().unwrap().split_whitespace();
  let method = request.next().unwrap().to_string();
  let path = request.next().unwrap().to_string();
  let headers: HashMap<_, _> = lines.filter_map(|line| line.split_once(':'))
    .map(|(key, value)| (key.to_ascii_lowercase(), value.trim().to_string())).collect();
  let length = headers.get("content-length").map(|value| value.parse::<usize>().unwrap()).unwrap_or(0);
  assert!(length < 64 * 1024, "HTTP test body too large");
  while bytes.len() < end + length {
    let n = stream.read(&mut chunk).unwrap();
    if n == 0 { return }
    bytes.extend_from_slice(&chunk[..n]);
  }
  requests.lock().unwrap().push((method, path.clone(), headers, bytes[end..end + length].to_vec()));
  let body = format!("<!doctype html><title>{path}</title><body>native popup test</body>");
  let response = format!("HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
  let _ = stream.write_all(response.as_bytes());
}
