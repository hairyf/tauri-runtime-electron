// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! Native document generations shared by every Electron webview.

use std::collections::BTreeMap;
use std::collections::BTreeSet;

use std::sync::{Arc, Mutex};

const MAX_NATIVE_FRAMES: usize = 256;
const MAX_NATIVE_FRAME_ID_BYTES: usize = 512;

#[derive(Debug, Default)]
struct NativeFrameState {
  browser_id: Option<i32>,
  generation: u64,
  exhausted: bool,
  loading: bool,
  observed_load_state: bool,
  main_frame: Option<String>,
  frames: BTreeMap<String, bool>,
}

/// The native facts [`FrameNavigationState::observe_document`] validates against.
///
/// The reference reads them straight off the `cef::Browser` it is handed
/// (`frame_count`, `frame_identifiers`, `frame_by_identifier`, `is_loading`).
/// Electron keeps the same facts in its main process
/// (`webContents.mainFrame.framesInSubtree`, `frame.detached`,
/// `webContents.isLoading()`), so they arrive here as data instead of a browser
/// handle and this module stays a pure state machine.
#[derive(Clone, Debug)]
pub(crate) struct NativeBrowserSnapshot {
  /// The Electron `webContents.id` the frames belong to.
  pub(crate) browser_id: i32,
  /// Whether the browser is loading, as Electron reports it.
  pub(crate) is_loading: bool,
  /// Every frame Electron lists, including ones parked in the back/forward cache.
  pub(crate) frame_count: usize,
  /// The frames that are attached to a renderer, which is what tracking compares.
  pub(crate) attached_frames: BTreeSet<String>,
}

/// Read-only navigation state for one exact native Electron browser lifetime. The
/// runtime advances it from the event stream before input submission can be
/// admitted on that same thread.
/// No lock is held while calling Electron or awaiting a renderer response.
#[derive(Clone, Debug)]
pub struct FrameNavigationState {
  state: Arc<Mutex<NativeFrameState>>,
}

/// Opaque process-local proof of an observed native browser/document lifetime.
/// Compare it with `WebviewSnapshot::document` in the final callback
/// before an effect. A token alone does not authorize an account or profile.
#[derive(Clone)]
pub struct NativeDocumentToken {
  state: Arc<Mutex<NativeFrameState>>,
  generation: u64,
}

impl PartialEq for NativeDocumentToken {
  fn eq(&self, other: &Self) -> bool {
    Arc::ptr_eq(&self.state, &other.state) && self.generation == other.generation
  }
}
impl Eq for NativeDocumentToken {}

impl std::fmt::Debug for NativeDocumentToken {
  fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    formatter
      .debug_struct("NativeDocumentToken")
      .finish_non_exhaustive()
  }
}

impl FrameNavigationState {
  pub(crate) fn new() -> Self {
    Self {
      state: Arc::default(),
    }
  }

  /// Identifies the same native browser lifetime independently of navigation.
  /// A replacement browser never matches, even if a caller reuses its label.
  pub fn is_same_browser(&self, other: &Self) -> bool {
    Arc::ptr_eq(&self.state, &other.state)
  }

  pub(crate) fn has_browser_id(&self, browser_id: i32) -> bool {
    self
      .state
      .lock()
      .is_ok_and(|state| state.browser_id == Some(browser_id))
  }

  pub(crate) fn close(&self) {
    if let Ok(mut state) = self.state.lock() {
      state.exhausted = true;
    }
  }

  /// Validates the ready generation against the native facts Electron reported.
  ///
  /// The reference calls this with the `cef::Browser` it is about to act on and
  /// compares the tracked frames against the browser's live frame list. Electron
  /// keeps that list in its main process (`webContents.mainFrame.framesInSubtree`)
  /// and the bridge exposes no command for it, so `WebviewSnapshot::document` uses
  /// [`document`](Self::document) instead: the same admission predicate, without
  /// the second opinion. This is the entry point for the day a native frame list
  /// exists, and `native: None` fails closed rather than minting a token nothing
  /// can be compared against — exactly what the reference does when
  /// `browser.is_valid()` is false.
  pub(crate) fn observe_document(
    &self,
    native: Option<&NativeBrowserSnapshot>,
  ) -> Option<NativeDocumentToken> {
    let generation = self.ready_generation()?;
    let native = native?;
    // Electron keeps listing a frame parked in the back/forward cache, detached
    // from any renderer. Tracking drops a frame on detach, so only attached frames
    // are compared; a restored frame reattaches, advancing the generation.
    if native.frame_count > MAX_NATIVE_FRAMES {
      return None;
    }
    self
      .admits_native_frames(
        generation,
        native.browser_id,
        native.is_loading,
        &native.attached_frames,
      )
      .then(|| NativeDocumentToken {
        state: Arc::clone(&self.state),
        generation,
      })
  }

