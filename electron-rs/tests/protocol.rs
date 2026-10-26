mod support;

use serde_json::{json, Value};
use std::path::Path;

const CSP: &str = "default-src 'none'; frame-src http://isolation-test.localhost";

fn observe(scenario: &str) -> Value {
  serde_json::from_str(&support::node(
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/protocol.mjs"),
    &[scenario],
  )).expect("protocol observations")
}

#[test]
fn virtual_protocols_preserve_origins_and_leave_ordinary_http_untouched() {
  let result = observe("http");
  assert_eq!(result["registrations"], json!(["tauri", "http", "https", "ipc", "asset", "isolation-test"]));
  for (index, transport) in ["http", "https"].into_iter().enumerate() {
    let item = &result["virtual"][index];
    assert_eq!(item["url"], format!("{transport}://tauri.localhost/assets/a%20b.js?v=1"));
    assert_eq!(item["sent"], json!({
      "event": "protocol.request", "scheme": "tauri", "uri": "tauri://localhost/assets/a%20b.js?v=1", "method": "POST",
      "headers": {"origin": format!("{transport}://tauri.localhost"), "content-type": "text/plain;charset=UTF-8"},
      "body": b"payload",
    }));
    assert_eq!(item["response"], json!({"status": 200,
      "headers": {"content-security-policy": CSP, "content-type": "text/plain"}, "body": b"resolved locally"}));
  }
  assert_eq!(result["origins"], json!(["http://tauri.localhost", "https://remote.example", "null"]));
  assert_eq!(result["isolation"], json!({"event": "protocol.request", "scheme": "isolation-test", "uri": "isolation-test://localhost/",
    "method": "GET", "headers": {}, "body": []}));
  assert_eq!(result["asset"]["event"], "protocol.request");
  assert_eq!(result["asset"]["uri"], "asset://localhost/file.png");
  let ordinary = result["ordinary"].as_array().unwrap();
  assert_eq!(ordinary.len(), 7);
  for item in ordinary {
    assert_eq!(item, &json!({"body": "ordinary HTTP", "sameRequest": true,
      "options": {"bypassCustomProtocolHandlers": true}}));
  }
  assert_eq!(result["forwardedToRust"], 0);
}

#[test]
fn ipc_authenticates_native_senders_and_preserves_binary_bytes() {
  let result = observe("ipc");
  assert_eq!(result["anonymous"], json!(["error", "error"]));
  assert_eq!(result["anonymousRequests"], 0);
  assert_eq!(result["acceptedMessages"], json!([
    {"webviewId": 1, "url": "http://tauri.localhost/", "body": "ping"},
    {"webviewId": 2, "url": "tauri://localhost/second", "body": "ping"},
    {"webviewId": 3, "url": "https://remote.example/", "body": "ping"},
  ]));
  assert_eq!(result["rejectedMessages"], 0);
  let binary = result["binary"].as_array().unwrap();
  assert_eq!(binary.len(), 3);
  for (index, origin) in ["http://tauri.localhost", "tauri://localhost", "https://remote.example"].into_iter().enumerate() {
    let item = &binary[index];
    assert_eq!(item["sent"], json!({
      "event": "protocol.request", "scheme": "ipc", "webviewId": index + 1, "uri": "ipc://localhost/echo?v=1", "method": "POST",
      "headers": {"origin": origin, "tauri-invoke-key": "test-key", "content-type": "application/octet-stream", "x-test": "request"},
      "body": [0, 1, 127, 128, 255],
    }));
    assert_eq!(item["typedBytes"], true);
    assert_eq!(item["response"], json!({"status": 206, "body": [0, 1, 127, 128, 255],
      "headers": {"content-security-policy": CSP, "tauri-response": "ok", "content-type": "application/octet-stream", "x-test": "binary"}}));
  }
  for (field, count, error) in [("invalidUrls", 10, "invalid IPC request"), ("invalidSenders", 4, "unknown IPC sender"),
    ("invalidPayloads", 4, "invalid IPC request")] {
    let errors = result[field].as_array().unwrap();
    assert_eq!(errors.len(), count, "{field}");
    for actual in errors { assert_eq!(actual, error, "{field}"); }
  }
  assert_eq!(result["rejectedRequests"], 0);
}

#[test]
fn head_and_null_body_statuses_preserve_representation_headers_without_payloads() {
  let result = observe("bodies");
  let rows = result.as_array().unwrap();
  assert_eq!(rows.len(), 16);
  for row in rows {
    let status = row["status"].as_u64().unwrap();
    let empty = row["method"] == "HEAD" || status != 200;
    assert_eq!(row["nullBody"], empty, "{row}");
    assert_eq!(row["response"]["status"], status);
    let bytes = if empty { b"".as_slice() } else { row["input"].as_str().unwrap().as_bytes() };
    assert_eq!(row["response"]["body"], json!(bytes));
    let mut headers = json!({"content-security-policy": CSP, "content-type": "text/plain", "etag": "\"unchanged\""});
    if status != 204 && status != 205 {
      headers["content-length"] = json!("18");
      headers["transfer-encoding"] = json!("chunked");
    }
    assert_eq!(row["response"]["headers"], headers, "{row}");
  }
}
