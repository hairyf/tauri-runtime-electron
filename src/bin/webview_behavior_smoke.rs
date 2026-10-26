//! Real child WebContentsView effects; getters alone and successful no-ops are not evidence.
use serde_json::{json, Value};
use std::{
  io::{Read, Write},
  net::{TcpListener, TcpStream},
  process::ExitCode,
  sync::{
    atomic::{AtomicUsize, Ordering},
    mpsc, Arc,
  },
  thread,
  time::{Duration, Instant},
};
use tauri_runtime::{
  dpi::{LogicalPosition, LogicalSize, Rect},
  webview::{PendingWebview, WebviewAttributes},
  window::{DetachedWindow, PendingWindow, RawWindow, WindowBuilder, WindowEvent, WindowId},
  Cookie, ExitRequestedEventAction, RunEvent, Runtime, RuntimeHandle, RuntimeInitArgs,
  WebviewDispatch, WindowDispatch,
};
use tauri_runtime_electron::{
  ElectronRuntime, ElectronRuntimeHandle, ElectronWebviewDispatcher, ElectronWindowBuilder,
  ElectronWindowDispatcher,
};
use tauri_utils::config::{WebviewUrl, WindowConfig};

type Webview = ElectronWebviewDispatcher;
type Window = DetachedWindow<(), ElectronRuntime>;
const WAIT: Duration = Duration::from_secs(8);

fn main() -> ExitCode {
  let runtime =
    <ElectronRuntime as Runtime<()>>::new(RuntimeInitArgs::default()).expect("start real Electron");
  let handle = <ElectronRuntime as Runtime<()>>::handle(&runtime);
  thread::spawn(|| {
    thread::sleep(Duration::from_secs(100));
    eprintln!("child webview behavior regression exceeded 100s");
    std::process::abort();
  });
  let worker = thread::spawn(move || {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| exercise(&handle)));
    let _ = <ElectronRuntimeHandle as RuntimeHandle<()>>::request_exit(
      &handle,
      i32::from(result.is_err()),
    );
    result
  });
  let status = <ElectronRuntime as Runtime<()>>::run_return(runtime, |event| {
    if let RunEvent::ExitRequested { code: None, tx } = event {
      let _ = tx.send(ExitRequestedEventAction::Prevent);
    }
  });
  if worker.join().expect("join webview behavior worker").is_ok() && status == 0 {
    ExitCode::SUCCESS
  } else {
    ExitCode::FAILURE
  }
}

