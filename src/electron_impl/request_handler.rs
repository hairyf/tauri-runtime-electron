// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! Custom URI schemes: the Electron equivalent of CEF's `RequestHandler`, `ResourceHandler`
//! and `SchemeHandlerFactory`.
//!
//! The reference's trio does four things. It answers TLS certificate errors, it reports
//! renderer termination, it applies the main-frame navigation policy, and it serves the
//! application's custom schemes from a `SchemeHandlerFactory` whose `ResourceHandler` reads
//! the request body, repairs a racy `Origin` header, runs the application's protocol handler
//! on the main thread and writes the response back. Only the last one is still this file's
//! job in Electron:
//!
//! * certificate errors are a `session.setCertificateVerifyProc` decision the bridge makes,
//!   from the policy [`certificate_error_answer`] spells out;
//! * renderer termination is `webview.processGone`, handled in `client/life_span.rs`, with
//!   [`termination_reason`] doing the mapping;
//! * the navigation policy is a blocking `webview.navigationRequested`, handled in
//!   `client/load.rs`.
//!
//! # What is deliberately not ported
//!
//! **The HTML and CSP rewriting.** The reference injects the webview's initialization
//! scripts into a `text/html` body, extends the document's `Content-Security-Policy` with
//! their hashes, and moves the `<meta charset>` declaration back into the first 1024 bytes so
//! Chromium does not decode the document as windows-1252. All three exist because CEF serves
//! the document itself and has to make the scripts it wants to run fit the page it is
//! serving. Electron injects initialization scripts itself, through `webview.create`'s
//! `initScripts` and `preloadScripts`, before the document runs — so there is no body to
//! rewrite, no CSP to extend and no encoding declaration to rescue. Porting them would
//! rewrite every HTML response twice for no effect.
//!
//! **The `Origin` repair.** The reference inserts the initiating main frame's origin when the
//! renderer sent none or a literal `null`, because a renderer can issue a request before its
//! execution context is wired to the loader. The bridge performs the same repair in
//! `protocol.request`, where it has the guest and its frame; the header arrives here already
//! repaired.
//!
//! **The scheme registry.** `SchemeRegistry` is keyed by `(browser identifier, scheme)` and
//! exists because a CEF scheme factory is registered per request context. Electron registers
//! a scheme once, app-wide, with `protocol.register`, and the handlers live in
//! [`RuntimeContext::protocol_handlers`] keyed by scheme alone.
//!
//! **The main-thread hop.** The reference runs the protocol handler on the runtime's main
//! thread because a CEF `ResourceHandler` runs on the IO thread, and a command that waits on
//! the main thread would deadlock with the event loop from there. `protocol.request` arrives
//! on the run loop's own thread, so the handler already runs where the reference wanted it;
//! [`MainThreadDispatcher`] is kept for a caller that needs to name that hop explicitly.

use std::{borrow::Cow, sync::Arc};

use base64::Engine;
use http::{
  HeaderMap, HeaderName, HeaderValue,
  header::{CONTENT_TYPE, ORIGIN},
};
use serde_json::{Map, Value, json};
use tauri_runtime::{UserEvent, webview::WebContentProcessTerminationReason};

use crate::runtime::{IPC_TIMEOUT, Message, RuntimeContext};

/// A scheme's handler and the webview that registered it.
///
/// The label is the answer for a request that cannot name its own webview, which is every
/// session-level one.
#[derive(Clone)]
pub(crate) struct RegisteredProtocol {
  pub(crate) label: String,
  pub(crate) handler: SchemeHandler,
}

