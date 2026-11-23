// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! The cookie store of one webview.
//!
//! The reference holds a CEF `CookieManager` per request context and drives it with two
//! `CookieVisitor` wrappers that collect cookies into a shared vector and deliver the result
//! over an `mpsc::Sender`, because a visit is asynchronous and CEF may report nothing at all.
//! Electron has no cookie handle: a webview's cookies live in the `session` its partition
//! names, and the bridge answers `webview.cookies` / `webview.setCookie` /
//! `webview.deleteCookie` synchronously. The webview id therefore *is* the manager.
//!
//! # Why the result still travels over a channel
//!
//! A caller written against the reference blocks on `recv` for exactly one result. That
//! contract is kept rather than simplified away, because the failure it protects against is
//! the same one: a query that answers nothing hangs the caller forever. The result is sent
//! from a guard's `Drop`, so every exit path — a successful query, an empty store, a refused
//! URL, a failed RPC, or a panic while converting — sends exactly once.

use std::sync::mpsc::Sender;

use serde_json::{Map, Value, json};
use tauri_runtime::{Cookie, Result};

use crate::{electron_impl::ipc::Ipc, runtime::IPC_TIMEOUT};

/// Where a cookie query delivers its result.
pub(crate) type CookieResultSender = Sender<Result<Vec<Cookie<'static>>>>;

/// The cookies collected so far, shared with the guard that delivers them.
type CollectedCookies = Vec<Cookie<'static>>;

/// Converts one cookie from the bridge's wire shape.
///
/// The field mapping is the reference's, with the two fields CEF has no place for carried
/// through rather than dropped: `sameSite` and `expirationDate` are part of what Electron
/// stores, and a cookie read back without them would be re-written as a lax session cookie.
fn cookie_from_value(value: &Value) -> Option<Cookie<'static>> {
  let name = value["name"].as_str()?.to_owned();
  let cookie_value = value["value"].as_str().unwrap_or_default().to_owned();

  let mut builder = Cookie::build((name, cookie_value));
  if let Some(domain) = value["domain"].as_str().filter(|domain| !domain.is_empty()) {
    builder = builder.domain(domain.to_owned());
  }
  if let Some(path) = value["path"].as_str().filter(|path| !path.is_empty()) {
    builder = builder.path(path.to_owned());
  }
  if value["secure"].as_bool().unwrap_or(false) {
    builder = builder.secure(true);
  }
  if value["httpOnly"].as_bool().unwrap_or(false) {
    builder = builder.http_only(true);
  }
  if let Some(same_site) = value["sameSite"].as_str().and_then(same_site_from_name) {
    builder = builder.same_site(same_site);
  }
  if let Some(expires) = value["expirationDate"]
    .as_f64()
    .and_then(expiration_from_unix_seconds)
  {
    builder = builder.expires(expires);
  }

  Some(builder.build().into_owned())
}

/// The wire shape of one cookie, for `webview.setCookie`.
fn cookie_payload(cookie: &Cookie<'_>) -> Value {
  let mut payload = Map::new();
  payload.insert("name".into(), json!(cookie.name()));
  payload.insert("value".into(), json!(cookie.value()));
  if let Some(domain) = cookie.domain() {
    payload.insert("domain".into(), json!(domain));
  }
  if let Some(path) = cookie.path() {
    payload.insert("path".into(), json!(path));
  }
  payload.insert("secure".into(), json!(cookie.secure().unwrap_or(false)));
  payload.insert(
    "httpOnly".into(),
    json!(cookie.http_only().unwrap_or(false)),
  );
  if let Some(same_site) = cookie.same_site().and_then(same_site_name) {
    payload.insert("sameSite".into(), json!(same_site));
  }
  if let Some(expires) = cookie.expires().and_then(expiration_seconds) {
    payload.insert("expirationDate".into(), json!(expires));
  }
  Value::Object(payload)
}

/// Electron's spelling of a same-site policy.
fn same_site_name(same_site: cookie::SameSite) -> Option<&'static str> {
  match same_site {
    cookie::SameSite::Strict => Some("strict"),
    cookie::SameSite::Lax => Some("lax"),
    cookie::SameSite::None => Some("no_restriction"),
  }
}

