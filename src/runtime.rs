// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

use std::{
  collections::HashMap,
  fmt,
  fs::create_dir_all,
  path::PathBuf,
  sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU32, Ordering},
    mpsc::{self, Receiver, Sender},
  },
  time::Duration,
};

use raw_window_handle::DisplayHandle;
use tauri_runtime::{
  DeviceEventFilter, Error, EventLoopProxy, Result, RunEvent, Runtime, RuntimeHandle,
  RuntimeInitArgs, UserEvent,
  dpi::PhysicalPosition,
  monitor::Monitor,
  webview::{DetachedWebview, PendingWebview},
  window::{DetachedWindow, PendingWindow, RawWindow, WindowEvent, WindowId},
};
use tauri_utils::Theme;

use crate::{
  DebugEnvironment,
  electron_impl::{
    client::{self},
    ipc::Ipc,
    request_handler,
  },
  platform::{NativeEventLoop, NativeWindow, NativeWindowEvent},
  webview::{
    self, ElectronWebviewAttributes, ElectronWebviewDispatcher, Webview, WebviewMessage,
    create_webview_detached,
  },
  window::{
    AppWindow, ElectronWindowDispatcher, WindowMessage, create_window_detached,
    native_monitor_to_tauri_monitor,
  },
  window_handle::SendRawDisplayHandle,
};

/// Re-exported so applications can name the bridge crate's types, mirroring the
/// reference runtime's `pub use cef;`.
pub use electron_rs;

/// The Electron release this runtime was built against, mirroring the reference's
/// `CEF_API_VERSION_LAST`.
pub const ELECTRON_API_VERSION_LAST: &str = electron_rs::DEFAULT_VERSION;

/// The switch that tells a helper process which API version the browser process runs.
const API_VERSION_SWITCH: &str = "tauri-electron-api-version";

/// Handles a non-browser process of this application.
///
/// CEF re-executes the application binary for its renderer, GPU and utility
/// processes, so the reference has to detect that case and hand the process to CEF
/// before any Tauri code runs. Electron launches its own helpers from its own
/// distribution and never re-executes the application, so there is nothing to
/// detect and nothing to hand over.
///
/// The function exists because `#[electron_entry_point]` expands to a call to it,
/// which keeps an application's `main` identical to the CEF runtime's.
pub fn run_electron_helper_process() {}

/// Observes a native window event before Tauri's own handling.
///
/// `Destroyed` is delivered before the window is released, so a handler can still
/// read the native handle.
pub type NativeWindowEventHandler =
  dyn Fn(&str, &dyn NativeEventLoop, Arc<NativeWindow>, &NativeWindowEvent) + Send + Sync;

/// Last-chance mutation of the settings handed to the Electron main process.
type SettingsCallback = dyn FnOnce(&mut ElectronSettings) + Send + Sync;

/// Which key Chromium's `os_crypt` uses for cookies and saved passwords.
///
/// Only those two are encrypted; everything else in the cache directory is
/// plaintext. `Mock` and `System` on a machine with no keyring both fall back to a
/// publicly known constant, which protects nothing at rest, and switching between
/// them invalidates the cookies already stored.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SecretStorage {
  /// Skip the OS secret store in development builds (`tauri::is_dev()`), use it in
  /// release builds.
  #[default]
  Auto,
  /// Always use Chromium's hard-coded constant: `--use-mock-keychain` on macOS,
  /// `--password-store=basic` on Linux.
  Mock,
  /// Always use the OS secret store; appends no switch.
  System,
}

/// What to do with Chromium's process sandbox.
///
/// Electron ships its own sandbox broker on every platform, so unlike the CEF
/// runtime there is no platform where the sandbox is structurally unavailable.
/// The policy still decides what happens when the OS refuses to start a sandboxed
/// renderer.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SandboxPolicy {
  /// Keep the sandbox where possible, warn where it cannot start.
  #[default]
  Auto,
  /// Never run unsandboxed: fail startup instead.
  Required,
  /// Always run unsandboxed, on every platform.
  Disabled,
}

/// Whether the DevTools protocol server runs, and over which transport.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum RemoteDebugging {
  /// No server.
  #[default]
  Disabled,
  /// Listen on a TCP port, as `--remote-debugging-port`.
  ///
  /// The port must be 1024–65535: anything below is a privileged port on every
  /// Unix and is refused with a warning.
  Port {
    /// The TCP port to listen on.
    port: u16,
    /// Origins allowed to connect, as `--remote-allow-origins`.
    allowed_origins: Vec<String>,
  },
  /// Speak the protocol over inherited file descriptors, as
  /// `--remote-debugging-pipe`.
  Pipe,
}

/// Whether the application may open a DevTools window at all.
///
/// Combined with the per-webview `WebviewAttributes::devtools`. The protocol
/// itself stays available whatever this says, because the runtime drives its own
/// startup over it.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DevToolsPolicy {
  /// Allow in a build that could open them anyway (debug, or the `devtools`
  /// feature), refuse otherwise.
  #[default]
  Auto,
  /// Allow whatever the build profile; per-webview `devtools` still applies.
  Allowed,
  /// Refuse whatever the build profile.
  Disallowed,
}

/// What to do when a TLS certificate fails to verify.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CertificateErrorPolicy {
  /// Show Chromium's SSL interstitial, which offers a way through.
  #[default]
  ChromeInterstitial,
  /// Cancel the request; no interstitial, no override.
  Cancel,
}

/// How Chromium resolves the proxy for every request.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ProxyConfig {
  /// Use the OS proxy configuration.
  #[default]
  System,
  /// Connect directly, ignoring any system proxy.
  Direct,
  /// Discover a proxy through WPAD.
  AutoDetect,
  /// Fetch a PAC script from `url`.
  PacScript {
    /// The PAC script URL.
    url: String,
  },
  /// A fixed proxy.
  FixedServers {
    /// `scheme://host:port`; a bare `host:port` means HTTP.
    server: String,
    /// Semicolon-delimited bypass list.
    bypass_list: Option<String>,
  },
}

impl ProxyConfig {
  /// Renders the Chromium `proxy` preference dictionary.
  ///
  /// A key the mode does not accept makes Chromium reject the whole dictionary,
  /// so `bypass_list` is written only when it is set.
  fn to_preference(&self) -> serde_json::Value {
    use serde_json::json;
    match self {
      Self::System => json!({ "mode": "system" }),
      Self::Direct => json!({ "mode": "direct" }),
      Self::AutoDetect => json!({ "mode": "auto_detect" }),
      Self::PacScript { url } => json!({ "mode": "pac_script", "pac_url": url }),
      Self::FixedServers {
        server,
        bypass_list,
      } => {
        let mut value = json!({ "mode": "fixed_servers", "server": server });
        if let Some(bypass_list) = bypass_list {
          value["bypass_list"] = json!(bypass_list);
        }
        value
      }
    }
  }
}

/// When a page may start playing media on its own.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum AutoplayPolicy {
  /// Chromium's own default: audible media needs a document gesture.
  #[default]
  Default,
  /// Play without any user interaction.
  NoUserGestureRequired,
  /// Require a gesture on the media element.
  UserGestureRequired,
  /// Require the user to have interacted with the document.
  DocumentUserActivationRequired,
}

impl AutoplayPolicy {
  /// The `--autoplay-policy` value, `None` to leave the switch off.
  fn as_switch_value(self) -> Option<&'static str> {
    match self {
      Self::Default => None,
      Self::NoUserGestureRequired => Some("no-user-gesture-required"),
      Self::UserGestureRequired => Some("user-gesture-required"),
      Self::DocumentUserActivationRequired => Some("document-user-activation-required"),
    }
  }
}

/// Which local interfaces WebRTC may reveal.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum WebRtcIpHandling {
  /// Chromium's default.
  #[default]
  Default,
  /// Offer public and private interfaces, which reveals the LAN address.
  DefaultPublicAndPrivateInterfaces,
  /// Offer only the public interface.
  DefaultPublicInterfaceOnly,
  /// Refuse UDP that does not go through the proxy.
  DisableNonProxiedUdp,
}

impl WebRtcIpHandling {
  /// The `--webrtc-ip-handling-policy` value, `None` to leave the switch off.
  ///
  /// The values are underscored, deliberately unlike [`AutoplayPolicy`]'s
  /// hyphenated ones.
  fn as_switch_value(self) -> Option<&'static str> {
    match self {
      Self::Default => None,
      Self::DefaultPublicAndPrivateInterfaces => Some("default_public_and_private_interfaces"),
      Self::DefaultPublicInterfaceOnly => Some("default_public_interface_only"),
      Self::DisableNonProxiedUdp => Some("disable_non_proxied_udp"),
    }
  }
}

