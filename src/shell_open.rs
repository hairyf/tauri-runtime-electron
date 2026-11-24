// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! Opening a URL or path through the Electron main process.
//!
//! Electron's `shell` hands the target to the operating system exactly as a
//! user's double click does. A child process spawned by the page instead cannot
//! activate a GUI window it did not create on Windows, which leaves the shell in
//! front of the application the user asked for.

use crate::rpc;

/// Hands `url` to the operating system from the Electron main process.
///
/// Resolves once the main process accepted the handoff; a URL with no registered
/// handler reports that failure so the caller can keep its own window in front
/// instead of hiding behind an application that never opened.
pub async fn open_external(client: &crate::ElectronClient, url: &str) -> Result<(), String> {
    client
        .call(rpc::AppOpenExternal {
            url: url.to_owned(),
        })
        .await
        .map_err(|error| format!("OPEN_EXTERNAL_FAILED: {error}"))
}

/// Opens `path` with its registered application from the Electron main process.
///
/// Electron answers with an empty string on success and a message when the
/// platform refused the request; the bridge reports the empty string as no
/// message, so a refusal travels as an error here rather than as a silent no-op.
pub async fn open_path(client: &crate::ElectronClient, path: &str) -> Result<(), String> {
    let message = client
        .call(rpc::AppOpenPath {
            path: path.to_owned(),
        })
        .await
        .map_err(|error| format!("OPEN_PATH_FAILED: {error}"))?;
    match message {
        None => Ok(()),
        Some(message) => Err(format!("OPEN_PATH_FAILED: {message}")),
    }
}
