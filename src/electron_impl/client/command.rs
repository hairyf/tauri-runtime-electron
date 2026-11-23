// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! Chrome's accelerator table, filtered down to what an application window can
//! meaningfully do.
//!
//! The reference's `CommandHandler` exists because a Chrome-style browser keeps its whole
//! accelerator table live even when it is hosted as a child view with no browser UI: Ctrl+N
//! opens a real Chrome window next to the app's and Ctrl+T a tab in a window the app does
//! not own. CEF reports those as numeric command ids resolved from `IDC_*` names; Electron
//! reports no command ids at all, so the same policy is written as the *chords* those names
//! stand for. The groups and their reasons are the reference's; only the spelling of a
//! command changed.
//!
//! A chord the tables do not name runs unchanged: clipboard, find in page, text selection,
//! undo and redo, and reload are all things an app window legitimately uses. DevTools and
//! zoom are not in the group tables — they follow the webview's `devtools` and
//! `zoom_hotkeys_enabled` attributes, exactly as in the reference.
//!
//! # Command ids have no Electron counterpart
//!
//! `cef_id_for_command_id_name`, the four `is_chrome_*` predicates and the app-menu
//! enablement rule are all absent: they exist to answer questions about Chrome's own UI,
//! which an Electron guest does not have. What remains is the part that decides whether a
//! keystroke reaches the renderer.
//!
//! # A menu command cannot reach the application
//!
//! `menu.command` and `app.menuCommand` report a click on a menu the bridge built from
//! `window.setMenu`. `tauri-runtime` 3.0.0-alpha.5 has no `RunEvent` for a menu command, so
//! there is no portable way to hand it to the application; it is reported here and dropped
//! rather than being turned into an event of a shape the application never asked for.

use serde_json::Value;
use tauri_runtime::UserEvent;

use crate::{
  ChromeCommandGroup,
  electron_impl::client::{self, keyboard::Input},
  runtime::RuntimeContext,
};

/// One accelerator: a physical key and the exact modifier set it is pressed with.
///
/// The modifiers are compared exactly, so Ctrl+Shift+N never matches Ctrl+N. `key` holds
/// the DOM `code` spelling (`KeyN`, `Digit1`, `PageDown`), which is what Electron reports
/// as the physical key and is stable across keyboard layouts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Chord {
  key: &'static str,
  control: bool,
  shift: bool,
  alt: bool,
  meta: bool,
}

const fn ctrl(key: &'static str) -> Chord {
  Chord {
    key,
    control: true,
    shift: false,
    alt: false,
    meta: false,
  }
}

const fn ctrl_shift(key: &'static str) -> Chord {
  Chord {
    key,
    control: true,
    shift: true,
    alt: false,
    meta: false,
  }
}

const fn alt(key: &'static str) -> Chord {
  Chord {
    key,
    control: false,
    shift: false,
    alt: true,
    meta: false,
  }
}

impl Chord {
  /// Whether `input` is exactly this chord.
  fn matches(&self, input: &Input) -> bool {
    input.is_key(self.key)
      && input.control == self.control
      && input.shift == self.shift
      && input.alt == self.alt
      && input.meta == self.meta
  }
}

/// Commands that open a second window or drive a tab strip.
///
/// A browser keeps its whole accelerator table live even when it is hosted as a child view
/// with no browser UI, so Ctrl+N opens a real browser window next to the app's and Ctrl+T a
/// tab in a window the app does not own. An app window has no tab strip for the rest to act
/// on. The commands with no default accelerator — duplicate, restore-to-new-window, move tab,
/// show as tab — cannot be reached from the keyboard at all, so they name no chord here.
const WINDOW_AND_TAB_COMMANDS: &[Chord] = &[
  ctrl("KeyN"),
  ctrl_shift("KeyN"),
  ctrl("KeyT"),
  ctrl_shift("KeyT"),
  ctrl("Tab"),
  ctrl_shift("Tab"),
  ctrl("PageDown"),
  ctrl("PageUp"),
  ctrl("Digit1"),
  ctrl("Digit2"),
  ctrl("Digit3"),
  ctrl("Digit4"),
  ctrl("Digit5"),
  ctrl("Digit6"),
  ctrl("Digit7"),
  ctrl("Digit8"),
  ctrl("Digit9"),
  ctrl_shift("KeyA"),
];