/// The same-site policy Electron's spelling names.
fn same_site_from_name(name: &str) -> Option<cookie::SameSite> {
  match name {
    "strict" => Some(cookie::SameSite::Strict),
    "lax" => Some(cookie::SameSite::Lax),
    "no_restriction" | "none" => Some(cookie::SameSite::None),
    _ => None,
  }
}

/// The expiry instant Electron's seconds-since-epoch number names.
fn expiration_from_unix_seconds(seconds: f64) -> Option<cookie::Expiration> {
  cookie::time::OffsetDateTime::from_unix_timestamp(seconds as i64)
    .ok()
    .map(cookie::Expiration::DateTime)
}

/// The seconds-since-epoch number of a cookie's expiry, when it has one.
fn expiration_seconds(expiration: cookie::Expiration) -> Option<i64> {
  match expiration {
    cookie::Expiration::DateTime(datetime) => Some(datetime.unix_timestamp()),
    // A session cookie has no instant to report; Electron spells that as an absent
    // `expirationDate`, which is what a session cookie is.
    cookie::Expiration::Session => None,
  }
}

/// Delivers a query's result exactly once, however the query ends.
struct CookieQuery {
  tx: CookieResultSender,
  collected: CollectedCookies,
}

impl CookieQuery {
  fn new(tx: CookieResultSender) -> Self {
    Self {
      tx,
      collected: Vec::new(),
    }
  }

  /// Replaces the collected cookies with what the bridge answered.
  fn collect(&mut self, cookies: Vec<Cookie<'static>>) {
    self.collected = cookies;
  }
}

impl Drop for CookieQuery {
  fn drop(&mut self) {
    let _ = self.tx.send(Ok(std::mem::take(&mut self.collected)));
  }
}

/// Queries the cookies Electron holds for `url`.
///
/// The reference passes `includeHttpOnly = 1`; Electron's `session.cookies.get` returns
/// http-only cookies for the same reason — the main process is not a renderer, so the flag
/// that hides them from page script does not apply.
pub(crate) fn visit_url_cookies(ipc: &Ipc, webview_id: u32, url: url::Url, tx: CookieResultSender) {
  let mut query = CookieQuery::new(tx);
  match ipc.call(
    "webview.cookies",
    json!({ "webviewId": webview_id, "url": url.as_str() }),
    IPC_TIMEOUT,
  ) {
    Ok(cookies) => query.collect(cookies_from_wire(&cookies)),
    Err(error) => {
      log::debug!("failed to read the cookies of webview {webview_id}: {error}");
    }
  }
}

/// Queries every cookie Electron holds for the webview's session.
pub(crate) fn visit_all_cookies(ipc: &Ipc, webview_id: u32, tx: CookieResultSender) {
  let mut query = CookieQuery::new(tx);
  match ipc.call(
    "webview.cookies",
    json!({ "webviewId": webview_id }),
    IPC_TIMEOUT,
  ) {
    Ok(cookies) => query.collect(cookies_from_wire(&cookies)),
    Err(error) => {
      log::debug!("failed to read the cookies of webview {webview_id}: {error}");
    }
  }
}

/// Writes one cookie into the webview's store.
///
/// The reference passes no completion callback, so the write is fire-and-forget; the RPC is
/// synchronous here, but a failure is still only logged, because a cookie write is not
/// something the caller of `WebviewDispatch::set_cookie` can act on.
pub(crate) fn set_cookie(ipc: &Ipc, webview_id: u32, url: Option<String>, cookie: Cookie<'static>) {
  let mut params = cookie_payload(&cookie);
  if let (Some(map), Some(url)) = (params.as_object_mut(), url) {
    map.insert("url".into(), json!(url));
  }
  if let Some(map) = params.as_object_mut() {
    map.insert("webviewId".into(), json!(webview_id));
  }
  if let Err(error) = ipc.call("webview.setCookie", params, IPC_TIMEOUT) {
    log::warn!("failed to set a cookie on webview {webview_id}: {error}");
  }
}

/// Removes one cookie from the webview's store by name.
pub(crate) fn delete_cookie(
  ipc: &Ipc,
  webview_id: u32,
  url: Option<String>,
  cookie: Cookie<'static>,
) {
  let mut params = json!({ "webviewId": webview_id, "name": cookie.name() });
  if let (Some(map), Some(url)) = (params.as_object_mut(), url) {
    map.insert("url".into(), json!(url));
  }
  if let Err(error) = ipc.call("webview.deleteCookie", params, IPC_TIMEOUT) {
    log::warn!("failed to delete a cookie from webview {webview_id}: {error}");
  }
}

