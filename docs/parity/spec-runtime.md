# Implementation spec — `runtime.rs`, `window_builder.rs`, `window_handle.rs`, `lib.rs`

Reference: `source/tauri/crates/tauri-runtime-cef/` @ `tauri-runtime-cef 3.0.0-alpha.7`
Target: `src/` (`tauri-runtime-electron`, branch `next`)
Status: contract document. Every line number below was read from the reference files, not inferred.

| reference file | lines | bytes |
| --- | --- | --- |
| `src/runtime.rs` | 4039 | 156400 |
| `src/window_builder.rs` | 565 | 16793 |
| `src/window_handle.rs` | 49 | 1568 |
| `src/lib.rs` | 54 | 1933 |

## 0. Conventions

**Rename map.** Only CEF-specific identifiers change. Everything else keeps its exact spelling.

| reference | target |
| --- | --- |
| `Cef` (builder struct) | `Electron` |
| `CefRuntime<T>` | `ElectronRuntime<T>` |
| `CefRuntimeHandle<T>` | `ElectronRuntimeHandle<T>` |
| `CefWebviewAttributes` | `ElectronWebviewAttributes` |
| `CefWebviewDispatcher<T>` | `ElectronWebviewDispatcher<T>` |
| `CefWindowDispatcher<T>` | `ElectronWindowDispatcher<T>` |
| `WinitCefApp<T>` / `TauriCefApp<T>` / `TauriCefHelperApp` | `WinitElectronApp<T>` / `TauriElectronApp<T>` / `TauriElectronHelperApp` |
| `run_cef_helper_process` | `run_electron_helper_process` |
| `cef_api_version` (builder method) | `electron_api_version` (see §7 — no Electron equivalent; keep the name only if the method is kept) |
| `API_VERSION_SWITCH = "tauri-cef-api-version"` | `"tauri-electron-api-version"` |
| `EventProxy`, `NewWindowOpener`, `NativeWindowEventHandler`, `SandboxPolicy`, `DebugEnvironment`, `SecretStorage`, `RemoteDebugging`, `DevToolsPolicy`, `CertificateErrorPolicy`, `ProxyConfig`, `AutoplayPolicy`, `WebRtcIpHandling`, `DowngradePolicy`, `WindowBuilderWrapper` | **unchanged** |

**What must not leak.** `use cef::*;` (runtime.rs:21) and `pub use cef;` (runtime.rs:89) bring CEF types into the public API surface: `cef::Settings`, `cef::ContentSettingTypes`, `cef::ContentSettingValues`, `LogSeverity` (= `cef::LogSeverity`), `LogItems` (= `cef::LogItems`), `cef::sys::CEF_API_VERSION_LAST`. The target crate has no `cef` dependency, so each of these is a hard API decision, marked in §7.

**Support-level rule used in §7.** `direct` = Electron exposes an equivalent API; `emulated` = achievable by combining Electron APIs (or by runtime-owned logic that needs no Electron API); `absent` = Electron/Chromium exposes no equivalent and the item cannot be honestly implemented.

---

## 1. Module inventory

### 1.1 `src/runtime.rs`

#### Crate attributes and imports

| line | declaration | intent |
| --- | --- | --- |
| 5 | `#![allow(clippy::arc_with_non_send_sync)]` | silence the lint for `Arc<dyn Fn>` handler storage. |
| 6 | `#![allow(clippy::too_many_arguments)]` | wrapper constructors take long positional lists. |
| 8 | `use std::{collections::HashMap, fmt, fs::create_dir_all, path::PathBuf, sync::{Arc, Mutex, atomic::{AtomicBool, AtomicPtr, AtomicU32, Ordering}, mpsc::{self, Receiver, Sender}}, time::Duration};` | std surface used across the file. |
| 21 | `use cef::*;` | pulls `Settings`, `CommandLine`, `CefString`, `LogSeverity`, `LogItems`, `State`, `sys`, … into scope unqualified. |
| 22 | `use raw_window_handle::{DisplayHandle, HasDisplayHandle};` | `RuntimeHandle::display_handle`. |
| 23 | `use tauri_runtime::{DeviceEventFilter, Error, EventLoopProxy, ExitRequestedEventAction, Result, RunEvent, Runtime, RuntimeHandle, RuntimeInitArgs, UserEvent, dpi::PhysicalPosition, monitor::Monitor, webview::{DetachedWebview, PendingWebview}, window::{DetachedWindow, DragDropEvent, PendingWindow, RawWindow, WebviewEvent, WindowEvent, WindowId}};` | the `tauri-runtime` trait surface this file implements. |
| 33 | `use tauri_utils::Theme;` | app-wide theme plumbing. |
| 34 | `use winit::{application::ApplicationHandler, data_transfer::{DataTransferId, TypeHint}, event::{StartCause, WindowEvent as WinitWindowEvent}, event_loop::{ActiveEventLoop, DndAction, EventLoop, EventLoopBuilder, EventLoopProxy as WinitEventLoopProxy}, window::WindowId as WinitWindowId};` | the winit event loop the runtime is built on. |
| 44 | `use crate::DebugEnvironment;` | re-used by the builder. |
| 45 | `use crate::external_message_pump::CefExternalPump;` | drives CEF's message pump from the winit loop. |
| 46 | `use crate::platform::EventLoopExt;` | per-OS event-loop extensions. |
| 47 | `use crate::{cef_impl::{client as browser_client, ipc, request_handler}, macros::wrap_with_args, webview::{self, AppWebview, CefWebviewAttributes, CefWebviewDispatcher, Webview, WebviewMessage, create_webview_detached}, window::{AppWindow, CefWindowDispatcher, WindowMessage, create_window_detached, winit_monitor_to_tauri_monitor, winit_theme_to_tauri_theme}, window_handle::SendRawDisplayHandle};` | crate-internal collaborators. |
| 67 | `use winit::platform::gtk4::EventLoopBuilderExtGtk4;` `#[cfg(linux/bsd)]` | `with_gtk4`. |
| 69 | `use winit::platform::macos::EventLoopBuilderExtMacOS;` `#[cfg(macos)]` | `with_default_menu`. |
| 71 | `use winit::platform::windows::EventLoopBuilderExtWindows;` `#[cfg(windows)]` | `with_msg_hook`. |

#### Type aliases and re-exports

| line | declaration | intent |
| --- | --- | --- |
| 75 | `pub type NativeWindowEventHandler = dyn Fn(&str, &dyn ActiveEventLoop, Arc<dyn winit::window::Window>, &WinitWindowEvent) + Send + Sync;` | native-event observer run before Tauri's own handling; `Destroyed` is delivered before the window is released. |
| 80 | `type SettingsCallback = dyn FnOnce(&mut cef::Settings) + Send + Sync;` | private alias for `Cef::with_settings`. |
| 89 | `pub use cef;` | re-exports the `cef` crate so applications can name `Settings`, `ContentSettingTypes`, `LogSeverity`, … |

#### Public enums (details in §3)

| line | declaration | intent |
| --- | --- | --- |
| 132 | `pub enum SecretStorage` | which key Chromium's `os_crypt` uses for cookies and saved passwords. |
| 164 | `pub enum SandboxPolicy` | what to do with Chromium's process sandbox. |
| 199 | `pub enum RemoteDebugging` | whether the DevTools protocol server runs, and over which transport. |
| 258 | `pub enum DevToolsPolicy` | whether the application may open a DevTools window at all. |
| 275 | `pub enum CertificateErrorPolicy` | interstitial vs cancel on a TLS certificate error. |
| 298 | `pub enum ProxyConfig` | how Chromium resolves the proxy for every request. |
| 357 | `pub enum AutoplayPolicy` | when a page may start playing media on its own. |
| 392 | `pub enum WebRtcIpHandling` | which local interfaces WebRTC may reveal. |
| 442 | `pub enum DowngradePolicy` | what to do with a profile a newer Chromium milestone last used. |
| 325 | `impl ProxyConfig { fn to_preference(&self) -> serde_json::Value }` | renders the Chromium `proxy` preference dictionary. |
| 373 | `impl AutoplayPolicy { fn as_switch_value(self) -> Option<&'static str> }` | `--autoplay-policy` value, `None` to leave the switch off. |
| 405 | `impl WebRtcIpHandling { fn as_switch_value(self) -> Option<&'static str> }` | `--webrtc-ip-handling-policy` value, `None` to leave the switch off. |

#### The `Cef` builder

| line | declaration | intent |
| --- | --- | --- |
| 471 | `#[derive(Default)] pub struct Cef { … }` (29 private fields, listed at 473–501) | selects and configures the runtime; passed to `tauri::Builder::runtime`. |
| 504 | `impl fmt::Debug for Cef` | prints every field; `settings_callback`/`window_event_handler` as `is_some()`. |
| 545 | `impl Cef { … }` | 37 `#[must_use]` builder methods, §2. |

`Cef` fields (all private, 473–502): `command_line_args: Vec<(String, Option<String>)>`, `disabled_features: Vec<String>`, `enabled_features: Vec<String>`, `deep_link_schemes: Vec<String>`, `cache_path: Option<PathBuf>`, `api_version: Option<i32>`, `secret_storage: SecretStorage`, `profile_preferences: Vec<(String, serde_json::Value)>`, `global_preferences: Vec<(String, serde_json::Value)>`, `content_settings: Vec<(cef::ContentSettingTypes, cef::ContentSettingValues)>`, `allow_chromium_command_line_args: bool`, `log_file: Option<PathBuf>`, `log_severity: Option<LogSeverity>`, `log_items: Option<LogItems>`, `locale: Option<String>`, `accept_language_list: Option<String>`, `user_agent: Option<String>`, `user_agent_product: Option<String>`, `javascript_flags: Option<String>`, `chrome_policy_id: Option<String>`, `persist_session_cookies: bool`, `remote_debugging: RemoteDebugging`, `devtools: DevToolsPolicy`, `debug_environment: DebugEnvironment`, `certificate_errors: CertificateErrorPolicy`, `sandbox: SandboxPolicy`, `downgrade: DowngradePolicy`, `settings_callback: Option<Box<SettingsCallback>>`, `window_event_handler: Option<Arc<NativeWindowEventHandler>>`.

#### Init-attr trait impls

| line | declaration | intent |
| --- | --- | --- |
| 1141 | `impl<T: UserEvent> tauri_runtime::RuntimeInitAttrs<T> for Cef` | binds `Cef` to `CefRuntime<T>`; §4.1. |
| 1174 | `impl<T: UserEvent> From<Cef> for tauri_runtime::dynamic::DynRuntimeInitAttrs<T>` | lets `Cef` be boxed into the dynamic runtime; §4.2. |

