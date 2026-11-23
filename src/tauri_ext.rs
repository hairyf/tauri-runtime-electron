// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! Extension traits exposing Electron-specific APIs on [`tauri`] types.
//!
//! The traits are implemented for the statically typed
//! [`ElectronRuntime`](crate::ElectronRuntime) and for the type-erased
//! [`tauri::DynRuntime`]. With the latter, the methods fail with
//! [`tauri_runtime::Error::RuntimeTypeMismatch`] when the application is not running
//! on Electron.

use std::sync::Arc;

use tauri::{EventLoopMessage, Manager, Runtime, Webview, WebviewWindow};
use tauri_runtime::dynamic::{DynWebviewAttributes, DynWebviewDispatcher, DynWindowOpener};

use crate::{
  ChromeCommandGroup, ConsoleMessage, DevToolsProtocol, ElectronBrowserSettings,
  ElectronWebviewAttributes, ElectronWebviewDispatcher, FrameEvent, NewWindowOpener, RuntimeStyle,
};

type Result<T> = std::result::Result<T, tauri::Error>;

fn not_electron() -> tauri::Error {
  tauri_runtime::Error::RuntimeTypeMismatch(
    "the application is not running on the Electron runtime".into(),
  )
  .into()
}

/// Webview dispatchers that may expose the underlying [`ElectronWebviewDispatcher`].
pub trait AsElectronWebviewDispatcher {
  /// Returns the Electron webview dispatcher, if the runtime is Electron.
  fn as_electron_webview_dispatcher(&self) -> Option<&ElectronWebviewDispatcher<EventLoopMessage>>;
}

impl AsElectronWebviewDispatcher for ElectronWebviewDispatcher<EventLoopMessage> {
  fn as_electron_webview_dispatcher(&self) -> Option<&ElectronWebviewDispatcher<EventLoopMessage>> {
    Some(self)
  }
}

impl AsElectronWebviewDispatcher for DynWebviewDispatcher<EventLoopMessage> {
  fn as_electron_webview_dispatcher(&self) -> Option<&ElectronWebviewDispatcher<EventLoopMessage>> {
    self.downcast_ref()
  }
}

/// Window openers that may expose the Electron [`NewWindowOpener`].
///
/// Lets a new window handler read the opener's source URL regardless of the runtime
/// generic in use:
///
/// ```rust,no_run
/// use tauri::{WebviewUrl, WebviewWindowBuilder, webview::NewWindowResponse};
/// use tauri_runtime_electron::AsElectronWindowOpener;
///
/// tauri::Builder::default()
///   .runtime(tauri_runtime_electron::Electron::default())
///   .setup(|app| {
///     WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
///       .on_new_window(|url, features| {
///         if let Some(opener) = features.opener().as_electron_window_opener() {
///           println!("{url} was opened by {:?}", opener.source_url());
///         }
///         NewWindowResponse::Allow
///       })
///       .build()?;
///     Ok(())
///   });
/// ```
pub trait AsElectronWindowOpener {
  /// Returns the Electron window opener, `None` when the opener belongs to another
  /// runtime.
  fn as_electron_window_opener(&self) -> Option<&NewWindowOpener>;
}

impl AsElectronWindowOpener for NewWindowOpener {
  fn as_electron_window_opener(&self) -> Option<&NewWindowOpener> {
    Some(self)
  }
}

impl AsElectronWindowOpener for DynWindowOpener {
  fn as_electron_window_opener(&self) -> Option<&NewWindowOpener> {
    self.downcast_ref()
  }
}

/// Runtime webview attributes that may expose the [`ElectronWebviewAttributes`].
pub trait AsElectronWebviewAttributes {
  /// Returns the Electron attributes, `None` when the attributes belong to another
  /// runtime.
  fn as_electron_webview_attributes_mut(&mut self) -> Option<&mut ElectronWebviewAttributes>;
}

impl AsElectronWebviewAttributes for ElectronWebviewAttributes {
  fn as_electron_webview_attributes_mut(&mut self) -> Option<&mut ElectronWebviewAttributes> {
    Some(self)
  }
}

impl AsElectronWebviewAttributes for DynWebviewAttributes {
  fn as_electron_webview_attributes_mut(&mut self) -> Option<&mut ElectronWebviewAttributes> {
    // Unlike a concrete foreign attribute set, an empty slot is *filled* with the
    // Electron defaults: the builder has not committed to another runtime yet.
    self.get_or_default()
  }
}

