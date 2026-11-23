// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! Chromium preferences and content settings.
//!
//! The reference's module exists because the CEF runtime creates Chrome-style browsers, so
//! each webview is backed by a real Chrome profile and inherits the browser-shaped behaviour
//! that comes with it: a "Save password?" bubble on any form submit, address and credit-card
//! save bubbles, a "Translate this page?" bubble, and a handful of background requests to
//! Google. All of that is turned off per request context right after the profile finishes
//! initializing, through two stores with two lifetimes — a *profile* preference on the
//! request context and a *global* preference in Chromium's local state.
//!
//! # None of that exists in Electron
//!
//! Electron does not create Chrome-style profiles. There is no password manager, no autofill,
//! no translate bubble, no alternate-error-page service, no omnibox suggest and no Privacy
//! Sandbox consent flow to keep off, so there is nothing to write and the preference tables
//! below are kept as the record of what the reference disables and why. The same goes for
//! `session.restore_on_startup`: Electron never restores a Chrome session into Chrome
//! windows. `devtools.availability` and `devtools.remote_debugging.allowed` are replaced by
//! the per-webview `devtools` flag on `webview.create` and by not appending
//! `--remote-debugging-port` at all.
//!
//! # What is live
//!
//! [`apply_default_content_settings`] is: Electron expresses the same application-wide
//! policy through `session.setPermissionRequestHandler` and `session.setPermissionCheckHandler`
//! per partition, which is what the bridge installs from the table this module sends. A page
//! cannot ask for a permission whose default is `Block`, and one whose default is `Allow`
//! never prompts — the reference's rule, in Electron's mechanism.
//!
//! # Safe Browsing
//!
//! [`PREFERENCES`] deliberately leaves `safebrowsing.enabled` alone, for the reference's
//! reason: a webview routinely loads content the developer does not control — OAuth and SSO
//! flows, embedded third-party pages, iframes, and the popups this runtime supports — so it
//! is not the closed world that would make the protection pointless.

use serde_json::{Map, Value, json};

use crate::{
  electron_impl::ipc::Ipc,
  runtime::{ContentSettingType, ContentSettingValue, IPC_TIMEOUT},
};

/// The `error` out-parameter every `set_preference` call has to pass.
///
/// Absent: there is no CEF preference service, so there is no out-parameter to build. The
/// function is kept so a caller ported from the reference still compiles, and answers the
/// unit value the Electron side has instead of a `CefString`.
#[allow(dead_code)]
pub(crate) fn set_preference_error_slot() {}

/// `devtools.availability`, the profile preference `DevToolsWindow::AllowDevToolsFor`
/// consults on every path that opens DevTools.
///
/// Absent as a preference: the Electron equivalent is the per-webview `devtools` boolean on
/// `webview.create`, which the bridge reads as `devtoolsAllowed`. The constant is kept
/// because the *reason* it exists is worth keeping — CEF gates `SendDevToolsMessage` on the
/// disallowing value and refuses it silently, so it can never be used as a hardening switch
/// by a runtime that drives its own startup over the DevTools protocol.
#[allow(dead_code)]
pub(crate) const DEVTOOLS_AVAILABILITY: &str = "devtools.availability";

/// The default `devtools.availability` value, matching Chromium's
/// `DeveloperToolsAvailability::kDisallowedForForceInstalledExtensions` — DevTools are
/// available everywhere but on a force-installed extension, which an Electron application
/// has none of. `2` is the value that forbids them outright.
#[allow(dead_code)]
pub(crate) const DEVTOOLS_ALLOWED: i64 = 0;

/// `devtools.remote_debugging.allowed`, the local-state preference
/// `RemoteDebuggingServer::GetInstance` consults before it will start a server for
/// `--remote-debugging-port` or `--remote-debugging-pipe`.
///
/// Absent: Electron's equivalent is not appending the switch, which the runtime decides from
/// [`RemoteDebugging`](crate::RemoteDebugging) before the process is spawned. There is no
/// preference to pin.
#[allow(dead_code)]
pub(crate) const REMOTE_DEBUGGING_ALLOWED: &str = "devtools.remote_debugging.allowed";

/// Chromium profile preferences forced off for every webview, with the reason each one is
/// unwanted in an application webview:
///
/// * `credentials_enable_service` - Chrome offers to save credentials typed
///   into any form; an app's login form is not the browser's business.
/// * `profile.password_manager_leak_detection` - on by default in Chromium, it
///   sends a hashed prefix of credentials typed into any form to Google to check
///   them against known breaches.
/// * `autofill.profile_enabled` / `autofill.credit_card_enabled` - the same
///   deal for postal addresses and payment cards, which additionally sync into
///   the user's Google account.
/// * `translate.enabled` - the translate bubble both covers app UI and ships
///   page text off to Google's translation service to decide whether to offer.
/// * `alternate_error_pages.enabled` - on a failed navigation Chrome sends the
///   URL that failed to Google to fetch suggestions for it.
/// * `search.suggest_enabled` - streams typed input to the profile's default
///   search engine; an app has no omnibox for this to serve.
/// * `privacy_sandbox.m1.*` - the Topics, Protected Audience and attribution
///   reporting APIs. Chrome only turns these on after its own consent flow, which
///   never runs in an embedded browser, so today they are already off; pinning
///   them means a future Chromium that flips the default cannot quietly enrol an
///   application's users in interest-based advertising.
///
/// `safebrowsing.enabled` is deliberately absent - see the module docs.
///
/// Absent as a *write*: Electron has none of these services. The table is kept because it is
/// the specification of what an application webview must not inherit, and because a future
/// Electron that grows one of them has the answer here.
const PREFERENCES: &[(&str, bool)] = &[
  ("credentials_enable_service", false),
  ("profile.password_manager_leak_detection", false),
  ("autofill.profile_enabled", false),
  ("autofill.credit_card_enabled", false),
  ("translate.enabled", false),
  ("alternate_error_pages.enabled", false),
  ("search.suggest_enabled", false),
  ("privacy_sandbox.m1.topics_enabled", false),
  ("privacy_sandbox.m1.fledge_enabled", false),
  ("privacy_sandbox.m1.ad_measurement_enabled", false),
];