#### New-window opener and event proxy

| line | declaration | intent |
| --- | --- | --- |
| 1181 | `pub struct NewWindowOpener { source_url: Option<url::Url> }` | information about the webview that requested a new window. |
| 1185 | `impl NewWindowOpener` | `pub(crate) fn new(source_url: Option<url::Url>) -> Self` (1186); `pub fn source_url(&self) -> Option<&url::Url>` (1195). |
| 1200 | `impl std::fmt::Debug for NewWindowOpener` | prints `source_url_observed: bool` only — the URL can carry credentials. |
| 1211 | `#[derive(Clone, Debug)] pub struct EventProxy<T: UserEvent> { context: RuntimeContext<T> }` | the `EventLoopProxy` handed to applications. |
| 1215 | `impl<T: UserEvent> EventLoopProxy<T> for EventProxy<T>` | §4.3. |

#### Runtime context and main-thread dispatch

| line | declaration | intent |
| --- | --- | --- |
| 1222 | `pub(crate) struct RuntimeContext<T: UserEvent>` | the cloneable handle every dispatcher holds: channel sender, wake proxy, id counters, shared config. |
| 1268 | `struct MainThreadDispatch<T: UserEvent> { app: *mut WinitCefApp<T>, event_loop: *const dyn ActiveEventLoop }` | scoped access to the current winit callback state. |
| 1273 | `struct MainThreadDispatchSlot<T: UserEvent> { current: AtomicPtr<MainThreadDispatch<T>>, #[cfg(macos)] wake_pending: AtomicBool }` | lock-free slot that tells `send_message` whether a callback is active. |
| 1280 | `impl<T: UserEvent> MainThreadDispatchSlot<T>` | `install` (1281), `is_active` (1287, macOS), `deliver_wake` (1295, macOS), `restore` (1306), `current` (1311). |
| 1326 | `impl<T: UserEvent> Default for MainThreadDispatchSlot<T>` | null pointer / `false`. |
| 1336 | `struct MainThreadDispatchGuard<T: UserEvent> { context, dispatch: Box<MainThreadDispatch<T>>, previous: *mut … }` | restores the slot on drop. |
| 1342 | `impl<T: UserEvent> Drop for MainThreadDispatchGuard<T>` | `restore(self.dispatch.as_mut(), self.previous)`. |
| 1355 | `fn dispatches_run_event<T: UserEvent>(message: &Message<T>) -> bool` | `true` for messages that must end in the application's `RunEvent` callback (never handled inline). |
| 1365 | `fn handle_main_thread_message<T: UserEvent>(context: &RuntimeContext<T>, message: Message<T>) -> std::result::Result<(), Message<T>>` | runs a message inline when a callback is active; returns `Err(message)` to queue it. |
| 1398 | `impl<T: UserEvent> fmt::Debug for RuntimeContext<T>` | prints nothing (`debug_struct("RuntimeContext").finish()`). |
| 1404 | `impl<T: UserEvent> RuntimeContext<T>` | `send_message` (1405), `wake_event_loop` (1427), `is_main_thread` (1454), `wait_for_creation` (1465), `run_on_main_thread` (1487), `next_window_id` (1496), `next_webview_id` (1500). |
| 1505 | `pub(crate) type AfterWindowCreationCallback = Box<dyn for<'a> Fn(RawWindow<'a>) + Send>;` | callback run once a window exists. |

#### Message plumbing

| line | declaration | intent |
| --- | --- | --- |
| 1507 | `pub(crate) enum Message<T: UserEvent>` | every cross-thread request; 21 variants, 1508–1578. |
| 1581 | `fn device_event_filter_to_winit(filter: DeviceEventFilter) -> winit::event_loop::DeviceEvents` | maps Tauri's filter to winit's (note the inverted mapping). |
| 1589 | `pub(crate) enum EventLoopMessage` | event-loop-thread-only requests: `SetTheme`, `SetDeviceEventFilter`, `PrimaryMonitor`, `MonitorFromPoint`, `AvailableMonitors`, `CursorPosition`, `DisplayHandle`, plus macOS `SetActivationPolicy`, `SetDockVisibility`, `ShowApplication`, `HideApplication`. |
| 1607 | `#[derive(Debug)] pub(crate) struct WinitDragDropState { id, paths, paths_requested, enter_position, latest_position, enter_emitted, drop_pending }` | winit-side drag-and-drop bookkeeping. |
| 1618 | `impl WinitDragDropState { fn position(&self) -> PhysicalPosition<f64> }` | latest position, else enter position, else default. |
| 1627 | `fn pending_native_drag_enter(native_drag_drop: &mut Option<WinitDragDropState>) -> Option<DragDropEvent>` | emits `Enter` once paths are known. |
| 1643 | `fn pending_native_drag_drop(native_drag_drop: &mut Option<WinitDragDropState>) -> Option<DragDropEvent>` | emits `Drop` once a drop is pending and `Enter` went out. |
| 1658 | `fn request_native_drag_paths(event_loop: &dyn ActiveEventLoop, native_drag_drop: &mut Option<WinitDragDropState>)` | asks winit for the `UriList` payload; drops the drag on failure. |
| 1681 | `macro_rules! event_loop_getter { ($self:ident, $variant:ident) => … }` | sends an `EventLoopMessage` and blocks on its reply channel. |
| 1694 | `fn find_monitor_from_point(monitors: impl Iterator<Item = winit::monitor::MonitorHandle>, x: f64, y: f64) -> Option<winit::monitor::MonitorHandle>` | hit-tests monitors by position + video-mode size. |
| 1713 | `fn is_cef_helper_process() -> bool` `#[cfg(macos)]` | detects a `… Helper (GPU/Renderer/Plugin/Alerts)` executable by name. |

#### Application state and the winit application handler

| line | declaration | intent |
| --- | --- | --- |
| 1733 | `pub(crate) struct AppState<T: UserEvent> { windows, closing_windows, winid_id_to_window_id_map, callback, live_browsers, live_popups, pending_popups, exiting }` | everything the run loop owns. |
| 1747 | `pub(crate) struct WinitCefApp<T: UserEvent> { context, receiver, state, scheme_registry }` | the `ApplicationHandler` implementation. |
| 1754 | `impl<T: UserEvent> WinitCefApp<T>` | `new` (1755), `run_callback` (1778), `install_current_dispatch` (1782), `drain_messages` (1800), `handle_message` (1806), `handle_event_loop_message` (2009), `remove_scheme_handler_entries` (2065), `emit_drag_drop_event` (2072), `emit_window_event` (2089), `emit_webview_event` (2098), `request_exit` (2114), `pub(crate) close_window` (2131), `pub(crate) request_window_close` (2202), `navigate_first_webview` (2229), `close_all_browsers` (2243), `set_browsers_accessibility_state` (2257, macOS), `exit_if_done` (2271). |
| 2302 | `impl<T: UserEvent> ApplicationHandler for WinitCefApp<T>` | `can_create_surfaces` (2303), `new_events` (2308), `proxy_wake_up` (2321), `about_to_wait` (2326), `window_event` (2340). |

#### Command line, helper process, version switch

| line | declaration | intent |
| --- | --- | --- |
| 2559 | `fn deep_link_arguments<I>(args: I, schemes: &[String]) -> Vec<String> where I: IntoIterator<Item = String>` | keeps only arguments that parse as URLs with a configured scheme. |
| 2578 | `fn append_command_line_args(command_line: &mut CommandLine, args: &[(String, Option<String>)])` | appends each entry as switch-with-value, `--`-prefixed bare switch, or positional argument. |
| 2593 | `wrap_with_args! { wrap_app => TauriCefAppArgs; struct TauriCefApp<T: UserEvent> { context, context_initialized, deep_link_schemes, restore_deep_link_arguments, internal_command_line_args, browser_command_line_args, api_version, disabled_features, enabled_features } impl App { … } }` | the CEF `App`: `render_process_handler` (2632), `browser_process_handler` (2636), `on_before_command_line_processing` (2645). |
| 2712 | `pub(crate) const API_VERSION_SWITCH: &str = "tauri-cef-api-version";` | passes the browser process's declared CEF API version to child processes. |
| 2720 | `fn child_process_api_version<I>(args: I) -> Option<i32> where I: IntoIterator<Item = std::ffi::OsString>` | reads `--tauri-cef-api-version=` but only when `--type=` is present; last occurrence wins. |
| 2741 | `pub fn run_cef_helper_process()` | entry point for non-browser CEF processes: sandbox, library loader, version check, `execute_process`. |
| 2776 | `wrap_app! { struct TauriCefHelperApp; impl App { fn render_process_handler(…) } }` | the child-process `App`. |

#### Runtime handle and runtime

| line | declaration | intent |
| --- | --- | --- |
| 2787 | `#[derive(Debug, Clone)] pub struct CefRuntimeHandle<T: UserEvent> { context: RuntimeContext<T> }` | the `RuntimeHandle` given to applications. |
| 2791 | `impl<T: UserEvent> RuntimeHandle<T> for CefRuntimeHandle<T>` | §4.4. |
| 2927 | `pub struct CefRuntime<T: UserEvent = tauri::EventLoopMessage> { event_loop, receiver, context, scheme_registry, #[cfg(macos)] _app_delegate }` | the runtime itself. |
| 2936 | `impl<T: UserEvent> fmt::Debug for CefRuntime<T>` | prints nothing. |
| 2958 | `struct TerminationSignals { sigint, sigterm, sighup }` `#[cfg(linux/bsd)]` | saves/restores the process signal policy CEF overwrites. |
| 2971 | `impl TerminationSignals` | `capture` (2972), `restore` (2982), `capture_one` (2988), `restore_one` (2997). |
| 3006 | `impl<T: UserEvent> CefRuntime<T> { fn init(mut event_loop_builder: EventLoopBuilder, runtime_args: RuntimeInitArgs<Cef>) -> Result<Self> }` | the whole startup sequence: sandbox, loader, version, child-process branch, switch assembly, cache path, downgrade, event loop, `execute_process`, `cef::initialize`, wait for context init, global preferences. |
| 3570 | `impl<T: UserEvent> Runtime<T> for CefRuntime<T>` | §4.5. |

#### Tests