  /// Captures an observed document only when every known frame is attached and
  /// native load completion has been observed. Compare with the final native
  /// snapshot before dispatch; this read does not query current frame IDs.
  pub fn document(&self) -> Option<NativeDocumentToken> {
    self
      .ready_generation()
      .map(|generation| NativeDocumentToken {
        state: Arc::clone(&self.state),
        generation,
      })
  }

  fn ready_generation(&self) -> Option<u64> {
    let state = self.state.lock().ok()?;
    let ready = Self::ready(&state);
    if !ready {
      log::debug!(
        "native document unavailable: exhausted={}, load_observed={}, loading={}, frames={}, attached={}, main_attached={}",
        state.exhausted,
        state.observed_load_state,
        state.loading,
        state.frames.len(),
        state.frames.values().filter(|attached| **attached).count(),
        state
          .main_frame
          .as_ref()
          .is_some_and(|id| state.frames.get(id) == Some(&true)),
      );
    }
    ready.then_some(state.generation)
  }

  fn ready(state: &NativeFrameState) -> bool {
    !state.exhausted
      && state.observed_load_state
      && !state.loading
      && !state.frames.is_empty()
      // Exceeding the tracked frame cap fails admission closed for as long as
      // it lasts, matching `observe_document`'s native `frame_count` guard.
      // Frames keep being tracked exactly, so draining below the cap recovers.
      && state.frames.len() <= MAX_NATIVE_FRAMES
      && state.frames.values().all(|attached| *attached)
      && state
        .main_frame
        .as_ref()
        .is_some_and(|id| state.frames.get(id) == Some(&true))
  }

  pub(crate) fn on_frame_event(&self, event: &crate::FrameEvent) {
    self.apply(
      event.browser_id,
      &event.frame_id,
      event.is_main,
      &event.kind,
    );
  }

  fn apply(&self, browser_id: i32, frame_id: &str, is_main: bool, kind: &crate::FrameEventKind) {
    use crate::FrameEventKind;

    let Ok(mut state) = self.state.lock() else {
      return;
    };
    if state.exhausted {
      return;
    }
    // A window the runtime opened for `window.open` can inherit an opener's event
    // handlers. Its browser identity never advances or grants authority over the
    // opener.
    if state
      .browser_id
      .is_some_and(|expected| expected != browser_id)
    {
      return;
    }
    // Bind the native browser identity before any rejection below can latch
    // exhaustion. IPC admission and event delivery are gated on this identity,
    // so an unexpected first notification must never leave it unrecorded.
    state.browser_id = Some(browser_id);
    let browser_wide = matches!(
      kind,
      FrameEventKind::MainFrameChanged
        | FrameEventKind::LoadingStateChanged { .. }
        | FrameEventKind::RendererTerminated
    );
    if frame_id.is_empty() && !browser_wide || frame_id.len() > MAX_NATIVE_FRAME_ID_BYTES {
      log::warn!(
        "native frame tracking exhausted: invalid identifier, empty={}, length={}, main={}, teardown={}",
        frame_id.is_empty(),
        frame_id.len(),
        is_main,
        matches!(kind, FrameEventKind::Detached | FrameEventKind::Destroyed),
      );
      state.exhausted = true;
      return;
    }
    let Some(generation) = state.generation.checked_add(1) else {
      state.exhausted = true;
      return;
    };
    state.generation = generation;
    match kind {
      FrameEventKind::Created => {
        state.frames.insert(frame_id.to_string(), false);
      }
      FrameEventKind::Attached => {
        state.frames.insert(frame_id.to_string(), true);
      }
      FrameEventKind::Detached | FrameEventKind::Destroyed => {
        state.frames.remove(frame_id);
        if state.main_frame.as_deref() == Some(frame_id) {
          state.main_frame = None;
        }
      }
      FrameEventKind::MainFrameChanged => {
        state.main_frame = is_main.then(|| frame_id.to_string());
      }
      FrameEventKind::NavigationStarted { .. } => {
        state.loading = true;
      }
      FrameEventKind::LoadingStateChanged { is_loading } => {
        state.observed_load_state = true;
        state.loading = *is_loading;
      }
      FrameEventKind::DocumentCommitted { .. }
      | FrameEventKind::NavigationFailed { .. }
      | FrameEventKind::AddressChanged { .. } => {}
      FrameEventKind::RendererTerminated => {
        state.frames.clear();
        state.main_frame = None;
        state.loading = true;
        state.observed_load_state = false;
      }
    }
  }