/// The cookie store handle of a webview.
///
/// CEF hands out a `CookieManager` per request context, which is what the reference's
/// `AppWebview::cookie_manager` returns. Electron has no such handle: the store belongs to
/// the `session` the webview's partition names, and the only way to reach it is the webview
/// id the functions above take. The accessor therefore answers [`None`] for a native handle;
/// a caller that needs the store uses the id it already has.
pub(crate) fn cookie_manager(_webview_id: u32) -> Option<()> {
  None
}

/// The cookies in a `webview.cookies` answer.
fn cookies_from_wire(value: &Value) -> Vec<Cookie<'static>> {
  value
    .as_array()
    .map(|cookies| cookies.iter().filter_map(cookie_from_value).collect())
    .unwrap_or_default()
}

/// The URL a cookie is written against, when the caller did not name one.
///
/// Electron stores a cookie against a URL rather than against a bare domain, so a cookie
/// that names only its domain has to be given a scheme to be written at all. `https` for a
/// secure cookie and `http` otherwise is what a browser would have used to set it.
pub(crate) fn cookie_url(cookie: &Cookie<'_>, fallback: Option<&url::Url>) -> String {
  if let Some(domain) = cookie.domain() {
    let scheme = if cookie.secure().unwrap_or(false) {
      "https"
    } else {
      "http"
    };
    return format!("{scheme}://{domain}/");
  }
  fallback
    .and_then(|url| url.host_str().map(|host| (url.scheme(), host)))
    .map(|(scheme, host)| format!("{scheme}://{host}/"))
    .unwrap_or_else(|| "http://localhost/".to_owned())
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn a_cookie_round_trips_through_the_wire_shape() {
    let value = json!({
      "name": "session",
      "value": "abc",
      "domain": "example.test",
      "path": "/app",
      "secure": true,
      "httpOnly": true,
      "sameSite": "no_restriction",
      "expirationDate": 1_700_000_000,
    });
    let cookie = cookie_from_value(&value).expect("the wire shape parses");
    assert_eq!(cookie.name(), "session");
    assert_eq!(cookie.value(), "abc");
    assert_eq!(cookie.domain(), Some("example.test"));
    assert_eq!(cookie.path(), Some("/app"));
    assert_eq!(cookie.secure(), Some(true));
    assert_eq!(cookie.http_only(), Some(true));
    assert_eq!(cookie.same_site(), Some(cookie::SameSite::None));
    assert_eq!(
      expiration_seconds(cookie.expires().unwrap()),
      Some(1_700_000_000)
    );

    let payload = cookie_payload(&cookie);
    assert_eq!(payload["sameSite"], "no_restriction");
    assert_eq!(payload["expirationDate"], 1_700_000_000);
  }

  #[test]
  fn an_empty_domain_or_path_is_not_carried() {
    let cookie = cookie_from_value(&json!({
      "name": "a",
      "value": "b",
      "domain": "",
      "path": "",
    }))
    .expect("the wire shape parses");
    assert_eq!(cookie.domain(), None);
    assert_eq!(cookie.path(), None);
    assert_eq!(cookie.secure(), None);
    assert_eq!(cookie.http_only(), None);
  }

  #[test]
  fn a_session_cookie_reports_no_expiry() {
    let cookie = cookie_from_value(&json!({ "name": "a", "value": "b" })).unwrap();
    assert!(cookie.expires().is_none());
    assert!(cookie_payload(&cookie).get("expirationDate").is_none());
  }

  #[test]
  fn a_query_answers_exactly_once_even_when_it_collects_nothing() {
    let (tx, rx) = std::sync::mpsc::channel();
    drop(CookieQuery::new(tx));
    assert!(rx.recv().unwrap().unwrap().is_empty());
    assert!(rx.recv().is_err());
  }

  #[test]
  fn a_cookie_without_a_url_is_written_against_its_domain() {
    let cookie = Cookie::build(("a", "b"))
      .domain("example.test")
      .secure(true)
      .build();
    assert_eq!(cookie_url(&cookie, None), "https://example.test/");

    let fallback = url::Url::parse("tauri://localhost/index.html").unwrap();
    let session = Cookie::build(("a", "b")).build();
    assert_eq!(cookie_url(&session, Some(&fallback)), "tauri://localhost/");
  }
}