/// Commands that treat the app's UI as a web document to be exported.
///
/// Printing, saving or viewing the source of a window hands the user the app's own bundled
/// markup, and Ctrl+O replaces that UI with a local document in the same webview.
/// `WebviewDispatch::print` still prints on request.
const DOCUMENT_COMMANDS: &[Chord] = &[
  ctrl("KeyP"),
  ctrl_shift("KeyP"),
  ctrl("KeyS"),
  ctrl("KeyU"),
  ctrl("KeyO"),
];

/// Commands that move focus into browser chrome the window does not have.
///
/// An app window has no omnibox, search box or bookmark bar, so these only take keyboard
/// focus somewhere the user cannot see; Home additionally navigates the webview away from
/// the app's own UI.
const BROWSER_CHROME_COMMANDS: &[Chord] = &[
  ctrl("KeyL"),
  ctrl("KeyE"),
  ctrl("KeyK"),
  ctrl_shift("KeyB"),
  ctrl("F6"),
  alt("Home"),
  alt("KeyE"),
  alt("KeyF"),
];

/// Commands that open one of the browser's own profile-wide surfaces.
///
/// History, downloads, bookmarks, settings and the task manager load browser WebUI pages *in
/// place of the app's UI*, in the very webview the chord was pressed in, and expose the
/// browsing data of every webview sharing the session.
const BROWSER_SURFACE_COMMANDS: &[Chord] = &[
  ctrl("KeyH"),
  ctrl("KeyJ"),
  ctrl_shift("KeyO"),
  ctrl("KeyD"),
  ctrl_shift("KeyD"),
  ctrl_shift("Delete"),
  ctrl_shift("Backspace"),
  ctrl_shift("Escape"),
  ctrl("F1"),
  ctrl_shift("KeyM"),
];

/// Commands that walk the webview's session history.
///
/// A Tauri webview is navigated to the application's URL as its first document, so going
/// back from it lands on nothing the application owns. `WebviewDispatch::go_back` and
/// `go_forward` call the guest directly and never reach this policy.
const HISTORY_COMMANDS: &[Chord] = &[alt("ArrowLeft"), alt("ArrowRight")];

/// Commands that open DevTools, blocked only when the webview disabled them.
///
/// `keyboard.rs` blocks F12 and the inspect chord; these cover the rest of the table.
const DEVTOOLS_COMMANDS: &[Chord] = &[ctrl_shift("KeyI"), ctrl_shift("KeyJ"), ctrl_shift("KeyC")];

/// Commands that change the page zoom, blocked only when the webview set
/// `zoom_hotkeys_enabled` to false — which is what that attribute asks for, and note that it
/// **defaults to false**. `WebviewDispatch::set_zoom` still zooms on request.
const ZOOM_COMMANDS: &[Chord] = &[
  ctrl("Equal"),
  ctrl("NumpadAdd"),
  ctrl("Minus"),
  ctrl("NumpadSubtract"),
  ctrl("Digit0"),
  ctrl("Numpad0"),
];

/// The chords of one [`ChromeCommandGroup`].
///
/// The wildcard arm is unreachable — [`ChromeCommandGroup::ALL`] names every variant — but
/// the enum is `#[non_exhaustive]`, so a group a later release adds is left unblocked rather
/// than being swallowed by a guess.
fn group_commands(group: ChromeCommandGroup) -> &'static [Chord] {
  match group {
    ChromeCommandGroup::WindowAndTab => WINDOW_AND_TAB_COMMANDS,
    ChromeCommandGroup::Document => DOCUMENT_COMMANDS,
    ChromeCommandGroup::BrowserChrome => BROWSER_CHROME_COMMANDS,
    ChromeCommandGroup::BrowserSurface => BROWSER_SURFACE_COMMANDS,
    ChromeCommandGroup::History => HISTORY_COMMANDS,
  }
}

