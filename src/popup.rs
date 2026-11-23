// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! Electron-owned popup lifetimes. Electron keeps its native window and opener
//! semantics; the runtime owns admission, observation and teardown, and answers
//! the blocking `setWindowOpenHandler` request with `webview.newWindowResponse`.
//!
//! The reference observes popups CEF creates on its own. Electron instead blocks
//! the opener inside `setWindowOpenHandler` until the runtime answers, so the
//! same bookkeeping runs on both sides of one request: [`PopupFamily::reserve`]
//! decides before anything is created, the runtime creates the window with
//! `window.create { newWindowRequest }`, and [`PopupFamily::created`] adopts it.
//! A request that is denied, or whose window never appears, is [`abort`]ed.
//!
//! Nothing in the crate drives the family yet: the Electron client handlers
//! answer the bridge directly, so the module is the runtime's popup registry and
//! the place the invariants below are proven.

#![allow(dead_code)]

use std::sync::{
  Arc, Mutex,
  atomic::{AtomicBool, Ordering},
};

use crate::{
  FrameNavigationState, NativeWindowToken,
  webview::{Webview, WebviewSnapshot},
};

/// Bounds on the live popups of one family, on its pending reservations, and on
/// the popup windows it tracks.
const MAX_POPUPS: usize = 128;

/// A reservation for a popup the runtime is about to create.
///
/// `identity` distinguishes two requests that carry the same `popup_id`, so a
/// reused request number can never adopt another request's window.
#[derive(Clone)]
pub(crate) struct PopupRequest {
  pub(crate) opener: FrameNavigationState,
  /// The opener's own webContents id, re-checked when the popup appears.
  opener_browser_id: i32,
  popup_id: u64,
  identity: Arc<()>,
}

impl PopupRequest {
  pub(crate) fn is_same(&self, other: &Self) -> bool {
    Arc::ptr_eq(&self.identity, &other.identity)
  }
}

/// One live popup: its webContents, the window that hosts it, its own navigation
/// state, the opener's state, a closing flag and its dialog state.
struct Popup {
  browser_id: i32,
  window: NativeWindowToken,
  state: FrameNavigationState,
  opener: FrameNavigationState,
  closing: AtomicBool,
  dialogs: crate::dialog::DialogState,
}

/// All popups of one root webview, plus reservations and the windows they live in.
pub(crate) struct PopupFamily {
  root: FrameNavigationState,
  closing: AtomicBool,
  popups: Mutex<Vec<Arc<Popup>>>,
  pending: Mutex<Vec<PopupRequest>>,
  windows: Mutex<Vec<NativeWindowToken>>,
}

impl PopupFamily {
  pub(crate) fn new(root: FrameNavigationState) -> Self {
    Self {
      root,
      closing: AtomicBool::new(false),
      popups: Mutex::default(),
      pending: Mutex::default(),
      windows: Mutex::default(),
    }
  }

  pub(crate) fn admits(&self, opener: &FrameNavigationState, browser_id: i32) -> bool {
    if self.closing.load(Ordering::Acquire) || !opener.has_browser_id(browser_id) {
      return false;
    }
    let Ok(popups) = self.popups.lock() else {
      return false;
    };
    popups.len() < MAX_POPUPS
      && (self.root.is_same_browser(opener)
        || popups.iter().any(|popup| {
          !popup.closing.load(Ordering::Acquire) && popup.state.is_same_browser(opener)
        }))
  }

  pub(crate) fn reserve(
    &self,
    opener: &FrameNavigationState,
    browser_id: i32,
    popup_id: u64,
  ) -> Option<PopupRequest> {
    if !self.admits(opener, browser_id) {
      return None;
    }
    let mut pending = self.pending.lock().ok()?;
    if pending.len() + self.popups.lock().ok()?.len() >= MAX_POPUPS
      || pending
        .iter()
        .any(|request| request.popup_id == popup_id && request.opener.is_same_browser(opener))
    {
      return None;
    }
    let request = PopupRequest {
      opener: opener.clone(),
      opener_browser_id: browser_id,
      popup_id,
      identity: Arc::new(()),
    };
    pending.push(request.clone());
    Some(request)
  }

  pub(crate) fn abort(&self, opener: &FrameNavigationState, popup_id: u64) -> Option<PopupRequest> {
    let mut pending = self.pending.lock().ok()?;
    let index = pending
      .iter()
      .position(|request| request.popup_id == popup_id && request.opener.is_same_browser(opener))?;
    Some(pending.remove(index))
  }

