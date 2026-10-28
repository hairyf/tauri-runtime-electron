//! Real Chromium popups through ordinary, unpatched Tauri builders and facades.
use serde_json::{json, Value};
use std::{
  io::{Read, Write},
  net::{TcpListener, TcpStream},
  process::ExitCode,
  sync::{mpsc, Arc, Mutex},
  thread,
  time::{Duration, Instant},
};
use tauri::{Emitter, Manager, WebviewUrl, WebviewWindowBuilder};
use tauri_runtime_electron::{
  new_window::{on_new_window, with_opener, NewWindowResponse},
  ElectronRuntime,
};

type App = tauri::AppHandle<ElectronRuntime>;
type Window = tauri::WebviewWindow<ElectronRuntime>;
const WAIT: Duration = Duration::from_secs(8);

fn main() -> ExitCode {
  thread::spawn(|| {
    thread::sleep(Duration::from_secs(100));
    eprintln!("Tauri popup regression exceeded 100s");
    std::process::exit(1);
  });
  let origin = server();
  let (tx, rx) = mpsc::sync_channel(1);
  let app = tauri::Builder::<ElectronRuntime>::new()
    .setup(move |app| {
      let window = WebviewWindowBuilder::new(
        app,
        "facade-opener",
        WebviewUrl::External(format!("{origin}/opener").parse()?),
      )
      .inner_size(420.0, 300.0)
      .build()?;
      let ui_thread = thread::current().id();
      let app = app.handle().clone();
      tx.send(thread::spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
          exercise(&app, &window, &origin, ui_thread)
        }));
        app.exit(i32::from(result.is_err()));
        result
      }))?;
      Ok(())
    })
    .build(tauri::generate_context!(
      "tests/assets/tauri-new-window/tauri.conf.json",
      test = true
    ))
    .expect("build ordinary Tauri app");
  let status = app.run_return(|_, event| {
    // Keep the loop alive until the worker has checked last-window cleanup.
    if let tauri::RunEvent::ExitRequested {
      code: None, api, ..
    } = event
    {
      api.prevent_exit();
    }
  });
  if rx
    .recv_timeout(WAIT)
    .expect("start popup worker")
    .join()
    .expect("join popup worker")
    .is_ok()
    && status == 0
  {
    ExitCode::SUCCESS
  } else {
    ExitCode::FAILURE
  }
}

fn eval(window: &Window, script: &str) -> Value {
  let (tx, rx) = mpsc::channel();
  window
    .eval_with_callback(script, move |value| {
      let _ = tx.send(value);
    })
    .expect("evaluate Tauri webview");
  serde_json::from_str(
    &rx
      .recv_timeout(WAIT)
      .expect("renderer evaluation timed out"),
  )
  .expect("JSON evaluation result")
}

fn expect(window: &Window, script: &str, want: Value) {
  let start = Instant::now();
  loop {
    let got = eval(window, script);
    if got == want {
      return;
    }
    assert!(start.elapsed() < WAIT, "{script}: got {got}, want {want}");
    thread::sleep(Duration::from_millis(40));
  }
}

fn wait_until(description: &str, mut check: impl FnMut() -> bool) {
  let start = Instant::now();
  while !check() {
    assert!(start.elapsed() < WAIT, "{description}");
    thread::sleep(Duration::from_millis(40));
  }
}

fn security(window: &Window) {
  assert_eq!(
    eval(
      window,
      "[typeof require, typeof process, typeof Buffer, typeof __TAURI_INTERNALS__.invoke]"
    ),
    json!(["undefined", "undefined", "undefined", "function"])
  );
  // The fixture grants no capabilities, including no remote-origin access.
  let title = window.title().expect("title before forbidden IPC");
  eval(window, "window.aclResult = null; __TAURI_INTERNALS__.invoke('plugin:window|set_title', {label: '', title: 'acl-must-deny'}).then(() => aclResult = {status: 'allowed'}, error => aclResult = {status: 'denied', message: String(error)}); true");
  expect(window, "aclResult?.status ?? null", json!("denied"));
  assert_eq!(
    eval(window, "/not allowed|denied/i.test(aclResult.message)"),
    true
  );
  assert_eq!(window.title().expect("title after forbidden IPC"), title);
}