  /// Final native admission compares the exact current frame identities,
  /// not just their count. A replacement cannot reuse a document capability.
  pub(crate) fn admits_native_frames(
    &self,
    expected: u64,
    browser_id: i32,
    is_loading: bool,
    frame_ids: &BTreeSet<String>,
  ) -> bool {
    let Ok(state) = self.state.lock() else {
      return false;
    };
    Self::ready(&state)
      && state.generation == expected
      && state.browser_id == Some(browser_id)
      && !is_loading
      && state.frames.keys().eq(frame_ids.iter())
  }
}

#[cfg(test)]
mod tests {
  use crate::FrameEventKind as Event;

  use super::*;

  fn apply(barrier: &FrameNavigationState, frame: &str, kind: Event) {
    barrier.apply(1, frame, frame == "main", &kind);
  }

  fn ready_main() -> FrameNavigationState {
    let barrier = FrameNavigationState::new();
    apply(&barrier, "main", Event::Created);
    apply(&barrier, "main", Event::MainFrameChanged);
    apply(&barrier, "main", Event::Attached);
    apply(
      &barrier,
      "main",
      Event::LoadingStateChanged { is_loading: false },
    );
    barrier
  }

  fn admits(barrier: &FrameNavigationState, generation: u64, frames: &[&str]) -> bool {
    barrier.admits_native_frames(
      generation,
      1,
      false,
      &frames.iter().map(|id| id.to_string()).collect(),
    )
  }

  #[test]
  fn child_navigation_replacement_and_detach_revoke_prior_documents() {
    let barrier = ready_main();
    let main_generation = barrier.ready_generation().unwrap();
    assert!(admits(&barrier, main_generation, &["main"]));
    apply(&barrier, "child-a", Event::Created);
    assert!(barrier.ready_generation().is_none());
    apply(&barrier, "child-a", Event::Attached);
    let child_generation = barrier.ready_generation().unwrap();
    assert!(!admits(&barrier, main_generation, &["main", "child-a"]));
    assert!(admits(&barrier, child_generation, &["main", "child-a"]));
    let url = url::Url::parse("https://example.test/frame").unwrap();
    apply(
      &barrier,
      "child-a",
      Event::NavigationStarted { url: url.clone() },
    );
    assert!(barrier.ready_generation().is_none());
    apply(&barrier, "child-a", Event::DocumentCommitted { url });
    assert!(barrier.ready_generation().is_none());
    apply(
      &barrier,
      "main",
      Event::LoadingStateChanged { is_loading: false },
    );
    assert!(!admits(&barrier, child_generation, &["main", "child-a"]));
    let loaded_generation = barrier.ready_generation().unwrap();
    assert!(admits(&barrier, loaded_generation, &["main", "child-a"]));
    // Even an equal native frame count must reject different identities.
    assert!(!admits(&barrier, loaded_generation, &["main", "child-b"]));
    apply(&barrier, "child-a", Event::Detached);
    apply(&barrier, "child-b", Event::Created);
    apply(&barrier, "child-b", Event::Attached);
    assert!(!admits(&barrier, loaded_generation, &["main", "child-b"]));
    let replacement_generation = barrier.ready_generation().unwrap();
    assert!(admits(
      &barrier,
      replacement_generation,
      &["main", "child-b"]
    ));
    apply(&barrier, "child-b", Event::Detached);
    assert!(!admits(&barrier, replacement_generation, &["main"]));
    assert!(admits(
      &barrier,
      barrier.ready_generation().unwrap(),
      &["main"]
    ));
  }

  #[test]
  fn pending_cancelled_navigation_waits_for_native_loading_completion() {
    let barrier = ready_main();
    let url = url::Url::parse("https://example.test/repeated").unwrap();
    apply(
      &barrier,
      "main",
      Event::NavigationStarted { url: url.clone() },
    );
    apply(
      &barrier,
      "main",
      Event::NavigationStarted { url: url.clone() },
    );
    apply(&barrier, "main", Event::NavigationFailed { url });
    assert!(barrier.ready_generation().is_none());
    apply(
      &barrier,
      "main",
      Event::LoadingStateChanged { is_loading: false },
    );
    assert!(barrier.ready_generation().is_some());
  }

