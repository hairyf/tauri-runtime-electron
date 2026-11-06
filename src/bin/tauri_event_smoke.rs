use serde_json::{json, Value};
use std::{
  io::{Read, Write},
  net::TcpListener,
  process::ExitCode,
  sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc,
  },
  thread,
  time::{Duration, Instant},
};
use tauri::{Emitter, WebviewUrl, WebviewWindowBuilder};
use tauri_runtime_electron::ElectronRuntime;

type Window = tauri::WebviewWindow<ElectronRuntime>;
const WAIT: Duration = Duration::from_secs(8);
const EVENT: &str = "callback-warning-probe";

fn eval(window: &Window, script: &str) -> Value {
  let (tx, rx) = mpsc::channel();
  window
    .eval_with_callback(script, move |value| {
      let _ = tx.send(value);
    })
    .unwrap();
  serde_json::from_str(&rx.recv_timeout(WAIT).expect("renderer evaluation timeout")).unwrap()
}

fn expect(window: &Window, script: &str, want: Value) {
  let start = Instant::now();
  loop {
    let got = eval(window, script);
    if got == want {
      return;
    }
    assert!(start.elapsed() < WAIT, "{script}: got {got}, want {want}; progress {}", eval(window, "window.cycleProgress ?? null"));
    thread::sleep(Duration::from_millis(10));
  }
}

fn exercise(window: &Window) {
  expect(
    window,
    "typeof window.__TAURI_INTERNALS__?.invoke",
    json!("function"),
  );
  eval(
    window,
    concat!(
      include_str!("../../source/tauri/crates/tauri/scripts/bundle.global.js"),
      "; window.__TAURI__ = __TAURI_IIFE__; true"
    ),
  );
  assert_eq!(eval(window, "typeof __TAURI__.event.listen"), "function");
  println!("Actual event unlisten helper: {}", eval(window, "__TAURI_EVENT_PLUGIN_INTERNALS__.unregisterListener.toString()"));
  eval(
    window,
    r#"window.warnings = []; const originalWarn = console.warn; console.warn = (...args) => { warnings.push(args.map(String).join(' ')); originalWarn(...args); }; window.payloads = []; window.ready = false; __TAURI__.event.listen('callback-warning-probe', event => payloads.push(event.payload)).then(unlisten => { window.unlisten = unlisten; ready = true; }); true"#,
  );
  expect(window, "ready", json!(true));
  eval(window, "window.emitted = false; __TAURI__.event.emitTo('event-probe', 'callback-warning-probe', {source:'emitTo',value:42}).then(() => emitted = true); true");
  expect(window, "emitted", json!(true));
  expect(
    window,
    "payloads",
    json!([{ "source": "emitTo", "value": 42 }]),
  );
  eval(window, "window.survivorPayloads = []; window.pairReady = false; (async () => { window.survivorOff = await __TAURI__.event.listen('callback-warning-probe', event => survivorPayloads.push(event.payload)); await unlisten(); await __TAURI__.event.emitTo('event-probe', 'callback-warning-probe', {source:'after-off',value:84}); pairReady = true; })(); true");
  expect(window, "pairReady", json!(true));
  assert_eq!(
    eval(window, "payloads"),
    json!([{"source":"emitTo","value":42}])
  );
  assert_eq!(
    eval(window, "survivorPayloads"),
    json!([{"source":"after-off","value":84}])
  );
  eval(window, "window.survivorStopped = false; survivorOff().then(() => survivorStopped = true); window.cyclePayloads = []; true");
  expect(window, "survivorStopped", json!(true));
  let stop = Arc::new(AtomicBool::new(false));
  let emitting = Arc::clone(&stop);
  let emitter = window.clone();
  let worker = thread::spawn(move || {
    let mut sequence = 0_u32;
    while !emitting.load(Ordering::Relaxed) {
      emitter
        .emit(EVENT, json!({"source":"rust", "sequence":sequence}))
        .unwrap();
      sequence += 1;
      thread::sleep(Duration::from_millis(1));
    }
    sequence
  });
  eval(
    window,
    r#"window.cyclesDone = false; window.cycleError = null; (async () => { for (let cycle = 0; cycle < 100; cycle++) { window.cycleProgress = 'listen:' + cycle; const off = await __TAURI__.event.listen('callback-warning-probe', event => cyclePayloads.push(event.payload)); await __TAURI__.event.emitTo('event-probe', 'callback-warning-probe', {source:'cycle',cycle}); window.cycleProgress = 'off:' + cycle; await off(); } cyclesDone = true; })().catch(error => cycleError = String(error)); true"#,
  );
  expect(window, "cyclesDone || cycleError", json!(true));
  stop.store(true, Ordering::Relaxed);
  let emitted = worker.join().unwrap();
  window.emit("event-probe-drained", ()).unwrap();
  assert_eq!(
    eval(window, "payloads"),
    json!([{"source":"emitTo","value":42}])
  );
  assert_eq!(eval(window, "cyclePayloads.some(payload => payload.source === 'rust')"), true);
  let warnings = eval(window, "warnings");
  println!(
    "Rust emits: {emitted}; warning count: {}; first warning: {}",
    warnings.as_array().unwrap().len(),
    warnings.as_array().unwrap().first().unwrap_or(&Value::Null)
  );
  assert!(emitted > 100, "Rust emit must overlap remounts");
  assert!(
    warnings.as_array().unwrap().is_empty(),
    "unlisten/remount must not produce any console warning"
  );
}

fn main() -> ExitCode {
  thread::spawn(|| {
    thread::sleep(Duration::from_secs(45));
    eprintln!("event smoke timeout");
    std::process::exit(1);
  });
  let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
  let url = format!("http://{}", listener.local_addr().unwrap());
  thread::spawn(move || {
    for mut stream in listener.incoming().flatten() {
      let mut request = [0; 4096];
      let _ = stream.read(&mut request);
      let body = "<!doctype html><title>event-probe</title>";
      let response = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: text/html\r\nConnection: close\r\n\r\n{body}",body.len());
      let _ = stream.write_all(response.as_bytes());
    }
  });
  let (tx, rx) = mpsc::channel();
  let mut context =
    tauri::generate_context!("tests/assets/tauri-event/tauri.conf.json", test = true);
  for command in ["listen", "unlisten", "emit_to"] {
    context.runtime_authority_mut().__allow_command(
      format!("plugin:event|{command}"),
      tauri_utils::acl::ExecutionContext::Remote {
        url: url.parse().unwrap(),
      },
    );
  }
  let app = tauri::Builder::<ElectronRuntime>::new()
    .setup(move |app| {
      let window =
        WebviewWindowBuilder::new(app, "event-probe", WebviewUrl::External(url.parse()?))
          .build()?;
      let app = app.handle().clone();
      tx.send(thread::spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| exercise(&window)));
        app.exit(i32::from(result.is_err()));
        result
      }))
      .unwrap();
      Ok(())
    })
    .build(context)
    .unwrap();
  let status = app.run_return(|_, _| {});
  if rx.recv_timeout(WAIT).unwrap().join().unwrap().is_ok() && status == 0 {
    ExitCode::SUCCESS
  } else {
    ExitCode::FAILURE
  }
}