  /// Adopts the popup whose reservation `request` names.
  ///
  /// Returns whether the popup was adopted. A rejected popup is left to the
  /// caller to close: the reference closes CEF's browser here, while the mirror
  /// owns no browser and only revokes the state, so the caller issues
  /// `webview.close` for the webContents it created.
  pub(crate) fn created(
    &self,
    browser_id: i32,
    window: NativeWindowToken,
    request: &PopupRequest,
    state: &FrameNavigationState,
  ) -> bool {
    let reserved = self
      .pending
      .lock()
      .map(|mut pending| {
        let found = pending.iter().any(|entry| entry.is_same(request));
        pending.retain(|entry| !entry.is_same(request));
        found
      })
      .unwrap_or(false);
    if !reserved || !self.admits(&request.opener, request.opener_browser_id) {
      state.close();
      return false;
    }
    let popup = Arc::new(Popup {
      browser_id,
      window,
      state: state.clone(),
      opener: request.opener.clone(),
      closing: AtomicBool::new(false),
      dialogs: crate::dialog::DialogState::new(state.clone()),
    });
    let Ok(mut popups) = self.popups.lock() else {
      state.close();
      return false;
    };
    popups.push(popup);
    true
  }

  /// Revoke descendants before the caller asks Electron to close them. The
  /// returned webContents ids are the ones to close; no family lock is held
  /// across that call.
  pub(crate) fn closed(&self, state: &FrameNavigationState, browser_id: i32) -> Vec<i32> {
    if !state.has_browser_id(browser_id) {
      return Vec::new();
    }
    state.close();
    let root_closed = self.root.is_same_browser(state);
    if root_closed {
      self.closing.store(true, Ordering::Release);
    }
    let descendants = {
      let Ok(mut popups) = self.popups.lock() else {
        return Vec::new();
      };
      popups.retain(|popup| !popup.state.is_same_browser(state));
      revoke_descendants(
        state,
        root_closed,
        popups
          .iter()
          .enumerate()
          .map(|(index, popup)| (index, &popup.state, &popup.opener, &popup.closing)),
      )
      .into_iter()
      .map(|index| popups[index].browser_id)
      .collect::<Vec<_>>()
    };
    descendants
  }

  /// Window close and app shutdown own the whole family, so teardown never
  /// depends on the root having observed a webContents identity. `closed` keeps
  /// its exact-identity guard for the per-webContents native callback; a root
  /// that was exhausted before its first frame event would otherwise leave every
  /// popup open and the event loop waiting on them forever.
  pub(crate) fn close_all(&self) -> Vec<i32> {
    self.closing.store(true, Ordering::Release);
    self.root.close();
    let Ok(popups) = self.popups.lock() else {
      return Vec::new();
    };
    revoke_descendants(
      &self.root,
      true,
      popups
        .iter()
        .enumerate()
        .map(|(index, popup)| (index, &popup.state, &popup.opener, &popup.closing)),
    )
    .into_iter()
    .map(|index| popups[index].browser_id)
    .collect()
  }

  /// A revoked family lost its root and admits no further popup, so nothing it
  /// still has reserved can ever be created. Electron does not always report the
  /// abort of a popup it discards, so the reservations of a revoked family are
  /// what the event loop would otherwise wait on forever.
  pub(crate) fn is_revoked(&self) -> bool {
    self.closing.load(Ordering::Acquire)
  }

  pub(crate) fn observe(&self) -> Vec<Webview> {
    if self.closing.load(Ordering::Acquire) {
      return Vec::new();
    }
    let popups = self
      .popups
      .lock()
      .map(|popups| popups.clone())
      .unwrap_or_default();
    // Several popups may share one window. Window identity is owned by the
    // family, never minted independently for each tab.
    let mut windows = self
      .windows
      .lock()
      .map(|windows| windows.clone())
      .unwrap_or_default();
    let observations = popups
      .iter()
      .filter_map(|popup| {
        if popup.closing.load(Ordering::Acquire) {
          return None;
        }
        if !windows.iter().any(|token| *token == popup.window) {
          if windows.len() >= MAX_POPUPS {
            return None;
          }
          windows.push(popup.window.clone());
        }
        let document = popup.state.document();
        let dialogs = popup.dialogs.snapshot(document.as_ref());
        let snapshot = WebviewSnapshot {
          browser_id: popup.browser_id,
          dialogs,
          document,
          window_label: None,
          window: Some(popup.window.clone()),
          // The runtime created this popup's window and never reparents it, so
          // there is no independently observed parent to check the webview
          // against: reporting a match would hand a caller gating a native
          // effect on `Some(true)` an assertion nothing verified.
          parent_matches: None,
          // The family performs no RPC, so it cannot sample the popup's live
          // geometry; the reference reads it from the CEF view. A caller that
          // needs bounds asks the window dispatcher.
          bounds: None,
          visible: None,
        };
        let mut native = Webview::new(popup.browser_id, snapshot, popup.state.clone());
        native.set_opener(popup.opener.clone());
        Some(native)
      })
      .collect();
    if let Ok(mut retained) = self.windows.lock() {
      *retained = windows;
    }
    observations
  }
}