| line | declaration | intent |
| --- | --- | --- |
| 3733 | `mod configuration_tests` | 9 tests: proxy spelling, modeless modes, default policies append no switch, Chromium's own spellings, `RemoteDebugging::Disabled` default, conservative defaults, typed option = preference, later preference wins. |
| 3881 | `mod child_process_api_version_tests` | 7 tests: reads the passed version, switch spelling, absent = default, garbage = default, last occurrence wins, browser process ignores it, non-Unicode skipped. |
| 3985 | `mod deep_link_argument_tests` | 4 tests: keeps configured links in order, drops everything else, empty scheme list keeps nothing, scheme matching is exact. |

20 `#[test]` functions in total (3736, 3754, 3770, 3788, 3796, 3810, 3815, 3840, 3860, 3888, 3906, 3912, 3923, 3940, 3962, 3970, 3996, 4007, 4025, 4030).

### 1.2 `src/window_builder.rs`

| line | declaration | intent |
| --- | --- | --- |
| 5 | `use tauri_runtime::{Icon, Result, dpi::{PhysicalSize, Size}, window::{WindowBuilder, WindowBuilderBase, WindowSizeConstraints}};` | the trait being implemented. |
| 10 | `use tauri_utils::{Theme, config::{Color, PreventOverflowConfig, WindowConfig}};` | config types. |
| 14 | `use winit::{dpi::{LogicalPosition, LogicalSize}, monitor::Fullscreen, window::{WindowAttributes, WindowButtons}};` | the winit attribute store. |
| 20 | `use crate::window::{AppWindowAttrs, max_size_constraint, min_size_constraint, tauri_theme_to_winit_theme, winit_theme_to_tauri_theme};` | crate-internal helpers. |
| 25 | `#[cfg(any(windows, macos))] use winit::raw_window_handle::RawWindowHandle;` | `parent`. |
| 28 | `#[cfg(macos)] use std::ptr::NonNull;` / `use tauri_runtime::dpi::Position;` / `use tauri_utils::TitleBarStyle;` | macOS-only builder inputs. |
| 35 | `#[cfg(windows)] use std::num::NonZeroIsize;` / `use windows::Win32::Foundation::HWND;` | Windows-only builder inputs. |
| 40 | `#[derive(Clone, Default, Debug)] pub struct WindowBuilderWrapper { pub(crate) attrs: AppWindowAttrs }` | the `WindowBuilder` implementation, wrapping winit's `WindowAttributes` plus Tauri-only fields. |
| 45 | `unsafe impl Send for WindowBuilderWrapper {}` | the attrs hold raw platform handles. |
| 47 | `impl WindowBuilderBase for WindowBuilderWrapper {}` | marker impl, no members. |
| 49 | `impl WindowBuilder for WindowBuilderWrapper` | 44 methods, §5. |
| 553 | `#[cfg(windows)] type PlatformAttributes = winit::platform::windows::WindowAttributesWindows;` | per-OS platform attribute type. |
| 555 | `#[cfg(macos)] type PlatformAttributes = winit::platform::macos::WindowAttributesMacOS;` | same, macOS. |
| 559 | `#[cfg(any(windows, macos))] fn platform_attrs(attrs: &mut WindowAttributes) -> Box<PlatformAttributes>` | takes the platform attrs out of `attrs` (or defaults) so a `with_*` can be applied. |

### 1.3 `src/window_handle.rs`

| line | declaration | intent |
| --- | --- | --- |
| 5 | `#[cfg(windows)] use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};` | Windows-only handle plumbing. |
| 7 | `#[cfg(windows)] use winit::window::Window as WinitWindow;` | source of the raw handles. |
| 10 | `pub(crate) struct SendRawWindowHandle(pub raw_window_handle::RawWindowHandle);` + `unsafe impl Send for SendRawWindowHandle {}` (11) | makes a raw window handle movable across the event-loop thread boundary. |
| 13 | `pub(crate) struct SendRawDisplayHandle(pub raw_window_handle::RawDisplayHandle);` + `unsafe impl Send for SendRawDisplayHandle {}` (14) | same for the display handle; used by `RuntimeHandle::display_handle`. |
| 16 | `#[cfg(windows)] #[derive(Clone, Copy, Debug)] pub(crate) struct SoftbufferWindowHandle { display: RawDisplayHandle, window: RawWindowHandle }` | a `HasWindowHandle + HasDisplayHandle` pair for the Windows software-rendered background surface. |
| 23 | `#[cfg(windows)] impl SoftbufferWindowHandle { pub(crate) fn new(window: &dyn WinitWindow) -> Option<Self> }` | captures both raw handles, `None` if either is unavailable. |
| 33 | `#[cfg(windows)] impl HasDisplayHandle for SoftbufferWindowHandle` | borrows the stored display handle. |
| 42 | `#[cfg(windows)] impl HasWindowHandle for SoftbufferWindowHandle` | borrows the stored window handle. |

### 1.4 `src/lib.rs`

| line | declaration | intent |
| --- | --- | --- |
| 5 | `#![allow(clippy::arc_with_non_send_sync)]` | handler storage. |
| 6 | `#![allow(clippy::too_many_arguments)]` | wrapper constructors. |
| 8 | `mod cef_impl;` | CEF client/handler implementations. |
| 9 | `mod devtools;` | DevTools message-id allocation. |
| 10 | `mod dialog;` | native dialog observation. |
| 13 | `/// The `Last Version` breadcrumb in the root cache path, and what a rollback to an older CEF does with the profile a newer one left there.` (11–12) `mod downgrade;` | profile downgrade handling. |
| 15 | `/// The diagnostic environment variables Chromium and CEF read, and the policy over them.` (14) `mod environment;` | `DebugEnvironment`. |
| 16 | `mod external_message_pump;` | CEF's external message pump. |
| 17 | `mod frame;` | frame events. |
| 22 | `mod frame_navigation;` | frame navigation state. |
| 24 | `/// The languages the user asked their operating system for.` (23) `mod locale;` | system `Accept-Language`. |
| 25 | `mod macros;` | `wrap_with_args!`. |
| 26 | `mod offscreen;` | offscreen rendering. |
| 28 | `mod platform;` | per-OS extensions. |
| 29 | `mod popup;` | popup families. |
| 30 | `mod runtime;` | this spec's main module. |
| 33 | `mod sandbox;` | the decision behind `SandboxPolicy`. |
| 35 | `/// Helpers for the Chromium command line the runtime hands to CEF.` (34) `mod switches;` | switch merging and warnings. |
| 36 | `mod tauri_ext;` | extension traits for `tauri`. |
| 37 | `mod webview;` | webview dispatch. |
| 41 | `mod window;` | window dispatch. |
| 42 | `mod window_builder;` | `WindowBuilderWrapper`. |
| 43 | `mod window_handle;` | raw-handle wrappers (private module — nothing is re-exported). |
| 18 | `pub use dialog::{NativeDialogKind, NativeDialogObservation, NativeDialogSnapshot, NativeDialogToken};` | dialog surface. |
| 21 | `pub use environment::DebugEnvironment;` | the debug-environment policy. |
| 27 | `pub use offscreen::{CursorHandler, Offscreen, OffscreenView};` | offscreen surface. |
| 38 | `pub use devtools::{DevToolsMessageIdExhausted, allocate_devtools_message_id};` | DevTools message ids. |
| 39 | `pub use frame::{FrameEvent, FrameEventHandler, FrameEventKind};` | frame events. |
| 40 | `pub use frame_navigation::{FrameNavigationState, NativeDocumentToken};` | frame navigation. |
| 45 | `pub use cef::sys::CEF_API_VERSION_LAST;` | the compiled-in CEF API version. |
| 46 | `#[cfg(target_os = "macos")] pub use platform::macos::setup_application as prepare_macos_application;` | macOS application bootstrap. |
| 48 | `pub use runtime::*;` | glob: every `pub` item of `runtime.rs` (§6). |
| 49 | `pub use tauri_ext::*;` | extension traits. |
| 50 | `/// Marks the application entry point so non-browser CEF processes (renderer, GPU, ...) are handled.` `pub use tauri_macros::cef_entry_point;` | the `#[cef_entry_point]` attribute macro. |
| 52 | `pub use webview::*;` | webview surface. |
| 53 | `pub use window::{CefWindowDispatcher, NativeWindowToken};` | window dispatcher and native token. |
| 54 | `pub use window_builder::WindowBuilderWrapper;` | the builder. |

Note: `window_handle.rs` exports nothing publicly. `CefRuntimeHandle` lives in `runtime.rs`, not `window_handle.rs` — the target crate's current `src/window_handle.rs` (which holds `ElectronRuntimeHandle`) does **not** mirror this split and must be corrected to match.

---

## 2. `Cef` builder surface

All 37 methods are `#[must_use]`, take `mut self`, and return `Self`. **Every one is pure config at call time** — none touches the OS, the filesystem, or Chromium when called. The "reaches" column therefore records what the value affects at `CefRuntime::init` time.