/// What to do with a profile a newer Electron/Chromium milestone last used.
///
/// Chromium migrates a profile forward only. The runtime writes the `Last Version`
/// breadcrumb Chromium does not write, and applies this policy when the breadcrumb
/// names a higher milestone. A downgrade within a milestone is left alone, and a
/// profile another running instance holds is never touched.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DowngradePolicy {
  /// Open the profile as is and log a warning naming both versions.
  #[default]
  KeepProfile,
  /// Move the root cache path aside and start empty, deleting the old one in the
  /// background.
  ResetProfile,
}

/// The severity Chromium writes to its log.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum LogSeverity {
  /// The most verbose level.
  Verbose,
  /// Informational messages.
  Info,
  /// Chromium's own default.
  #[default]
  Default,
  /// Warnings and errors only.
  Warning,
  /// Errors only.
  Error,
  /// Nothing.
  Disable,
}

impl LogSeverity {
  /// The `--log-severity` value.
  fn as_switch_value(self) -> &'static str {
    match self {
      Self::Verbose => "verbose",
      Self::Info => "info",
      Self::Default => "default",
      Self::Warning => "warning",
      Self::Error => "error",
      Self::Disable => "disable",
    }
  }
}

/// Which fields the runtime prepends to each log line.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum LogItems {
  /// Chromium's own choice of fields.
  #[default]
  Default,
  /// Timestamp, thread id, level.
  Time,
  /// Timestamp, thread id, level and the source location.
  Verbose,
}

/// The settings handed to the Electron main process before it creates anything.
///
/// The Electron counterpart of `cef::Settings`: every field is a runtime-wide
/// policy that has to be in place before the first window exists.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct ElectronSettings {
  /// Chromium's user-data directory.
  pub cache_path: Option<PathBuf>,
  /// Where Chromium writes its log.
  pub log_file: Option<PathBuf>,
  /// How verbose that log is.
  pub log_severity: LogSeverity,
  /// Which fields each line carries.
  pub log_items: LogItems,
  /// The User-Agent sent with every request.
  pub user_agent: Option<String>,
  /// The product token spliced into Chromium's own User-Agent.
  pub user_agent_product: Option<String>,
  /// Chromium's own UI locale.
  pub locale: Option<String>,
  /// The `Accept-Language` list and `navigator.language`.
  pub accept_language_list: Option<String>,
  /// Whether session cookies survive a restart.
  pub persist_session_cookies: bool,
  /// V8 flags.
  pub javascript_flags: Option<String>,
  /// The Chrome enterprise policy source.
  pub chrome_policy_id: Option<String>,
  /// Whether Chromium's process sandbox is disabled.
  pub no_sandbox: bool,
}

/// A permission category, the Electron counterpart of `cef::ContentSettingTypes`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ContentSettingType {
  /// Camera access.
  MediaStreamCamera,
  /// Microphone access.
  MediaStreamMicrophone,
  /// Geolocation.
  Geolocation,
  /// Desktop notifications.
  Notifications,
  /// Clipboard reads.
  ClipboardRead,
  /// Mouse lock.
  MouseLock,
  /// Fullscreen without a gesture.
  Fullscreen,
  /// Any category not listed above, by its Chromium name.
  Other(&'static str),
}

impl ContentSettingType {
  /// The Electron permission name this maps to.
  pub(crate) fn permission(self) -> &'static str {
    match self {
      Self::MediaStreamCamera => "media",
      Self::MediaStreamMicrophone => "media",
      Self::Geolocation => "geolocation",
      Self::Notifications => "notifications",
      Self::ClipboardRead => "clipboard-read",
      Self::MouseLock => "pointerLock",
      Self::Fullscreen => "fullscreen",
      Self::Other(name) => name,
    }
  }
}

/// The default answer for a permission, the Electron counterpart of
/// `cef::ContentSettingValues`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ContentSettingValue {
  /// Allow without asking.
  Allow,
  /// Block without asking.
  Block,
  /// Ask the application's permission handler.
  Ask,
}

impl ContentSettingValue {
  /// The Electron permission-check answer.
  pub(crate) fn answer(self) -> Option<bool> {
    match self {
      Self::Allow => Some(true),
      Self::Block => Some(false),
      Self::Ask => None,
    }
  }
}

/// Selects the [`ElectronRuntime`], e.g.
/// `tauri::Builder::default().runtime(Electron::default())`.
#[derive(Default)]
pub struct Electron {
  command_line_args: Vec<(String, Option<String>)>,
  disabled_features: Vec<String>,
  enabled_features: Vec<String>,
  deep_link_schemes: Vec<String>,
  /// Extra application protocols that must be privileged before Electron starts.
  schemes: Vec<String>,
  cache_path: Option<PathBuf>,
  secret_storage: SecretStorage,
  profile_preferences: Vec<(String, serde_json::Value)>,
  global_preferences: Vec<(String, serde_json::Value)>,
  content_settings: Vec<(ContentSettingType, ContentSettingValue)>,
  allow_chromium_command_line_args: bool,
  log_file: Option<PathBuf>,
  log_severity: Option<LogSeverity>,
  log_items: Option<LogItems>,
  locale: Option<String>,
  accept_language_list: Option<String>,
  user_agent: Option<String>,
  user_agent_product: Option<String>,
  javascript_flags: Option<String>,
  chrome_policy_id: Option<String>,
  persist_session_cookies: bool,
  remote_debugging: RemoteDebugging,
  devtools: DevToolsPolicy,
  debug_environment: DebugEnvironment,
  certificate_errors: CertificateErrorPolicy,
  sandbox: SandboxPolicy,
  downgrade: DowngradePolicy,
  settings_callback: Option<Box<SettingsCallback>>,
  window_event_handler: Option<Arc<NativeWindowEventHandler>>,
  /// An explicit Electron executable, overriding the prepared distribution.
  binary: Option<PathBuf>,
  /// The application identifier, handed to Electron as its AppUserModelID.
  identifier: Option<String>,
  /// The declared API version, when the application names one.
  api_version: Option<i32>,
  /// How long Electron gets to start and connect back.
  startup_timeout: Option<Duration>,
}

impl fmt::Debug for Electron {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("Electron")
      .field("command_line_args", &self.command_line_args)
      .field("disabled_features", &self.disabled_features)
      .field("enabled_features", &self.enabled_features)
      .field("deep_link_schemes", &self.deep_link_schemes)
      .field("schemes", &self.schemes)
      .field("cache_path", &self.cache_path)
      .field("secret_storage", &self.secret_storage)
      .field("profile_preferences", &self.profile_preferences)
      .field("global_preferences", &self.global_preferences)
      .field("content_settings", &self.content_settings)
      .field(
        "allow_chromium_command_line_args",
        &self.allow_chromium_command_line_args,
      )
      .field("log_file", &self.log_file)
      .field("log_severity", &self.log_severity)
      .field("log_items", &self.log_items)
      .field("locale", &self.locale)
      .field("accept_language_list", &self.accept_language_list)
      .field("user_agent", &self.user_agent)
      .field("user_agent_product", &self.user_agent_product)
      .field("javascript_flags", &self.javascript_flags)
      .field("chrome_policy_id", &self.chrome_policy_id)
      .field("persist_session_cookies", &self.persist_session_cookies)
      .field("remote_debugging", &self.remote_debugging)
      .field("devtools", &self.devtools)
      .field("debug_environment", &self.debug_environment)
      .field("certificate_errors", &self.certificate_errors)
      .field("sandbox", &self.sandbox)
      .field("downgrade", &self.downgrade)
      .field("settings_callback", &self.settings_callback.is_some())
      .field("window_event_handler", &self.window_event_handler.is_some())
      .field("binary", &self.binary)
      .field("identifier", &self.identifier)
      .field("api_version", &self.api_version)
      .field("startup_timeout", &self.startup_timeout)
      .finish()
  }
}

impl Electron {
  /// Observes native window events before Tauri's own handling.
  #[must_use]
  pub fn on_window_event<F>(mut self, handler: F) -> Self
  where
    F:
      Fn(&str, &dyn NativeEventLoop, Arc<NativeWindow>, &NativeWindowEvent) + Send + Sync + 'static,
  {
    self.window_event_handler = Some(Arc::new(handler));
    self
  }

  /// Mutates the [`ElectronSettings`] immediately before Electron starts.
  #[must_use]
  pub fn with_settings<F>(mut self, callback: F) -> Self
  where
    F: FnOnce(&mut ElectronSettings) + Send + Sync + 'static,
  {
    self.settings_callback = Some(Box::new(callback));
    self
  }

  /// Appends one Chromium switch, browser process only.
  #[must_use]
  pub fn command_line_arg<K: Into<String>, V: Into<String>>(
    mut self,
    key: K,
    value: Option<V>,
  ) -> Self {
    self
      .command_line_args
      .push((key.into(), value.map(Into::into)));
    self
  }

