// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! Native protocol observations of Electron's JavaScript dialog UI.
//! No custom dialog handler, callback, message text, or prompt value is retained.
//!
//! Electron answers `content::JavaScriptDialogManager` with a no-op
//! `RunJavaScriptDialog`, so no dialog ever opens and the DevTools `Page` domain
//! never reports `Page.javascriptDialogOpening`. The only dialog-shaped signal the
//! main process does surface is `will-prevent-unload`, which fires when a page's
//! `beforeunload` handler refuses to let the document go. That is enough to
//! observe a `BeforeUnload` dialog's *existence*; it says nothing about the other
//! three kinds, so those report `known: false` and the runtime never claims to
//! have seen a dialog it cannot see.

use crate::{FrameNavigationState, NativeDocumentToken};
use std::sync::{Arc, Mutex};

/// The kinds of native JavaScript dialog a page can raise.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum NativeDialogKind {
  /// `alert()`.
  Alert,
  /// `confirm()`.
  Confirm,
  /// `prompt()`.
  Prompt,
  /// The dialog a `beforeunload` handler raises.
  BeforeUnload,
}

/// Opaque lifetime of one native JavaScript dialog notification. Compare with
/// the current UI-thread snapshot before submitting any dialog action.
#[derive(Clone)]
pub struct NativeDialogToken(Arc<()>);
impl PartialEq for NativeDialogToken {
  fn eq(&self, other: &Self) -> bool {
    Arc::ptr_eq(&self.0, &other.0)
  }
}
impl Eq for NativeDialogToken {}
impl std::fmt::Debug for NativeDialogToken {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("NativeDialogToken").finish_non_exhaustive()
  }
}

/// What was observed about one dialog.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct NativeDialogSnapshot {
  /// Identifies this dialog, so a stale action cannot apply to a later one.
  pub token: NativeDialogToken,
  /// Which dialog the page raised.
  pub kind: NativeDialogKind,
  /// Whether a non-DevTools dialog handler exists.
  ///
  /// This does not establish native visibility, the exact button label, or the
  /// current prompt value.
  pub has_browser_handler: bool,
}

/// The dialog state of one document.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct NativeDialogObservation {
  /// A dialog event was observed for this document. False never means absent.
  pub known: bool,
  /// The dialog currently open, when one was observed.
  pub dialog: Option<NativeDialogSnapshot>,
}

#[derive(Default)]
struct ObservedDialog {
  document: Option<NativeDocumentToken>,
  observation: NativeDialogObservation,
}

/// The dialog state of one webview, keyed to the document it was observed on.
#[derive(Clone)]
pub(crate) struct DialogState {
  browser: FrameNavigationState,
  state: Arc<Mutex<ObservedDialog>>,
}

impl DialogState {
  pub(crate) fn new(browser: FrameNavigationState) -> Self {
    Self {
      browser,
      state: Arc::default(),
    }
  }

  /// Whether `id` is still the browser this state belongs to.
  pub(crate) fn accepts_browser(&self, id: i32) -> bool {
    self.browser.has_browser_id(id)
  }

  /// The dialog state of `document`, empty when it belongs to another document.
  pub(crate) fn snapshot(&self, document: Option<&NativeDocumentToken>) -> NativeDialogObservation {
    self
      .state
      .lock()
      .ok()
      .filter(|state| state.document.as_ref() == document)
      .map(|state| state.observation.clone())
      .unwrap_or_default()
  }

