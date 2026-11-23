// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! File drag-and-drop into a webview: the Electron equivalent of `DragHandler` plus the
//! `ResourceRequestHandler` it leans on.
//!
//! CEF's `DragHandler` only reports `on_drag_enter`; the over/drop/leave positions come from
//! an injected script that reports them over a private fetch URL, which a
//! `ResourceRequestHandler` intercepts and cancels. Electron is in exactly the same
//! position, and for the same reason: `webContents` exposes no `drag-enter`, `drag-over` or
//! `drop` event. Dragging files *into* web content is an ordinary HTML5 capability, so the
//! renderer sees `dragenter`/`dragover`/`drop`/`dragleave` with a `DataTransfer`, and the
//! injected-script bridge is the mechanism on both runtimes rather than a CEF workaround.
//!
//! # What changed, and why it has to
//!
//! `DragData::file_paths` is absent: there is no native drag callback and no native
//! file-path list. A sandboxed renderer's `File` objects carry no path either, so the paths
//! have to come from `webUtils.getPathForFile(file)`, which only a preload can call. The
//! script therefore asks the preload for them — through a `contextBridge` function the
//! preload exposes — and reports them alongside the position. That is the reference's own
//! split, with the two halves arriving from different places: the paths from the preload,
//! the viewport position from the page.
//!
//! The interception point is `session.webRequest.onBeforeRequest` in the bridge rather than
//! a `ResourceRequestHandler`, so [`intercepts`] is what the bridge applies and
//! [`dispatch_script_event`] is what it calls once it has cancelled the request.

use std::{
  path::PathBuf,
  sync::{Arc, Mutex},
};

use tauri_runtime::{
  RunEvent, UserEvent,
  dpi::PhysicalPosition,
  webview::InitializationScript,
  window::{DragDropEvent, WebviewEvent, WindowEvent},
};
use url::Url;

use crate::{
  electron_impl::client::{self, send_window_event},
  runtime::RuntimeContext,
};

/// The private path the injected script reports through.
pub(crate) const DRAG_DROP_BRIDGE_PATH: &str = "/__tauri_electron_drag_drop__";

/// The bridge's script.
///
/// Idempotent through its own global, listening in the capture phase so a page handler that
/// stops propagation cannot hide a drag from the runtime, and reporting
/// `{ type, x, y, paths }` where the position is in physical pixels. The paths come from the
/// preload: `webUtils.getPathForFile` is the modern replacement for the removed `File.path`,
/// and a sandboxed renderer has no other way to name a dropped file.
const DRAG_DROP_INIT_SCRIPT: &str = r#"
(() => {
  if (window.__TAURI_ELECTRON_DRAG_DROP__) {
    return;
  }

  Object.defineProperty(window, "__TAURI_ELECTRON_DRAG_DROP__", {
    value: true,
    configurable: false,
  });

  const PATH = "/__tauri_electron_drag_drop__";
  let entered = false;

  const position = (event) => ({
    x: event.clientX * window.devicePixelRatio,
    y: event.clientY * window.devicePixelRatio,
  });

  // The preload exposes this through `contextBridge`; without it a drop is still reported,
  // it simply carries no paths, which is what a renderer-only drag looks like.
  const paths = (event) => {
    const resolve = window.__TAURI_ELECTRON_DRAG_DROP_PATHS__;
    const files = event.dataTransfer ? event.dataTransfer.files : null;
    if (typeof resolve !== "function" || !files || files.length === 0) {
      return [];
    }
    const resolved = [];
    for (const file of Array.from(files)) {
      const path = resolve(file);
      if (typeof path === "string" && path.length > 0) {
        resolved.push(path);
      }
    }
    return resolved;
  };

  const send = (type, event) => {
    const pos = position(event);
    const url = new URL(PATH, window.location.href);
    url.searchParams.set(
      "payload",
      JSON.stringify({ type, x: pos.x, y: pos.y, paths: paths(event) }),
    );
    fetch(url.href, {
      method: "GET",
      cache: "no-store",
      credentials: "omit",
    }).catch(() => {});
  };

  const listen = (eventName, handler) => {
    window.addEventListener(eventName, handler, { capture: true });
  };

  listen("dragenter", (event) => {
    if (!entered) {
      entered = true;
      send("enter", event);
    }
  });

  listen("dragover", (event) => {
    if (!entered) {
      entered = true;
      send("enter", event);
    }
    send("over", event);
  });

  listen("drop", (event) => {
    if (!entered) {
      send("enter", event);
    }
    entered = false;
    send("drop", event);
  });

  listen("dragleave", (event) => {
    const x = event.clientX;
    const y = event.clientY;
    if (entered && (x <= 0 || y <= 0 || x >= window.innerWidth || y >= window.innerHeight)) {
      entered = false;
      send("leave", event);
    }
  });
})();
"#;