  /// Appends many Chromium switches, browser process only.
  #[must_use]
  pub fn command_line_args<K: Into<String>, V: Into<String>>(
    mut self,
    args: impl IntoIterator<Item = (K, Option<V>)>,
  ) -> Self {
    self.command_line_args.extend(
      args
        .into_iter()
        .map(|(key, value)| (key.into(), value.map(Into::into))),
    );
    self
  }

  /// Privileges one custom URL scheme before Electron starts.
  ///
  /// Tauri 3 does not pass builder-registered protocols in its startup arguments, so
  /// a scheme the application serves itself has to be named here as well as
  /// registered with the webview. The reference needs no equivalent: CEF takes its
  /// scheme list from the same `RuntimeInitArgs` the application already fills in.
  ///
  /// ```no_run
  /// # use tauri_runtime_electron::Electron;
  /// let runtime = Electron::default().custom_scheme("stream");
  /// ```
  #[must_use]
  pub fn custom_scheme(mut self, scheme: impl Into<String>) -> Self {
    self.schemes.push(scheme.into());
    self
  }

  /// Privileges many custom URL schemes before Electron starts.
  #[must_use]
  pub fn custom_schemes<S: Into<String>>(mut self, schemes: impl IntoIterator<Item = S>) -> Self {
    self.schemes.extend(schemes.into_iter().map(Into::into));
    self
  }

  /// The schemes recognised when the application is relaunched by a deep link.
  #[must_use]
  pub fn deep_link_schemes<S: Into<String>>(
    mut self,
    schemes: impl IntoIterator<Item = S>,
  ) -> Self {
    self
      .deep_link_schemes
      .extend(schemes.into_iter().map(Into::into));
    self
  }

  /// Chromium's user-data directory.
  #[must_use]
  pub fn root_cache_path<P: AsRef<std::path::Path>>(mut self, path: P) -> Self {
    self.cache_path = Some(path.as_ref().to_path_buf());
    self
  }

  /// What to do with a profile a newer milestone last used.
  #[must_use]
  pub fn downgrade(mut self, policy: DowngradePolicy) -> Self {
    self.downgrade = policy;
    self
  }

  /// Which key `os_crypt` uses.
  #[must_use]
  pub fn secret_storage(mut self, storage: SecretStorage) -> Self {
    self.secret_storage = storage;
    self
  }

  /// Sets one boolean profile preference.
  #[must_use]
  pub fn profile_preference<K: Into<String>>(mut self, name: K, enabled: bool) -> Self {
    self
      .profile_preferences
      .push((name.into(), serde_json::Value::Bool(enabled)));
    self
  }

  /// Sets one typed profile preference.
  #[must_use]
  pub fn profile_preference_value<K: Into<String>, V: Into<serde_json::Value>>(
    mut self,
    name: K,
    value: V,
  ) -> Self {
    self.profile_preferences.push((name.into(), value.into()));
    self
  }

  /// Sets one local-state preference.
  #[must_use]
  pub fn global_preference<K: Into<String>, V: Into<serde_json::Value>>(
    mut self,
    name: K,
    value: V,
  ) -> Self {
    self.global_preferences.push((name.into(), value.into()));
    self
  }

  /// The default answer for a permission, for every origin.
  #[must_use]
  pub fn default_content_setting(
    mut self,
    content_type: ContentSettingType,
    value: ContentSettingValue,
  ) -> Self {
    self.content_settings.push((content_type, value));
    self
  }

  /// Whether release builds honour Chromium switches from the process command line.
  ///
  /// Development builds always allow them.
  #[must_use]
  pub fn allow_chromium_command_line_args(mut self, allow: bool) -> Self {
    self.allow_chromium_command_line_args = allow;
    self
  }

  /// Where Chromium writes its log.
  #[must_use]
  pub fn log_file<P: AsRef<std::path::Path>>(mut self, path: P) -> Self {
    self.log_file = Some(path.as_ref().to_path_buf());
    self
  }

  /// How verbose that log is.
  #[must_use]
  pub fn log_severity(mut self, severity: LogSeverity) -> Self {
    self.log_severity = Some(severity);
    self
  }

  /// Which fields each log line carries.
  #[must_use]
  pub fn log_items(mut self, items: LogItems) -> Self {
    self.log_items = Some(items);
    self
  }

  /// Chromium's own UI locale.
  #[must_use]
  pub fn locale<S: Into<String>>(mut self, locale: S) -> Self {
    self.locale = Some(locale.into());
    self
  }

  /// The `Accept-Language` list and `navigator.language`.
  #[must_use]
  pub fn accept_language_list<S: Into<String>>(mut self, languages: S) -> Self {
    self.accept_language_list = Some(languages.into());
    self
  }

  /// What to do with Chromium's process sandbox.
  #[must_use]
  pub fn sandbox(mut self, policy: SandboxPolicy) -> Self {
    self.sandbox = policy;
    self
  }

  /// Adds to `--disable-features`, merged with anything already configured.
  #[must_use]
  pub fn disable_features<S: Into<String>>(
    mut self,
    features: impl IntoIterator<Item = S>,
  ) -> Self {
    self
      .disabled_features
      .extend(features.into_iter().map(Into::into));
    self
  }

  /// Adds to `--enable-features`, merged with anything already configured.
  #[must_use]
  pub fn enable_features<S: Into<String>>(mut self, features: impl IntoIterator<Item = S>) -> Self {
    self
      .enabled_features
      .extend(features.into_iter().map(Into::into));
    self
  }

  /// Whether Chromium's diagnostic environment variables are honoured.
  #[must_use]
  pub fn debug_environment(mut self, policy: DebugEnvironment) -> Self {
    self.debug_environment = policy;
    self
  }

  /// Whether the DevTools protocol server runs, and over which transport.
  #[must_use]
  pub fn remote_debugging(mut self, remote_debugging: RemoteDebugging) -> Self {
    self.remote_debugging = remote_debugging;
    self
  }

  /// Whether a DevTools window may open at all.
  #[must_use]
  pub fn devtools(mut self, policy: DevToolsPolicy) -> Self {
    self.devtools = policy;
    self
  }

  /// How TLS certificate errors are handled.
  #[must_use]
  pub fn certificate_errors(mut self, policy: CertificateErrorPolicy) -> Self {
    self.certificate_errors = policy;
    self
  }

  /// How Chromium resolves the proxy.
  #[must_use]
  pub fn proxy(mut self, proxy: ProxyConfig) -> Self {
    // `System` writes no preference at all, so an application that never calls
    // this keeps Chromium's own resolution.
    if proxy != ProxyConfig::System {
      self
        .profile_preferences
        .push(("proxy".into(), proxy.to_preference()));
    }
    self
  }

  /// When a page may start playing media on its own.
  #[must_use]
  pub fn autoplay(mut self, policy: AutoplayPolicy) -> Self {
    if let Some(value) = policy.as_switch_value() {
      self
        .command_line_args
        .push(("autoplay-policy".into(), Some(value.into())));
    }
    self
  }

  /// Which local interfaces WebRTC may reveal.
  #[must_use]
  pub fn webrtc_ip_handling(mut self, policy: WebRtcIpHandling) -> Self {
    if let Some(value) = policy.as_switch_value() {
      self
        .command_line_args
        .push(("webrtc-ip-handling-policy".into(), Some(value.into())));
    }
    self
  }

  /// Whether the spell checker runs.
  #[must_use]
  pub fn spell_checking(mut self, enabled: bool) -> Self {
    self
      .profile_preferences
      .push(("browser.enable_spellchecking".into(), enabled.into()));
    self
  }

  /// Whether Safe Browsing runs.
  #[must_use]
  pub fn safe_browsing(mut self, enabled: bool) -> Self {
    self
      .profile_preferences
      .push(("safebrowsing.enabled".into(), enabled.into()));
    self
  }

  /// Whether Chromium's component updater runs.
  #[must_use]
  pub fn component_updates(mut self, enabled: bool) -> Self {
    if !enabled {
      self
        .command_line_args
        .push(("disable-component-update".into(), None));
    }
    self
  }

  /// The whole User-Agent string.
  #[must_use]
  pub fn user_agent<S: Into<String>>(mut self, user_agent: S) -> Self {
    self.user_agent = Some(user_agent.into());
    self
  }

  /// The product token spliced into Chromium's own User-Agent.
  ///
  /// Ignored when [`Self::user_agent`] is also set.
  #[must_use]
  pub fn user_agent_product<S: Into<String>>(mut self, product: S) -> Self {
    self.user_agent_product = Some(product.into());
    self
  }

  /// Whether session cookies survive a restart.
  #[must_use]
  pub fn persist_session_cookies(mut self, persist: bool) -> Self {
    self.persist_session_cookies = persist;
    self
  }