  #[test]
  fn foreign_browser_unobserved_frames_and_exhaustion_fail_closed() {
    let barrier = ready_main();
    let generation = barrier.ready_generation().unwrap();
    barrier.apply(2, "popup", true, &Event::Created);
    assert_eq!(barrier.ready_generation(), Some(generation));
    assert!(!barrier.admits_native_frames(
      generation,
      2,
      false,
      &BTreeSet::from(["main".to_string()])
    ));
    assert!(!admits(&barrier, generation, &["main", "unobserved"]));
    barrier.state.lock().unwrap().generation = u64::MAX;
    apply(&barrier, "main", Event::Attached);
    assert!(barrier.ready_generation().is_none());
    apply(
      &barrier,
      "main",
      Event::LoadingStateChanged { is_loading: false },
    );
    assert!(barrier.ready_generation().is_none());
  }
  #[test]
  fn missing_main_frame_and_renderer_termination_revoke_prior_generations() {
    let barrier = ready_main();
    let generation = barrier.ready_generation().unwrap();
    barrier.apply(1, "", false, &Event::MainFrameChanged);
    assert!(barrier.ready_generation().is_none());
    apply(&barrier, "main", Event::MainFrameChanged);
    assert_ne!(barrier.ready_generation().unwrap(), generation);
    barrier.apply(1, "", false, &Event::RendererTerminated);
    assert!(barrier.ready_generation().is_none());
    barrier.apply(
      1,
      "",
      false,
      &Event::LoadingStateChanged { is_loading: false },
    );
    assert!(barrier.ready_generation().is_none());
    apply(&barrier, "replacement", Event::Created);
    apply(&barrier, "replacement", Event::Attached);
    barrier.apply(1, "replacement", true, &Event::MainFrameChanged);
    assert_ne!(barrier.ready_generation().unwrap(), generation);
  }
  #[test]
  fn document_tokens_cannot_cross_native_lifetimes_or_navigation() {
    let first = ready_main();
    let second = ready_main();
    assert_eq!(first.ready_generation(), second.ready_generation());
    assert!(first.is_same_browser(&first.clone()));
    assert!(!first.is_same_browser(&second));
    let before = first.document().unwrap();
    assert_eq!(before, first.clone().document().unwrap());
    assert_ne!(before, second.document().unwrap());
    apply(&first, "child", Event::Created);
    assert!(first.document().is_none());
    apply(&first, "child", Event::Attached);
    assert_ne!(before, first.document().unwrap());
  }
  #[test]
  fn a_rejected_first_event_still_records_the_native_browser_identity() {
    // `AddressChanged`/`NavigationFailed` tolerate a missing native frame, so a
    // browser's very first notification can carry an empty identifier.
    let url = url::Url::parse("https://example.test/frameless").unwrap();
    let barrier = FrameNavigationState::new();
    barrier.apply(1, "", false, &Event::AddressChanged { url });
    // Document admission still fails closed, ...
    assert!(barrier.document().is_none());
    // ... but IPC and event delivery stay bound to this exact browser.
    assert!(barrier.has_browser_id(1));
    assert!(!barrier.has_browser_id(2));
  }
  #[test]
  fn a_transient_frame_count_overflow_recovers_once_frames_drain() {
    let barrier = ready_main();
    for index in 0..MAX_NATIVE_FRAMES {
      apply(&barrier, &format!("child-{index}"), Event::Created);
      apply(&barrier, &format!("child-{index}"), Event::Attached);
    }
    assert!(barrier.ready_generation().is_none());
    apply(&barrier, "child-0", Event::Detached);
    assert!(barrier.ready_generation().is_some());
    assert!(barrier.has_browser_id(1));
  }
  #[test]
  fn native_admission_needs_the_reported_frame_identities() {
    let barrier = ready_main();
    let generation = barrier.ready_generation().unwrap();
    // No synchronous native view of the browser: fail closed.
    assert!(barrier.observe_document(None).is_none());
    let matching = NativeBrowserSnapshot {
      browser_id: 1,
      is_loading: false,
      frame_count: 1,
      attached_frames: BTreeSet::from(["main".to_string()]),
    };
    assert!(barrier.observe_document(Some(&matching)).is_some());
    // A browser this state does not belong to is rejected.
    let foreign = NativeBrowserSnapshot {
      browser_id: 2,
      ..matching.clone()
    };
    assert!(barrier.observe_document(Some(&foreign)).is_none());
    // A frame list that disagrees with what was tracked is rejected, and so is a
    // browser that is still loading.
    let unobserved = NativeBrowserSnapshot {
      attached_frames: BTreeSet::from(["main".to_string(), "child".to_string()]),
      frame_count: 2,
      ..matching.clone()
    };
    assert!(barrier.observe_document(Some(&unobserved)).is_none());
    let loading = NativeBrowserSnapshot {
      is_loading: true,
      ..matching.clone()
    };
    assert!(barrier.observe_document(Some(&loading)).is_none());
    // Exceeding the native frame cap fails closed.
    let overflowing = NativeBrowserSnapshot {
      frame_count: MAX_NATIVE_FRAMES + 1,
      ..matching.clone()
    };
    assert!(barrier.observe_document(Some(&overflowing)).is_none());
    assert_eq!(barrier.ready_generation(), Some(generation));
  }
}
