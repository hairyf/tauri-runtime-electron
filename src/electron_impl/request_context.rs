// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! The session a webview's storage lives in, the Electron equivalent of a CEF
//! `RequestContext`.
//!
//! The reference creates a per-webview `RequestContext`, registers Tauri's custom URI scheme
//! handler factories on it, and arranges for a continuation to fire once the underlying
//! Chromium `Profile` is fully created. That last part is the bulk of the reference file and
//! it exists for one reason: CEF initializes a request context asynchronously unless its
//! `cache_path` is the global one or empty, and creating a browser against a context that has
//! not finished `CreateProfileAsync` fails `VerifyBrowserContext()` and returns null. The
//! whole `on_request_context_initialized` dance, the nestable-task pumping and the
//! `wait_for_deferred_init` spin loop are consequences of that.
//!
//! Electron's equivalent is `session.fromPartition`, and it is created **synchronously**. The
//! deferred-init protocol is therefore unnecessary, and what is left of this module is the
//! part that decides *which* session a webview gets: the same path and hash rules the
//! reference uses, so a profile keeps its identity across runtimes.
//!
//! # Preferences and proxies
//!
//! `apply_app_webview_preferences` and the content-setting table live in
//! [`preferences`](crate::electron_impl::preferences), because Electron applies them
//! app-wide rather than per context. The per-webview proxy is reported rather than applied:
//! Electron's `session.setProxy` has no counterpart in the bridge, and a webview that asked
//! for a proxy and silently did not get one would send its traffic straight out.

use std::{
  fs::create_dir_all,
  path::{Component, Path, PathBuf},
  sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
  },
  time::Duration,
};

use base64::Engine;
use sha2::{Digest, Sha256};
use tauri_runtime::webview::WebviewAttributes;
use tauri_utils::Theme;

use crate::electron_impl::ipc::Ipc;

/// Applies the application theme to a webview's context.
///
/// Emulated, and only app-wide: Electron has no per-request-context Chrome colour scheme.
/// The closest mechanism is `nativeTheme.themeSource`, which is what `app.theme` writes and
/// what [`crate::runtime`]'s own theme handling uses, so a per-webview request is answered
/// with the same app-wide call rather than being dropped.
pub(crate) fn apply_theme_scheme(ipc: &Ipc, theme: Option<Theme>) {
  let name = match theme {
    Some(Theme::Dark) => "dark",
    Some(Theme::Light) => "light",
    // `Theme` is `#[non_exhaustive]`: a variant a later release adds has no Electron spelling
    // of its own, and `system` is the answer that defers to the platform rather than guessing.
    Some(_) | None => "system",
  };
  if let Err(error) = ipc.call(
    "app.theme",
    serde_json::json!({ "theme": name }),
    crate::runtime::IPC_TIMEOUT,
  ) {
    log::warn!("failed to apply the {name} theme: {error}");
  }
}

/// Resolves a per-webview storage path under the application's cache root.
///
/// CEF requires `RequestContextSettings.cache_path` to be either empty (which puts the
/// context in incognito mode) or an absolute path that is equal to, or a child directory of,
/// `Settings.root_cache_path`. Any value outside of that root makes
/// `request_context_create_context` fail. Electron has no such constraint, but the same rule
/// is kept because it is what gives a webview a *stable identity*: distinct
/// `data_directory` values produce distinct profiles, and the same value maps to the same
/// on-disk profile across runs — and across runtimes, which is what lets an application
/// migrate from the CEF backend without losing its storage.
///
/// * a path that already lives under the cache root is used directly, so callers that opt in
///   to a path under the app cache get the exact location they asked for;
/// * a relative path without parent (`..`) components is joined onto the root (typical short
///   labels); and
/// * anything else derives a stable direct child folder under `<root>/Profile-<hash>`.
pub(crate) fn resolve_request_context_cache_path(
  global_cache_path: &Path,
  data_directory: &Path,
) -> PathBuf {
  if data_directory.is_absolute() {
    if data_directory.starts_with(global_cache_path) {
      return data_directory.to_path_buf();
    } else {
      log::warn!(
        "data directory is not a child of the global cache path, we will derive a profile hash from it"
      );
    }
  } else if !data_directory
    .components()
    .any(|component| matches!(component, Component::ParentDir))
  {
    return global_cache_path.join(data_directory);
  } else {
    log::warn!(
      "data directory is a relative path with parent components, we will derive a profile hash from it"
    );
  }

  let mut hasher = Sha256::new();
  hasher.update(data_directory.as_os_str().as_encoded_bytes());
  let hash = hasher.finalize();
  let suffix = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&hash[..16]);
  let path = global_cache_path.join(format!("Profile-{suffix}"));
  log::info!(
    "derived profile hash from data directory: {suffix}, cache path: {}",
    path.display()
  );
  path
}