  /// V8 flags.
  #[must_use]
  pub fn javascript_flags<S: Into<String>>(mut self, flags: S) -> Self {
    self.javascript_flags = Some(flags.into());
    self
  }

  /// The Chrome enterprise policy source.
  #[must_use]
  pub fn chrome_policy_id<S: Into<String>>(mut self, policy_id: S) -> Self {
    self.chrome_policy_id = Some(policy_id.into());
    self
  }

  /// Declares the API version the application was compiled against.
  ///
  /// CEF gates its ABI on this number because the wrapper and the binary
  /// distribution are versioned separately. Electron ships its JavaScript and
  /// native halves from one distribution, so there is no ABI to declare: the value
  /// is recorded for parity and reported when it disagrees with the release this
  /// build pinned.
  #[must_use]
  pub fn electron_api_version(mut self, version: i32) -> Self {
    let expected = ELECTRON_API_VERSION_LAST
      .split('.')
      .next()
      .and_then(|major| major.parse::<i32>().ok());
    if expected.is_some_and(|expected| expected != version) {
      log::warn!(
        "[electron] declared API version {version} differs from the pinned Electron release \
         {ELECTRON_API_VERSION_LAST}; the distribution's own version wins"
      );
    }
    self.api_version = Some(version);
    self
  }

  /// An explicit Electron executable, overriding the prepared distribution.
  #[must_use]
  pub fn binary(mut self, binary: impl Into<PathBuf>) -> Self {
    self.binary = Some(binary.into());
    self
  }

  /// The application identifier, handed to Electron as its AppUserModelID.
  #[must_use]
  pub fn identifier<S: Into<String>>(mut self, identifier: S) -> Self {
    self.identifier = Some(identifier.into());
    self
  }

  /// How long Electron gets to start and connect back.
  #[must_use]
  pub fn startup_timeout(mut self, timeout: Duration) -> Self {
    self.startup_timeout = Some(timeout);
    self
  }

  /// Resolves the Electron executable without downloading or repairing anything.
  fn resolve_binary(&self) -> PathBuf {
    if let Some(binary) = &self.binary {
      return binary.clone();
    }
    // The build script prepared a distribution for this build and reported it here,
    // which is the copy `cargo run` and `cargo test` must use.
    if let Some(staged) = option_env!("ELECTRON_RS_STAGED_BINARY") {
      let staged = PathBuf::from(staged);
      if staged.is_file() {
        return staged;
      }
    }
    if let Ok(binary) = electron_rs::electron_binary!(None::<&Path>) {
      return binary;
    }
    // A packaged app keeps the distribution beside its executable or under
    // `resources/`. `bundle.resources` maps `target/electron` to `runtime`, so the
    // layout is `resources/runtime/<platform>-<arch>/`, which `electron-rs` accepts
    // as a resource root. On macOS `resources/` is `Contents/Resources`, one level up
    // from `Contents/MacOS`.
    if let Ok(executable) = std::env::current_exe()
      && let Some(dir) = executable.parent()
    {
      for candidate in [
        dir.join("electron"),
        dir.join("runtime"),
        dir.join("resources").join("electron"),
        dir.join("resources").join("runtime"),
        dir.join("..").join("Resources").join("electron"),
        dir.join("..").join("Resources").join("runtime"),
      ] {
        if let Ok(binary) = electron_rs::electron_binary(&candidate) {
          return binary;
        }
      }
    }
    PathBuf::from("electron")
  }
}

use std::path::Path;

/// Information about the webview that requested a new window.
#[derive(Clone)]
pub struct NewWindowOpener {
  source_url: Option<url::Url>,
}

impl NewWindowOpener {
  pub(crate) fn new(source_url: Option<url::Url>) -> Self {
    Self { source_url }
  }

  /// The URL of the document that called `window.open`, when Electron reported it.
  pub fn source_url(&self) -> Option<&url::Url> {
    self.source_url.as_ref()
  }
}

impl std::fmt::Debug for NewWindowOpener {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    // The URL can carry credentials, so only whether one was observed is printed.
    f.debug_struct("NewWindowOpener")
      .field("source_url_observed", &self.source_url.is_some())
      .finish()
  }
}

/// The `EventLoopProxy` handed to applications.
#[derive(Clone, Debug)]
pub struct EventProxy<T: UserEvent> {
  context: RuntimeContext<T>,
}

impl<T: UserEvent> EventLoopProxy<T> for EventProxy<T> {
  fn send_event(&self, event: T) -> Result<()> {
    self.context.send_message(Message::UserEvent(event))
  }
}

/// The cloneable handle every dispatcher holds.
pub(crate) struct RuntimeContext<T: UserEvent> {
  is_running: Arc<AtomicBool>,
  windows: Arc<Mutex<HashMap<WindowId, AppWindow>>>,
  pub(crate) run_tx: Sender<Message<T>>,
  main_thread_id: std::thread::ThreadId,
  next_window_id: Arc<AtomicU32>,
  next_webview_id: Arc<AtomicU32>,
  /// Connection to the Electron main process.
  pub(crate) ipc: Arc<Ipc>,
  /// The events Electron pushed, drained by the run loop.
  pub(crate) events: Arc<Mutex<Receiver<electron_rs::Event>>>,
  /// Custom protocols registered by Tauri, served through Electron's
  /// `protocol.handle`.
  pub(crate) protocol_handlers: Arc<Mutex<HashMap<String, request_handler::SchemeHandler>>>,
  /// Page origin per webview label, injected as the `Origin` header on
  /// custom-protocol requests.
  pub(crate) webview_origins: Arc<Mutex<HashMap<String, String>>>,
  /// Native window handle per window, asked from Electron once.
  pub(crate) native_handles: Arc<Mutex<HashMap<WindowId, isize>>>,
  /// Whether a DevTools window may open, resolved from [`DevToolsPolicy`].
  pub(crate) devtools_allowed: bool,
  /// How TLS certificate errors are handled.
  pub(crate) certificate_errors: CertificateErrorPolicy,
  /// The native-event observer, run before Tauri's own handling.
  pub(crate) window_event_handler: Option<Arc<NativeWindowEventHandler>>,
  /// The application theme, as last set.
  pub(crate) theme: Arc<Mutex<Option<Theme>>>,
  /// The app-wide `Accept-Language` list.
  pub(crate) accept_language_list: Option<String>,
}

impl<T: UserEvent> Clone for RuntimeContext<T> {
  fn clone(&self) -> Self {
    Self {
      is_running: Arc::clone(&self.is_running),
      windows: Arc::clone(&self.windows),
      run_tx: self.run_tx.clone(),
      main_thread_id: self.main_thread_id,
      next_window_id: Arc::clone(&self.next_window_id),
      next_webview_id: Arc::clone(&self.next_webview_id),
      ipc: Arc::clone(&self.ipc),
      events: Arc::clone(&self.events),
      protocol_handlers: Arc::clone(&self.protocol_handlers),
      webview_origins: Arc::clone(&self.webview_origins),
      native_handles: Arc::clone(&self.native_handles),
      devtools_allowed: self.devtools_allowed,
      certificate_errors: self.certificate_errors,
      window_event_handler: self.window_event_handler.clone(),
      theme: Arc::clone(&self.theme),
      accept_language_list: self.accept_language_list.clone(),
    }
  }
}

impl<T: UserEvent> fmt::Debug for RuntimeContext<T> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("RuntimeContext")
      .field("main_thread_id", &self.main_thread_id)
      .field("devtools_allowed", &self.devtools_allowed)
      .finish_non_exhaustive()
  }
}