/// The graph walk is independent of native calls. Revoke every descendant before
/// returning ids to close, including when callbacks arrive out of order.
fn revoke_descendants<'a>(
  state: &FrameNavigationState,
  root_closed: bool,
  nodes: impl Iterator<
    Item = (
      usize,
      &'a FrameNavigationState,
      &'a FrameNavigationState,
      &'a AtomicBool,
    ),
  > + Clone,
) -> Vec<usize> {
  let mut revoked = vec![state.clone()];
  let mut indices = Vec::new();
  loop {
    let before = indices.len();
    for (index, state, opener, closing) in nodes.clone() {
      if !closing.load(Ordering::Acquire)
        && (root_closed || revoked.iter().any(|parent| parent.is_same_browser(opener)))
      {
        closing.store(true, Ordering::Release);
        state.close();
        revoked.push(state.clone());
        indices.push(index);
      }
    }
    if indices.len() == before {
      return indices;
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  fn created(id: i32) -> FrameNavigationState {
    let state = FrameNavigationState::new();
    state.on_frame_event(&crate::FrameEvent {
      browser_id: id,
      frame_id: "main".into(),
      is_main: true,
      kind: crate::FrameEventKind::Created,
    });
    state
  }
  #[test]
  fn closing_an_opener_revokes_only_its_exact_descendants() {
    let root = created(1);
    let first = created(2);
    let nested = created(3);
    let sibling = created(4);
    // Deliberately put a descendant before its opener.
    let nodes = [
      (nested.clone(), first.clone(), AtomicBool::new(false)),
      (first.clone(), root.clone(), AtomicBool::new(false)),
      (sibling.clone(), root.clone(), AtomicBool::new(false)),
    ];
    let refs = || {
      nodes
        .iter()
        .enumerate()
        .map(|(index, (state, opener, closing))| (index, state, opener, closing))
    };
    assert!(revoke_descendants(&created(2), false, refs()).is_empty());
    assert_eq!(revoke_descendants(&first, false, refs()), vec![0]);
    assert!(nodes[0].2.load(Ordering::Acquire));
    assert!(!nodes[2].2.load(Ordering::Acquire));
    assert_eq!(revoke_descendants(&root, true, refs()), vec![1, 2]);
    assert!(nodes.iter().all(|node| node.2.load(Ordering::Acquire)));
    assert!(revoke_descendants(&root, true, refs()).is_empty());
  }
  #[test]
  fn pending_popups_are_bounded_and_retained_until_exact_creation_or_abort() {
    let root = created(1);
    let family = PopupFamily::new(root.clone());
    let first = family.reserve(&root, 1, 7).unwrap();
    assert!(family.reserve(&root, 1, 7).is_none());
    assert!(family.abort(&created(1), 7).is_none());
    for id in 8..(7 + MAX_POPUPS as u64) {
      assert!(family.reserve(&root, 1, id).is_some());
    }
    assert!(family.reserve(&root, 1, 1000).is_none());
    family.closed(&root, 1);
    assert_eq!(family.pending.lock().unwrap().len(), MAX_POPUPS);
    assert!(family.abort(&root, 7).unwrap().is_same(&first));
    assert!(family.abort(&root, 7).is_none());
    assert!(family.reserve(&root, 1, 1000).is_none());
  }
  #[test]
  fn teardown_revokes_a_family_still_holding_an_unresolved_reservation() {
    let root = created(1);
    let family = PopupFamily::new(root.clone());
    // Electron discarded this popup without ever creating or aborting it.
    let request = family.reserve(&root, 1, 7).unwrap();
    assert!(!family.is_revoked());
    // A popup's own native close leaves the family live, so the event loop
    // keeps waiting for the reservations that can still be created.
    family.closed(&created(2), 2);
    assert!(!family.is_revoked());
    assert_eq!(family.pending.lock().unwrap().len(), 1);
    // Window close and app shutdown own the family outright: once revoked, no
    // reservation of it can ever be created, so none may hold the loop open.
    family.close_all();
    assert!(family.is_revoked());
    assert!(!family.admits(&request.opener, 1));
  }
  #[test]
  fn popup_admission_requires_the_live_exact_native_opener() {
    let root = created(1);
    let family = PopupFamily::new(root.clone());
    assert!(family.admits(&root, 1));
    assert!(!family.admits(&root, 2));
    assert!(!family.admits(&created(1), 1));
    assert!(!family.admits(&FrameNavigationState::new(), 1));
    family.closed(&root, 2);
    assert!(family.admits(&root, 1));
    family.closed(&root, 1);
    assert!(!family.admits(&root, 1));
    assert!(family.observe().is_empty());
  }
  #[test]
  fn teardown_closes_a_family_whose_root_never_observed_a_browser_id() {
    // A root that never observed a native frame event has no browser identity.
    let root = FrameNavigationState::new();
    assert!(!root.has_browser_id(1));
    let family = PopupFamily::new(root.clone());
    // The per-webContents native callback cannot match an unobserved identity.
    family.closed(&root, 1);
    assert!(!family.closing.load(Ordering::Acquire));
    // Window close and app shutdown own the family outright and must not.
    family.close_all();
    assert!(family.closing.load(Ordering::Acquire));
    assert!(family.observe().is_empty());
  }
}