/// The chord that toggles the window's fullscreen state, which is the reference's
/// `IDC_FULLSCREEN`.
///
/// F11 on Windows and Linux, Ctrl+Cmd+F on macOS — the accelerators the reference resolves
/// `IDC_FULLSCREEN` to.
fn fullscreen_chord() -> Chord {
  if cfg!(target_os = "macos") {
    Chord {
      key: "KeyF",
      control: true,
      shift: false,
      alt: false,
      meta: true,
    }
  } else {
    Chord {
      key: "F11",
      control: false,
      shift: false,
      alt: false,
      meta: false,
    }
  }
}

/// The accelerator policy of one webview, mirroring `TauriCefCommandHandler`.
pub(crate) struct TauriElectronCommandHandler {
  devtools_enabled: bool,
  zoom_hotkeys_enabled: bool,
  allowed_chrome_commands: Vec<ChromeCommandGroup>,
}

impl TauriElectronCommandHandler {
  pub(crate) fn new(
    devtools_enabled: bool,
    zoom_hotkeys_enabled: bool,
    allowed_chrome_commands: Vec<ChromeCommandGroup>,
  ) -> Self {
    Self {
      devtools_enabled,
      zoom_hotkeys_enabled,
      allowed_chrome_commands,
    }
  }

  /// Whether the webview swallows `input`.
  ///
  /// A group the webview named in `allowed_chrome_commands` is skipped entirely, so its
  /// commands run the way they would in a browser.
  pub(crate) fn blocks(&self, input: &Input) -> bool {
    ChromeCommandGroup::ALL
      .iter()
      .filter(|group| !self.allowed_chrome_commands.contains(group))
      .any(|group| {
        group_commands(*group)
          .iter()
          .any(|chord| chord.matches(input))
      })
      || (!self.devtools_enabled && DEVTOOLS_COMMANDS.iter().any(|chord| chord.matches(input)))
      || (!self.zoom_hotkeys_enabled && ZOOM_COMMANDS.iter().any(|chord| chord.matches(input)))
  }

  /// Whether `input` is the fullscreen toggle.
  ///
  /// The reference answers `IDC_FULLSCREEN` before consulting the block tables, because
  /// fullscreen is not a command it swallows: it is one it *performs*, on the window rather
  /// than on the webview.
  pub(crate) fn toggles_fullscreen(&self, input: &Input) -> bool {
    fullscreen_chord().matches(input)
  }
}

/// Applies the accelerator policy to one `before-input-event` payload.
///
/// `true` means the input was swallowed and must not be forwarded to the renderer.
pub(crate) fn blocks(
  devtools_enabled: bool,
  zoom_hotkeys_enabled: bool,
  allowed_chrome_commands: &[ChromeCommandGroup],
  input: &Value,
) -> bool {
  TauriElectronCommandHandler::new(
    devtools_enabled,
    zoom_hotkeys_enabled,
    allowed_chrome_commands.to_vec(),
  )
  .blocks(&Input::from_value(input))
}

/// Toggles the fullscreen state of the window hosting a webview.
///
/// This is the reference's `Message::ToggleFullscreen`, performed here rather than posted to
/// the run loop: the state it flips lives in Electron, so it is read with `window.geometry`
/// and written with `window.fullscreen`.
pub(crate) fn toggle_fullscreen<T: UserEvent>(context: &RuntimeContext<T>, webview_id: u32) {
  let Some(window_id) = client::window_id_of::<T>(webview_id) else {
    log::debug!("ignoring a fullscreen toggle for the unknown webview {webview_id}");
    return;
  };
  let Some(serial) = client::window_serial(window_id) else {
    log::warn!("cannot toggle fullscreen: the runtime has no Electron serial for {window_id:?}");
    return;
  };

  let fullscreen = context
    .ipc
    .call(
      "window.geometry",
      serde_json::json!({ "windowId": serial }),
      crate::runtime::IPC_TIMEOUT,
    )
    .map(|geometry| geometry["fullscreen"].as_bool().unwrap_or(false))
    .unwrap_or(false);

  if let Err(error) = context.ipc.call(
    "window.fullscreen",
    serde_json::json!({ "windowId": serial, "fullscreen": !fullscreen }),
    crate::runtime::IPC_TIMEOUT,
  ) {
    log::warn!("failed to toggle the fullscreen state of window {serial}: {error}");
  }
}