impl<T: UserEvent> RuntimeContext<T> {
  /// Runs `task` on the runtime's own thread, inline when already there.
  pub(crate) fn on_main_thread<R: Send + 'static>(
    &self,
    task: impl FnOnce(&Self) -> R + Send + 'static,
  ) -> Result<R> {
    if std::thread::current().id() == self.main_thread_id {
      return Ok(task(self));
    }
    let context = self.clone();
    let (tx, rx) = mpsc::sync_channel(1);
    self
      .run_tx
      .send(Message::Task(Box::new(move || {
        let _ = tx.send(task(&context));
      })))
      .map_err(|_| Error::FailedToSendMessage)?;
    rx.recv().map_err(|_| Error::FailedToReceiveMessage)
  }

  /// Delivers `message` to the run loop, or runs it here when already on the
  /// runtime's own thread.
  ///
  /// The reference gets this from winit: its event loop runs on the main thread, so
  /// a getter called from the main thread is answered by the loop that is already
  /// pumping. This runtime's loop is a drain the application drives, so a getter
  /// called from the main thread would queue a message that only that same thread
  /// could process, and block forever waiting for it. Window and webview messages
  /// therefore run inline on the main thread; everything else keeps queueing, so
  /// events still arrive in order through `run_iteration`.
  pub(crate) fn send_message(&self, message: Message<T>) -> Result<()> {
    if std::thread::current().id() == self.main_thread_id {
      match message {
        Message::Task(task) => {
          // A task scheduled onto the runtime's own thread can reach the client it
          // is running under, which is how `ElectronClient` becomes managed state.
          crate::command_bridge::with_client(self.ipc.client(), task);
          return Ok(());
        }
        Message::Window(id, message) => {
          crate::window::handle_window_message(self, id, message);
          return Ok(());
        }
        Message::Webview(id, message) => {
          crate::webview::handle_webview_message(self, id, message);
          return Ok(());
        }
        message => {
          return self
            .run_tx
            .send(message)
            .map_err(|_| Error::FailedToSendMessage);
        }
      }
    }
    self
      .run_tx
      .send(message)
      .map_err(|_| Error::FailedToSendMessage)
  }

  pub(crate) fn next_window_id(&self) -> WindowId {
    self.next_window_id.fetch_add(1, Ordering::Relaxed).into()
  }

  pub(crate) fn next_webview_id(&self) -> u32 {
    self.next_webview_id.fetch_add(1, Ordering::Relaxed)
  }

  pub(crate) fn is_main_thread(&self) -> bool {
    std::thread::current().id() == self.main_thread_id
  }

  /// The native handle of a window, as Electron knows it.
  pub(crate) fn native_handle(&self, id: WindowId) -> Result<isize> {
    if !self.is_main_thread() {
      return self.on_main_thread(move |context| context.native_handle(id))?;
    }
    if let Some(handle) = self
      .native_handles
      .lock()
      .unwrap_or_else(|poisoned| poisoned.into_inner())
      .get(&id)
    {
      return Ok(*handle);
    }
    let value = self
      .ipc
      .call(
        "window.nativeHandle",
        serde_json::json!({ "windowId": crate::window::window_number(id) }),
        IPC_TIMEOUT,
      )
      .map_err(create_window_err)?;
    let handle = value["handle"]
      .as_i64()
      .ok_or_else(|| create_window_err("electron returned no native handle".to_string()))?;
    let handle = handle as isize;
    self
      .native_handles
      .lock()
      .unwrap_or_else(|poisoned| poisoned.into_inner())
      .insert(id, handle);
    Ok(handle)
  }

  /// The window registered under `id`, if it still exists.
  pub(crate) fn with_window<R>(&self, id: WindowId, f: impl FnOnce(&AppWindow) -> R) -> Option<R> {
    self
      .windows
      .lock()
      .unwrap_or_else(|poisoned| poisoned.into_inner())
      .get(&id)
      .map(f)
  }

  /// Registers a window so dispatchers can find it.
  pub(crate) fn insert_window(&self, id: WindowId, window: AppWindow) {
    self
      .windows
      .lock()
      .unwrap_or_else(|poisoned| poisoned.into_inner())
      .insert(id, window);
  }

  /// Removes a window, returning whether it existed.
  pub(crate) fn remove_window(&self, id: WindowId) -> bool {
    self
      .windows
      .lock()
      .unwrap_or_else(|poisoned| poisoned.into_inner())
      .remove(&id)
      .is_some()
  }

  /// The labels of every live window.
  pub(crate) fn window_labels(&self) -> Vec<String> {
    self
      .windows
      .lock()
      .unwrap_or_else(|poisoned| poisoned.into_inner())
      .values()
      .map(|window| window.label.clone())
      .collect()
  }

  /// Records the origin a webview's document is served from.
  pub(crate) fn set_webview_origin(&self, label: &str, origin: String) {
    self
      .webview_origins
      .lock()
      .unwrap_or_else(|poisoned| poisoned.into_inner())
      .insert(label.to_owned(), origin);
  }

  /// The origin recorded for a webview label.
  pub(crate) fn webview_origin(&self, label: &str) -> Option<String> {
    self
      .webview_origins
      .lock()
      .unwrap_or_else(|poisoned| poisoned.into_inner())
      .get(label)
      .cloned()
  }

  /// Registers a custom protocol handler.
  pub(crate) fn register_protocol(&self, scheme: String, handler: request_handler::SchemeHandler) {
    self
      .protocol_handlers
      .lock()
      .unwrap_or_else(|poisoned| poisoned.into_inner())
      .insert(scheme, handler);
  }

  /// The handler registered for `scheme`.
  pub(crate) fn protocol(&self, scheme: &str) -> Option<request_handler::SchemeHandler> {
    self
      .protocol_handlers
      .lock()
      .unwrap_or_else(|poisoned| poisoned.into_inner())
      .get(scheme)
      .cloned()
  }

  /// The application theme as last set through the runtime.
  pub(crate) fn theme(&self) -> Option<Theme> {
    *self
      .theme
      .lock()
      .unwrap_or_else(|poisoned| poisoned.into_inner())
  }

  /// Runs the native-event observer, when one was configured.
  pub(crate) fn observe_native_event(
    &self,
    label: &str,
    event_loop: &dyn NativeEventLoop,
    window: Arc<NativeWindow>,
    event: &NativeWindowEvent,
  ) {
    if let Some(handler) = &self.window_event_handler {
      handler(label, event_loop, window, event);
    }
  }
}

/// A message for the run loop.
pub(crate) enum Message<T: UserEvent> {
  Task(Box<dyn FnOnce() + Send>),
  Window(WindowId, WindowMessage),
  Webview(u32, WebviewMessage),
  UserEvent(T),
  RequestExit(i32),
  /// A window's native event, delivered to Tauri's run loop.
  WindowEvent(WindowId, Box<WindowEvent>),
}

/// Timeout for a single IPC round trip with the Electron process.
pub(crate) const IPC_TIMEOUT: Duration = Duration::from_secs(15);

/// Default time the Electron process gets to start and connect back.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(60);

pub(crate) fn io_err(message: String) -> Box<dyn std::error::Error + Send + Sync> {
  Box::new(std::io::Error::other(message))
}

pub(crate) fn create_window_err(message: String) -> Error {
  Error::CreateWindow(io_err(message))
}

pub(crate) fn create_webview_err(message: String) -> Error {
  Error::CreateWebview(io_err(message))
}

impl<T: UserEvent> tauri_runtime::RuntimeInitAttrs<T> for Electron {
  type Runtime = ElectronRuntime<T>;

  fn apply_config(&mut self, config: &tauri_utils::config::Config) -> Result<()> {
    /// The `deep-link` plugin's desktop configuration, as one entry or a list.
    #[derive(serde::Deserialize)]
    #[serde(untagged)]
    enum DesktopDeepLinks {
      One(DeepLinkProtocol),
      Many(Vec<DeepLinkProtocol>),
    }

    #[derive(serde::Deserialize)]
    struct DeepLinkProtocol {
      schemes: Vec<String>,
    }

    let Some(value) = config
      .plugins
      .0
      .get("deep-link")
      .and_then(|plugin| plugin.get("desktop"))
    else {
      return Ok(());
    };
    let links: DesktopDeepLinks = serde_json::from_value(value.clone()).map_err(Error::Json)?;
    let protocols = match links {
      DesktopDeepLinks::One(protocol) => vec![protocol],
      DesktopDeepLinks::Many(protocols) => protocols,
    };
    for protocol in protocols {
      self.deep_link_schemes.extend(protocol.schemes);
    }
    Ok(())
  }
}

impl<T: UserEvent> From<Electron> for tauri_runtime::dynamic::DynRuntimeInitAttrs<T> {
  fn from(attrs: Electron) -> Self {
    Self::new(attrs)
  }
}

/// The runtime handle: everything an application can do without the run loop.
pub struct ElectronRuntimeHandle<T: UserEvent> {
  context: RuntimeContext<T>,
}

impl<T: UserEvent> Clone for ElectronRuntimeHandle<T> {
  fn clone(&self) -> Self {
    Self {
      context: self.context.clone(),
    }
  }
}

impl<T: UserEvent> fmt::Debug for ElectronRuntimeHandle<T> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("ElectronRuntimeHandle")
      .finish_non_exhaustive()
  }
}

impl<T: UserEvent> RuntimeHandle<T> for ElectronRuntimeHandle<T> {
  type Runtime = ElectronRuntime<T>;

  fn create_proxy(&self) -> EventProxy<T> {
    EventProxy {
      context: self.context.clone(),
    }
  }

  #[cfg(target_os = "macos")]
  fn set_activation_policy(
    &self,
    activation_policy: tauri_runtime::ActivationPolicy,
  ) -> Result<()> {
    let policy = match activation_policy {
      tauri_runtime::ActivationPolicy::Regular => "regular",
      tauri_runtime::ActivationPolicy::Accessory => "accessory",
      tauri_runtime::ActivationPolicy::Prohibited => "prohibited",
      _ => "regular",
    };
    self
      .context
      .ipc
      .call(
        "app.activationPolicy",
        serde_json::json!({ "policy": policy }),
        IPC_TIMEOUT,
      )
      .map(|_| ())
      .map_err(create_window_err)
  }