fn window(
  handle: &ElectronRuntimeHandle,
  label: &str,
  url: &str,
  ipc: Option<mpsc::Sender<String>>,
) -> Window {
  let config = WindowConfig {
    width: 640.0,
    height: 480.0,
    x: Some(80.0),
    y: Some(80.0),
    visible: true,
    // BaseWindow uses real Windows occlusion; keep the test console off the page.
    always_on_top: cfg!(windows),
    ..Default::default()
  };
  let mut pending =
    PendingWindow::<(), ElectronRuntime>::new(ElectronWindowBuilder::with_config(&config), label)
      .unwrap();
  let mut page = PendingWebview::new(
    WebviewAttributes::new(WebviewUrl::External(url.parse().unwrap())),
    label,
  )
  .unwrap();
  page.url = url.into();
  if let Some(ipc) = ipc {
    page.ipc_handler = Some(Box::new(move |_, request| {
      let _ = ipc.send(request.body().clone());
    }));
  }
  pending.set_webview(page);
  <ElectronRuntimeHandle as RuntimeHandle<()>>::create_window(
    handle,
    pending,
    None::<fn(RawWindow<'_>)>,
  )
  .expect("create native host window")
}

fn main_page(window: &Window) -> Webview {
  window.webview.as_ref().unwrap().webview.dispatcher.clone()
}

fn eval(webview: &Webview, script: &str) -> Value {
  let (tx, rx) = mpsc::channel();
  WebviewDispatch::<()>::eval_script_with_callback(webview, script, move |value| {
    let _ = tx.send(value);
  })
  .expect("evaluate real child renderer");
  serde_json::from_str(
    &rx
      .recv_timeout(WAIT)
      .expect("renderer evaluation timed out"),
  )
  .expect("JSON renderer result")
}

fn expect(webview: &Webview, script: &str, want: Value) {
  let start = Instant::now();
  loop {
    let got = eval(webview, script);
    if got == want {
      return;
    }
    assert!(start.elapsed() < WAIT, "{script}: got {got}, want {want}; renderer={}", eval(webview, "JSON.stringify({visibility:document.visibilityState,focus:document.hasFocus(),samples:window.resizeSamples,viewport:[innerWidth,innerHeight],root:document.documentElement.getBoundingClientRect().toJSON()})"));
    thread::sleep(Duration::from_millis(40));
  }
}

fn rect(webview: &Webview) -> [i32; 4] {
  let rect = WebviewDispatch::<()>::bounds(webview).expect("read native child bounds");
  let position = rect.position.to_physical::<i32>(1.0);
  let size = rect.size.to_physical::<u32>(1.0);
  [
    position.x,
    position.y,
    size.width as i32,
    size.height as i32,
  ]
}

fn physical(scale: f64, logical: [i32; 4]) -> [i32; 4] {
  logical.map(|value| (f64::from(value) * scale).round() as i32)
}

fn layout(webview: &Webview, scale: f64, logical: [i32; 4]) {
  expect(
    webview,
    "[innerWidth, innerHeight]",
    json!([logical[2], logical[3]]),
  );
  assert_eq!(
    rect(webview),
    physical(scale, logical),
    "native child rect must agree with the renderer viewport"
  );
}

fn destroyed(window: &ElectronWindowDispatcher) -> mpsc::Receiver<()> {
  let (tx, rx) = mpsc::channel();
  WindowDispatch::<()>::on_window_event(window, move |event| {
    if matches!(event, WindowEvent::Destroyed) {
      let _ = tx.send(());
    }
  });
  rx
}

fn evidence(member: &str, test: &str, assertion: &str) {
  println!(
    "[runtime-evidence] {}",
    json!({
      "member": member, "test": format!("webview_behavior_smoke::{test}"),
      "assertion": assertion, "kind": "behavior", "status": "pass",
    })
  );
}

fn exercise(handle: &ElectronRuntimeHandle) {
  let origin = server();
  let source = window(handle, "behavior-source", &format!("{origin}/source"), None);
  let source_main = main_page(&source);
  expect(&source_main, "location.pathname", json!("/source"));
  expect(&source_main, "document.readyState", json!("complete"));
  let (ipc_tx, ipc_rx) = mpsc::channel();
  let mut attributes = WebviewAttributes::new(WebviewUrl::External(
    format!("{origin}/scope/page").parse().unwrap(),
  ))
  .incognito(true)
  .focused(false);
  attributes.bounds = Some(Rect {
    position: LogicalPosition::new(20.0, 30.0).into(),
    size: LogicalSize::new(240.0, 160.0).into(),
  });
  let mut pending =
    PendingWebview::<(), ElectronRuntime>::new(attributes, "behavior-child").unwrap();
  pending.url = format!("{origin}/scope/page");
  pending.ipc_handler = Some(Box::new(move |_, request| {
    let _ = ipc_tx.send(request.body().clone());
  }));
  let child =
    <ElectronRuntimeHandle as RuntimeHandle<()>>::create_webview(handle, source.id, pending)
      .expect("create real native WebContentsView")
      .dispatcher;
  expect(&child, "document.readyState", json!("complete"));
  expect(&child, "location.pathname", json!("/scope/page"));
  assert_eq!(
    eval(&child, "[typeof require, typeof process, typeof Buffer]"),
    json!(["undefined", "undefined", "undefined"])
  );
  let scale = WindowDispatch::<()>::scale_factor(&source.dispatcher).unwrap();
  assert_eq!(eval(&child, "devicePixelRatio"), json!(scale));
  layout(&child, scale, [20, 30, 240, 160]);
  evidence("WebviewDispatch::bounds", "bounds", "native child bounds equal requested parent-relative rect and physical renderer viewport, not the host window rect");
  assert_eq!(
    WebviewDispatch::<()>::position(&child).unwrap(),
    LogicalPosition::new(20.0, 30.0).to_physical::<i32>(scale)
  );
  evidence("WebviewDispatch::position", "position", "child position is the requested parent-relative physical offset rather than the parent's screen position");
  assert_eq!(
    WebviewDispatch::<()>::size(&child).unwrap(),
    LogicalSize::new(240.0, 160.0).to_physical::<u32>(scale)
  );
  assert_eq!(eval(&child, "[innerWidth, innerHeight]"), json!([240, 160]));
  evidence(
    "WebviewDispatch::size",
    "size",
    "child native physical size corresponds to its real 240x160 renderer viewport",
  );

  geometry(&child, scale);
  reload(&child);
  focus_visibility(&source.dispatcher, &source_main, &child);
  auto_resize(&source.dispatcher, &source_main, &child, scale);
  browsing_data(&child, &origin);
  reparent_main(handle, &origin);
  // Do not occlude the source while testing real renderer ResizeObserver updates.
  let target = window(handle, "behavior-target", &format!("{origin}/target"), None);
  let target_main = main_page(&target);
  expect(&target_main, "location.pathname", json!("/target"));
  expect(&target_main, "document.readyState", json!("complete"));
  reparent(&source, &source_main, &target, &child, &ipc_rx);
  println!("real child WebContentsView behavior regressions passed");
}

fn geometry(child: &Webview, scale: f64) {
  expect(child, "resizeSamples.length > 0", json!(true));
  let nonce = eval(child, "pageNonce");
  eval(child, "window.resizeSamples = []; true");
  WebviewDispatch::<()>::set_bounds(
    child,
    Rect {
      position: LogicalPosition::new(40.0, 50.0).into(),
      size: LogicalSize::new(280.0, 180.0).into(),
    },
  )
  .expect("set native child bounds");
  layout(child, scale, [40, 50, 280, 180]);
  expect(
    child,
    "resizeSamples.some(([width, height]) => width === 280 && height === 180)",
    json!(true),
  );
  evidence("WebviewDispatch::set_bounds", "set_bounds", "changes native child rect to 40,50/280x180 and the renderer ResizeObserver observes the new viewport");

  WebviewDispatch::<()>::set_size(child, LogicalSize::new(300.0, 200.0).into())
    .expect("resize native child");
  layout(child, scale, [40, 50, 300, 200]);
  expect(
    child,
    "resizeSamples.some(([width, height]) => width === 300 && height === 200)",
    json!(true),
  );
  evidence("WebviewDispatch::set_size", "set_size", "changes renderer viewport and ResizeObserver to 300x200 while preserving native child position");

  let physical_position = LogicalPosition::new(60.0, 70.0).to_physical::<i32>(scale);
  WebviewDispatch::<()>::set_position(child, physical_position.into())
    .expect("move native child in physical coordinates");
  layout(child, scale, [60, 70, 300, 200]);
  assert_eq!(
    eval(child, "pageNonce"),
    nonce,
    "geometry changes must retain the existing renderer document"
  );
  evidence("WebviewDispatch::set_position", "set_position", "physical child offset moves to logical 60,70 without resizing or replacing the renderer document");
}

fn reload(child: &Webview) {
  let nonce = eval(child, "pageNonce");
  let number = eval(child, "loadNumber").as_u64().unwrap();
  eval(child, "window.ephemeralBeforeReload = true; true");
  WebviewDispatch::<()>::reload(child).expect("reload child renderer");
  expect(
    child,
    &format!("document.readyState === 'complete' && window.loadNumber > {number}"),
    json!(true),
  );
  assert_ne!(
    eval(child, "pageNonce"),
    nonce,
    "reload must fetch a new HTTP document"
  );
  assert_eq!(eval(child, "typeof ephemeralBeforeReload"), "undefined");
  evidence("WebviewDispatch::reload", "reload", "HTTP load counter advances, renderer nonce changes and ephemeral JS state is gone after child reload");
}

fn focus_visibility(window: &ElectronWindowDispatcher, main: &Webview, child: &Webview) {
  WebviewDispatch::<()>::set_focus(main).expect("focus host page before transfer");
  expect(main, "document.hasFocus()", json!(true));
  expect(child, "document.hasFocus()", json!(false));
  WebviewDispatch::<()>::set_focus(child).expect("focus native child");
  expect(child, "document.hasFocus()", json!(true));
  expect(main, "document.hasFocus()", json!(false));
  evidence("WebviewDispatch::set_focus", "set_focus", "focus moves from host page to native child, independently confirmed by both real documents' hasFocus()");

  eval(child, "window.visibilityChanges = []; document.addEventListener('visibilitychange', () => visibilityChanges.push(document.visibilityState)); true");
  WebviewDispatch::<()>::hide(child).expect("hide native child");
  expect(child, "document.visibilityState", json!("hidden"));
  expect(child, "visibilityChanges.includes('hidden')", json!(true));
  assert!(
    WindowDispatch::<()>::is_visible(window).unwrap(),
    "hiding a child must not hide its parent"
  );
  expect(main, "document.visibilityState", json!("visible"));
  evidence("WebviewDispatch::hide", "hide", "native child renderer becomes hidden and emits visibilitychange while its parent window/page remain visible");

  WebviewDispatch::<()>::show(child).expect("show native child");
  expect(child, "document.visibilityState", json!("visible"));
  expect(
    child,
    "visibilityChanges.join(',')",
    json!("hidden,visible"),
  );
  evidence("WebviewDispatch::show", "show", "previously hidden child renderer becomes visible and emits the second real visibilitychange event");
}

fn auto_resize(window: &ElectronWindowDispatcher, main: &Webview, child: &Webview, scale: f64) {
  WebviewDispatch::<()>::set_bounds(
    child,
    Rect {
      position: LogicalPosition::new(20.0, 30.0).into(),
      size: LogicalSize::new(200.0, 120.0).into(),
    },
  )
  .unwrap();
  layout(child, scale, [20, 30, 200, 120]);
  let parent_before = WindowDispatch::<()>::inner_size(window).unwrap();
  let child_before = rect(child);
  eval(child, "window.resizeSamples = []; true");
  WebviewDispatch::<()>::set_auto_resize(child, true).expect("enable child proportional resize");
  WindowDispatch::<()>::set_size(window, LogicalSize::new(800.0, 600.0).into()).unwrap();
  expect(main, "[innerWidth, innerHeight]", json!([800, 600]));
  let parent_after = WindowDispatch::<()>::inner_size(window).unwrap();
  let ratio_x = f64::from(parent_after.width) / f64::from(parent_before.width);
  let ratio_y = f64::from(parent_after.height) / f64::from(parent_before.height);
  let want = [
    (f64::from(child_before[0]) * ratio_x).round() as i32,
    (f64::from(child_before[1]) * ratio_y).round() as i32,
    (f64::from(child_before[0] + child_before[2]) * ratio_x).round() as i32
      - (f64::from(child_before[0]) * ratio_x).round() as i32,
    (f64::from(child_before[1] + child_before[3]) * ratio_y).round() as i32
      - (f64::from(child_before[1]) * ratio_y).round() as i32,
  ];
  let logical_width = (f64::from(want[2]) / scale).round() as i32;
  let logical_height = (f64::from(want[3]) / scale).round() as i32;
  expect(
    child,
    "[innerWidth, innerHeight]",
    json!([logical_width, logical_height]),
  );
  let grown = rect(child);
  assert!(
    grown
      .iter()
      .zip(want)
      .all(|(got, want)| (got - want).abs() <= scale.ceil() as i32),
    "auto-resized native rect {grown:?}, want {want:?}"
  );
  assert!(
    grown[2] > child_before[2] && grown[3] > child_before[3],
    "auto resize cannot be a no-op"
  );
  expect(child, &format!("resizeSamples.some(([width, height]) => width === {logical_width} && height === {logical_height})"), json!(true));

  WebviewDispatch::<()>::set_auto_resize(child, false).expect("disable child proportional resize");
  WindowDispatch::<()>::set_size(window, LogicalSize::new(740.0, 560.0).into()).unwrap();
  expect(main, "[innerWidth, innerHeight]", json!([740, 560]));
  for _ in 0..5 {
    thread::sleep(Duration::from_millis(40));
    assert_eq!(
      rect(child),
      grown,
      "disabled auto resize must leave the child rect fixed"
    );
  }
  assert_eq!(
    eval(child, "[innerWidth, innerHeight]"),
    json!([logical_width, logical_height])
  );
  evidence("WebviewDispatch::set_auto_resize", "set_auto_resize", "enabling scales child native position/edges, renderer viewport and ResizeObserver with the parent; disabling keeps them fixed on another parent resize");
}

fn cookie_named(cookies: &[Cookie<'_>], name: &str, value: &str) -> bool {
  cookies
    .iter()
    .any(|cookie| cookie.name() == name && cookie.value() == value)
}

fn http_cookies(child: &Webview) -> String {
  eval(
    child,
    "fetch('/scope/echo', {cache: 'no-store'}).then(response => response.text())",
  )
  .as_str()
  .unwrap()
  .to_owned()
}

fn browsing_data(child: &Webview, origin: &str) {
  let scoped =
    WebviewDispatch::<()>::cookies_for_url(child, format!("{origin}/scope/echo").parse().unwrap())
      .unwrap();
  let server_cookie = scoped
    .iter()
    .find(|cookie| cookie.name() == "fromServer")
    .expect("HTTP Set-Cookie must reach child session");
  assert_eq!(server_cookie.value(), "wire");
  assert_eq!(server_cookie.path(), Some("/scope"));
  assert_eq!(server_cookie.http_only(), Some(true));
  let outside =
    WebviewDispatch::<()>::cookies_for_url(child, format!("{origin}/outside").parse().unwrap())
      .unwrap();
  assert!(
    !cookie_named(&outside, "fromServer", "wire"),
    "URL filtering must honor cookie paths"
  );
  evidence(
    "WebviewDispatch::cookies_for_url",
    "cookies_for_url",
    "reads HTTP-set HttpOnly /scope cookie for its matching URL and excludes it for /outside",
  );

  eval(child, "document.cookie = 'jsCookie=renderer; Path=/'; true");
  let all = WebviewDispatch::<()>::cookies(child).unwrap();
  assert!(cookie_named(&all, "fromServer", "wire") && cookie_named(&all, "jsCookie", "renderer"));
  assert_eq!(eval(child, "document.cookie.includes('jsCookie=renderer') && !document.cookie.includes('fromServer=wire')"), true);
  evidence("WebviewDispatch::cookies", "cookies", "session enumeration contains both real HTTP HttpOnly and renderer-written cookies while document.cookie hides the HttpOnly one");

  let cookie = Cookie::build(("apiCookie", "runtime"))
    .path("/scope")
    .http_only(true)
    .build();
  assert!(
    cookie.domain().is_none(),
    "exercise URL derived from the committed child page"
  );
  WebviewDispatch::<()>::set_cookie(child, cookie.clone())
    .expect("set session cookie through runtime");
  let wire = http_cookies(child);
  assert!(
    wire
      .split(';')
      .any(|pair| pair.trim() == "apiCookie=runtime"),
    "real HTTP request must send the runtime cookie: {wire}"
  );
  assert_eq!(
    eval(child, "document.cookie.includes('apiCookie=runtime')"),
    false
  );
  evidence("WebviewDispatch::set_cookie", "set_cookie", "domainless runtime HttpOnly cookie is sent in a real child HTTP Cookie header but is not exposed to document.cookie");

  WebviewDispatch::<()>::delete_cookie(child, cookie).expect("delete runtime cookie");
  let wire = http_cookies(child);
  assert!(
    !wire.contains("apiCookie=")
      && wire.contains("jsCookie=renderer")
      && wire.contains("fromServer=wire"),
    "delete must remove only the selected cookie from HTTP: {wire}"
  );
  assert!(!WebviewDispatch::<()>::cookies(child)
    .unwrap()
    .iter()
    .any(|cookie| cookie.name() == "apiCookie"));
  evidence("WebviewDispatch::delete_cookie", "delete_cookie", "selected runtime cookie disappears from real HTTP requests and cookie enumeration while both unrelated cookies remain");

  assert_eq!(eval(child, "(async () => { localStorage.setItem('behavior-token', 'persist'); const db = await new Promise((resolve, reject) => { const request = indexedDB.open('behavior-db', 1); request.onupgradeneeded = () => request.result.createObjectStore('proof'); request.onsuccess = () => resolve(request.result); request.onerror = () => reject(request.error); }); await new Promise((resolve, reject) => { const transaction = db.transaction('proof', 'readwrite'); transaction.objectStore('proof').put('persist', 'token'); transaction.oncomplete = resolve; transaction.onerror = () => reject(transaction.error); }); db.close(); const cache = await caches.open('behavior-cache'); await cache.put('/cached-proof', new Response('persist')); return [localStorage.getItem('behavior-token'), (await indexedDB.databases()).some(db => db.name === 'behavior-db'), await caches.has('behavior-cache')]; })()"), json!(["persist", true, true]));
  assert!(
    !http_cookies(child).is_empty(),
    "clear-data precondition requires real cookies"
  );
  WebviewDispatch::<()>::clear_all_browsing_data(child).expect("clear private child session data");
  expect(child, "localStorage.getItem('behavior-token')", Value::Null);
  assert_eq!(eval(child, "(async () => [(await indexedDB.databases()).some(db => db.name === 'behavior-db'), await caches.has('behavior-cache'), document.cookie])()"), json!([false, false, ""]));
  assert!(
    http_cookies(child).is_empty(),
    "real HTTP Cookie header must be empty after clearing the session"
  );
  assert!(WebviewDispatch::<()>::cookies(child).unwrap().is_empty());
  evidence("WebviewDispatch::clear_all_browsing_data", "clear_all_browsing_data", "clears seeded localStorage, IndexedDB and CacheStorage plus renderer/session cookies and the actual HTTP Cookie header in an isolated child session");
}

fn reparent(
  source: &Window,
  main: &Webview,
  target: &Window,
  child: &Webview,
  ipc: &mpsc::Receiver<String>,
) {
  expect(main, "location.pathname", json!("/source"));
  let nonce = eval(
    child,
    "window.migrationNonce = crypto.randomUUID(); migrationNonce",
  );
  let before = rect(child);
  let load_number = eval(child, "loadNumber");
  WebviewDispatch::<()>::reparent(child, source.id).expect("same-parent child reparent is valid");
  assert_eq!(rect(child), before);
  assert_eq!(eval(child, "migrationNonce"), nonce);
  assert!(
    WebviewDispatch::<()>::reparent(child, WindowId::from(u32::MAX)).is_err(),
    "unknown parent must be rejected without losing the child"
  );
  assert_eq!(eval(child, "migrationNonce"), nonce);
  WebviewDispatch::<()>::reparent(child, target.id)
    .expect("move existing child to another native host");
  assert_eq!(
    rect(child),
    before,
    "native reparent must preserve physical child bounds"
  );
  assert_eq!(
    eval(child, "migrationNonce"),
    nonce,
    "reparent must retain the original renderer, not reload it"
  );
  assert_eq!(eval(child, "loadNumber"), load_number);

  let source_closed = destroyed(&source.dispatcher);
  WindowDispatch::<()>::destroy(&source.dispatcher).expect("destroy old native parent");
  source_closed
    .recv_timeout(WAIT)
    .expect("old parent must deliver Destroyed");
  assert_eq!(
    eval(child, "migrationNonce"),
    nonce,
    "moved child must survive its original parent's destruction"
  );
  assert_eq!(rect(child), before);
  WebviewDispatch::<()>::eval_script(child, "window.ipc.postMessage('moved-after-source-close')")
    .unwrap();
  assert_eq!(
    ipc
      .recv_timeout(WAIT)
      .expect("moved child's Rust IPC handler must survive old-parent cleanup"),
    "moved-after-source-close"
  );

  let target_closed = destroyed(&target.dispatcher);
  WindowDispatch::<()>::destroy(&target.dispatcher).expect("destroy new native parent");
  target_closed
    .recv_timeout(WAIT)
    .expect("new parent must deliver Destroyed");
  assert!(
    WebviewDispatch::<()>::bounds(child).is_err(),
    "closing the target must destroy its adopted child"
  );
  assert!(
    WebviewDispatch::<()>::eval_script_with_callback(child, "migrationNonce", |_| {}).is_err(),
    "closed adopted renderer must no longer evaluate"
  );
  assert!(WindowDispatch::<()>::title(&source.dispatcher).is_err());
  assert!(WindowDispatch::<()>::title(&target.dispatcher).is_err());
  evidence("WebviewDispatch::reparent", "reparent", "real child keeps physical bounds, HTTP load counter, JS nonce and Rust IPC across native host migration; old-parent destruction leaves it alive, target destruction cleans it; same-parent succeeds and unknown-parent migration fails explicitly");
}

fn reparent_main(handle: &ElectronRuntimeHandle, origin: &str) {
  let (ipc_tx, ipc_rx) = mpsc::channel();
  let source = window(
    handle,
    "main-migration-source",
    &format!("{origin}/main-source"),
    Some(ipc_tx),
  );
  let target = window(
    handle,
    "main-migration-target",
    &format!("{origin}/main-target"),
    None,
  );
  let main = main_page(&source);
  expect(&main, "location.pathname", json!("/main-source"));
  expect(&main, "document.readyState", json!("complete"));
  eval(&main, "window.migrationState = {nonce: crypto.randomUUID(), count: 41, nested: {proof: 'kept'}}; true");
  let snapshot = "[pageNonce, loadNumber, migrationState]";
  let before_state = eval(&main, snapshot);
  let before_bounds = rect(&main);
  WebviewDispatch::<()>::eval_script(&main, "window.ipc.postMessage('main-before-move')").unwrap();
  assert_eq!(
    ipc_rx
      .recv_timeout(WAIT)
      .expect("original main IPC handler"),
    "main-before-move"
  );

  WebviewDispatch::<()>::reparent(&main, source.id).expect("same-parent main reparent");
  assert_eq!(rect(&main), before_bounds);
  assert_eq!(eval(&main, snapshot), before_state);
  assert!(
    WebviewDispatch::<()>::reparent(&main, WindowId::from(u32::MAX)).is_err(),
    "unknown target must reject main migration"
  );
  assert_eq!(rect(&main), before_bounds);
  assert_eq!(
    eval(&main, snapshot),
    before_state,
    "rejected migration must not damage the main document"
  );
  WebviewDispatch::<()>::reparent(&main, target.id)
    .expect("migrate the existing ordinary main webview");
  assert_eq!(rect(&main), before_bounds);
  assert_eq!(
    eval(&main, snapshot),
    before_state,
    "main migration must preserve document nonce, HTTP load number and nested JS state"
  );

  let source_closed = destroyed(&source.dispatcher);
  WindowDispatch::<()>::destroy(&source.dispatcher).expect("destroy main's original native parent");
  source_closed
    .recv_timeout(WAIT)
    .expect("main source Destroyed");
  assert_eq!(
    eval(&main, snapshot),
    before_state,
    "migrated main must survive destruction of its old parent"
  );
  assert_eq!(eval(&main, "++migrationState.count"), 42);
  assert_eq!(rect(&main), before_bounds);
  WebviewDispatch::<()>::eval_script(&main, "window.ipc.postMessage('main-after-source-close')")
    .unwrap();
  assert_eq!(
    ipc_rx
      .recv_timeout(WAIT)
      .expect("original main Rust IPC handler survives migration and old-parent cleanup"),
    "main-after-source-close"
  );
  let target_closed = destroyed(&target.dispatcher);
  WindowDispatch::<()>::destroy(&target.dispatcher).expect("destroy main's new native parent");
  target_closed
    .recv_timeout(WAIT)
    .expect("main target Destroyed");
  assert!(
    WebviewDispatch::<()>::bounds(&main).is_err(),
    "target destruction must clean up its migrated main"
  );
  assert!(
    WebviewDispatch::<()>::eval_script_with_callback(&main, snapshot, |_| {}).is_err(),
    "destroyed migrated main must not evaluate"
  );
  assert!(WindowDispatch::<()>::title(&source.dispatcher).is_err());
  assert!(WindowDispatch::<()>::title(&target.dispatcher).is_err());
  evidence("WebviewDispatch::reparent", "reparent_main", "ordinary main webview retains physical bounds, document nonce, HTTP load number and nested JS state through same/other-parent migration; unknown target fails without damage; original Rust IPC and renderer survive old-parent destruction, new-parent destruction invalidates both bounds and evaluation");
}

fn server() -> String {
  let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind local behavior fixture");
  let origin = format!("http://{}", listener.local_addr().unwrap());
  let loads = Arc::new(AtomicUsize::new(0));
  thread::spawn(move || {
    for stream in listener.incoming() {
      let loads = loads.clone();
      thread::spawn(move || serve(stream.expect("fixture connection"), &loads));
    }
  });
  origin
}

fn serve(mut stream: TcpStream, loads: &AtomicUsize) {
  stream.set_read_timeout(Some(WAIT)).unwrap();
  let mut bytes = Vec::new();
  let mut chunk = [0; 4096];
  loop {
    let n = match stream.read(&mut chunk) {
      Ok(0) | Err(_) => return,
      Ok(n) => n,
    };
    bytes.extend_from_slice(&chunk[..n]);
    if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
      break;
    }
    assert!(bytes.len() < 64 * 1024, "HTTP fixture header too large");
  }
  let request = String::from_utf8_lossy(&bytes);
  let path = request.split_whitespace().nth(1).unwrap();
  let (mime, body, cookie) = if path == "/scope/echo" {
    let cookie = request
      .lines()
      .filter_map(|line| line.split_once(':'))
      .find(|(name, _)| name.eq_ignore_ascii_case("cookie"))
      .map(|(_, value)| value.trim())
      .unwrap_or("");
    ("text/plain", cookie.to_owned(), "")
  } else {
    let number = loads.fetch_add(1, Ordering::SeqCst) + 1;
    let body = format!("<!doctype html><style>html,body{{margin:0;width:100%;height:100%;overflow:hidden}}</style><title>{path}</title><body>real child fixture<script>window.loadNumber={number};window.pageNonce=crypto.randomUUID();window.resizeSamples=[];new ResizeObserver(() => resizeSamples.push([innerWidth,innerHeight])).observe(document.documentElement);</script></body>");
    (
      "text/html",
      body,
      if path == "/scope/page" {
        "Set-Cookie: fromServer=wire; Path=/scope; HttpOnly; SameSite=Lax\r\n"
      } else {
        ""
      },
    )
  };
  let response = format!("HTTP/1.1 200 OK\r\nContent-Type: {mime}\r\nCache-Control: no-store\r\n{cookie}Content-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
  let _ = stream.write_all(response.as_bytes());
}
