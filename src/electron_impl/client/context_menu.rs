// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! Context-menu policy for the webview.
//!
//! The reference creates Chrome-style browsers, so the model handed to
//! `on_before_context_menu` is Chrome's own page context menu — back/forward/reload, save
//! as, print, translate, view page source, "Search the web for…", open link in a new
//! tab/window/incognito window and Inspect — and the handler filters it down to the entries
//! that mean something inside an application window, matching each by its IDC id.
//!
//! Electron hands the page's parameters to `webContents.on('context-menu')` and lets the
//! embedder build the menu, so there is nothing to filter: the *reduced* template is built
//! directly, and the policy is which entries it may contain. The bridge builds that
//! template in TypeScript; this module is the policy it implements, and the reason each
//! entry is excluded lives here rather than in a comment next to the code that omits it.
//!
//! Everything not listed is kept, which covers what an app genuinely wants: undo/redo,
//! cut/copy/paste (including paste as plain text), delete, select all, the spellcheck
//! suggestions and add-to-dictionary, emoji, and the copy-link-address / copy-image family.

use serde::{Deserialize, Serialize};

/// One entry of the template the bridge builds.
///
/// The shape is Electron's `MenuItemConstructorOptions` minus the fields a page context menu
/// never needs, plus `command`: Electron's template has no command name of its own, so the
/// bridge names the entry and maps that name to a `click` handler.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ContextMenuItem {
  /// Electron's `type`: `normal`, `separator`, `checkbox`, `radio` or `submenu`.
  #[serde(rename = "type", skip_serializing_if = "Option::is_none", default)]
  pub(crate) kind: Option<String>,
  /// Electron's `role`, when the entry is one of Electron's own.
  #[serde(skip_serializing_if = "Option::is_none", default)]
  pub(crate) role: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none", default)]
  pub(crate) label: Option<String>,
  /// The bridge's name for this entry, which is what the policy below matches.
  #[serde(skip_serializing_if = "Option::is_none", default)]
  pub(crate) command: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none", default)]
  pub(crate) enabled: Option<bool>,
  #[serde(skip_serializing_if = "Option::is_none", default)]
  pub(crate) submenu: Option<Vec<ContextMenuItem>>,
}

impl ContextMenuItem {
  /// A separator entry.
  pub(crate) fn separator() -> Self {
    Self {
      kind: Some("separator".to_owned()),
      ..Default::default()
    }
  }

  /// A plain entry the bridge named.
  pub(crate) fn command(command: &str) -> Self {
    Self {
      kind: Some("normal".to_owned()),
      command: Some(command.to_owned()),
      ..Default::default()
    }
  }

  /// Whether this entry draws as a rule rather than as an action.
  fn is_separator(&self) -> bool {
    self.kind.as_deref() == Some("separator")
  }
}

/// Entries that navigate, print, save, or hand the page to a web service.
///
/// None of them belong in an application window, and most of them lead somewhere the app has
/// no control over. The names are the bridge's; the comments group them the way the
/// reference groups its IDC tables.
const BROWSER_ONLY_COMMANDS: &[&str] = &[
  // Navigation and page lifecycle.
  "back",
  "forward",
  "reload",
  "reload-bypassing-cache",
  "reload-clearing-cache",
  "reload-frame",
  // Saving, printing, and looking behind the page.
  "save-page",
  "print",
  "print-without-preview",
  "view-source",
  "view-frame-source",
  "save-link-as",
  "save-image-as",
  "save-media-as",
  "save-video-frame-as",
  // Opening a browser surface the app does not own.
  "open-link-new-tab",
  "open-link-new-window",
  "open-link-incognito",
  "open-link-in-profile",
  "open-link-bookmark-app",
  "open-link-split-view",
  "open-image-new-tab",
  "open-original-image-new-tab",
  "open-audio-video-new-tab",
  "go-to-url",
  "open-in-reading-mode",
  "add-link-to-reading-list",
  // Sending the page, a selection, or a frame off to a web service.
  "translate",
  "partial-translate",
  "search-web-for",
  "search-web-for-new-tab",
  "search-web-for-image",
  "search-web-for-video-frame",
  "search-lens-for-image",
  "search-lens-for-video-frame",
  "lens-overlay",
  "lens-region-search",
  "web-region-search",
  "sharing-submenu",
  "generate-qr-code",
  "route-media",
];

/// Entries that open DevTools. Kept when the webview enables devtools, removed otherwise.
const DEVTOOLS_COMMANDS: &[&str] = &[
  "inspect",
  "inspect-with-devtools",
  "inspect-background-page",
  "toggle-devtools",
  "devtools",
  "devtools-console",
  "devtools-devices",
];