  #[cfg(target_os = "macos")]
  fn set_dock_visibility(&self, visible: bool) -> Result<()> {
    self
      .context
      .ipc
      .call(
        "app.dockVisibility",
        serde_json::json!({ "visible": visible }),
        IPC_TIMEOUT,
      )
      .map(|_| ())
      .map_err(create_window_err)
  }

  fn request_exit(&self, code: i32) -> Result<()> {
    self.context.send_message(Message::RequestExit(code))
  }

  fn create_window<F: Fn(RawWindow) + Send + 'static>(
    &self,
    pending: PendingWindow<T, Self::Runtime>,
    after_window_creation: Option<F>,
  ) -> Result<DetachedWindow<T, Self::Runtime>> {
    create_window_detached(&self.context, pending, after_window_creation)
  }

  fn create_webview(
    &self,
    window_id: WindowId,
    pending: PendingWebview<T, Self::Runtime>,
  ) -> Result<DetachedWebview<T, Self::Runtime>> {
    create_webview_detached(&self.context, window_id, pending)
  }

  fn run_on_main_thread<F: FnOnce() + Send + 'static>(&self, f: F) -> Result<()> {
    self.context.send_message(Message::Task(Box::new(f)))
  }

  fn display_handle(
    &self,
  ) -> std::result::Result<DisplayHandle<'_>, raw_window_handle::HandleError> {
    // Electron owns the display server connection, so the runtime has no handle of
    // its own to lend. `SendRawDisplayHandle` keeps the type honest without
    // pretending a foreign-process connection is ours.
    let handle = SendRawDisplayHandle::none();
    Ok(unsafe { DisplayHandle::borrow_raw(handle.as_raw()) })
  }

  fn primary_monitor(&self) -> Result<Option<Monitor>> {
    let displays = self
      .context
      .ipc
      .call("window.displays", serde_json::json!({}), IPC_TIMEOUT)
      .map_err(create_window_err)?;
    Ok(native_monitor_to_tauri_monitor(&displays["primary"]))
  }

  fn monitor_from_point(&self, x: f64, y: f64) -> Result<Option<Monitor>> {
    let display = self
      .context
      .ipc
      .call(
        "window.displayForPoint",
        serde_json::json!({ "x": x, "y": y }),
        IPC_TIMEOUT,
      )
      .map_err(create_window_err)?;
    Ok(native_monitor_to_tauri_monitor(&display))
  }

  fn available_monitors(&self) -> Result<Vec<Monitor>> {
    let displays = self
      .context
      .ipc
      .call("window.displays", serde_json::json!({}), IPC_TIMEOUT)
      .map_err(create_window_err)?;
    Ok(
      displays["displays"]
        .as_array()
        .map(|list| {
          list
            .iter()
            .filter_map(native_monitor_to_tauri_monitor)
            .collect()
        })
        .unwrap_or_default(),
    )
  }

  fn cursor_position(&self) -> Result<PhysicalPosition<f64>> {
    let value = self
      .context
      .ipc
      .call("window.cursorPosition", serde_json::json!({}), IPC_TIMEOUT)
      .map_err(create_window_err)?;
    Ok(PhysicalPosition {
      x: value["x"].as_f64().unwrap_or_default(),
      y: value["y"].as_f64().unwrap_or_default(),
    })
  }

  fn set_theme(&self, theme: Option<Theme>) {
    if let Err(error) = apply_app_theme(&self.context, theme) {
      log::warn!("[electron] set theme failed: {error}");
    }
  }

  #[cfg(target_os = "macos")]
  fn show(&self) -> Result<()> {
    self
      .context
      .ipc
      .call("app.show", serde_json::json!({}), IPC_TIMEOUT)
      .map(|_| ())
      .map_err(create_window_err)
  }

  #[cfg(target_os = "macos")]
  fn hide(&self) -> Result<()> {
    self
      .context
      .ipc
      .call("app.hide", serde_json::json!({}), IPC_TIMEOUT)
      .map(|_| ())
      .map_err(create_window_err)
  }

  fn set_device_event_filter(&self, _filter: DeviceEventFilter) {
    // No Electron equivalent: Chromium owns the input pipeline and never hands this
    // runtime a device event stream to filter.
    crate::unsupported::report("RuntimeHandle::set_device_event_filter");
  }

  fn custom_scheme_url(&self, scheme: &str, https: bool) -> String {
    // Electron serves every registered scheme over the HTTP-family alias, because
    // Chromium only reports a total length for partial media responses there.
    let protocol = if https { "https" } else { "http" };
    format!("{protocol}://{scheme}.localhost")
  }

  fn webview_version(&self) -> Result<String> {
    webview::webview_version()
  }

  #[cfg(any(target_os = "macos", target_os = "ios"))]
  fn fetch_data_store_identifiers<F: FnOnce(Vec<[u8; 16]>) + Send + 'static>(
    &self,
    cb: F,
  ) -> Result<()> {
    // Electron keeps one session per partition name, not a WKWebsiteDataStore
    // registry, so there is no identifier list to hand out.
    cb(Vec::new());
    Ok(())
  }

  #[cfg(any(target_os = "macos", target_os = "ios"))]
  fn remove_data_store<F: FnOnce(Result<()>) + Send + 'static>(
    &self,
    _uuid: [u8; 16],
    cb: F,
  ) -> Result<()> {
    // See `fetch_data_store_identifiers`: partitions are cleared by name through
    // `webview.clearBrowsingData`, not by an opaque identifier.
    cb(Ok(()));
    Ok(())
  }
}

/// The application theme is app-wide in Electron: `nativeTheme.themeSource`.
fn apply_app_theme<T: UserEvent>(context: &RuntimeContext<T>, theme: Option<Theme>) -> Result<()> {
  let name = match theme {
    Some(Theme::Dark) => "dark",
    Some(Theme::Light) => "light",
    _ => "system",
  };
  context
    .ipc
    .call(
      "app.theme",
      serde_json::json!({ "theme": name }),
      IPC_TIMEOUT,
    )
    .map(|_| ())
    .map_err(create_window_err)?;
  *context
    .theme
    .lock()
    .unwrap_or_else(|poisoned| poisoned.into_inner()) = theme;
  Ok(())
}

/// The runtime itself: the run loop and the window/webview factories.
pub struct ElectronRuntime<T: UserEvent = tauri::EventLoopMessage> {
  is_running: Arc<AtomicBool>,
  pub(crate) context: RuntimeContext<T>,
  run_rx: Receiver<Message<T>>,
  /// Whether `RunEvent::Ready` has been delivered.
  ready_sent: bool,
}

impl<T: UserEvent> fmt::Debug for ElectronRuntime<T> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("ElectronRuntime")
      .field("context", &self.context)
      .field("is_running", &self.is_running.load(Ordering::Relaxed))
      .finish_non_exhaustive()
  }
}

impl<T: UserEvent> ElectronRuntime<T> {
  fn init(ipc: Arc<Ipc>, events: Receiver<electron_rs::Event>, attrs: &Electron) -> Self {
    let is_running = Arc::new(AtomicBool::new(false));
    let (run_tx, run_rx) = mpsc::channel();
    let context = RuntimeContext {
      is_running: Arc::clone(&is_running),
      windows: Default::default(),
      run_tx,
      main_thread_id: std::thread::current().id(),
      next_window_id: Default::default(),
      next_webview_id: Default::default(),
      ipc,
      events: Arc::new(Mutex::new(events)),
      protocol_handlers: Default::default(),
      webview_origins: Default::default(),
      native_handles: Default::default(),
      devtools_allowed: resolve_devtools_allowed(attrs.devtools),
      certificate_errors: attrs.certificate_errors,
      window_event_handler: attrs.window_event_handler.clone(),
      theme: Default::default(),
      accept_language_list: attrs.accept_language_list.clone(),
    };
    Self {
      is_running,
      context,
      run_rx,
      ready_sent: false,
    }
  }