| # | line | signature | configures | mechanism | default when unset | reaches |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | 553 | `pub fn on_window_event<F>(mut self, handler: F) -> Self where F: Fn(&str, &dyn ActiveEventLoop, Arc<dyn winit::window::Window>, &WinitWindowEvent) + Send + Sync + 'static` | a native-event observer run before Tauri's handling | stored in `window_event_handler`, invoked from `WinitCefApp::window_event` and `close_window` | `None` | pure-config (runtime callback) |
| 2 | 568 | `pub fn with_settings<F>(mut self, callback: F) -> Self where F: FnOnce(&mut cef::Settings) + Send + Sync + 'static` | last-chance mutation of `cef::Settings` | called at runtime.rs:3496, immediately before `cef::initialize` | `None` | chromium |
| 3 | 583 | `pub fn command_line_arg<K: Into<String>, V: Into<String>>(mut self, key: K, value: Option<V>) -> Self` | one switch, browser process only | `browser_command_line_args` → `CommandLine::append_switch_with_value` / `append_switch` / `append_argument` in `on_before_command_line_processing` | empty vec | chromium |
| 4 | 598 | `pub fn command_line_args<K: Into<String>, V: Into<String>>(mut self, args: impl IntoIterator<Item = (K, Option<V>)>) -> Self` | many switches, browser process only | same as #3 | empty vec | chromium |
| 5 | 612 | `pub fn deep_link_schemes<S: Into<String>>(mut self, schemes: impl IntoIterator<Item = S>) -> Self` | schemes recognised on relaunch | `deep_link_arguments()` filter feeding `BrowserProcessHandler::on_already_running_app_relaunch`; `apply_config` also merges the `deep-link` plugin config | empty vec | pure-config (feeds the OS relaunch path) |
| 6 | 631 | `pub fn root_cache_path<P: AsRef<std::path::Path>>(mut self, path: P) -> Self` | Chromium user-data directory | `Settings::cache_path`; also holds the `Last Version` breadcrumb | `dirs::cache_dir()` (fallback `temp_dir()`) `/ {identifier} / "cef"` | chromium + os (filesystem) |
| 7 | 643 | `pub fn downgrade(mut self, policy: DowngradePolicy) -> Self` | rollback behaviour | `crate::downgrade::prepare_root_cache_path`, before the first CEF call | `DowngradePolicy::KeepProfile` | os (filesystem) |
| 8 | 655 | `pub fn cef_api_version(mut self, version: i32) -> Self` | declared CEF ABI version | `cef::api_hash(version, 0)` gate + `--tauri-cef-api-version` for children | `cef::sys::CEF_API_VERSION_LAST` | chromium |
| 9 | 673 | `pub fn secret_storage(mut self, storage: SecretStorage) -> Self` | key used by `os_crypt` | macOS `--use-mock-keychain`; Linux `--password-store=basic`; Windows untouched (DPAPI) | `SecretStorage::Auto` | os (keychain/keyring) + chromium |
| 10 | 705 | `pub fn profile_preference<K: Into<String>>(mut self, name: K, enabled: bool) -> Self` | one boolean profile preference | request-context `PreferenceManager` | empty vec | chromium |
| 11 | 741 | `pub fn profile_preference_value<K: Into<String>, V: Into<serde_json::Value>>(mut self, name: K, value: V) -> Self` | one typed profile preference | same as #10 | empty vec | chromium |
| 12 | 757 | `pub fn global_preference<K: Into<String>, V: Into<serde_json::Value>>(mut self, name: K, value: V) -> Self` | one local-state preference | `preference_manager_get_global`, after context init | empty vec | chromium |
| 13 | 803 | `pub fn default_content_setting(mut self, content_type: cef::ContentSettingTypes, value: cef::ContentSettingValues) -> Self` | default answer for a permission, every origin | request-context `ContentSetting` default | empty vec | chromium |
| 14 | 833 | `pub fn allow_chromium_command_line_args(mut self, allow: bool) -> Self` | whether release builds honour Chromium switches from the process command line | `Settings::command_line_args_disabled` | `false` (dev builds always allow) | chromium |
| 15 | 848 | `pub fn log_file<P: AsRef<std::path::Path>>(mut self, path: P) -> Self` | log destination | `Settings::log_file` | `{cache_path}/cef.log` | chromium + os (file) |
| 16 | 864 | `pub fn log_severity(mut self, severity: LogSeverity) -> Self` | log verbosity | `Settings::log_severity` | dev: `LogSeverity::DEFAULT`; release: `LogSeverity::WARNING` | chromium |
| 17 | 879 | `pub fn locale<S: Into<String>>(mut self, locale: S) -> Self` | Chromium's own UI locale | `Settings::locale` | unset (CEF default; only `en-US` pak ships) | chromium |
| 18 | 890 | `pub fn accept_language_list<S: Into<String>>(mut self, languages: S) -> Self` | `Accept-Language` / `navigator.language` | `Settings::accept_language_list` | `crate::locale::system_accept_language_list()` | os (reads system locale) + chromium |
| 19 | 906 | `pub fn sandbox(mut self, policy: SandboxPolicy) -> Self` | Chromium process sandbox | `crate::sandbox::resolve_sandbox_decision` → `Settings::no_sandbox` (never a hand-appended `--no-sandbox`) | `SandboxPolicy::Auto` | os (process sandbox) + chromium |
| 20 | 919 | `pub fn disable_features<S: Into<String>>(mut self, features: impl IntoIterator<Item = S>) -> Self` | `--disable-features`, merged | `switches::append_merged_switch` | empty vec | chromium |
| 21 | 933 | `pub fn enable_features<S: Into<String>>(mut self, features: impl IntoIterator<Item = S>) -> Self` | `--enable-features`, merged | `switches::append_merged_switch` | empty vec | chromium |
| 22 | 952 | `pub fn debug_environment(mut self, policy: DebugEnvironment) -> Self` | whether `SSLKEYLOGFILE` and the crash-reporter overrides are honoured | empty `--ssl-key-log-file` + `std::env::remove_var` of the crash overrides | `DebugEnvironment::Auto` | os (process environment) |
| 23 | 965 | `pub fn remote_debugging(mut self, remote_debugging: RemoteDebugging) -> Self` | DevTools protocol server | `--remote-debugging-port` / `--remote-debugging-pipe` / `--remote-allow-origins`; pins `devtools.remote_debugging.allowed` off when disabled | `RemoteDebugging::Disabled` | chromium (+ network listener) |
| 24 | 976 | `pub fn devtools(mut self, policy: DevToolsPolicy) -> Self` | whether a DevTools window may open at all | runtime gate `devtools_allowed`; pins `devtools.availability` to allowed | `DevToolsPolicy::Auto` | pure-config + chromium (pref pin) |
| 25 | 986 | `pub fn certificate_errors(mut self, policy: CertificateErrorPolicy) -> Self` | TLS certificate-error handling | `RuntimeContext::certificate_errors`, consumed by the request handler | `CertificateErrorPolicy::ChromeInterstitial` | pure-config (runtime handler) |
| 26 | 995 | `pub fn proxy(mut self, proxy: ProxyConfig) -> Self` | proxy resolution | pushes `("proxy", proxy.to_preference())` into `profile_preferences` | `ProxyConfig::System` (no preference written) | chromium |
| 27 | 1004 | `pub fn autoplay(mut self, policy: AutoplayPolicy) -> Self` | autoplay gate | `--autoplay-policy` | `AutoplayPolicy::Default` (no switch) | chromium |
| 28 | 1015 | `pub fn webrtc_ip_handling(mut self, policy: WebRtcIpHandling) -> Self` | WebRTC interface exposure | `--webrtc-ip-handling-policy` | `WebRtcIpHandling::Default` (no switch) | chromium |
| 29 | 1032 | `pub fn spell_checking(mut self, enabled: bool) -> Self` | spell checker | profile preference `browser.enable_spellchecking` | on (no preference written) | chromium |
| 30 | 1048 | `pub fn safe_browsing(mut self, enabled: bool) -> Self` | Safe Browsing | profile preference `safebrowsing.enabled` | on (no preference written) | chromium |
| 31 | 1063 | `pub fn component_updates(mut self, enabled: bool) -> Self` | Chromium component updater | `--disable-component-update` (only when `false`) | on (no switch) | chromium |
| 32 | 1079 | `pub fn user_agent<S: Into<String>>(mut self, user_agent: S) -> Self` | whole User-Agent string | `Settings::user_agent`; logs a warning if `user_agent_product` was also set | `None` | chromium |
| 33 | 1089 | `pub fn user_agent_product<S: Into<String>>(mut self, product: S) -> Self` | product token spliced into Chromium's UA | `Settings::user_agent_product`; ignored when `user_agent` is set | `None` | chromium |
| 34 | 1101 | `pub fn persist_session_cookies(mut self, persist: bool) -> Self` | whether session cookies survive a restart | `Settings::persist_session_cookies` | `false` | chromium |
| 35 | 1112 | `pub fn javascript_flags<S: Into<String>>(mut self, flags: S) -> Self` | V8 flags | `Settings::javascript_flags` | `None` | chromium |
| 36 | 1126 | `pub fn chrome_policy_id<S: Into<String>>(mut self, policy_id: S) -> Self` | Chrome enterprise policy source | `Settings::chrome_policy_id` | `None` | os (reads platform policy store) + chromium |
| 37 | 1135 | `pub fn log_items(mut self, items: LogItems) -> Self` | fields CEF prepends to each log line | `Settings::log_items` | CEF's own choice | chromium |

**Runtime-side defaults the builder does not expose** (must be reproduced verbatim; they are part of the contract even though no method sets them):

- `internal_command_line_args` always gets `--no-first-run` (runtime.rs:3387).
- `profile_preferences[0]` is forced to `devtools.availability = allowed` (3233–3239) — *never* the disallowing value, because CEF gates `SendDevToolsMessage` on it.
- `global_preferences[0]` gets `devtools.remote_debugging.allowed = false` whenever no transport was configured (3292–3300), including a rejected port.
- `Settings::external_message_pump = 1` (3450).
- `RemoteDebugging::Port` below 1024 is rejected with a warning (3257).
- Linux/BSD additionally force `--ozone-platform=x11`, `GDK_BACKEND=x11`, `gdk::set_allowed_backends("x11")`, `with_gtk4()`, `gtk::declare_version(V4)` (3318–3343).
- macOS: `event_loop_builder.with_default_menu(false)` (3352).

---

## 3. Public enums

All nine are `pub`, `#[non_exhaustive]`, derive `Debug + Default + Clone + PartialEq + Eq` (plus `Copy` where noted), and mark exactly one variant `#[default]`.

**There are no `serde` impls and no `FromStr` impls on any of these enums.** The only serde derive in `runtime.rs` is a private helper (`#[derive(serde::Deserialize)] enum DesktopDeepLinks`, line 1151, inside `apply_config`). Any target implementation that adds serde/`FromStr` is *adding* API, not mirroring it — do not add them without an explicit decision.

### 3.1 `SecretStorage` — line 132

`#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)] #[non_exhaustive]`

| line | variant | intent |
| --- | --- | --- |
| 137 | `Auto` (`#[default]`) | skip the OS secret store in dev builds (`tauri::is_dev()`), use it in release. |
| 143 | `Mock` | always use Chromium's hard-coded constant: `--use-mock-keychain` (macOS), `--password-store=basic` (Linux). |
| 146 | `System` | always use the OS secret store; appends no switch. |

Doc intent: only cookies and saved passwords are encrypted by `os_crypt`; everything else is plaintext in the cache directory. Mock/basic use a publicly known constant, so they give no protection at rest. Switching modes invalidates stored cookies.

### 3.2 `SandboxPolicy` — line 164

`#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)] #[non_exhaustive]`

| line | variant | intent |
| --- | --- | --- |
| 176 | `Auto` (`#[default]`) | keep the sandbox where possible, warn where not (Windows always; Linux/BSD AppImage without setuid helper or userns). |
| 183 | `Required` | never run unsandboxed: fail startup instead. |
| 189 | `Disabled` | always run unsandboxed, on every platform. |