/// The context-menu policy of one webview, mirroring `TauriCefContextMenuHandler`.
///
/// Nothing in this crate calls it: the bridge builds the template in TypeScript and never
/// asks the Rust side about it, so the policy has one implementation there and this is the
/// specification it is held to. It is kept compiled and tested rather than described in a
/// comment, because the two copies have to agree and only one of them can be unit-tested.
pub(crate) struct TauriElectronContextMenuHandler {
  devtools_enabled: bool,
}

impl TauriElectronContextMenuHandler {
  pub(crate) fn new(devtools_enabled: bool) -> Self {
    Self { devtools_enabled }
  }

  /// Filters `template` down to what an application window may offer.
  ///
  /// Removing an entry the menu does not carry is a no-op, so the whole policy applies to
  /// every menu without first asking which kind of menu it is.
  ///
  /// An empty template is left empty on purpose: Electron then shows no menu at all, which
  /// is the right outcome for a menu with nothing in it.
  pub(crate) fn on_before_context_menu(&self, template: &mut Vec<ContextMenuItem>) {
    template.retain(|item| {
      item
        .command
        .as_deref()
        .is_none_or(|command| !BROWSER_ONLY_COMMANDS.contains(&command))
    });
    if !self.devtools_enabled {
      template.retain(|item| {
        item
          .command
          .as_deref()
          .is_none_or(|command| !DEVTOOLS_COMMANDS.contains(&command))
      });
    }
    remove_redundant_separators(template);
  }
}

/// Drops the separators the removals leave behind: a menu must not open or end with one, and
/// two in a row draw as a double rule.
///
/// The loop starts as if a separator had just been seen, which is what drops the leading
/// ones. Every iteration advances on an entry that was *not* removed, and a removal holds the
/// index still because the entries after it shifted down into that slot. The reference is
/// explicit about why the index must never be held by a removal that did not happen: a menu
/// model that refuses a removal does not shrink, so retrying it spins the UI thread. A `Vec`
/// cannot refuse, but the discipline is kept so the two implementations stay comparable.
fn remove_redundant_separators(template: &mut Vec<ContextMenuItem>) {
  let mut previous_was_separator = true;
  let mut index = 0;
  while index < template.len() {
    if template[index].is_separator() {
      if previous_was_separator {
        template.remove(index);
        continue;
      }
      previous_was_separator = true;
    } else {
      previous_was_separator = false;
    }
    index += 1;
  }

  while template.last().is_some_and(ContextMenuItem::is_separator) {
    template.pop();
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn template(commands: &[&str]) -> Vec<ContextMenuItem> {
    commands
      .iter()
      .map(|command| {
        if *command == "-" {
          ContextMenuItem::separator()
        } else {
          ContextMenuItem::command(command)
        }
      })
      .collect()
  }

  fn names(template: &[ContextMenuItem]) -> Vec<String> {
    template
      .iter()
      .map(|item| item.command.clone().unwrap_or_else(|| "-".to_owned()))
      .collect()
  }

  #[test]
  fn browser_only_entries_are_removed() {
    let mut menu = template(&["undo", "reload", "copy", "print", "select-all"]);
    TauriElectronContextMenuHandler::new(true).on_before_context_menu(&mut menu);
    assert_eq!(names(&menu), ["undo", "copy", "select-all"]);
  }

  #[test]
  fn devtools_entries_follow_the_webview_attribute() {
    let mut enabled = template(&["copy", "inspect"]);
    TauriElectronContextMenuHandler::new(true).on_before_context_menu(&mut enabled);
    assert_eq!(names(&enabled), ["copy", "inspect"]);

    let mut disabled = template(&["copy", "inspect"]);
    TauriElectronContextMenuHandler::new(false).on_before_context_menu(&mut disabled);
    assert_eq!(names(&disabled), ["copy"]);
  }

  #[test]
  fn leading_trailing_and_doubled_separators_are_dropped() {
    let mut menu = template(&["-", "undo", "-", "-", "copy", "-"]);
    TauriElectronContextMenuHandler::new(true).on_before_context_menu(&mut menu);
    assert_eq!(names(&menu), ["undo", "-", "copy"]);
  }

  #[test]
  fn an_empty_menu_stays_empty() {
    let mut menu = template(&["reload", "print"]);
    TauriElectronContextMenuHandler::new(true).on_before_context_menu(&mut menu);
    assert!(menu.is_empty());
  }

  #[test]
  fn the_kept_family_survives() {
    let mut menu = template(&[
      "undo",
      "redo",
      "-",
      "cut",
      "copy",
      "paste",
      "paste-and-match-style",
      "delete",
      "select-all",
      "-",
      "copy-link-address",
      "copy-image",
    ]);
    TauriElectronContextMenuHandler::new(true).on_before_context_menu(&mut menu);
    assert_eq!(
      names(&menu),
      [
        "undo",
        "redo",
        "-",
        "cut",
        "copy",
        "paste",
        "paste-and-match-style",
        "delete",
        "select-all",
        "-",
        "copy-link-address",
        "copy-image",
      ]
    );
  }
}
