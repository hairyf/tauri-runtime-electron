# Parity spec — `webview.rs`, `tauri_ext.rs`, `frame.rs`, `frame_navigation.rs`

Implementation contract for porting the reference CEF runtime's webview surface to the
Electron-backed `tauri-runtime-electron` crate.

| | |
| --- | --- |
| Reference crate | `source/tauri/crates/tauri-runtime-cef/` — `tauri-runtime-cef 3.0.0-alpha.7` |
| Reference dependency | `source/tauri` workspace: `tauri-runtime ~3.0.0-alpha.5`, `tauri-utils ~3.0.0-alpha.4`. That `WebviewDispatch` has **no** `on_webview_event` member — see Appendix B.7 |
| Target crate | `src/` — `tauri-runtime-electron` (branch `next`), Tauri v3, Electron via the `electron-rs` JSON-RPC bridge. `Cargo.toml` pins `tauri-runtime 3.0.0-alpha.5` (aligned with the reference) and `electron-rs` from git |
| Files in scope | `webview.rs` (2500 lines by the reader's count; the brief says 2501 — trailing-line off-by-one, contents read in full), `tauri_ext.rs` (524 lines), `frame.rs` (79 lines), `frame_navigation.rs` (462 lines) |
| Method | all four files read in full, line by line; every declaration below is copied verbatim from the source with bodies collapsed |
| Out of scope (referenced only) | `runtime.rs`, `window.rs`, `popup.rs`, `offscreen.rs`, `dialog.rs`, `devtools.rs`, `cef_impl/*`, `platform/*` |

The goal is a **1:1 mirror**: same file layout, same public API shape, same API support
level. Where the reference has no Electron equivalent the item must still exist with the
same signature and report unsupported rather than disappear.

---

## 0. Naming and layout contract

### 0.1 File layout

The target must gain the two files it is missing. `src/webview.rs` and `src/tauri_ext.rs`
already exist but are a much smaller, older surface and are to be rewritten from this spec.

| reference file | target file | status |
| --- | --- | --- |
| `src/webview.rs` | `src/webview.rs` | exists, rewrite to this spec |
| `src/tauri_ext.rs` | `src/tauri_ext.rs` | exists as a stub, rewrite to this spec |
| `src/frame.rs` | `src/frame.rs` | add (or port) |
| `src/frame_navigation.rs` | `src/frame_navigation.rs` | add (or port) |

`lib.rs` must re-export exactly as the reference does (`lib.rs:39-40, 49, 52`):

```rust
pub use frame::{FrameEvent, FrameEventHandler, FrameEventKind};
pub use frame_navigation::{FrameNavigationState, NativeDocumentToken};
pub use tauri_ext::*;
pub use webview::*;
```

### 0.2 Rename map (`Cef*` → `Electron*`)

| reference name | target name | where |
| --- | --- | --- |
| `CefWebviewDispatcher` | `ElectronWebviewDispatcher` | `webview.rs:1613` |
| `CefWebviewAttributes` | `ElectronWebviewAttributes` | `webview.rs:1498` |
| `CefInitScript` | `ElectronInitScript` | `webview.rs:1553` |
| `AsCefWebviewDispatcher` | `AsElectronWebviewDispatcher` | `tauri_ext.rs:31` |
| `AsCefWebviewAttributes` | `AsElectronWebviewAttributes` | `tauri_ext.rs:88` |
| `AsCefWindowOpener` | `AsElectronWindowOpener` | `tauri_ext.rs:70` |
| `WebviewBuilderCefExt` | `WebviewBuilderElectronExt` | `tauri_ext.rs:414` |
| `WebviewCefExt` | `WebviewElectronExt` | `tauri_ext.rs:119` |
| `WebviewWindowBuilderCefExt` | `WebviewWindowBuilderElectronExt` | `tauri_ext.rs:268` |
| `as_cef_webview_dispatcher` | `as_electron_webview_dispatcher` | `tauri_ext.rs:33` |
| `as_cef_webview_attributes_mut` | `as_electron_webview_attributes_mut` | `tauri_ext.rs:90` |
| `as_cef_window_opener` | `as_electron_window_opener` | `tauri_ext.rs:72` |
| `with_cef_webview_attributes` | `with_electron_webview_attributes` | `tauri_ext.rs:106` |
| `with_cef_webview` | `with_electron_webview` | `tauri_ext.rs:207` |
| `not_cef` | `not_electron` | `tauri_ext.rs:23` |
| `TauriCefBrowserClient*`, `WinitCefApp`, `CefRuntime`, `Cef` | `TauriElectron*`, `ElectronApp`, `ElectronRuntime`, `Electron` | outside scope, same rule |

### 0.3 Names that must **not** change

`Webview`, `WebviewSnapshot`, `ConsoleMessage`, `ConsoleMessageLevel`, `ConsoleMessageHandler`,
`ChromeCommandGroup`, `DevToolsProtocol`, `DevToolsProtocolHandler`, `FrameEvent`,
`FrameEventKind`, `FrameEventHandler`, `FrameNavigationState`, `NativeDocumentToken`,
`RuntimeStyle`, `BrowserSettingsCallback`, `NewWindowOpener`, `PageFullscreen`, `BoundsRate`,
`AppWebview`, `WebviewMessage`, `PendingInitialLoads`, `INITIAL_LOAD_URL`,
`layout_app_window`, `compute_child_bounds_rate`, `initialization_scripts`,
`allocate_devtools_message_id`, `DevToolsMessageIdExhausted`, `NativeWindowToken`,
`NativeDialogObservation`, `Offscreen`, `OffscreenView`, `CursorHandler`.

`RuntimeStyle` keeps its name and its `Alloy`/`Chrome` variants even though Electron has no
runtime style (§6.6): the type is public API, and the builder method that takes it must keep
its signature.

### 0.4 CEF vocabulary that has to be re-typed, not renamed

These appear in signatures inside the four files and must be replaced by Electron-shaped
types, not merely renamed:

| CEF type in a signature | Electron replacement |
| --- | --- |
| `cef::Browser` (returned by `Webview::browser`) | opaque `ElectronWebviewHandle` wrapping `webContents.id` (§6.1) |
| `cef::BrowserSettings` (in `BrowserSettingsCallback`) | `ElectronBrowserSettings` — a plain Rust settings struct (§6.6) |
| `cef::Registration` | an `ElectronDebuggerRegistration` guard that detaches the CDP session on drop |
| `cef::LogSeverity`, `CefString` (in `ConsoleMessage::from_cef`) | main-process `console-message` payload types |
| `cef::Browser`, `cef::Frame` (in `emit_frame_event`) | the frame-event payload produced by the Electron side |
| `cef::BrowserHost` (in `apply_user_agent_override`) | `webContentsId` + `webContents.setUserAgent` |
| `cef::sys::cef_window_handle_t` (in `build_browser_child`) | the host `BrowserWindow` id |
| `cef::RuntimeStyle` | `RuntimeStyle` (kept, ignored) |

---

## 1. Module inventory

Every module-level item of the four files is listed, in source order, with its verbatim
declaration (bodies collapsed), its line number, and a one-line intent. Where the source has a
doc comment the intent is condensed from it; where it has none (most private helpers) the intent
is an inferred summary of what the body does. Line numbers are 1-based and refer to the reference
files as read.

### 1.1 `src/webview.rs`

#### 1.1.1 Imports (lines 5–33)

| line | declaration | intent |
| --- | --- | --- |
| 5 | `use std::collections::HashMap;` | DevTools pending-load map, popup window map. |
| 6 | `use std::sync::Arc;` | Shared handlers, script lists, registrations. |
| 7–11 | `use std::sync::{Mutex, atomic::{AtomicBool, Ordering}, mpsc::{self, Receiver, Sender}};` | Offscreen visibility flag, blocking getter channels. |
| 13 | `use cef::*;` | CEF types (`Browser`, `BrowserHost`, `CefString`, `Registration`, `wrap_task!`, `wrap_dev_tools_message_observer!`). |
| 14 | `use sha2::{Digest, Sha256};` | CSP hash of each init script. |
| 15–23 | `use tauri_runtime::{Cookie, Error, Result, Runtime, UserEvent, WebviewDispatch, dpi::{PhysicalPosition, PhysicalSize, Position, Rect, Size}, webview::{DetachedWebview, InitializationScript, PendingWebview, UriSchemeProtocolHandler, WebviewAttributes}, window::WindowId};` | Portable runtime types the dispatcher implements against. |
| 24–28 | `use tauri_utils::{Theme, config::{BackgroundThrottlingPolicy, Color}, html::normalize_script_for_csp};` | Theme for the request context, background colour, CSP normalisation. |
| 29 | `use url::Url;` | Navigation and cookie URLs. |
| 31 | `use crate::cef_impl::{client as browser_client, cookie, request_context, request_handler};` | Browser client handlers, cookie visitors, request context creation, scheme registry. |
| 32 | `use crate::runtime::{CefRuntime, Message, RuntimeContext, WinitCefApp};` | Event-loop message plumbing. |
| 33 | `use crate::window::AppWindow;` | Parent window state (label, lifetime token, children, surface size). |

#### 1.1.2 Items

| line | declaration (verbatim, bodies collapsed) | doc-comment intent |
| --- | --- | --- |
| 35–46 | `#[derive(Clone)] pub struct Webview { browser: cef::Browser, snapshot: WebviewSnapshot, frame_navigation_state: crate::FrameNavigationState, popups: Vec<Webview>, opener: Option<crate::FrameNavigationState> }` | Handle to the native CEF browser backing a Tauri webview, exposed through `WebviewDispatch::with_webview`. |
| 48–81 | `#[derive(Clone, Debug)] #[non_exhaustive] pub struct WebviewSnapshot { … }` (8 public fields, §2.2) | Native state sampled on the CEF UI thread immediately before a `with_webview` callback; asserts nothing about renderer responsiveness or occlusion. |
| 83–140 | `impl Webview { … }` (8 members, §1.1.3) | Accessors for the snapshot, the live navigation state, popups/openers and the browser. |
| 84–88 | `pub(crate) fn new(browser: cef::Browser, snapshot: WebviewSnapshot, frame_navigation_state: crate::FrameNavigationState) -> Self` | Constructs a handle with no popups and no opener. |
| 100–102 | `pub fn snapshot(&self) -> &WebviewSnapshot` | Returns the state sampled for this callback; retaining the handle does not refresh it. |
| 106–108 | `pub fn frame_navigation_state(&self) -> &crate::FrameNavigationState` | Read-only **live** navigation state for this browser lifetime, unlike `snapshot`. |
| 110–112 | `pub(crate) fn set_opener(&mut self, opener: crate::FrameNavigationState)` | Marks this handle as a CEF-owned popup of that opener lifetime. |
| 116–118 | `pub fn popups(&self) -> &[Webview]` | CEF-owned popup descendants sampled in the same UI-thread callback; they have no Tauri label. |
| 121–123 | `pub fn opener(&self) -> Option<&crate::FrameNavigationState>` | Exact native opener lifetime, if this is a CEF-owned popup. |
| 127–131 | `pub fn for_document(&self, document: &crate::NativeDocumentToken) -> Option<&Webview>` | Selects the observed document within this runtime-owned browser family; valid only for the current callback. |
| 137–139 | `pub fn browser(&self) -> cef::Browser` | The `cef::Browser` backing this webview, from which the rest of the CEF API is reachable. |
| 142–144 | `pub fn webview_version() -> tauri_runtime::Result<String>` | Embedded Chromium version string. |
| 146–150 | `#[inline] fn color_to_argb(color: Color) -> u32` | Packs `(r,g,b,a)` into `0xAARRGGBB` for `BrowserSettings::background_color`. |
| 152–218 | `fn browser_settings_from_webview_attributes(webview_attributes: &WebviewAttributes) -> cef::BrowserSettings` | Maps the subset of portable attributes CEF's `BrowserSettings` supports; documents the attributes deliberately ignored and why. |
| 220–269 | `fn warn_about_unsupported_attributes(label: &str, attributes: &WebviewAttributes)` | Logs one warning per non-default attribute the CEF runtime cannot honour, naming the runtime-wide alternative. |
| 271–297 | `#[derive(Debug, Clone)] pub enum DevToolsProtocol { … }` (3 variants, §2.6) | A CDP notification observed on a native browser; observers see the whole browser. |
| 299 | `pub(crate) type DevToolsProtocolHandler = dyn Fn(DevToolsProtocol) + Send + Sync;` | Internal handler alias for the per-browser observer fan-out. |
| 301–350 | `#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)] #[non_exhaustive] pub enum ChromeCommandGroup { … }` (5 variants, §2.5) | A family of Chrome commands the runtime swallows in an application window. |
| 352–362 | `impl ChromeCommandGroup { pub(crate) const ALL: &'static [Self] = &[Self::WindowAndTab, Self::Document, Self::BrowserChrome, Self::BrowserSurface, Self::History]; }` | Every group, i.e. what is blocked when a webview allows none; the blocklist is resolved from this. |
| 364–380 | `#[derive(Clone, Debug)] #[non_exhaustive] pub struct ConsoleMessage { … }` (4 public fields, §2.3) | One renderer console message, reported synchronously on the CEF UI thread. |
| 382–400 | `#[derive(Clone, Copy, Debug, PartialEq, Eq)] #[non_exhaustive] pub enum ConsoleMessageLevel { … }` (6 variants, §2.4) | Severity of a `ConsoleMessage`. |
| 402–403 | `pub type ConsoleMessageHandler = dyn Fn(ConsoleMessage) + Send + Sync + 'static;` | Synchronous observer of renderer console output. |
| 405–406 | `pub type BrowserSettingsCallback = dyn Fn(&mut cef::BrowserSettings) + Send + Sync + 'static;` | Last look at a webview's `BrowserSettings` before its browser is created. |
| 408–433 | `impl ConsoleMessage { pub(crate) fn from_cef(level: cef::LogSeverity, message: Option<&CefString>, source: Option<&CefString>, line: i32) -> Self }` | Maps CEF log severity and strings onto the portable struct. |
| 434–473 | `pub(crate) enum WebviewMessage { … }` (35 variants, §1.1.4) | Every operation the event loop performs for a webview dispatcher. |
| 475–483 | `#[derive(Clone, Copy)] pub(crate) struct BoundsRate { pub(crate) x: f32, pub(crate) y: f32, pub(crate) width: f32, pub(crate) height: f32 }` | A webview's bounds as a fraction of its parent window, for auto-resize. |
| 485–494 | `impl Default for BoundsRate` | Full-window default `(0, 0, 1, 1)`. |
| 496–515 | `pub(crate) struct AppWebview { … }` (15 fields, §1.1.5) | The runtime's live per-webview state on the event-loop thread. |
| 517–526 | `#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)] pub(crate) enum PageFullscreen { #[default] Off, On { owns_window: bool } }` | Whether the page shows an element through the HTML Fullscreen API, and whether the window was put in fullscreen for it. |
| 528–586 | `impl AppWebview { … }` | Layout, visibility and URL helpers. |
| 529–543 | `fn resize(&self, scale: f64, x: i32, y: i32, width: i32, height: i32)` | Writes the requested logical rect for offscreen views, else applies physical bounds. |
| 545–568 | `pub(crate) fn set_bounds(&mut self, parent_size: PhysicalSize<u32>, scale: f64, bounds: Rect)` | Updates the bounds rate when one is set, then notifies move/resize start, resizes, notifies end. |
| 570–578 | `pub(crate) fn set_visible(&self, visible: bool)` | Tells CEF it was hidden/shown, then flips the offscreen flag (with redraw) or applies native visibility. |
| 580–585 | `pub fn url(&self) -> Option<String>` | Current main-frame URL, `None` when there is no main frame. |
| 588–1412 | `impl<T: UserEvent> WinitCefApp<T> { … }` | Event-loop side: creation, message routing, and the two message handlers. |
| 589–615 | `pub(crate) fn create_webview(&mut self, window_id: WindowId, webview_id: u32, pending: PendingWebview<T, CefRuntime<T>>) -> Result<()>` | Creates a webview inside an existing window; errors when the window is gone. |
| 620–693 | `pub(crate) fn build_and_attach_webview(context: &RuntimeContext<T>, scheme_registry: &request_handler::SchemeRegistry, live_browsers: &mut usize, appwindow: &mut AppWindow, webview_id: u32, drag_drop_event_target: browser_client::DragDropEventTarget, pending: PendingWebview<T, CefRuntime<T>>) -> Result<()>` | Builds a webview and attaches it to a window, bumping `live_browsers` and relaying it out; shared by window and child creation. |
| 695–1005 | `pub(crate) fn build_browser_child(context: &RuntimeContext<T>, scheme_registry: &request_handler::SchemeRegistry, window_id: WindowId, webview_id: u32, parent: cef::sys::cef_window_handle_t, parent_size: PhysicalSize<u32>, scale: f64, theme: Option<Theme>, drag_drop_event_target: browser_client::DragDropEventTarget, offscreen: Option<crate::OffscreenView>, mut pending: PendingWebview<T, CefRuntime<T>>) -> Option<AppWebview>` | Creates the request context and the CEF browser, registers scripts and observers, and returns the live state; `None` on creation failure. |
| 1007–1047 | `pub(crate) fn handle_webview_message(&mut self, window_id: WindowId, webview_id: u32, message: WebviewMessage)` | Entry point for `Message::Webview`; returns early while exiting; routes window-dependent variants separately. |
| 1049–1222 | `fn handle_window_dependent_webview_message(&mut self, window_id: WindowId, webview_id: u32, message: WebviewMessage)` | Handles the variants that read window metrics (bounds/size/position/auto-resize/`with_webview`/reparent). |
| 1224–1411 | `fn handle_window_independent_webview_message(child: &mut AppWebview, message: WebviewMessage)` | Handles everything that needs only the child (script, navigation, cookies, DevTools, zoom, background, browsing data). |
| 1414–1418 | `#[derive(Clone, Copy, Debug)] pub enum RuntimeStyle { Alloy, Chrome }` | CEF browser runtime style selector. |
| 1420–1530 | `#[derive(Default, Clone)] pub struct CefWebviewAttributes { … }` (6 public fields, §2.8) | CEF-specific webview attributes set through the builder extension; the doc block covers permission requests and Chrome accelerators. |
| 1532–1550 | `impl std::fmt::Debug for CefWebviewAttributes` | Redacts every closure to `is_some()`. |
| 1552–1557 | `#[derive(Debug, Clone)] pub struct CefInitScript { pub(crate) script: String, pub(crate) hash: String, for_main_frame_only: bool }` | One initialization script plus its CSP hash and frame scope. |
| 1559–1581 | `impl CefInitScript { … }` | Frame scoping and construction. |
| 1561–1563 | `pub(crate) fn runs_in_frame(&self, is_main_frame: bool) -> bool` | `is_main_frame || !self.for_main_frame_only`. |
| 1565–1580 | `fn new(script: InitializationScript) -> Self` | Stores the script, computes `'sha256-<base64(sha256(normalize_script_for_csp(script)))>'`. |
| 1583–1593 | `#[cfg(any(debug_assertions, feature = "devtools"))] fn is_alloy_style(runtime_style: Option<RuntimeStyle>) -> bool` | Whether the created browser will be Alloy style (macOS always forces Alloy with a native parent). |
| 1595–1610 | `pub(crate) fn initialization_scripts(attrs: &mut WebviewAttributes) -> Arc<Vec<CefInitScript>>` | Builds the script list: drag-drop script first, then the app's scripts (drained from the attributes). |
| 1612–1617 | `#[derive(Debug, Clone)] pub struct CefWebviewDispatcher<T: UserEvent> { pub(crate) window_id: Arc<Mutex<WindowId>>, pub(crate) webview_id: u32, pub(crate) context: RuntimeContext<T> }` | Thread-safe handle to one webview, implementing `WebviewDispatch`. |
| 1619–1666 | `impl<T: UserEvent> CefWebviewDispatcher<T> { … }` | The two CEF-specific dispatcher methods. |
| 1629–1637 | `pub fn send_dev_tools_message(&self, message: &[u8]) -> Result<()>` | Sends a caller-built CDP message and waits for the send result. |
| 1652–1665 | `pub fn on_dev_tools_protocol<F: Fn(DevToolsProtocol) + Send + Sync + 'static>(&self, f: F) -> Result<()>` | Registers a CDP observer on this browser and waits for the registration result. |
| 1668–1693 | `pub(crate) fn create_webview_detached<T: UserEvent>(context: &RuntimeContext<T>, window_id: WindowId, pending: PendingWebview<T, CefRuntime<T>>) -> Result<DetachedWebview<T, CefRuntime<T>>>` | Creates a webview outside any window and waits for the event loop so a failure surfaces to the caller. |
| 1695–1702 | `fn getter<T: UserEvent, R>(context: &RuntimeContext<T>, message: Message<T>, receiver: Receiver<Result<R>>) -> Result<R>` | Sends a message and blocks for its reply, mapping a closed channel to `FailedToReceiveMessage`. |
| 1704–1720 | `macro_rules! webview_getter { ($self:ident, $variant:ident) => { … } }` | Getter boilerplate that drops the window-id lock **before** blocking. |
| 1722–1771 | `#[cfg(test)] mod getter_tests { … }` | Proves the getter does not hold the window lock while waiting for the UI thread. |
| 1773–2047 | `impl<T: UserEvent> WebviewDispatch<T> for CefWebviewDispatcher<T> { type Runtime = CefRuntime<T>; … }` | The 34 trait members (§3). |
| 2049–2082 | `pub(crate) fn layout_app_window(appwindow: &AppWindow)` | Repositions every child webview from its bounds rate; hides the Linux X11 host when all children are offscreen. |
| 2084–2119 | `pub(crate) fn compute_child_bounds_rate(bounds: Option<&Rect>, auto_resize: bool, parent_size: PhysicalSize<u32>, scale: f64) -> Option<BoundsRate>` | Full-window default when bounds are unset, `None` when auto-resize is off, else the physical bounds over the parent size. |
| 2121–2133 | `pub(crate) const INITIAL_LOAD_URL: &str = concat!("data:text/html;charset=utf-8,", "%3C!doctype%20html%3E", …);` | Inert placeholder document (`data-tauri-cef-internal="initial-load"`) loaded so a `BrowserHost` exists before the real navigation. |
| 2135–2142 | `pub(crate) type PendingInitialLoads = Arc<Mutex<HashMap<i32, (Browser, String)>>>;` | Maps a pending `Page.addScriptToEvaluateOnNewDocument` id to the `(browser, real_url)` whose load waits for it. |
| 2144–2219 | `cef::wrap_dev_tools_message_observer! { struct TauriDevToolsProtocolObserver { handlers: Arc<Mutex<Vec<Arc<DevToolsProtocolHandler>>>>, pending_initial_loads: PendingInitialLoads, dialogs: crate::dialog::DialogState } impl DevToolsMessageObserver { fn on_dev_tools_message(…) -> c_int; fn on_dev_tools_method_result(…); fn on_dev_tools_event(…); } }` | Per-browser CDP observer: fans out all three notification kinds, resolves deferred initial loads, feeds dialog observation. |
| 2221–2238 | `fn runtime_evaluate_result_to_json(result: Option<&[u8]>) -> String` | Extracts the JSON-serialised `value` of a `Runtime.evaluate` result; empty on error, exception or missing value. |
| 2240 | `type EvalScriptCallback = Box<dyn Fn(String) + Send + 'static>;` | One-shot callback type for `eval_script_with_callback`. |
| 2242–2275 | `cef::wrap_dev_tools_message_observer! { struct EvalScriptWithCallbackDevToolsObserver { message_id: i32, callback: Arc<Mutex<Option<EvalScriptCallback>>>, registration: Arc<Mutex<Option<cef::Registration>>> } impl DevToolsMessageObserver { fn on_dev_tools_method_result(…); } }` | One-shot observer that claims exactly its own message id, delivers the result, and unregisters itself. |
| 2277–2309 | `fn apply_user_agent_override(host: &BrowserHost, user_agent: &str)` | Sends `Emulation.setUserAgentOverride` before the first navigation; client hints deliberately untouched. (Lines 2277–2279 are a stray doc comment describing `add_dev_tools_observer` — a doc bug in the reference; mirror or fix, but do not change behaviour.) |
| 2311–2326 | `pub(crate) fn add_dev_tools_observer(browser: &Browser, handlers: Arc<Mutex<Vec<Arc<DevToolsProtocolHandler>>>>, pending_initial_loads: PendingInitialLoads, dialogs: crate::dialog::DialogState) -> Option<cef::Registration>` | Registers the observer, sends `Page.enable`, returns the registration that must be kept alive. |
| 2328–2368 | `fn devtools_initialization_script_source(initialization_scripts: &[CefInitScript], custom_protocol_scheme: &str, custom_scheme_domain_names: &[String]) -> Option<String>` | Builds the single CDP document-start source that guards every script by protocol and frame. |
| 2370–2409 | `fn register_initialization_scripts(browser: &Browser, initialization_scripts: &[CefInitScript], custom_protocol_scheme: &str, custom_scheme_domain_names: &[String], initial_url: String, pending_initial_loads: &PendingInitialLoads) -> std::result::Result<bool, crate::DevToolsMessageIdExhausted>` | Sends `Page.addScriptToEvaluateOnNewDocument` and parks the real URL until it is acknowledged. |
| 2411–2422 | `wrap_task! { struct LoadInitialUrlTask { browser: Browser, initial_url: String } impl Task { fn execute(&self); } }` | UI-thread task that performs the deferred real navigation. |
| 2424–2427 | `fn post_load_initial_url(browser: Browser, initial_url: String)` | Posts `LoadInitialUrlTask` to `TID_UI`. |
| 2429–2468 | `pub(crate) fn load_initial_url_after_registering_initialization_scripts(browser: &Browser, initialization_scripts: &[CefInitScript], custom_protocol_scheme: &str, custom_scheme_domain_names: &[String], initial_url: &str, pending_initial_loads: &PendingInitialLoads)` | Registers scripts then either loads immediately, waits for the CDP ack, or closes the browser on id exhaustion. |
| 2470–2474 | `fn load_initial_url(browser: &Browser, initial_url: &str)` | Loads the URL in the main frame. |
| 2476–2500 | `#[cfg(test)] mod tests { fn background_is_transparent_only_on_request() }` | Background colour defaults to opaque white, transparent only on request, explicit colour wins. |

#### 1.1.3 `impl Webview` member list

| line | member | intent |
| --- | --- | --- |
| 84 | `new` | `pub(crate)` constructor. |
| 100 | `snapshot` | Sampled state. |
| 106 | `frame_navigation_state` | Live navigation state. |
| 110 | `set_opener` | `pub(crate)`, popup bookkeeping. |
| 116 | `popups` | Popup descendants. |
| 121 | `opener` | Popup opener lifetime. |
| 127 | `for_document` | Document lookup across the family. |
| 137 | `browser` | Native browser handle. |

#### 1.1.4 `WebviewMessage` variants (lines 434–473)

35 variants: 32 unconditional plus 3 behind `#[cfg(any(debug_assertions, feature = "devtools"))]`.

```rust
pub(crate) enum WebviewMessage {
  EvaluateScript(String),
  EvaluateScriptWithCallback(String, Box<dyn Fn(String) + Send + 'static>),
  Navigate(Url),
  Reload,
  GoBack,
  CanGoBack(Sender<Result<bool>>),
  GoForward,
  CanGoForward(Sender<Result<bool>>),
  Print,
  Close,
  Show,
  Hide,
  SetPosition(Position),
  SetSize(Size),
  SetBounds(Rect),
  SetFocus,
  Reparent(WindowId, Sender<Result<()>>),
  SetAutoResize(bool),
  SetZoom(f64),
  SetBackgroundColor(Option<Color>),
  ClearAllBrowsingData,
  Url(Sender<Result<String>>),
  Bounds(Sender<Result<Rect>>),
  Position(Sender<Result<PhysicalPosition<i32>>>),
  Size(Sender<Result<PhysicalSize<u32>>>),
  WithWebview(Box<dyn FnOnce(Webview) + Send>),
  CookiesForUrl(Url, Sender<Result<Vec<Cookie<'static>>>>),
  Cookies(Sender<Result<Vec<Cookie<'static>>>>),
  SetCookie(Cookie<'static>),
  DeleteCookie(Cookie<'static>),
  #[cfg(any(debug_assertions, feature = "devtools"))]
  OpenDevTools,
  #[cfg(any(debug_assertions, feature = "devtools"))]
  CloseDevTools,
  #[cfg(any(debug_assertions, feature = "devtools"))]
  IsDevToolsOpen(Sender<bool>),
  SendDevToolsMessage(Vec<u8>, Sender<Result<()>>),
  OnDevToolsProtocol(Arc<DevToolsProtocolHandler>, Sender<Result<()>>),
}
```

#### 1.1.5 `AppWebview` fields (lines 496–515)

```rust
pub(crate) struct AppWebview {
  pub(crate) webview_id: u32,
  pub(crate) label: String,
  pub(crate) browser: cef::Browser,
  pub(crate) browser_id: i32,
  pub(crate) frame_navigation_state: crate::FrameNavigationState,
  pub(crate) popup_family: Arc<crate::popup::PopupFamily>,
  pub(crate) dialogs: crate::dialog::DialogState,
  pub(crate) host: cef::BrowserHost,
  pub(crate) uri_scheme_protocols: Arc<HashMap<String, Arc<Box<UriSchemeProtocolHandler>>>>,
  pub(crate) devtools_protocol_handlers: Arc<Mutex<Vec<Arc<DevToolsProtocolHandler>>>>,
  /// Keeps the DevTools message observer registered. Dropping this unregisters the observer.
  pub(crate) devtools_observer_registration: Arc<Mutex<Option<cef::Registration>>>,
  /// Whether a DevTools window may be opened for this webview. The DevTools *protocol*
  /// stays available either way — the runtime's own startup rides on it.
  pub(crate) devtools_enabled: bool,
  pub(crate) bounds_rate: Option<BoundsRate>,
  pub(crate) offscreen: Option<crate::OffscreenView>,
  pub(crate) page_fullscreen: PageFullscreen,
}
```

---

### 1.2 `src/frame.rs`

| line | declaration | doc-comment intent |
| --- | --- | --- |
| 5 | `use std::sync::Arc;` | Handler sharing. |
| 7–26 | `#[derive(Clone, Debug)] #[non_exhaustive] pub struct FrameEvent { pub browser_id: i32, pub frame_id: String, pub is_main: bool, pub kind: FrameEventKind }` | One browser-process notification about a native CEF frame; synchronous on the CEF UI thread, includes child frames. |
| 28–56 | `#[derive(Clone, Debug)] #[non_exhaustive] pub enum FrameEventKind { … }` (11 variants, §2.11) | Native lifecycle phases reported for every frame. |
| 58–59 | `pub type FrameEventHandler = dyn Fn(FrameEvent) + Send + Sync + 'static;` | Synchronous observer for native frame lifecycle events. |
| 61–79 | `pub(crate) fn emit_frame_event(handler: &Option<Arc<FrameEventHandler>>, browser: Option<&mut cef::Browser>, frame: Option<&mut cef::Frame>, kind: FrameEventKind)` | Builds a `FrameEvent` from the native browser/frame and calls the handler; no-op when either is missing. |

Field docs worth preserving verbatim in the port:

- `browser_id` — "Native browser identity. Every event delivered to one webview's observer carries that webview's own identity; a CEF-owned popup is a different browser and is not reported here."
- `frame_id` — "CEF's opaque identifier for this native frame lifetime. Empty for a browser-wide notification when no main frame exists."
- `is_main` — "Whether CEF identifies this as the main frame at callback time."
- `kind` — "Native lifecycle phase."

---

### 1.3 `src/frame_navigation.rs`

| line | declaration | doc-comment intent |
| --- | --- | --- |
| 5 | `//! Native document generations shared by every CEF webview.` | Module purpose. |
| 7–8 | `use std::collections::BTreeMap; use std::collections::BTreeSet;` | Ordered frame map (deterministic comparison) and frame-id set. |
| 10 | `use std::sync::{Arc, Mutex};` | Shared state. |
| 12 | `const MAX_NATIVE_FRAMES: usize = 256;` | Tracked-frame cap; exceeding it fails admission closed. |
| 13 | `const MAX_NATIVE_FRAME_ID_BYTES: usize = 512;` | Frame-identifier length cap; exceeding it latches exhaustion. |
| 15–24 | `#[derive(Debug, Default)] struct NativeFrameState { browser_id: Option<i32>, generation: u64, exhausted: bool, loading: bool, observed_load_state: bool, main_frame: Option<String>, frames: BTreeMap<String, bool> }` | All state for one native browser lifetime. |
| 26–32 | `#[derive(Clone, Debug)] pub struct FrameNavigationState { state: Arc<Mutex<NativeFrameState>> }` | Read-only navigation state for one exact native browser lifetime; advanced on the CEF UI thread before input admission. |
| 34–41 | `#[derive(Clone)] pub struct NativeDocumentToken { state: Arc<Mutex<NativeFrameState>>, generation: u64 }` | Opaque process-local proof of an observed native browser/document lifetime. |
| 43–48 | `impl PartialEq for NativeDocumentToken { … } impl Eq for NativeDocumentToken {}` | `Arc::ptr_eq(&self.state, &other.state) && self.generation == other.generation`. |
| 50–56 | `impl std::fmt::Debug for NativeDocumentToken` | `debug_struct("NativeDocumentToken").finish_non_exhaustive()`. |
| 58–270 | `impl FrameNavigationState { … }` | State machine (§5.8). |
| 59–63 | `pub(crate) fn new() -> Self` | Fresh state with a default (unbound) browser id. |
| 65–69 | `pub fn is_same_browser(&self, other: &Self) -> bool` | Identifies the same native browser lifetime independently of navigation. |
| 71–76 | `pub(crate) fn has_browser_id(&self, browser_id: i32) -> bool` | Whether this state is bound to that native browser identity. |
| 78–82 | `pub(crate) fn close(&self)` | Latches `exhausted` (teardown / popup revocation). |
| 84–115 | `pub(crate) fn observe_document(&self, browser: &cef::Browser) -> Option<NativeDocumentToken>` | Validates the ready generation against the live native frame identities and load state. |
| 117–127 | `pub fn document(&self) -> Option<NativeDocumentToken>` | Captures a token from state alone, without querying current CEF frame ids. |
| 129–147 | `fn ready_generation(&self) -> Option<u64>` | `ready()` plus a debug log naming which condition failed. |
| 149–163 | `fn ready(state: &NativeFrameState) -> bool` | The admission predicate (§5.8). |
| 165–172 | `pub(crate) fn on_frame_event(&self, event: &crate::FrameEvent)` | Applies a `FrameEvent` to the state. |
| 174–250 | `fn apply(&self, browser_id: i32, frame_id: &str, is_main: bool, kind: &crate::FrameEventKind)` | The state transition table (§5.8). |
| 252–269 | `pub(crate) fn admits_native_frames(&self, expected: u64, browser_id: i32, is_loading: bool, frame_ids: &BTreeSet<String>) -> bool` | Final admission: exact frame identities, generation, browser identity and load state. |
| 272–462 | `#[cfg(test)] mod tests { … }` | Six tests pinning the invariants (§5.8.7). |

---

### 1.4 `src/tauri_ext.rs`

| line | declaration | doc-comment intent |
| --- | --- | --- |
| 5–9 | `//! Extension traits exposing CEF-specific APIs on [`tauri`] types.` | Traits are implemented for `CefRuntime` and for `tauri::DynRuntime`; with the latter the methods fail with `Error::RuntimeTypeMismatch` off CEF. |
| 11 | `use std::sync::Arc;` | Handler wrapping. |
| 13 | `use tauri::{EventLoopMessage, Manager, Runtime, Webview, WebviewWindow};` | The `tauri` types the traits extend. |
| 14 | `use tauri_runtime::dynamic::{DynWebviewAttributes, DynWebviewDispatcher, DynWindowOpener};` | Type-erased downcast targets. |
| 16–19 | `use crate::{CefWebviewAttributes, CefWebviewDispatcher, ChromeCommandGroup, ConsoleMessage, DevToolsProtocol, FrameEvent, NewWindowOpener, RuntimeStyle};` | Items the traits expose. |
| 21 | `type Result<T> = std::result::Result<T, tauri::Error>;` | The `tauri`-level error the extension traits return. |
| 23–28 | `fn not_cef() -> tauri::Error` | Builds `Error::RuntimeTypeMismatch("the application is not running on the CEF runtime")`. |
| 30–34 | `pub trait AsCefWebviewDispatcher { fn as_cef_webview_dispatcher(&self) -> Option<&CefWebviewDispatcher<EventLoopMessage>>; }` | Webview dispatchers that may expose the underlying CEF dispatcher. |
| 36–40 | `impl AsCefWebviewDispatcher for CefWebviewDispatcher<EventLoopMessage>` | Returns `Some(self)`. |
| 42–46 | `impl AsCefWebviewDispatcher for DynWebviewDispatcher<EventLoopMessage>` | `self.downcast_ref()`. |
| 48–73 | `pub trait AsCefWindowOpener { fn as_cef_window_opener(&self) -> Option<&NewWindowOpener>; }` | Window openers that may expose the CEF `NewWindowOpener`; doc carries a runnable example reading `opener.source_url()` from `on_new_window`. |
| 75–79 | `impl AsCefWindowOpener for NewWindowOpener` | Returns `Some(self)`. |
| 81–85 | `impl AsCefWindowOpener for DynWindowOpener` | `self.downcast_ref()`. |
| 87–91 | `pub trait AsCefWebviewAttributes { fn as_cef_webview_attributes_mut(&mut self) -> Option<&mut CefWebviewAttributes>; }` | Runtime webview attributes that may expose the CEF attributes. |
| 93–97 | `impl AsCefWebviewAttributes for CefWebviewAttributes` | Returns `Some(self)`. |
| 99–103 | `impl AsCefWebviewAttributes for DynWebviewAttributes` | `self.get_or_default()` — **installs** CEF attributes when the slot is empty. |
| 105–116 | `fn with_cef_webview_attributes<A: AsCefWebviewAttributes>(attributes: &mut A, f: impl FnOnce(&mut CefWebviewAttributes))` | Applies `f`, or logs a warning when the builder already holds another runtime's attributes. |
| 118–208 | `pub trait WebviewCefExt { … }` (3 methods, §4.1) | CEF-specific APIs of `tauri::Webview` and `tauri::WebviewWindow`. |
| 149 | `fn send_dev_tools_message(&self, message: &[u8]) -> Result<()>;` | Send a CDP message whose `id` came from `allocate_devtools_message_id`. |
| 198–201 | `fn on_dev_tools_protocol<F: Fn(DevToolsProtocol) + Send + Sync + 'static>(&self, f: F) -> Result<()>;` | Register a CDP observer scoped to this webview's own native browser. |
| 207 | `fn with_cef_webview<F: FnOnce(&crate::Webview) + Send + 'static>(&self, f: F) -> Result<()>;` | Run a closure with the CEF webview handle on the CEF UI thread. |
| 210–245 | `impl<R: Runtime> WebviewCefExt for Webview<R> where R::WebviewDispatcher: AsCefWebviewDispatcher` | Delegates to the dispatcher; `with_cef_webview` downcasts the `with_webview` argument to `crate::Webview`. |
| 247–265 | `impl<R: Runtime> WebviewCefExt for WebviewWindow<R> where R::WebviewDispatcher: AsCefWebviewDispatcher` | Forwards each method to `self.as_ref()`. |
| 267–356 | `pub trait WebviewWindowBuilderCefExt { … }` (5 methods, §4.2) | CEF-specific APIs of `tauri::WebviewWindowBuilder`. |
| 358–410 | `impl<'a, R: Runtime, M: Manager<R>> WebviewWindowBuilderCefExt for tauri::WebviewWindowBuilder<'a, R, M> where R::RuntimeWebviewAttributes: AsCefWebviewAttributes` | Stores each setting in the runtime-specific attributes. |
| 412–470 | `#[cfg(feature = "unstable")] pub trait WebviewBuilderCefExt { … }` (same 5 methods, §4.3) | CEF-specific APIs of `tauri::webview::WebviewBuilder`. |
| 472–524 | `#[cfg(feature = "unstable")] impl<R: Runtime> WebviewBuilderCefExt for tauri::webview::WebviewBuilder<R> where R::RuntimeWebviewAttributes: AsCefWebviewAttributes` | Same storage, for the unstable builder. |

---

## 2. Public types

Every type below is reachable as `tauri_runtime_cef::…` because `lib.rs` does
`pub use webview::*; pub use tauri_ext::*;` plus the explicit `frame`/`frame_navigation`
re-exports. The Electron crate must expose the same paths.

### 2.1 `Webview` (`webview.rs:35-46`)

```rust
#[derive(Clone)]
pub struct Webview {
  browser: cef::Browser,
  snapshot: WebviewSnapshot,
  frame_navigation_state: crate::FrameNavigationState,
  popups: Vec<Webview>,
  opener: Option<crate::FrameNavigationState>,
}
```

- All five fields are **private**; the only public surface is the eight methods in §1.1.3.
- No `Debug` impl. `Clone` shares the native browser handle (CEF refcounted) and clones the
  snapshot; `popups` is a `Vec<Webview>` so a handle owns its observed descendants.
- Doc: "A handle to the native CEF browser backing a Tauri webview. This is the
  runtime-specific webview object exposed through `tauri_runtime::WebviewDispatch::with_webview`."

### 2.2 `WebviewSnapshot` (`webview.rs:48-81`)

```rust
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct WebviewSnapshot {
  pub browser_id: i32,
  pub dialogs: crate::NativeDialogObservation,
  pub document: Option<crate::NativeDocumentToken>,
  pub window_label: Option<String>,
  pub window: Option<crate::NativeWindowToken>,
  pub parent_matches: Option<bool>,
  pub bounds: Option<Rect>,
  pub visible: Option<bool>,
}
```

| field | doc |
| --- | --- |
| `browser_id` | Native browser identity, distinct for each popup. |
| `dialogs` | Native JavaScript dialog observation. Unknown is distinct from absent. |
| `document` | All-frame document generation validated against the current native frame identities and load state. `None` means document admission is unavailable. |
| `window_label` | Runtime window label; CEF-owned popups have no Tauri window label. |
| `window` | Opaque lifetime of the runtime window. Labels and native handle values may be reused after teardown; this token distinguishes their replacements. `None` means the runtime could not observe a native window lifetime. |
| `parent_matches` | Whether the actual native parent matches the observed native window. `None` means the relationship could not be established. A CEF-owned popup always reports `None`, permanently: CEF owns its native window. Offscreen webviews also report `None`. |
| `bounds` | Current bounds relative to the native parent, in the indicated DPI units. Offscreen bounds are the requested layout, in logical pixels. |
| `visible` | Native view visibility. `None` means native inspection was unavailable. Separate from occlusion, minimization and page lifecycle. Offscreen webviews report their requested visibility. |

`#[non_exhaustive]` matters: downstream crates cannot construct it, so the Electron port can
add fields without a breaking change — but must not remove or rename these.

### 2.3 `ConsoleMessage` (`webview.rs:364-380`)

```rust
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct ConsoleMessage {
  pub level: ConsoleMessageLevel,
  pub message: String,
  pub source: String,
  pub line: i32,
}
```

| field | doc |
| --- | --- |
| `level` | How severe the renderer considers the message. |
| `message` | The message text, already formatted by the renderer the way DevTools shows it. |
| `source` | What wrote the message — a script URL, usually. Empty when CEF names none. |
| `line` | The 1-based line in `source`. Zero when CEF names none. |

### 2.4 `ConsoleMessageLevel` (`webview.rs:382-400`)

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ConsoleMessageLevel {
  Verbose, // console.debug
  Info,    // console.log / console.info; also CEF's default severity (INFO)
  Warning, // console.warn
  Error,   // console.error and renderer-reported errors (uncaught exception, blocked subresource)
  Fatal,   // fatal log severity; no console API produces one
  Other,   // a severity this build does not name
}
```

### 2.5 `ChromeCommandGroup` (`webview.rs:301-362`)

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ChromeCommandGroup {
  WindowAndTab,
  Document,
  BrowserChrome,
  BrowserSurface,
  History,
}
```

| variant | covers (from the doc) |
| --- | --- |
| `WindowAndTab` | Ctrl+N, Ctrl+Shift+N, Ctrl+T and the whole tab strip: new window, new incognito window, new tab, duplicate, restore, reorder, select tab 1–8. |
| `Document` | Ctrl+P, Ctrl+S, Ctrl+U, Ctrl+O: print, print without preview, save page, view source, open file, PWA install and shortcut commands. |
| `BrowserChrome` | Ctrl+L and neighbours: focus omnibox / search box / toolbar / menu bar / bookmarks bar, plus Home and open-current-URL. |
| `BrowserSurface` | Ctrl+H, Ctrl+J, Ctrl+D, Ctrl+Shift+Delete and the rest: history, downloads, bookmarks, settings, clear browsing data, task manager, sign-in, about, feedback. |
| `History` | Alt+Left and Alt+Right: back and forward through the session history. |

`ALL` (`pub(crate)`) is the default blocklist and the source of truth for it: a variant added
to the enum is blocked by default. DevTools and zoom are deliberately **not** groups — they
follow `WebviewAttributes::devtools` and `WebviewAttributes::zoom_hotkeys_enabled`.

### 2.6 `DevToolsProtocol` (`webview.rs:271-299`)

```rust
#[derive(Debug, Clone)]
pub enum DevToolsProtocol {
  Message(Vec<u8>),
  Event { method: String, params: Vec<u8> },
  MethodResult { message_id: i32, success: bool, result: Vec<u8> },
}
```

| variant | doc |
| --- | --- |
| `Message(Vec<u8>)` | The raw agent message, before it is classified as an event or a result. |
| `Event { method, params }` | An agent event. Events are unsolicited and carry no request identifier. |
| `MethodResult { message_id, success, result }` | The result of one request. `message_id` correlates with the `id` of the request that produced it; compare it against an identifier from `allocate_devtools_message_id`. A result whose identifier you did not allocate answers someone else's request. Numeric correlation does not authorize a browser or document. |

Type-level doc: observers see the whole browser, including requests issued by the runtime
itself and by other callers, so nothing is scoped to one producer; no notification names a
browser because an observer is registered on one native browser and never receives another's
traffic.

### 2.7 `RuntimeStyle` (`webview.rs:1414-1418`)

```rust
#[derive(Clone, Copy, Debug)]
pub enum RuntimeStyle {
  Alloy,
  Chrome,
}
```

No doc comment, no `#[non_exhaustive]`, no `PartialEq`. `None` on the attribute means "let
CEF pick" (`cef::RuntimeStyle::DEFAULT`).

### 2.8 `CefWebviewAttributes` (`webview.rs:1420-1550`)

```rust
#[derive(Default, Clone)]
pub struct CefWebviewAttributes {
  pub offscreen: Option<crate::Offscreen>,
  pub runtime_style: Option<RuntimeStyle>,
  pub frame_event_handler: Option<Arc<crate::FrameEventHandler>>,
  pub console_message_handler: Option<Arc<ConsoleMessageHandler>>,
  pub allowed_chrome_commands: Vec<ChromeCommandGroup>,
  pub browser_settings_callback: Option<Arc<BrowserSettingsCallback>>,
}
```

| field | doc |
| --- | --- |
| `offscreen` | Windowless rendering and application-provided CEF callbacks. |
| `runtime_style` | The browser runtime style; CEF picks one when not set. |
| `frame_event_handler` | Observer of the native lifecycle events of every frame of the webview. Scoped to this webview's own native browser; popups are separate browsers and are never reported here. |
| `console_message_handler` | Observer of the renderer's console output. Scoped to this webview's own native browser; neither a popup's output nor a DevTools window's is reported here. |
| `allowed_chrome_commands` | Families of Chrome commands this webview keeps rather than swallows. Empty by default, which blocks every group. |
| `browser_settings_callback` | Last look at `cef::BrowserSettings` before the browser is created; runs after the portable attributes are mapped. |

The type-level doc block (`1420-1496`) is long and load-bearing: it documents permission
request semantics (once-per-origin persistence, `PermissionKind::Other` for kinds Tauri has no
name for, `DisplayCapture` never granted by an `Allow`) and the Chrome accelerators an app
window does not get (zoom defaults to off, history accelerators dead because of the placeholder
first navigation). The Electron port must carry the same doc text with the CEF-specific claims
corrected where Electron differs (§6).

`Debug` is hand-written (`1532-1550`): `runtime_style` verbatim, `offscreen` /
`frame_event_handler` / `console_message_handler` / `browser_settings_callback` as
`is_some()`, `allowed_chrome_commands` verbatim.

### 2.9 `CefInitScript` (`webview.rs:1552-1581`)

```rust
#[derive(Debug, Clone)]
pub struct CefInitScript {
  pub(crate) script: String,
  pub(crate) hash: String,
  for_main_frame_only: bool,
}
```

- Public type with **no public fields and no public constructor**: `new` is private,
  `runs_in_frame` is `pub(crate)`. It exists so the runtime can hold scripts across the
  scheme registry; downstream code cannot build one.
- `hash` is `'sha256-<base64-standard(sha256(normalize_script_for_csp(script)))>'`, consumed by
  the custom-protocol handler to extend the document CSP.
- The Electron port keeps the same shape (`ElectronInitScript`) with the same visibility.

### 2.10 `FrameEvent` / `FrameEventKind` (`frame.rs:7-56`)

```rust
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct FrameEvent {
  pub browser_id: i32,
  pub frame_id: String,
  pub is_main: bool,
  pub kind: FrameEventKind,
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum FrameEventKind {
  Created,
  Attached,
  Detached,
  Destroyed,
  NavigationStarted { url: url::Url },
  DocumentCommitted { url: url::Url },
  NavigationFailed { url: url::Url },
  AddressChanged { url: url::Url },
  MainFrameChanged,
  LoadingStateChanged { is_loading: bool },
  RendererTerminated,
}
```

| variant | doc |
| --- | --- |
| `Created` | A frame object exists, but may not yet have a renderer connection. |
| `Attached` | Commands can be routed to the renderer. Reattachment is also reported. |
| `Detached` | The frame can no longer route commands to its renderer. |
| `Destroyed` | The native frame object is being destroyed. |
| `NavigationStarted { url }` | A navigation was admitted by the existing navigation policy, before commit. |
| `DocumentCommitted { url }` | A document committed, before its contents begin loading. |
| `NavigationFailed { url }` | Navigation failed or was cancelled. **Not** a document-ready signal. |
| `AddressChanged { url }` | An address changed, including a same-document history or fragment change. |
| `MainFrameChanged` | The browser assigned this frame as its main frame. |
| `LoadingStateChanged { is_loading }` | Browser-wide load state. `false` follows every frame's load-end/error notifications, including cancelled navigation. Does not assert DOM, application, network-idle or renderer responsiveness. |
| `RendererTerminated` | The renderer terminated. Prior document generations cannot be reused. |

### 2.11 `FrameNavigationState` / `NativeDocumentToken` (`frame_navigation.rs:26-56`)

```rust
#[derive(Clone, Debug)]
pub struct FrameNavigationState {
  state: Arc<Mutex<NativeFrameState>>,
}

#[derive(Clone)]
pub struct NativeDocumentToken {
  state: Arc<Mutex<NativeFrameState>>,
  generation: u64,
}
```

- `FrameNavigationState`: private field; public API is `is_same_browser` and `document`
  (`new`, `has_browser_id`, `close`, `observe_document`, `on_frame_event`,
  `admits_native_frames` are `pub(crate)`).
- `NativeDocumentToken`: private fields; `Clone`, `PartialEq`, `Eq`, `Debug`
  (`finish_non_exhaustive`). Equality is pointer identity **and** generation, so a token from
  another browser never matches even at the same generation number.

### 2.12 Public type aliases

| alias | declaration | file:line |
| --- | --- | --- |
| `ConsoleMessageHandler` | `pub type ConsoleMessageHandler = dyn Fn(ConsoleMessage) + Send + Sync + 'static;` | `webview.rs:403` |
| `BrowserSettingsCallback` | `pub type BrowserSettingsCallback = dyn Fn(&mut cef::BrowserSettings) + Send + Sync + 'static;` | `webview.rs:406` |
| `FrameEventHandler` | `pub type FrameEventHandler = dyn Fn(FrameEvent) + Send + Sync + 'static;` | `frame.rs:59` |

Crate-private aliases that are part of the internal contract (must exist, same shape):

| alias | declaration | file:line |
| --- | --- | --- |
| `DevToolsProtocolHandler` | `pub(crate) type DevToolsProtocolHandler = dyn Fn(DevToolsProtocol) + Send + Sync;` | `webview.rs:299` |
| `PendingInitialLoads` | `pub(crate) type PendingInitialLoads = Arc<Mutex<HashMap<i32, (Browser, String)>>>;` | `webview.rs:2142` |
| `EvalScriptCallback` | `type EvalScriptCallback = Box<dyn Fn(String) + Send + 'static>;` | `webview.rs:2240` |
| `Result` (tauri_ext) | `type Result<T> = std::result::Result<T, tauri::Error>;` | `tauri_ext.rs:21` |

Note `DevToolsProtocolHandler` has **no** `'static` bound in the alias itself; the `'static`
comes from the `Arc<dyn …>` uses. Keep it identical so type inference in the port matches.

---

## 3. `WebviewDispatch for CefWebviewDispatcher`

`webview.rs:1773-2047`, `type Runtime = CefRuntime<T>;` at 1774. 34 members on desktop
(`with_ios_webview` is `cfg(target_os = "ios")` and absent here).

Every member except the getters is fire-and-forget: it sends `Message::Webview` and returns as
soon as the message is queued. Getters (`webview_getter!`) block on a reply channel.

| line | member | what it does |
| --- | --- | --- |
| 1776 | `run_on_main_thread<F: FnOnce() + Send + 'static>(&self, f: F) -> Result<()>` | Delegates to `RuntimeContext::run_on_main_thread` (runs inline on the main thread, else queues `Message::Task`). |
| 1780 | `with_webview<F: FnOnce(<Self::Runtime as Runtime<T>>::Webview) + Send + 'static>(&self, f: F) -> Result<()>` | Queues `WebviewMessage::WithWebview(Box::new(f))`; the UI thread samples the snapshot and calls `f`. |
| 1791 | `open_devtools(&self)` | Queues `OpenDevTools`; with devtools compiled out, logs a warning instead. |
| 1804 | `close_devtools(&self)` | Queues `CloseDevTools`; no-op when compiled out. |
| 1815 | `is_devtools_open(&self) -> Result<bool>` | Round-trips `IsDevToolsOpen`; returns `Ok(false)` when compiled out. |
| 1832 | `url(&self) -> Result<String>` | `webview_getter!(self, Url)`; a missing main frame yields `""`. |
| 1836 | `bounds(&self) -> Result<Rect>` | `webview_getter!(self, Bounds)`; `FailedToSendMessage` when the platform cannot read bounds. |
| 1840 | `position(&self) -> Result<PhysicalPosition<i32>>` | `webview_getter!(self, Position)`; bounds position converted to physical with the window scale factor. |
| 1844 | `size(&self) -> Result<PhysicalSize<u32>>` | `webview_getter!(self, Size)`; bounds size converted to physical. |
| 1848 | `navigate(&self, url: Url) -> Result<()>` | Queues `Navigate`; the UI thread loads it in the main frame. |
| 1856 | `reload(&self) -> Result<()>` | Queues `Reload` → `browser.reload()`. |
| 1864 | `go_back(&self) -> Result<()>` | Queues `GoBack` → `browser.go_back()`. |
| 1872 | `can_go_back(&self) -> Result<bool>` | Round-trips `CanGoBack` → `browser.can_go_back() == 1`. |
| 1876 | `go_forward(&self) -> Result<()>` | Queues `GoForward` → `browser.go_forward()`. |
| 1884 | `can_go_forward(&self) -> Result<bool>` | Round-trips `CanGoForward` → `browser.can_go_forward() == 1`. |
| 1888 | `print(&self) -> Result<()>` | Queues `Print` → `host.print()`. |
| 1896 | `close(&self) -> Result<()>` | Queues `Close` → `host.close_browser(0)` (not forced). |
| 1904 | `set_bounds(&self, bounds: Rect) -> Result<()>` | Queues `SetBounds`; window-dependent path recomputes the bounds rate and resizes. |
| 1912 | `set_size(&self, size: Size) -> Result<()>` | Queues `SetSize`; keeps the current position. |
| 1920 | `set_position(&self, position: Position) -> Result<()>` | Queues `SetPosition`; keeps the current size. |
| 1928 | `set_focus(&self) -> Result<()>` | Queues `SetFocus` → `host.set_focus(1)`. |
| 1936 | `hide(&self) -> Result<()>` | Queues `Hide` → `set_visible(false)`. |
| 1944 | `show(&self) -> Result<()>` | Queues `Show` → `set_visible(true)`. |
| 1952 | `eval_script<S: Into<String>>(&self, script: S) -> Result<()>` | Queues `EvaluateScript`; main-frame `execute_java_script`, result discarded, errors invisible. |
| 1960 | `eval_script_with_callback<S: Into<String>>(&self, script: S, callback: impl Fn(String) + Send + 'static) -> Result<()>` | Queues `EvaluateScriptWithCallback`; the callback runs on the UI thread with a JSON string (§5.2). |
| 1972 | `reparent(&self, window_id: WindowId) -> Result<()>` | Round-trips `Reparent`; on success updates `*self.window_id` to the new window. |
| 1986 | `cookies_for_url(&self, url: Url) -> Result<Vec<Cookie<'static>>>` | Round-trips `CookiesForUrl`; URL-filtered visit (§5.3). |
| 1996 | `cookies(&self) -> Result<Vec<Cookie<'static>>>` | `webview_getter!(self, Cookies)`; whole store of the request context. |
| 2000 | `set_cookie(&self, cookie: Cookie<'_>) -> Result<()>` | Queues `SetCookie(cookie.into_owned())`; fire-and-forget. |
| 2008 | `delete_cookie(&self, cookie: Cookie<'_>) -> Result<()>` | Queues `DeleteCookie(cookie.into_owned())`; fire-and-forget. |
| 2016 | `set_auto_resize(&self, auto_resize: bool) -> Result<()>` | Queues `SetAutoResize`; recomputes or clears the bounds rate. |
| 2024 | `set_zoom(&self, scale_factor: f64) -> Result<()>` | Queues `SetZoom`; converts linear factor to CEF's log level. |
| 2032 | `set_background_color(&self, color: Option<Color>) -> Result<()>` | Queues `SetBackgroundColor`; platform-specific (Windows currently a no-op). |
| 2040 | `clear_all_browsing_data(&self) -> Result<()>` | Queues `ClearAllBrowsingData`; request-context-scoped cookie + HTTP-cache clear (§5.4). |

`CefWebviewDispatcher` also exposes two inherent methods (§1.1.2, `1629`/`1652`) that the
`WebviewCefExt` trait forwards to.

---

## 4. Extension traits in `tauri_ext.rs`

Purpose (module doc): expose CEF-specific APIs on `tauri` types, implemented for the statically
typed `CefRuntime` **and** for the type-erased `tauri::DynRuntime`; with the latter the methods
fail with `tauri_runtime::Error::RuntimeTypeMismatch` when the app is not running on CEF.

### 4.1 `WebviewCefExt` (`tauri_ext.rs:118-265`)

```rust
pub trait WebviewCefExt {
  fn send_dev_tools_message(&self, message: &[u8]) -> Result<()>;
  fn on_dev_tools_protocol<F: Fn(DevToolsProtocol) + Send + Sync + 'static>(&self, f: F) -> Result<()>;
  fn with_cef_webview<F: FnOnce(&crate::Webview) + Send + 'static>(&self, f: F) -> Result<()>;
}
```

| method | exact signature | what it enables on the `tauri` side |
| --- | --- | --- |
| `send_dev_tools_message` | `fn send_dev_tools_message(&self, message: &[u8]) -> Result<()>;` | Send a raw CDP message from `Webview`/`WebviewWindow`; the doc requires the `id` to come from `allocate_devtools_message_id` and explains the shared id space and the runtime's reserved range. Carries a runnable `Page.enable` example. |
| `on_dev_tools_protocol` | `fn on_dev_tools_protocol<F: Fn(DevToolsProtocol) + Send + Sync + 'static>(&self, f: F) -> Result<()>;` | Register an observer for method results and events on this webview's own browser; doc stresses matching `MethodResult` against an allocated id and that popups are separate browsers observed through `Webview::popups`. Carries a runnable example. |
| `with_cef_webview` | `fn with_cef_webview<F: FnOnce(&crate::Webview) + Send + 'static>(&self, f: F) -> Result<()>;` | Run a closure with the CEF webview handle on the CEF UI thread; the snapshot is sampled immediately before the closure and not refreshed afterwards. |

Implementations:

- `impl<R: Runtime> WebviewCefExt for Webview<R> where R::WebviewDispatcher: AsCefWebviewDispatcher`
  (`210-245`): `send_dev_tools_message`/`on_dev_tools_protocol` resolve
  `self.dispatcher().as_cef_webview_dispatcher().ok_or_else(not_cef)?` and forward.
  `with_cef_webview` first fails fast with `not_cef()` when the dispatcher is not CEF, then
  calls `self.with_webview(move |webview| if let Some(webview) = webview.downcast_ref::<crate::Webview>() { f(webview) })`.
  Note the silent-drop behaviour: if the downcast fails inside the callback, `f` never runs and
  no error is returned (the check above makes that unreachable in practice).
- `impl<R: Runtime> WebviewCefExt for WebviewWindow<R> where R::WebviewDispatcher: AsCefWebviewDispatcher`
  (`247-265`): each method forwards to `self.as_ref()`.

### 4.2 `WebviewWindowBuilderCefExt` (`tauri_ext.rs:267-410`)

```rust
pub trait WebviewWindowBuilderCefExt {
  #[must_use] fn browser_runtime_style(self, style: RuntimeStyle) -> Self;
  #[must_use] fn on_frame_event<F: Fn(FrameEvent) + Send + Sync + 'static>(self, handler: F) -> Self;
  #[must_use] fn on_console_message<F: Fn(ConsoleMessage) + Send + Sync + 'static>(self, handler: F) -> Self;
  #[must_use] fn allow_chrome_commands<I: IntoIterator<Item = ChromeCommandGroup>>(self, groups: I) -> Self;
  #[must_use] fn with_browser_settings<F: Fn(&mut cef::BrowserSettings) + Send + Sync + 'static>(self, callback: F) -> Self;
}
```

| method | exact signature | what it enables |
| --- | --- | --- |
| `browser_runtime_style` | `fn browser_runtime_style(self, style: RuntimeStyle) -> Self;` | Sets `ElectronWebviewAttributes::runtime_style` (Alloy/Chrome/let-CEF-pick). |
| `on_frame_event` | `fn on_frame_event<F: Fn(FrameEvent) + Send + Sync + 'static>(self, handler: F) -> Self;` | Stores `Arc::new(handler)` in `frame_event_handler`. Doc: runs synchronously on the CEF UI thread, must return promptly, does not replace `on_navigation`, popups never reported. |
| `on_console_message` | `fn on_console_message<F: Fn(ConsoleMessage) + Send + Sync + 'static>(self, handler: F) -> Self;` | Stores `Arc::new(handler)` in `console_message_handler`. Doc: works without DevTools open, does not suppress CEF's own logging, popups and DevTools windows excluded. |
| `allow_chrome_commands` | `fn allow_chrome_commands<I: IntoIterator<Item = ChromeCommandGroup>>(self, groups: I) -> Self;` | Collects the iterator into a `Vec` and **replaces** `allowed_chrome_commands` (calling twice replaces the previous list). Doc lists what is swallowed by default and that DevTools/zoom accelerators are not covered. Carries an example. |
| `with_browser_settings` | `fn with_browser_settings<F: Fn(&mut cef::BrowserSettings) + Send + Sync + 'static>(self, callback: F) -> Self;` | Stores `Arc::new(callback)` in `browser_settings_callback`; runs after the portable mapping so it can override decisions and reach fields Tauri has no attribute for (font families/sizes, `remote_fonts`, `local_storage`, `databases`, `webgl`, `tab_to_links`, `javascript_dom_paste`, `default_encoding`). Carries an example. |

Implementation (`358-410`) for
`tauri::WebviewWindowBuilder<'a, R, M> where R::RuntimeWebviewAttributes: AsCefWebviewAttributes`:
each method calls `with_cef_webview_attributes(self.runtime_specific_attributes_mut(), |attributes| …)`
and returns `self`. `allow_chrome_commands` clones the collected `Vec` into the attribute
(`attributes.allowed_chrome_commands = groups.clone();`), and `with_browser_settings` clones the
`Arc` (`browser_settings_callback = Some(callback.clone())`) — both harmless redundancies to
mirror or drop, but the observable behaviour is identical.

### 4.3 `WebviewBuilderCefExt` (`tauri_ext.rs:412-524`, `#[cfg(feature = "unstable")]`)

Identical five methods with identical signatures, implemented for
`tauri::webview::WebviewBuilder<R> where R::RuntimeWebviewAttributes: AsCefWebviewAttributes`,
with the same storage behaviour. The doc text is the same minus the two runnable examples.
The whole trait and impl are behind `#[cfg(feature = "unstable")]`; the target crate must gate
them the same way.

### 4.4 Supporting items

| line | item | behaviour |
| --- | --- | --- |
| 21 | `type Result<T> = std::result::Result<T, tauri::Error>;` | Private alias; do not export. |
| 23–28 | `fn not_cef() -> tauri::Error` | `tauri_runtime::Error::RuntimeTypeMismatch("the application is not running on the CEF runtime".into()).into()`. The Electron port must produce the same error kind with an Electron-worded message. |
| 99–103 | `impl AsCefWebviewAttributes for DynWebviewAttributes` | Uses `get_or_default()`, i.e. it **inserts** CEF attributes into an empty slot instead of failing. The Electron port must keep this asymmetry: `DynWebviewAttributes` gets default attributes, a concrete foreign `RuntimeWebviewAttributes` does not. |
| 105–116 | `fn with_cef_webview_attributes<A: AsCefWebviewAttributes>(attributes: &mut A, f: impl FnOnce(&mut CefWebviewAttributes))` | Applies `f` when the attributes are CEF's; otherwise `log::warn!("ignoring the CEF webview attributes: attributes of another runtime were already set on the webview builder")`. This is why every builder method is infallible and returns `self` unchanged on mismatch. |

---

## 5. Behavioural details that matter for parity

### 5.1 How initial scripts are registered

1. **Collection** — `initialization_scripts(&mut attrs)` (`1595`):
   1. if `attrs.drag_drop_handler_enabled`, push `CefInitScript::new(browser_client::drag_drop_initialization_script())` **first**;
   2. then `std::mem::take(&mut attrs.initialization_scripts)`, each wrapped by `CefInitScript::new`;
   3. returns `Arc<Vec<CefInitScript>>`. The attribute is drained: a second call sees no app scripts.
2. **DevTools shortcut injection** — before collection, in `build_browser_child` (`723-733`):
   when `devtools_enabled` **and** (`offscreen.is_some()` **or** `is_alloy_style(runtime_style)`),
   `tauri_runtime::webview::devtools_shortcut_script()` is pushed onto
   `pending.webview_attributes.initialization_scripts` with `for_main_frame_only: true`.
   A Chrome-style browser must **not** get it: it dispatches `IDC_DEV_TOOLS` for the same chord
   and the toggle would close the window the accelerator just opened.
3. **Hash** — `CefInitScript::new` computes
   `hash = format!("'sha256-{}'", base64_standard(sha256(normalize_script_for_csp(script))))`.
   `normalize_script_for_csp` is `tauri_utils::html::normalize_script_for_csp`.
4. **Delivery path A — CDP document-start, for every navigation.**
   `devtools_initialization_script_source` (`2328`) builds **one** source string:

   ```js
   {
     const __TAURI_CEF_INIT_CUSTOM_PROTOCOL__ = "<scheme>:";
     const __TAURI_CEF_INIT_CUSTOM_DOMAINS__ = new Set(["<scheme>.localhost", …]);
     const __TAURI_CEF_INIT_IS_CUSTOM_PROTOCOL__ =
       location.protocol === __TAURI_CEF_INIT_CUSTOM_PROTOCOL__
       && __TAURI_CEF_INIT_CUSTOM_DOMAINS__.has(location.hostname);
     const __TAURI_CEF_INIT_IS_MAIN_FRAME__ = (() => { try { return window.top === window; } catch (_) { return false; } })();
     if (!__TAURI_CEF_INIT_IS_CUSTOM_PROTOCOL__[ && __TAURI_CEF_INIT_IS_MAIN_FRAME__]) {
       <script>
     }
     …
   }
   ```

   The `&& __TAURI_CEF_INIT_IS_MAIN_FRAME__` clause is present only for
   `for_main_frame_only` scripts. Returns `None` when the list is empty or JSON encoding fails.
   `register_initialization_scripts` (`2370`) then allocates a **runtime-reserved** CDP id, sends
   `{"id":<id>,"method":"Page.addScriptToEvaluateOnNewDocument","params":{"source":<source>}}`,
   and inserts `(browser, initial_url)` into `pending_initial_loads` under that id.
5. **Delivery path B — custom-protocol HTML injection.** Documents served by a registered URI
   scheme (`http(s)://<scheme>.localhost`) do not reliably receive the CDP hook, so
   `cef_impl/request_handler.rs` injects the `runs_in_frame(is_main_frame)`-filtered scripts into
   the HTML body and adds their `hash` values to the document CSP. This is the reason for the
   protocol guard in path A and for `hash` existing at all.
6. **Deferred first navigation** — `load_initial_url_after_registering_initialization_scripts`
   (`2437`):
   - `Ok(false)` (nothing to register, or the send failed) → `post_load_initial_url` immediately;
   - `Ok(true)` → **wait**; `TauriDevToolsProtocolObserver::on_dev_tools_method_result` removes
     the pending entry and calls `post_load_initial_url`;
   - `Err(DevToolsMessageIdExhausted)` → `log::error!("CEF initialization failed: {error}")` and
     `host.close_browser(1)`. The webview must never navigate without the requested
     document-start scripts or reuse another operation's acknowledgement.
   - `post_load_initial_url` posts a `LoadInitialUrlTask` to `TID_UI`; the real load is **never**
     performed inline (that would re-enter navigation from the creation/CDP stack).
7. **Placeholder first document** — the browser is created at `INITIAL_LOAD_URL`
   (`2121`), a `data:text/html` document whose `<html>`/`<body>` carry
   `data-tauri-cef-internal="initial-load"` and whose title is "Tauri CEF Initial Load". It
   exists only so a `BrowserHost` exists before the CDP registration. Consequence: the app's
   first screen is history entry #2, which is why `ChromeCommandGroup::History` is blocked by
   default and why the page context menu drops Back/Forward.

### 5.2 How `eval_script_with_callback` reports results

`handle_window_independent_webview_message` → `WebviewMessage::EvaluateScriptWithCallback`
(`1233-1271`):

1. `crate::devtools::allocate_runtime_devtools_message_id()`; on `Err`, call
   `callback(String::new())` and return. No CDP traffic.
2. Build `EvalScriptWithCallbackDevToolsObserver { message_id, callback: Arc<Mutex<Option<…>>>, registration: Arc<Mutex<Option<cef::Registration>>> }`
   and `host.add_dev_tools_message_observer(&mut observer)`.
   - registration `None` → `callback(String::new())`.
3. Store the registration, then send
   `{"id":<id>,"method":"Runtime.evaluate","params":{"expression":<script>,"returnByValue":true}}`.
4. `host.send_dev_tools_message(...) != 1` → drop the registration, `callback(String::new())`.
5. `on_dev_tools_method_result` (`2250`): return unless `message_id == self.message_id`; take the
   callback (so it fires **at most once**); compute the result; invoke; drop the registration
   (unregisters the observer).
6. Result conversion — `runtime_evaluate_result_to_json` (`2221`): `None` → `""`; invalid JSON →
   `""`; `exceptionDetails` present → `""`; otherwise take `result` (falling back to the whole
   value) and `serde_json::to_string` its `value` member; missing `value` → `""`. When
   `success == 0` the result is `""` without parsing.

Consequences to preserve:

- The callback runs **on the CEF UI thread**, not on the caller's thread.
- `WebviewDispatch::eval_script_with_callback` itself only queues the message and returns
  `Ok(())`; it never runs the callback synchronously.
- The callback is never called if the browser dies before the result arrives.
- A thrown exception is reported as `""` — matching `tauri-runtime-wry`'s documented Windows
  limitation, not as an error.
- Plain `eval_script` (`1226`) uses `frame.execute_java_script(script, "", 0)` on the main frame:
  no result, no error surface, silent no-op when there is no main frame.

### 5.3 How cookies are filtered per URL

- Cookie manager: `AppWebview::cookie_manager()` = `host.request_context()?.cookie_manager(None)`
  (`cef_impl/cookie.rs:162`). It is the **request context's** manager, shared by every webview on
  that profile (`data_store_identifier` / `data_directory`), not per webview.
- `cookies_for_url(url)` → `manager.visit_url_cookies(url, includeHttpOnly = 1, visitor)`.
  Filtering is Chromium's URL-cookie filter (domain/path/secure/expiry), **including httpOnly**
  cookies. The visitor collects every visited cookie as `(name, value, domain, path, secure,
  http_only)` and sends the vector from its `Drop` impl — so empty store, failed visit and normal
  completion all deliver exactly one result and the dispatcher's blocking `recv` cannot hang.
- `cookies()` → `manager.visit_all_cookies(visitor)`, same `Drop` delivery.
- No cookie manager → both getters return `Ok(Vec::new())`; setters are silent no-ops.
- `set_cookie(cookie)` → `manager.set_cookie(url = child.url(), cef_cookie, None)` where
  `child.url()` is the **current main-frame URL** (`Option<String>`, `None` if there is no main
  frame). Only `name`, `value`, `domain`, `path`, `secure`, `http_only` are mapped; `same_site`,
  `expires` and `max_age` are dropped. Fire-and-forget (no callback, no error).
- `delete_cookie(cookie)` → `manager.delete_cookies(url = child.url(), name = cookie.name(), None)`.
  Only URL + name identify the cookie; domain/path are ignored.
- Parity trap: the URL is the page's, not the cookie's domain. A cookie for another domain is
  set/deleted against the wrong URL. Mirror this or document the deviation explicitly.

### 5.4 How `clear_all_browsing_data` scopes its range

`WebviewMessage::ClearAllBrowsingData` (`1311-1319`):

1. `cookie_manager().delete_cookies(None, None, None)` — every cookie in the store;
2. `cookie_manager().flush_store(None)`;
3. `host.request_context().clear_http_cache(None)`.

Scope: **this browser's request context**, i.e. every webview sharing that profile. It is not
per-webview and not per-origin. It does **not** clear local storage, IndexedDB, service workers,
cache storage or the HTTP auth cache — a documented gap relative to the method name that the
Electron port should either reproduce exactly or call out. Nothing is awaited; the dispatcher
returns `Ok(())` when the message is queued.

### 5.5 How DevTools messages are allocated and id-matched

`devtools.rs` (out of scope but the contract these files rely on):

- Two process-global `AtomicI32` counters. Callers: `1 ..= 999_999_999`
  (`allocate_devtools_message_id`, public). Runtime: `1_000_000_000 ..= i32::MAX`
  (`allocate_runtime_devtools_message_id`, `pub(crate)`).
- Both fail closed with `DevToolsMessageIdExhausted` at their own boundary; ids are never reused,
  not even after cancellation or browser teardown, and neither range can reach the other.
- Every runtime-internal request (`Page.enable`, `Page.addScriptToEvaluateOnNewDocument`,
  `Runtime.evaluate`, `Emulation.setUserAgentOverride`) uses the reserved range. Therefore:
  - `pending_initial_loads` and the eval callbacks only ever match runtime requests;
  - an app that hardcodes an id cannot consume an internal result;
  - a caller cannot answer an internal request by picking its number.
- Public contract: `DevToolsProtocol::MethodResult.message_id` correlates with an id from
  `allocate_devtools_message_id`; numeric correlation authorizes nothing (no browser, no
  document).

### 5.6 How the DevTools observer is registered and fanned out

- One `TauriDevToolsProtocolObserver` per native browser, registered with
  `host.add_dev_tools_message_observer`, kept alive by the `cef::Registration` in
  `AppWebview::devtools_observer_registration` (`Arc<Mutex<Option<cef::Registration>>>`).
  Dropping the registration unregisters the observer.
- Registration is **eager**, at browser creation (`939-944`), so dialog observation and the
  initial-load handshake work even when the app registers no observer. `add_dev_tools_observer`
  (`2311`) also sends `Page.enable` with a reserved id.
- Fan-out: `on_dev_tools_message` → `DevToolsProtocol::Message(bytes)`; `on_dev_tools_event` →
  `DevToolsProtocol::Event { method, params }`; `on_dev_tools_method_result` →
  `DevToolsProtocol::MethodResult { message_id, success: success != 0, result }`. Each is
  delivered to every handler in `handlers: Arc<Mutex<Vec<Arc<DevToolsProtocolHandler>>>>`, cloned
  per handler.
- Extra duties: a method result first resolves `pending_initial_loads` (deferred first
  navigation); an event first feeds `dialogs.on_event(method, params)` **only when**
  `dialogs.accepts_browser(browser.identifier())`.
- `on_dev_tools_protocol` (`1373-1400`) pushes the handler, then registers a **new** observer only
  if the registration slot is `None` (i.e. the eager registration failed); that path also sends
  `Page.enable` and returns `Err(Error::FailedToSendMessage)` if registration fails. Otherwise it
  returns `Ok(())` immediately.
- Popups get their own observer with an **empty** handler list (`popup.rs:143`): dialog
  observation only, never the opener's app callbacks. A `DevToolsProtocol` notification carries
  page content, network activity and dialog messages with no browser identity to separate them,
  and a popup navigates wherever its own content goes (SSO/OAuth), so this isolation is
  deliberate.
- User agent override (`2293`) is a separate one-shot CDP request
  (`Emulation.setUserAgentOverride`) sent **before** the initial navigation so the first request
  already carries it; client hints are deliberately left alone. A popup keeps the process-wide
  `Cef::user_agent`.

### 5.7 How frame navigation state is tracked

State (`frame_navigation.rs:15-24`): `browser_id: Option<i32>`, `generation: u64`,
`exhausted: bool`, `loading: bool`, `observed_load_state: bool`, `main_frame: Option<String>`,
`frames: BTreeMap<String, bool>` (frame id → attached).

`apply()` (`174-250`), in order:

1. lock failure or `exhausted` → ignore the event.
2. `browser_id` already set and different → ignore. A runtime-managed popup can inherit an
   opener's handlers; its identity never advances or grants authority over the opener.
3. Bind `state.browser_id = Some(browser_id)` **before** any rejection below, so IPC admission and
   event delivery are always gated on a recorded identity.
4. Reject and latch `exhausted = true` when the frame id is empty and the event is not
   browser-wide (`MainFrameChanged`, `LoadingStateChanged`, `RendererTerminated`), or when
   `frame_id.len() > MAX_NATIVE_FRAME_ID_BYTES` (512). A warning names the reason.
5. `generation = generation.checked_add(1)`; overflow latches `exhausted`.
6. Transition per kind:
   - `Created` → `frames.insert(id, false)`
   - `Attached` → `frames.insert(id, true)`
   - `Detached | Destroyed` → `frames.remove(id)`; clear `main_frame` if it was this frame
   - `MainFrameChanged` → `main_frame = is_main.then(|| id.to_string())`
   - `NavigationStarted` → `loading = true`
   - `LoadingStateChanged { is_loading }` → `observed_load_state = true; loading = is_loading`
   - `DocumentCommitted | NavigationFailed | AddressChanged` → no state change (they still bump
     the generation, which revokes older tokens)
   - `RendererTerminated` → `frames.clear(); main_frame = None; loading = true; observed_load_state = false`

`ready(state)` (`149`): `!exhausted && observed_load_state && !loading && !frames.is_empty()
&& frames.len() <= MAX_NATIVE_FRAMES (256) && all attached && main_frame is Some and attached`.
Exceeding the cap fails closed while it lasts but keeps tracking exactly, so draining below the
cap recovers.

`document()` (`120`): `ready_generation().map(|g| NativeDocumentToken { state, generation: g })`
— a pure state read, no CEF calls.

`observe_document(browser)` (`84`): `ready_generation()?`; reject `browser.is_valid() == 0` or
`browser.frame_count() > MAX_NATIVE_FRAMES`; build the attached set from
`browser.frame_identifiers()` filtered by `browser.frame_by_identifier(id).is_valid() != 0`
(CEF keeps listing frames parked in the back/forward cache, detached from any renderer — Chrome
style parks the internal initial document there on the first navigation, and tracking drops
frames on detach); then `admits_native_frames(generation, browser.identifier(), browser.is_loading(), &attached)`
→ token.

`admits_native_frames` (`254`): `ready(&state) && state.generation == expected
&& state.browser_id == Some(browser_id) && !is_loading && state.frames.keys().eq(frame_ids.iter())`
— exact identity comparison, so an equal frame **count** with different ids is rejected.

`close()` latches `exhausted` (teardown, popup revocation). `has_browser_id(id)` gates IPC
admission and app event delivery.

Invariants pinned by the tests (`272-462`):

1. child navigation, frame replacement and detach revoke prior documents;
2. a cancelled navigation waits for native loading completion;
3. a foreign browser, unobserved frames and exhaustion all fail closed;
4. a missing main frame and renderer termination revoke prior generations;
5. tokens cannot cross native lifetimes or navigation;
6. a rejected first event still records the native browser identity;
7. a transient frame-count overflow recovers once frames drain.

### 5.8 How frame events reach the app

- Producer: `emit_frame_event` (`frame.rs:61`) — `browser_id = browser.identifier()`,
  `frame_id = frame.identifier()` (empty when no frame), `is_main = frame.is_main() != 0`. No-op
  when either the handler or the browser is absent.
- Wiring in `build_browser_child` (`767-780`): the client's `frame_event_handler` is **always**
  `Some(...)`. It first calls `frame_navigation_state.on_frame_event(&event)` (internal, sees
  every notification), then forwards to the app handler **only if**
  `frame_navigation_state.has_browser_id(event.browser_id)`. CEF can route a browser this webview
  does not own through the same client (a DevTools window is the standing case) and a `FrameEvent`
  carries the full URL, so those must never reach an app observer.

### 5.9 How popups and openers are represented

- `Webview::popups()` returns `child.popup_family.observe()` (`1217`), sampled in the same
  UI-thread callback. Each popup `Webview` has `window_label: None`, `parent_matches: None`
  (permanently — CEF owns the popup's native window, so there is no independently observed
  parent), its own `FrameNavigationState` and `DialogState`, and `opener` set via `set_opener`.
- The popup `window` token is minted by the family and shared by tabs of one CEF window
  (`popup.rs:255-287`), capped at `MAX_POPUPS = 128`. `bounds` are the view's rect in logical
  pixels; `visible` is `view.is_drawn() && window.is_visible() && !window.is_minimized()`.
- `Webview::opener()` returns the opener's `FrameNavigationState` (exact native lifetime), `None`
  for a runtime-owned webview.
- `for_document` (`127`) searches `self` then `popups` for a snapshot whose `document` equals the
  given token, so a caller holding a token can find the handle that owns it.
- `NewWindowOpener` (`runtime.rs:1181`) is the `Runtime::WindowOpener`:
  `pub struct NewWindowOpener { source_url: Option<url::Url> }` with
  `pub fn source_url(&self) -> Option<&url::Url>` and a `Debug` that prints only
  `source_url_observed: bool` (URLs can carry credentials). `AsCefWindowOpener` downcasts it out
  of a `DynWindowOpener`.
- App observers registered on the opener never see popup traffic: separate browser, separate
  handler list, separate `FrameNavigationState`.

### 5.10 Message routing and threading

- Every dispatcher method sends `Message::Webview { window_id, webview_id, message }` through
  `RuntimeContext::send_message`, which handles main-thread messages inline when possible and
  otherwise queues and wakes the loop.
- `handle_webview_message` (`1007`) returns immediately while `state.exiting` (avoids a macOS
  exit crash). Window-dependent variants — `SetBounds`, `SetSize`, `SetPosition`, `Position`,
  `Size`, `SetAutoResize`, `WithWebview`, `Reparent` — go to
  `handle_window_dependent_webview_message` because they read window metrics
  (`safe_surface_size`, `scale_factor`); everything else goes to
  `handle_window_independent_webview_message(child, message)`.
- `webview_getter!` (`1704`) reads `*self.window_id.lock().unwrap()` into a local **before**
  calling `getter`, because CEF page-load callbacks on the UI thread may need the same lock to
  dispatch work before servicing the getter. Holding it would deadlock. A unit test
  (`getter_tests`) pins this.
- `getter` (`1695`) sends the message and blocks on `receiver.recv()`, mapping a closed channel
  to `Error::FailedToReceiveMessage`.
- `window_id` is `Arc<Mutex<WindowId>>` because `reparent` rewrites it, and only after the UI
  thread confirms success.
- `create_webview_detached` (`1668`) allocates `context.next_webview_id()`, sends
  `Message::CreateWebview`, waits through `context.wait_for_creation` (which `try_recv`s on the
  main thread instead of blocking, to avoid deadlocking the loop that must handle the message),
  and returns a `DetachedWebview` with a fresh dispatcher.

### 5.11 Geometry and auto-resize

- `BoundsRate { x, y, width, height }` are fractions of the parent; `Default` is `(0, 0, 1, 1)`,
  i.e. full window.
- `compute_child_bounds_rate(bounds, auto_resize, parent_size, scale)` (`2088`): `bounds == None`
  → `Some(BoundsRate::default())` (a webview with no bounds fills the window); `!auto_resize` →
  `None`; else the physical position/size divided by `parent_size.max(1)`.
- `AppWebview::set_bounds(parent_size, scale, bounds)` (`545`): recompute `bounds_rate` when one
  is set, then `host.notify_move_or_resize_started()`, `resize(...)`, `host.was_resized()`.
- `resize` (`529`): offscreen views store the requested **logical** rect
  (`x/scale`, `y/scale`, `max(0)/scale`); everything else applies physical bounds.
- `layout_app_window` (`2054`): recompute every child that has a rate from the current parent
  size; children with fixed bounds keep their last bounds. On Linux the X11 host is unmapped when
  every child is offscreen (an offscreen webview paints into the toplevel itself).
- `set_auto_resize(true)` recomputes the rate from the current bounds; `false` clears it.
- `bounds()` returns the platform rect (offscreen: the requested logical rect; Windows: the
  child HWND rect mapped into the parent's client coordinates). `position()`/`size()` convert it
  to physical with the window scale factor.

### 5.12 Zoom, background, visibility, print, close, focus, reparent

- `set_zoom(scale_factor)` (`1298`): CEF's level is logarithmic, `percentage = 1.2^level`; the
  conversion is `ln(scale_factor) / ln(1.2)`, with a non-positive factor mapping to `0.0`, then
  `host.set_zoom_level(level)`.
- `set_background_color(color)` → platform `AppWebview::set_background_color` (Windows is a
  no-op with a TODO).
- `show`/`hide` → `set_visible` (`570`): `host.was_hidden(visible ? 0 : 1)`, then either the
  offscreen flag + `request_redraw()` or `apply_visible(visible)`.
- `print` → `host.print()`; `close` → `host.close_browser(0)` (not forced); `set_focus` →
  `host.set_focus(1)`.
- `reparent` (`1122-1187`): offscreen → `Err(Error::CreateWebview("offscreen webviews cannot be
  reparented"))`; same window → `Ok(())`; unknown target → `Err(Error::WindowNotFound)`; else
  leave HTML fullscreen (and exit window fullscreen if the page owned it), remove from the source
  window, `reparent`, re-apply bounds against the destination's `safe_surface_size`, raise to top
  on Windows (re-parenting does not preserve z-order), push, `Ok(())`.

### 5.13 Browser settings mapping and unsupported attributes

- `browser_settings_from_webview_attributes` (`187`):
  `javascript` = disabled iff `javascript_disabled`; `javascript_access_clipboard` = enabled iff
  `clipboard`; `background_color` = `color_to_argb(color)` when set, else `0x00_00_00_00` when
  `transparent`, else `0xFF_FF_FF_FF` (CEF reads alpha 0 as transparent painting, not "unset");
  `chrome_status_bubble` and `chrome_zoom_bubble` disabled; everything else `Default`.
- The app's `browser_settings_callback` runs **after** this mapping so it can override decisions.
- `warn_about_unsupported_attributes` (`226`) logs once per non-default unsupported attribute:
  `additional_browser_args`, `accept_first_mouse`, `browser_extensions_enabled` /
  `extensions_path`, a `background_throttling` other than `Throttle`, and (Windows only) a
  `scroll_bar_style` other than `Default`. Each message names the runtime-wide alternative.
- Attributes served elsewhere: `user_agent` → CDP `Emulation.setUserAgentOverride` (§5.6);
  `zoom_hotkeys_enabled` → the client's command handler (zoom reaches a browser through
  Chromium's accelerator table, not a browser setting); `proxy_url` → the request-context
  preference; `data_store_identifier` / `data_directory` → the request context cache path.
- `RuntimeStyle` → `cef::RuntimeStyle::{ALLOY, CHROME, DEFAULT}`; `is_alloy_style` (`1591`) also
  treats macOS as Alloy because Chrome style cannot have a native parent there.
- `webview_version()` → `crate::downgrade::ChromiumVersion::embedded().to_string()`.

### 5.14 Small contracts worth not losing

- `ConsoleMessage::from_cef` (`409`): `LOGSEVERITY_VERBOSE → Verbose`,
  `DEFAULT | INFO → Info`, `WARNING → Warning`, `ERROR → Error`, `FATAL → Fatal`, `_ → Other`;
  `message`/`source` from `Option<&CefString>` → `String` (empty when `None`); `line` verbatim.
- `AppWebview::url()` (`580`) returns `main_frame().url()`, `None` without a main frame; the
  `Url` getter maps `None` to `String::default()`.
- `ChromeCommandGroup::ALL` is the single source of truth for the blocklist.
- `CefWebviewAttributes::Debug` never formats a closure.
- `initialization_scripts` mutates the attributes (drains the scripts); the resulting
  `Arc<Vec<CefInitScript>>` is cloned into the scheme registry so custom-protocol responses can
  inject the same scripts and CSP hashes.
- `create_webview` errors with `Error::CreateWebview("window {window_id:?} does not exist")`;
  `build_and_attach_webview` errors with `Error::CreateWebview("failed to create CEF browser")`
  when the child cannot be built.
- `build_and_attach_webview` raises each new windowed child above its siblings and pins the
  z-order on Windows; an offscreen child clears the window's background surface instead.

---

## 6. Electron feasibility notes

Legend: **direct** = Electron has an equivalent API; **emulated** = achievable by combining
Electron APIs; **absent** = Electron/Chromium exposes no equivalent (the item must still exist
and report unsupported). Mechanisms are named as Electron main-process APIs plus the
`electron-rs` RPC command that carries them (`rpc::webview::*` → wire `webview.*`); the bridge's
current command set is enumerated in [`spec-electronrs.md`](spec-electronrs.md) §1.8.2 and its
event catalogue in §2.1. Where the bridge has **no** command today, the row says so — those are
new bridge work, not existing capability.

### 6.1 `Webview`, `Webview::browser`, `with_webview`

| item | verdict | mechanism |
| --- | --- | --- |
| `Webview` struct | **emulated** | A Rust handle holding `webview_id`/`webContents.id`, the sampled `WebviewSnapshot`, the live `FrameNavigationState`, the observed popup list and the opener state. There is no in-process browser object, so all fields become ids + shared state. |
| `Webview::browser()` | **emulated** | Return an opaque `ElectronWebviewHandle { web_contents_id: i32 }` that can be used with the bridge (`webview.*` methods). The CEF method's purpose — synchronously reaching the native browser API — is **absent**; document that in the method doc. |
| `with_webview` | **emulated** | Sample the snapshot over IPC — `webview.bounds` returns `rpc::webview::Snapshot { scale_factor, bounds, content_bounds, visible }` — then run the closure on the loop thread. The CEF version runs it on the CEF UI thread; Electron has no equivalent thread affinity, so document the difference. |
| `WebviewSnapshot.browser_id` | **direct** | The Electron `webContents.id`, delivered as `contentsId` by the `webview.attached` event (`spec-electronrs.md` §2.1 #18). |
| `WebviewSnapshot.document` | **emulated** | `FrameNavigationState::observe_document` fed by a frame-event stream that the bridge does not emit today (§6.4); the "native frame list" becomes `webContents.mainFrame.framesInSubtree` filtered by `frame.detached === false`. |
| `WebviewSnapshot.dialogs` | **absent** today / **emulated** with new bridge work | The bridge has no dialog event. Electron's source is CDP `Page.javascriptDialogOpening` / `Page.javascriptDialogClosed` through `webContents.debugger`, which requires new `webview.debugger.*` commands (§6.2). Until then `NativeDialogObservation` must report `known: false`. |
| `WebviewSnapshot.window_label` | **direct** | The runtime's own window→label map in `RuntimeContext` (`webview.attached` carries `windowId`). |
| `WebviewSnapshot.window` | **direct** | The runtime's own `NativeWindowToken`, minted per `BrowserWindow` lifetime (pure Rust, port as-is). |
| `WebviewSnapshot.parent_matches` | **emulated** | The bridge knows which window hosts which webview (`webview.attached`'s `windowId`, `webview::Config`'s `windowId`); compare that with the tracked window and report `Some(true/false)`. `None` only when the mapping is unknown. |
| `WebviewSnapshot.bounds` | **direct** | `webview.bounds` → `Snapshot.bounds` / `Snapshot.content_bounds` (with `scale_factor` for the logical/physical conversion). |
| `WebviewSnapshot.visible` | **direct** | `webview.bounds` → `Snapshot.visible`. |
| `Webview::popups()` / `opener()` | **emulated** | The blocking `webview.newWindow` request (with `frameName`, `features`, `disposition`, `position`, `size`) plus `webview.newWindowResponse` (`NewWindowDecision { action, window_id }`) and `window.create.newWindowRequest`: the runtime records each allowed window as a popup of the opener, with the opener's `FrameNavigationState`. `opener.source_url` comes from the request's `url`. |
| `for_document` | **direct** | Pure Rust over the snapshot list. |

### 6.2 DevTools protocol surface

| item | verdict | mechanism |
| --- | --- | --- |
| `DevToolsProtocol` enum | **direct** | `webContents.debugger.on('message', (event, method, params, sessionId))` → `Event`; the raw `Message` variant can be reconstructed by serialising `{method, params}`; `MethodResult` is synthesised from the `sendCommand` promise. |
| `send_dev_tools_message(&[u8])` | **emulated** | **No bridge command exists today**: the port must add `webview.debugger.sendCommand` (parse the caller's JSON, `debugger.sendCommand(method, params)`, then fan a synthesised `MethodResult { message_id: <caller id>, success, result }` out to the registered handlers). Electron's `Debugger` takes method+params, not an id-carrying frame, so the caller's `id` never reaches Chromium. Alternative that is **direct**: a `--remote-debugging-port` WebSocket, where raw id-carrying messages are accepted (extra transport, extra port, extra security surface). |
| `on_dev_tools_protocol` | **emulated** | **No bridge command exists today**: add `webview.debugger.attach` / `detach` plus a `webview.debugger.message` event, backed by `debugger.attach('1.3')` + `debugger.on('message')`; the registration guard detaches on drop. Note `debugger.attach` throws if DevTools is already open on that `webContents` (and DevTools opening detaches the session), which CEF does not do — surface it as `Err(Error::FailedToSendMessage)`. |
| `allocate_devtools_message_id` / `DevToolsMessageIdExhausted` | **direct** | Pure Rust; port `devtools.rs` verbatim (same two ranges, same fail-closed semantics). With the `sendCommand` path the allocator only namespaces the synthesised results, but the public contract stays identical. |
| `Page.enable`, `Page.addScriptToEvaluateOnNewDocument`, `Runtime.evaluate`, `Emulation.setUserAgentOverride` | **absent** from the bridge today | Same CDP methods would work through `webContents.debugger.sendCommand`, but each needs a new bridge command. The port should **not** need them: `webview.eval` covers script evaluation, `webview::Config.init_scripts` covers document-start scripts, and `webview::Config.user_agent` covers the user agent (§6.6). |
| `DevToolsProtocol` scoping to one browser | **direct** | A `Debugger` is per `webContents`, so an observer never sees another browser's traffic — the same isolation CEF gives per `BrowserHost`. |

### 6.3 Console messages and Chrome commands

| item | verdict | mechanism |
| --- | --- | --- |
| `ConsoleMessage` / `on_console_message` | **emulated** | **No bridge event exists today**: add a `webview.consoleMessage` event backed by `webContents.on('console-message', (event, details))` with `details.level` (`'info' \| 'warning' \| 'error' \| 'debug'`), `details.message`, `details.sourceId`, `details.lineNumber`. Map `debug → Verbose`, `info → Info`, `warning → Warning`, `error → Error`. |
| `ConsoleMessageLevel::Fatal` | **absent** | No Electron source produces it; keep the variant, never emit it. |
| `ConsoleMessageLevel::Other` | **emulated** | Emit for an unrecognised `details.level` string (forward compatibility). |
| `ChromeCommandGroup` / `allow_chrome_commands` | **emulated (mostly absent)** | Electron binds no Chrome accelerator table: new-window/tab, print, save, view-source, omnibox, history, downloads and settings commands simply do not exist, so `WindowAndTab`, `Document`, `BrowserChrome`, `BrowserSurface` and `History` have nothing to block. Keep the enum, `ALL` and the builder method for API shape; store `allowed_chrome_commands` and act on nothing. The two accelerators that *do* exist — DevTools and zoom — would need a `before-input-event` surface in the bridge and are deliberately not groups. |
| `RuntimeStyle` / `browser_runtime_style` | **absent** | Electron has no Alloy/Chrome distinction and no browser chrome. Store the value; act on nothing. Keep the type and method signature. |

### 6.4 Frame events and navigation state

| item | verdict | mechanism |
| --- | --- | --- |
| `FrameEvent.browser_id` | **direct** | The Electron `webContents.id` (`contentsId` in `webview.attached`). |
| `FrameEvent.frame_id` | **emulated** | CEF's identifier is per frame *lifetime*; Electron's `frameTreeNodeId` is per frame *tree node* and survives navigations. Use a composite `"{frameTreeNodeId}:{processId}:{routingId}"` (or `webFrameMain.fromFrameToken`'s token) so a cross-process navigation yields a new id, matching CEF. Needs the new frame events below, since the bridge sends none today. |
| `FrameEvent.is_main` | **direct** | `frame === webContents.mainFrame` (or `frame.parent === null`). |
| `FrameEventKind::Created` | **emulated** | `webContents.on('frame-created', (event, {frame}))` — needs a new `webview.frameCreated` bridge event. |
| `FrameEventKind::Attached` | **emulated** | No attach event. Emit on `frame-created` once the frame has a renderer (`frame.processId` non-zero) and on `did-frame-navigate`/`dom-ready` for reattachment. |
| `FrameEventKind::Detached` | **emulated** | No detach event. Detect by diffing `mainFrame.framesInSubtree` after each navigation/`frame-created`, or by observing `frame.detached` on the retained handle. |
| `FrameEventKind::Destroyed` | **emulated** | Same diff, emitted when the frame disappears from the subtree; also on `webContents` destruction for the main frame. |
| `FrameEventKind::NavigationStarted` | **emulated** | `did-start-navigation` with `details.isSameDocument === false`; `details.frame` supplies the identity. The bridge currently reports only main-frame `webview.loadStarted` (no `isSameDocument`, no child frames), so a new per-frame event is required for parity. |
| `FrameEventKind::DocumentCommitted` | **emulated** | `did-frame-navigate` (any frame; `isMainFrame` flag) / `did-navigate` for the main frame; the bridge's `webview.navigationCommitted` covers only the main frame. |
| `FrameEventKind::NavigationFailed` | **emulated** | `did-fail-load` (`isMainFrame`, `errorCode`, `validatedURL`); the bridge's `webview.loadFailed` covers main-frame failures only and suppresses `ERR_ABORTED`. |
| `FrameEventKind::AddressChanged` | **emulated** | `did-navigate-in-page` for same-document changes, plus `did-navigate` for full navigations; no bridge event today. |
| `FrameEventKind::MainFrameChanged` | **emulated** | No direct event. Emit when `webContents.mainFrame`'s composite identity changes (after `did-navigate` with a new `processId`/`routingId`, or after `render-process-gone` + reload), and once at startup. |
| `FrameEventKind::LoadingStateChanged` | **emulated** | `did-start-loading` / `did-stop-loading` (browser-wide, matching the CEF doc); the bridge's `webview.loadStarted`/`webview.loadFinished` are the closest existing pair but are per-navigation, not per-load-state. |
| `FrameEventKind::RendererTerminated` | **direct** | `render-process-gone` → the bridge's `webview.processGone { reason, exitCode }`. |
| `emit_frame_event` | **emulated** | Becomes a Rust helper over the IPC event payload instead of `cef::Browser`/`cef::Frame`. |
| `FrameNavigationState`, `NativeDocumentToken` | **direct** | Pure Rust state machine; port verbatim. Only `observe_document`'s native calls change: `browser.identifier()` → `webContents.id`, `frame_count()`/`frame_identifiers()`/`frame_by_identifier()` → `mainFrame.framesInSubtree` + `frame.detached`, `is_loading()` → `webContents.isLoading()`. |
| `FrameEventHandler`, `ElectronWebviewAttributes::frame_event_handler` | **direct** | Host-side callbacks fed by the IPC event stream; same `Arc<dyn Fn(FrameEvent)>` shape. |

### 6.5 `WebviewDispatch` members

| member | verdict | mechanism |
| --- | --- | --- |
| `run_on_main_thread` | **direct** | The target's `Message::Task` (already implemented). |
| `navigate` | **direct** | `webContents.loadURL(url)` (`webview.navigate`). |
| `reload` | **direct** | `webContents.reload()` (`webview.reload`). |
| `can_go_back` / `go_back` / `can_go_forward` / `go_forward` | **direct** | `webContents.navigationHistory.{canGoBack,goBack,canGoForward,goForward}` (the `webContents.canGoBack()` family is deprecated). |
| `url` | **direct** | `webContents.getURL()`. |
| `print` | **direct** | `webContents.print({ silent: false })` (`webview.print`). |
| `close` | **direct** | `webContents.close()` for a window-hosted page; element removal for a `<webview>` guest (`webview.close`). |
| `set_focus` | **direct** | `webContents.focus()` (`webview.focus`). |
| `show` / `hide` | **direct** | `webview.visibility` (`SetVisibility { visible }`). |
| `bounds` / `position` / `size` | **direct** | `webview.bounds` → `Snapshot { scale_factor, bounds, content_bounds, visible }`; convert with `scale_factor` for `position()`/`size()`. |
| `set_bounds` / `set_size` / `set_position` | **direct** | `webview.setBounds` with `BoundsInput { position?, size?, physical? }`; size/position read `webview.bounds` first. |
| `set_auto_resize` | **direct** | `webview.setAutoResize` plus the runtime's own `BoundsRate` + `layout_app_window` recomputation on window resize. |
| `eval_script` | **direct** | `webview.eval` (`Eval { script }`) → `webContents.executeJavaScript(script, true)`. |
| `eval_script_with_callback` | **direct** | `webview.eval` returns `serde_json::Value`; serialise it to a JSON string for the callback and map `null`/`undefined` to `""` so the observable result matches `runtime_evaluate_result_to_json`. |
| `reparent` | **emulated** | The bridge declares `webview.reparent { windowId }`; the JS side must move (or recreate) the guest in the destination window. Where that is impossible for a `<webview>` guest, report unsupported (`crate::unsupported::report("Webview::reparent")`) as the target does today. |
| `cookies_for_url` | **direct** | `webview.cookies { url? }` — Chromium's URL filter, httpOnly included. |
| `cookies` | **direct** | `webview.cookies` without `url`. |
| `set_cookie` | **direct** | `webview.setCookie` (`CookieInit { url, name, value, domain?, path?, secure?, http_only?, same_site?, expiration_date? }`). Electron **requires** a `url`; keep the CEF behaviour of deriving it from the current page URL and document that `domain`/`path` must be consistent with it. |
| `delete_cookie` | **direct** | `webview.deleteCookie { url, name }` — URL + name only, matching CEF. |
| `clear_all_browsing_data` | **direct** | `webview.clearBrowsingData` backed by `session.clearStorageData({ storages: ['cookies'] })` + `session.flushStorageData()` + `session.clearCache()`, which mirrors CEF's request-context scope exactly. `session.clearData({ dataTypes })` (Electron ≥ 32) is broader and is **not** parity. |
| `set_zoom` | **direct** | `webview.setZoom { scaleFactor }` → `webContents.setZoomFactor(factor)` — linear, so the `ln/ln(1.2)` conversion is dropped. |
| `set_background_color` | **direct** | `webview.backgroundColor { color: Option<String> }` (explicit `null` for "unset"); note the bridge's webview variant takes a CSS string while the window variant takes `Option<Color>` (`spec-electronrs.md` D2). |
| `open_devtools` / `close_devtools` / `is_devtools_open` | **direct** | `webview.openDevtools` / `closeDevtools` / `isDevtoolsOpen`; gate with `webview::Config.devtools` and the runtime's `devtools_allowed`. |
| `send_dev_tools_message` / `on_dev_tools_protocol` (inherent) | see §6.2 | New `webview.debugger.*` commands over `webContents.debugger`. |

### 6.6 Attributes, settings and the rest

| item | verdict | mechanism |
| --- | --- | --- |
| `ElectronWebviewAttributes` (6 fields) | **emulated** | Same field names and types; `offscreen` → **absent** (§ below), `runtime_style` → no-op, `frame_event_handler` / `console_message_handler` → direct, `allowed_chrome_commands` → no-op, `browser_settings_callback` → emulated over an `ElectronBrowserSettings` struct that maps onto `rpc::webview::Config`'s typed fields. |
| `ElectronWebviewAttributes::offscreen` / `Offscreen` / `OffscreenView` / `CursorHandler` | **absent** | Electron has no windowless browser: `webContents.startPainting`/`capturePage`/`beginFrameSubscription` capture an existing view, they do not replace the render target. Keep the type (it is public API) and reject the configuration with a warning, or omit the field only if the whole API shape is documented as reduced. |
| `BrowserSettingsCallback` / `with_browser_settings` | **emulated** | `ElectronBrowserSettings` mirrors the `BrowserSettings` fields the bridge can honour — `javascript_disabled`, `background_color`, `transparent`, `accept_first_mouse`, `devtools`, `background_throttling`, plus `webview::Config`'s `additional` flatten map for anything else — and is sent as part of `webview.create`. Fields with no `webview::Config` counterpart (`remote_fonts`, `local_storage`, `databases`, `tab_to_links`, `javascript_dom_paste`, `default_encoding`, font families/sizes) must be documented as ignored. |
| `ElectronInitScript` / `initialization_scripts` | **direct** | `rpc::webview::InitScript { Source(String) \| Detailed { script, main_frame_only } }` is the same shape as `CefInitScript` (script + `for_main_frame_only`), sent as `webview::Config.init_scripts` at `webview.create`. The JS side installs them before page scripts (preload/`webFrame.executeJavaScript`), so the CEF deferred-first-navigation dance is not needed. |
| `ElectronInitScript::hash` | **absent** | The CSP hash exists because CEF injects scripts into custom-scheme HTML. Electron's `protocol.handle` responses can be rewritten the same way, but if scripts are delivered through `init_scripts` no CSP hash is needed. Keep the field (it is private) and compute it identically for shape parity, or drop it — no public API changes either way. |
| `INITIAL_LOAD_URL` | **absent** | Not needed: `webview.create` takes the real `url` together with `init_scripts`, so there is no placeholder document and no second history entry. (Consequence: `ChromeCommandGroup::History` has no reason to exist in Electron, and the "back lands on a blank page" caveat in the `CefWebviewAttributes` doc does not apply.) |
| `RuntimeStyle` (kept) | **absent** | See §6.3. |
| `warn_about_unsupported_attributes` | **mixed** | `javascript_disabled`, `devtools`, `background_throttling`, `accept_first_mouse`, `transparent`, `background_color`, `user_agent`, `data_directory` are **direct** via `webview::Config`; `additional_browser_args` is **emulated** only through the `additional` flatten map (untyped, JS must read it); `browser_extensions_enabled`/`extensions_path`, `scroll_bar_style`, `proxy_url`, `general_autofill_enabled`, `allow_link_preview` are **absent** (no bridge command). The warning list must shrink accordingly, but the function stays because several attributes still have no per-webview mapping. |
| `data_store_identifier` / `data_directory` | **direct** | `webview::Config.data_directory` (+ `incognito`), implemented as `session.fromPartition('persist:<id>')` per webview. |
| `proxy_url` | **absent** | No bridge command; would need a `session.setProxy` RPC. |
| `user_agent` | **direct** at creation | `webview::Config.user_agent`. Changing it after creation needs a new command (`webContents.setUserAgent`); the reference's CDP override is not available. |
| `zoom_hotkeys_enabled` | **emulated** | Not in `webview::Config`; needs a `before-input-event` surface in the bridge (or JS-side handling) to intercept Ctrl+±/0. |
| `AppWebview` | **emulated** | Replace `cef::Browser`/`cef::BrowserHost` with `web_contents_id` + the RPC handle; keep `frame_navigation_state`, `dialogs`, `bounds_rate`, `page_fullscreen`, `label`, `webview_id`; drop `uri_scheme_protocols` / `devtools_observer_registration` to the main process (or keep them as the Rust-side registry if protocols stay Rust-driven). |
| `PageFullscreen` | **emulated** | `webContents.on('enter-html-full-screen'/'leave-html-full-screen')` behind a new bridge event; the window-fullscreen bookkeeping (`owns_window`) is runtime-side and ports as-is. |
| `layout_app_window` / `compute_child_bounds_rate` / `BoundsRate` | **direct** | Pure Rust; only the platform `resize` becomes a `webview.setBounds` RPC call. |
| `webview_getter!` / `getter` | **direct** | Pure Rust; keep the "drop the window-id lock before blocking" discipline (an RPC call is also a blocking wait). |
| `create_webview_detached` | **direct** | Same message + `wait_for_creation` shape; the creation result comes from `webview.create` → `webview::Created { webview_id }`. |
| `webview_version()` | **emulated** | The `ready` event carries only `{ electron, token }` (`spec-electronrs.md` §2.1 #1); the bridge must add `process.versions.chrome` (or the runtime reports the Electron version and documents the difference). |
| `ElectronWebviewDispatcher` | **direct** | Rename and keep `{ window_id: Arc<Mutex<WindowId>>, webview_id: u32, context: RuntimeContext }`. |
| `AsElectronWebviewDispatcher` / `AsElectronWindowOpener` / `AsElectronWebviewAttributes` | **direct** | Pure Rust downcasts over `tauri_runtime::dynamic`; port verbatim. |
| `WebviewElectronExt` / `WebviewWindowBuilderElectronExt` / `WebviewBuilderElectronExt` | **direct** | Pure Rust; only `with_browser_settings`'s parameter type changes (`&mut ElectronBrowserSettings`), and `with_electron_webview` passes the Electron `Webview` handle. |

### 6.7 Cross-cutting Electron mechanisms the port depends on

1. **`webContents.debugger`** — the only transport for CDP: `attach(protocolVersion)`,
   `sendCommand(method, commandParams, sessionId) -> Promise`, `isAttached()`, `detach()`, and
   the `message` / `detach` events. Per `webContents`, so it gives the same per-browser isolation
   CEF's `add_dev_tools_message_observer` gives. **The bridge exposes none of it today**
   (`spec-electronrs.md` §1.8.2), so `send_dev_tools_message`/`on_dev_tools_protocol` and the
   dialog observation need new `webview.debugger.*` commands. Caveat: attaching fails while
   DevTools is open on the same `webContents`, and DevTools opening detaches the session — the
   port must re-attach or report the failure.
2. **`webview::Config`** — the creation-time analogue of `BrowserSettings` + the webview
   attributes: `init_scripts`, `preload_scripts`, `user_agent`, `data_directory`, `incognito`,
   `auto_resize`, `bounds`, `policy`, `background_color`, `transparent`, `accept_first_mouse`,
   `javascript_disabled`, `devtools`, `background_throttling`, plus a `#[serde(flatten)]`
   `additional` map. This is why the port does **not** need the CEF init-script/placeholder-URL
   machinery.
3. **`webContents.on('before-input-event')`** — the only way to emulate Chrome's accelerator
   table: swallow or synthesise the chords a `ChromeCommandGroup` would cover, and implement
   `zoom_hotkeys_enabled`. No bridge command today.
4. **`session`** — the request-context analogue: cookies (`webview.cookies`/`setCookie`/
   `deleteCookie`), cache + storage data (`webview.clearBrowsingData`), partitions
   (`data_directory`). Proxy and extensions have no bridge command.
5. **`webFrameMain`** — the frame analogue: `frameTreeNodeId`, `processId`, `routingId`,
   `frameToken`, `parent`, `top`, `frames`, `framesInSubtree`, `detached`, `url`, `isDestroyed()`.
6. **`webContents` events** — the `FrameEvent` source: `frame-created`, `did-start-navigation`,
   `did-frame-navigate`, `did-navigate`, `did-navigate-in-page`, `did-fail-load`,
   `did-start-loading`, `did-stop-loading`, `render-process-gone`,
   `enter-html-full-screen` / `leave-html-full-screen`, `console-message`, and the blocking
   `setWindowOpenHandler`. The bridge forwards only `webview.loadStarted`, `loadFinished`,
   `loadFailed`, `navigationCommitted`, `titleChanged`, `processGone`, `closed`, `attached` and
   the `webview.newWindow` blocking request; everything else is new bridge work.

---

## Appendix A — implementation checklist

1. Add `src/frame.rs` and `src/frame_navigation.rs` with the exact contents of §1.2/§1.3,
   including the six `frame_navigation` tests and the `frame` doc comments.
2. Rewrite `src/webview.rs` to the inventory in §1.1: public types in §2, `WebviewDispatch` in
   §3, behaviour in §5. Keep every doc comment that states a contract (the `CefWebviewAttributes`
   doc block, `WebviewSnapshot` field docs, `ChromeCommandGroup` variant docs, `DevToolsProtocol`
   docs) with CEF-specific claims corrected.
3. Rewrite `src/tauri_ext.rs` to §4, including the `unstable` gate on `WebviewBuilderElectronExt`
   and the `DynWebviewAttributes::get_or_default()` asymmetry.
4. Re-export from `lib.rs` exactly as §0.1.
5. Keep `devtools.rs` allocation semantics (§5.5) — the two ranges and fail-closed behaviour are
   public API.
6. Extend the `electron-rs` bridge with the commands/events the feasibility tables name that do
   not exist yet (`spec-electronrs.md` §1.8.2 / §2.1): `webview.debugger.attach` /
   `detach` / `sendCommand` + a `webview.debugger.message` event, a `webview.consoleMessage`
   event, per-frame events (`webview.frameCreated`, `frameDetached`, `frameNavigation`,
   `frameLoadState`, `frameAddressChanged`), `webview.setUserAgent`,
   `webview.setBackgroundThrottling`, `webview.setZoomHotkeys`, and the Chromium version in
   `ready`. `webview.bounds`/`setBounds`/`visibility`/`setAutoResize`/`eval`/`cookies`/
   `setCookie`/`deleteCookie`/`clearBrowsingData`/`print`/`close`/`focus`/`setZoom`/
   `openDevtools`/`closeDevtools`/`isDevtoolsOpen`/`reparent`/`newWindowResponse` already exist.
7. Report every `absent` item through `crate::unsupported::report` with a CEF-parity message, and
   never silently succeed on one.

## Appendix B — open questions for the implementer

1. **`send_dev_tools_message` transport.** `webContents.debugger.sendCommand` cannot carry a
   caller-chosen `id`; synthesising `MethodResult` keeps the public API but changes the wire
   behaviour (the caller's id never reaches Chromium). The alternative — a
   `--remote-debugging-port` WebSocket — is closer to CEF but adds a listening port. Decide and
   document.
2. **`Webview::browser()`.** Returning an id handle preserves the signature but not the CEF
   method's promise ("from the browser you can reach the rest of the CEF API"). Either document
   the reduction in the method doc or return an error/`Option`.
3. **`offscreen`.** Electron has no windowless browser. Keeping the field with an
   unsupported-on-use path preserves the API shape; dropping it reduces the shape. The goal
   statement says 1:1 shape, so keep it and report.
4. **`FrameEventKind::Attached` / `Detached` / `Destroyed`.** Electron exposes no attach/detach
   notifications; the port must synthesise them from frame-list diffs. The exact emission points
   determine whether `FrameNavigationState` ever reaches `ready()` — this is the highest-risk
   item in the port and needs a test harness before the rest of the frame logic is trusted.
5. **`clear_all_browsing_data` breadth.** CEF clears cookies + HTTP cache only. Electron's
   `session.clearData()` would clear far more. Keep CEF's narrower behaviour for parity, or widen
   it and document the deviation.
6. **`CefInitScript::hash`.** Private, so it can be dropped without an API change; keeping it
   costs a SHA-256 per script and keeps the custom-protocol CSP path open.
7. **`tauri-runtime` trait member set.** The reference is built against the vendored workspace's
   `tauri-runtime ~3.0.0-alpha.5`, whose `WebviewDispatch` has 34 desktop members and **no**
   `on_webview_event`. The target's `Cargo.toml` now pins the same `3.0.0-alpha.5`, so the port
   should drop the current `src/webview.rs`'s extra `on_webview_event` member (and keep
   `with_ios_webview` `cfg`-gated exactly as the reference does). Re-check the trait against the
   resolved version before porting §3: the member list is what the impl must satisfy.