Doc intent: a Windows build currently runs unsandboxed whatever this says, because CEF needs a Chromium-toolchain sandbox broker; the policy only decides whether the runtime warns (`Auto`) or refuses (`Required`).

### 3.3 `RemoteDebugging` — line 199

`#[derive(Debug, Default, Clone, PartialEq, Eq)]` — **no `Copy`** (holds a `Vec<String>`).

| line | variant | intent |
| --- | --- | --- |
| 204 | `Disabled` (`#[default]`) | no server; additionally pins `devtools.remote_debugging.allowed` off. |
| 216 | `Port { port: u16, allowed_origins: Vec<String> }` | listen on a TCP port, as `--remote-debugging-port`; port must be 1024–65535. |
| 227 | `Pipe` | speak the protocol over inherited file descriptors, as `--remote-debugging-pipe`. |

### 3.4 `DevToolsPolicy` — line 258

`#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)] #[non_exhaustive]`

| line | variant | intent |
| --- | --- | --- |
| 262 | `Auto` (`#[default]`) | allow in a build that could open them anyway (debug, or `devtools` feature), refuse otherwise. |
| 267 | `Allowed` | allow whatever the build profile; per-webview `devtools` still applies. |
| 269 | `Disallowed` | refuse whatever the build profile. |

Doc intent: application-wide gate combined with per-webview `WebviewAttributes::devtools`; deliberately **not** Chromium's `devtools.availability` preference, because CEF silently refuses `SendDevToolsMessage` on a profile carrying `kDisallowed` and the runtime drives its own startup over the DevTools protocol. The protocol stays available whatever this says.

### 3.5 `CertificateErrorPolicy` — line 275

`#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)] #[non_exhaustive]`

| line | variant | intent |
| --- | --- | --- |
| 282 | `ChromeInterstitial` (`#[default]`) | show Chrome's SSL interstitial, which offers a way through. |
| 288 | `Cancel` | cancel the request; no interstitial, no override. |

### 3.6 `ProxyConfig` — line 298

`#[derive(Debug, Default, Clone, PartialEq, Eq)]` — **no `Copy`** (holds `String`s).

| line | variant | intent |
| --- | --- | --- |
| 304 | `System` (`#[default]`) | use the OS proxy configuration. |
| 306 | `Direct` | connect directly, ignoring any system proxy. |
| 308 | `AutoDetect` | discover a proxy through WPAD. |
| 310 | `PacScript { url: String }` | fetch a PAC script from `url`. |
| 315 | `FixedServers { server: String, bypass_list: Option<String> }` | fixed proxy; `scheme://host:port` (bare `host:port` means HTTP), semicolon-delimited bypass list. |

Helper: `fn to_preference(&self) -> serde_json::Value` (line 327, **private**). Exact output:

| variant | JSON |
| --- | --- |
| `System` | `{"mode":"system"}` |
| `Direct` | `{"mode":"direct"}` |
| `AutoDetect` | `{"mode":"auto_detect"}` |
| `PacScript` | `{"mode":"pac_script","pac_url":<url>}` |
| `FixedServers` | `{"mode":"fixed_servers","server":<server>}` + `"bypass_list":<list>` **only when `Some`** |

The absent-vs-empty bypass list matters: Chromium rejects the whole `proxy` dictionary when it carries a key the mode does not accept (asserted by the test at 3755).

### 3.7 `AutoplayPolicy` — line 357

`#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)] #[non_exhaustive]`

| line | variant | intent |
| --- | --- | --- |
| 361 | `Default` (`#[default]`) | Chromium's own default (audible media needs a document gesture). |
| 366 | `NoUserGestureRequired` | play without any user interaction. |
| 368 | `UserGestureRequired` | require a gesture on the media element. |
| 370 | `DocumentUserActivationRequired` | require the user to have interacted with the document. |

Helper: `fn as_switch_value(self) -> Option<&'static str>` (line 375, **private**): `Default → None`, `NoUserGestureRequired → Some("no-user-gesture-required")`, `UserGestureRequired → Some("user-gesture-required")`, `DocumentUserActivationRequired → Some("document-user-activation-required")`. Values are **hyphenated**.

### 3.8 `WebRtcIpHandling` — line 392

`#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)] #[non_exhaustive]`

| line | variant | intent |
| --- | --- | --- |
| 395 | `Default` (`#[default]`) | Chromium's default. |
| 397 | `DefaultPublicAndPrivateInterfaces` | offer public and private interfaces (reveals the LAN address). |
| 399 | `DefaultPublicInterfaceOnly` | offer only the public interface. |
| 402 | `DisableNonProxiedUdp` | refuse UDP that does not go through the proxy. |

Helper: `fn as_switch_value(self) -> Option<&'static str>` (line 407, **private**): `Default → None`, `DefaultPublicAndPrivateInterfaces → Some("default_public_and_private_interfaces")`, `DefaultPublicInterfaceOnly → Some("default_public_interface_only")`, `DisableNonProxiedUdp → Some("disable_non_proxied_udp")`. Values are **underscored** — deliberately different from the autoplay spelling (asserted at 3797).

### 3.9 `DowngradePolicy` — line 442

`#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)] #[non_exhaustive]`

| line | variant | intent |
| --- | --- | --- |
| 450 | `KeepProfile` (`#[default]`) | open the profile as is and log a warning naming both versions. |
| 459 | `ResetProfile` | move the root cache path aside and start empty, deleting the old one in the background. |

Doc intent: Chromium migrates a profile forward only; the runtime writes the `Last Version` breadcrumb CEF does not write, and applies this policy when the breadcrumb names a higher milestone. A downgrade within a milestone is left alone, and a profile another running instance holds is never touched.

---

## 4. Trait implementations

### 4.1 `impl<T: UserEvent> tauri_runtime::RuntimeInitAttrs<T> for Cef` — line 1141

| member | line | what it does |
| --- | --- | --- |
| `type Runtime = CefRuntime<T>;` | 1142 | binds the attrs to the runtime. |
| `fn apply_config(&mut self, config: &tauri_utils::config::Config) -> Result<()>` | 1144 | reads `plugins["deep-link"].desktop`, deserialises it as one `DeepLinkProtocol` or a list (private `DesktopDeepLinks` enum, 1151), and extends `deep_link_schemes` with every `schemes` entry. Always `Ok(())`; a malformed config is `Error::Json`. |

### 4.2 `impl<T: UserEvent> From<Cef> for tauri_runtime::dynamic::DynRuntimeInitAttrs<T>` — line 1174

| member | line | what it does |
| --- | --- | --- |
| `fn from(attrs: Cef) -> Self` | 1175 | `Self::new(attrs)` — boxes the builder for the dynamic runtime. |

### 4.3 `impl<T: UserEvent> EventLoopProxy<T> for EventProxy<T>` — line 1215

| member | line | what it does |
| --- | --- | --- |
| `fn send_event(&self, event: T) -> Result<()>` | 1216 | `self.context.send_message(Message::UserEvent(event))`. |

### 4.4 `impl<T: UserEvent> RuntimeHandle<T> for CefRuntimeHandle<T>` — line 2791

| member | line | cfg | what it does |
| --- | --- | --- | --- |
| `type Runtime = CefRuntime<T>;` | 2792 | | associated runtime. |
| `fn create_proxy(&self) -> <Self::Runtime as Runtime<T>>::EventLoopProxy` | 2794 | | clones the context into a new `EventProxy`. |
| `fn set_activation_policy(&self, activation_policy: tauri_runtime::ActivationPolicy) -> Result<()>` | 2801 | macOS | sends `EventLoopMessage::SetActivationPolicy`. |
| `fn set_dock_visibility(&self, visible: bool) -> Result<()>` | 2810 | macOS | sends `EventLoopMessage::SetDockVisibility`. |
| `fn request_exit(&self, code: i32) -> Result<()>` | 2815 | | sends `Message::RequestExit(code)`. |
| `fn custom_scheme_url(&self, scheme: &str, https: bool) -> String` | 2822 | | `"{http or https}://{scheme}.localhost"` — CEF's custom-scheme URL form. |
| `fn webview_version(&self) -> Result<String>` | 2829 | | delegates to `crate::webview_version()`. |
| `fn create_window<F: Fn(RawWindow<'_>) + Send + 'static>(&self, pending: PendingWindow<T, Self::Runtime>, after_window_creation: Option<F>) -> Result<DetachedWindow<T, Self::Runtime>>` | 2833 | | `create_window_detached`. |
| `fn create_webview(&self, window_id: WindowId, pending: PendingWebview<T, Self::Runtime>) -> Result<DetachedWebview<T, Self::Runtime>>` | 2841 | | `create_webview_detached`. |
| `fn run_on_main_thread<F: FnOnce() + Send + 'static>(&self, f: F) -> Result<()>` | 2849 | | `RuntimeContext::run_on_main_thread` (inline when already on the main thread). |
| `fn display_handle(&self) -> std::result::Result<DisplayHandle<'_>, raw_window_handle::HandleError>` | 2853 | | round-trips `EventLoopMessage::DisplayHandle`, then `DisplayHandle::borrow_raw`. |
| `fn primary_monitor(&self) -> Result<Option<Monitor>>` | 2863 | | `event_loop_getter!(self, PrimaryMonitor)`. |
| `fn monitor_from_point(&self, x: f64, y: f64) -> Result<Option<Monitor>>` | 2867 | | `EventLoopMessage::MonitorFromPoint`. |
| `fn available_monitors(&self) -> Result<Vec<Monitor>>` | 2877 | | `event_loop_getter!(self, AvailableMonitors)`. |
| `fn cursor_position(&self) -> Result<PhysicalPosition<f64>>` | 2881 | | `event_loop_getter!(self, CursorPosition)`. |
| `fn set_theme(&self, theme: Option<Theme>)` | 2885 | | sends `EventLoopMessage::SetTheme`; errors swallowed. |
| `fn show(&self) -> Result<()>` | 2891 | macOS | `EventLoopMessage::ShowApplication`. |
| `fn hide(&self) -> Result<()>` | 2897 | macOS | `EventLoopMessage::HideApplication`. |
| `fn set_device_event_filter(&self, filter: DeviceEventFilter)` | 2902 | | sends `EventLoopMessage::SetDeviceEventFilter`; errors swallowed. |
| `fn fetch_data_store_identifiers<F: FnOnce(Vec<[u8; 16]>) + Send + 'static>(&self, cb: F) -> Result<()>` | 2908 | macOS/iOS | stub: calls back with an empty `Vec`. |
| `fn remove_data_store<F: FnOnce(Result<()>) + Send + 'static>(&self, _uuid: [u8; 16], cb: F) -> Result<()>` | 2917 | macOS/iOS | stub: calls back with `Ok(())`. |