fn exercise(app: &App, main: &Window, origin: &str, ui_thread: thread::ThreadId) {
  assert_ne!(
    thread::current().id(),
    ui_thread,
    "registration must start on an ordinary worker"
  );
  expect(main, "document.title", json!("/opener"));
  security(main);
  tokio::runtime::Builder::new_current_thread()
    .enable_time()
    .build()
    .expect("async consumer bridge runtime")
    .block_on(async {
      tokio::time::timeout(WAIT, async {
        let webview: &tauri::Webview<ElectronRuntime> = main.as_ref();
        let bridge = tauri_runtime_electron::main_bridge(webview)
          .await.expect("resolve existing Electron client through the Tauri UI route");
        let electron: std::process::Output = bridge.run(
          "const {isMainThread,threadId}=require('node:worker_threads');console.log(JSON.stringify({pid:process.pid,ready:require('electron').app.isReady(),isMainThread,threadId}));return 1n;",
        ).timeout(WAIT).await.expect("execute on actual Electron main");
        let node_bridge = Arc::clone(&bridge);
        let node: std::process::Output = tokio::task::spawn_blocking(move || node_bridge.node(&[
          "-e", "const {isMainThread,threadId}=require('node:worker_threads');console.log(JSON.stringify({pid:process.pid,isMainThread,threadId,args:process.argv.slice(1)}));",
          "--", "hello",
        ])).await.expect("join native Node process").expect("execute in a separate Node process");
        assert!(electron.status.success(), "{electron:?}");
        assert!(node.status.success(), "{node:?}");
        assert!(electron.stderr.is_empty() && node.stderr.is_empty());
        let electron: Value = serde_json::from_slice(&electron.stdout).unwrap();
        let node: Value = serde_json::from_slice(&node.stdout).unwrap();
        assert!(electron["pid"].as_u64().unwrap() > 0);
        assert_eq!(electron["ready"], true);
        assert_eq!(electron["isMainThread"], true);
        assert_eq!(electron["threadId"], 0);
        assert!(node["pid"].as_u64().unwrap() > 0);
        assert_ne!(node["pid"], electron["pid"]);
        assert_eq!(node["isMainThread"], true);
        assert_eq!(node["threadId"], 0);
        assert_eq!(node["args"], json!(["hello"]));
        let node_bridge = Arc::clone(&bridge);
        let bytes = tokio::task::spawn_blocking(move || node_bridge.node(&[
          "-e", "process.stdout.write(Buffer.from([0,255,195]));process.stderr.write(Buffer.from([128,10]));",
        ])).await.expect("join Node binary output").expect("collect Node binary output");
        assert!(bytes.status.success(), "{bytes:?}");
        assert_eq!(bytes.stdout, [0, 255, 195]);
        assert_eq!(bytes.stderr, [128, 10]);
        use std::{future::IntoFuture as _, sync::atomic::{AtomicBool, Ordering}};
        {
          let script = "process.stdout.write(Buffer.from([0,255,195]));await new Promise(resolve=>setTimeout(resolve,1000));process.stderr.write(Buffer.from([128,10]));return {done:true};";
          let command = bridge.run(script);
          let first = Arc::new(AtomicBool::new(false));
          let observed = Arc::clone(&first);
          let (chunks, received) = mpsc::channel();
          let mut pending = command.timeout(WAIT).max_output_bytes(64).on_output(move |chunk| {
            chunks.send(chunk).unwrap();
            observed.store(true, Ordering::SeqCst);
          }).into_future();
          tokio::select! {
            biased;
            result = &mut pending => panic!("command completed before live chunk: {result:?}"),
            _ = async {
              while !first.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(10)).await;
              }
            } => {}
          }
          let output = pending.await.expect("collect real streamed command");
          assert!(output.status.success(), "{output:?}");
          assert_eq!(output.stdout, [0, 255, 195]);
          assert_eq!(output.stderr, [128, 10]);
          let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
          for (sequence, chunk) in received.try_iter().enumerate() {
            assert_eq!(chunk.sequence, sequence as u64);
            if chunk.stream == electron_rs::OutputStream::Stdout { stdout.extend(chunk.data); }
            else { stderr.extend(chunk.data); }
          }
          assert_eq!(stdout, output.stdout);
          assert_eq!(stderr, output.stderr);
        }
      }).await.expect("Tauri consumer bridge check timed out");
    });
  let calls: Arc<Mutex<Vec<String>>> = Default::default();
  let observed = calls.clone();
  let child_calls: Arc<Mutex<Vec<String>>> = Default::default();
  let children = child_calls.clone();
  let handle = app.clone();
  let invalid = main.clone();
  let (tx, rx) = mpsc::sync_channel(1);
  // This call must route from the worker to the correct runtime/UI webview.
  on_new_window(main.as_ref(), move |url, features| {
    assert_eq!(
      thread::current().id(),
      ui_thread,
      "popup decisions belong to the UI thread"
    );
    observed.lock().unwrap().push(url.path().to_owned());
    match url.path() {
      "/deny" => NewWindowResponse::Deny,
      "/ui-allow" => NewWindowResponse::Allow,
      "/posted-deny" => {
        // Ordinary emit/eval calls return immediately while Chromium waits for
        // this decision. Both must execute later, not disappear as RPC errors.
        invalid.eval("window.postedFromPolicy = (window.postedFromPolicy ?? 0) + 1")
          .expect("post eval inside native policy");
        handle.emit("policy-posted", "delivered").expect("emit inside native policy");
        NewWindowResponse::Deny
      }
      "/allow" => NewWindowResponse::Allow,
      "/invalid" => NewWindowResponse::create(&invalid).expect("resolve unrelated Tauri window"),
      "/abandon-deny" | "/abandon-allow" => {
        with_opener(features.opener(), || {
          WebviewWindowBuilder::new(
            &handle,
            "facade-unused",
            WebviewUrl::External("about:blank".parse().unwrap()),
          )
          .build()
        })
        .expect("build unused ordinary Tauri destination");
        if url.path() == "/abandon-deny" {
          NewWindowResponse::Deny
        } else {
          NewWindowResponse::Allow
        }
      }
      "/create" => {
        assert_eq!(features.frame_name, "facade-native");
        let popup = with_opener(features.opener(), || {
          WebviewWindowBuilder::new(
            &handle,
            "facade-created",
            WebviewUrl::External("about:blank#must-not-navigate".parse().unwrap()),
          )
          .inner_size(360.0, 240.0)
          .initialization_script("window.facadeInit = 'before-page'")
          .build()
        })
        .expect("build ordinary Tauri popup destination");
        let child_observed = children.clone();
        // Reenter both facades in the synchronous native UI callback: queued
        // tasks here would deadlock, and a stale TLS route would hit the opener.
        on_new_window(popup.as_ref(), move |url, _| {
          assert_eq!(thread::current().id(), ui_thread);
          child_observed.lock().unwrap().push(url.path().to_owned());
          NewWindowResponse::Deny
        })
        .expect("register facade inside UI callback without deadlock");
        let response = NewWindowResponse::create(&popup)
          .expect("select Tauri popup inside UI callback without deadlock");
        tx.send(popup).expect("send managed Tauri popup");
        response
      }
      _ => panic!("unexpected opener popup: {url}"),
    }
  })
  .expect("register facade from ordinary worker");

  eval(main, "window.postedFromPolicy = 0; true");
  assert_eq!(eval(main, &format!("window.open('{origin}/posted-deny') === null")), true);
  expect(main, "window.postedFromPolicy", json!(1));

  // The API must not wait on Chromium while running on its own event thread.
  let ui_window = main.clone();
  let ui_url = format!("{origin}/ui-allow");
  let (ui_tx, ui_rx) = mpsc::channel();
  main.run_on_main_thread(move || {
    let (completed, completion) = mpsc::channel();
    ui_window.eval_with_callback(
      format!("window.uiPopup = window.open('{ui_url}'); !!uiPopup"),
      move |value| {
        assert_eq!(thread::current().id(), ui_thread, "evaluation callback must return on UI");
        let _ = completed.send(value);
      },
    ).expect("queue callback evaluation from UI thread");
    ui_tx.send(completion).unwrap();
  }).expect("schedule real UI-thread popup evaluation");
  let completion = ui_rx.recv_timeout(WAIT).expect("UI eval must return before renderer completes");
  assert_eq!(completion.recv_timeout(WAIT).expect("UI popup evaluation completes"), "true");
  expect(main, "uiPopup.document.title", json!("/ui-allow"));
  eval(main, "uiPopup.close(); true");
  expect(main, "uiPopup.closed", json!(true));

  assert_eq!(
    eval(main, &format!("window.open('{origin}/deny') === null")),
    true
  );
  assert_eq!(
    eval(main, &format!("window.open('{origin}/invalid') === null")),
    true
  );
  assert_eq!(
    eval(
      main,
      &format!("window.allowed = window.open('{origin}/allow'); !!allowed")
    ),
    true
  );
  expect(main, "allowed.document.title", json!("/allow"));
  assert_eq!(
    eval(
      main,
      "[allowed.opener === window, typeof allowed.require, typeof allowed.process]"
    ),
    json!([true, "undefined", "undefined"])
  );
  eval(main, "allowed.close(); true");
  expect(main, "allowed.closed", json!(true));

  assert_eq!(
    eval(
      main,
      &format!("window.open('{origin}/abandon-deny') === null")
    ),
    true
  );
  reuse_abandoned_label(app);
  assert_eq!(
    eval(
      main,
      &format!("window.abandonedAllow = window.open('{origin}/abandon-allow'); !!abandonedAllow")
    ),
    true
  );
  expect(
    main,
    "abandonedAllow.document.title",
    json!("/abandon-allow"),
  );
  assert_eq!(eval(main, "abandonedAllow.opener === window"), true);
  reuse_abandoned_label(app);
  eval(main, "abandonedAllow.close(); true");
  expect(main, "abandonedAllow.closed", json!(true));

  assert_eq!(
    eval(
      main,
      &format!("window.created = window.open('{origin}/create', 'facade-native'); !!created")
    ),
    true
  );
  let popup = rx
    .recv_timeout(WAIT)
    .expect("Tauri Create callback reached");
  expect(&popup, "document.title", json!("/create"));
  expect(&popup, "document.readyState", json!("complete"));
  assert_eq!(
    popup.url().expect("query native Tauri popup URL").as_str(),
    format!("{origin}/create"),
    "Tauri must report Chromium's popup URL, not the builder placeholder"
  );
  assert_eq!(eval(main, "[created.opener === window, created.location.pathname, created.facadeInit, created.pageSawInit, created.pageSawOpener]"),
    json!([true, "/create", "before-page", "before-page", true]));
  assert_eq!(
    eval(&popup, "[opener.document.title, document.referrer]"),
    json!(["/opener", format!("{origin}/opener")])
  );
  assert_eq!(app.get_webview_window("facade-created").unwrap(), popup);
  assert!(matches!(
    NewWindowResponse::create(&popup).expect("resolve Tauri popup from worker"),
    NewWindowResponse::Create { .. }
  ));
  security(&popup);
  eval(main, "window.received = null; onmessage = event => received = [event.data, event.source === created]; true");
  eval(
    &popup,
    "opener.postMessage('facade-native-message', location.origin); true",
  );
  expect(main, "received", json!(["facade-native-message", true]));
  assert_eq!(
    eval(
      &popup,
      &format!("window.open('{origin}/child-deny') === null")
    ),
    true
  );
  assert_eq!(*child_calls.lock().unwrap(), ["/child-deny"]);
  assert_eq!(
    *calls.lock().unwrap(),
    [
      "/posted-deny",
      "/ui-allow",
      "/deny",
      "/invalid",
      "/allow",
      "/abandon-deny",
      "/abandon-allow",
      "/create"
    ]
  );

  popup
    .set_title("facade-managed")
    .expect("manage ordinary Tauri destination");
  assert_eq!(popup.title().unwrap(), "facade-managed");
  popup.close().expect("close ordinary Tauri destination");
  expect(main, "created.closed", json!(true));
  wait_until("Tauri must remove the closed destination", || {
    app.get_webview_window("facade-created").is_none()
  });
  assert!(
    popup.title().is_err(),
    "closed native destination must leave the runtime registry"
  );
  main.close().expect("close Tauri opener");
  wait_until("Tauri must remove the closed opener", || {
    app.get_webview_window("facade-opener").is_none()
  });
  assert!(
    app.webview_windows().is_empty(),
    "no managed Tauri windows may leak"
  );

  evidence("tauri_runtime_electron::on_new_window", "ordinary worker registration reaches native popup callback; UI-thread reentrant registration, Deny/Allow, native WindowProxy/opener, child policy isolation and close cleanup");
  evidence("NewWindowResponse::create", "ordinary WebviewWindowBuilder destination selected in UI callback and resolved from worker; original destination URL and before-page initializer preserved, native opener/message routing, unrelated target denied, remote IPC ACL denied and no Node globals");
  println!("ordinary Tauri facade popup regressions passed");
}