  /// Starts Electron and connects to it.
  fn start(mut attrs: Electron, custom_schemes: &[String]) -> Result<Self> {
    let binary = attrs.resolve_binary();
    log::debug!("[electron] launching {}", binary.display());

    let mut args = Vec::new();

    // Chromium reads its diagnostic environment variables before any embedder hook
    // runs, so the policy has to be applied before Electron is spawned.
    let is_dev = tauri::is_dev();
    if crate::environment::neutralizes_tls_key_log(attrs.debug_environment, is_dev) {
      push_switch(&mut args, "ssl-key-log-file", Some(""));
    }
    crate::environment::remove_crash_reporter_overrides(attrs.debug_environment, is_dev);

    // Switches the runtime always sets, mirroring the reference's
    // `internal_command_line_args`.
    push_switch(&mut args, "no-first-run", None);
    if let Some(severity) = attrs.log_severity {
      push_switch(&mut args, "log-severity", Some(severity.as_switch_value()));
    }
    if let Some(file) = &attrs.log_file {
      push_switch(&mut args, "log-file", Some(&file.display().to_string()));
    }
    if let Some(locale) = &attrs.locale {
      push_switch(&mut args, "lang", Some(locale));
    }
    if let Some(list) = attrs
      .accept_language_list
      .clone()
      .or_else(crate::locale::system_accept_language_list)
    {
      push_switch(&mut args, "accept-lang", Some(&list));
    }
    if let Some(flags) = &attrs.javascript_flags {
      push_switch(&mut args, "js-flags", Some(flags));
    }
    if let Some(policy_id) = &attrs.chrome_policy_id {
      push_switch(&mut args, "chrome-policy-id", Some(policy_id));
    }
    if let Some(product) = &attrs.user_agent_product {
      if attrs.user_agent.is_some() {
        log::warn!("[electron] user_agent_product is ignored because a whole user_agent was set");
      } else {
        push_switch(&mut args, "user-agent-product", Some(product));
      }
    }
    if attrs.persist_session_cookies {
      push_switch(&mut args, "persist-session-cookies", None);
    }
    if !attrs.allow_chromium_command_line_args && !is_dev {
      // Release builds refuse Chromium switches from the process command line,
      // unless the application asked for them.
      push_switch(
        &mut args,
        "disable-features",
        Some("AllowCommandLineSwitchesFromProcess"),
      );
    }
    match &attrs.remote_debugging {
      RemoteDebugging::Disabled => {}
      RemoteDebugging::Port {
        port,
        allowed_origins,
      } => {
        if *port < 1024 {
          log::warn!(
            "[electron] remote debugging port {port} is privileged; refusing to listen on it"
          );
        } else {
          push_switch(&mut args, "remote-debugging-port", Some(&port.to_string()));
          if !allowed_origins.is_empty() {
            push_switch(
              &mut args,
              "remote-allow-origins",
              Some(&allowed_origins.join(",")),
            );
          }
        }
      }
      RemoteDebugging::Pipe => push_switch(&mut args, "remote-debugging-pipe", None),
    }
    if let Some(value) = secret_storage_switch(attrs.secret_storage, is_dev) {
      push_switch(&mut args, value.0, Some(value.1));
    }
    if let Some(decision) = crate::sandbox::resolve_sandbox_decision(attrs.sandbox)
      && decision.no_sandbox
    {
      push_switch(&mut args, "no-sandbox", None);
    }
    // Merged feature switches, so a caller's own `--disable-features` is not lost.
    let disabled = attrs.disabled_features.clone();
    let enabled = attrs.enabled_features.clone();
    crate::switches::append_merged_switch(&mut args, "disable-features", &disabled);
    crate::switches::append_merged_switch(&mut args, "enable-features", &enabled);
    for (key, value) in &attrs.command_line_args {
      match value {
        Some(value) => args.push(format!("--{key}={value}")),
        None => args.push(format!("--{key}")),
      }
    }

    // The cache path is per application, so two apps never share a profile.
    let cache_path = attrs.cache_path.clone().unwrap_or_else(|| {
      let root = dirs::cache_dir().unwrap_or_else(std::env::temp_dir);
      let identifier = attrs
        .identifier
        .clone()
        .unwrap_or_else(|| "tauri-runtime-electron".to_owned());
      root.join(identifier).join("electron")
    });
    create_dir_all(&cache_path)
      .map_err(|error| create_window_err(format!("create {}: {error}", cache_path.display())))?;
    crate::downgrade::prepare_root_cache_path(&cache_path, attrs.downgrade)
      .map_err(create_window_err)?;
    // The breadcrumb is only meaningful if Electron really uses this directory as
    // its profile, so the path has to reach Chromium as well.
    push_switch(
      &mut args,
      "user-data-dir",
      Some(&cache_path.display().to_string()),
    );

    let mut settings = ElectronSettings {
      cache_path: Some(cache_path),
      log_file: attrs.log_file.clone(),
      log_severity: attrs.log_severity.unwrap_or(LogSeverity::Default),
      log_items: attrs.log_items.unwrap_or_default(),
      user_agent: attrs.user_agent.clone(),
      user_agent_product: attrs.user_agent_product.clone(),
      locale: attrs.locale.clone(),
      accept_language_list: attrs.accept_language_list.clone(),
      persist_session_cookies: attrs.persist_session_cookies,
      javascript_flags: attrs.javascript_flags.clone(),
      chrome_policy_id: attrs.chrome_policy_id.clone(),
      no_sandbox: attrs.sandbox == SandboxPolicy::Disabled,
    };
    if let Some(callback) = attrs.settings_callback.take() {
      callback(&mut settings);
    }

    // Electron only accepts privileged schemes at startup, so they travel on the
    // command line and are installed before `app.whenReady()`. `asset` is not in
    // Tauri's `custom_schemes` (its handler is registered per webview), but it needs
    // the same privileges for `convertFileSrc` fetches to work.
    let mut schemes: Vec<String> = ["ipc", "tauri", "asset"]
      .iter()
      .map(|scheme| (*scheme).to_owned())
      .collect();
    // The schemes the application named on the builder, then the ones Tauri and the
    // deep-link config carry.
    for scheme in custom_schemes
      .iter()
      .chain(&attrs.schemes)
      .chain(&attrs.deep_link_schemes)
    {
      let scheme = scheme.to_ascii_lowercase();
      if !is_valid_scheme(&scheme) {
        return Err(create_window_err(format!(
          "invalid custom URL scheme: {scheme:?}"
        )));
      }
      if !schemes.contains(&scheme) {
        schemes.push(scheme);
      }
    }
    push_switch(&mut args, "tauri-schemes", Some(&schemes.join(",")));
    if let Some(identifier) = &attrs.identifier {
      push_switch(&mut args, "tauri-identifier", Some(identifier.as_str()));
    }
    push_switch(
      &mut args,
      API_VERSION_SWITCH,
      Some(ELECTRON_API_VERSION_LAST),
    );

    let (ipc, events) = Ipc::launch(
      &binary,
      &args,
      attrs.startup_timeout.unwrap_or(STARTUP_TIMEOUT),
    )
    .map_err(create_window_err)?;

    // Preferences and content settings travel as one startup payload: Electron has
    // to apply them before the first session is used.
    if !attrs.profile_preferences.is_empty()
      || !attrs.global_preferences.is_empty()
      || !attrs.content_settings.is_empty()
    {
      let preferences = attrs
        .profile_preferences
        .iter()
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect::<serde_json::Map<_, _>>();
      let global = attrs
        .global_preferences
        .iter()
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect::<serde_json::Map<_, _>>();
      let content = attrs
        .content_settings
        .iter()
        .map(|(kind, value)| {
          (
            kind.permission().to_owned(),
            serde_json::json!({ "answer": value.answer() }),
          )
        })
        .collect::<serde_json::Map<_, _>>();
      ipc
        .call(
          "app.configure",
          serde_json::json!({
            "preferences": preferences,
            "globalPreferences": global,
            "contentSettings": content,
            "certificateErrors": match attrs.certificate_errors {
              CertificateErrorPolicy::ChromeInterstitial => "interstitial",
              CertificateErrorPolicy::Cancel => "cancel",
            },
            "devtoolsAllowed": resolve_devtools_allowed(attrs.devtools),
          }),
          IPC_TIMEOUT,
        )
        .map_err(create_window_err)?;
    }

    Ok(Self::init(ipc, events, &attrs))
  }

  /// Drains Electron's event stream and Tauri's message queue once.
  /// One turn of the loop: deliver `Ready` once, drain the runtime's own messages and
  /// then the Electron event stream, and report whether the loop should keep going.
  ///
  /// The turn waits up to a millisecond for its first message, which is what makes
  /// `run_iteration` one turn rather than a spin. A quiet turn is not a reason to
  /// stop — only a closed bridge is — so the answer is `false` for exactly the cases
  /// that end the application.
  fn pump<F: FnMut(RunEvent<T>)>(&mut self, callback: &mut F) -> bool {
    // `Ready` is delivered once, on the first pump, whether the application drives
    // the loop with `run` or with `run_iteration` — Tauri's setup code waits for it
    // either way.
    if !self.ready_sent {
      self.ready_sent = true;
      callback(RunEvent::Ready);
    }

    // Wait for at least one message so `run_iteration` never spins.
    match self.run_rx.recv_timeout(Duration::from_millis(1)) {
      Ok(message) => self.handle_message(message, callback),
      Err(mpsc::RecvTimeoutError::Timeout) => {}
      Err(mpsc::RecvTimeoutError::Disconnected) => return false,
    }
    while let Ok(message) = self.run_rx.try_recv() {
      self.handle_message(message, callback);
    }

    // Electron events first, so a window that closed is gone before Tauri asks
    // about it.
    let events: Vec<electron_rs::Event> = {
      let receiver = self
        .context
        .events
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
      let mut batch = Vec::new();
      while let Ok(event) = receiver.try_recv() {
        batch.push(event);
      }
      batch
    };
    for event in events {
      if !self.handle_electron_event(event, callback) {
        return false;
      }
    }
    true
  }