/// The script as the webview builder wants it.
///
/// `for_main_frame_only: false`, as in the reference: a drag can land in a subframe, and the
/// position it reports is relative to the frame it landed in.
pub(crate) fn drag_drop_initialization_script() -> InitializationScript {
  InitializationScript {
    script: DRAG_DROP_INIT_SCRIPT.to_string(),
    for_main_frame_only: false,
  }
}

/// The drag/drop state machine of one webview.
#[derive(Default)]
pub(crate) struct DragDropState {
  pub(crate) paths: Option<Vec<PathBuf>>,
  /// Whether a drop carried paths, which is what the reference's native `on_drag_enter`
  /// established. Without it a renderer-only drag (selected text, a page element) would be
  /// reported as a file drop.
  pub(crate) native_entered: bool,
  pub(crate) entered: bool,
}

/// Where a drag/drop event is reported.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum DragDropEventTarget {
  Window,
  Webview,
}

/// One report from the injected script.
#[derive(Clone, Debug, PartialEq, serde::Deserialize)]
pub(crate) struct DragDropScriptEvent {
  #[serde(rename = "type")]
  pub(crate) kind: String,
  pub(crate) x: f64,
  pub(crate) y: f64,
  /// The paths the preload resolved for this drag, empty when it resolved none.
  #[serde(default)]
  pub(crate) paths: Vec<String>,
}

/// The drag/drop handler of one webview, mirroring `TauriCefDragHandler`.
///
/// The reference's `on_drag_enter` reads the native `DragData`; there is no such callback
/// here, so the same state transition is driven by the paths the preload resolved. It is
/// kept separate from [`event_from_script_event`] because the two halves arrive separately:
/// the preload knows the files, the page knows where they are.
pub(crate) struct TauriElectronDragHandler {
  drag_drop_state: Arc<Mutex<DragDropState>>,
}

impl TauriElectronDragHandler {
  pub(crate) fn new(drag_drop_state: Arc<Mutex<DragDropState>>) -> Self {
    Self { drag_drop_state }
  }

  /// A drag entered the webview carrying `paths`.
  ///
  /// Resets `entered` — a new drag is a new gesture — and records the paths the preload
  /// resolved. An empty list leaves the state without paths, which is what makes the script's
  /// `enter` report nothing and keeps a text drag from being reported as a file drop.
  pub(crate) fn on_drag_enter(&self, paths: Vec<PathBuf>) {
    let mut state = self
      .drag_drop_state
      .lock()
      .unwrap_or_else(|poisoned| poisoned.into_inner());
    state.entered = false;
    state.paths = (!paths.is_empty()).then_some(paths);
    state.native_entered = state.paths.is_some();
  }
}