`find_class` and `run_on_android_context` are **not** overridden — they keep the trait's default bodies.

### 4.5 `impl<T: UserEvent> Runtime<T> for CefRuntime<T>` — line 3570

Associated types (3571–3578):

| member | value |
| --- | --- |
| `type WindowDispatcher` | `CefWindowDispatcher<T>` |
| `type WebviewDispatcher` | `CefWebviewDispatcher<T>` |
| `type Handle` | `CefRuntimeHandle<T>` |
| `type EventLoopProxy` | `EventProxy<T>` |
| `type RuntimeWebviewAttributes` | `CefWebviewAttributes` |
| `type Webview` | `Webview` |
| `type RuntimeInitAttrs` | `Cef` |
| `type WindowOpener` | `NewWindowOpener` |

Members:

| member | line | cfg | what it does |
| --- | --- | --- | --- |
| `fn new(args: RuntimeInitArgs<Self::RuntimeInitAttrs>) -> Result<Self>` | 3580 | | `Self::init(EventLoopBuilder::default(), args)`. |
| `fn new_any_thread(args: RuntimeInitArgs<Self::RuntimeInitAttrs>) -> Result<Self>` | 3592 | not macOS | same, with `event_loop_builder.with_any_thread(true)`. |
| `fn create_proxy(&self) -> Self::EventLoopProxy` | 3598 | | clones the context into `EventProxy`. |
| `fn handle(&self) -> Self::Handle` | 3604 | | clones the context into `CefRuntimeHandle`. |
| `fn create_window<F: Fn(RawWindow<'_>) + Send + 'static>(&self, pending: PendingWindow<T, Self>, after_window_creation: Option<F>) -> Result<DetachedWindow<T, Self>>` | 3610 | | `create_window_detached`. |
| `fn create_webview(&self, window_id: WindowId, pending: PendingWebview<T, Self>) -> Result<DetachedWebview<T, Self>>` | 3618 | | `create_webview_detached`. |
| `fn primary_monitor(&self) -> Option<Monitor>` | 3626 | | event-loop getter, errors flattened to `None`. |
| `fn monitor_from_point(&self, x: f64, y: f64) -> Option<Monitor>` | 3633 | | `MonitorFromPoint`, errors flattened to `None`. |
| `fn available_monitors(&self) -> Vec<Monitor>` | 3646 | | event-loop getter, errors flattened to empty. |
| `fn cursor_position(&self) -> Result<PhysicalPosition<f64>>` | 3653 | | event-loop getter. |
| `fn set_theme(&self, theme: Option<Theme>)` | 3657 | | sends `EventLoopMessage::SetTheme`. |
| `fn set_activation_policy(&mut self, activation_policy: tauri_runtime::ActivationPolicy)` | 3663 | macOS | sends `EventLoopMessage::SetActivationPolicy`. |
| `fn set_activate_ignoring_other_apps(&mut self, ignore: bool)` | 3669 | macOS | **not supported**: warns when `ignore == false`, does nothing else. |
| `fn set_dock_visibility(&mut self, visible: bool)` | 3681 | macOS | sends `EventLoopMessage::SetDockVisibility`. |
| `fn show(&self)` | 3687 | macOS | `EventLoopMessage::ShowApplication`. |
| `fn hide(&self)` | 3693 | macOS | `EventLoopMessage::HideApplication`. |
| `fn set_device_event_filter(&mut self, filter: DeviceEventFilter)` | 3698 | | applies directly on the owned `event_loop`. |
| `fn run_iteration<F: FnMut(RunEvent<T>) + 'static>(&mut self, mut callback: F)` | 3704 | | drains only `Message::UserEvent`, pumps CEF once, then `RunEvent::MainEventsCleared`. |
| `fn run_return<F: FnMut(RunEvent<T>) + 'static>(self, callback: F) -> i32` | 3714 | | calls `run`, always returns `0` (`TODO`: real exit code). |
| `fn run<F: FnMut(RunEvent<T>) + 'static>(self, callback: F)` | 3720 | | builds `WinitCefApp`, `event_loop.run_app(app)`, then `cef::shutdown()`. |

---

## 5. `WindowBuilderWrapper`

`pub struct WindowBuilderWrapper { pub(crate) attrs: AppWindowAttrs }` (line 41), `#[derive(Clone, Default, Debug)]`, `unsafe impl Send` (45), `impl WindowBuilderBase` (47, empty), `impl WindowBuilder` (49).

`WindowBuilder::new()` (50) sets title `"Tauri App"`, `visible(true)`, `focused(true)`, and on Windows `window_classname("Tauri Window")` plus `with_clip_children(false)` — the last is required because a per-pixel-alpha child webview would otherwise composite against black.

| line | method | forwards to |
| --- | --- | --- |
| 82 | `fn with_config(config: &WindowConfig) -> Self` | chains the setters below from a `WindowConfig`; `always_on_bottom` wins over `always_on_top`; builds `WindowSizeConstraints` from `min_*`/`max_*`; macOS-only `hidden_title`, `title_bar_style`, `traffic_light_position`, `tabbing_identifier`; `window_classname` and `prevent_overflow` from config. |
| 158 | `fn center(mut self) -> Self` | `attrs.center = true` |
| 163 | `fn position(mut self, x: f64, y: f64) -> Self` | `WindowAttributes::with_position(LogicalPosition)` |
| 168 | `fn inner_size(mut self, width: f64, height: f64) -> Self` | `with_surface_size(LogicalSize)` |
| 176 | `fn min_inner_size(mut self, min_width: f64, min_height: f64) -> Self` | `with_min_surface_size(LogicalSize)` |
| 184 | `fn max_inner_size(mut self, max_width: f64, max_height: f64) -> Self` | `with_max_surface_size(LogicalSize)` |
| 192 | `fn inner_size_constraints(mut self, constraints: WindowSizeConstraints) -> Self` | `min_size_constraint` / `max_size_constraint` into `min_surface_size` / `max_surface_size` |
| 200 | `fn prevent_overflow(mut self) -> Self` | `attrs.prevent_overflow = Some(PhysicalSize::new(0,0).into())` |
| 205 | `fn prevent_overflow_with_margin(mut self, margin: Size) -> Self` | `attrs.prevent_overflow = Some(margin)` |
| 210 | `fn resizable(mut self, resizable: bool) -> Self` | `with_resizable` |
| 215 | `fn maximizable(mut self, maximizable: bool) -> Self` | `enabled_buttons.set(WindowButtons::MAXIMIZE, …)` |
| 224 | `fn minimizable(mut self, minimizable: bool) -> Self` | `enabled_buttons.set(WindowButtons::MINIMIZE, …)` |
| 233 | `fn closable(mut self, closable: bool) -> Self` | `enabled_buttons.set(WindowButtons::CLOSE, …)` |
| 242 | `fn title<S: Into<String>>(mut self, title: S) -> Self` | `with_title` |
| 247 | `fn fullscreen(mut self, fullscreen: bool) -> Self` | `with_fullscreen(bool.then_some(Fullscreen::Borderless(None)))` |
| 255 | `fn focused(mut self, focused: bool) -> Self` | `with_active` |
| 260 | `fn focusable(self, _focusable: bool) -> Self` | **no-op** (`// TODO`) |
| 265 | `fn maximized(mut self, maximized: bool) -> Self` | `with_maximized` |
| 270 | `fn visible(mut self, visible: bool) -> Self` | `with_visible` |
| 275 | `fn transparent(mut self, transparent: bool) -> Self` | `with_transparent` |
| 280 | `fn decorations(mut self, decorations: bool) -> Self` | `with_decorations` |
| 285 | `fn always_on_bottom(mut self, always_on_bottom: bool) -> Self` | `with_window_level(AlwaysOnBottom)` when true, `with_window_level(Normal)` when false |
| 294 | `fn always_on_top(mut self, always_on_top: bool) -> Self` | `with_window_level(AlwaysOnTop)` when true, `with_window_level(Normal)` when false |
| 304 | `fn visible_on_all_workspaces(mut self, visible_on_all_workspaces: bool) -> Self` | `attrs.visible_on_all_workspaces` on macOS/Linux/BSD only; no-op on Windows |
| 320 | `fn content_protected(mut self, protected: bool) -> Self` | `with_content_protected` |
| 325 | `fn icon(mut self, icon: Icon) -> Result<Self>` | `tauri_icon_to_winit_icon` → `with_window_icon(Some(..))` |
| 332 | `fn skip_taskbar(mut self, skip: bool) -> Self` | Windows: `with_skip_taskbar`; Linux/BSD: `attrs.skip_taskbar`; macOS: no-op |
| 359 | `fn background_color(mut self, color: Color) -> Self` | `attrs.background_color = Some(color)` |
| 368 | `fn shadow(mut self, enable: bool) -> Self` | Windows `with_undecorated_shadow`; macOS `with_has_shadow` |
| 391 | `fn owner(mut self, owner: HWND) -> Self` (windows) | `with_owner_window(owner.0)` |
| 401 | `fn parent(mut self, parent: HWND) -> Self` (windows) | `with_parent_window(RawWindowHandle::Win32(..))` |
| 411 | `fn drag_and_drop(mut self, enabled: bool) -> Self` (windows) | `with_drag_and_drop` |
| 421 | `fn parent(mut self, parent: *mut std::ffi::c_void) -> Self` (macos) | `NSWindow` → `contentView` → `AppKitWindowHandle` → `with_parent_window` |
| 454 | `fn transient_for(mut self, parent: *mut std::ffi::c_void) -> Self` (linux/bsd) | `attrs.transient_for = Some(gtk::Window::from_glib_full(..))` |
| 465 | `fn title_bar_style(mut self, style: TitleBarStyle) -> Self` (macos) | `Visible` → transparent=false/fullsize=false; `Transparent` → true/false; `Overlay` → true/true |
| 487 | `fn traffic_light_position<P: Into<Position>>(mut self, position: P) -> Self` (macos) | `attrs.traffic_light_position = Some(..)` |
| 493 | `fn hidden_title(mut self, hidden: bool) -> Self` (macos) | `with_title_hidden` |
| 503 | `fn tabbing_identifier(mut self, identifier: &str) -> Self` (macos) | `with_tabbing_identifier` |
| 512 | `fn theme(mut self, theme: Option<Theme>) -> Self` | `with_theme(tauri_theme_to_winit_theme(..))` |
| 521 | `fn window_classname<S: Into<String>>(mut self, _window_classname: S) -> Self` | Windows: `with_class_name`; elsewhere no-op |
| 536 | `fn no_redirection_bitmap(#[allow(unused_mut)] mut self, _enable: bool) -> Self` | **no-op** (`// TODO`) |
| 540 | `fn has_icon(&self) -> bool` | `attrs.inner.window_icon.is_some()` |
| 544 | `fn get_theme(&self) -> Option<Theme>` | `attrs.inner.preferred_theme.map(winit_theme_to_tauri_theme)` |