/// `session.restore_on_startup`, the profile preference Chrome consults when the first
/// browser of a profile opens, and the value the runtime pins it to.
///
/// `1` ("continue where you left off") makes `SessionService::RestoreIfNecessary` reopen
/// every tab the profile's last session had — after a crash or a kill too — in Chrome browser
/// windows of its own. Those tabs are the application's own webviews, so each launch would
/// bring back hidden copies of the previous run's windows that the runtime does not own,
/// never closes, and that keep the process alive once the application's windows are gone.
/// `5` is Chrome's "open the New Tab page", which restores nothing.
///
/// Absent: Electron never restores a session into windows of its own.
const SESSION_RESTORE_ON_STARTUP: (&str, i64) = ("session.restore_on_startup", 5);

/// Applies [`PREFERENCES`], then `overrides`, to a webview.
///
/// Absent. A CEF preference can only be written once the request context's underlying
/// `Profile` exists, which is why the reference defers it to
/// `on_request_context_initialized`; Electron has no profile store to write, and the
/// `preferences` table the application configured travels to the bridge once at startup
/// through `app.configure`. The names are logged at debug so an application that configured
/// one can see it was not applied.
pub(crate) fn apply_app_webview_preferences(webview_id: u32, overrides: &[(String, Value)]) {
  if overrides.is_empty() {
    return;
  }
  log::debug!(
    "webview {webview_id} configured {} profile preference(s) that Electron has no store for: {}",
    overrides.len(),
    overrides
      .iter()
      .map(|(name, _)| name.as_str())
      .collect::<Vec<_>>()
      .join(", ")
  );
  crate::unsupported::report("preferences::apply_app_webview_preferences");
}

/// Applies the application's default content settings.
///
/// A null URL pair is how CEF spells "the default for every origin", which is the
/// application-wide policy an embedder wants. Electron spells it as the permission handlers
/// the bridge installs per partition: a `Block` default makes the handler refuse without
/// asking, an `Allow` default makes it grant without asking, and `Ask` leaves the decision to
/// the application's permission handler.
pub(crate) fn apply_default_content_settings(
  ipc: &Ipc,
  settings: &[(ContentSettingType, ContentSettingValue)],
) {
  if settings.is_empty() {
    return;
  }

  let content = settings
    .iter()
    .map(|(kind, value)| {
      (
        kind.permission().to_owned(),
        json!({ "answer": value.answer() }),
      )
    })
    .collect::<Map<_, _>>();

  if let Err(error) = ipc.call(
    "app.configure",
    json!({ "contentSettings": content }),
    IPC_TIMEOUT,
  ) {
    log::warn!("failed to apply the default content settings: {error}");
  }
}

/// Writes one preference on any store that has a preference manager.
///
/// Absent: Electron has no preference-manager API, so there is no store to write and no
/// answer to give. The function keeps the reference's `#[must_use]` contract — a caller for
/// whom the write is the whole point, the proxy being the standing case, must see that it
/// did not happen — and always answers `false`.
#[must_use]
pub(crate) fn set_preference(webview_id: u32, name: &str, _value: &Value) -> bool {
  log::debug!(
    "webview {webview_id} asked for the {name} preference, which Electron has no store for"
  );
  crate::unsupported::report("preferences::set_preference");
  false
}

/// Applies the preferences that live in the browser's local state rather than in a profile.
///
/// Absent: there is no local-state store. The runtime's own global preferences travel to the
/// bridge once at startup through `app.configure`, which is the only place they can be
/// applied, so an override list arriving here is reported rather than silently dropped.
pub(crate) fn apply_global_preferences(overrides: &[(String, Value)]) {
  if overrides.is_empty() {
    return;
  }
  log::debug!(
    "{} global preference(s) have no Electron store: {}",
    overrides.len(),
    overrides
      .iter()
      .map(|(name, _)| name.as_str())
      .collect::<Vec<_>>()
      .join(", ")
  );
  crate::unsupported::report("preferences::apply_global_preferences");
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn the_disabled_preferences_are_all_disabled() {
    // Every entry exists to turn something off; an entry set to `true` would be a
    // default this module has no business asserting.
    for (name, enabled) in PREFERENCES {
      assert!(!enabled, "{name} is listed as a preference forced off");
    }
  }

  #[test]
  fn safe_browsing_is_not_disabled_by_default() {
    assert!(
      !PREFERENCES
        .iter()
        .any(|(name, _)| *name == "safebrowsing.enabled"),
      "Safe Browsing stays on unless the application asks otherwise; see the module docs"
    );
  }

  #[test]
  fn the_privacy_sandbox_apis_are_pinned_off() {
    for api in [
      "privacy_sandbox.m1.topics_enabled",
      "privacy_sandbox.m1.fledge_enabled",
      "privacy_sandbox.m1.ad_measurement_enabled",
    ] {
      assert!(PREFERENCES.iter().any(|(name, _)| *name == api));
    }
  }

  #[test]
  fn session_restore_opens_the_new_tab_page() {
    // `1` would reopen the previous session's webviews as Chrome windows of their own.
    assert_eq!(SESSION_RESTORE_ON_STARTUP.1, 5);
  }
}