  /// Turns one Electron event into Tauri events. Returns `false` to stop the loop.
  fn handle_electron_event<F: FnMut(RunEvent<T>)>(
    &mut self,
    event: electron_rs::Event,
    callback: &mut F,
  ) -> bool {
    client::dispatch(&self.context, event, callback)
  }

  fn handle_message<F: FnMut(RunEvent<T>)>(&mut self, message: Message<T>, callback: &mut F) {
    match message {
      Message::Task(task) => task(),
      Message::UserEvent(event) => callback(RunEvent::UserEvent(event)),
      Message::RequestExit(code) => {
        // Tauri answers through the channel: `Prevent` keeps the loop running, and
        // anything else lets it stop.
        let (tx, rx) = mpsc::channel();
        callback(RunEvent::ExitRequested {
          code: Some(code),
          tx,
        });
        if rx.try_recv().is_err() {
          self.is_running.store(false, Ordering::Relaxed);
        }
      }
      Message::Window(id, message) => {
        crate::window::handle_window_message(&self.context, id, message)
      }
      Message::Webview(id, message) => {
        crate::webview::handle_webview_message(&self.context, id, message)
      }
      Message::WindowEvent(id, event) => {
        callback(RunEvent::WindowEvent {
          label: self
            .context
            .with_window(id, |window| window.label.clone())
            .unwrap_or_default(),
          event: *event,
        });
      }
    }
  }

  /// Runs the loop until Electron disconnects or the application exits.
  fn run_loop<F: FnMut(RunEvent<T>)>(&mut self, mut callback: F) {
    self.is_running.store(true, Ordering::Relaxed);
    while self.pump(&mut callback) && self.is_running.load(Ordering::Relaxed) {}
    self.is_running.store(false, Ordering::Relaxed);
  }
}

/// Appends one Chromium switch, with `=value` when there is a value.
fn push_switch(args: &mut Vec<String>, key: &str, value: Option<&str>) {
  match value {
    Some(value) => args.push(format!("--{key}={value}")),
    None => args.push(format!("--{key}")),
  }
}

/// The switch that selects Chromium's `os_crypt` key, when the policy asks for one.
///
/// Windows is untouched: DPAPI is always the key, and Chromium has no switch that
/// would replace it.
fn secret_storage_switch(
  storage: SecretStorage,
  is_dev: bool,
) -> Option<(&'static str, &'static str)> {
  let mock = match storage {
    SecretStorage::Auto => is_dev,
    SecretStorage::Mock => true,
    SecretStorage::System => false,
  };
  if !mock {
    return None;
  }
  if cfg!(target_os = "macos") {
    Some(("use-mock-keychain", ""))
  } else if cfg!(any(
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd"
  )) {
    Some(("password-store", "basic"))
  } else {
    None
  }
}

/// Whether a DevTools window may open, resolved from the policy and the build.
fn resolve_devtools_allowed(policy: DevToolsPolicy) -> bool {
  match policy {
    DevToolsPolicy::Auto => tauri::is_dev() || cfg!(feature = "devtools"),
    DevToolsPolicy::Allowed => true,
    DevToolsPolicy::Disallowed => false,
  }
}

/// A URL scheme is a lower-case ASCII letter followed by letters, digits, `+`, `-`
/// or `.`, per RFC 3986.
fn is_valid_scheme(scheme: &str) -> bool {
  let mut chars = scheme.chars();
  matches!(chars.next(), Some(first) if first.is_ascii_alphabetic())
    && chars.all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '+' | '-' | '.'))
}

impl<T: UserEvent> Runtime<T> for ElectronRuntime<T> {
  type WindowDispatcher = ElectronWindowDispatcher<T>;
  type WebviewDispatcher = ElectronWebviewDispatcher<T>;
  type Handle = ElectronRuntimeHandle<T>;
  type EventLoopProxy = EventProxy<T>;
  type RuntimeWebviewAttributes = ElectronWebviewAttributes;
  type RuntimeInitAttrs = Electron;
  type WindowOpener = NewWindowOpener;
  type Webview = Webview;

  fn new(args: RuntimeInitArgs<Electron>) -> Result<Self> {
    let identifier = args.identifier.clone();
    let mut attrs = args.runtime_init_attrs;
    // The application's own identifier wins; Tauri's is the fallback.
    if attrs.identifier.is_none() {
      attrs.identifier = Some(identifier);
    }
    Self::start(attrs, &args.custom_schemes)
  }

  #[cfg(any(
    windows,
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd"
  ))]
  fn new_any_thread(args: RuntimeInitArgs<Electron>) -> Result<Self> {
    Self::new(args)
  }

  fn create_proxy(&self) -> Self::EventLoopProxy {
    EventProxy {
      context: self.context.clone(),
    }
  }

  fn handle(&self) -> Self::Handle {
    ElectronRuntimeHandle {
      context: self.context.clone(),
    }
  }

  fn create_window<F: Fn(RawWindow) + Send + 'static>(
    &self,
    pending: PendingWindow<T, Self>,
    after_window_creation: Option<F>,
  ) -> Result<DetachedWindow<T, Self>> {
    create_window_detached(&self.context, pending, after_window_creation)
  }

  fn create_webview(
    &self,
    window_id: WindowId,
    pending: PendingWebview<T, Self>,
  ) -> Result<DetachedWebview<T, Self>> {
    create_webview_detached(&self.context, window_id, pending)
  }

  fn primary_monitor(&self) -> Option<Monitor> {
    self.handle().primary_monitor().ok().flatten()
  }

  fn monitor_from_point(&self, x: f64, y: f64) -> Option<Monitor> {
    self.handle().monitor_from_point(x, y).ok().flatten()
  }

  fn available_monitors(&self) -> Vec<Monitor> {
    self.handle().available_monitors().unwrap_or_default()
  }

  fn cursor_position(&self) -> Result<PhysicalPosition<f64>> {
    self.handle().cursor_position()
  }

  fn set_theme(&self, theme: Option<Theme>) {
    self.handle().set_theme(theme);
  }

  #[cfg(target_os = "macos")]
  fn set_activation_policy(&mut self, activation_policy: tauri_runtime::ActivationPolicy) {
    if let Err(error) = self.handle().set_activation_policy(activation_policy) {
      log::warn!("[electron] set activation policy failed: {error}");
    }
  }

  #[cfg(target_os = "macos")]
  fn set_activate_ignoring_other_apps(&mut self, ignore: bool) {
    // No Electron equivalent: `app.focus({ steal: true })` is the closest, and it
    // only affects activation, not the "ignore other apps" flag.
    if let Err(error) = self.context.ipc.call(
      "app.focus",
      serde_json::json!({ "steal": ignore }),
      IPC_TIMEOUT,
    ) {
      log::warn!("[electron] focus app failed: {error}");
    }
  }

  #[cfg(target_os = "macos")]
  fn set_dock_visibility(&mut self, visible: bool) {
    if let Err(error) = self.handle().set_dock_visibility(visible) {
      log::warn!("[electron] set dock visibility failed: {error}");
    }
  }

  #[cfg(target_os = "macos")]
  fn show(&self) {
    if let Err(error) = self.handle().show() {
      log::warn!("[electron] show app failed: {error}");
    }
  }

  #[cfg(target_os = "macos")]
  fn hide(&self) {
    if let Err(error) = self.handle().hide() {
      log::warn!("[electron] hide app failed: {error}");
    }
  }

  fn set_device_event_filter(&mut self, filter: DeviceEventFilter) {
    self.handle().set_device_event_filter(filter);
  }

  #[cfg(any(
    target_os = "macos",
    windows,
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd"
  ))]
  fn run_iteration<F: FnMut(RunEvent<T>)>(&mut self, mut callback: F) {
    self.is_running.store(true, Ordering::Relaxed);
    self.pump(&mut callback);
    self.is_running.store(false, Ordering::Relaxed);
  }

  fn run<F: FnMut(RunEvent<T>) + 'static>(self, callback: F) {
    let mut runtime = self;
    runtime.run_loop(callback);
  }

  fn run_return<F: FnMut(RunEvent<T>)>(mut self, callback: F) -> i32 {
    let mut callback = callback;
    self.run_loop(&mut callback);
    0
  }
}