fn reuse_abandoned_label(app: &App) {
  wait_until("Tauri must forget an unused popup destination", || {
    app.get_webview_window("facade-unused").is_none()
  });
  let replacement = WebviewWindowBuilder::new(
    app,
    "facade-unused",
    WebviewUrl::External("about:blank".parse().unwrap()),
  )
  .build()
  .expect("abandoned label must be reusable");
  replacement.close().expect("close replacement Tauri window");
  wait_until("Tauri must remove the replacement window", || {
    app.get_webview_window("facade-unused").is_none()
  });
}

fn evidence(member: &str, assertion: &str) {
  println!(
    "[runtime-evidence] {}",
    json!({
      "member": member, "test": "tauri_new_window_smoke::exercise", "assertion": assertion,
      "kind": "behavior", "status": "pass",
    })
  );
}

fn server() -> String {
  let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind local fixture");
  let origin = format!("http://{}", listener.local_addr().unwrap());
  thread::spawn(move || {
    for stream in listener.incoming() {
      thread::spawn(move || serve(stream.expect("fixture connection")));
    }
  });
  origin
}

fn serve(mut stream: TcpStream) {
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
  let (mime, body) = if path == "/page.js" {
    (
      "text/javascript",
      "window.pageSawInit = window.facadeInit ?? null; window.pageSawOpener = opener !== null;"
        .to_owned(),
    )
  } else {
    ("text/html", format!("<!doctype html><title>{path}</title><script src='/page.js'></script><body>Tauri facade native popup fixture</body>"))
  };
  let response = format!("HTTP/1.1 200 OK\r\nContent-Type: {mime}\r\nContent-Security-Policy: default-src 'self'; connect-src 'self' ipc: http://ipc.localhost\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
  let _ = stream.write_all(response.as_bytes());
}
