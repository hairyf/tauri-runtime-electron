mod support;

use serde_json::{json, Value};
use std::path::Path;

fn observations(scenario: &str) -> Value {
  let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/webview-policy.mjs");
  serde_json::from_str(&support::node(script, &[scenario])).expect("webview policy observations")
}

#[test]
fn navigation_pauses_native_requests_and_cleans_up_every_decision_path() {
  let observed = observations("navigation");
  let clean = json!({ "pending": 0, "timers": 0, "destroyedListeners": [0, 0] });
  let waiting = &observed["waiting"];
  assert_eq!(waiting["main"]["request"]["webviewId"], 10);
  assert_eq!(waiting["child"]["request"]["webviewId"], 20);
  assert_ne!(
    waiting["main"]["request"]["id"],
    waiting["child"]["request"]["id"]
  );
  for owner in ["main", "child"] {
    assert_eq!(waiting[owner]["request"]["url"], waiting[owner]["url"]);
    assert_eq!(waiting[owner]["replies"], json!([]));
    assert_eq!(observed["decided"][owner]["method"], "POST");
    assert_eq!(observed["decided"][owner]["body"], "original POST body");
  }
  assert_eq!(
    waiting["cleanup"],
    json!({ "pending": 2, "timers": 2, "destroyedListeners": [1, 1] })
  );
  assert_eq!(
    waiting["timers"],
    json!([
      { "delay": 15_000, "unreferenced": true },
      { "delay": 15_000, "unreferenced": true },
    ])
  );

  for owner in ["child", "iframe"] {
    assert_eq!(
      observed["denied"][owner]["replies"],
      json!([{ "cancel": true }])
    );
    assert_eq!(observed["denied"][owner]["stopsAtReply"], json!([0]));
  }
  assert_eq!(observed["denied"]["main"]["replies"], json!([]));
  assert_eq!(observed["denied"]["stops"], json!([0, 0]));
  assert_eq!(
    observed["decided"]["main"]["replies"],
    json!([{ "cancel": false }])
  );
  assert_eq!(observed["decided"]["main"]["stopsAtReply"], json!([0]));
  assert_eq!(
    observed["decided"]["child"]["replies"],
    json!([{ "cancel": true }])
  );
  assert_eq!(observed["decided"]["cleanup"], clean);

  let bypass = observed["bypass"].as_array().unwrap();
  assert_eq!(bypass.len(), 4);
  for navigation in bypass {
    assert_eq!(navigation["replies"], json!([{}]));
    assert_eq!(navigation["stopsAtReply"], json!([0]));
    assert!(
      navigation["request"].is_null(),
      "unknown/no-policy/non-navigation requests bypass the host"
    );
  }
  for case in ["invalidAllow", "failedStop"] {
    assert_eq!(
      observed["rejected"][case]["replies"],
      json!([{ "cancel": true }])
    );
    assert_eq!(
      observed["rejected"][case]["stopsAtReply"],
      json!([1]),
      "stop precedes cancellation even when it throws"
    );
  }
  assert_eq!(observed["rejected"]["stopError"], "stop failed");
  assert_eq!(observed["rejected"]["cleanup"], clean);

  let lifecycle = observed["lifecycle"].as_array().unwrap();
  assert_eq!(lifecycle.len(), 2);
  for (case, (reason, stops)) in lifecycle.iter().zip([("timeout", 1), ("destroyed", 0)]) {
    assert_eq!(case["reason"], reason);
    assert_eq!(
      case["finished"]["navigation"]["replies"],
      json!([{ "cancel": true }])
    );
    assert_eq!(
      case["finished"]["navigation"]["stopsAtReply"],
      json!([stops])
    );
    assert_eq!(case["finished"]["cleanup"], clean);
    assert_eq!(
      case["afterLate"], case["finished"]["navigation"],
      "late decisions and timers answer only once"
    );
    assert_eq!(case["lateStops"], 0);
  }
  let retired = observed["retired"].as_array().unwrap();
  assert_eq!(retired.len(), 2);
  for (case, event) in retired.iter().zip(["completed", "errorOccurred"]) {
    assert_eq!(case["event"], event);
    assert_eq!(case["unrelatedPending"], true);
    assert_eq!(case["ended"], clean);
    assert_eq!(case["stale"]["replies"], json!([]));
    assert_eq!(case["newerWaiting"]["replies"], json!([]));
    assert_eq!(case["newer"]["replies"], json!([{ "cancel": false }]));
    assert_eq!(
      case["lateStops"], 0,
      "retired navigation cannot stop a newer request"
    );
  }
  let popups = observed["popups"].as_array().unwrap();
  assert_eq!(popups.len(), 9);
  for popup in popups {
    assert_eq!(popup["reply"], json!({ "action": "deny" }));
  }
  assert_eq!(observed["external"], json!([]));
  assert_eq!(
    observed["loads"],
    json!([["https://page.invalid/"], ["https://child.invalid/"]]),
    "decisions never replay POST navigation with loadURL"
  );
  assert_eq!(observed["cleanup"], clean);
}

