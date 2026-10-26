//! Real renderer/background API smoke. Native pixels stay unknown without an independent observer.
use std::{
  process::ExitCode,
  sync::mpsc,
  thread,
  time::{Duration, Instant},
};
use tauri_runtime::{
  webview::{PendingWebview, WebviewAttributes},
  window::{DetachedWindow, PendingWindow, RawWindow, WindowBuilder},
  ExitRequestedEventAction, RunEvent, Runtime, RuntimeHandle, RuntimeInitArgs, WebviewDispatch,
  WindowDispatch,
};
use tauri_runtime_electron::{
  ElectronRuntime, ElectronRuntimeHandle, ElectronWebviewDispatcher as Webview,
  ElectronWindowBuilder,
};
use tauri_utils::config::{Color, WebviewUrl};

const WAIT: Duration = Duration::from_secs(8);
type Target = DetachedWindow<(), ElectronRuntime>;

fn main() -> ExitCode {
  let runtime =
    <ElectronRuntime as Runtime<()>>::new(RuntimeInitArgs::default()).expect("start real Electron");
  let handle = <ElectronRuntime as Runtime<()>>::handle(&runtime);
  let opaque = Color(31, 97, 141, 255);
  let translucent = Color(179, 58, 58, 128);
  let first = create(
    &handle,
    "rendering-builder",
    ElectronWindowBuilder::new()
      .inner_size(360.0, 240.0)
      .transparent(true)
      .background_color(opaque),
  );
  let second = create(
    &handle,
    "rendering-dispatch",
    ElectronWindowBuilder::new().inner_size(360.0, 240.0),
  );
  let worker_handle = handle.clone();
  let worker_first = first.clone();
  let worker_second = second.clone();
  let worker = thread::spawn(move || {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
      exercise(&worker_first, &worker_second, opaque, translucent);
    }));
    let _ = WindowDispatch::<()>::destroy(&worker_first.dispatcher);
    let _ = WindowDispatch::<()>::destroy(&worker_second.dispatcher);
    let _ = <ElectronRuntimeHandle as RuntimeHandle<()>>::request_exit(
      &worker_handle,
      i32::from(result.is_err()),
    );
    result.is_ok()
  });
  let status = <ElectronRuntime as Runtime<()>>::run_return(runtime, |event| {
    if let RunEvent::ExitRequested { code: None, tx } = event {
      let _ = tx.send(ExitRequestedEventAction::Prevent);
    }
  });
  if worker.join().expect("join rendering behavior worker") && status == 0 {
    ExitCode::SUCCESS
  } else {
    ExitCode::FAILURE
  }
}

fn create(handle: &ElectronRuntimeHandle, label: &str, builder: ElectronWindowBuilder) -> Target {
  let mut pending =
    PendingWindow::<(), ElectronRuntime>::new(builder, label).expect("build rendering window");
  let url = format!(
    "data:text/html,<body data-proof='{label}' style='margin:0;background:transparent'>renderer</body>"
  );
  let mut page = PendingWebview::new(
    WebviewAttributes::new(WebviewUrl::External(url.parse().expect("renderer URL")))
      .transparent(true),
    label,
  )
  .expect("build rendering webview");
  page.url = url;
  pending.set_webview(page);
  <ElectronRuntimeHandle as RuntimeHandle<()>>::create_window(
    handle,
    pending,
    None::<fn(RawWindow<'_>)>,
  )
  .expect("create rendering window")
}

fn exercise(first: &Target, second: &Target, opaque: Color, translucent: Color) {
  let first_page = first
    .webview
    .as_ref()
    .expect("builder webview")
    .webview
    .dispatcher
    .clone();
  let second_page = second
    .webview
    .as_ref()
    .expect("dispatch webview")
    .webview
    .dispatcher
    .clone();
  wait_for_eval(
    &first_page,
    "document.body.dataset.proof",
    "rendering-builder",
  );
  wait_for_eval(
    &second_page,
    "document.body.dataset.proof",
    "rendering-dispatch",
  );

  // Registration is exercised, but the bridge has no native WebviewEvent source
  // (only window/lifecycle events), so no callback is treated as positive proof.
  WebviewDispatch::<()>::on_webview_event(&first_page, |_| {});
  WebviewDispatch::<()>::set_background_color(&first_page, Some(translucent))
    .expect("set webview background color");
  WindowDispatch::<()>::set_background_color(&first.dispatcher, Some(opaque))
    .expect("set window background color");
  WindowDispatch::<()>::set_background_color(&second.dispatcher, Some(translucent))
    .expect("set second window background color");
  assert_eq!(
    eval(&first_page, "document.body.dataset.proof"),
    serde_json::json!("rendering-builder")
  );
  assert_eq!(
    eval(&second_page, "document.body.dataset.proof"),
    serde_json::json!("rendering-dispatch")
  );

  unknown(
    "WindowBuilder::transparent",
    "real transparent windows and live renderer pages were created, but native alpha pixels need an independent OS observer",
  );
  unknown(
    "WindowBuilder::background_color",
    "the distinct opaque builder color was sent during real window creation, but native backing pixels need an independent compositor probe",
  );
  unknown(
    "WebviewDispatch::set_background_color",
    "the distinct translucent color was sent to a live webview, but browser backing pixels need an independent compositor probe",
  );
  unknown(
    "WindowDispatch::set_background_color",
    "distinct opaque/translucent colors were sent to two live windows, but native backing pixels need an independent compositor probe",
  );
  unknown(
    "WebviewDispatch::on_webview_event",
    "registration completed but this bridge exposes no valid observable native WebviewEvent stimulus",
  );
}

fn eval(page: &Webview, script: &str) -> serde_json::Value {
  let (tx, rx) = mpsc::channel();
  WebviewDispatch::<()>::eval_script_with_callback(page, script, move |value| {
    let _ = tx.send(value);
  })
  .expect("schedule renderer evaluation");
  let value = rx
    .recv_timeout(WAIT)
    .expect("renderer evaluation timed out");
  serde_json::from_str(&value).expect("renderer evaluation JSON")
}

fn wait_for_eval(page: &Webview, script: &str, expected: &str) {
  let started = Instant::now();
  loop {
    if eval(page, script) == serde_json::json!(expected) {
      return;
    }
    assert!(
      started.elapsed() < WAIT,
      "renderer did not reach expected value: {expected}"
    );
    thread::sleep(Duration::from_millis(40));
  }
}

fn unknown(member: &str, assertion: &str) {
  println!(
    "[runtime-evidence] {}",
    serde_json::json!({
      "member": member,
      "test": "rendering_behavior_smoke::exercise",
      "assertion": assertion,
      "kind": "behavior",
      "status": "unknown",
    })
  );
}