/// The application's handler for one custom scheme.
///
/// The reference's `UriSchemeProtocolHandler` takes a responder so it can answer later, and so
/// does this one. A handler that blocked until the responder fired would deadlock the run loop:
/// an `async` Tauri command answers from the async runtime, and reaching a window API from it
/// needs the run loop to turn — which a blocked loop cannot do.
pub(crate) type SchemeHandler = Arc<
  dyn Fn(&str, http::Request<Vec<u8>>, Box<dyn FnOnce(http::Response<Cow<'static, [u8]>>) + Send>)
    + Send
    + Sync,
>;

/// Runs a closure on the runtime's main thread.
///
/// Type-erased so a handler that is not generic over the user event type can carry it.
/// Electron's protocol handling does not need the hop — see the module docs — but the type is
/// what the reference's `WebResourceHandler` carried, and a caller that must reach the run
/// loop has no other name for it.
pub(crate) type MainThreadDispatcher = Arc<dyn Fn(Box<dyn FnOnce() + Send>) + Send + Sync>;

/// Builds a [`MainThreadDispatcher`] for a runtime context.
pub(crate) fn main_thread_dispatcher<T: UserEvent>(
  context: &RuntimeContext<T>,
) -> MainThreadDispatcher {
  let context = context.clone();
  Arc::new(move |task| {
    if let Err(error) = context.send_message(Message::Task(task)) {
      log::warn!("failed to run a task on the runtime thread: {error}");
    }
  })
}

/// The bridge is asking the application to serve a custom-scheme request.
///
/// The answer travels back as one `protocol.respond` call, which may be sent from any thread:
/// an `async` command answers after the run loop has turned, so this function hands the
/// responder to the handler and returns without waiting for it.
pub(crate) fn on_protocol_request<T: UserEvent>(context: &RuntimeContext<T>, params: &Value) {
  let Some(id) = params["id"].as_u64() else {
    log::debug!("ignoring a `protocol.request` without an id");
    return;
  };
  let scheme = params["scheme"].as_str().unwrap_or_default();

  let Some(registered) = context.protocol(scheme) else {
    log::debug!("no protocol handler is registered for the `{scheme}` scheme");
    respond(&context.ipc, id, not_found(scheme));
    return;
  };
  let handler = &registered.handler;

  // The label the handler is called with belongs to the webview that asked. A request that
  // carries its identity (the authenticated IPC fetch) is answered for that webview; the
  // session-level ones — every navigation and subresource — carry none, because Electron's
  // `protocol.handle` has no sender to name, so they fall back to the webview that registered
  // the scheme.
  let label = crate::electron_impl::client::webview_id(params)
    .and_then(|webview_id| crate::webview::with_app_webview(webview_id, |w| w.label.clone()))
    .unwrap_or_else(|| registered.label.clone());

  let uri = params["uri"].as_str().unwrap_or_default();
  let method = http::Method::from_bytes(params["method"].as_str().unwrap_or_default().as_bytes())
    .unwrap_or(http::Method::GET);

  let mut request = http::Request::builder().method(method).uri(uri);
  if let Some(headers) = request.headers_mut() {
    *headers = request_headers(params);
  }
  let body = request_body(params);
  let Ok(request) = request.body(body) else {
    // The reference drops a URL `http::Request` refuses rather than answering it: there is
    // no request to hand the application.
    log::debug!("dropping a custom-scheme request whose URI `http::Request` refused: {uri:?}");
    respond(&context.ipc, id, not_found(scheme));
    return;
  };

  let ipc = Arc::clone(&context.ipc);
  handler(
    &label,
    request,
    Box::new(move |response| respond(&ipc, id, response)),
  );
}

/// Answers a custom-scheme request.
fn respond(
  ipc: &crate::electron_impl::ipc::Ipc,
  id: u64,
  response: http::Response<Cow<'static, [u8]>>,
) {
  let (parts, body) = response.into_parts();
  let payload = json!({
    "id": id,
    "status": parts.status.as_u16(),
    "headers": response_headers(&parts.headers),
    "body": base64::engine::general_purpose::STANDARD.encode(body.as_ref()),
  });
  if let Err(error) = ipc.call("protocol.respond", payload, IPC_TIMEOUT) {
    log::warn!("failed to answer the custom-scheme request {id}: {error}");
  }
}

/// The response a scheme with no handler answers with.
fn not_found(scheme: &str) -> http::Response<Cow<'static, [u8]>> {
  let body = format!("no protocol handler is registered for the `{scheme}` scheme");
  http::Response::builder()
    .status(http::StatusCode::NOT_FOUND)
    .header(CONTENT_TYPE, "text/plain; charset=utf-8")
    .body(Cow::Owned(body.into_bytes()))
    .unwrap_or_else(|_| http::Response::new(Cow::Owned(Vec::new())))
}

/// The request headers the bridge forwarded.
fn request_headers(params: &Value) -> HeaderMap {
  let mut headers = HeaderMap::new();
  let Some(entries) = params["headers"].as_object() else {
    return headers;
  };
  for (name, value) in entries {
    let (Ok(name), Some(value)) = (
      HeaderName::from_bytes(name.as_bytes()),
      value
        .as_str()
        .and_then(|value| HeaderValue::from_str(value).ok()),
    ) else {
      continue;
    };
    headers.append(name, value);
  }
  headers
}

/// The response headers, as the bridge's `Record<string, string>`.
fn response_headers(headers: &HeaderMap) -> Map<String, Value> {
  let mut entries = Map::new();
  for (name, value) in headers {
    // A header value that is not UTF-8 cannot be spelled in a JSON string, and the bridge
    // would have to reject it anyway.
    if let Ok(value) = value.to_str() {
      entries.insert(name.as_str().to_owned(), json!(value));
    }
  }
  entries
}

/// The request body, decoded from the base64 the bridge sends.
fn request_body(params: &Value) -> Vec<u8> {
  params["body"]
    .as_str()
    .and_then(|body| base64::engine::general_purpose::STANDARD.decode(body).ok())
    .unwrap_or_default()
}

/// Whether a request carries an `Origin` the bridge could not repair.
///
/// The bridge overwrites `headers.origin` with the request's initiator origin when it has
/// one, so this is a diagnostic rather than a policy: a request that arrives with no origin
/// or a literal `null` is one whose initiator was not the non-opaque main frame, which is
/// exactly the case the reference leaves alone.
pub(crate) fn origin_is_unrepaired(headers: &HeaderMap) -> bool {
  headers
    .get(ORIGIN)
    .map(|value| value.as_bytes() == b"null")
    .unwrap_or(true)
}

/// The Electron answer for a TLS certificate that did not validate, per
/// [`CertificateErrorPolicy`](crate::CertificateErrorPolicy).
///
/// `session.setCertificateVerifyProc` answers with `0` to use Chromium's own verdict — which
/// shows the SSL interstitial with its "proceed anyway" link — and `-2` to fail the request
/// outright, so the page gets a network error it cannot click past. That is the reference's
/// `ChromeInterstitial` / `Cancel` split, with Electron's numbers.
pub(crate) fn certificate_error_answer(policy: crate::CertificateErrorPolicy) -> i32 {
  match policy {
    crate::CertificateErrorPolicy::ChromeInterstitial => 0,
    crate::CertificateErrorPolicy::Cancel => -2,
  }
}

/// Logs a cancelled certificate error without naming the URL.
///
/// The URL can carry credentials and tokens, so only its origin is logged — the reference's
/// rule, kept verbatim.
pub(crate) fn log_cancelled_certificate(request_url: &str, error: &str) {
  let origin = url::Url::parse(request_url)
    .map(|url| url.origin().ascii_serialization())
    .unwrap_or_else(|_| "an unknown origin".to_owned());
  log::warn!(
    "cancelling a request to {origin}: its TLS certificate did not validate ({error}), and the \
     runtime is set to CertificateErrorPolicy::Cancel"
  );
}

/// The portable termination reason Electron's `render-process-gone` reason names.
///
/// Electron reports `clean-exit`, `abnormal-exit`, `killed`, `crashed`, `oom`,
/// `launch-failed` and `integrity-failure`. Everything else — including the empty string a
/// malformed payload produces — is `Unknown`, which is never a normal exit.
pub(crate) fn termination_reason(reason: &str) -> WebContentProcessTerminationReason {
  use WebContentProcessTerminationReason as Reason;
  match reason {
    "clean-exit" => Reason::Normal,
    "abnormal-exit" => Reason::Abnormal,
    "killed" => Reason::Killed,
    "crashed" => Reason::Crashed,
    "oom" => Reason::OutOfMemory,
    "launch-failed" => Reason::LaunchFailed,
    "integrity-failure" => Reason::IntegrityFailure,
    _ => Reason::Unknown,
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::CertificateErrorPolicy;

  fn handler(status: u16, body: &'static str) -> SchemeHandler {
    Arc::new(move |_label, _request, respond| {
      respond(
        http::Response::builder()
          .status(status)
          .header(CONTENT_TYPE, "text/html")
          .body(Cow::Borrowed(body.as_bytes()))
          .unwrap(),
      );
    })
  }

  /// Runs a handler and returns the response it answered with.
  fn answer(handler: &SchemeHandler) -> http::Response<Cow<'static, [u8]>> {
    let (tx, rx) = std::sync::mpsc::channel();
    handler(
      "main",
      http::Request::new(Vec::new()),
      Box::new(move |response| {
        let _ = tx.send(response);
      }),
    );
    rx.recv().expect("the handler answered")
  }

  #[test]
  fn every_electron_termination_reason_is_preserved() {
    use WebContentProcessTerminationReason as Reason;
    for (reason, expected) in [
      ("clean-exit", Reason::Normal),
      ("abnormal-exit", Reason::Abnormal),
      ("killed", Reason::Killed),
      ("crashed", Reason::Crashed),
      ("oom", Reason::OutOfMemory),
      ("launch-failed", Reason::LaunchFailed),
      ("integrity-failure", Reason::IntegrityFailure),
      ("", Reason::Unknown),
      ("something-new", Reason::Unknown),
    ] {
      assert_eq!(termination_reason(reason), expected, "{reason}");
    }
  }

  #[test]
  fn the_certificate_policy_maps_onto_electrons_numbers() {
    assert_eq!(
      certificate_error_answer(CertificateErrorPolicy::ChromeInterstitial),
      0
    );
    assert_eq!(certificate_error_answer(CertificateErrorPolicy::Cancel), -2);
  }

  #[test]
  fn a_request_body_round_trips_through_base64() {
    let body = base64::engine::general_purpose::STANDARD.encode(b"hello");
    assert_eq!(request_body(&json!({ "body": body })), b"hello");
    assert!(request_body(&json!({ "body": "not base64!" })).is_empty());
    assert!(request_body(&json!({})).is_empty());
  }

  #[test]
  fn request_headers_become_a_header_map() {
    let headers = request_headers(&json!({
      "headers": { "content-type": "application/json", "origin": "tauri://localhost" }
    }));
    assert_eq!(headers.get(CONTENT_TYPE).unwrap(), "application/json");
    assert_eq!(headers.get(ORIGIN).unwrap(), "tauri://localhost");
    assert!(!origin_is_unrepaired(&headers));
  }

  #[test]
  fn a_null_origin_is_reported_as_unrepaired() {
    let headers = request_headers(&json!({ "headers": { "origin": "null" } }));
    assert!(origin_is_unrepaired(&headers));
    assert!(origin_is_unrepaired(&HeaderMap::new()));
  }

  #[test]
  fn a_handler_answer_carries_its_status_headers_and_body() {
    let response = answer(&handler(201, "created"));
    let (parts, body) = response.into_parts();
    assert_eq!(parts.status.as_u16(), 201);
    assert_eq!(
      response_headers(&parts.headers)["content-type"],
      "text/html"
    );
    assert_eq!(body.as_ref(), b"created");
  }

  #[test]
  fn an_unregistered_scheme_answers_not_found() {
    let response = not_found("asset");
    assert_eq!(response.status(), http::StatusCode::NOT_FOUND);
    assert!(
      String::from_utf8_lossy(response.body().as_ref()).contains("asset"),
      "the answer names the scheme that has no handler"
    );
  }
}