#[test]
fn generated_preloads_scope_initialization_and_ipc_to_the_right_frames() {
  let observed = observations("preload");
  assert_eq!(
    observed["trafficLightPosition"],
    json!({ "x": 12, "y": 18 })
  );
  assert_eq!(observed["tabbingIdentifier"], "editor-tabs");
  assert_eq!(observed["plainPreload"], true);
  let frames = observed["frames"].as_array().unwrap();
  assert_eq!(frames.len(), 4);
  for (frame, (owner, is_main)) in frames.iter().zip([
    ("main", true),
    ("main", false),
    ("child", true),
    ("child", false),
  ]) {
    assert_eq!(frame["owner"], owner);
    assert_eq!(frame["isMainFrame"], is_main);
    assert_eq!(frame["required"], json!(["electron"]));
    assert_eq!(
      frame["preferences"],
      json!({
        "contextIsolation": true, "sandbox": true, "nodeIntegration": false,
        "nodeIntegrationInSubFrames": true,
      })
    );
    assert_eq!(
      frame["mainScript"],
      if is_main { json!(1) } else { Value::Null }
    );
    assert_eq!(frame["everyFrameScript"], 1);
    assert_eq!(frame["hasIpc"], is_main);
    if is_main {
      assert_eq!(frame["sent"], json!([["tauri-ipc", "42"]]));
      assert_eq!(frame["fetchReply"], "reply");
      assert_eq!(
        frame["fetchCall"],
        json!({
          "channel": "tauri-ipc-fetch", "body": [1, 2], "sameRequest": true, "calls": 1,
        })
      );
    } else {
      assert_eq!(frame["sent"], json!([]));
      assert!(frame["fetchReply"].is_null());
      assert!(frame["fetchCall"].is_null());
    }
  }
  assert_eq!(
    observed["fetchErrors"],
    json!([
      "unknown IPC sender",
      "unknown IPC sender",
      "unknown IPC sender"
    ])
  );
  assert_eq!(
    observed["messages"],
    json!([{
      "event": "ipc.message",
      "params": { "webviewId": 10, "url": "https://page.invalid/", "body": "main message" },
    }]),
    "non-main, unknown and opaque senders never emit IPC"
  );
}

#[test]
fn permissions_route_by_webview_and_require_an_explicit_allow_decision() {
  let observed = observations("permissions");
  let decisions = observed["decisions"].as_array().unwrap();
  let expected = [
    ("media", "camera", 10, true),
    ("media", "microphone", 20, false),
    ("media", "microphone", 10, false),
    ("geolocation", "geolocation", 20, false),
    ("notifications", "notifications", 10, true),
    ("clipboard-read", "clipboard-read", 20, true),
    ("display-capture", "display-capture", 10, true),
    ("midi", "midi", 20, true),
    ("midiSysex", "midi", 10, true),
    ("sensors", "sensors", 20, true),
  ];
  assert_eq!(decisions.len(), expected.len());
  let mut ids = Vec::new();
  for (decision, (permission, kind, webview, allowed)) in decisions.iter().zip(expected) {
    assert_eq!(decision["permission"], permission);
    assert_eq!(decision["request"]["kind"], kind);
    assert_eq!(decision["request"]["webviewId"], webview);
    assert_eq!(decision["waiting"], json!([]));
    assert_eq!(
      decision["replies"],
      json!([allowed]),
      "duplicate permission replies are ignored"
    );
    let id = decision["request"]["id"].as_u64().unwrap();
    assert!(id > 0 && !ids.contains(&id));
    ids.push(id);
  }
  let bypass = observed["bypass"].as_array().unwrap();
  assert_eq!(bypass.len(), 3);
  for decision in bypass {
    assert!(decision["request"].is_null());
    assert_eq!(decision["replies"], json!([false]));
  }
  assert_eq!(observed["pending"], 0);
  assert_eq!(observed["timers"], 0);
}

