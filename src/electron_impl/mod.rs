// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! The Electron backend's internals, mirroring `cef_impl/` file for file.
//!
//! The reference directory holds the pieces that only exist because a browser engine is
//! embedded in the process: the client handlers Chromium calls back into, the request
//! context and its preferences, the custom-scheme factory and the cookie manager. On the
//! Electron side the engine lives in another process and reaches this one as named events
//! over the JSON-RPC socket, so the same six responsibilities exist with the same names —
//! only the mechanism changes from a C callback to an event.
//!
//! # Why this whole tree allows dead code
//!
//! The reference's items are called by the engine: CEF invokes `on_title_change` because the
//! client is installed on the browser. Nothing invokes the Electron equivalent — the bridge
//! sends `webview.titleChanged` if and only if it implements the forward — so an item here is
//! reachable exactly when the bridge has a matching event or method. Several are complete and
//! tested while that forward does not exist yet (`webview.consoleMessage`,
//! `webview.dragDrop`, `session.setProxy`), and the module-level `allow` keeps the mirror
//! intact instead of deleting the reference's surface or littering it with per-item
//! attributes. Every such gap is documented on the item itself.

#![allow(dead_code)]

pub(crate) mod client;
pub(crate) mod cookie;
pub(crate) mod ipc;
pub(crate) mod preferences;
pub(crate) mod request_context;
pub(crate) mod request_handler;