  /// Records one dialog-shaped event.
  ///
  /// `will-prevent-unload` is the only one Electron surfaces; a `Page.*` method
  /// name is accepted so a future DevTools transport can feed the same state
  /// without changing this signature.
  pub(crate) fn on_event(&self, method: &str, params: &[u8]) {
    let observation = match method {
      "Page.javascriptDialogOpening" => {
        #[derive(serde::Deserialize)]
        struct Opening {
          #[serde(rename = "type")]
          kind: NativeDialogKind,
          #[serde(rename = "hasBrowserHandler")]
          has_browser_handler: bool,
        }
        // Dialog payloads contain arbitrary page text. Bound parsing and retain
        // only the native kind/handler facts; malformed input revokes old refs.
        let opening = (params.len() <= 262_144)
          .then(|| serde_json::from_slice::<Opening>(params).ok())
          .flatten();
        opening
          .map(|opening| NativeDialogObservation {
            known: true,
            dialog: Some(NativeDialogSnapshot {
              token: NativeDialogToken(Arc::new(())),
              kind: opening.kind,
              has_browser_handler: opening.has_browser_handler,
            }),
          })
          .unwrap_or_default()
      }
      "will-prevent-unload" => NativeDialogObservation {
        known: true,
        dialog: Some(NativeDialogSnapshot {
          token: NativeDialogToken(Arc::new(())),
          kind: NativeDialogKind::BeforeUnload,
          // Electron always supplies its own unload confirmation, so a handler
          // exists by construction.
          has_browser_handler: true,
        }),
      },
      "Page.javascriptDialogClosed" => NativeDialogObservation {
        known: true,
        dialog: None,
      },
      _ => return,
    };
    let document = self.browser.document();
    if let Ok(mut state) = self.state.lock() {
      *state = ObservedDialog {
        document,
        observation,
      };
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn ready() -> FrameNavigationState {
    let browser = FrameNavigationState::new();
    for kind in [
      crate::FrameEventKind::Created,
      crate::FrameEventKind::Attached,
      crate::FrameEventKind::MainFrameChanged,
      crate::FrameEventKind::LoadingStateChanged { is_loading: false },
    ] {
      browser.on_frame_event(&crate::FrameEvent {
        browser_id: 1,
        frame_id: "main".into(),
        is_main: true,
        kind,
      });
    }
    browser
  }

  const CONFIRM: &[u8] = br#"{"type":"confirm","hasBrowserHandler":true,"message":"non-secret fixture","defaultPrompt":"omitted fixture"}"#;

  #[test]
  fn exact_dialog_lifetimes_rotate_on_reopen_and_do_not_cross_browsers() {
    let browser = ready();
    let state = DialogState::new(browser.clone());
    let document = browser.document().unwrap();
    assert!(!state.snapshot(Some(&document)).known);
    state.on_event("Page.javascriptDialogOpening", CONFIRM);
    let first = state.snapshot(Some(&document)).dialog.unwrap();
    assert_eq!(first.kind, NativeDialogKind::Confirm);
    assert!(first.has_browser_handler);
    state.on_event("Page.javascriptDialogClosed", b"unread prompt value");
    assert!(state.snapshot(Some(&document)).known);
    assert!(state.snapshot(Some(&document)).dialog.is_none());
    state.on_event("Page.javascriptDialogOpening", CONFIRM);
    assert_ne!(
      state.snapshot(Some(&document)).dialog.unwrap().token,
      first.token
    );
    let other = DialogState::new(ready());
    other.on_event("Page.javascriptDialogOpening", CONFIRM);
    assert_ne!(
      other
        .state
        .lock()
        .unwrap()
        .observation
        .dialog
        .as_ref()
        .unwrap()
        .token,
      first.token
    );
    assert!(!state.accepts_browser(2));
  }

  #[test]
  fn navigation_unknown_protocol_and_missing_handler_facts_fail_closed() {
    let browser = ready();
    let state = DialogState::new(browser.clone());
    let document = browser.document().unwrap();
    state.on_event("Page.javascriptDialogOpening", CONFIRM);
    browser.close();
    assert!(!state.snapshot(browser.document().as_ref()).known);
    state.on_event("Page.javascriptDialogOpening", br#"{"type":"confirm"}"#);
    assert!(!state.snapshot(None).known);
    state.on_event(
      "Page.javascriptDialogOpening",
      br#"{"type":"unknown","hasBrowserHandler":true}"#,
    );
    assert!(state.snapshot(None).dialog.is_none());
    assert!(state.snapshot(Some(&document)).dialog.is_none());
  }

  #[test]
  fn unload_prevention_is_the_one_dialog_electron_surfaces() {
    let browser = ready();
    let state = DialogState::new(browser.clone());
    let document = browser.document().unwrap();
    state.on_event("will-prevent-unload", b"");
    let snapshot = state.snapshot(Some(&document)).dialog.unwrap();
    assert_eq!(snapshot.kind, NativeDialogKind::BeforeUnload);
    assert!(snapshot.has_browser_handler);
  }
}
