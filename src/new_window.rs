//! Native Chromium popups without Tauri's process-local WebView2/WebKit handles.
//!
//! Register with [`ElectronWebviewDispatcher::on_new_window`]. In a `Create`
//! callback build the destination with [`with_opener`], then return its dispatcher.
//! Chromium supplies the actual guest: no replacement page or emulated WindowProxy.

#[cfg(feature = "tauri")]
use crate::runtime::{create_webview_err, IPC_TIMEOUT};
use crate::{ElectronWebviewDispatcher, ElectronWindowDispatcher};
use std::{cell::RefCell, rc::Rc, sync::Weak};
use tauri_runtime::Result;
use url::Url;

pub(crate) type NewWindowHandler = dyn Fn(Url, NewWindowFeatures) -> NewWindowResponse + Send;

/// A one-request capability. It cannot link an unrelated runtime or later popup.
#[derive(Clone, Debug)]
pub struct NewWindowOpener {
  pub(crate) request: u64,
  pub(crate) ipc: Weak<crate::rpc_runtime::Ipc>,
}

/// Browser-requested window properties. Geometry is in logical pixels.
#[derive(Clone, Debug)]
pub struct NewWindowFeatures {
  pub frame_name: String,
  pub features: String,
  pub disposition: String,
  pub position: Option<tauri_runtime::dpi::LogicalPosition<f64>>,
  pub size: Option<tauri_runtime::dpi::LogicalSize<f64>>,
  pub(crate) opener: NewWindowOpener,
}

impl NewWindowFeatures {
  /// Associate the next window built in [`with_opener`] with this native request.
  pub fn opener(&self) -> NewWindowOpener {
    self.opener.clone()
  }
}

/// Decision for `window.open`, target links, and target form submissions.
pub enum NewWindowResponse {
  Allow,
  Deny,
  /// A window built using this request's [`with_opener`].
  Create {
    window: ElectronWindowDispatcher,
  },
}

thread_local! {
  static OPENER: RefCell<Option<NewWindowOpener>> = const { RefCell::new(None) };
  static WEBVIEW: RefCell<Option<ElectronWebviewDispatcher>> = const { RefCell::new(None) };
}

/// Build a destination using the ordinary Tauri builder inside `build`.
/// Only the next window builder on this thread consumes the opener. Return it with
/// `NewWindowResponse::create`; Chromium retains the native browsing context.
pub fn with_opener<R>(opener: NewWindowOpener, build: impl FnOnce() -> R) -> R {
  struct Restore(Option<NewWindowOpener>);
  impl Drop for Restore {
    fn drop(&mut self) {
      OPENER.with(|slot| slot.replace(self.0.take()));
    }
  }
  let _restore = Restore(OPENER.with(|slot| slot.replace(Some(opener))));
  build()
}

pub(crate) fn take_opener() -> Option<NewWindowOpener> {
  OPENER.with(|slot| slot.borrow_mut().take())
}

pub(crate) fn with_webview<R>(webview: ElectronWebviewDispatcher, task: impl FnOnce() -> R) -> R {
  struct Restore(Option<ElectronWebviewDispatcher>);
  impl Drop for Restore {
    fn drop(&mut self) {
      WEBVIEW.with(|slot| slot.replace(self.0.take()));
    }
  }
  let _restore = Restore(WEBVIEW.with(|slot| slot.replace(Some(webview))));
  task()
}

impl ElectronWebviewDispatcher {
  /// Register a synchronous UI-thread decision. Default is `Allow`.
  /// Build `Create` destinations inside `with_opener(features.opener(), || ...)`.
  /// Do not wait for page execution in this callback: Chromium has not yet
  /// created the guest, just as with a native WebView new-window callback.
  pub fn on_new_window<F>(&self, handler: F) -> Result<()>
  where
    F: Fn(Url, NewWindowFeatures) -> NewWindowResponse + Send + 'static,
  {
    let id = self.id;
    self.context.on_main_thread(move |context| {
      context
        .new_window_handlers
        .borrow_mut()
        .insert(id, Rc::new(handler));
    })
  }
}

#[cfg(feature = "tauri")]
/// Install the Electron callback on an ordinary Tauri webview (no upstream patch).
pub fn on_new_window<F>(
  webview: &tauri::Webview<crate::ElectronRuntime>,
  handler: F,
) -> tauri::Result<()>
where
  F: Fn(Url, NewWindowFeatures) -> NewWindowResponse + Send + 'static,
{
  let (tx, rx) = std::sync::mpsc::sync_channel(1);
  webview.run_on_main_thread(move || {
    let result = WEBVIEW
      .with(|slot| slot.borrow().clone())
      .ok_or_else(|| create_webview_err("Electron webview route is unavailable".into()))
      .and_then(|webview| webview.on_new_window(handler));
    let _ = tx.send(result);
  })?;
  rx.recv_timeout(IPC_TIMEOUT)
    .map_err(|error| std::io::Error::other(error.to_string()))?
    .map_err(Into::into)
}

#[cfg(feature = "tauri")]
impl NewWindowResponse {
  /// Select the Tauri window built with this request's opener.
  pub fn create(window: &tauri::WebviewWindow<crate::ElectronRuntime>) -> tauri::Result<Self> {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    window.run_on_main_thread(move || {
      let result = WEBVIEW
        .with(|slot| slot.borrow().clone())
        .ok_or_else(|| create_webview_err("Electron webview route is unavailable".into()))
        .and_then(|webview| webview.context.window_for_webview(webview.id))
        .map(|window| Self::Create { window });
      let _ = tx.send(result);
    })?;
    rx.recv_timeout(IPC_TIMEOUT)
      .map_err(|error| std::io::Error::other(error.to_string()))?
      .map_err(Into::into)
  }
}
