pub use electron_rs::rpc::browser::Reservation as BrowserGuestReservation;

pub(crate) type OpenHandler = dyn Fn(String, String) + Send;

#[cfg(feature = "tauri")]
pub(crate) fn route<R: Send + 'static>(
  webview: &tauri::Webview<crate::ElectronRuntime>,
  task: impl FnOnce(crate::ElectronWebviewDispatcher) -> tauri_runtime::Result<R> + Send + 'static,
) -> tauri::Result<R> {
  let (tx, rx) = std::sync::mpsc::sync_channel(1);
  webview.run_on_main_thread(move || {
    let result = crate::new_window::WEBVIEW.with(|slot| slot.borrow().clone())
      .ok_or_else(|| crate::runtime::create_webview_err("Electron webview route is unavailable".into()))
      .and_then(task);
    let _ = tx.send(result);
  })?;
  rx.recv_timeout(crate::runtime::IPC_TIMEOUT)
    .map_err(|error| std::io::Error::other(error.to_string()))?
    .map_err(Into::into)
}

#[cfg(feature = "tauri")]
pub fn set_webview_trusted_origin(
  webview: &tauri::Webview<crate::ElectronRuntime>, origin: &str,
) -> tauri::Result<()> {
  let origin = origin.to_owned();
  route(webview, move |view| view.webview_call(electron_rs::rpc::browser::SetTrustedOrigin { webview_id: view.id, origin }))
}

#[cfg(feature = "tauri")]
/// Navigates to the exact target and switches trust only when its main frame commits.
/// Cancellation before commit preserves the previous authenticated binding.
pub fn navigate_webview_with_trusted_origin(
  webview: &tauri::Webview<crate::ElectronRuntime>, origin: &str, url: &str,
) -> tauri::Result<()> {
  let origin = origin.to_owned();
  let url = url.to_owned();
  route(webview, move |view| view.webview_call(electron_rs::rpc::browser::SetTrustedOriginAndNavigate { webview_id: view.id, origin, url }))
}

#[cfg(feature = "tauri")]
pub fn acquire_browser_guest(
  webview: &tauri::Webview<crate::ElectronRuntime>, workspace: &str,
) -> tauri::Result<BrowserGuestReservation> {
  let workspace = workspace.to_owned();
  route(webview, move |view| view.webview_call(electron_rs::rpc::browser::Acquire { webview_id: view.id, workspace }))
}

#[cfg(feature = "tauri")]
pub fn release_browser_guest(
  webview: &tauri::Webview<crate::ElectronRuntime>, lease: &str,
) -> tauri::Result<()> {
  let lease = lease.to_owned();
  route(webview, move |view| view.webview_call(electron_rs::rpc::browser::Release { webview_id: view.id, lease }))
}

#[cfg(feature = "tauri")]
pub fn on_browser_open_requested<F>(
  webview: &tauri::Webview<crate::ElectronRuntime>, handler: F,
) -> tauri::Result<()>
where F: Fn(String, String) + Send + 'static {
  route(webview, move |view| {
    let id = view.id;
    view.context.on_main_thread(move |context| {
      context.browser_open_handlers.borrow_mut().insert(id, std::rc::Rc::new(handler));
    })
  })
}