/// Modifies the Electron attributes of a webview builder, if the builder's
/// attributes are not of another runtime.
fn with_electron_webview_attributes<A: AsElectronWebviewAttributes>(
  attributes: &mut A,
  f: impl FnOnce(&mut ElectronWebviewAttributes),
) {
  match attributes.as_electron_webview_attributes_mut() {
    Some(attributes) => f(attributes),
    None => log::warn!(
      "ignoring the Electron webview attributes: attributes of another runtime were already set on the webview builder"
    ),
  }
}

/// Electron-specific APIs of [`tauri::Webview`] and [`tauri::WebviewWindow`].
pub trait WebviewElectronExt {
  /// Send a message to the DevTools agent. The message should be a UTF-8 encoded JSON
  /// string following the Chrome DevTools Protocol format.
  ///
  /// Callers share one native request identifier space on this browser, so the
  /// message's `id` must come from
  /// [`allocate_devtools_message_id`](crate::allocate_devtools_message_id).
  /// A hardcoded or self-incremented `id` can collide with a request another
  /// caller already sent, which consumes that producer's
  /// [`DevToolsProtocol::MethodResult`]. The runtime's own requests are issued
  /// from a reserved range the public allocator never returns, so they cannot
  /// be consumed this way.
  ///
  /// # Examples
  ///
  /// ```rust,no_run
  /// use tauri::Manager;
  /// use tauri_runtime_electron::{WebviewElectronExt, allocate_devtools_message_id};
  ///
  /// tauri::Builder::default()
  ///   .runtime(tauri_runtime_electron::Electron::default())
  ///   .setup(|app| {
  ///     let webview = app.get_webview_window("main").unwrap();
  ///     // Enable Page domain to receive page lifecycle events
  ///     let message_id = allocate_devtools_message_id()?;
  ///     let msg = format!(r#"{{"id":{message_id},"method":"Page.enable","params":{{}}}}"#);
  ///     webview.send_dev_tools_message(msg.as_bytes())?;
  ///     Ok(())
  ///   });
  /// ```
  fn send_dev_tools_message(&self, message: &[u8]) -> Result<()>;

  /// Register a callback to receive DevTools protocol messages. Messages include
  /// both method results and events from the DevTools agent.
  ///
  /// The callback observes the whole browser, including requests the runtime and
  /// other callers sent. Match [`DevToolsProtocol::MethodResult`] against an
  /// identifier obtained from
  /// [`allocate_devtools_message_id`](crate::allocate_devtools_message_id)
  /// instead of assuming every result belongs to this observer.
  ///
  /// It is scoped to this webview's own native browser, so a popup the runtime
  /// opened as a window of its own is a separate browser whose protocol traffic —
  /// its page content, its network activity and its dialog messages — is never
  /// reported here; observe popups through
  /// [`Webview::popups`](crate::Webview::popups).
  ///
  /// # Examples
  ///
  /// ```rust,no_run
  /// use tauri::Manager;
  /// use tauri_runtime_electron::{DevToolsProtocol, WebviewElectronExt, allocate_devtools_message_id};
  ///
  /// tauri::Builder::default()
  ///   .runtime(tauri_runtime_electron::Electron::default())
  ///   .setup(|app| {
  ///     let webview = app.get_webview_window("main").unwrap();
  ///     let message_id = allocate_devtools_message_id()?;
  ///     webview.on_dev_tools_protocol(move |protocol| {
  ///       match protocol {
  ///         DevToolsProtocol::Message(msg) => {
  ///           if let Ok(s) = std::str::from_utf8(&msg) {
  ///             println!("DevTools message: {}", s);
  ///           }
  ///         }
  ///         DevToolsProtocol::Event { method, params } => {
  ///           println!("DevTools event: {} {:?}", method, params);
  ///         }
  ///         // Only this result answers the request sent below.
  ///         DevToolsProtocol::MethodResult { message_id: id, success, .. } if id == message_id => {
  ///           println!("Page.enable success={}", success);
  ///         }
  ///         DevToolsProtocol::MethodResult { .. } => {}
  ///       }
  ///     })?;
  ///     let msg = format!(r#"{{"id":{message_id},"method":"Page.enable","params":{{}}}}"#);
  ///     webview.send_dev_tools_message(msg.as_bytes())?;
  ///     Ok(())
  ///   });
  /// ```
  fn on_dev_tools_protocol<F: Fn(DevToolsProtocol) + Send + Sync + 'static>(
    &self,
    f: F,
  ) -> Result<()>;

  /// Executes a closure with the Electron webview handle, on the runtime's own
  /// thread.
  ///
  /// See [`crate::Webview`] for the native state it exposes, which is sampled
  /// immediately before the closure runs and is not refreshed afterwards.
  fn with_electron_webview<F: FnOnce(&crate::Webview) + Send + 'static>(&self, f: F) -> Result<()>;
}