/// A menu entry was clicked.
///
/// `menu.command` carries the window and the command id the entry was built with, and
/// `app.menuCommand` the command id of an application-menu entry. Neither can be delivered:
/// `tauri-runtime` 3.0.0-alpha.5 has no menu event in `RunEvent`, so the click is reported
/// and dropped instead of being invented as some other event.
pub(crate) fn on_menu_command<T: UserEvent>(_context: &RuntimeContext<T>, params: &Value) {
  log::debug!(
    "menu command {:?} was clicked on window {:?}",
    params["commandId"],
    params["windowId"]
  );
  crate::unsupported::report("client::command::on_menu_command");
}

#[cfg(test)]
mod tests {
  use super::*;
  use serde_json::json;

  fn input(value: Value) -> Input {
    Input::from_value(&value)
  }

  fn handler() -> TauriElectronCommandHandler {
    TauriElectronCommandHandler::new(false, false, Vec::new())
  }

  #[test]
  fn a_blocked_group_swallows_its_chords() {
    let handler = handler();
    assert!(handler.blocks(&input(
      json!({ "type": "keyDown", "code": "KeyN", "control": true })
    )));
    assert!(handler.blocks(&input(
      json!({ "type": "keyDown", "code": "KeyP", "control": true })
    )));
    assert!(handler.blocks(&input(
      json!({ "type": "keyDown", "code": "ArrowLeft", "alt": true })
    )));
  }

  #[test]
  fn an_allowed_group_runs_unchanged() {
    let handler =
      TauriElectronCommandHandler::new(false, false, vec![ChromeCommandGroup::Document]);
    assert!(!handler.blocks(&input(
      json!({ "type": "keyDown", "code": "KeyP", "control": true })
    )));
    // The other groups are still swallowed.
    assert!(handler.blocks(&input(
      json!({ "type": "keyDown", "code": "KeyN", "control": true })
    )));
  }

  #[test]
  fn the_modifier_set_has_to_match_exactly() {
    let handler = handler();
    // Ctrl+Shift+N is its own chord, and neither matches the other.
    assert!(handler.blocks(&input(
      json!({ "type": "keyDown", "code": "KeyN", "control": true, "shift": true })
    )));
    // Ctrl+Alt+N is not a command the tables name.
    assert!(!handler.blocks(&input(
      json!({ "type": "keyDown", "code": "KeyN", "control": true, "alt": true })
    )));
  }

  #[test]
  fn devtools_and_zoom_follow_their_attributes() {
    let disabled = handler();
    assert!(disabled.blocks(&input(
      json!({ "type": "keyDown", "code": "KeyI", "control": true, "shift": true })
    )));
    assert!(disabled.blocks(&input(
      json!({ "type": "keyDown", "code": "Equal", "control": true })
    )));

    let enabled = TauriElectronCommandHandler::new(true, true, Vec::new());
    assert!(!enabled.blocks(&input(
      json!({ "type": "keyDown", "code": "KeyI", "control": true, "shift": true })
    )));
    assert!(!enabled.blocks(&input(
      json!({ "type": "keyDown", "code": "Equal", "control": true })
    )));
  }

  #[test]
  fn an_unlisted_chord_runs_unchanged() {
    let handler = handler();
    assert!(!handler.blocks(&input(
      json!({ "type": "keyDown", "code": "KeyC", "control": true })
    )));
    assert!(!handler.blocks(&input(
      json!({ "type": "keyDown", "code": "KeyR", "control": true })
    )));
  }

  #[test]
  fn the_fullscreen_chord_is_recognised() {
    let handler = handler();
    let chord = if cfg!(target_os = "macos") {
      json!({ "type": "keyDown", "code": "KeyF", "control": true, "meta": true })
    } else {
      json!({ "type": "keyDown", "code": "F11" })
    };
    assert!(handler.toggles_fullscreen(&input(chord)));
  }
}
