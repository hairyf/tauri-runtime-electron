// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! Developer tools for a webview.
//!
//! CEF's `devtools.rs` allocates the CDP message ids a remote debugging session
//! numbered its own requests with. Electron owns that numbering
//! (`webContents.openDevTools` / `devtools-protocol`), so all this module keeps
//! is the Tauri API: open, close, query.
//!
//! The F12/Ctrl+Shift+I binding is *not* here: Tauri's
//! `devtools_shortcut_script()` injects a page-level shortcut for browser
//! runtimes that do not have one, while Electron's window needs a native
//! `before-input-event` hook — see `electron-rs/assets/main.cjs`.

use crate::rpc;
use tauri_runtime::Result;

use crate::webview::ElectronWebviewDispatcher;

pub(crate) fn open(webview: &ElectronWebviewDispatcher) -> Result<()> {
  webview
    .webview_call(rpc::WebviewOpenDevtools {
      webview_id: webview.id,
    })
    .map(|_| ())
}

pub(crate) fn close(webview: &ElectronWebviewDispatcher) -> Result<()> {
  webview
    .webview_call(rpc::WebviewCloseDevtools {
      webview_id: webview.id,
    })
    .map(|_| ())
}

pub(crate) fn is_open(webview: &ElectronWebviewDispatcher) -> Result<bool> {
  webview.webview_call(rpc::WebviewIsDevtoolsOpen {
    webview_id: webview.id,
  })
}
