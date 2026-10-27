//! Access the authenticated client of an already running Electron main process.

/// A shared client for the current runtime, not a new Electron process.
pub type ElectronBridge = std::sync::Arc<electron_rs::Electron>;

impl crate::ElectronWebviewDispatcher {
  /// Share this webview's existing main-process client.
  pub fn main_bridge(&self) -> ElectronBridge {
    self.context.ipc.client()
  }
}

/// Resolve an ordinary Tauri webview to its existing Electron client.
///
/// Call from async application code; no renderer JavaScript or global runtime
/// lookup is involved. The returned client supports `node_in_main` and
/// `electron_in_main`, including streamed output callbacks.
#[cfg(feature = "tauri")]
pub async fn main_bridge(
  webview: &tauri::Webview<crate::ElectronRuntime>,
) -> tauri::Result<ElectronBridge> {
  let (sender, receiver) = tokio::sync::oneshot::channel();
  webview.run_on_main_thread(move || {
    let _ = sender.send(crate::new_window::current_main_bridge());
  })?;
  let bridge = tokio::time::timeout(crate::runtime::IPC_TIMEOUT, receiver)
    .await
    .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "Electron webview route timed out"))?
    .map_err(|_| std::io::Error::new(std::io::ErrorKind::BrokenPipe, "Electron webview route closed"))?;
  bridge.ok_or_else(|| std::io::Error::other("Electron webview route is unavailable").into())
}