/// Turns one script report into the portable drag/drop event, if it is one.
///
/// The transitions are the reference's exactly: `enter` once, `over` only while entered,
/// `drop` only while entered and only when paths are known, `leave` only when it had
/// entered. Anything else is not a drag this runtime reports.
pub(crate) fn event_from_script_event(
  drag_drop_state: &Arc<Mutex<DragDropState>>,
  script_event: DragDropScriptEvent,
) -> Option<DragDropEvent> {
  let position = PhysicalPosition::new(script_event.x, script_event.y);
  let mut state = drag_drop_state
    .lock()
    .unwrap_or_else(|poisoned| poisoned.into_inner());

  match script_event.kind.as_str() {
    "enter" => {
      if state.entered {
        return None;
      }
      // The paths arrive with the first report of the gesture, because that is the only
      // report that carries the `DataTransfer` the preload resolves them from.
      if state.paths.is_none() && !script_event.paths.is_empty() {
        state.paths = Some(script_event.paths.iter().map(PathBuf::from).collect());
        state.native_entered = true;
      }
      if !state.native_entered {
        return None;
      }

      let paths = state.paths.clone()?;
      state.entered = true;
      Some(DragDropEvent::Enter { paths, position })
    }
    "over" => state.entered.then_some(DragDropEvent::Over { position }),
    "drop" => {
      let paths = state.entered.then(|| state.paths.take()).flatten();
      state.entered = false;
      state.native_entered = false;
      paths.map(|paths| DragDropEvent::Drop { paths, position })
    }
    "leave" => {
      state.native_entered = false;
      state.paths = None;

      if state.entered {
        state.entered = false;
        Some(DragDropEvent::Leave)
      } else {
        None
      }
    }
    _ => None,
  }
}

/// Whether `url` is the bridge's private report, and what it carried.
///
/// This is the reference's `on_before_resource_load`: the bridge cancels a request whose path
/// is [`DRAG_DROP_BRIDGE_PATH`] and hands the payload here. A payload that does not parse is
/// still a request to cancel, so the answer is `Some(None)`.
pub(crate) fn intercepts(url: &Url) -> Option<Option<DragDropScriptEvent>> {
  if url.path() != DRAG_DROP_BRIDGE_PATH {
    return None;
  }
  let event = url
    .query_pairs()
    .find_map(|(key, value)| (key == "payload").then(|| value.into_owned()))
    .and_then(|payload| serde_json::from_str::<DragDropScriptEvent>(&payload).ok());
  Some(event)
}

/// Reports one intercepted script event to the application.
///
/// The reference posts `Message::DragDropScriptEvent` and lets the run loop decide between
/// the window and the webview target. Both answers exist here too, but only one of them has
/// a message in this runtime: a window event travels through the run loop, while a webview
/// event is a `RunEvent` the loop never produces, so it is handed to the callback directly.
///
/// Nothing calls this yet: the bridge cancels the private request in
/// `session.webRequest.onBeforeRequest` and does not forward it, so a drag currently reaches
/// the page and not the application. The state machine above is complete and tested, which is
/// what makes wiring the forward a bridge-only change.
pub(crate) fn dispatch_script_event<T: UserEvent, F: FnMut(RunEvent<T>)>(
  context: &RuntimeContext<T>,
  webview_id: u32,
  target: DragDropEventTarget,
  drag_drop_state: &Arc<Mutex<DragDropState>>,
  script_event: DragDropScriptEvent,
  callback: &mut F,
) {
  let Some(event) = event_from_script_event(drag_drop_state, script_event) else {
    return;
  };

  match target {
    DragDropEventTarget::Window => {
      if let Some(window_id) = client::window_id_of::<T>(webview_id) {
        send_window_event(context, window_id, WindowEvent::DragDrop(event));
      }
    }
    DragDropEventTarget::Webview => {
      if let Some(label) = client::label_of::<T>(webview_id) {
        callback(RunEvent::WebviewEvent {
          label,
          event: WebviewEvent::DragDrop(event),
        });
      }
    }
  }
  let _ = context;
}

#[cfg(test)]
mod tests {
  use super::*;

  fn state() -> Arc<Mutex<DragDropState>> {
    Arc::new(Mutex::new(DragDropState::default()))
  }