Helper: `fn platform_attrs(attrs: &mut WindowAttributes) -> Box<PlatformAttributes>` (559) — takes the platform attribute block out of `attrs` (defaulting when absent) so a `with_*` can be applied and put back.

---

## 6. Public exports from `lib.rs`

Exact list, in file order:

| line | export | origin module |
| --- | --- | --- |
| 18 | `pub use dialog::{NativeDialogKind, NativeDialogObservation, NativeDialogSnapshot, NativeDialogToken};` | `crate::dialog` |
| 21 | `pub use environment::DebugEnvironment;` | `crate::environment` |
| 27 | `pub use offscreen::{CursorHandler, Offscreen, OffscreenView};` | `crate::offscreen` |
| 38 | `pub use devtools::{DevToolsMessageIdExhausted, allocate_devtools_message_id};` | `crate::devtools` |
| 39 | `pub use frame::{FrameEvent, FrameEventHandler, FrameEventKind};` | `crate::frame` |
| 40 | `pub use frame_navigation::{FrameNavigationState, NativeDocumentToken};` | `crate::frame_navigation` |
| 45 | `pub use cef::sys::CEF_API_VERSION_LAST;` | external crate `cef` |
| 46–47 | `#[cfg(target_os = "macos")] pub use platform::macos::setup_application as prepare_macos_application;` | `crate::platform::macos` |
| 48 | `pub use runtime::*;` | `crate::runtime` |
| 49 | `pub use tauri_ext::*;` | `crate::tauri_ext` |
| 51 | `pub use tauri_macros::cef_entry_point;` | external crate `tauri_macros` |
| 52 | `pub use webview::*;` | `crate::webview` |
| 53 | `pub use window::{CefWindowDispatcher, NativeWindowToken};` | `crate::window` |
| 54 | `pub use window_builder::WindowBuilderWrapper;` | `crate::window_builder` |

**What `pub use runtime::*;` (line 48) actually exports** — the complete set of `pub` items in `runtime.rs`:

`NativeWindowEventHandler` (75), `cef` (89), `SecretStorage` (132), `SandboxPolicy` (164), `RemoteDebugging` (199), `DevToolsPolicy` (258), `CertificateErrorPolicy` (275), `ProxyConfig` (298), `AutoplayPolicy` (357), `WebRtcIpHandling` (392), `DowngradePolicy` (442), `Cef` (472), `NewWindowOpener` (1181), `EventProxy` (1211), `run_cef_helper_process` (2741), `CefRuntimeHandle` (2787), `CefRuntime` (2927).

Everything else in `runtime.rs` is `pub(crate)` or private and is **not** exported: `SettingsCallback`, `RuntimeContext`, `MainThreadDispatch*`, `dispatches_run_event`, `handle_main_thread_message`, `AfterWindowCreationCallback`, `Message`, `EventLoopMessage`, `WinitDragDropState`, the three `pending_native_*`/`request_native_drag_paths` helpers, `event_loop_getter!`, `find_monitor_from_point`, `is_cef_helper_process`, `AppState`, `WinitCefApp`, `deep_link_arguments`, `append_command_line_args`, `TauriCefApp`, `API_VERSION_SWITCH`, `child_process_api_version`, `TauriCefHelperApp`, `TerminationSignals`, and the test modules.

---

## 7. Electron feasibility notes

Mechanisms are named against what the bridge already exposes (`electron/main.cjs` command table: `window.*`, `webview.*`, `app.*`, `protocol.*`) or against a standard Electron API that would need a new RPC method (marked **new RPC**). `electron/main.cjs` line citations refer to that file as of commit `22aa5cd`.

### 7.1 Builder methods

| # | item | verdict | Electron / electron-rs mechanism |
| --- | --- | --- | --- |
| 1 | `on_window_event` | emulated | No native event stream reaches Rust. Forward `BrowserWindow` events (`resize`, `move`, `focus`, `blur`, `close`, `closed`) and `webContents` events as RPC notifications and re-emit them. **The signature cannot be kept**: there is no winit `ActiveEventLoop` or `winit::window::Window` in an Electron runtime. Keep the name, re-type the payload (label + a window handle + a runtime-defined event enum). |
| 2 | `with_settings` | absent (as written) / emulated (retargeted) | `cef::Settings` does not exist. The capability maps onto a launch-config struct consumed before `Electron::with_args`, but the exact API is gone. |
| 3–4 | `command_line_arg(s)` | direct | Electron is Chromium: `app.commandLine.appendSwitch(key, value)` in the main process, or pass the switches as launch arguments (the target already has `extra_args`). Must be applied before `app.whenReady()`. |
| 5 | `deep_link_schemes` | direct | `app.requestSingleInstanceLock()` + `app.on('second-instance', (event, argv, cwd))`; registration with the OS via `app.setAsDefaultProtocolClient(scheme)`. |
| 6 | `root_cache_path` | direct | `app.setPath('userData', path)` before `app.whenReady()` (the bridge already does this at `main.cjs:112`). |
| 7 | `downgrade` | emulated | Pure runtime-owned filesystem logic — no Electron API involved. Electron does not write a `Last Version` breadcrumb, so the runtime must write and read it itself in `userData`. |
| 8 | `cef_api_version` | absent | CEF ABI versioning has no Electron analogue. Electron's own version comes from the handshake (`Ipc.version`, `process.versions.electron`). |
| 9 | `secret_storage` | direct | Same Chromium switches: `--use-mock-keychain` (macOS), `--password-store=basic` (Linux); Windows keeps DPAPI. Append in the main process before ready. |
| 10–11 | `profile_preference`, `profile_preference_value` | absent | Electron exposes no Chromium `PrefService`/`PreferenceManager`. Individual preferences must be re-expressed as switches or session APIs; there is no general escape hatch. |
| 12 | `global_preference` | absent | Same: no local-state preference API in Electron. |
| 13 | `default_content_setting` | emulated | `session.setPermissionRequestHandler` (already used at `main.cjs:299`) + `session.setPermissionCheckHandler` (**new RPC**) answer per-origin permission requests; a stored *default* per content type has to be enforced by the runtime. |
| 14 | `allow_chromium_command_line_args` | emulated | Electron never clears its own command line. The main process must filter `process.argv` itself before acting on it — the runtime owns `main.cjs`, so this is achievable, but it is runtime policy, not an Electron feature. |
| 15 | `log_file` | emulated | `app.commandLine.appendSwitch('enable-logging', 'file')` + `--log-file=<path>`; Chromium's own log, not CEF's. |
| 16 | `log_severity` | emulated | `--log-level=<n>` / `--v=<n>` switches. There is no `LogSeverity` enum; the mapping must be defined by the runtime. |
| 17 | `locale` | direct | `app.commandLine.appendSwitch('lang', locale)`. |
| 18 | `accept_language_list` | direct | `session.setUserAgent(userAgent, acceptLanguages)` (**new RPC**) or `--accept-lang=` switch. |
| 19 | `sandbox` | emulated | `webPreferences.sandbox` (already `true` at `main.cjs:434`) plus the `--no-sandbox` switch. Electron has no broker-based policy object; `Required` can only be honoured by refusing to start when `--no-sandbox` is in effect. |
| 20–21 | `disable_features`, `enable_features` | direct | `app.commandLine.appendSwitch('disable-features', …)` / `'enable-features'`. The *merging* semantics must be re-implemented (Chromium replaces a switch value rather than extending it). |
| 22 | `debug_environment` | emulated | `SSLKEYLOGFILE` is refused with an empty `--ssl-key-log-file`; Electron has no CEF crash-reporter variables, but the same `std::env::remove_var` policy can be applied to `ELECTRON_ENABLE_LOGGING`/`BREAKPAD_DUMP_LOCATION` before launching the child. |
| 23 | `remote_debugging` | direct | `--remote-debugging-port=<port>` / `--remote-debugging-pipe` / `--remote-allow-origins=` (the bridge already appends `remote-debugging-port` at `main.cjs:98`). The `devtools.remote_debugging.allowed` preference pin is **absent**. |
| 24 | `devtools` | direct | `webPreferences.devTools` (already wired at `main.cjs:444`) plus `webview.openDevtools` / `webview.closeDevtools` / `webview.isDevtoolsOpen` RPCs. |
| 25 | `certificate_errors` | direct | `app.on('certificate-error', (event, webContents, url, error, certificate, callback))` — `event.preventDefault(); callback(false)` for `Cancel`, `callback(true)` for the interstitial path; or `session.setCertificateVerifyProc`. **new RPC** (event must reach Rust or be decided in the main process). |
| 26 | `proxy` | direct | `session.setProxy({ mode, proxyRules, proxyBypassRules, pacScript })` (**new RPC**). Note the bridge's own comment: Electron proxies are per session, not per window. |
| 27 | `autoplay` | direct | `--autoplay-policy=<value>` with the identical hyphenated values. |
| 28 | `webrtc_ip_handling` | direct | `--webrtc-ip-handling-policy=<value>` with the identical underscored values. |
| 29 | `spell_checking` | direct | `session.setSpellCheckerEnabled(bool)` (**new RPC**). |
| 30 | `safe_browsing` | absent | Electron ships without Google Safe Browsing; there is no service to enable or disable. |
| 31 | `component_updates` | emulated | `--disable-component-update` switch; Electron does not run the component updater by default, so this is mostly a no-op that keeps the API honest. |
| 32 | `user_agent` | direct | `webPreferences.userAgent` (already wired at `main.cjs:442`) or `session.setUserAgent`. |
| 33 | `user_agent_product` | absent | Electron can only replace the whole UA string; there is no product-token splice. Emulating it means string surgery on Chromium's UA, which would drift with every Electron upgrade. |
| 34 | `persist_session_cookies` | absent | No Electron/Chromium API exposes session-cookie persistence. |
| 35 | `javascript_flags` | direct | `app.commandLine.appendSwitch('js-flags', flags)`. |
| 36 | `chrome_policy_id` | absent | Electron does not read Chrome enterprise policy. |
| 37 | `log_items` | absent | CEF-specific log-line prefixing. |