impl<R: Runtime> WebviewElectronExt for Webview<R>
where
  R::WebviewDispatcher: AsElectronWebviewDispatcher,
{
  fn send_dev_tools_message(&self, message: &[u8]) -> Result<()> {
    self
      .dispatcher()
      .as_electron_webview_dispatcher()
      .ok_or_else(not_electron)?
      .send_dev_tools_message(message)
      .map_err(Into::into)
  }

  fn on_dev_tools_protocol<F: Fn(DevToolsProtocol) + Send + Sync + 'static>(
    &self,
    f: F,
  ) -> Result<()> {
    self
      .dispatcher()
      .as_electron_webview_dispatcher()
      .ok_or_else(not_electron)?
      .on_dev_tools_protocol(f)
      .map_err(Into::into)
  }

  fn with_electron_webview<F: FnOnce(&crate::Webview) + Send + 'static>(&self, f: F) -> Result<()> {
    if self.dispatcher().as_electron_webview_dispatcher().is_none() {
      return Err(not_electron());
    }
    self.with_webview(move |webview| {
      if let Some(webview) = webview.downcast_ref::<crate::Webview>() {
        f(webview)
      }
    })
  }
}

impl<R: Runtime> WebviewElectronExt for WebviewWindow<R>
where
  R::WebviewDispatcher: AsElectronWebviewDispatcher,
{
  fn send_dev_tools_message(&self, message: &[u8]) -> Result<()> {
    self.as_ref().send_dev_tools_message(message)
  }

  fn on_dev_tools_protocol<F: Fn(DevToolsProtocol) + Send + Sync + 'static>(
    &self,
    f: F,
  ) -> Result<()> {
    self.as_ref().on_dev_tools_protocol(f)
  }

  fn with_electron_webview<F: FnOnce(&crate::Webview) + Send + 'static>(&self, f: F) -> Result<()> {
    self.as_ref().with_electron_webview(f)
  }
}

/// Electron-specific APIs of [`tauri::WebviewWindowBuilder`].
pub trait WebviewWindowBuilderElectronExt {
  /// Sets the browser runtime style.
  ///
  /// See [`RuntimeStyle`] for more information. Electron has no Alloy/Chrome
  /// distinction: the value is stored, and a request for
  /// [`RuntimeStyle::Chrome`] is reported as unsupported rather than silently
  /// ignored.
  #[must_use]
  fn browser_runtime_style(self, style: RuntimeStyle) -> Self;

  /// Observes native frame lifecycle events for main and child frames.
  ///
  /// The callback runs on the runtime's own thread. It must return promptly and
  /// must not wait for an event-loop operation. This observer does not replace the
  /// navigation policy configured by `on_navigation`. It is scoped to this
  /// webview's own native browser, so a popup the runtime opened as a window of
  /// its own is a separate browser that is never reported here — observe popups
  /// through [`Webview::popups`](crate::Webview::popups).
  #[must_use]
  fn on_frame_event<F: Fn(FrameEvent) + Send + Sync + 'static>(self, handler: F) -> Self;

  /// Observes the messages the renderer writes to the JavaScript console,
  /// without DevTools having to be open.
  ///
  /// The callback runs on the runtime's own thread, so it must return promptly and
  /// must not wait for an event-loop operation. Observing a message does not
  /// suppress it: the renderer logs it as it normally would. It is scoped to this
  /// webview's own native browser, so neither a popup's output nor that of a
  /// DevTools window opened on this webview is reported here.
  #[must_use]
  fn on_console_message<F: Fn(ConsoleMessage) + Send + Sync + 'static>(self, handler: F) -> Self;