#[test]
fn sessions_isolate_profiles_and_install_policies_and_protocols_once() {
  let observed = observations("sessions");
  assert!(observed["defaultSelection"].is_null());
  assert_eq!(
    observed["aliases"],
    json!([observed["shared"], observed["shared"]])
  );
  assert_ne!(observed["shared"], observed["separate"]);
  let private = observed["private"].as_array().unwrap();
  assert_eq!(private.len(), 2);
  assert_ne!(private[0], private[1]);
  for session in private {
    assert_ne!(session, &observed["shared"]);
  }
  let partitions = observed["partitions"].as_array().unwrap();
  assert_eq!(partitions.len(), 2);
  for partition in partitions {
    assert!(!partition.as_str().unwrap().starts_with("persist:"));
  }
  let paths = observed["pathCalls"].as_array().unwrap();
  assert_eq!(
    paths.len(),
    5,
    "incognito takes precedence over a data directory"
  );
  for path in paths {
    assert_eq!(path["absolute"], true);
  }
  assert_eq!(paths[0]["path"], paths[1]["path"]);
  assert_eq!(paths[0]["path"], paths[2]["path"]);
  let retirement = &observed["retirement"];
  assert_eq!(
    retirement["remaining"],
    json!([retirement["active"]["request"]["id"]]),
    "native request IDs are scoped to a session"
  );
  assert_eq!(retirement["retired"]["replies"], json!([]));
  assert_eq!(
    retirement["active"]["replies"],
    json!([{ "cancel": false }])
  );

  let targets = observed["targets"].as_array().unwrap();
  assert_eq!(targets.len(), 6);
  for target in targets {
    assert_eq!(target["navigationInstalls"], 1);
    assert_eq!(target["requestEndInstalls"], 2);
    assert_eq!(target["permissionInstalls"], 1);
    assert_eq!(target["downloadListeners"], 1);
    assert_eq!(
      target["protocols"],
      json!(["asset", "http", "https", "tauri"])
    );
    assert_eq!(
      target["protocolCalls"], target["protocols"],
      "duplicate registration must not replace handlers"
    );
    assert_eq!(target["tauri"]["event"], "protocol.request");
    assert_eq!(target["tauri"]["params"]["uri"], "tauri://localhost/first");
    assert_eq!(target["asset"]["event"], "protocol.request");
    assert_eq!(target["asset"]["params"]["uri"], "asset://localhost/second");
    assert_eq!(target["asset"]["params"]["scheme"], "asset");
    let fallback = target["fallback"].as_array().unwrap();
    assert_eq!(fallback.len(), 3);
    for (request, url) in fallback.iter().zip([
      "https://remote.invalid/",
      "http://tauri.localhost:1420/",
      "http://unregistered.localhost/",
    ]) {
      assert_eq!(request["url"], url);
      assert_eq!(request["body"], "ordinary HTTP");
      assert_eq!(request["sameRequest"], true);
      assert_eq!(request["receiver"], true);
      assert_eq!(
        request["options"],
        json!({ "bypassCustomProtocolHandlers": true })
      );
    }
    assert_eq!(
      target["forwarded"], 3,
      "fallback fetch uses this session, not the default session"
    );
    assert_eq!(target["protocolMessagesDuringFallback"], 0);
  }
  assert_eq!(
    observed["cleanup"],
    json!({ "pending": 0, "timers": 0, "destroyedListeners": [0, 0] })
  );
}
