//! Real desktop bridge coverage, separate from the deterministic Node/VM tests.
//!
//! Run `prepare_bridge` first, then `real_bridge`, both with
//! `--ignored --exact --test-threads=1`. Preparation owns only
//! `target/electron-bridge`; on Linux, make its `<platform>-<arch>/chrome-sandbox`
//! root-owned and mode 4755 BETWEEN those commands. No sandbox-disabling flags.
//! This exercises a named subset, not every RPC or native desktop effect.
//! Metadata gives each preparation a unique profile; only staged output is
//! removed afterwards, not Electron's profile under the user's app-data root.

use std::{
  fs,
  path::{Path, PathBuf},
  process::{Child, Command, Stdio},
  sync::{
    mpsc::{self, Receiver, RecvTimeoutError},
    Arc,
  },
  thread,
  time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use electron_rs::{
  arch, find_binary, platform, Electron, ElectronBuilder, Metadata, DEFAULT_VERSION,
};
use serde_json::{json, Value};

const WAIT: Duration = Duration::from_secs(20);
const WORKER: &str = "ELECTRON_RS_BRIDGE_TEST_WORKER";
const WINDOW: u32 = 41;
const MAIN: u32 = 71;
const CHILD: u32 = 72;
const PAGE: &str = r#"<!doctype html><html><head>
<meta charset="utf-8"><title>electron-rs bridge</title>
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; script-src 'unsafe-inline'; connect-src ipc: http://ipc.localhost">
<link rel="icon" href="data:,"></head><body><p>real bridge</p><script>
(async () => {
  try {
    window.ipc.postMessage(JSON.stringify({kind: 'loaded', origin: location.origin,
      require: typeof require, process: typeof process}));
    const endpoint = location.protocol === 'http:' ? 'http://ipc.localhost' : 'ipc://localhost';
    const response = await fetch(endpoint + '/roundtrip?case=bytes', {
      method: 'POST', headers: {'content-type': 'application/octet-stream', 'x-bridge-test': 'request'},
      body: Uint8Array.of(0, 128, 255, 10),
    });
    window.bridgeResult = {kind: 'fetch', status: response.status,
      header: response.headers.get('x-bridge-test'),
      bytes: Array.from(new Uint8Array(await response.arrayBuffer()))};
    window.ipc.postMessage(JSON.stringify(window.bridgeResult));
  } catch (error) {
    window.ipc.postMessage(JSON.stringify({kind: 'error', message: String(error)}));
  }
})();
</script></body></html>"#;

fn repository() -> &'static Path {
  Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap()
}

fn stage_root() -> PathBuf {
  repository().join("target/electron-bridge")
}

fn distribution() -> PathBuf {
  stage_root().join(format!("{}-{}", platform(), arch()))
}

// The parent also bounds APIs such as node(), which have no timeout parameter.
// Inherit output: a Chromium descendant retaining a pipe must not hang the test.
fn in_worker(name: &str, timeout: Duration, keep_stage: bool) {
  let mut cleanup = RemoveDirectory(Some(stage_root()));
  let mut command = Command::new(std::env::current_exe().unwrap());
  command
    .args([
      name,
      "--ignored",
      "--exact",
      "--test-threads=1",
      "--nocapture",
    ])
    .env(WORKER, name)
    .env_remove("TAURI_RUNTIME_ELECTRON_OWNER")
    .stdout(Stdio::inherit())
    .stderr(Stdio::inherit());
  #[cfg(unix)]
  {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
  }
  let mut worker = Worker(command.spawn().expect("launch isolated bridge test worker"));
  let deadline = Instant::now() + timeout;
  let status = loop {
    match worker.0.try_wait().expect("poll bridge test worker") {
      Some(status) => break status,
      None if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
      None => panic!("{name} exceeded {timeout:?}"),
    }
  };
  assert!(status.success(), "{name} worker failed: {status}");
  if keep_stage {
    cleanup.0 = None;
  }
}

struct Worker(Child);