  /// Keeps the named families of Chrome commands rather than swallowing them.
  ///
  /// The reference runtime has to swallow Chrome's accelerator table because a
  /// Chrome style browser keeps it live even when hosted as a child view with no
  /// browser UI. Electron binds no such table, so there is nothing to swallow and
  /// nothing to keep: the value is stored and reported as unsupported when it is
  /// non-empty. Calling this more than once replaces the previous list.
  ///
  /// DevTools and zoom accelerators are not covered here: they follow
  /// `WebviewAttributes::devtools` and `WebviewAttributes::zoom_hotkeys_enabled`.
  ///
  /// ```no_run
  /// # use tauri_runtime_electron::{AsElectronWebviewAttributes, ChromeCommandGroup};
  /// # fn f<R, M>(builder: tauri::WebviewWindowBuilder<'_, R, M>)
  /// # where
  /// #   R: tauri::Runtime,
  /// #   M: tauri::Manager<R>,
  /// #   R::RuntimeWebviewAttributes: AsElectronWebviewAttributes,
  /// # {
  /// use tauri_runtime_electron::WebviewWindowBuilderElectronExt;
  /// // Ctrl+P prints and Alt+Left goes back, as a user expects.
  /// builder.allow_chrome_commands([ChromeCommandGroup::Document, ChromeCommandGroup::History]);
  /// # }
  /// ```
  #[must_use]
  fn allow_chrome_commands<I: IntoIterator<Item = ChromeCommandGroup>>(self, groups: I) -> Self;

  /// Takes a last look at the [`ElectronBrowserSettings`] before the webview is
  /// created.
  ///
  /// The runtime maps the portable `WebviewAttributes` onto these settings first,
  /// so this can change what it decided as well as reach the settings Tauri has no
  /// attribute for. Anything `webview::Config` has no typed field for goes in
  /// [`ElectronBrowserSettings::additional`] and is sent to the main process
  /// verbatim.
  ///
  /// ```no_run
  /// # use tauri_runtime_electron::AsElectronWebviewAttributes;
  /// # fn f<R, M>(builder: tauri::WebviewWindowBuilder<'_, R, M>)
  /// # where
  /// #   R: tauri::Runtime,
  /// #   M: tauri::Manager<R>,
  /// #   R::RuntimeWebviewAttributes: AsElectronWebviewAttributes,
  /// # {
  /// use tauri_runtime_electron::WebviewWindowBuilderElectronExt;
  ///
  /// // An app that ships its own fonts has no use for the ones a page asks for.
  /// builder.with_browser_settings(|settings| {
  ///   settings.additional.insert("remoteFonts".into(), false.into());
  /// });
  /// # }
  /// ```
  #[must_use]
  fn with_browser_settings<F: Fn(&mut ElectronBrowserSettings) + Send + Sync + 'static>(
    self,
    callback: F,
  ) -> Self;
}

impl<'a, R: Runtime, M: Manager<R>> WebviewWindowBuilderElectronExt
  for tauri::WebviewWindowBuilder<'a, R, M>
where
  R::RuntimeWebviewAttributes: AsElectronWebviewAttributes,
{
  fn browser_runtime_style(mut self, style: RuntimeStyle) -> Self {
    with_electron_webview_attributes(self.runtime_specific_attributes_mut(), |attributes| {
      attributes.runtime_style = Some(style);
    });
    self
  }

  fn on_frame_event<F: Fn(FrameEvent) + Send + Sync + 'static>(mut self, handler: F) -> Self {
    let handler = Arc::new(handler);
    with_electron_webview_attributes(self.runtime_specific_attributes_mut(), |attributes| {
      attributes.frame_event_handler = Some(handler);
    });
    self
  }

  fn on_console_message<F: Fn(ConsoleMessage) + Send + Sync + 'static>(
    mut self,
    handler: F,
  ) -> Self {
    let handler = Arc::new(handler);
    with_electron_webview_attributes(self.runtime_specific_attributes_mut(), |attributes| {
      attributes.console_message_handler = Some(handler);
    });
    self
  }

  fn allow_chrome_commands<I: IntoIterator<Item = ChromeCommandGroup>>(
    mut self,
    groups: I,
  ) -> Self {
    let groups = groups.into_iter().collect::<Vec<_>>();
    with_electron_webview_attributes(self.runtime_specific_attributes_mut(), |attributes| {
      attributes.allowed_chrome_commands = groups.clone();
    });
    self
  }

  fn with_browser_settings<F: Fn(&mut ElectronBrowserSettings) + Send + Sync + 'static>(
    mut self,
    callback: F,
  ) -> Self {
    let callback = Arc::new(callback);
    with_electron_webview_attributes(self.runtime_specific_attributes_mut(), |attributes| {
      attributes.browser_settings_callback = Some(callback.clone());
    });
    self
  }
}

/// Electron-specific APIs of [`tauri::webview::WebviewBuilder`].
#[cfg(feature = "unstable")]
pub trait WebviewBuilderElectronExt {
  /// Sets the browser runtime style.
  ///
  /// See [`RuntimeStyle`] for more information. Electron has no Alloy/Chrome
  /// distinction: the value is stored, and a request for
  /// [`RuntimeStyle::Chrome`] is reported as unsupported rather than silently
  /// ignored.
  #[must_use]
  fn browser_runtime_style(self, style: RuntimeStyle) -> Self;