/// The session name a [`WebviewAttributes::data_store_identifier`] maps to, spelled as the
/// UUID WKWebView would show for it.
fn data_store_directory_name(identifier: &[u8; 16]) -> String {
  let hex: String = identifier
    .iter()
    .map(|byte| format!("{byte:02x}"))
    .collect();
  format!(
    "DataStore-{}-{}-{}-{}-{}",
    &hex[0..8],
    &hex[8..12],
    &hex[12..16],
    &hex[16..20],
    &hex[20..32]
  )
}

/// Continuation invoked once the session of a webview exists.
///
/// Receives the partition name, or [`None`] for a webview that asked to be incognito. The
/// reference passes a fresh `RequestContext` handle; the partition name is the equivalent
/// handle here, because it is what every later `session.fromPartition` call resolves to the
/// same store.
pub(crate) type RequestContextInitContinuation = Box<dyn FnOnce(Option<String>) + 'static>;

/// Wraps a continuation so that it always flips a shared completion flag when it exits,
/// regardless of how it exits (normal return, early `return`, or panic).
///
/// Returns the completion flag plus the wrapped continuation.
///
/// Kept even though Electron needs no deferred initialization: the guard is what makes
/// [`wait_for_deferred_init`] safe to call on a path that may never run the continuation, and
/// a caller that keeps the reference's structure gets the same guarantee.
pub(crate) fn deferred_init_continuation<F>(
  work: F,
) -> (Arc<AtomicBool>, RequestContextInitContinuation)
where
  F: FnOnce(Option<String>) + 'static,
{
  struct Guard(Arc<AtomicBool>);
  impl Drop for Guard {
    fn drop(&mut self) {
      self.0.store(true, Ordering::SeqCst);
    }
  }

  let flag = Arc::new(AtomicBool::new(false));
  let guard = Guard(flag.clone());
  let wrapped: RequestContextInitContinuation = Box::new(move |partition| {
    let _guard = guard;
    work(partition);
  });
  (flag, wrapped)
}

/// Blocks the calling thread until `flag` is `true`.
///
/// The reference has two branches here because it may run on CEF's UI thread, where it has to
/// pump the message loop itself — with nestable tasks enabled — for a posted init task to
/// ever run. There is no such thread in Electron: a session is created synchronously, so the
/// continuation has already run by the time anyone waits on the flag and this returns
/// immediately. The sleep loop is kept for the one case that is still possible, a
/// continuation that never runs at all, so a caller blocks for a millisecond at a time rather
/// than spinning a core.
pub(crate) fn wait_for_deferred_init(flag: &Arc<AtomicBool>) {
  while !flag.load(Ordering::SeqCst) {
    std::thread::sleep(Duration::from_millis(1));
  }
}

/// The session a webview's storage lives in.
///
/// `None` means the webview asked to be incognito, which Electron spells as a partition
/// without the `persist:` prefix — an in-memory store that is discarded with the session.
/// `Some(partition)` names a persistent store, and the name is what makes two webviews share
/// or not share their cookies: the reference's request-context identity, expressed the way
/// Electron names one.
///
/// The bridge's `webview.create` takes `incognito` and `dataDirectory` rather than a
/// partition, and derives the same name; this function is the rule that derivation follows.
pub(crate) fn request_context_from_webview_attributes(
  global_cache_path: &Path,
  webview_attributes: &WebviewAttributes,
) -> Option<String> {
  if webview_attributes.incognito {
    return None;
  }

  if let Some(data_directory) = &webview_attributes.data_directory {
    let cache_path = resolve_request_context_cache_path(global_cache_path, data_directory);
    if let Err(error) = create_dir_all(&cache_path) {
      log::error!(
        "failed to create the session directory {}: {error}",
        cache_path.display()
      );
    }
    return Some(persistent_partition(&cache_path));
  }

  if let Some(identifier) = &webview_attributes.data_store_identifier {
    // WKWebView's `WKWebsiteDataStore(forIdentifier:)` names a persistent store by UUID.
    // Electron's separate store is a separate partition, so the identifier names one the way
    // a relative `data_directory` does. `data_directory` wins when both are set: an
    // application sets both to cover the platforms each one is for, and either gives it the
    // same isolation here.
    return Some(format!("persist:{}", data_store_directory_name(identifier)));
  }

  Some("persist:default".to_owned())
}

/// The partition name of a resolved storage path.
///
/// The directory's own name, not the whole path: Electron derives a partition's directory
/// from its name, so a name that is a path would nest directories under `Partitions/` and
/// could not be created on Windows at all. The name is what carries the identity — a
/// `Profile-<hash>` for a derived path, the caller's own label for a relative one.
fn persistent_partition(cache_path: &Path) -> String {
  match cache_path.file_name().map(|name| name.to_string_lossy()) {
    Some(name) if !name.is_empty() => format!("persist:{name}"),
    _ => "persist:default".to_owned(),
  }
}

