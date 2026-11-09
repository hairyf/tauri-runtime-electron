//! Creation-time options for one webview that only exist while it is built.
//!
//! Tauri sets such options on the builder before the runtime reads them back,
//! but the official `tauri` crate cannot be extended here. A guard carries them
//! to [`crate::runtime`] through a label-keyed registry and deregisters on drop,
//! so a failed build never leaks options into the next webview with that label.

use std::{collections::HashMap, sync::{Mutex, OnceLock}};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AuthenticatedSession {
  Memory,
  Persistent(String),
}

#[derive(Default)]
struct Options {
  generation: u64,
  browser_host: bool,
  authenticated_web_host_session: Option<AuthenticatedSession>,
  authenticated_web_host: Option<String>,
  preload_scripts: Vec<String>,
}

fn options() -> &'static Mutex<HashMap<String, Options>> {
  static OPTIONS: OnceLock<Mutex<HashMap<String, Options>>> = OnceLock::new();
  OPTIONS.get_or_init(Default::default)
}

fn next_generation() -> u64 {
  static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
  NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// Options applied while the webview with this label is created.
///
/// Hold the guard across the builder call that creates the webview:
///
/// ```no_run
/// # use tauri_runtime_electron::WebviewSetup;
/// let setup = WebviewSetup::new("harness-1").preload_script("globalThis.ready = true");
/// # let _ = &setup;
/// ```
///
/// Registering a second setup for the same label replaces the first: the older
/// guard no longer owns the registration, so dropping it changes nothing.
#[must_use = "a WebviewSetup only applies while the guard is alive"]
pub struct WebviewSetup {
  label: String,
  generation: u64,
}

impl WebviewSetup {
  /// Register a setup for `label`, replacing any pending options it had.
  pub fn new(label: impl Into<String>) -> Self {
    let label = label.into();
    let generation = next_generation();
    options().lock().unwrap().insert(label.clone(), Options { generation, ..Default::default() });
    Self { label, generation }
  }

  /// Treat this webview as a trusted browser host rather than a normal page.
  pub fn browser_host(self) -> Self {
    let mut options = options().lock().unwrap();
    if let Some(pending) = options.get_mut(&self.label).filter(|pending| pending.generation == self.generation) {
      pending.browser_host = true;
    }
    self
  }

  pub fn authenticated_web_host(self, host: &crate::AuthenticatedWebHost) -> Self {
    let mut options = options().lock().unwrap();
    if let Some(pending) = options.get_mut(&self.label).filter(|pending| pending.generation == self.generation) {
      pending.authenticated_web_host_session = Some(AuthenticatedSession::Memory);
      pending.authenticated_web_host = Some(host.id().to_owned());
    }
    self
  }

  pub fn authenticated_web_host_session(self) -> Self {
    let mut options = options().lock().unwrap();
    if let Some(pending) = options.get_mut(&self.label).filter(|pending| pending.generation == self.generation) {
      pending.authenticated_web_host_session = Some(AuthenticatedSession::Memory);
    }
    self
  }

  pub fn authenticated_web_host_session_key(self, key: impl Into<String>) -> Self {
    let mut options = options().lock().unwrap();
    if let Some(pending) = options.get_mut(&self.label).filter(|pending| pending.generation == self.generation) {
      pending.authenticated_web_host_session = Some(AuthenticatedSession::Persistent(key.into()));
    }
    self
  }

  /// Append a source snippet to this webview's own preload.
  ///
  /// The snippet runs in the isolated preload world, where `require('electron')`
  /// is available, and it applies to this webview only.
  pub fn preload_script(self, source: impl Into<String>) -> Self {
    let mut options = options().lock().unwrap();
    if let Some(pending) = options.get_mut(&self.label).filter(|pending| pending.generation == self.generation) {
      pending.preload_scripts.push(source.into());
    }
    self
  }
}

impl Drop for WebviewSetup {
  fn drop(&mut self) {
    let mut options = options().lock().unwrap();
    if options.get(&self.label).is_some_and(|pending| pending.generation == self.generation) {
      options.remove(&self.label);
    }
  }
}

pub(crate) fn take_authenticated_web_host(label: &str) -> Option<String> {
  options().lock().unwrap().get_mut(label)?.authenticated_web_host.take()
}

pub(crate) fn take_authenticated_web_host_session(label: &str) -> Option<AuthenticatedSession> {
  options().lock().unwrap().get_mut(label)?.authenticated_web_host_session.take()
}

pub(crate) fn take_browser_host(label: &str) -> bool {
  options().lock().unwrap().get_mut(label).is_some_and(|pending| std::mem::take(&mut pending.browser_host))
}

pub(crate) fn take_preload_scripts(label: &str) -> Option<Vec<String>> {
  let scripts = std::mem::take(&mut options().lock().unwrap().get_mut(label)?.preload_scripts);
  (!scripts.is_empty()).then_some(scripts)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn setup_registration_crosses_threads_and_cleans_up() {
    let setup = WebviewSetup::new("setup-crosses-threads").preload_script("one");
    std::thread::spawn(|| {
      assert_eq!(take_preload_scripts("setup-crosses-threads").as_deref(), Some(&["one".to_owned()][..]));
      assert_eq!(take_preload_scripts("setup-crosses-threads"), None);
    }).join().unwrap();
    drop(setup);
    assert_eq!(take_preload_scripts("setup-crosses-threads"), None);
    assert!(!take_browser_host("setup-crosses-threads"));
  }

  #[test]
  fn authenticated_session_key_is_consumed_once() {
    let setup = WebviewSetup::new("setup-session-key").authenticated_web_host_session_key("profile-desktop-main");
    assert_eq!(
      take_authenticated_web_host_session("setup-session-key"),
      Some(AuthenticatedSession::Persistent("profile-desktop-main".into()))
    );
    assert_eq!(take_authenticated_web_host_session("setup-session-key"), None);
    drop(setup);
    assert_eq!(take_authenticated_web_host_session("setup-session-key"), None);
  }

  #[test]
  fn authenticated_session_setup_without_a_key_stays_in_memory() {
    let setup = WebviewSetup::new("setup-session-memory").authenticated_web_host_session();
    assert_eq!(take_authenticated_web_host_session("setup-session-memory"), Some(AuthenticatedSession::Memory));
    assert_eq!(take_authenticated_web_host_session("setup-session-memory"), None);
    drop(setup);
  }

  #[test]
  fn setup_collects_browser_host_and_preload_scripts() {
    let setup = WebviewSetup::new("setup-options").browser_host().preload_script("one").preload_script("two");
    assert!(take_browser_host("setup-options"));
    assert!(!take_browser_host("setup-options"));
    assert_eq!(take_preload_scripts("setup-options").as_deref(), Some(&["one".to_owned(), "two".to_owned()][..]));
    drop(setup);
    assert_eq!(take_preload_scripts("setup-options"), None);
  }

  #[test]
  fn dropping_a_replaced_setup_keeps_the_current_registration() {
    let previous = WebviewSetup::new("setup-replaces").browser_host();
    let current = WebviewSetup::new("setup-replaces").preload_script("kept");
    drop(previous);
    assert_eq!(take_preload_scripts("setup-replaces").as_deref(), Some(&["kept".to_owned()][..]));
    assert!(!take_browser_host("setup-replaces"), "the replacement dropped the older options");
    drop(current);
    assert_eq!(take_preload_scripts("setup-replaces"), None);
  }
}