  /// Observes native frame lifecycle events for main and child frames.
  ///
  /// The callback runs on the runtime's own thread. It must return promptly and
  /// must not wait for an event-loop operation. This observer does not replace the
  /// navigation policy configured by `on_navigation`. It is scoped to this
  /// webview's own native browser, so a popup the runtime opened as a window of
  /// its own is a separate browser that is never reported here — observe popups
  /// through [`Webview::popups`](crate::Webview::popups).
  #[must_use]
  fn on_frame_event<F: Fn(FrameEvent) + Send + Sync + 'static>(self, handler: F) -> Self;

  /// Observes the messages the renderer writes to the JavaScript console,
  /// without DevTools having to be open.
  ///
  /// The callback runs on the runtime's own thread, so it must return promptly and
  /// must not wait for an event-loop operation. Observing a message does not
  /// suppress it: the renderer logs it as it normally would. It is scoped to this
  /// webview's own native browser, so neither a popup's output nor that of a
  /// DevTools window opened on this webview is reported here.
  #[must_use]
  fn on_console_message<F: Fn(ConsoleMessage) + Send + Sync + 'static>(self, handler: F) -> Self;

  /// Keeps the named families of Chrome commands rather than swallowing them.
  ///
  /// The reference runtime has to swallow Chrome's accelerator table because a
  /// Chrome style browser keeps it live even when hosted as a child view with no
  /// browser UI. Electron binds no such table, so there is nothing to swallow and
  /// nothing to keep: the value is stored and reported as unsupported when it is
  /// non-empty. Calling this more than once replaces the previous list.
  ///
  /// DevTools and zoom accelerators are not covered here: they follow
  /// `WebviewAttributes::devtools` and `WebviewAttributes::zoom_hotkeys_enabled`.
  #[must_use]
  fn allow_chrome_commands<I: IntoIterator<Item = ChromeCommandGroup>>(self, groups: I) -> Self;

  /// Takes a last look at the [`ElectronBrowserSettings`] before the webview is
  /// created.
  ///
  /// The runtime maps the portable `WebviewAttributes` onto these settings first,
  /// so this can change what it decided as well as reach the settings Tauri has no
  /// attribute for. Anything `webview::Config` has no typed field for goes in
  /// [`ElectronBrowserSettings::additional`] and is sent to the main process
  /// verbatim.
  #[must_use]
  fn with_browser_settings<F: Fn(&mut ElectronBrowserSettings) + Send + Sync + 'static>(
    self,
    callback: F,
  ) -> Self;
}

#[cfg(feature = "unstable")]
impl<R: Runtime> WebviewBuilderElectronExt for tauri::webview::WebviewBuilder<R>
where
  R::RuntimeWebviewAttributes: AsElectronWebviewAttributes,
{
  fn browser_runtime_style(mut self, style: RuntimeStyle) -> Self {
    with_electron_webview_attributes(self.runtime_specific_attributes_mut(), |attributes| {
      attributes.runtime_style = Some(style);
    });
    self
  }

  fn on_frame_event<F: Fn(FrameEvent) + Send + Sync + 'static>(mut self, handler: F) -> Self {
    let handler = Arc::new(handler);
    with_electron_webview_attributes(self.runtime_specific_attributes_mut(), |attributes| {
      attributes.frame_event_handler = Some(handler);
    });
    self
  }

  fn on_console_message<F: Fn(ConsoleMessage) + Send + Sync + 'static>(
    mut self,
    handler: F,
  ) -> Self {
    let handler = Arc::new(handler);
    with_electron_webview_attributes(self.runtime_specific_attributes_mut(), |attributes| {
      attributes.console_message_handler = Some(handler);
    });
    self
  }

  fn allow_chrome_commands<I: IntoIterator<Item = ChromeCommandGroup>>(
    mut self,
    groups: I,
  ) -> Self {
    let groups = groups.into_iter().collect::<Vec<_>>();
    with_electron_webview_attributes(self.runtime_specific_attributes_mut(), |attributes| {
      attributes.allowed_chrome_commands = groups.clone();
    });
    self
  }

  fn with_browser_settings<F: Fn(&mut ElectronBrowserSettings) + Send + Sync + 'static>(
    mut self,
    callback: F,
  ) -> Self {
    let callback = Arc::new(callback);
    with_electron_webview_attributes(self.runtime_specific_attributes_mut(), |attributes| {
      attributes.browser_settings_callback = Some(callback.clone());
    });
    self
  }
}
