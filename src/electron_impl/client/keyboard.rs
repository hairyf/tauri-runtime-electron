// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! Blocks the DevTools keyboard shortcuts when the webview disabled DevTools.
//!
//! The reference's `KeyboardHandler` exists because a Chrome-style browser keeps its whole
//! accelerator table live: F12 and the inspect chord open DevTools whatever the embedder
//! asked for, so the runtime swallows them in `on_pre_key_event`. Electron has the same
//! exposure through `webContents.on('before-input-event')`, where `event.preventDefault()`
//! is the whole answer — there is no `is_keyboard_shortcut` out-parameter to set.
//!
//! The bridge already swallows F12. This module is the policy it applies, extended with the
//! inspect chord, so both halves of the rule live in one place instead of one in TypeScript
//! and one in Rust.

use serde_json::Value;

/// The parts of Electron's `before-input-event` input this policy reads.
///
/// Electron reports a DOM-shaped input: `type` is `keyDown`/`keyUp`/`char`, `key` is the
/// printable value (`"i"`, `"F12"`), `code` is the physical key (`"KeyI"`, `"F12"`), and
/// the four modifier booleans are separate fields.
#[derive(Clone, Debug, Default)]
pub(crate) struct Input {
  pub(crate) kind: String,
  pub(crate) key: String,
  pub(crate) code: String,
  pub(crate) control: bool,
  pub(crate) shift: bool,
  pub(crate) meta: bool,
  pub(crate) alt: bool,
}

impl Input {
  /// Reads one `before-input-event` payload.
  pub(crate) fn from_value(value: &Value) -> Self {
    let flag = |name: &str| value[name].as_bool().unwrap_or(false);
    Self {
      kind: value["type"].as_str().unwrap_or_default().to_owned(),
      key: value["key"].as_str().unwrap_or_default().to_owned(),
      code: value["code"].as_str().unwrap_or_default().to_owned(),
      control: flag("control"),
      shift: flag("shift"),
      meta: flag("meta"),
      alt: flag("alt"),
    }
  }

  /// Whether this input is a key press rather than a release or a character event.
  ///
  /// CEF's equivalent is `KEYEVENT_RAWKEYDOWN`; Electron reports the same moment as
  /// `keyDown`, and `rawKeyDown` is accepted too so a bridge that forwards Chromium's own
  /// spelling is not silently ignored.
  pub(super) fn is_key_down(&self) -> bool {
    matches!(self.kind.as_str(), "keyDown" | "rawKeyDown")
  }

  /// Whether the physical key is `name`, in either spelling Electron reports.
  pub(super) fn is_key(&self, name: &str) -> bool {
    self.code.eq_ignore_ascii_case(name) || self.key.eq_ignore_ascii_case(name)
  }
}

/// The DevTools shortcut policy of one webview, mirroring `TauriCefKeyboardHandler`.
pub(crate) struct TauriElectronKeyboardHandler {
  devtools_enabled: bool,
}

impl TauriElectronKeyboardHandler {
  pub(crate) fn new(devtools_enabled: bool) -> Self {
    Self { devtools_enabled }
  }

  /// Whether the webview swallows `input` before the renderer sees it.
  ///
  /// Only a key press is considered, and only while DevTools are disabled: a webview that
  /// allows DevTools wants the shortcuts to work, and the reference does not touch them.
  pub(crate) fn blocks(&self, input: &Input) -> bool {
    if self.devtools_enabled || !input.is_key_down() {
      return false;
    }

    // F12 opens DevTools on every platform.
    if input.is_key("F12") {
      return true;
    }

    // Ctrl+Shift+I is the inspect chord on Windows and Linux; macOS uses Cmd+Alt+I, which
    // is what Electron reports as `meta` + `alt`.
    if cfg!(target_os = "macos") {
      input.is_key("KeyI") && input.meta && input.alt
    } else {
      input.is_key("KeyI") && input.control && input.shift
    }
  }
}

/// Applies the policy to one `before-input-event` payload.
///
/// The bridge calls this before it dispatches the input to the renderer; `true` means the
/// input was swallowed and must not be forwarded.
pub(crate) fn blocks_devtools_shortcut(devtools_enabled: bool, input: &Value) -> bool {
  TauriElectronKeyboardHandler::new(devtools_enabled).blocks(&Input::from_value(input))
}

#[cfg(test)]
mod tests {
  use super::*;
  use serde_json::json;

  fn input(value: Value) -> Input {
    Input::from_value(&value)
  }

  #[test]
  fn f12_is_swallowed_when_devtools_are_disabled() {
    let handler = TauriElectronKeyboardHandler::new(false);
    assert!(handler.blocks(&input(
      json!({ "type": "keyDown", "key": "F12", "code": "F12" })
    )));
    assert!(handler.blocks(&input(json!({ "type": "rawKeyDown", "code": "F12" }))));
  }

  #[test]
  fn nothing_is_swallowed_when_devtools_are_enabled() {
    let handler = TauriElectronKeyboardHandler::new(true);
    assert!(!handler.blocks(&input(
      json!({ "type": "keyDown", "key": "F12", "code": "F12" })
    )));
  }

  #[test]
  fn only_a_key_press_is_considered() {
    let handler = TauriElectronKeyboardHandler::new(false);
    assert!(!handler.blocks(&input(
      json!({ "type": "keyUp", "key": "F12", "code": "F12" })
    )));
    assert!(!handler.blocks(&input(json!({ "type": "char", "key": "F12" }))));
  }

  #[test]
  fn the_inspect_chord_matches_the_platform() {
    let handler = TauriElectronKeyboardHandler::new(false);
    let chord = if cfg!(target_os = "macos") {
      json!({ "type": "keyDown", "key": "i", "code": "KeyI", "meta": true, "alt": true })
    } else {
      json!({ "type": "keyDown", "key": "i", "code": "KeyI", "control": true, "shift": true })
    };
    assert!(handler.blocks(&input(chord)));

    let plain = json!({ "type": "keyDown", "key": "i", "code": "KeyI" });
    assert!(!handler.blocks(&input(plain)));
  }
}