### 7.2 Public enums

| item | verdict | mechanism |
| --- | --- | --- |
| `SecretStorage` | direct | same switches as #9 above. |
| `SandboxPolicy` | emulated | `webPreferences.sandbox` + `--no-sandbox`; `Required` enforced by the runtime refusing to start. |
| `RemoteDebugging` | direct | `--remote-debugging-port` / `--remote-debugging-pipe` / `--remote-allow-origins`. |
| `DevToolsPolicy` | direct | `webPreferences.devTools` + `webview.openDevtools` RPC; the `Auto` build-profile rule is pure runtime logic. |
| `CertificateErrorPolicy` | direct | `app.on('certificate-error')`. |
| `ProxyConfig` | direct | `session.setProxy`; the JSON shape in §3.6 is Chromium's preference, which Electron's `setProxy` options mirror (`mode`, `proxyRules`, `proxyBypassRules`, `pacScript`). |
| `AutoplayPolicy` | direct | `--autoplay-policy`. |
| `WebRtcIpHandling` | direct | `--webrtc-ip-handling-policy`. |
| `DowngradePolicy` | emulated | runtime-owned filesystem logic over `userData`. |

### 7.3 Trait members

| item | verdict | mechanism |
| --- | --- | --- |
| `RuntimeInitAttrs::apply_config` | direct | pure config parsing (`tauri_utils::config`), no runtime dependency. |
| `From<Cef> for DynRuntimeInitAttrs` | direct | pure Rust. |
| `EventLoopProxy::send_event` | direct | the runtime's own channel; no Electron API. |
| `RuntimeHandle::create_proxy` / `handle` | direct | pure Rust. |
| `RuntimeHandle::set_activation_policy` (macOS) | direct | `app.setActivationPolicy` — RPC already exists (`app.activationPolicy`). |
| `RuntimeHandle::set_dock_visibility` (macOS) | direct | `app.dock.hide()/show()` — RPC `app.dockVisibility`. |
| `RuntimeHandle::request_exit` | direct | `app.quit()` — the bridge already calls it (`main.cjs:1767`). |
| `RuntimeHandle::custom_scheme_url` | emulated | Electron custom schemes are `scheme://host/path` after `protocol.registerSchemesAsPrivileged` (already used at `main.cjs:124`), **not** CEF's `http://<scheme>.localhost`. The URL form must change, so anything matching on it changes too. |
| `RuntimeHandle::webview_version` | direct | the handshake already reports the Electron version (`Ipc.version`); `process.versions.chrome` for the Chromium version. |
| `RuntimeHandle::create_window` / `create_webview` | direct | `window.create` / `webview.create` RPCs. |
| `RuntimeHandle::run_on_main_thread` | direct | the runtime's own message pump; no Electron API. |
| `RuntimeHandle::display_handle` | emulated | `window.nativeHandle` RPC returns the native handle; the runtime reconstructs a `RawWindowHandle` (`Win32`/`AppKit`/`Xlib`) from it. |
| `RuntimeHandle::primary_monitor` / `available_monitors` / `monitor_from_point` | direct | `screen.getPrimaryDisplay()` / `screen.getAllDisplays()` / `screen.getDisplayNearestPoint()` — RPCs `window.displays`, `window.currentDisplay`, `window.displayForPoint` exist. |
| `RuntimeHandle::cursor_position` | direct | `screen.getCursorScreenPoint()` — RPC `window.cursorPosition`. |
| `RuntimeHandle::set_theme` | direct | `nativeTheme.themeSource` — RPC `app.theme`; `nativeTheme.on('updated')` already drives theme-change events. |
| `RuntimeHandle::show` / `hide` (macOS) | direct | `app.show()` / `app.hide()` — RPCs `app.show`, `app.hide`. |
| `RuntimeHandle::set_device_event_filter` | absent | Electron exposes no device-event stream. (The target crate's `docs/coverage.md` already records this.) |
| `RuntimeHandle::fetch_data_store_identifiers` | absent | Electron partitions are string-keyed (`main.cjs:342`), not UUID-keyed. The reference already stubs this to an empty `Vec`. |
| `RuntimeHandle::remove_data_store` | emulated | `session.fromPartition(key).clearStorageData()` (**new RPC**) — but only for partitions the runtime itself created; a UUID cannot be resolved. |
| `Runtime::new` / `new_any_thread` | direct | no winit loop, so `new_any_thread` is trivially satisfiable. |
| `Runtime::create_window` / `create_webview` | direct | `window.create` / `webview.create` RPCs. |
| `Runtime::primary_monitor` / `monitor_from_point` / `available_monitors` / `cursor_position` | direct | as in the `RuntimeHandle` rows above. |
| `Runtime::set_theme` | direct | `nativeTheme.themeSource`. |
| `Runtime::set_activation_policy` (macOS) | direct | `app.setActivationPolicy`. |
| `Runtime::set_activate_ignoring_other_apps` (macOS) | absent | The reference itself does not implement it (warns only). Keep the same behaviour. |
| `Runtime::set_dock_visibility` / `show` / `hide` (macOS) | direct | `app.dock`, `app.show`, `app.hide`. |
| `Runtime::set_device_event_filter` | absent | as above. |
| `Runtime::run_iteration` / `run_return` / `run` | direct | the runtime's own loop over the RPC event stream; `run_return` returning `0` is the reference's own TODO. |

### 7.4 Module-level items with no Electron counterpart

| item | verdict | note |
| --- | --- | --- |
| `pub use cef;` (89) | absent | No `cef` crate. The target's equivalent is the `electron-rs` re-export, if any — decide explicitly; a glob re-export of `electron_rs` would put its whole surface in this crate's public API. |
| `cef_api_version` + `API_VERSION_SWITCH` + `child_process_api_version` (655, 2712, 2720) | absent | Electron manages its own helper processes; nothing to declare or forward. |
| `run_cef_helper_process` + `TauriCefHelperApp` (2741, 2776) | absent | Electron's helper processes are its own binaries, not the application re-entered. |
| `is_cef_helper_process` (1713) | absent | macOS CEF helper detection; Electron names its helpers differently and the Rust process is never one. |
| `TerminationSignals` (2958) | absent | Exists only to undo CEF's SIGINT/SIGTERM/SIGHUP handlers. Electron installs none in the Rust process. |
| `MainThreadDispatchSlot::deliver_wake` / `MainThreadWake` (1295) | absent | macOS AppKit nested-loop workaround for winit's proxy; no winit, no nested loop. |
| `SoftbufferWindowHandle` + `HasWindowHandle`/`HasDisplayHandle` impls (`window_handle.rs:16–49`) | absent | Windows software-rendered background surface for the CEF child HWND; Electron draws its own background. |
| `SendRawWindowHandle` (`window_handle.rs:10`) | direct | Still needed if `display_handle` is implemented; keep the type. |
| `SendRawDisplayHandle` (`window_handle.rs:13`) | direct | Same; the reference's `EventLoopMessage::DisplayHandle` round-trip has no Electron equivalent, but the wrapper type itself is just a `Send` newtype. |
| `WinitDragDropState` and the `pending_native_drag_*` helpers (1607–1679) | emulated | Electron delivers drag-and-drop through the renderer's DOM `drop` events and `webContents`; the runtime can keep the same state machine fed by RPC notifications instead of winit events. |
| `event_loop_getter!` (1681) | direct | A plain channel round-trip; keep as-is. |
| `find_monitor_from_point` (1694) | direct | `screen.getDisplayNearestPoint`. |
| `deep_link_arguments` (2559) | direct | Pure URL filtering; keep verbatim (rename the CEF references in its doc comment). |
| `append_command_line_args` (2578) | emulated | `CommandLine` is a CEF type; the same switch/argument classification must be re-expressed as an `argv` vector for `Electron::with_args`. |
| `device_event_filter_to_winit` (1581) | absent | winit-only. Note the inverted mapping is intentional in the reference — do not "fix" it if the function is kept. |
| `WinitCefApp` (1747) + `ApplicationHandler` impl (2302) | absent | The whole winit application handler. The Electron runtime's loop is driven by the RPC event stream instead; the message-dispatch *semantics* (`close_window`, `request_window_close`, `exit_if_done`, `live_browsers` accounting) must be preserved, the winit scaffolding must not. |
| `WindowBuilderWrapper` (§5) | emulated | `window.create` accepts title/size/position/resizable/maximizable/minimizable/closable/fullscreen/alwaysOnTop/skipTaskbar/focusable/contentProtected/center/shadow/transparent/decorations/backgroundColor/icon/parent/visibleOnAllWorkspaces/theme. Parity note: the bridge *does* have a `window.focusable` RPC, but the reference's `focusable` builder method is a no-op (`// TODO`, line 260) and `no_redirection_bitmap` (536) is one too — a 1:1 mirror keeps both as no-ops. `owner`/`parent(HWND)`/`drag_and_drop`/`transient_for`/`title_bar_style`/`traffic_light_position`/`hidden_title`/`tabbing_identifier` have no Electron equivalent in the current bridge and are recorded as such in `docs/coverage.md`. |

---

## 8. Implementation checklist

1. `src/runtime.rs` must contain, in this order: the nine public enums with their private helpers, `Electron` + `impl fmt::Debug`, the 37 `#[must_use]` builder methods, `RuntimeInitAttrs` + `From<…> for DynRuntimeInitAttrs`, `NewWindowOpener`, `EventProxy` + `EventLoopProxy`, the context/dispatch machinery, the message enums, the app handler, the runtime handle + `RuntimeHandle`, the runtime + `Runtime<T>`, and the three test modules with the same 20 assertions.
2. `src/window_builder.rs` must export `WindowBuilderWrapper` with all 44 `WindowBuilder` methods, including the two documented no-ops (`focusable`, `no_redirection_bitmap`).
3. `src/window_handle.rs` must contain only the raw-handle newtypes; `ElectronRuntimeHandle` belongs in `runtime.rs` (the target crate currently has this backwards).
4. `src/lib.rs` must expose the same `pub use` list, with `cef`-specific entries replaced by their documented Electron decisions.
5. Every `absent` verdict in §7 needs an explicit, documented decision in the target — either a `unsupported`-style report (matching the target's existing convention) or removal from the public surface. Silence is not parity.