/// The Electron `proxyRules` string a proxy URL names.
///
/// The scheme mapping is the reference's: `socks5h` is `socks5` to Chromium, `socks4a` is
/// `socks4`, and anything unrecognised is an HTTP proxy. `None` when the URL has no host,
/// which is the case the reference ignores with a warning.
fn proxy_rules(proxy_url: &url::Url) -> Option<String> {
  let scheme = match proxy_url.scheme() {
    "socks5" | "socks5h" => "socks5",
    "socks4" | "socks4a" => "socks4",
    "https" => "https",
    _ => "http",
  };
  let host = proxy_url.host_str()?;
  Some(match proxy_url.port_or_known_default() {
    Some(port) => format!("{scheme}://{host}:{port}"),
    None => format!("{scheme}://{host}"),
  })
}

/// Applies a fixed-server proxy to a webview's session.
///
/// Emulated in principle — Electron's `session.setProxy({ mode: 'fixed_servers', proxyRules })`
/// has exactly the reference's semantics — but not reachable: the bridge exposes no
/// `setProxy` method, so a webview that asked for a proxy would silently send its traffic
/// straight out. That is the one failure the reference treats as a warning rather than a
/// debug line, and it is a warning here too.
pub(crate) fn apply_proxy(webview_id: u32, proxy_url: &url::Url) {
  match proxy_rules(proxy_url) {
    Some(rules) => log::warn!(
      "webview {webview_id} asked for the proxy {rules}, but the Electron bridge has no \
       session.setProxy method; its traffic is not proxied"
    ),
    None => log::warn!("ignoring proxy URL without a host: {proxy_url}"),
  }
  crate::unsupported::report("request_context::apply_proxy");
}

#[cfg(test)]
mod data_store_tests {
  use super::*;

  #[test]
  fn a_data_store_identifier_names_a_uuid_shaped_directory() {
    let identifier = [
      0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd,
      0xef,
    ];
    assert_eq!(
      data_store_directory_name(&identifier),
      "DataStore-12345678-9abc-def0-0123-456789abcdef"
    );
  }

  #[test]
  fn distinct_identifiers_name_distinct_directories() {
    assert_ne!(
      data_store_directory_name(&[0; 16]),
      data_store_directory_name(&[1; 16])
    );
  }

  #[test]
  fn a_relative_label_becomes_a_child_of_the_cache_root() {
    let root = Path::new("/cache");
    assert_eq!(
      resolve_request_context_cache_path(root, Path::new("app-a")),
      Path::new("/cache/app-a")
    );
    assert_eq!(
      persistent_partition(&resolve_request_context_cache_path(
        root,
        Path::new("app-a")
      )),
      "persist:app-a"
    );
  }

  #[test]
  fn a_path_outside_the_cache_root_derives_a_stable_profile_name() {
    // Both paths must be absolute on the host, or the "outside the root" branch is
    // never reached: `/elsewhere` is relative on Windows.
    let root = std::env::temp_dir().join("cache");
    let outside = std::env::temp_dir().join("elsewhere").join("data");
    let first = resolve_request_context_cache_path(&root, &outside);
    let second = resolve_request_context_cache_path(&root, &outside);
    assert_eq!(first, second);
    assert!(first.starts_with(&root));
    let name = first.file_name().unwrap().to_string_lossy().into_owned();
    assert!(name.starts_with("Profile-"));
  }

  #[test]
  fn a_path_inside_the_cache_root_is_used_as_asked() {
    let root = Path::new("/cache");
    assert_eq!(
      resolve_request_context_cache_path(root, Path::new("/cache/mine")),
      Path::new("/cache/mine")
    );
  }

  #[test]
  fn the_proxy_scheme_mapping_is_the_references() {
    for (url, expected) in [
      ("socks5://127.0.0.1:1080", "socks5://127.0.0.1:1080"),
      ("socks5h://127.0.0.1:1080", "socks5://127.0.0.1:1080"),
      ("socks4a://127.0.0.1:1080", "socks4://127.0.0.1:1080"),
      ("https://proxy.test", "https://proxy.test:443"),
      ("http://proxy.test:8080", "http://proxy.test:8080"),
      ("ftp://proxy.test:21", "http://proxy.test:21"),
    ] {
      let url = url::Url::parse(url).unwrap();
      assert_eq!(proxy_rules(&url).as_deref(), Some(expected));
    }
  }

  #[test]
  fn a_proxy_url_without_a_host_names_no_rules() {
    let url = url::Url::parse("socks5:///nohost").unwrap();
    assert_eq!(proxy_rules(&url), None);
  }
}