impl Drop for Worker {
  fn drop(&mut self) {
    #[cfg(windows)]
    if matches!(self.0.try_wait(), Ok(None)) {
      // Kill only this worker's descendants, never every Electron on the host.
      let _ = Command::new("taskkill")
        .args(["/F", "/T", "/PID", &self.0.id().to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    }
    #[cfg(windows)]
    {
      // A panicking worker may already have exited after reaping only the main
      // process. Match our unique image exactly, never a wildcard or stock app.
      let marker = fs::read(distribution().join("electron-rs.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
      if let Some(name) = marker.as_ref().and_then(|value| value["appName"].as_str()) {
        if name
          .strip_prefix("electron-rs-bridge-")
          .is_some_and(|suffix| {
            !suffix.is_empty()
              && suffix
                .bytes()
                .all(|byte| byte.is_ascii_digit() || byte == b'-')
          })
        {
          let _ = Command::new("taskkill")
            .args(["/F", "/T", "/IM", &format!("{name}.exe")])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        }
      }
    }
    #[cfg(unix)]
    {
      // A worker owns a fresh process group, including Electron's helpers.
      let _ = Command::new("kill")
        .args(["-KILL", "--", &format!("-{}", self.0.id())])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    }
    let _ = self.0.kill();
    let _ = self.0.wait();
  }
}

struct RemoveDirectory(Option<PathBuf>);

impl Drop for RemoveDirectory {
  fn drop(&mut self) {
    let Some(path) = &self.0 else { return };
    // Only test-owned output is removable; never the cache or runtime root.
    assert!(path == &stage_root() || path == &stage_root().join("resources"));
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
      match fs::remove_dir_all(path) {
        Ok(()) => break,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
        Err(_) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
        Err(error) if thread::panicking() => {
          eprintln!("clean {}: {error}", path.display());
          break;
        }
        Err(error) => panic!("clean {}: {error}", path.display()),
      }
    }
  }
}

struct Running(Arc<Electron>);

impl Drop for Running {
  fn drop(&mut self) {
    self.0.shutdown();
  }
}

#[test]
#[ignore = "stages real Electron; run explicitly before real_bridge and Linux sandbox setup"]
fn prepare_bridge() {
  if std::env::var(WORKER).as_deref() != Ok("prepare_bridge") {
    return in_worker("prepare_bridge", Duration::from_secs(300), true);
  }
  let unique = format!(
    "{}-{}",
    std::process::id(),
    SystemTime::now()
      .duration_since(UNIX_EPOCH)
      .unwrap()
      .as_nanos()
  );
  let name = format!("electron-rs-bridge-{unique}");
  let metadata = Metadata {
    app_name: Some(name.clone()),
    // Linux supports executable renaming, not native identifier/icon branding.
    identifier: matches!(platform().as_str(), "win32" | "darwin")
      .then(|| format!("org.tauri.electron-rs-bridge-{unique}")),
    ..Metadata::default()
  };
  let runtime = repository().join("target/electron");
  let runtime_dist = runtime.join(format!("{}-{}", platform(), arch()));
  let snapshot = |root: &Path| {
    [
      "main.cjs",
      "preload.cjs",
      "electron-rs.json",
      ".electron-rs-version",
    ]
    .map(|name| fs::read(root.join(name)).ok())
  };
  let before = snapshot(&runtime_dist);
  let resources = stage_root().join("resources");
  let _resources_cleanup = RemoveDirectory(Some(resources.clone()));
  let source = ElectronBuilder::new(DEFAULT_VERSION)
    .path(&runtime)
    .output(resources.join(format!("{}-{}", platform(), arch())))
    .metadata(metadata.clone());
  source
    .build()
    .expect("stage actual DEFAULT_VERSION from the runtime root/cache (or acquire it)");
  fs::write(source.directory().join("bridge-resource-marker"), &name).unwrap();

  // The unique marker proves this resources mapping won over cache/local fallback.
  let builder = ElectronBuilder::new(DEFAULT_VERSION)
    .path(format!("{{resources}}/{}-{}", platform(), arch()))
    .resources_dir(&resources)
    .output(distribution())
    .metadata(metadata);
  builder
    .build()
    .expect("copy the real prepared distribution through {resources}");
  let binary = builder.binary().unwrap();
  assert!(binary.starts_with(distribution()));
  assert!(!distribution().starts_with(&runtime) && !runtime.starts_with(distribution()));
  assert_eq!(
    fs::read_to_string(distribution().join("bridge-resource-marker")).unwrap(),
    name
  );
  for path in [distribution(), stage_root(), binary.clone()] {
    assert_eq!(
      fs::canonicalize(find_binary(path).unwrap()).unwrap(),
      fs::canonicalize(&binary).unwrap()
    );
  }
  let relative = match platform().as_str() {
    "win32" => format!("{name}.exe"),
    "darwin" => format!("{name}.app/Contents/MacOS/{name}"),
    "linux" => name.clone(),
    other => panic!("real desktop bridge tests do not support {other}"),
  };
  assert_eq!(binary, distribution().join(relative));
  let marker = fs::read(distribution().join("electron-rs.json")).unwrap();
  let binary_modified = fs::metadata(&binary).unwrap().modified().unwrap();
  fs::write(distribution().join("main.cjs"), b"stale test asset").unwrap();
  fs::remove_file(distribution().join("preload.cjs")).unwrap();
  builder
    .build()
    .expect("repair assets without restaging/rebranding the binary");
  assert_eq!(
    fs::read(distribution().join("main.cjs")).unwrap(),
    include_bytes!("../assets/main.cjs")
  );
  assert_eq!(
    fs::read(distribution().join("preload.cjs")).unwrap(),
    include_bytes!("../assets/preload.cjs")
  );
  let main_modified = fs::metadata(distribution().join("main.cjs"))
    .unwrap()
    .modified()
    .unwrap();
  builder.build().expect("repeat preparation idempotently");
  assert_eq!(
    fs::metadata(distribution().join("main.cjs"))
      .unwrap()
      .modified()
      .unwrap(),
    main_modified
  );
  assert_eq!(
    fs::metadata(&binary).unwrap().modified().unwrap(),
    binary_modified
  );
  assert_eq!(
    fs::read(distribution().join("electron-rs.json")).unwrap(),
    marker
  );
  let output = builder.node(&["-e", "process.stdout.write(JSON.stringify({electron:process.versions.electron,platform:process.platform,arch:process.arch,answer:6*7}))"])
    .expect("run the staged real binary in Node mode");
  let result: Value = serde_json::from_slice(&output.stdout).unwrap();
  assert_eq!(
    result,
    json!({"electron":DEFAULT_VERSION,"platform":platform(),"arch":arch(),"answer":42})
  );
  assert!(builder
    .node(&[
      "-e",
      "process.stderr.write('bridge-node-error');process.exit(23)"
    ])
    .unwrap_err()
    .contains("bridge-node-error"));
  assert_eq!(
    snapshot(&runtime_dist),
    before,
    "preparation must not mutate the root runtime distribution"
  );
  println!(
    "prepared real Electron {DEFAULT_VERSION}: {}",
    binary.display()
  );
}

fn receive(receiver: &Receiver<Value>, deadline: Instant, description: &str) -> Value {
  let event = receiver
    .recv_timeout(deadline.saturating_duration_since(Instant::now()))
    .unwrap_or_else(|error| panic!("waiting for {description}: {error}"));
  assert_ne!(
    event["event"], "webview.processGone",
    "renderer crashed: {event}"
  );
  event
}

fn event_matching(
  receiver: &Receiver<Value>,
  description: &str,
  mut matches: impl FnMut(&Value) -> bool,
) -> Value {
  let deadline = Instant::now() + WAIT;
  loop {
    let event = receive(receiver, deadline, description);
    if matches(&event) {
      return event;
    }
    assert_ne!(
      event["event"], "ipc.closed",
      "Electron disconnected while waiting for {description}"
    );
  }
}

fn call(electron: &Electron, method: &str, params: Value) -> Value {
  electron
    .call(method, params, WAIT)
    .unwrap_or_else(|error| panic!("{method}: {error}"))
}

fn eval(electron: &Electron, webview: u32, script: &str) -> Value {
  call(
    electron,
    "webview.eval",
    json!({"webviewId":webview,"script":script}),
  )
}

fn geometry_matching(
  electron: &Electron,
  description: &str,
  matches: impl Fn(&Value) -> bool,
) -> Value {
  let deadline = Instant::now() + WAIT;
  loop {
    let geometry = call(electron, "window.geometry", json!({"windowId":WINDOW}));
    if matches(&geometry) {
      return geometry;
    }
    assert!(
      Instant::now() < deadline,
      "{description}: last geometry {geometry}"
    );
    thread::sleep(Duration::from_millis(20));
  }
}

#[test]
#[ignore = "requires real Electron preparation and a desktop (Xvfb/X11 plus setuid sandbox on Linux)"]
fn real_bridge() {
  if std::env::var(WORKER).as_deref() != Ok("real_bridge") {
    return in_worker("real_bridge", Duration::from_secs(180), false);
  }
  let binary = find_binary(stage_root())
    .expect("run prepare_bridge, then configure the staged Linux sandbox before real_bridge");
  let marker: Value =
    serde_json::from_slice(&fs::read(distribution().join("electron-rs.json")).unwrap()).unwrap();
  assert!(marker["appName"]
    .as_str()
    .unwrap()
    .starts_with("electron-rs-bridge-"));
  if cfg!(target_os = "linux") {
    assert!(
      marker["identifier"].is_null(),
      "Linux supports name-only branding"
    );
  } else {
    assert!(marker["identifier"]
      .as_str()
      .unwrap()
      .starts_with("org.tauri.electron-rs-bridge-"));
  }
  #[cfg(target_os = "linux")]
  {
    use std::os::unix::fs::MetadataExt;
    let sandbox = fs::metadata(distribution().join("chrome-sandbox")).unwrap();
    assert_eq!(
      sandbox.uid(),
      0,
      "chown root:root the staged chrome-sandbox after prepare_bridge"
    );
    assert_eq!(
      sandbox.gid(),
      0,
      "chown root:root the staged chrome-sandbox after prepare_bridge"
    );
    assert_eq!(
      sandbox.mode() & 0o7777,
      0o4755,
      "chmod 4755 the staged chrome-sandbox after prepare_bridge"
    );
  }
  let mut args = vec!["--tauri-schemes=bridge,ipc".to_string()];
  if cfg!(target_os = "linux") {
    // Native handle assertions below target X11, not Wayland's handle contract.
    args.push("--ozone-platform=x11".into());
  }
  let running =
    Running(Electron::with_args(&binary, 0, &args, WAIT).expect("real Electron handshake"));
  let electron = &running.0;
  assert_eq!(electron.version, DEFAULT_VERSION);
  let events = electron.events().unwrap();
  assert!(electron.events().unwrap_err().contains("already taken"));
  let subscription = electron.subscribe();
  let (callback_tx, callback) = mpsc::channel();
  electron.on(move |event| {
    let _ = callback_tx.send(event);
  });
  let output = electron.node(&["-e", "process.stdout.write(JSON.stringify({electron:process.versions.electron,answer:21*2,page:Buffer.from(process.argv[1]).toString('base64')}))", PAGE]).unwrap();
  let node: Value = serde_json::from_slice(&output.stdout).unwrap();
  assert_eq!(node["electron"], DEFAULT_VERSION);
  assert_eq!(node["answer"], 42);
  assert!(electron
    .node(&[
      "-e",
      "process.stderr.write('connected-node-error');process.exit(23)"
    ])
    .unwrap_err()
    .contains("connected-node-error"));
  assert_eq!(
    electron
      .call("bridge.unknown", Value::Null, WAIT)
      .unwrap_err(),
    "unknown method bridge.unknown"
  );
  for scheme in ["bridge", "ipc"] {
    assert_eq!(
      call(electron, "protocol.register", json!({"scheme":scheme})),
      Value::Null
    );
  }
  // Windows exercises the WebView2-style localhost mapping; macOS/Linux use
  // the privileged native custom scheme. Both must retain a non-opaque origin.
  let origin = if cfg!(windows) {
    "http://bridge.localhost"
  } else {
    "bridge://localhost"
  };
  let url = format!("{origin}/index.html");
  call(
    electron,
    "window.create",
    json!({
      "windowId":WINDOW,"title":"bridge initial","width":320,"height":240,"visible":false,
      "webview":{"webviewId":MAIN,"url":url}
    }),
  );
  let deadline = Instant::now() + WAIT;
  let (mut page_requests, mut ipc_requests) = (0, 0);
  let (mut loaded, mut posted, mut fetched) = (false, false, None);
  while !(loaded && posted && fetched.is_some()) {
    let event = receive(
      &events,
      deadline,
      "real page load, preload postMessage and binary IPC fetch",
    );
    let params = &event["params"];
    match event["event"].as_str() {
      Some("protocol.request") => match params["scheme"].as_str() {
        Some("bridge") => {
          page_requests += 1;
          assert_eq!(params["uri"], "bridge://localhost/index.html");
          assert_eq!(params["method"], "GET");
          call(
            electron,
            "protocol.respond",
            json!({"id":params["id"],"status":200,
            "headers":{"content-type":"text/html; charset=utf-8"},"body":node["page"]}),
          );
        }
        Some("ipc") => {
          ipc_requests += 1;
          assert_eq!(params["webviewId"], MAIN);
          assert_eq!(params["uri"], "ipc://localhost/roundtrip?case=bytes");
          assert_eq!(params["method"], "POST");
          assert_eq!(params["headers"]["origin"], origin);
          assert_eq!(params["headers"]["x-bridge-test"], "request");
          assert_eq!(params["body"], "AID/Cg==", "request bytes [0,128,255,10]");
          call(
            electron,
            "protocol.respond",
            json!({"id":params["id"],"status":201,
            "headers":{"content-type":"application/octet-stream","x-bridge-test":"response"},
            "body":"/wCAQQ=="}),
          ); // Response bytes [255,0,128,65].
        }
        _ => panic!("unexpected protocol request: {event}"),
      },
      Some("webview.loadFinished") if params["webviewId"] == MAIN => {
        assert_eq!(params["url"], url);
        loaded = true;
      }
      Some("ipc.message") => {
        assert_eq!(params["webviewId"], MAIN);
        assert_eq!(params["url"], url);
        let body: Value = serde_json::from_str(params["body"].as_str().unwrap()).unwrap();
        match body["kind"].as_str() {
          Some("loaded") => {
            assert_eq!(
              body,
              json!({"kind":"loaded","origin":origin,"require":"undefined","process":"undefined"})
            );
            posted = true;
          }
          Some("fetch") => {
            assert_eq!(
              body,
              json!({"kind":"fetch","status":201,"header":"response","bytes":[255,0,128,65]})
            );
            fetched = Some(event);
          }
          _ => panic!("page reported failure: {body}"),
        }
      }
      Some("ipc.closed") => panic!("Electron closed during page roundtrip"),
      _ => {}
    }
  }
  assert_eq!((page_requests, ipc_requests), (1, 1));
  let fetched = fetched.unwrap();
  for receiver in [&subscription, &callback] {
    assert_eq!(
      event_matching(receiver, "subscription/callback fetch event", |event| event
        == &fetched),
      fetched
    );
  }
  assert_eq!(
    eval(
      electron,
      MAIN,
      "({title:document.title,url:location.href,bytes:window.bridgeResult.bytes})"
    ),
    json!({"title":"electron-rs bridge","url":url,"bytes":[255,0,128,65]})
  );
  native_window_and_child(electron, &events);
  electron.shutdown();
  for receiver in [&events, &subscription, &callback] {
    event_matching(receiver, "shutdown closure notification", |event| {
      event["event"] == "ipc.closed"
    });
    assert!(matches!(
      receiver.recv_timeout(WAIT),
      Err(RecvTimeoutError::Disconnected)
    ));
  }
  assert!(matches!(
    electron.subscribe().recv_timeout(WAIT),
    Err(RecvTimeoutError::Disconnected)
  ));
  assert!(electron
    .call("window.geometry", json!({"windowId":WINDOW}), WAIT)
    .unwrap_err()
    .contains("is gone"));
  electron.shutdown(); // Idempotent, including the guard's eventual shutdown.
  println!(
    "real Electron {DEFAULT_VERSION} bridge passed on {}-{}",
    platform(),
    arch()
  );
}

fn native_window_and_child(electron: &Electron, events: &Receiver<Value>) {
  call(
    electron,
    "window.setTitle",
    json!({"windowId":WINDOW,"title":"bridge native title"}),
  );
  geometry_matching(electron, "native title setter/getter", |geometry| {
    geometry["title"] == "bridge native title"
  });
  call(
    electron,
    "window.size",
    json!({"windowId":WINDOW,"width":420,"height":280,"physical":false}),
  );
  geometry_matching(
    electron,
    "native content size in physical pixels",
    |geometry| {
      let scale = geometry["scaleFactor"].as_f64().unwrap();
      geometry["contentBounds"]["width"] == (420.0 * scale).round() as u64
        && geometry["contentBounds"]["height"] == (280.0 * scale).round() as u64
    },
  );
  let handle = call(electron, "window.nativeHandle", json!({"windowId":WINDOW}));
  assert!(
    handle["handle"].as_u64().is_some_and(|value| value > 0),
    "expected HWND / NSView / X11 id: {handle}"
  );
  call(electron, "window.show", json!({"windowId":WINDOW}));
  geometry_matching(electron, "native window visibility", |geometry| {
    geometry["visible"] == true
  });
  call(
    electron,
    "webview.create",
    json!({"windowId":WINDOW,"webviewId":CHILD,
    "url":"data:text/html,<title>native-child</title><p>native child</p>",
    "bounds":{"position":{"x":11,"y":13},"size":{"width":180,"height":120}}}),
  );
  event_matching(events, "native child load", |event| {
    event["event"] == "webview.loadFinished" && event["params"]["webviewId"] == CHILD
  });
  assert_eq!(eval(electron, CHILD, "document.title"), "native-child");
  assert_eq!(
    eval(electron, MAIN, "document.title"),
    "electron-rs bridge",
    "child RPC must not target its parent's document"
  );
  call(
    electron,
    "webview.setBounds",
    json!({"webviewId":CHILD,
    "position":{"x":17,"y":19,"physical":false},"size":{"width":160,"height":100,"physical":false}}),
  );
  let bounds = call(electron, "webview.bounds", json!({"webviewId":CHILD}));
  let scale = bounds["scaleFactor"].as_f64().unwrap();
  assert_eq!(
    bounds["bounds"],
    json!({"x":(17.0*scale).round() as u64,"y":(19.0*scale).round() as u64,
    "width":(160.0*scale).round() as u64,"height":(100.0*scale).round() as u64})
  );
  for visible in [false, true] {
    call(
      electron,
      "webview.visibility",
      json!({"webviewId":CHILD,"visible":visible}),
    );
    assert_eq!(
      call(electron, "webview.bounds", json!({"webviewId":CHILD}))["visible"],
      visible
    );
  }
  call(electron, "webview.close", json!({"webviewId":CHILD}));
  assert!(electron
    .call(
      "webview.eval",
      json!({"webviewId":CHILD,"script":"1"}),
      WAIT
    )
    .unwrap_err()
    .contains("unknown webview"));
  call(electron, "window.hide", json!({"windowId":WINDOW}));
  geometry_matching(electron, "native hide", |geometry| {
    geometry["visible"] == false
  });
  call(electron, "window.close", json!({"windowId":WINDOW}));
  event_matching(events, "native close request", |event| {
    event["event"] == "window.closeRequested" && event["params"]["windowId"] == WINDOW
  });
  call(electron, "window.geometry", json!({"windowId":WINDOW})); // Close is host-vetoed until destroy.
  call(electron, "window.destroy", json!({"windowId":WINDOW}));
  event_matching(events, "native destruction", |event| {
    event["event"] == "window.closed" && event["params"]["windowId"] == WINDOW
  });
  assert!(electron
    .call("window.geometry", json!({"windowId":WINDOW}), WAIT)
    .unwrap_err()
    .contains("unknown window"));
}