  fn event(kind: &str, paths: &[&str]) -> DragDropScriptEvent {
    DragDropScriptEvent {
      kind: kind.to_owned(),
      x: 10.0,
      y: 20.0,
      paths: paths.iter().map(|path| (*path).to_owned()).collect(),
    }
  }

  #[test]
  fn the_script_is_idempotent_and_reports_paths() {
    let script = drag_drop_initialization_script();
    assert!(script.script.contains("__TAURI_ELECTRON_DRAG_DROP__"));
    assert!(script.script.contains("__TAURI_ELECTRON_DRAG_DROP_PATHS__"));
    assert!(script.script.contains("cache: \"no-store\""));
    assert!(!script.for_main_frame_only);
  }

  #[test]
  fn a_drag_without_paths_is_not_reported() {
    let state = state();
    assert!(event_from_script_event(&state, event("enter", &[])).is_none());
    assert!(event_from_script_event(&state, event("over", &[])).is_none());
  }

  #[test]
  fn a_file_drag_reports_enter_over_and_drop() {
    let state = state();
    match event_from_script_event(&state, event("enter", &["/tmp/a"])) {
      Some(DragDropEvent::Enter { paths, position }) => {
        assert_eq!(paths, [PathBuf::from("/tmp/a")]);
        assert_eq!(position, PhysicalPosition::new(10.0, 20.0));
      }
      other => panic!("expected Enter, got {other:?}"),
    }
    // A second enter is not a second gesture.
    assert!(event_from_script_event(&state, event("enter", &["/tmp/a"])).is_none());
    assert!(matches!(
      event_from_script_event(&state, event("over", &[])),
      Some(DragDropEvent::Over { .. })
    ));
    match event_from_script_event(&state, event("drop", &[])) {
      Some(DragDropEvent::Drop { paths, .. }) => assert_eq!(paths, [PathBuf::from("/tmp/a")]),
      other => panic!("expected Drop, got {other:?}"),
    }
    // The gesture is over, so nothing follows it.
    assert!(event_from_script_event(&state, event("over", &[])).is_none());
  }

  #[test]
  fn a_leave_only_reports_when_it_had_entered() {
    let state = state();
    assert!(event_from_script_event(&state, event("leave", &[])).is_none());
    event_from_script_event(&state, event("enter", &["/tmp/a"]));
    assert!(matches!(
      event_from_script_event(&state, event("leave", &[])),
      Some(DragDropEvent::Leave)
    ));
  }

  #[test]
  fn only_the_private_path_is_intercepted() {
    let report = Url::parse(&format!(
      "tauri://localhost{DRAG_DROP_BRIDGE_PATH}?payload=%7B%22type%22%3A%22enter%22%2C%22x%22%3A1%2C%22y%22%3A2%7D"
    ))
    .unwrap();
    let event = intercepts(&report).expect("the private path is intercepted");
    let event = event.expect("the payload parses");
    assert_eq!(event.kind, "enter");
    assert_eq!((event.x, event.y), (1.0, 2.0));

    let other = Url::parse("tauri://localhost/index.html").unwrap();
    assert!(intercepts(&other).is_none());
  }

  #[test]
  fn a_malformed_payload_is_still_cancelled() {
    let report = Url::parse(&format!(
      "tauri://localhost{DRAG_DROP_BRIDGE_PATH}?payload=not-json"
    ))
    .unwrap();
    assert_eq!(intercepts(&report), Some(None));
  }

  #[test]
  fn the_native_half_records_the_paths_the_preload_resolved() {
    let state = state();
    let handler = TauriElectronDragHandler::new(state.clone());
    handler.on_drag_enter(vec![PathBuf::from("/tmp/a")]);
    assert!(state.lock().unwrap().native_entered);
    handler.on_drag_enter(Vec::new());
    assert!(!state.lock().unwrap().native_entered);
  }
}
