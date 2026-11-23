# Parity spec — `window.rs`, `window_handle.rs`, `popup.rs`, `offscreen.rs`, `platform/`

Status: implementation contract for the `next` branch of `tauri-runtime-electron`.
Source of truth: `source/tauri/crates/tauri-runtime-cef/` @ `3.0.0-alpha.7`.
Target: `src/` of this crate (`tauri-runtime-electron`), Electron-backed through `electron-rs`.

This document is descriptive only. It contains no code to copy verbatim except the declarations it
quotes from the reference, which are the API shape the target must reproduce.

---

## 0. Conventions and evidence base

### 0.1 Files read in full

| Reference file | Lines | Target counterpart (mirror path) |
| --- | --- | --- |
| `src/window.rs` | 1951 | `src/window.rs` |
| `src/window_handle.rs` | 49 | `src/window_handle.rs` |
| `src/popup.rs` | 467 | `src/popup.rs` |
| `src/offscreen.rs` | 118 | `src/offscreen.rs` |
| `src/platform/mod.rs` | 54 | `src/platform/mod.rs` |
| `src/platform/windows/{mod,window,webview,event_loop,icon,monitor}.rs` | 9 / 174 / 234 / 25 / 34 / 32 | `src/platform/windows/…` |
| `src/platform/macos/{mod,application,dock,event_loop,fullscreen,monitor,progress,utils,wake,webview,window}.rs` | 22 / 275 / 94 / 99 / 180 / 35 / 138 / 77 / 122 / 128 / 349 | `src/platform/macos/…` |
| `src/platform/linux/{mod,window,webview,event_loop,monitor,taskbar,utils}.rs` | 12 / 358 / 170 / 49 / 133 / 178 / 154 | `src/platform/linux/…` |

The mirror paths are the 1:1 layout the target must reach. At the time of writing the target has
`src/platform/mod.rs` only (a `monitor_from_value` helper) and no `src/platform/{windows,macos,linux}/`
directories, so every `platform/` row in this document is a file to be created, not one to be edited.

> The target crate is being rewritten in parallel with this document. Statements about the target's
> *current* contents (§0.1, §4.6, §7.5) are a snapshot taken while reading it and may already be stale;
> the reference-side facts (every line number and declaration) are stable.

Every line number in this document is a line number in the reference file named by the enclosing
heading.

### 0.2 Naming rule

`Cef*` → `Electron*` (`CefWindowDispatcher` → `ElectronWindowDispatcher`, `CefRuntime` →
`ElectronRuntime`, `CefWebviewDispatcher` → `ElectronWebviewDispatcher`, `CefX11Host` →
`ElectronX11Host`, `CefWinitApp` → `ElectronApp`, `CefWinitApplication` → …). Names that are not
CEF-specific keep their exact spelling: `NativeWindowToken`, `AppWindow`, `AppWindowAttrs`,
`WindowMessage`, `FullscreenTarget`, `SendGtkWindow`, `SendGtkBox`, `SendRawWindowHandle`,
`SendRawDisplayHandle`, `SoftbufferWindowHandle`, `MonitorExt`, `EventLoopExt`, `Offscreen`,
`OffscreenView`, `CursorHandler`, `PopupFamily`, `PopupRequest`, `FullscreenTransition`,
`MainThreadWake`, `TrafficLightObserver`, `TaskbarIndicator`, `HostGeometry`.

CEF *type* references inside otherwise-kept names (`cef::RenderHandler`, `cef::CursorType`,
`cef::CursorInfo`, `cef::WindowInfo`, `cef::sys::cef_window_handle_t`, `Browser`, `Registration`) are
not renamable and are called out individually in §5 and §7.5.

### 0.3 Feasibility vocabulary (used in §7)

| Tag | Meaning |
| --- | --- |
| `direct` | Electron exposes an API with the same semantics; the mirror calls it (possibly through an `electron-rs` RPC). |
| `emulated` | Achievable by combining Electron APIs (or an Electron API plus a native shim on the runtime side), with a documented semantic difference. |
| `absent` | Neither Electron nor Chromium exposes an equivalent. The item keeps its name and signature for API-shape parity and becomes a documented no-op (`Ok(())`) or `Err(...)`, exactly as the reference itself does for platforms it does not support. |

`absent` items are *not* to be deleted: the goal is a 1:1 API, so the item must exist and must fail
or no-op the same way the reference's unsupported-platform arms do.

### 0.4 Electron mechanism vocabulary

`electron-rs` is pinned at `c97c052ba2f923912dbeba4b277a45d57091d958` (`Cargo.toml:20,26`); the
vendored snapshot at `.temp/electron-rs` is that exact commit. Mechanism names below are the
`electron-rs` RPC methods (`src/rpc.rs`) and the TypeScript implementation
(`src-ts/runtime/window.ts`, `src-ts/runtime/native/*.ts`) behind them, cross-checked against the
vendored Electron tree at `source/electron` (`docs/api/base-window.md`,
`shell/browser/native_window_views.cc`, `shell/browser/native_window_mac.mm`).

Window RPCs available today (`rpc.rs:359-453`): `window.create`, `window.ready`, `window.setTitle`,
`window.geometry`, `window.size`, `window.position`, `window.minSize`, `window.maxSize`,
`window.sizeConstraints`, `window.resizable`, `window.maximizable`, `window.minimizable`,
`window.closable`, `window.maximize`, `window.unmaximize`, `window.minimize`, `window.unminimize`,
`window.fullscreen` (`x`,`y` optional), `window.alwaysOnTop`, `window.cursor`,
`window.setCursorIcon`, `window.setCursorVisible`, `window.setParentWindow`, `window.setMenu`,
`window.skipTaskbar`, `window.focusable`, `window.contentProtected`, `window.center`,
`window.requestAttention`, `window.show`, `window.hide`, `window.focus`, `window.nativeHandle`,
`window.close`, `window.destroy`, `window.visibleOnAllWorkspaces`, `window.icon`, `window.enabled`,
`window.enabledQuery`, `window.simpleFullscreen`, `window.progressBar`, `window.shadow`,
`window.decorations`, `window.badgeCount`, `window.overlayIcon`, `window.backgroundColor`,
`window.backgroundMaterial`, `window.theme`, `window.ignoreCursorEvents`, `window.themeQuery`,
`window.displays`, `window.cursorPosition`, `window.currentDisplay`, `window.displayForPoint`,
`window.titleBarStyle`, `window.trafficLight`, `window.startDrag`, `window.popupMenu`.
App-level RPCs used by `platform/`: `app.setMenu`, `app.badgeLabel`, `app.focus`, `app.show`,
`app.hide`, `app.activationPolicy`, `app.dockVisibility`, `app.theme`, `shell.openExternal`.

---

## 1. Module inventory

### 1.1 `window.rs`

Imports (lines 5–59): `std::{sync::{Arc, Mutex, mpsc::{self, Receiver, Sender}}, time::{Duration, Instant}}`,
`cef::ImplBrowserHost`, `raw_window_handle::HasWindowHandle`, `tauri_runtime::{Error, Icon,
ProgressBarState, Result, UserAttentionType, UserEvent, WindowDispatch, dpi::{…}, monitor::Monitor,
webview::{DetachedWebview, PendingWebview}, window::{CursorIcon, DetachedWindow, DetachedWindowWebview,
PendingWindow, RawWindow, WindowId, WindowSizeConstraints}}`, `tauri_utils::{Theme, config::Color}`,
`winit::{event_loop::ActiveEventLoop, monitor::{Fullscreen, MonitorHandle}, window::{Window as
WinitWindow, WindowAttributes, WindowLevel}}`, `crate::platform::{EventLoopExt, MonitorExt}`,
`std::marker::PhantomData` (desktop cfg), `winit::platform::macos::WindowExtMacOS` (macOS),
`winit::platform::windows::WindowExtWindows` (Windows), `crate::window_handle::SoftbufferWindowHandle`
(Windows), and `crate::{cef_impl::{client as browser_client, request_context}, runtime::{…},
webview::{…}, window_builder::WindowBuilderWrapper, window_handle::SendRawWindowHandle}`.

#### Module-level items

| Line | Declaration (verbatim, bodies collapsed) | Intent |
| --- | --- | --- |
| 68 | `pub(crate) struct SendGtkWindow(*mut std::ffi::c_void);` | `Send` wrapper so a `GtkApplicationWindow*` can cross the message channel (Linux/BSD only). |
| 77 | `unsafe impl Send for SendGtkWindow {}` | Manual `Send`; the pointer is only dereferenced on the main thread. |
| 86 | `pub(crate) struct SendGtkBox(*mut std::ffi::c_void);` | Same wrapper for the default vertical `GtkBox`. |
| 95 | `unsafe impl Send for SendGtkBox {}` | Manual `Send` for the box pointer. |
| 97 | `pub(crate) fn tauri_theme_to_winit_theme(theme: Option<Theme>) -> Option<winit::window::Theme>` | Maps Tauri theme to winit theme; any non-`Light`/`Dark` variant (e.g. `Auto`) becomes `Light`. |
| 105 | `pub(crate) fn winit_theme_to_tauri_theme(theme: winit::window::Theme) -> Theme` | Inverse mapping, total. |
| 112 | `fn tauri_resize_direction_to_winit(direction: tauri_runtime::ResizeDirection) -> winit::window::ResizeDirection` | 1:1 mapping of the eight `ResizeDirection` variants. |
| 127 | `fn calculate_window_center_position(window_size: PhysicalSize<u32>, monitor: &MonitorHandle) -> PhysicalPosition<i32>` | Centers inside the monitor **work area**, clamped at 0 so an oversized window pins to the work-area origin. |
| 138 | `fn find_monitor_for_position(monitors: impl Iterator<Item = MonitorHandle>, position: Position) -> Option<MonitorHandle>` | First monitor whose position+video-mode rectangle contains the point (half-open interval). |
| 160 | `fn clamp_surface_size(attrs: &WindowAttributes, scale_factor: f64) -> PhysicalSize<u32>` | Requested surface size (default 800×600) clamped by min/max surface size, in physical pixels. |
| 181 | `fn apply_prevent_overflow(attrs: &mut WindowAttributes, window_size: &mut PhysicalSize<u32>, monitor: &MonitorHandle, margin: Size)` | Shrinks the requested size to `work_area − margin` and writes it back into `attrs.surface_size`. |
| 201 | `fn prepare_window_attributes(event_loop: &dyn ActiveEventLoop, attrs: &mut AppWindowAttrs)` | Creation-time `center` / `prevent_overflow` resolution, including the Windows non-client frame correction via `AdjustWindowRect`. |
| 256 | `fn paired_size_constraint(width: Option<tauri_runtime::dpi::PixelUnit>, height: Option<tauri_runtime::dpi::PixelUnit>, unconstrained: u32) -> Option<Size>` | Pairs the two per-axis constraints into one `Size`; mixed logical/physical or both-`None` yields `None`. |
| 292 | `pub(crate) fn min_size_constraint(width: Option<tauri_runtime::dpi::PixelUnit>, height: Option<tauri_runtime::dpi::PixelUnit>) -> Option<Size>` | `paired_size_constraint(.., 0)`. |
| 299 | `pub(crate) fn max_size_constraint(width: Option<tauri_runtime::dpi::PixelUnit>, height: Option<tauri_runtime::dpi::PixelUnit>) -> Option<Size>` | `paired_size_constraint(.., u32::MAX)`. |
| 310 | `pub(crate) enum FullscreenTarget { CurrentMonitor, MonitorAt(PhysicalPosition<f64>) }` | A fullscreen request kept in the form it arrived, so a macOS request held during a transition can be replayed later. `#[derive(Clone, Copy, Debug)]` at 309. |
| 317 | `pub(crate) enum WindowMessage { … }` | The message set the dispatcher sends to the event loop; 77 variants — one per `WindowDispatch` member except `run_on_main_thread`, which bypasses the window channel (see §1.1.1). |
| 419 | `type SoftbufferSurface = softbuffer::Surface<SoftbufferWindowHandle, SoftbufferWindowHandle>;` (Windows only) | Alias for the window background-paint surface. |
| 424 | `pub struct NativeWindowToken(Arc<()>);` | Public opaque identity of one native window lifetime (see §3). `#[derive(Clone)]` at 423. |
| 427 | `pub(crate) fn new() -> Self` | Mints a fresh identity. |
| 432 | `impl PartialEq for NativeWindowToken` | `Arc::ptr_eq` on the inner allocation. |
| 438 | `impl Eq for NativeWindowToken {}` | Total equality. |
| 440 | `impl std::fmt::Debug for NativeWindowToken` | Prints `NativeWindowToken { .. }` via `finish_non_exhaustive()`, never the address. |
| 449 | `mod native_window_identity_tests` (`#[cfg(test)]`) | Asserts clones compare equal, distinct tokens never do, and a dropped original does not make a later token equal. |
| 466 | `const PENDING_ACTIVATION_TIMEOUT: Duration = Duration::from_secs(5);` | Upper bound on retrying the initial raise of a focused window. |
| 468 | `pub(crate) struct AppWindow { … }` | The runtime's per-window state (fields below). |
| 501 | `pub(crate) struct AppWindowAttrs { … }` | Creation attributes plus the runtime-only flags the builder cannot express (fields below). `#[derive(Clone, Debug, Default)]` at 500. |
| 537 | `impl AppWindow { … }` | Window-state helpers (methods below). |
| 763 | `impl<T: UserEvent> WinitCefApp<T> { … }` | Event-loop side: window creation, activation, host relayout, message handling. |
| 1308 | `pub struct CefWindowDispatcher<T: UserEvent> { pub(crate) window_id: WindowId, pub(crate) context: RuntimeContext<T> }` | Public, cloneable, thread-safe window handle; `#[derive(Debug, Clone)]` at 1307. |
| 1313 | `fn getter<T: UserEvent, R>(context: &RuntimeContext<T>, message: Message<T>, receiver: Receiver<Result<R>>) -> Result<R>` | Sends a message, blocks on the reply channel, maps a dropped sender to `Error::FailedToReceiveMessage`. |
| 1322 | `macro_rules! window_getter { ($self:ident, $variant:ident) => {…} }` | Builds the channel + `Message::Window` + `getter(..)` for the 25 getters that use it. |
| 1336 | `impl<T: UserEvent> WindowDispatch<T> for CefWindowDispatcher<T> { … }` | The 78-member trait implementation (see §2). |
| 1834 | `pub(crate) fn create_window_detached<T, F>(context: &RuntimeContext<T>, pending: PendingWindow<T, CefRuntime<T>>, after_window_creation: Option<F>) -> Result<DetachedWindow<T, CefRuntime<T>>> where T: UserEvent, F: Fn(RawWindow<'_>) + Send + 'static` | Allocates ids, sends `Message::CreateWindow`, waits for the event loop, returns the detached window (+ its initial webview). |
| 1893 | `pub(crate) fn winit_monitor_to_tauri_monitor(monitor: &winit::monitor::MonitorHandle) -> Monitor` | winit monitor → `Monitor` (name, scale, position, video-mode size, `MonitorExt::work_area`). |
| 1906 | `pub(crate) fn tauri_icon_to_winit_icon(icon: Icon) -> Result<winit::icon::Icon>` | RGBA `Icon` → `RgbaIcon`, mapping an invalid buffer to `Error::InvalidIcon`. |
| 1912 | `fn tauri_cursor_to_winit_cursor(cursor: CursorIcon) -> winit::cursor::CursorIcon` | Exhaustive `CursorIcon` → winit cursor mapping; `Hand`→`Grab`, `Arrow`→`Default`, unknown→`Default`. |

#### 1.1.1 `WindowMessage` variants (line: variant)

| Line | Variant | Line | Variant | Line | Variant |
| --- | --- | --- | --- | --- | --- |
| 318 | `Close` | 349* | `GtkWindow(Sender<Result<SendGtkWindow>>)` | 377 | `SetAlwaysOnTop(bool)` |
| 319 | `Destroy` | 357* | `DefaultVBox(Sender<Result<SendGtkBox>>)` | 378 | `SetVisibleOnAllWorkspaces(bool)` |
| 320 | `ScaleFactor(Sender<Result<f64>>)` | 358 | `RawWindowHandle(Sender<Result<SendRawWindowHandle>>)` | 379 | `SetContentProtected(bool)` |
| 321 | `InnerPosition(Sender<Result<PhysicalPosition<i32>>>)` | 359 | `Theme(Sender<Result<Theme>>)` | 380 | `SetSize(Size)` |
| 322 | `OuterPosition(Sender<Result<PhysicalPosition<i32>>>)` | 360 | `Center` | 381 | `SetMinSize(Option<Size>)` |
| 323 | `InnerSize(Sender<Result<PhysicalSize<u32>>>)` | 361 | `RequestUserAttention(Option<UserAttentionType>)` | 382 | `SetMaxSize(Option<Size>)` |
| 324 | `OuterSize(Sender<Result<PhysicalSize<u32>>>)` | 362 | `SetEnabled(bool)` | 383 | `SetSizeConstraints(WindowSizeConstraints)` |
| 325 | `IsFullscreen(Sender<Result<bool>>)` | 363 | `SetResizable(bool)` | 384 | `SetPosition(Position)` |
| 326 | `IsMinimized(Sender<Result<bool>>)` | 364 | `SetMaximizable(bool)` | 385 | `SetFullscreen(bool)` |
| 327 | `IsMaximized(Sender<Result<bool>>)` | 365 | `SetMinimizable(bool)` | 386 | `SetFullscreenOnMonitor(PhysicalPosition<f64>)` |
| 328 | `IsFocused(Sender<Result<bool>>)` | 366 | `SetClosable(bool)` | 388 | `ToggleFullscreen` (F11) |
| 329 | `IsDecorated(Sender<Result<bool>>)` | 367 | `SetTitle(String)` | 392* | `ApplyPendingFullscreen` (macOS) |
| 330 | `IsResizable(Sender<Result<bool>>)` | 368 | `Maximize` | 394* | `SetSimpleFullscreen(bool)` (macOS) |
| 331 | `IsMaximizable(Sender<Result<bool>>)` | 369 | `Unmaximize` | 395 | `SetFocus` |
| 332 | `IsMinimizable(Sender<Result<bool>>)` | 370 | `Minimize` | 398 | `SetFocusable(bool)` (no-op, `#[allow(unused)]`) |
| 333 | `IsClosable(Sender<Result<bool>>)` | 371 | `Unminimize` | 399 | `SetIcon(Icon<'static>)` |
| 334 | `IsVisible(Sender<Result<bool>>)` | 372 | `Show` | 400 | `SetSkipTaskbar(bool)` |
| 335 | `IsEnabled(Sender<Result<bool>>)` | 373 | `Hide` | 401 | `SetCursorGrab(bool)` |
| 336 | `IsAlwaysOnTop(Sender<Result<bool>>)` | 374 | `SetDecorations(bool)` | 402 | `SetCursorVisible(bool)` |
| 337 | `Title(Sender<Result<String>>)` | 375 | `SetShadow(bool)` | 403 | `SetCursorIcon(CursorIcon)` |
| 338 | `CurrentMonitor(Sender<Result<Option<Monitor>>>)` | 376 | `SetAlwaysOnBottom(bool)` | 404 | `SetCursorPosition(Position)` |
| 339 | `PrimaryMonitor(Sender<Result<Option<Monitor>>>)` | | | 405 | `SetIgnoreCursorEvents(bool)` |
| 340 | `MonitorFromPoint(Sender<Result<Option<Monitor>>>, f64, f64)` | | | 406 | `SetProgressBar(ProgressBarState)` |
| 341 | `AvailableMonitors(Sender<Result<Vec<Monitor>>>)` | | | 407 | `SetBadgeCount(Option<i64>, Option<String>)` |
| | | | | 408 | `SetBadgeLabel(Option<String>)` |
| | | | | 409 | `SetOverlayIcon(Option<Icon<'static>>)` |
| | | | | 410 | `SetTitleBarStyle(tauri_utils::TitleBarStyle)` |
| | | | | 411 | `SetTrafficLightPosition(Position)` |
| | | | | 412 | `SetTheme(Option<Theme>)` |
| | | | | 413 | `SetBackgroundColor(Option<Color>)` |
| | | | | 414 | `StartDragging` |
| | | | | 415 | `StartResizeDragging(tauri_runtime::ResizeDirection)` |

77 variants total (24 in the first column, 24 in the second, 29 in the third), read down each
column in declaration order.

`*` = inside a `#[cfg(any(target_os = "linux", dragonfly, freebsd, netbsd, openbsd))]` (GtkWindow,
DefaultVBox) or `#[cfg(target_os = "macos")]` (ApplyPendingFullscreen, SetSimpleFullscreen) block.

#### 1.1.2 `AppWindow` fields (line: field)

| Line | Field | Note |
| --- | --- | --- |
| 469 | `pub(crate) lifetime: NativeWindowToken` | Identity handed to webviews; see §3. |
| 471 | `pub(crate) id: WindowId` | `#[allow(unused)]`. |
| 472 | `pub(crate) label: String` | Tauri window label. |
| 474 | `pub(crate) background_surface: Option<SoftbufferSurface>` | Windows only. |
| 475 | `pub(crate) window: Arc<dyn WinitWindow>` | The native window. |
| 476 | `pub(crate) attrs: AppWindowAttrs` | Live attributes. |
| 477 | `pub(crate) children: Vec<AppWebview>` | Attached webviews (the CEF children). |
| 478 | `pub(crate) native_drag_drop: Option<WinitDragDropState>` | winit drag-and-drop state. |
| 488 | `pub(crate) cef_host: crate::platform::linux::CefX11Host` | Linux/BSD only: X11 parent for CEF children. |
| 492 | `pub(crate) pending_activation: Option<Instant>` | Deadline for the initial raise. |
| 496 | `pub(crate) fullscreen_transition: objc2::rc::Retained<crate::platform::macos::FullscreenTransition>` | macOS only. |

#### 1.1.3 `AppWindowAttrs` fields (line: field)

502 `inner: WindowAttributes` · 503 `center: bool` · 504 `background_color: Option<Color>` ·
505 `prevent_overflow: Option<Size>` · 507 `traffic_light_position: Option<Position>` (macOS) ·
516 `visible_on_all_workspaces: bool` (macOS/Linux/BSD) · 524 `skip_taskbar: bool` (Linux/BSD) ·
534 `transient_for: Option<gtk::Window>` (Linux/BSD).

#### 1.1.4 `impl AppWindow` (537–761)

| Line | Method | Behavior |
| --- | --- | --- |
| 538 | `pub(crate) fn center(&self)` | Centers on the current (else primary) monitor using the **DWM visible frame height** on decorated Windows windows, then `set_outer_position`. |
| 570 | `pub(crate) fn activate(&self)` | Native nudge first (macOS `activate_application()`, Linux `raise_native()`), then `focus_window()`. |
| 593 | `pub(crate) fn request_fullscreen(&self, target: Option<FullscreenTarget>)` | macOS: hold the request while `fullscreen_transition.in_progress()`. Otherwise `None`→leave, `CurrentMonitor`→`Borderless(None)`, `MonitorAt(pos)`→monitor containing `pos` (out-of-range = no-op). |
| 626 | `pub(crate) fn request_window_fullscreen(&mut self, target: Option<FullscreenTarget>)` | App/user fullscreen: takes pages out of HTML fullscreen on exit (`host.exit_fullscreen(1)`), marks the window as owned by the app, then `request_fullscreen`. |
| 643 | `pub(crate) fn toggle_fullscreen(&mut self)` | F11 semantics: leave if the window is (or is about to be) fullscreen **or** any page is in HTML fullscreen; enter otherwise. |
| 656 | `fn fullscreen_requested(&self) -> bool` | State including a request held during a macOS transition. |
| 671 | `pub(crate) fn set_page_fullscreen(&mut self, webview_id: u32, fullscreen: bool)` | HTML-fullscreen bookkeeping: the first page that needs it puts the window fullscreen (`owns_window`), the last one to leave restores it; a window made fullscreen by the app stays. |
| 710 | `pub(crate) fn preferred_theme(&self) -> Option<Theme>` | Window-level theme override, if any. |
| 718 | `pub(crate) fn resolved_theme(&self, app_wide_theme: Option<Theme>) -> Option<Theme>` | Window override, else app-wide. |
| 725 | `pub(crate) fn safe_surface_size(&self) -> PhysicalSize<u32>` | Size available for child layout: on Linux the X11 host size (excludes GTK menus), elsewhere the window surface size. |
| 747 | `pub(crate) fn set_theme(&mut self, theme: Option<Theme>)` | Stores + applies the winit theme, pushes the scheme to every child's CEF request context, and (macOS) re-applies the traffic-light inset. |
| 755 | `fn apply_cef_theme(&self, theme: Option<Theme>)` | `request_context::apply_theme_scheme` for each child. |

#### 1.1.5 `impl<T: UserEvent> WinitCefApp<T>` (763–1305)

| Line | Method | Behavior |
| --- | --- | --- |
| 764 | `pub(crate) fn create_window(&mut self, event_loop: &dyn ActiveEventLoop, window_id: WindowId, webview_id: Option<u32>, pending: Box<PendingWindow<T, CefRuntime<T>>>, _after_window_creation: Option<AfterWindowCreationCallback>) -> Result<()>` | Applies app-wide theme, `prepare_window_attributes`, creates the winit window, the Linux X11 host, the macOS fullscreen observer, the `AppWindow`, the platform side effects (macOS: movability, workspaces, traffic lights; Linux: workspaces, skip-taskbar, transient-for; Windows: background surface; non-Windows: background color), invokes `after_window_creation` with the platform raw handle, builds the initial webview **before** registration, then registers the window. |
| 937 | `pub(crate) fn apply_pending_activations(&mut self)` | Raises windows created focused once winit reports them visible, giving up after `PENDING_ACTIVATION_TIMEOUT`. |
| 971 | `pub(crate) fn apply_pending_host_layouts(&mut self)` (Linux/BSD) | Re-lays out CEF children when GTK resized the X11 host without a toplevel resize. |
| 979 | `pub(crate) fn handle_window_message(&mut self, event_loop: &dyn ActiveEventLoop, window_id: WindowId, message: WindowMessage)` | The single consumer of `WindowMessage`: `Close`/`Destroy` first (they borrow the window mutably), then a second `match` on every remaining variant (§2 lists the per-variant behavior). |

#### 1.1.6 `impl WindowDispatch` helpers

`window_getter!` (1322) is used by 25 members (`scale_factor`, the 18 state getters, `title`,
`current_monitor`, `primary_monitor`, `available_monitors`, `gtk_window`, `default_vbox`,
`window_handle`, `theme`); `monitor_from_point` (1424) builds its message by hand because it carries
two extra `f64`s; everything else is `self.context.send_message(Message::Window { window_id, message })`.

### 1.2 `window_handle.rs`

| Line | Declaration | Intent |
| --- | --- | --- |
| 10 | `pub(crate) struct SendRawWindowHandle(pub raw_window_handle::RawWindowHandle);` | Makes a `RawWindowHandle` (not `Send`) transportable through the message channel. |
| 11 | `unsafe impl Send for SendRawWindowHandle {}` | Manual `Send`; the handle is only borrowed on the UI thread. |
| 13 | `pub(crate) struct SendRawDisplayHandle(pub raw_window_handle::RawDisplayHandle);` | Same for `RawDisplayHandle`. |
| 14 | `unsafe impl Send for SendRawDisplayHandle {}` | Manual `Send`. |
| 18 | `pub(crate) struct SoftbufferWindowHandle { display: RawDisplayHandle, window: RawWindowHandle }` (Windows only, `#[derive(Clone, Copy, Debug)]` at 17) | Owned copy of winit's raw handles so a `softbuffer` surface can outlive the borrow. |
| 25 | `pub(crate) fn new(window: &dyn WinitWindow) -> Option<Self>` | Copies `display_handle()`/`window_handle()`, `None` if either is unavailable. |
| 34 | `impl HasDisplayHandle for SoftbufferWindowHandle` | Borrows the stored raw display handle. |
| 43 | `impl HasWindowHandle for SoftbufferWindowHandle` | Borrows the stored raw window handle. |

No item in this file is exported from the crate root; all are `pub(crate)`.

### 1.3 `popup.rs`

Module doc (5–7): *"CEF-owned popup lifetimes. CEF keeps its native window and opener semantics; the
runtime owns observation, teardown, and the separate protocol observer, which serves the runtime's
own dialog observation and no app callback."*

| Line | Declaration | Intent |
| --- | --- | --- |
| 20 | `const MAX_POPUPS: usize = 128;` | Hard cap on popups per family, and on tracked popup windows. |
| 23 | `pub(crate) struct PopupRequest { pub(crate) opener: FrameNavigationState, popup_id: i32, identity: Arc<()> }` (`#[derive(Clone)]` at 22) | A reservation for a popup CEF is about to create; `identity` distinguishes two requests with the same `popup_id`. |
| 29 | `pub(crate) fn is_same(&self, other: &Self) -> bool` | `Arc::ptr_eq` on `identity`. |
| 34 | `struct Popup { browser: Browser, state: FrameNavigationState, opener: FrameNavigationState, closing: AtomicBool, _observer: Registration, dialogs: crate::dialog::DialogState }` | One live popup: its browser, its own navigation state, the opener's state, a closing flag, the runtime's private devtools observer (kept alive by the field), and its dialog state. |
| 43 | `pub(crate) struct PopupFamily { root: FrameNavigationState, closing: AtomicBool, popups: Mutex<Vec<Arc<Popup>>>, pending: Mutex<Vec<PopupRequest>>, windows: Mutex<Vec<(Window, NativeWindowToken)>> }` | All popups of one root webview, plus reservations and the native windows they live in. |
| 52 | `pub(crate) fn new(root: FrameNavigationState) -> Self` | Empty, live family. |
| 62 | `pub(crate) fn admits(&self, opener: &FrameNavigationState, browser_id: i32) -> bool` | Admission rule (see §4.2). |
| 76 | `pub(crate) fn reserve(&self, opener: &FrameNavigationState, browser_id: i32, popup_id: i32) -> Option<PopupRequest>` | Records a reservation if admitted, under `MAX_POPUPS`, and not a duplicate `(popup_id, opener)`. |
| 102 | `pub(crate) fn abort(&self, opener: &FrameNavigationState, popup_id: i32) -> Option<PopupRequest>` | Removes and returns the matching reservation (CEF discarded the popup). |
| 110 | `pub(crate) fn created(&self, browser: &Browser, request: &PopupRequest, state: &FrameNavigationState)` | Validates the reservation, installs the private observer, and registers the popup; closes the browser on any failure. |
| 168 | `pub(crate) fn closed(&self, state: &FrameNavigationState, browser_id: i32)` | Revokes the exact browser and its descendants, then asks CEF to close them. |
| 208 | `pub(crate) fn close_all(&self)` | Window close / app shutdown: revokes the whole family regardless of observed identity. |
| 240 | `pub(crate) fn is_revoked(&self) -> bool` | Whether the family lost its root. |
| 244 | `pub(crate) fn observe(&self) -> Vec<Webview>` | Snapshots every live popup as a `Webview` (see §4.4). |
| 333 | `fn revoke_descendants<'a>(state: &FrameNavigationState, root_closed: bool, nodes: impl Iterator<Item = (usize, &'a FrameNavigationState, &'a FrameNavigationState, &'a AtomicBool)> + Clone) -> Vec<usize>` | Order-independent graph walk that marks and closes every descendant of a revoked opener. |
| 366 | `mod tests` (`#[cfg(test)]`) | Five tests: exact-descendant revocation, bounded reservations, teardown of a family with an unresolved reservation, admission requiring the live exact opener, teardown of a family whose root never observed a browser id. |

No item in this file is exported from the crate root; all are `pub(crate)`.

### 1.4 `offscreen.rs`

| Line | Declaration | Intent |
| --- | --- | --- |
| 14 | `pub type CursorHandler = dyn Fn(cef::CursorType, Option<&cef::CursorInfo>) + Send + Sync;` | Cursor-change notification on the CEF UI thread; any custom cursor buffer is valid only during the call. |
| 27 | `pub struct Offscreen { pub shared_texture_enabled: bool, pub external_begin_frame_enabled: bool, pub cursor_handler: Option<Arc<CursorHandler>>, pub(crate) render_handler: Arc<dyn Fn(OffscreenView) -> cef::RenderHandler + Send + Sync> }` (`#[derive(Clone)]` at 26) | Windowless-browser configuration: the app owns painting, input/IME forwarding and composition; Tauri keeps IPC, protocols and lifecycle. |
| 45 | `pub fn new<F: Fn(OffscreenView) -> cef::RenderHandler + Send + Sync + 'static>(handler: F) -> Self` | Builds the config with both paint options off and no cursor handler. |
| 56 | `pub(crate) fn window_info(&self, parent: cef::sys::cef_window_handle_t) -> cef::WindowInfo` | `WindowInfo::default().set_as_windowless(parent)` (windowless rendering on, `RuntimeStyle::ALLOY`), then the two paint flags. |
| 66 | `pub struct OffscreenView { pub(crate) window: Weak<dyn winit::window::Window>, pub(crate) bounds: Arc<Mutex<Rect>>, pub(crate) visible: Arc<AtomicBool> }` (`#[derive(Clone)]` at 65) | Live layout of one offscreen webview, shared with its render handler; the window is weak so CEF holding the handler cannot keep a closed window alive. |
| 77 | `pub fn window(&self) -> Option<Arc<dyn winit::window::Window>>` | Native host for scale, screen coordinates, IME and redraws; `None` once released. |
| 82 | `pub fn request_redraw(&self)` | Asks the host window to redraw, if it still exists. |
| 89 | `pub fn bounds(&self) -> Rect` | Bounds relative to the host, in logical pixels (poisoned lock is recovered). |
| 94 | `pub fn is_visible(&self) -> bool` | Requested visibility, independent of native occlusion. |
| 100 | `mod tests` (`#[cfg(test)]`) | Asserts Alloy style, windowless rendering, and the two paint flags. |

Crate-root exports (`lib.rs:27`): `pub use offscreen::{CursorHandler, Offscreen, OffscreenView};`.

### 1.5 `platform/mod.rs`

| Line | Declaration | Visibility | Intent |
| --- | --- | --- | --- |
| 6 | `mod windows;` (`#[cfg(windows)]`) | private | Windows backend modules. |
| 9 | `pub mod macos;` (`#[cfg(target_os = "macos")]`) | `pub` inside a private `platform` module | macOS backend. |
| 18 | `pub mod linux;` (`#[cfg(any(linux, dragonfly, freebsd, netbsd, openbsd))]`) | same | Linux/BSD backend. |
| 23 | `pub(crate) trait MonitorExt { fn work_area(&self) -> PhysicalRect<i32, u32>; }` | crate-internal | Work area of a monitor; doc says it is a native shim pending winit support. |
| 30 | `fn monitor_bounds(monitor: &MonitorHandle) -> PhysicalRect<i32, u32>` | private | Full monitor rect (position + current video mode), used as the work-area fallback. |
| 40 | `pub trait EventLoopExt { … }` | `pub` inside a private module | Event-loop-level operations the dispatcher needs. Methods: 42 `set_activation_policy(&self, policy: tauri_runtime::ActivationPolicy)` (macOS), 44 `set_dock_visibility(&self, visible: bool)` (macOS), 46 `show_application(&self)` (macOS), 48 `hide_application(&self)` (macOS), 50 `set_progress_bar(&self, state: tauri_runtime::ProgressBarState)` (macOS), 51 `set_badge_count(&self, count: Option<i64>, desktop_filename: Option<String>)`, 52 `set_badge_label(&self, label: Option<String>)`, 53 `cursor_position(&self) -> tauri_runtime::Result<tauri_runtime::dpi::PhysicalPosition<f64>>`. |

Because `platform` itself is `mod platform;` (private, `lib.rs:28`), the only path from the crate
root into `platform/` is `lib.rs:47`:
`pub use platform::macos::setup_application as prepare_macos_application;` (macOS only).

### 1.6 `platform/windows/`

`mod.rs` (9 lines) declares `mod event_loop; mod icon; mod monitor; mod webview; mod window;` — all
private; nothing is re-exported.

| File:line | Declaration | Mechanism |
| --- | --- | --- |
| `window.rs:29` | `pub(crate) fn cef_host_handle(&self) -> cef::sys::cef_window_handle_t` | `HWND` wrapped as `cef::sys::HWND`. |
| `window.rs:33` | `pub(crate) fn hwnd(&self) -> HWND` | `window_handle()` → `RawWindowHandle::Win32`; panics on another handle kind. |
| `window.rs:44` | `pub(crate) fn is_enabled(&self) -> bool` | `IsWindowEnabled`. |
| `window.rs:48` | `pub(crate) fn set_enabled(&self, enabled: bool)` | `EnableWindow`. |
| `window.rs:52` | `pub(crate) fn set_overlay_icon(&self, icon: Option<Icon<'static>>)` | `ITaskbarList3::SetOverlayIcon` via `CoCreateInstance`; `DestroyIcon` after use; `None` clears. |
| `window.rs:70` | `pub(crate) fn set_progress_bar(&self, state: ProgressBarState)` | `ITaskbarList3::SetProgressState` (`TBPF_NOPROGRESS/NORMAL/INDETERMINATE/PAUSED/ERROR`) + `SetProgressValue(progress.min(100), 100)`. |
| `window.rs:94` | `pub(crate) fn set_background_color(&mut self, _color: Option<Color>)` | No native call; only `request_redraw()` (the color lives in `attrs`, painted by `draw_background_surface`). |
| `window.rs:112` | `pub(crate) fn draw_background_surface(&mut self)` | `softbuffer` surface over the window; skips offscreen children; color from `attrs.background_color`, else transparent→`0` / opaque→`0x00ff_ffff`. |
| `window.rs:161` | `pub(crate) fn dwm_visible_frame_height(&self) -> Option<u32>` | `DwmGetWindowAttribute(DWMWA_EXTENDED_FRAME_BOUNDS)` → visible height for centering. |
| `webview.rs:23` | `pub(crate) fn native_parent_matches(&self, parent: &AppWindow) -> Option<bool>` | `IsWindow` + `GetParent` with `SetLastError(ERROR_SUCCESS)` to distinguish "no parent" from failure. |
| `webview.rs:43` | `pub(crate) fn native_visible(&self) -> Option<bool>` | `IsWindow` → `IsWindowVisible`. |
| `webview.rs:52` | `pub(crate) fn hwnd(&self) -> HWND` | `host.window_handle()`. |
| `webview.rs:57` | `pub(crate) fn set_background_color(&self, _color: Option<Color>)` | No-op (TODO in source). |
| `webview.rs:61` | `pub(crate) fn bounds(&self) -> Option<Rect>` | Offscreen view bounds, else `GetWindowRect` + `MapWindowPoints` into the parent. |
| `webview.rs:101` | `pub(crate) fn reparent(&self, parent: &AppWindow)` | `SetParent`, then `unclip_siblings`. |
| `webview.rs:123` | `pub(crate) fn unclip_siblings(&self)` | Clears `WS_CLIPSIBLINGS` and forces `SWP_FRAMECHANGED` so translucent webviews composite against the one below. |
| `webview.rs:146` | `pub(crate) fn apply_visible(&self, visible: bool)` | `ShowWindow(SW_SHOW/SW_HIDE)`. |
| `webview.rs:153` | `pub(crate) fn destroy_host_window(&self)` | `DestroyWindow` (CEF sees `WM_NCDESTROY` → `WindowDestroyed`). |
| `webview.rs:157-160` | `const PIN_Z_ORDER_SUBCLASS_ID: usize = 124;` `const Z_ORDER_UNPINNED: usize = 0;` `const Z_ORDER_PINNED: usize = 1;` | Subclass id and `dwRefData` values for the z-order pin. |
| `webview.rs:163` | `unsafe extern "system" fn pin_z_order_subclass_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM, _subclass_id: usize, pinned: usize) -> LRESULT` | Adds `SWP_NOZORDER` to every `WM_WINDOWPOSCHANGING` while pinned. |
| `webview.rs:186` | `fn set_z_order_pinned(&self, pinned: bool)` | `SetWindowSubclass` with the same id: installs the first time, toggles `dwRefData` afterwards. |
| `webview.rs:203` | `pub(crate) fn raise_to_top(&self)` | Unpin → `SetWindowPos(HWND_TOP)` → pin. |
| `webview.rs:221` | `pub(crate) fn apply_physical_bounds(&self, _scale: f64, x: i32, y: i32, width: i32, height: i32)` | `SetWindowPos(SWP_NOZORDER \| SWP_NOACTIVATE)`. |
| `event_loop.rs:12` | `fn set_badge_count(&self, _count: Option<i64>, _desktop_filename: Option<String>)` | Unsupported on Windows (empty). |
| `event_loop.rs:16` | `fn set_badge_label(&self, _label: Option<String>)` | Unsupported on Windows (empty). |
| `event_loop.rs:20` | `fn cursor_position(&self) -> Result<PhysicalPosition<f64>>` | `GetCursorPos`, `Error::FailedToGetCursorPosition` on failure. |
| `icon.rs:8` | `pub fn icon_to_hicon(icon: Icon<'static>) -> Option<HICON>` | Validates RGBA length, builds the AND mask (`alpha - 255`), swaps R/B, `CreateIcon`. |
| `monitor.rs:12` | `fn work_area(&self) -> PhysicalRect<i32, u32>` | `GetMonitorInfoW` → `rcWork`, falling back to `monitor_bounds`. |

### 1.7 `platform/macos/`

`mod.rs`: `mod application; mod dock; mod event_loop; mod fullscreen; mod monitor; mod progress; mod
utils; mod wake; mod webview; mod window;` plus
`pub use application::setup_application;` (16),
`pub(crate) use application::{AppDelegate, AppDelegateEvent, activate_application, set_application_event_handler};` (17-19),
`pub(crate) use fullscreen::FullscreenTransition;` (20), `pub(crate) use wake::MainThreadWake;` (21),
`pub(crate) use window::nswindow;` (22).

| File:line | Declaration | Mechanism |
| --- | --- | --- |
| `application.rs:28` | `pub(crate) struct CefWinitApplicationIvars { handling_send_event: Cell<Bool>, last_dock_show_ms: Cell<u64>, delegate: Cell<*const AppDelegate> }` | Ivars of the `NSApplication` subclass. |
| `application.rs:34` | `pub(crate) enum AppDelegateEvent { TryTerminate, Reopen { has_visible_windows: bool }, AccessibilityChanged { enabled: bool }, OpenURLs { urls: Vec<url::Url> } }` | Delegate events forwarded to the runtime. |
| `application.rs:41` | `pub(crate) struct CefAppDelegateIvars { on_event: Box<dyn Fn(AppDelegateEvent)> }` | Delegate callback storage. |
| `application.rs:50` | `pub(crate) struct AppDelegate` (`define_class!`, `"CefWinitAppDelegate"`, `MainThreadOnly`) | `NSApplicationDelegate`: `application:openURLs:`, `applicationShouldTerminate:` (→ `TerminateNow`), `applicationShouldHandleReopen:hasVisibleWindows:`, `applicationSupportsSecureRestorableState:` (→ true), plus `tryToTerminateApplication:` and `enableAccessibility:`. |
| `application.rs:111` | `pub(crate) struct CefWinitApplication` (`define_class!`, super `NSApplication`) | Overrides `sendEvent:` (tracks `handling_send_event`), adds `tauriTransformProcessToForeground`, `tauriActivateCurrentApplication`, `terminate:`, `accessibilitySetValue:forAttribute:`; conforms to `CrAppControlProtocol`, `CrAppProtocol`, `CefAppProtocol` — the CEF-required `NSApplication` shape. |
| `application.rs:182` | `fn new(mtm: MainThreadMarker, on_event: Box<dyn Fn(AppDelegateEvent)>) -> Retained<Self>` | Allocates the delegate. |
| `application.rs:187` | `fn emit(&self, event: AppDelegateEvent)` | Invokes the handler. |
| `application.rs:195` | `pub fn shared_application() -> Retained<Self>` | `[CefWinitApplication sharedApplication]`. |
| `application.rs:198` | `pub fn last_dock_show(&self) -> Option<Instant>` | Reads the `last_dock_show_ms` ivar (`0` = never; stored as ms+1). |
| `application.rs:207` | `pub fn set_last_dock_show(&self, instant: Instant)` | Stores ms since `instant_epoch()` + 1. |
| `application.rs:232` | `pub(crate) fn activate_application()` | `-[NSApplication activate]` when available, else deprecated `activateIgnoringOtherApps:`. |
| `application.rs:256` | `pub fn setup_application()` | Installs the CEF-compatible `NSApplication` subclass before any `NSApplication` exists; panics off the main thread or if another class owns the singleton. Re-exported as `prepare_macos_application`. |
| `application.rs:262` | `pub(crate) fn set_application_event_handler(on_event: Box<dyn Fn(AppDelegateEvent)>) -> Retained<AppDelegate>` | Creates the delegate, stores a non-owning pointer in the app ivars, sets it as `NSApplication.delegate`, returns the retained handle. |
| `dock.rs:16` | `const DOCK_SHOW_TIMEOUT: Duration = Duration::from_secs(1);` | Ignore hide requests within 1 s of a show (Electron's duplicate-icon workaround). |
| `dock.rs:17` | `const DOCK_BUNDLE_IDENTIFIER: &str = "com.apple.dock";` | Used to activate the Dock before a delayed foreground transform. |
| `dock.rs:20` | `pub fn set_dock_visibility(&self, visible: bool)` | Show/hide the Dock tile. |
| `dock.rs:28` | `fn set_dock_hide(&self)` | `set_windows_can_hide(false)` + `TransformProcessType(kProcessTransformToUIElementApplication)`, skipped inside the 1 s window. |
| `dock.rs:45` | `fn set_dock_show(&self)` | Records the time, `set_windows_can_hide(true)`, then either the delayed path (active app: activate Dock, transform after 1 s, reactivate after 2 s) or an immediate foreground transform. |
| `dock.rs:61` | `fn set_windows_can_hide(&self, can_hide: bool)` | `setCanHide:` on every window. |
| `dock.rs:68` | `fn perform_delayed_dock_show(&self)` | `performSelector:afterDelay:` for `tauriTransformProcessToForeground` (1.0 s) and `tauriActivateCurrentApplication` (2.0 s). |
| `dock.rs:86` | `fn activate_dock()` | `NSRunningApplication::runningApplicationsWithBundleIdentifier("com.apple.dock")` → activate. |
| `event_loop.rs:19` | `fn set_activation_policy(&self, policy: tauri_runtime::ActivationPolicy)` | `NSApplication::setActivationPolicy(Regular/Accessory/Prohibited)`. |
| `event_loop.rs:34` | `fn set_dock_visibility(&self, visible: bool)` | Delegates to `CefWinitApplication::set_dock_visibility`. |
| `event_loop.rs:43` | `fn show_application(&self)` | `NSApp.unhide(None)`. |
| `event_loop.rs:51` | `fn hide_application(&self)` | `NSApp.hide(None)`. |
| `event_loop.rs:59` | `fn set_progress_bar(&self, state: ProgressBarState)` | `progress::set_dock_progress_bar`. |
| `event_loop.rs:63` | `fn set_badge_count(&self, count: Option<i64>, _desktop_filename: Option<String>)` | `set_badge_label(count.map(\|c\| c.to_string()))`. |
| `event_loop.rs:67` | `fn set_badge_label(&self, label: Option<String>)` | `NSDockTile::setBadgeLabel`. |
| `event_loop.rs:78` | `fn cursor_position(&self) -> Result<PhysicalPosition<f64>>` | `NSEvent::mouseLocation()` (bottom-left origin, logical) → flip against the primary screen height → physical. |
| `fullscreen.rs:43` | `const TRANSITION_TIMEOUT: f64 = 3.0;` | A transition that has not ended after 3 s is treated as over. |
| `fullscreen.rs:45` | `pub(crate) struct FullscreenTransitionIvars { in_progress: Cell<bool>, pending: Cell<Option<Option<FullscreenTarget>>>, on_transition_end: Box<dyn Fn()> }` | Transition state; `Some(None)` is a pending request to leave fullscreen. |
| `fullscreen.rs:60` | `pub(crate) struct FullscreenTransition` (`define_class!`, `"TauriCefFullscreenTransition"`) | Observes `NSWindowWill/DidEnter/ExitFullScreenNotification`; `windowWillChangeFullScreen:` starts the timeout, `windowDidChangeFullScreen:` ends it, `transitionTimedOut:` is the guard. |
| `fullscreen.rs:97` | `pub(crate) fn observe(nswindow: &NSWindow, on_transition_end: impl Fn() + 'static) -> Retained<Self>` | Registers the four notifications for one window. |
| `fullscreen.rs:142` | `fn cancel_timeout(&self)` | `cancelPreviousPerformRequestsWithTarget:selector:object:`. |
| `fullscreen.rs:154` | `fn transition_ended(&self)` | Clears `in_progress`, fires `on_transition_end` if a request is pending. |
| `fullscreen.rs:162` | `pub(crate) fn in_progress(&self) -> bool` | Whether a transition is animating. |
| `fullscreen.rs:167` | `pub(crate) fn hold(&self, target: Option<FullscreenTarget>)` | Keeps the latest request made during the transition. |
| `fullscreen.rs:172` | `pub(crate) fn pending(&self) -> Option<Option<FullscreenTarget>>` | Peeks at the held request. |
| `fullscreen.rs:177` | `pub(crate) fn take_pending(&self) -> Option<Option<FullscreenTarget>>` | Takes it (used by `ApplyPendingFullscreen`). |
| `monitor.rs:12` | `fn work_area(&self) -> PhysicalRect<i32, u32>` | `NSScreen::visibleFrame` vs `frame` deltas applied to the winit position, converted back to physical. |
| `progress.rs:14` | `struct DockProgressIndicatorIvars { state: Cell<ProgressBarStatus> }` | Status driving the bar color. |
| `progress.rs:29` | `struct DockProgressIndicator` (`define_class!`, super `NSProgressIndicator`) | Custom `drawRect:`: rounded track + progress bar, yellow when paused, red on error, blue otherwise. |
| `progress.rs:60` | `fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self>` | Allocates with a frame. |
| `progress.rs:65` | `fn set_state(&self, status: ProgressBarStatus)` | Stores the status. |
| `progress.rs:70` | `fn draw_rounded_rect(rect: NSRect)` | `NSBezierPath` rounded rect fill. |
| `progress.rs:75` | `pub fn set_dock_progress_bar(state: ProgressBarState)` | Installs/reuses the indicator in the dock tile content view, sets value/visibility, `dock_tile.display()`. |
| `progress.rs:98` | `fn dock_progress_indicator(app: &NSApplication, dock_tile: &NSDockTile, mtm: MainThreadMarker) -> Option<Retained<DockProgressIndicator>>` | Lazily builds the dock tile content view (app icon image view) and the indicator. |
| `progress.rs:128` | `fn existing_progress_indicator(content_view: &NSView) -> Option<Retained<DockProgressIndicator>>` | Finds an already-installed indicator among the subviews. |
| `utils.rs:21` | `struct ProcessSerialNumber { highLongOfPSN: u32, lowLongOfPSN: u32 }` (`#[repr(C)]`) | Argument type of `TransformProcessType`. |
| `utils.rs:26` | `pub fn transform_process_type(transform_state: ProcessApplicationTransformState)` | `TransformProcessType(kCurrentProcess, state)`. |
| `utils.rs:38` | `pub fn ns_color_from_tauri_color(color: Color) -> Retained<NSColor>` | sRGB `NSColor` from 8-bit channels. |
| `utils.rs:49` | `pub fn instant_epoch() -> Instant` | Process-wide `OnceLock<Instant>` baseline. |
| `utils.rs:54` | `pub(crate) fn set_associated_object<O: ClassType>(object: &O, key: *const c_void, value: &AnyObject)` | `objc_setAssociatedObject(RETAIN_NONATOMIC)`. |
| `utils.rs:69` | `pub(crate) fn associated_object<O: ClassType>(object: &O, key: *const c_void) -> Option<Retained<AnyObject>>` | `objc_getAssociatedObject` + retain. |
| `wake.rs:26` | `type OnPass = dyn Fn() + Send + Sync;` | The per-run-loop-pass callback. |
| `wake.rs:31` | `pub(crate) struct MainThreadWake { inner: Arc<Inner> }` (`#[derive(Clone)]`) | Wakes winit's loop from any thread without re-entering its handler. |
| `wake.rs:55` | `unsafe extern "C-unwind" fn on_before_sources(_: *mut CFRunLoopObserver, _: CFRunLoopActivity, info: *mut c_void)` | Observer callout invoking the boxed closure. |
| `wake.rs:66` | `unsafe extern "C-unwind" fn release(info: *const c_void)` | Frees the boxed closure when the observer dies. |
| `wake.rs:74` | `pub(crate) fn new(on_pass: impl Fn() + Send + Sync + 'static) -> Self` | Installs a `BeforeSources` observer in `kCFRunLoopCommonModes` on the main run loop. |
| `wake.rs:119` | `pub(crate) fn wake_up(&self)` | `CFRunLoopWakeUp`; calls coalesce. |
| `webview.rs:17` | `fn try_nsview(&self) -> Option<Retained<NSView>>` | Retains CEF's `NSView` for a synchronous observation; `None` when destroyed. |
| `webview.rs:24` | `pub(crate) fn nsview(&self) -> Retained<NSView>` | Panicking variant. |
| `webview.rs:28` | `pub(crate) fn native_parent_matches(&self, parent: &AppWindow) -> Option<bool>` | Compares `superview` with the window's content view. |
| `webview.rs:34` | `pub(crate) fn native_visible(&self) -> Option<bool>` | `!isHiddenOrHasHiddenAncestor`. |
| `webview.rs:38` | `pub(crate) fn set_background_color(&self, color: Option<Color>)` | Layer-backed view; sets the layer background color (skipped for offscreen). |
| `webview.rs:58` | `pub(crate) fn bounds(&self) -> Option<Rect>` | View frame relative to the parent, flipping Y when the parent is not flipped. |
| `webview.rs:83` | `pub(crate) fn reparent(&self, parent: &AppWindow)` | `parent.addSubview(view)`. |
| `webview.rs:90` | `pub(crate) fn apply_visible(&self, visible: bool)` | `setHidden(!visible)`. |
| `webview.rs:102` | `pub(crate) fn destroy_host_window(&self)` | `removeFromSuperview` (dealloc reports `WindowDestroyed` to CEF). |
| `webview.rs:106` | `pub(crate) fn apply_physical_bounds(&self, scale: f64, x: i32, y: i32, width: i32, height: i32)` | Physical → logical division, Y flip for non-flipped parents, `setFrame`. |
| `window.rs:33` | `fn nsview(window: &dyn winit::window::Window) -> Retained<NSView>` | Retains the content view from `RawWindowHandle::AppKit`. |
| `window.rs:45` | `pub(crate) fn nswindow(window: &dyn winit::window::Window) -> Retained<NSWindow>` | The `NSWindow` behind a winit window. |
| `window.rs:52` | `pub(crate) fn cef_host_handle(&self) -> cef::sys::cef_window_handle_t` | The content `NSView*`. |
| `window.rs:57` | `pub(crate) fn nsview(&self) -> Retained<NSView>` | Content view of this window. |
| `window.rs:61` | `pub(crate) fn set_enabled(&self, enabled: bool)` | Disable = begin a 0.5-alpha titled sheet; enable = `endSheet`. |
| `window.rs:94` | `pub(crate) fn is_enabled(&self) -> bool` | No attached sheet. |
| `window.rs:102` | `pub(crate) fn set_traffic_light_position(&self, position: &Position)` | Insets the three standard buttons and installs the reset observer. |
| `window.rs:121` | `pub(crate) fn reapply_traffic_light_position_after_appearance_change(&self)` | Re-applies the inset on the next run-loop turn after a theme change. |
| `window.rs:142` | `pub(crate) fn set_title_bar_style(&self, style: TitleBarStyle)` | `Visible`→(not transparent, no `FullSizeContentView`), `Transparent`→transparent, `Overlay`→transparent + `FullSizeContentView`; then `apply_movability`. |
| `window.rs:162` | `pub(crate) fn apply_movability(&self)` | `setMovable(!overlay && !simple_fullscreen)`. |
| `window.rs:175` | `pub(crate) fn set_visible_on_all_workspaces(&self, visible: bool)` | `NSWindowCollectionBehavior::CanJoinAllSpaces`. |
| `window.rs:186` | `pub(crate) fn set_background_color(&self, color: Option<Color>)` | `setOpaque(alpha == 255)` + `setBackgroundColor`, defaulting to `windowBackgroundColor`. |
| `window.rs:200` | `fn inset_traffic_lights(nswindow: &NSWindow, x: f64, y: f64)` | Resizes the title-bar container and repositions close/miniaturize/zoom. |
| `window.rs:240` | `fn observe_traffic_light_resets(nswindow: &NSWindow, position: NSPoint)` | Registers one `TrafficLightObserver` for seven window notifications. |
| `window.rs:281` | `fn reapply_traffic_light_position(nswindow: &NSWindow, position: NSPoint)` | Skips fullscreen (buttons belong to the overlay), else re-insets. |
| `window.rs:294` | `struct TrafficLightObserverIvars { position: Cell<NSPoint> }` | The position the observer owns. |
| `window.rs:303` | `struct TrafficLightObserver` (`define_class!`, `"TauriCefTrafficLightObserver"`) | `handleWindowNotification:` and `reapplyTrafficLightPosition:`. |
| `window.rs:327` | `fn new(mtm: MainThreadMarker, position: NSPoint) -> Retained<Self>` | Allocates with the position. |
| `window.rs:335` | `fn traffic_light_observer_key() -> *const c_void` | Private associated-object key. |
| `window.rs:340` | `fn traffic_light_observer(nswindow: &NSWindow) -> Option<Retained<TrafficLightObserver>>` | Reads the associated observer. |
| `window.rs:347` | `fn set_traffic_light_observer(nswindow: &NSWindow, observer: &TrafficLightObserver)` | Writes it. |

### 1.8 `platform/linux/`

`mod.rs`: `mod event_loop; mod monitor; mod taskbar; mod utils; mod webview; mod window;` plus
`pub(crate) use window::CefX11Host;` (12).

| File:line | Declaration | Mechanism |
| --- | --- | --- |
| `window.rs:37` | `pub(crate) struct CefX11Host { default_vbox: gtk::Box, xid: c_ulong, colormap: c_ulong, geometry: Rc<HostGeometry>, background_color_provider: RefCell<Option<gtk::CssProvider>>, mapped: Cell<bool> }` | 24-bit TrueColor X11 parent for CEF children, kept sized to GTK's content box so GTK UI (menus) stays outside the CEF child stack. |
| `window.rs:51` | `struct HostGeometry { size: Cell<PhysicalSize<u32>>, needs_relayout: Cell<bool> }` | Geometry shared with the GTK `layout` handler. |
| `window.rs:58` | `pub(crate) fn new(window: &dyn winit::window::Window) -> Option<Self>` | Builds `default_vbox` (with a `webview_area` child), creates the X11 container, connects `surface.connect_layout` to keep it positioned. |
| `window.rs:112` | `pub(crate) fn default_vbox(&self) -> gtk::Box` | Clone of the runtime's default vertical box. |
| `window.rs:118` | `fn background_color_class(&self) -> String` | `tauri-cef-window-background-{xid}` CSS class, unique per window. |
| `window.rs:122` | `pub(crate) fn set_visible(&self, visible: bool)` | `XMapWindow`/`XUnmapWindow` on the host (idempotent). |
| `window.rs:135` | `pub(crate) fn size(&self) -> PhysicalSize<u32>` | Last host size. |
| `window.rs:140` | `pub(crate) fn take_needs_relayout(&self) -> bool` | Clears and returns the relayout flag. |
| `window.rs:144` | `fn take_background_color_provider(&self) -> Option<gtk::CssProvider>` | Removes the previous provider. |
| `window.rs:148` | `fn set_background_color_provider(&self, provider: gtk::CssProvider)` | Stores the current one. |
| `window.rs:153` | `impl Drop for CefX11Host` | Removes the CSS provider, `XDestroyWindow`, `XFreeColormap`. |
| `window.rs:170` | `pub(crate) fn cef_host_handle(&self) -> cef::sys::cef_window_handle_t` | The host `xid`. |
| `window.rs:174` | `pub(crate) fn xid(&self) -> c_ulong` | Toplevel X11 window id from the raw handle (`Xlib` or `Xcb`). |
| `window.rs:178` | `pub(crate) fn raise_native(&self)` | `utils::activate_window(xid)`. |
| `window.rs:183` | `pub(crate) fn apply_transient_for(&self)` | `gtk_window.set_transient_for(attrs.transient_for)`. |
| `window.rs:196` | `pub(crate) fn set_enabled(&self, enabled: bool)` | `gtk_window.set_sensitive(enabled)` — GTK tree only; CEF children stay interactive. |
| `window.rs:204` | `pub(crate) fn is_enabled(&self) -> bool` | `is_sensitive()`, default `true`. |
| `window.rs:214` | `pub(crate) fn set_background_color(&self, color: Option<Color>)` | Per-window CSS provider (`rgba(...)`), replacing the previous one; `None` removes the class. |
| `window.rs:253` | `pub(crate) fn set_skip_taskbar(&self, skip: bool)` | `_NET_WM_STATE_SKIP_TASKBAR` client message. |
| `window.rs:257` | `pub(crate) fn set_visible_on_all_workspaces(&self, visible: bool)` | `_NET_WM_STATE_STICKY` client message. |
| `window.rs:261` | `pub(crate) fn set_progress_bar(&self, state: ProgressBarState)` | `taskbar::set_progress_bar`. |
| `window.rs:266` | `fn window_xid(window: &dyn winit::window::Window) -> c_ulong` | `RawWindowHandle::Xlib`/`Xcb` → `c_ulong`; panics otherwise. |
| `window.rs:275` | `fn create_cef_container(parent_xid: c_ulong, initial_size: PhysicalSize<u32>) -> Option<(c_ulong, c_ulong)>` | `XMatchVisualInfo(24, TrueColor)`, `XCreateColormap`, `XCreateWindow`, `XMapWindow`. |
| `window.rs:336` | `fn set_cef_container_bounds(xid: c_ulong, gtk_window: &gtk::ApplicationWindow, webview_area: &gtk::Box, scale_factor: f64) -> PhysicalSize<u32>` | Logical GTK geometry × scale → `XMoveResizeWindow`, returns the new physical size. |
| `webview.rs:16` | `pub(crate) fn native_parent_matches(&self, parent: &AppWindow) -> Option<bool>` | `XQueryTree` → compare parent with `cef_host_handle()`. |
| `webview.rs:42` | `pub(crate) fn native_visible(&self) -> Option<bool>` | `XGetWindowAttributes` → `map_state == IsViewable`. |
| `webview.rs:57` | `fn xid(&self) -> xlib::Window` | `host.window_handle()`, asserting non-zero. |
| `webview.rs:63` | `pub(crate) fn set_background_color(&self, color: Option<Color>)` | Intentional no-op (native child background ≠ Chromium's rendered background). |
| `webview.rs:69` | `pub(crate) fn bounds(&self) -> Option<Rect>` | Offscreen view bounds, else `XGetGeometry`. |
| `webview.rs:106` | `pub(crate) fn reparent(&self, parent: &AppWindow)` | `XReparentWindow` into the X11 host + `XMapRaised`. |
| `webview.rs:118` | `pub(crate) fn apply_visible(&self, visible: bool)` | Visible: clears `_NET_WM_STATE` then `XMapWindow`; hidden: sets `_NET_WM_STATE_HIDDEN` then `XUnmapWindow`. |
| `webview.rs:154` | `pub(crate) fn apply_physical_bounds(&self, _scale: f64, x: i32, y: i32, width: i32, height: i32)` | `XMoveResizeWindow` with `max(1)` dimensions (no `XSync`). |
| `event_loop.rs:14` | `fn set_badge_count(&self, count: Option<i64>, desktop_filename: Option<String>)` | `taskbar::set_badge_count`. |
| `event_loop.rs:18` | `fn set_badge_label(&self, _label: Option<String>)` | Unsupported on Linux/BSD. |
| `event_loop.rs:22` | `fn cursor_position(&self) -> Result<PhysicalPosition<f64>>` | `XQueryPointer` on the root window. |
| `monitor.rs:15` | `fn work_area(&self) -> PhysicalRect<i32, u32>` | `_NET_WORKAREA` (+ `_NET_CURRENT_DESKTOP`) intersected with the monitor rect. |
| `monitor.rs:21` | `fn x11_work_area(monitor_bounds: PhysicalRect<i32, u32>) -> Option<PhysicalRect<i32, u32>>` | Reads the EWMH properties and intersects. |
| `monitor.rs:51` | `fn get_cardinal_property(xlib: &xlib::Xlib, display: *mut xlib::Display, window: xlib::Window, name: &str) -> Option<Vec<c_long>>` | `XGetWindowProperty` for 32-bit `XA_CARDINAL` arrays. |
| `monitor.rs:102` | `fn intersect_rects(a: PhysicalRect<i32, u32>, b: PhysicalRect<i32, u32>) -> Option<PhysicalRect<i32, u32>>` | Rectangle intersection, `None` when empty. |
| `monitor.rs:121` | `fn rect_right(rect: PhysicalRect<i32, u32>) -> i32` | Saturating right edge. |
| `monitor.rs:128` | `fn rect_bottom(rect: PhysicalRect<i32, u32>) -> i32` | Saturating bottom edge. |
| `taskbar.rs:9` | `pub(super) fn set_progress_bar(state: ProgressBarState)` | Updates the thread-local Unity launcher indicator. |
| `taskbar.rs:13` | `pub(super) fn set_badge_count(count: Option<i64>, desktop_filename: Option<String>)` | Updates the Unity launcher count. |
| `taskbar.rs:18` | `struct UnityLib` (`#[derive(WrapperApi)]`) | `libunity` entry points loaded with `dlopen2`. |
| `taskbar.rs:30` | `struct TaskbarIndicator { desktop_filename, desktop_filename_c_str, unity_lib, attempted_load, unity_inspector, unity_entry }` | Lazily-loaded Unity state. |
| `taskbar.rs:42` | `static TASKBAR_INDICATOR: RefCell<TaskbarIndicator>` (`thread_local!`) | Per-thread indicator. |
| `taskbar.rs:46` | `fn new() -> Self` | Empty state. |
| `taskbar.rs:59` | `fn ensure_lib_load(&mut self)` | Tries `libunity.so.4`, `.6`, `.9`; resolves the inspector. |
| `taskbar.rs:81` | `fn ensure_entry_load(&mut self)` | Resolves the launcher entry for the desktop file id. |
| `taskbar.rs:92` | `fn is_unity_running(&self) -> bool` | `unity_inspector_get_unity_running == 1`. |
| `taskbar.rs:102` | `fn update(&mut self, progress: ProgressBarState)` | Sets progress (0–100 → 0.0–1.0) and visibility. |
| `taskbar.rs:143` | `fn update_count(&mut self, count: Option<i64>, desktop_filename: Option<String>)` | Sets/clears the launcher count. |
| `utils.rs:13-19` | `NET_WM_STATE_REMOVE/ADD`, `NET_ACTIVE_WINDOW_SOURCE_PAGER`, `CURRENT_TIME`, `CLIENT_MESSAGE`, `SUBSTRUCTURE_REDIRECT_MASK`, `SUBSTRUCTURE_NOTIFY_MASK` | EWMH constants. |
| `utils.rs:21` | `static XLIB: LazyLock<Option<xlib::Xlib>>` | `libX11` handle. |
| `utils.rs:26` | `static DISPLAY: RefCell<Option<Display>>` (`thread_local!`) | Lazily opened X display. |
| `utils.rs:29` | `pub(super) fn with_cef_display<R>(default: R, f: impl FnOnce(&xlib::Xlib, *mut xlib::Display) -> R) -> R` | Runs `f` on CEF's own X display (`cef::get_xdisplay()`), flushing after. |
| `utils.rs:48` | `pub(super) fn with_x11<R>(default: R, f: impl FnOnce(&xlib::Xlib, *mut xlib::Display) -> R) -> R` | Runs `f` on the runtime's own X display, flushing after. |
| `utils.rs:72` | `pub(super) fn atom(xlib: &xlib::Xlib, display: *mut xlib::Display, name: &str) -> c_ulong` | `XInternAtom`. |
| `utils.rs:84` | `pub(super) fn activate_window(xid: c_ulong)` | `XRaiseWindow` + `_NET_ACTIVE_WINDOW` client message with source indication 2 ("pager") to defeat focus-stealing prevention. |
| `utils.rs:121` | `pub(super) fn set_wm_state(xid: c_ulong, add: bool, atom1: &str, atom2: Option<&str>)` | `_NET_WM_STATE` client message (add/remove). |

---

## 2. `WindowDispatch for CefWindowDispatcher`

```rust
#[derive(Debug, Clone)]
pub struct CefWindowDispatcher<T: UserEvent> {
  pub(crate) window_id: WindowId,
  pub(crate) context: RuntimeContext<T>,
}
```

`impl<T: UserEvent> WindowDispatch<T> for CefWindowDispatcher<T>` starts at line 1336 with
`type Runtime = CefRuntime<T>; type WindowBuilder = WindowBuilderWrapper;`.

The trait in this workspace (`source/tauri/crates/tauri-runtime/src/lib.rs:816-1190`) has **80**
required members; `activity_name` (`#[cfg(target_os = "android")]`, 958) and `scene_identifier`
(`#[cfg(target_os = "ios")]`, 962) are compiled out on desktop, so **78** are implemented here — the
full desktop surface. There is no `on_window_event` member in this `tauri-runtime` version. Every
member maps to exactly one `WindowMessage` variant.

Two dispatch shapes are used:

* **getter** — `window_getter!(self, Variant)` → `mpsc::channel` → `context.send_message(...)` →
  `receiver.recv()`, mapping a dropped sender to `Error::FailedToReceiveMessage`.
* **setter/action** — `self.context.send_message(Message::Window { window_id, message: … })`,
  returning `Result<()>` for the *queueing* only; failures happen on the event loop and are not
  reported back.

`handle_window_message` (979) is the only consumer; the per-member "effect" column below is what that
match arm does.

| Line | Member | Effect on the event loop | Platform gating |
| --- | --- | --- | --- |
| 1340 | `run_on_main_thread<F: FnOnce() + Send + 'static>(&self, f: F) -> Result<()>` | `context.run_on_main_thread(f)` — no window involvement. | none |
| 1344 | `scale_factor(&self) -> Result<f64>` | `window.scale_factor()`. | none |
| 1348 | `inner_position(&self) -> Result<PhysicalPosition<i32>>` | `window.surface_position()` (client area origin). | none |
| 1352 | `outer_position(&self) -> Result<PhysicalPosition<i32>>` | `window.outer_position()`, mapped to `Error::FailedToGetMonitor` on failure. | none |
| 1356 | `inner_size(&self) -> Result<PhysicalSize<u32>>` | `window.surface_size()`. | none |
| 1360 | `outer_size(&self) -> Result<PhysicalSize<u32>>` | `window.outer_size()`. | none |
| 1364 | `is_fullscreen(&self) -> Result<bool>` | `window.fullscreen().is_some()`. | none |
| 1368 | `is_minimized(&self) -> Result<bool>` | `window.is_minimized().unwrap_or(false)`. | none |
| 1372 | `is_maximized(&self) -> Result<bool>` | `window.is_maximized()`. | none |
| 1376 | `is_focused(&self) -> Result<bool>` | `window.has_focus()`. | none |
| 1380 | `is_decorated(&self) -> Result<bool>` | `window.is_decorated()`. | none |
| 1384 | `is_resizable(&self) -> Result<bool>` | `window.is_resizable()`. | none |
| 1388 | `is_maximizable(&self) -> Result<bool>` | `enabled_buttons().contains(MAXIMIZE)`. | trait doc: unsupported on Linux |
| 1392 | `is_minimizable(&self) -> Result<bool>` | `enabled_buttons().contains(MINIMIZE)`. | trait doc: unsupported on Linux |
| 1396 | `is_closable(&self) -> Result<bool>` | `enabled_buttons().contains(CLOSE)`. | none |
| 1400 | `is_visible(&self) -> Result<bool>` | `window.is_visible().unwrap_or(true)`. | none |
| 1404 | `is_enabled(&self) -> Result<bool>` | `AppWindow::is_enabled()` (per-OS impl). | none |
| 1408 | `is_always_on_top(&self) -> Result<bool>` | `attrs.inner.window_level == AlwaysOnTop` (state, not a native query). | none |
| 1412 | `title(&self) -> Result<String>` | `window.title()`. | none |
| 1416 | `current_monitor(&self) -> Result<Option<Monitor>>` | `window.current_monitor()` → `winit_monitor_to_tauri_monitor`. | none |
| 1420 | `primary_monitor(&self) -> Result<Option<Monitor>>` | `window.primary_monitor()` → same. | none |
| 1424 | `monitor_from_point(&self, x: f64, y: f64) -> Result<Option<Monitor>>` | Hand-built message; first monitor whose rect contains the point (inclusive upper bounds here). | none |
| 1436 | `available_monitors(&self) -> Result<Vec<Monitor>>` | `window.available_monitors()` → same. | none |
| 1447 | `gtk_window(&self) -> Result<*mut std::ffi::c_void>` | `gtk_window()` + `to_glib_full()` → `SendGtkWindow` (**transfer full**). | Linux/BSD only |
| 1458 | `default_vbox(&self) -> Result<*mut std::ffi::c_void>` | `cef_host.default_vbox()` + `to_glib_full()` → `SendGtkBox`. | Linux/BSD only |
| 1462 | `window_handle(&self) -> std::result::Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError>` | `window.window_handle()` → `SendRawWindowHandle` → `WindowHandle::borrow_raw`; any error becomes `HandleError::Unavailable`. | none |
| 1470 | `theme(&self) -> Result<Theme>` | `window.theme()` mapped, defaulting to `Theme::Light`. | none |
| 1474 | `center(&self) -> Result<()>` | `AppWindow::center()`. | none |
| 1481 | `request_user_attention(&self, request_type: Option<UserAttentionType>) -> Result<()>` | `window.request_user_attention(Critical/Informational/None)`. | none |
| 1488 | `create_window<F: Fn(RawWindow<'_>) + Send + 'static>(&mut self, pending: PendingWindow<T, Self::Runtime>, after_window_creation: Option<F>) -> Result<DetachedWindow<T, Self::Runtime>>` | `create_window_detached(&self.context, pending, after_window_creation)`. | none |
| 1496 | `create_webview(&mut self, pending: PendingWebview<T, Self::Runtime>) -> Result<DetachedWebview<T, Self::Runtime>>` | `create_webview_detached(&self.context, self.window_id, pending)`. | none |
| 1503 | `set_resizable(&self, resizable: bool) -> Result<()>` | `window.set_resizable`. | none |
| 1510 | `set_enabled(&self, enabled: bool) -> Result<()>` | `AppWindow::set_enabled`. | none |
| 1517 | `set_maximizable(&self, maximizable: bool) -> Result<()>` | Toggles the `MAXIMIZE` flag in `enabled_buttons()`. | none |
| 1524 | `set_minimizable(&self, minimizable: bool) -> Result<()>` | Toggles `MINIMIZE`. | none |
| 1531 | `set_closable(&self, closable: bool) -> Result<()>` | Toggles `CLOSE`. | none |
| 1538 | `set_title<S: Into<String>>(&self, title: S) -> Result<()>` | `window.set_title(&title)`. | none |
| 1545 | `maximize(&self) -> Result<()>` | `set_maximized(true)`. | none |
| 1552 | `unmaximize(&self) -> Result<()>` | `set_maximized(false)`. | none |
| 1559 | `minimize(&self) -> Result<()>` | `set_minimized(true)`. | none |
| 1566 | `unminimize(&self) -> Result<()>` | `set_minimized(false)`. | none |
| 1573 | `show(&self) -> Result<()>` | `set_visible(true)`. | none |
| 1580 | `hide(&self) -> Result<()>` | `set_visible(false)`. | none |
| 1587 | `close(&self) -> Result<()>` | `request_window_close(window_id, event_loop)` (runs the close-request pipeline, may be vetoed). | none |
| 1594 | `destroy(&self) -> Result<()>` | `close_window(window_id, event_loop)` (unconditional teardown). | none |
| 1601 | `set_decorations(&self, decorations: bool) -> Result<()>` | `window.set_decorations`, then (macOS) `apply_movability()`. | none |
| 1608 | `set_shadow(&self, enable: bool) -> Result<()>` | Windows: `set_undecorated_shadow`; macOS: `set_has_shadow`; Linux: nothing. | Windows + macOS |
| 1615 | `set_always_on_bottom(&self, value: bool) -> Result<()>` | `window_level = AlwaysOnBottom \| Normal`, stored in attrs and applied. | none |
| 1622 | `set_always_on_top(&self, value: bool) -> Result<()>` | `window_level = AlwaysOnTop \| Normal`, stored and applied. | none |
| 1629 | `set_visible_on_all_workspaces(&self, value: bool) -> Result<()>` | Stores the flag and calls `AppWindow::set_visible_on_all_workspaces` (macOS, Linux/BSD). | macOS + Linux/BSD |
| 1636 | `set_content_protected(&self, protected: bool) -> Result<()>` | `window.set_content_protected`. | none (winit no-ops where unsupported) |
| 1643 | `set_size(&self, size: Size) -> Result<()>` | `window.request_surface_size(size)` (client size). | none |
| 1650 | `set_min_size(&self, size: Option<Size>) -> Result<()>` | `set_min_surface_size`. | none |
| 1657 | `set_max_size(&self, size: Option<Size>) -> Result<()>` | `set_max_surface_size`. | none |
| 1664 | `set_size_constraints(&self, constraints: WindowSizeConstraints) -> Result<()>` | `min_size_constraint`/`max_size_constraint` → min/max surface size (per-axis constraints are collapsed; TODO to upstream). | none |
| 1671 | `set_position(&self, position: Position) -> Result<()>` | `set_outer_position`. | none |
| 1678 | `set_fullscreen(&self, fullscreen: bool) -> Result<()>` | `request_window_fullscreen(fullscreen.then_some(CurrentMonitor))`. | none |
| 1685 | `set_fullscreen_on_monitor(&self, position: PhysicalPosition<f64>) -> Result<()>` | `request_window_fullscreen(Some(MonitorAt(position)))`. | none |
| 1693 | `set_simple_fullscreen(&self, enable: bool) -> Result<()>` | `window.set_simple_fullscreen(enable)`; leaving also re-applies movability. | **macOS only** (`#[cfg]` on the member) |
| 1700 | `set_focus(&self) -> Result<()>` | `AppWindow::activate()` (native nudge + focus). | none |
| 1707 | `set_focusable(&self, focusable: bool) -> Result<()>` | `WindowMessage::SetFocusable` → **no-op** (TODO in source). | none |
| 1714 | `set_icon(&self, icon: Icon) -> Result<()>` | `tauri_icon_to_winit_icon` then `set_window_icon`; invalid icons are dropped silently. | none |
| 1721 | `set_skip_taskbar(&self, skip: bool) -> Result<()>` | Windows: `window.set_skip_taskbar`; Linux/BSD: `AppWindow::set_skip_taskbar` (`_NET_WM_STATE_SKIP_TASKBAR`). | Windows + Linux/BSD |
| 1728 | `set_cursor_grab(&self, grab: bool) -> Result<()>` | `set_cursor_grab(Confined \| None)`; the error is discarded. | none |
| 1735 | `set_cursor_visible(&self, visible: bool) -> Result<()>` | `set_cursor_visible`. | none |
| 1742 | `set_cursor_icon(&self, icon: CursorIcon) -> Result<()>` | `tauri_cursor_to_winit_cursor` → `set_cursor`. | none |
| 1749 | `set_cursor_position<Pos: Into<Position>>(&self, position: Pos) -> Result<()>` | `set_cursor_position` (warps the OS cursor). | none |
| 1756 | `set_ignore_cursor_events(&self, ignore: bool) -> Result<()>` | `set_cursor_hittest(!ignore)`. | none |
| 1763 | `start_dragging(&self) -> Result<()>` | `window.drag_window()`. | none |
| 1770 | `start_resize_dragging(&self, direction: tauri_runtime::ResizeDirection) -> Result<()>` | `drag_resize_window(direction)`. | none |
| 1777 | `set_badge_count(&self, count: Option<i64>, desktop_filename: Option<String>) -> Result<()>` | `event_loop.set_badge_count` (macOS dock badge / Linux Unity count / Windows no-op). | none (per-OS behavior) |
| 1784 | `set_badge_label(&self, label: Option<String>) -> Result<()>` | `event_loop.set_badge_label` (macOS dock label). | macOS real |
| 1791 | `set_overlay_icon(&self, icon: Option<Icon>) -> Result<()>` | Windows: `AppWindow::set_overlay_icon`; elsewhere nothing. | Windows only |
| 1798 | `set_progress_bar(&self, progress_state: ProgressBarState) -> Result<()>` | macOS: `event_loop.set_progress_bar` (dock); else `AppWindow::set_progress_bar` (Windows taskbar / Linux Unity). | per-OS impl |
| 1805 | `set_title_bar_style(&self, style: tauri_utils::TitleBarStyle) -> Result<()>` | macOS: `AppWindow::set_title_bar_style`; elsewhere nothing. | macOS only |
| 1812 | `set_traffic_light_position(&self, position: Position) -> Result<()>` | macOS: stores the position and calls `AppWindow::set_traffic_light_position`; elsewhere nothing. | macOS only |
| 1819 | `set_theme(&self, theme: Option<Theme>) -> Result<()>` | `AppWindow::set_theme` (winit theme + CEF request-context scheme + macOS traffic-light reapply). | none |
| 1826 | `set_background_color(&self, color: Option<Color>) -> Result<()>` | Stores the color and calls `AppWindow::set_background_color` (per-OS). | none |

Non-`WindowDispatch` producers of `WindowMessage`: `WindowMessage::ToggleFullscreen` (F11,
`IDC_FULLSCREEN`, runtime-internal), `WindowMessage::ApplyPendingFullscreen` (macOS transition
observer), `WindowMessage::Close`/`Destroy` from the window event pipeline.

---

## 3. `NativeWindowToken`

```rust
/// Opaque identity of one runtime-owned native window lifetime. Reparented
/// webviews observe the destination token; same-label replacements never match.
#[derive(Clone)]
pub struct NativeWindowToken(Arc<()>);
```

* **Identity carried** — nothing but the address of a private `Arc<()>` allocation. One token is
  minted per *native window lifetime* (`NativeWindowToken::new()` in `AppWindow` construction,
  `window.rs:811`), never per label, never per handle value. A window that is destroyed and recreated
  with the same label gets a different token.
* **Equality** — `impl PartialEq` compares with `Arc::ptr_eq(&self.0, &other.0)` (432-436);
  `impl Eq` (438). Clones share the allocation and therefore compare equal; two independently minted
  tokens never do, even after the original was dropped (asserted by the test at 453-461).
* **Debug** — `finish_non_exhaustive()` on a `debug_struct("NativeWindowToken")`, so the output is
  `NativeWindowToken { .. }` and never leaks an address (440-446).
* **Constructor** — `pub(crate) fn new()` (427); the type is publicly nameable but only the runtime
  can mint one.
* **Consumers** — `AppWindow::lifetime` (`window.rs:469`), `WebviewSnapshot.window:
  Option<NativeWindowToken>` (`webview.rs:66`, filled from `appwindow.lifetime.clone()` at
  `webview.rs:1199`), and `PopupFamily::windows: Mutex<Vec<(Window, NativeWindowToken)>>`
  (`popup.rs:48`, minted per native CEF popup *window* at `popup.rs:284` and shared by every popup
  tab in that window).
* **Exported** — `lib.rs:53`: `pub use window::{CefWindowDispatcher, NativeWindowToken};` →
  `ElectronWindowDispatcher`, `NativeWindowToken` (unchanged name).

Electron feasibility: `direct`. The mirror mints the token in its own window record; Electron's
`BrowserWindow.id`/`webContents.id` are *not* suitable substitutes because they are reused across
recreation and are not opaque. Nothing about the token needs an Electron API.

---

## 4. `popup.rs`

### 4.1 What the reference file actually is

`popup.rs` is **CEF-owned popup lifetime management**, not `window.open` feature parsing. CEF creates
the popup's native window itself (the runtime installs no `OnBeforePopup`; a crate-wide search for
`WindowFeatures`, `window_features`, `noopener`, `noreferrer`, `window.open` and `features` finds only
the Chromium `--enable-features`/`--disable-features` switch handling in `runtime.rs`). The runtime
therefore only *observes* what CEF created: it reserves a slot before creation, adopts the browser
when it appears, and revokes the family when an opener dies.

> **Premise correction for the deliverable.** The task brief describes `popup.rs` as containing "the
> window-feature parsing rules (which `window.open` features map to which window attributes)". The
> reference contains no such code. The *target* crate's current `src/popup.rs` is a feature parser
> (71 lines, `window_features(&str) -> (Option<LogicalSize<f64>>, Option<LogicalPosition<f64>>)`);
> that is a divergence to be reconciled, not a mirror of this file. Both are documented below so the
> implementer can decide deliberately: §4.2–4.5 = the reference semantics that must be mirrored;
> §4.6 = the target's current parser (for the record); §4.7 = how Electron actually routes popups and
> parses features, which is what an Electron-side mirror has to build on.

### 4.2 Admission, reservation, creation

* `const MAX_POPUPS: usize = 128` (20) bounds **both** the live popups and the pending reservations
  of one family, and separately the tracked popup windows in `observe()`.
* `admits(opener, browser_id)` (62-74) is true only when: the family is not closing; the opener state
  has observed exactly that `browser_id`; fewer than `MAX_POPUPS` live popups; **and** the opener is
  either the family root or a live (not closing) popup's own browser. A popup therefore only opens
  popups from its own live browser, not from a stale identity.
* `reserve(opener, browser_id, popup_id)` (76-100) re-checks `admits`, then refuses when
  `pending.len() + popups.len() >= MAX_POPUPS` or when a reservation with the same `popup_id` **and**
  the same opener browser already exists. On success it pushes a `PopupRequest` whose `identity` is a
  fresh `Arc<()>` and returns a clone.
* `abort(opener, popup_id)` (102-108) removes the reservation matching that `(popup_id, opener
  browser)` pair and returns it; `None` when CEF never reserved it.
* `created(browser, request, state)` (110-164): the reservation must still be present (`is_same`
  identity match) **and** `admits(request.opener, host.opener_identifier())` must hold; otherwise the
  new browser is closed (`state.close(); host.close_browser(1)`). On success it creates a
  `DialogState`, installs the runtime's **private** devtools observer with empty app handler lists
  (`add_dev_tools_observer(browser, Arc::default(), Arc::default(), dialogs)`), and pushes the
  `Popup`. Every failure path closes the browser.

### 4.3 Teardown

* `closed(state, browser_id)` (168-201) — the per-browser native callback. Guarded by
  `state.has_browser_id(browser_id)`; marks the state closed; if the state *is* the root, sets
  `closing`; then `revoke_descendants` marks and closes every descendant (including out-of-order
  arrivals) and the resulting browsers are closed with `host.close_browser(1)`. No family lock is held
  across a CEF call.
* `close_all()` (208-234) — window close and app shutdown own the whole family: sets `closing`,
  closes the root state **without** requiring an observed identity, revokes every descendant, closes
  their browsers. The doc explains why: a root that was exhausted before its first frame event would
  otherwise leave every popup open and the event loop waiting forever.
* `is_revoked()` (240-242) — `closing` flag. A revoked family admits nothing, so nothing it still has
  reserved can ever be created; CEF does not always report the abort of a discarded popup, so this is
  what keeps the event loop from waiting on those reservations.
* `revoke_descendants` (333-363) — pure graph walk (no CEF calls): starting from the revoked state,
  repeatedly mark any node whose opener is already revoked (or every node when `root_closed`), until
  a fixed point. Returns the indices to close. The test at 379-403 deliberately puts a descendant
  before its opener to prove order independence.

### 4.4 `observe() -> Vec<Webview>` (244-328)

Snapshots every live popup into a `Webview`:

1. Returns empty when the family is closing.
2. Clones the popup list and the tracked `(Window, NativeWindowToken)` list; drops tracked windows
   that are invalid or closed.
3. Per popup: skip when closing or the browser is invalid; resolve `browser_view_get_for_browser`;
   resolve the view's `Window`; map that window to a token — reuse the existing token when the same
   native window was seen before (**several popup tabs may share one CEF window; window identity is
   owned by the family, never minted per tab**), else mint one unless `MAX_POPUPS` windows are already
   tracked.
4. `bounds` = the view's `bounds()` as a logical `Rect`; `visible` =
   `Some(view.is_drawn() && window.is_visible() && !window.is_minimized())`, or `None` when either the
   view or the window could not be observed.
5. `document` = `popup.state.observe_document(&browser)`; `dialogs` = `popup.dialogs.snapshot(document)`.
6. Builds `WebviewSnapshot { browser_id, dialogs, document, window_label: None, window,
   parent_matches: None, bounds, visible }`. `window_label` is always `None` (a CEF popup has no Tauri
   window label) and `parent_matches` is always `None` — the runtime never reparents a popup, so there
   is no independently observed parent to check against and reporting `Some(true)` would assert
   something nothing verified.
7. `Webview::new(browser, snapshot, popup.state.clone())`, then `set_opener(popup.opener.clone())`.
8. Writes the (possibly grown) window list back.

### 4.5 Tests (366-466)

`closing_an_opener_revokes_only_its_exact_descendants` (379), `pending_popups_are_bounded_and_retained_until_exact_creation_or_abort`
(405), `teardown_revokes_a_family_still_holding_an_unresolved_reservation` (422),
`popup_admission_requires_the_live_exact_native_opener` (440),
`teardown_closes_a_family_whose_root_never_observed_a_browser_id` (454). These encode the invariants
above and should be ported with the module.

### 4.6 The target's current parser (divergence, for the record)

`src/popup.rs` today: `pub(crate) fn window_features(features: &str) -> (Option<LogicalSize<f64>>,
Option<LogicalPosition<f64>>)` with these exact rules:

| Input | Rule |
| --- | --- |
| Splitting | `split(',')`; a part without `=` is skipped entirely (a bare `popup` flag is ignored). |
| Value parsing | `value.trim().parse::<f64>()`; a parse failure skips the pair. |
| Size keys | `width` \| `innerWidth` → width; `height` \| `innerHeight` → height. |
| Position keys | `left` \| `screenX` \| `x` → x; `top` \| `screenY` \| `y` → y. |
| Pairing | A `LogicalSize` is returned only when **both** width and height were seen; a `LogicalPosition` only when both x and y were seen. |
| Everything else | Ignored (no `menubar`, `toolbar`, `resizable`, `noopener`, … handling). |

It is called from `src/runtime.rs:904` and `src/runtime.rs:2039` to fill
`NewWindowFeatures::new(size, position, opener)` — the only two fields `NewWindowFeatures` carries
(`source/tauri/crates/tauri-runtime/src/webview.rs:160-165`).

### 4.7 Electron's own routing and feature parsing (what an Electron mirror must build on)

* `webContents.setWindowOpenHandler(handler)` (`source/electron/docs/api/web-contents.md:1461`) is
  called **synchronously** from `-will-add-new-contents` with
  `details = { url, frameName, features, disposition, referrer, postBody }`
  (`source/electron/lib/browser/api/web-contents.ts:261-300`). Returning `{action:'deny'}` blocks;
  `{action:'allow', overrideBrowserWindowOptions}` lets Electron create the window.
* Electron itself does **not** create the window in C++: `WebContents::AddNewContents` emits
  `-add-new-contents` with `window_features.bounds` and the raw features string
  (`shell/browser/api/electron_api_web_contents.cc:1612-1660`), and the JS layer creates it in
  `openGuestWindow` (`lib/browser/guest-window-manager.ts:45-81`) as
  `{ show: false, width: 800, height: 600, ...parseFeatures(features).options,
  ...overrideBrowserWindowOptions }` — i.e. **defaults 800×600, features next, app overrides last**.
* `parseFeatures` (`lib/browser/parse-features-string.ts:153-183`) is the authoritative feature map:
  `left`→`x`, `top`→`y`; `resizable` is dropped (spec: windows are always resizable); `width`,
  `height`, `innerWidth`, `innerHeight`, `x`, `y`, `minWidth`, `minHeight`, `maxWidth`, `maxHeight`,
  `opacity` are `parseInt`ed; every other value coerces `true`/`1`/`yes`/absent → `true`,
  `false`/`0`/`no` → `false`, else the raw string. Only keys in `allowedWindowOptions` survive
  (`top`, `left`, `innerWidth`, `innerHeight`, `x`, `y`, `width`, `height`, `minWidth`, `minHeight`,
  `maxWidth`, `maxHeight`, `opacity`, `show`, `center`, `useContentSize`, `frame`, `transparent`,
  `hasShadow`, `movable`, `closable`, `focusable`, `minimizable`, `maximizable`, `fullscreenable`,
  `alwaysOnTop`, `skipTaskbar`, `modal`, `acceptFirstMouse`, `autoHideMenuBar`,
  `enableLargerThanScreen`, `paintWhenInitiallyHidden`, `roundedCorners`, `thickFrame`,
  `disableAutoHideCursor`, `hiddenInMissionControl`, `title`, `backgroundColor`, `tabbingIdentifier`,
  `titleBarStyle`, `vibrancy`, `visualEffectState`, `backgroundMaterial`); `zoomFactor`,
  `nodeIntegration`, `javascript`, `contextIsolation`, `webviewTag` are split off into
  `webPreferences` instead. Note the documented deviation: Electron maps `width`/`height` to the
  **outer** size, not to the content size the spec requires.
* `electron-rs` exposes this as `webview.newWindowResponse { id, response: NewWindowDecision }` and
  `window.create { …, new_window_request: Option<u64> }` (`rpc.rs:359-372, 642`), so the runtime can
  answer the handler with `deny` and create the window itself through its normal path — which is what
  the target's `src/popup.rs` module doc already says it does.

---

## 5. `offscreen.rs`

Purpose: windowless (OSR) browsers. The application owns painting, input/IME forwarding and
composition; Tauri keeps IPC, custom protocols and browser lifecycle. Offscreen webviews use
**Alloy** runtime style and cannot be reparented. The app must enable
`settings.windowless_rendering_enabled` through `Cef::with_settings` before creating one, receives
native input/redraws through `Cef::on_window_event`, and reaches CEF's host API through
`Webview::browser`.

### 5.1 Public API

| Item | Signature | Semantics |
| --- | --- | --- |
| `CursorHandler` | `pub type CursorHandler = dyn Fn(cef::CursorType, Option<&cef::CursorInfo>) + Send + Sync;` | Cursor-change notification delivered on the CEF UI thread. Any custom cursor buffer is valid **only during the callback**. |
| `Offscreen::shared_texture_enabled` | `pub shared_texture_enabled: bool` | Delivers shared GPU resources through `RenderHandler::on_accelerated_paint`; open/copy them during the callback, respecting CEF's resource lifetime. Default `false`. |
| `Offscreen::external_begin_frame_enabled` | `pub external_begin_frame_enabled: bool` | The application must call `BrowserHost::send_external_begin_frame` to produce frames. Default `false`. |
| `Offscreen::cursor_handler` | `pub cursor_handler: Option<Arc<CursorHandler>>` | Handles cursor changes; every other display notification stays with Tauri. Default `None`. |
| `Offscreen::new` | `pub fn new<F: Fn(OffscreenView) -> cef::RenderHandler + Send + Sync + 'static>(handler: F) -> Self` | Creates one render handler **per webview**, on the CEF UI thread. The handler must implement `view_rect` and a paint callback, read bounds/scale from the `OffscreenView`, and call `request_redraw()` after painting. CEF view rects are logical pixels, paint buffers physical. CEF callbacks must not block on queued Tauri window operations. |
| `OffscreenView::window` | `pub fn window(&self) -> Option<Arc<dyn winit::window::Window>>` | Native host used for scale, screen coordinates, IME and redraw requests; `None` once the runtime released the window. Do not keep it past `Destroyed`. |
| `OffscreenView::request_redraw` | `pub fn request_redraw(&self)` | Asks the host window to redraw, if it still exists. |
| `OffscreenView::bounds` | `pub fn bounds(&self) -> Rect` | Bounds relative to the host, in logical pixels. |
| `OffscreenView::is_visible` | `pub fn is_visible(&self) -> bool` | Requested visibility, independent of native window occlusion. |
| `Offscreen::window_info` | `pub(crate) fn window_info(&self, parent: cef::sys::cef_window_handle_t) -> cef::WindowInfo` | Internal: `set_as_windowless(parent)` (windowless rendering on, `RuntimeStyle::ALLOY`) plus the two paint flags. |

All fields of `OffscreenView` are `pub(crate)`; the struct is `Clone` and its only public surface is
the four methods above. `Offscreen::render_handler` is `pub(crate)`.

### 5.2 Electron feasibility

| Item | Tag | Mechanism / note |
| --- | --- | --- |
| `Offscreen` (as a mode) | `emulated` | Electron OSR is a **webPreferences** flag, not a separate browser: `webPreferences.offscreen: true` (or `{ useSharedTexture: true, sharedTexturePixelFormat }`), documented at `docs/api/structures/web-preferences.md`. Frames arrive as `webContents.on('paint', (event, dirty, image))`; `setFrameRate(fps)`, `startPainting()`, `stopPainting()`, `isPainting()`, `invalidate()`, `isOffscreen()` exist. There is still a `BrowserWindow` behind it (hide it), unlike CEF's windowless browser. |
| `shared_texture_enabled` | `direct` | `webPreferences.offscreen.useSharedTexture` + the `texture` argument of the `paint` event (`OffscreenSharedTexture`). Requires the `paint` listener to release the texture. |
| `external_begin_frame_enabled` | `absent` | Electron has no `sendExternalBeginFrame`; `webContents.invalidate()` + `setFrameRate` is the closest control. |
| `cursor_handler` | `absent` | Electron's OSR surfaces no cursor-type notification; the CSS cursor stays inside the renderer. The type can be kept and never invoked, or dropped — see §7.5. |
| `Offscreen::new(render_handler)` | `emulated` | The `cef::RenderHandler` parameter cannot be mirrored (CEF type). Mirror as a paint-callback trait fed by the `paint` event; `view_rect` becomes "the bounds the runtime already tracks". |
| `OffscreenView::window` | `absent` | There is no winit window. Returning `None` always is the honest mirror; the scale/IME duties move to the Electron window record. |
| `OffscreenView::request_redraw` | `direct` | `webContents.invalidate()`. |
| `OffscreenView::bounds` | `direct` | The runtime's own tracked bounds (`webview.setBounds` bookkeeping). |
| `OffscreenView::is_visible` | `direct` | The runtime's own visibility flag (`webview.visibility`). |

---

## 6. `platform/` — public items, mechanisms, visibility

`platform` is a **private** module. The only crate-root path into it is
`prepare_macos_application` (`lib.rs:47`, macOS only). Everything else is `pub(crate)`, `pub(super)`
or module-private. `pub mod macos;` / `pub mod linux;` are `pub` only inside the private parent, so
they are not reachable from outside the crate.

### 6.1 `platform/mod.rs`

| Item | Visibility | Mechanism | Electron feasibility |
| --- | --- | --- | --- |
| `MonitorExt::work_area` | `pub(crate)` trait | Per-OS native shim because winit has no work-area API (Windows `GetMonitorInfoW`, macOS `NSScreen.visibleFrame`, Linux `_NET_WORKAREA`). | `direct` — Electron's `Display.workArea` already carries it (`window.displays`, `window.currentDisplay`, `window.displayForPoint`). The trait can keep its shape with an Electron-backed implementation; the native shims are unnecessary. |
| `monitor_bounds` | private | Full monitor rect used as the work-area fallback. | `direct` (`Display.bounds`). |
| `EventLoopExt::set_activation_policy` | `pub` in private module, macOS | `NSApplication.setActivationPolicy`. | `direct` — `app.activationPolicy` (`app::SetActivationPolicy`). |
| `EventLoopExt::set_dock_visibility` | macOS | `TransformProcessType` + Electron's own duplicate-icon workaround. | `direct` — `app.dockVisibility` (`app.dock.hide()/show()`); Electron implements the same workaround internally. |
| `EventLoopExt::show_application` | macOS | `NSApp.unhide`. | `direct` — `app.show`. |
| `EventLoopExt::hide_application` | macOS | `NSApp.hide`. | `direct` — `app.hide`. |
| `EventLoopExt::set_progress_bar` | macOS | Dock-tile `NSProgressIndicator` subclass. | `direct` — `window.progressBar` (Electron's dock tile implementation is the same design, `native_window_mac.mm:1246`). |
| `EventLoopExt::set_badge_count` | all | macOS → badge label; Linux → Unity count; Windows → no-op. | `direct` — `window.badgeCount` (app-level). |
| `EventLoopExt::set_badge_label` | all | macOS `NSDockTile.setBadgeLabel`; Linux/Windows no-op. | `direct` — `app.badgeLabel`. |
| `EventLoopExt::cursor_position` | all | Windows `GetCursorPos`; macOS `NSEvent.mouseLocation` flipped + scaled; Linux `XQueryPointer`. | `direct` — `window.cursorPosition`. |

### 6.2 `platform/windows/`

All items are `pub(crate)`/private inside a private module. Mechanisms: Win32 (`EnableWindow`,
`IsWindowEnabled`, `ITaskbarList3`, `DwmGetWindowAttribute`, `SetWindowSubclass`, `SetWindowPos`,
`SetParent`, `ShowWindow`, `DestroyWindow`, `GetParent`, `MapWindowPoints`, `CreateIcon`,
`GetMonitorInfoW`, `GetCursorPos`) plus `softbuffer` for the window background.

Electron feasibility, item by item:

| Item | Tag | Note |
| --- | --- | --- |
| `cef_host_handle`, `hwnd` | `direct` | `window.nativeHandle` → HWND; the mirror keeps the same accessor shape. |
| `is_enabled`, `set_enabled` | `direct` | `window.enabledQuery` / `window.enabled` (Electron `IsWindowEnabled`/`EnableWindow`, `native_window_views.cc:689,709`). |
| `set_overlay_icon` | `direct` | `window.overlayIcon` (Electron `ITaskbarList3::SetOverlayIcon`, Windows only). |
| `set_progress_bar` | `direct` | `window.progressBar` (Electron taskbar host; same `TBPF_*` states). |
| `set_background_color` | `direct` | `window.backgroundColor`. |
| `draw_background_surface` | `absent` | Electron paints the window background itself (root view + compositor background, `native_window_views.cc:1344-1347`); there is no softbuffer surface to drive. |
| `dwm_visible_frame_height` | `emulated` | Still possible (`DwmGetWindowAttribute` on the Electron HWND), but unnecessary: `window.center` already centers correctly, so `center()` should delegate instead of re-deriving the frame. |
| `native_parent_matches` | `emulated` | No native parent relationship to query: compare the runtime's own window/webview ids. |
| `native_visible` | `direct` | `webview.bounds` snapshot carries `visible`. |
| `set_background_color` (webview) | `direct` | `webview.backgroundColor`. |
| `bounds` | `direct` | `webview.bounds`. |
| `reparent` | `direct` | `webview.reparent { webview_id, window_id }`. |
| `unclip_siblings`, `pin_z_order_subclass_proc`, `set_z_order_pinned`, `raise_to_top` | `absent` | These exist because CEF creates **native child HWNDs** that Views marks `WS_CLIPSIBLINGS`. Electron's `WebContentsView` children are composited views, not HWNDs, so there is nothing to unclip or pin. |
| `apply_visible` | `direct` | `webview.visibility`. |
| `destroy_host_window` | `direct` | `webview.close`. |
| `apply_physical_bounds` | `direct` | `webview.setBounds` (`physical: true`). |
| `icon_to_hicon` | `absent` | Electron takes a `NativeImage` (`window.icon` / `window.overlayIcon`); no HICON conversion is needed. Keep the module for layout parity, or document it as unused. |
| `set_badge_count`, `set_badge_label` | `absent` | Windows has no badge; the reference no-ops too. |
| `cursor_position` | `direct` | `window.cursorPosition`. |
| `work_area` | `direct` | `Display.workArea`. |

### 6.3 `platform/macos/`

Mechanisms: `objc2`/`objc2-app-kit` (`NSApplication` subclass, `NSApplicationDelegate`, `NSWindow`
style mask, `NSWindowCollectionBehavior`, `NSDockTile`, `NSProgressIndicator`, `NSBezierPath`),
`objc2-application-services` (`TransformProcessType`), CoreFoundation run-loop observer, and
associated objects.

| Item | Tag | Note |
| --- | --- | --- |
| `setup_application` → `prepare_macos_application` | `emulated` | Electron owns `NSApplication`; a custom `NSApplication` subclass (needed only for CEF's `CefAppProtocol`) cannot be installed. The mirror must keep the public function and make it a validated no-op (assert the main thread / that Electron is the app owner). Electron's `app` events replace `AppDelegateEvent`. |
| `AppDelegate`, `AppDelegateEvent`, `set_application_event_handler` | `emulated` | `app.on('open-file' \| 'activate' \| 'before-quit')` + `app.requestSingleInstanceLock` cover the four events; the delegate class itself is `absent`. |
| `CefWinitApplication` (+ `last_dock_show`, `set_last_dock_show`) | `absent` | CEF-specific `NSApplication` subclass. The dock-show timestamp bookkeeping is internal to Electron's `app.dock` implementation. |
| `activate_application` | `direct` | `app.focus({ steal: true })` (electron-rs `app::Focus { steal }`), which uses the same `-[NSApplication activate]`-when-available logic. |
| `set_dock_visibility`, `set_dock_hide`, `set_dock_show`, `set_windows_can_hide`, `perform_delayed_dock_show`, `activate_dock` | `direct` | `app.dockVisibility`; Electron's `NativeWindowMac`/browser_mac implements exactly the workaround this file copies. |
| `set_activation_policy`, `show_application`, `hide_application`, `set_progress_bar`, `set_badge_count`, `set_badge_label`, `cursor_position` | `direct` | `app.activationPolicy`, `app.show`, `app.hide`, `window.progressBar`, `window.badgeCount`, `app.badgeLabel`, `window.cursorPosition`. |
| `FullscreenTransition` (+ ivars, `observe`, `in_progress`, `hold`, `pending`, `take_pending`) | `emulated` | Electron emits `enter-full-screen`/`leave-full-screen`; electron-rs already waits on them with a 10 s timeout (`window.ts:482-498`). Keep the type for API-shape parity but drive it from those events, not from AppKit notifications. `ApplyPendingFullscreen` has no counterpart because the mirror serializes on the event instead of holding the request. |
| `work_area` | `direct` | `Display.workArea`. |
| `progress` module (`DockProgressIndicator`, `set_dock_progress_bar`, …) | `direct` | `window.progressBar`; Electron's dock tile indicator is the same design. |
| `utils::{transform_process_type, ns_color_from_tauri_color, instant_epoch, set_associated_object, associated_object}` | `emulated`/`absent` | Process transform → `app.dockVisibility`; colors → `window.backgroundColor`; associated objects are only needed for the traffic-light observer, which Electron replaces. |
| `MainThreadWake`, `OnPass`, `on_before_sources`, `release` | `absent` | Exists to wake winit's run loop without re-entering its handler. The Electron runtime has no winit loop; its message queue plays this role. Keep the module only if the mirror keeps a main-thread wake primitive. |
| `webview` impl items (`nsview`, `native_parent_matches`, `native_visible`, `set_background_color`, `bounds`, `reparent`, `apply_visible`, `destroy_host_window`, `apply_physical_bounds`) | `direct`/`emulated` | `webview.setBounds`, `webview.visibility`, `webview.backgroundColor`, `webview.reparent`, `webview.close`; `native_parent_matches` compares runtime ids. `nsview()` has no Electron equivalent (`absent`) because the webview is a composited view, not an `NSView` the app owns. |
| `nswindow`, `nsview`, `cef_host_handle` | `emulated` | `window.nativeHandle` returns the `NSView*`; the `NSWindow` is reachable from it via AppKit on the main thread. |
| `set_enabled`, `is_enabled` | `direct` | `window.enabled` / `window.enabledQuery` — Electron's macOS implementation is the same attached-sheet trick (`native_window_mac.mm:497-520`). |
| `set_traffic_light_position` | `direct` | Electron `win.setWindowButtonPosition({x,y})` (macOS, `base-window.md:1560`); electron-rs routes `window.trafficLight` to its own `appkit.node` shim (`setTrafficLightPosition`) because it predates/parallels that API. Either route is acceptable; Electron re-applies the inset itself on relayout. |
| `reapply_traffic_light_position_after_appearance_change` | `absent` | Only needed because the reference insets `NSWindow` frames by hand; Electron's `setWindowButtonPosition` keeps its own state. |
| `set_title_bar_style` | `emulated` | Electron's `titleBarStyle` is a **constructor** option (`hidden`/`hiddenInset`/`customButtonsOnHover`) plus `titleBarOverlay` (Windows/Linux); it cannot be changed after creation. electron-rs `window.titleBarStyle` → macOS `appkit.node` `configureStyle(handle, decorated, style, hiddenTitle)` (a native shim), no-op elsewhere. So: macOS `emulated` (via shim), Windows/Linux `absent`. |
| `apply_movability` | `direct` | `win.setMovable` (macOS/Windows). |
| `set_visible_on_all_workspaces` | `direct` | `window.visibleOnAllWorkspaces` → `setVisibleOnAllWorkspaces(value, { visibleOnFullScreen: true })`. |
| `set_background_color` | `direct` | `window.backgroundColor`. |
| `TrafficLightObserver` + helpers | `absent` | Replaced by Electron's own bookkeeping (see above). |

### 6.4 `platform/linux/`

Mechanisms: GTK4 (via winit's `WindowExtGtk4`) for the widget tree and CSS backgrounds, raw Xlib
through `x11_dl` for the X11 child host, EWMH client messages, `dlopen2` for `libunity`, and
`cef::get_xdisplay()` for CEF's own display connection.

| Item | Tag | Note |
| --- | --- | --- |
| `CefX11Host` (all methods, `Drop`) | `absent` | Exists only because CEF creates native X11 child windows that must be kept under GTK's content box. Electron's `WebContentsView` is composited, so there is no host window, no 24-bit visual, no colormap, no GTK `layout` hook. Keep the type name only if the mirror wants the `AppWindow::cef_host` field to stay; otherwise the field becomes a unit placeholder. |
| `default_vbox`, `apply_transient_for`, `set_enabled`, `is_enabled`, `set_background_color` (GTK paths) | `absent` | No GTK widget tree exists. `set_visible_on_all_workspaces`/`set_skip_taskbar` (`_NET_WM_STATE_*`) are `absent` too: Electron's `setVisibleOnAllWorkspaces` works on Linux via the widget, but `setSkipTaskbar` is a no-op on Linux (`native_window_views.cc:1267` is Windows-only). |
| `xid`, `raise_native` | `emulated` | `window.focus` covers activation; the XID itself is `window.nativeHandle`. |
| `set_progress_bar` | `direct` | `window.progressBar` (Electron links `libunity` itself, `native_window_views.cc:1609`). |
| `window_xid`, `create_cef_container`, `set_cef_container_bounds` | `absent` | X11 host plumbing. |
| `webview` impl items | `direct`/`emulated` | `webview.setBounds`, `webview.visibility`, `webview.reparent`, `webview.close`; `native_parent_matches` compares runtime ids; `set_background_color` is already a no-op in the reference. |
| `set_badge_count` | `direct` | `window.badgeCount` (Electron's Linux badge uses the same Unity launcher entry). |
| `set_badge_label` | `absent` | Unsupported on Linux in the reference and in Electron. |
| `cursor_position` | `direct` | `window.cursorPosition`. |
| `work_area` (`x11_work_area`, `get_cardinal_property`, `intersect_rects`, `rect_right`, `rect_bottom`) | `direct` | `Display.workArea`; the EWMH parsing is unnecessary. |
| `taskbar` (`TaskbarIndicator`, `UnityLib`, `TASKBAR_INDICATOR`, `ensure_lib_load`, `ensure_entry_load`, `is_unity_running`, `update`, `update_count`) | `direct` | `window.progressBar` / `window.badgeCount`; Electron owns the `libunity` loading. |
| `utils` (`with_x11`, `with_cef_display`, `atom`, `activate_window`, `set_wm_state`) | `absent`/`emulated` | `activate_window` → `window.focus`; `set_wm_state` → `setVisibleOnAllWorkspaces`/`setSkipTaskbar` (the latter is a no-op on Linux); the rest is X11 plumbing with no Electron counterpart. |

---

## 7. Electron feasibility notes

### 7.1 Every `WindowDispatch` member

| Member | Tag | Concrete Electron / electron-rs mechanism |
| --- | --- | --- |
| `run_on_main_thread` | `direct` | The runtime's task queue (`Message::Task`); Electron's main process is the only thread that touches windows. |
| `scale_factor` | `direct` | `window.geometry` → `scaleFactor` (electron-rs derives it from `devicePixelRatio`/display scale). |
| `inner_position` | `direct` | `window.geometry` → `contentBounds.{x,y}`. |
| `outer_position` | `direct` | `window.geometry` → `bounds.{x,y}`. |
| `inner_size` | `direct` | `window.geometry` → `contentBounds.{width,height}`. |
| `outer_size` | `direct` | `window.geometry` → `bounds.{width,height}`. |
| `is_fullscreen` | `direct` | `window.geometry` → `fullscreen` (includes macOS simple fullscreen). |
| `is_minimized` | `direct` | `window.geometry` → `minimized`. |
| `is_maximized` | `direct` | `window.geometry` → `maximized`. |
| `is_focused` | `direct` | `window.geometry` → `focused`. |
| `is_decorated` | `emulated` | `window.geometry` → `decorated`; macOS queries AppKit, Windows/Linux report the runtime's stored flag (Electron cannot query the frame). |
| `is_resizable` | `direct` | `window.geometry` → `resizable`. |
| `is_maximizable` | `emulated` | `window.geometry` → `maximizable` (macOS via AppKit, Windows native; Linux reports the stored flag and `setMaximizable` is skipped). |
| `is_minimizable` | `emulated` | `window.geometry` → `minimizable` (Linux reports the stored flag). |
| `is_closable` | `emulated` | `window.geometry` → `closable` (Linux reports the stored flag). |
| `is_visible` | `direct` | `window.geometry` → `visible`. |
| `is_enabled` | `direct` | `window.enabledQuery` (Electron `IsWindowEnabled` / Linux event-listening state / macOS attached sheet). |
| `is_always_on_top` | `direct` | `window.geometry` → `alwaysOnTop` (`isAlwaysOnTop()`). |
| `title` | `direct` | `window.geometry` → `title`. |
| `current_monitor` | `direct` | `window.currentDisplay`. |
| `primary_monitor` | `direct` | `window.displays` → `primary`. |
| `monitor_from_point` | `direct` | `window.displayForPoint { x, y }`. |
| `available_monitors` | `direct` | `window.displays` → `displays[]`. |
| `gtk_window` | `absent` | Electron has no GTK widget; return `Err` (Linux-only member, same cfg as the reference). |
| `default_vbox` | `absent` | Same; return `Err`. |
| `window_handle` | `direct` | `window.nativeHandle` → HWND / `NSView*` / X11 `Window`; wrap in `SendRawWindowHandle` and `borrow_raw`. Caveats in §7.2. |
| `theme` | `direct` | `window.themeQuery`. |
| `center` | `direct` | `window.center`. |
| `request_user_attention` | `emulated` | `window.requestAttention { flash }` (`flashFrame`); the Critical/Informational distinction is lost (§7.2). |
| `create_window` | `direct` | `window.create` (+ `window.ready`), then `window.nativeHandle` for `RawWindow`. |
| `create_webview` | `direct` | `webview.create`. |
| `set_resizable` | `direct` | `window.resizable`. |
| `set_enabled` | `direct` | `window.enabled`. |
| `set_maximizable` | `emulated` | `window.maximizable` (Linux: stored flag only). |
| `set_minimizable` | `emulated` | `window.minimizable` (Linux: stored flag only). |
| `set_closable` | `emulated` | `window.closable` (Linux: stored flag only). |
| `set_title` | `direct` | `window.setTitle`. |
| `maximize` | `direct` | `window.maximize`. |
| `unmaximize` | `direct` | `window.unmaximize`. |
| `minimize` | `direct` | `window.minimize`. |
| `unminimize` | `direct` | `window.unminimize` (`restore`; deferred, 15 s timeout). |
| `show` | `direct` | `window.show`. |
| `hide` | `direct` | `window.hide`. |
| `close` | `direct` | `window.close` (Electron fires `close`; the runtime keeps its own close-request pipeline). |
| `destroy` | `direct` | `window.destroy`. |
| `set_decorations` | `emulated` | `window.decorations`: macOS via the `appkit.node` `configureStyle` shim; Windows/Linux only update the stored flag (Electron's frame is creation-time). |
| `set_shadow` | `direct` (macOS) / `absent` (Windows, Linux) | `window.shadow` → `setHasShadow`; real on macOS only (§7.2). |
| `set_always_on_bottom` | `absent` | Electron has no z-order below normal (`setAlwaysOnTop` levels are all above). Emulate natively: Windows `SetWindowPos(HWND_BOTTOM)`, Linux `_NET_WM_STATE_BELOW`. |
| `set_always_on_top` | `direct` | `window.alwaysOnTop`. |
| `set_visible_on_all_workspaces` | `direct` | `window.visibleOnAllWorkspaces` (macOS/Linux; Windows no-op, as in the reference). |
| `set_content_protected` | `direct` | `window.contentProtected` (macOS `NSWindowSharingNone`, Windows `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)`; Linux no-op). |
| `set_size` | `direct` | `window.size { width, height, physical }` (content size). |
| `set_min_size` | `direct` | `window.minSize`. |
| `set_max_size` | `direct` | `window.maxSize`. |
| `set_size_constraints` | `direct` | `window.sizeConstraints` (`SizeConstraints` map with per-axis `Option`). |
| `set_position` | `direct` | `window.position`. |
| `set_fullscreen` | `direct` | `window.fullscreen` (deferred, 25 s timeout; leaving restores the pre-fullscreen bounds). |
| `set_fullscreen_on_monitor` | `emulated` | `window.fullscreen { fullscreen: true, x, y }` → `findDisplay(x,y)` → `setBounds(display.bounds)` → `setFullScreen(true)` (§7.2). |
| `set_simple_fullscreen` | `direct` | `window.simpleFullscreen` → `setSimpleFullScreen` (macOS; on Windows/Linux Electron's `SetSimpleFullScreen` is `SetFullScreen`). Member is macOS-only in the trait, matching. |
| `set_focus` | `direct` | `window.focus` (+ macOS `app.focus({steal:true})`). |
| `set_focusable` | `direct` | `window.focusable` (`setFocusable`, macOS/Windows). **Better than the reference**, which no-ops — see §7.5. |
| `set_icon` | `direct` (Windows/Linux) / `absent` (macOS) | `window.icon` (`setIcon` is Windows/Linux only; macOS uses the app icon). |
| `set_skip_taskbar` | `direct` (Windows) / `absent` (Linux, macOS) | `window.skipTaskbar` → `setSkipTaskbar`; Electron implements it on Windows only (`native_window_views.cc:1267`), macOS is an empty body (`native_window_mac.mm:976`). |
| `set_cursor_grab` | `absent` | No Electron/Chromium API to confine the OS cursor; pointer lock is page-initiated (§7.2). |
| `set_cursor_visible` | `emulated` | `window.setCursorVisible` → `insertCSS('* { cursor: none !important }')` on every webContents (§7.2). |
| `set_cursor_icon` | `emulated` | `window.setCursorIcon` → `insertCSS('* { cursor: <css> !important }')`; needs a `CursorIcon` → CSS keyword table. |
| `set_cursor_position` | `absent` | No Electron API; native shims only (Win32 `SetCursorPos`, X11 `XWarpPointer`, macOS `CGWarpMouseCursorPosition`). |
| `set_ignore_cursor_events` | `direct` | `window.ignoreCursorEvents` → `setIgnoreMouseEvents(ignore, { forward: true })`. |
| `start_dragging` | `emulated` | macOS: `window.startDrag` → `appkit.node` `startDrag` (`performWindowDragWithEvent:`). Windows/Linux: native shim (`ReleaseCapture` + `WM_NCLBUTTONDOWN`/`HTCAPTION`; `_NET_WM_MOVERESIZE`). |
| `start_resize_dragging` | `emulated` (Windows/Linux) / `absent` (macOS) | No Electron API; native shim per platform (§7.2). |
| `set_badge_count` | `direct` | `window.badgeCount` (app-level `app.setBadgeCount`); `None` → `0`; `desktop_filename` has no counterpart (§7.2). |
| `set_badge_label` | `direct` (macOS) / `absent` (elsewhere) | `app.badgeLabel` → `app.dock.setBadge`. |
| `set_overlay_icon` | `direct` (Windows) / `absent` (elsewhere) | `window.overlayIcon` → `setOverlayIcon(image, description)` (§7.2). |
| `set_progress_bar` | `direct` | `window.progressBar`; progress 0–100 → 0.0–1.0, `None` status → `-1`; `Indeterminate` needs an electron-rs gap filled (§7.2). |
| `set_title_bar_style` | `emulated` (macOS) / `absent` (Windows/Linux) | macOS: `window.titleBarStyle` → `appkit.node` `configureStyle`; elsewhere creation-time only (§7.2). |
| `set_traffic_light_position` | `direct` (macOS) / no-op elsewhere | `window.trafficLight` → `appkit.node` `setTrafficLightPosition`, or Electron's `setWindowButtonPosition` (§7.2). |
| `set_theme` | `direct` | `window.theme` (deferred) → `nativeTheme` per webContents + `themeChanged`. |
| `set_background_color` | `direct` | `window.backgroundColor`. |

### 7.2 The items called out for special care

**`gtk_window`** — Reference: Linux-only member returning a *transfer-full*
`GtkApplicationWindow*` (`gtk_window()` from winit's GTK4 extension, `to_glib_full()`), documented as
never null and main-thread-only. Electron: **`absent`**. Electron draws with Chromium views on
Ozone; there is no `GtkApplicationWindow` in the process to hand out. The mirror keeps the member
under the identical `#[cfg(any(target_os = "linux", dragonfly, freebsd, netbsd, openbsd))]` and
returns `Err(...)`; the "never null" contract cannot be honored and must be documented as broken by
the runtime, not by the caller.

**`default_vbox`** — Reference: returns the `GtkBox` the runtime created inside the GTK window
(transfer-full). Electron: **`absent`**, same reasoning; return `Err(...)`.

**`window_handle` / raw handles** — Reference: `window.window_handle()` → `SendRawWindowHandle` →
`WindowHandle::borrow_raw`, error → `HandleError::Unavailable`. Electron: **`direct`** through
`window.nativeHandle` (`getNativeWindowHandle()`; HWND on Windows, `NSView*` on macOS, X11 `Window`
on Linux). Four caveats the implementer must handle: (1) the handle arrives over an RPC, so it is a
snapshot — the borrow is only as valid as the window's lifetime, exactly like the reference's
borrow; (2) Electron sends a JavaScript `Number`, so precision is limited to 2^53 (fine for HWNDs and
x86_64 pointers, but the wrapper must not assume `u64` fidelity); (3) the reference's
`SendRawWindowHandle`/`SendRawDisplayHandle` wrappers exist because `RawWindowHandle` is not `Send` —
keep them; (4) the current target substitutes a muda-owned hidden window on Windows for menu support
(`src/window.rs:223-225`) — that is a divergence from 1:1 and should be dropped or documented, since
the reference always returns the real window handle.

**`set_traffic_light_position`** — Reference: macOS-only effect. Stores `attrs.traffic_light_position`,
insets the three standard buttons by hand (`inset_traffic_lights`), and installs a
`TrafficLightObserver` that re-applies the inset after any of seven window notifications, plus a
deferred re-apply after a theme change. Electron: **`direct`** — `win.setWindowButtonPosition({x,y})`
(macOS, `base-window.md:1560`), or electron-rs's `window.trafficLight` → `appkit.node`
`setTrafficLightPosition(handle, x, y)`. Electron keeps the position in
`NativeWindowMac::traffic_light_position_` and re-applies it on layout itself
(`native_window_mac.mm:1500-1516`), so the observer machinery is unnecessary. Non-macOS: no-op, as in
the reference.

**`set_title_bar_style`** — Reference: macOS-only effect; `Visible` → not transparent, no
`FullSizeContentView`; `Transparent` → `setTitlebarAppearsTransparent(true)`; `Overlay` → transparent
**and** `FullSizeContentView`; then `apply_movability()` (movable unless overlay or simple
fullscreen). Electron: **`emulated` on macOS, `absent` on Windows/Linux**. `titleBarStyle`
(`hidden`/`hiddenInset`/`customButtonsOnHover`) and `titleBarOverlay` are constructor options — the
frame cannot be rebuilt after creation (`setTitleBarOverlay` only restyles an already-enabled
overlay). electron-rs therefore routes `window.titleBarStyle` to a native `appkit.node`
`configureStyle(handle, decorated, style, hiddenTitle)` shim on macOS and does nothing elsewhere. The
mirror must preserve the macOS semantics (including the movability side effect) and no-op elsewhere,
and must not promise a Windows/Linux post-creation change.

**`set_shadow`** — Reference: Windows `set_undecorated_shadow`, macOS `set_has_shadow`, nothing on
Linux. Electron: `setHasShadow` is real on macOS (`[window setHasShadow:]`,
`native_window_mac.mm:1132`); on Windows it is a **no-op** — the shadow is DWM's and Electron
deliberately stopped touching it (`native_window_views.cc:1349-1352`); on Linux only the CSD frame
insets change. So: macOS `direct`, Windows/Linux `absent` (silently no-op, matching Electron).

**`set_content_protected`** — Reference: winit `set_content_protected`. Electron: `direct` —
`setContentProtection` is implemented on macOS (`NSWindowSharingNone`) and Windows
(`SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)`), and is a no-op on Linux
(`native_window_views.cc:1437` is `#if BUILDFLAG(IS_WIN)`). Same platform gating as the reference.

**`set_visible_on_all_workspaces`** — Reference: macOS `CanJoinAllSpaces`, Linux
`_NET_WM_STATE_STICKY`, Windows nothing. Electron: `direct` — `setVisibleOnAllWorkspaces(visible,
{ visibleOnFullScreen: true })`; documented as macOS/Linux and explicitly "does nothing on Windows"
(`base-window.md:1385-1402`). Note the macOS `skipTransformProcessType` option: Electron transforms
the process type by default, which briefly hides window and dock; the reference has the same
behavior through `dock.rs`, so the default is the matching choice.

**`set_cursor_grab`** — Reference: `set_cursor_grab(Confined)` / `None`, error discarded. Electron:
**`absent`**. There is no API to confine the OS cursor to a window, and Chromium's pointer lock is
page-initiated (`element.requestPointerLock()`), which is not equivalent (it hides the cursor and
locks to the element, and the page can refuse). Options: (a) document as unsupported and no-op,
matching the reference's silent error-discard; (b) native shims — Win32 `ClipCursor` on the HWND, X11
`XGrabPointer`, macOS `CGAssociateMouseAndMouseCursorPosition`. Recommendation: (a) for the mirror,
with the native shim as an opt-in follow-up, because (b) needs a release path that the trait does not
provide (`set_cursor_grab(false)` must undo it).

**`start_resize_dragging`** — Reference: `drag_resize_window(direction)`. Electron: **`absent`** as
an API; **`emulated`** natively: Windows `ReleaseCapture()` + `SendMessageW(hwnd, WM_NCLBUTTONDOWN,
HT<direction>, 0)` (the current target already does this, `src/window.rs:493-504`); Linux
`_NET_WM_MOVERESIZE` client message with the direction code; macOS has no public equivalent (only
`performWindowDragWithEvent:`, which moves rather than resizes). The `ResizeDirection` → `HT*`
mapping is 1:1 (East `HTRIGHT`, North `HTTOP`, NorthEast `HTTOPRIGHT`, NorthWest `HTTOPLEFT`, South
`HTBOTTOM`, SouthEast `HTBOTTOMRIGHT`, SouthWest `HTBOTTOMLEFT`, West `HTLEFT`).

**`set_badge_count`** — Reference: macOS dock badge (the count rendered as a label), Linux Unity
launcher count, Windows no-op; `desktop_filename` selects the Unity `.desktop` entry. Electron:
`direct` through `window.badgeCount` → `app.setBadgeCount(count)` (macOS/Linux; Windows no-op). Two
mappings to get right: `None` must become `0` (Electron clears at 0; the RPC field is `u32`, so
negative counts clamp to 0), and `desktop_filename` has **no** counterpart — Electron resolves the
launcher entry from the app's own `.desktop` file (`app.setDesktopName`). On macOS this member and
`set_badge_label` collapse onto the same dock badge, which is fine because the reference's macOS
`set_badge_count` already delegates to `set_badge_label(count.to_string())`.

**`set_overlay_icon`** — Reference: Windows `ITaskbarList3::SetOverlayIcon` with a runtime-built
`HICON` (AND mask from alpha, R/B swap), `DestroyIcon` afterwards; no-op elsewhere. Electron:
`direct` — `window.overlayIcon { icon: Option<Icon>, description }` → `setOverlayIcon(image,
description)`, Windows only (`native_window_mac.mm:1291` is an empty body). The RGBA→`NativeImage`
conversion happens in the Electron main process (`window.ts:568-575`), so no HICON code is needed.
The `description` argument (accessibility text) has no Tauri counterpart; pass an empty string or the
window title.

**`set_progress_bar`** — Reference: macOS dock-tile `NSProgressIndicator` (custom-drawn, yellow when
paused, red on error), Windows `ITaskbarList3` (`TBPF_*` + value/100), Linux Unity
(`unity_launcher_entry_set_progress`, 0.0–1.0). Electron: `direct` — `setProgressBar(progress,
{ mode })`: valid range [0, 1.0], `progress < 0` removes the bar, `progress > 1` is indeterminate,
`mode` (`none`/`normal`/`indeterminate`/`error`/`paused`) is Windows-only, Linux uses the
LauncherEntry D-Bus API and does **not** support indeterminate (`base-window.md:1160-1180`). Mapping:
`ProgressBarState.progress` is 0–100 → divide by 100; `status: None` → `progress = -1`;
`Paused`/`Error` → the matching mode; `Indeterminate` → **gap**: electron-rs's
`window::ProgressState` enum only has `None|Normal|Paused|Error`, so `indeterminate` cannot be sent
through the typed request (the current target sends the string `"indeterminate"`, which
`window.ts:343-345` maps to mode `normal` — i.e. silently wrong). Fix by extending the enum or by
sending `progress > 1`.

**`request_user_attention`** — Reference: `request_user_attention(Critical | Informational | None)`.
Electron: **`emulated`** — `flashFrame(bool)` only. On Windows/Linux it flashes the taskbar entry;
on macOS `FlashFrame(true)` is `[NSApp requestUserAttention:NSCriticalRequest]`
(`native_window_mac.mm:967-974`), so it is always *critical*. The Informational/Critical distinction
is `absent` unless the macOS path uses `app.dock.bounce('informational'|'critical')`, which
electron-rs does not expose. Note also that `flashFrame(false)` cancels, matching `None`.

**`set_simple_fullscreen`** — Reference: macOS-only trait member; winit `set_simple_fullscreen`
(true simple fullscreen, no Space change), and leaving it re-applies movability. Electron: `direct` —
`window.simpleFullscreen` → `setSimpleFullScreen` (macOS real; on Windows/Linux Electron's
`SetSimpleFullScreen` is just `SetFullScreen`, `native_window_views.cc:1284`). The member stays
macOS-gated exactly as in the reference.

**`set_fullscreen_on_monitor`** — Reference: `FullscreenTarget::MonitorAt(position)` → the monitor
whose rect contains the position → `Fullscreen::Borderless(Some(monitor))`; a position outside every
monitor is a silent no-op. Electron: **`emulated`** — `setFullScreen(true)` takes no monitor;
electron-rs's `window.fullscreen { fullscreen, x, y }` resolves `findDisplay(x, y, true)`, saves the
normal bounds, leaves fullscreen if already there, unmaximizes, sets the bounds to the target
display's origin (clamped to its size), then toggles fullscreen; on exit it restores the saved
bounds (`window.ts:499-529`). Semantic differences to document: the window is *moved* to the target
display first (visible jump on some platforms), the target display is resolved by **point-in-display**
rather than by exact monitor identity, and an unresolvable point leaves the window unchanged
(`return null`), which matches the reference's no-op.

### 7.3 Non-`WindowDispatch` items

| Item | Tag | Note |
| --- | --- | --- |
| `NativeWindowToken` | `direct` | Runtime-owned identity; no Electron dependency (§3). |
| `WindowMessage` / `handle_window_message` | `emulated` | Internal plumbing: the mirror's message queue + RPC calls replace the winit event-loop dispatch. The public shape to preserve is the `WindowDispatch` surface, not the enum. |
| `AppWindow`, `AppWindowAttrs` | `emulated` | Internal window record; keep the field set that other modules read (`lifetime`, `label`, `attrs`, `children`, `background_color`, `visible_on_all_workspaces`, `skip_taskbar`, `traffic_light_position`, `transient_for`, `pending_activation`). |
| `FullscreenTarget` | `direct` | Pure data; the Electron path consumes it through `window.fullscreen { x, y }`. |
| `prepare_macos_application` | `emulated` | Public API; must exist and be a validated no-op under Electron (§6.3). |
| `Offscreen`/`OffscreenView`/`CursorHandler` | see §5.2 | Electron OSR is a webPreferences flag; `CursorHandler` and `external_begin_frame_enabled` are `absent`. |
| `PopupFamily`/`PopupRequest`/`Popup` | `emulated` | Electron's `setWindowOpenHandler` is synchronous and the runtime must answer before the window exists, so the reservation/creation split maps onto `webview.newWindowResponse` + `window.create { new_window_request }`; the family/teardown invariants (§4.2-4.4) transfer unchanged because they are pure bookkeeping over browser identities. |

### 7.4 electron-rs gaps that the mirror must resolve

1. **`ProgressState::Indeterminate`** — not in `rpc::window::ProgressState`; needed for
   `ProgressBarStatus::Indeterminate`.
2. **`window.badgeCount { count: u32 }`** — no `Option`; the mirror must map `None` → `0`.
3. **No app-level `dock.bounce`** — the only route to `UserAttentionType::Informational` on macOS.
4. **No `setEnabled`-independent `isEnabled` on Linux** — Electron's Linux `IsEnabled` is
   `!enable_event_listening_`, which is fine, but `setEnabled(false)` disables event listening rather
   than the GTK tree the reference disables; behavior differs for CEF children (which the reference
   leaves interactive anyway).
5. **No cursor confinement / warp / native cursor APIs** — see §7.2.
6. **`window.titleBarStyle` is a macOS-only shim** — Windows/Linux post-creation changes remain
   impossible.
7. **`window.startDrag` is macOS-only** — Windows/Linux need a native shim.
8. **`window.setCursorIcon`/`setCursorVisible` are CSS injections** — per webContents; the mirror must
   apply them to every webview of the window, and they are overridden by page CSS with `!important`
   unless the injected rule keeps `!important` (it does).

### 7.5 Open API-shape decisions (cannot be mirrored verbatim)

1. **`CursorHandler`** — `dyn Fn(cef::CursorType, Option<&cef::CursorInfo>)` names CEF types. Options:
   (a) keep the name and replace the parameters with an Electron-side cursor enum (recommended if the
   type must stay public); (b) keep the CEF signature behind a `cef` feature; (c) drop it. Since
   Electron surfaces no cursor notification, (a) documents an API that never fires.
2. **`Offscreen::new`'s `cef::RenderHandler`** — replace with a paint-callback trait fed by the
   `paint` event, or keep the CEF type behind a feature.
3. **`OffscreenView::window()`** — returns `Option<Arc<dyn winit::window::Window>>`; Electron has no
   winit window. Options: (a) always `None` (honest, keeps the signature); (b) change the return type
   (breaks 1:1).
4. **`RawWindow.gtk_window` ("never null")** — cannot be honored; the mirror must either abort the
   `after_window_creation` callback on Linux or pass a documented placeholder.
5. **`set_focusable`** — the reference no-ops (TODO); Electron can do it. Recommendation: implement it
   and document the improvement, or keep the no-op for strict parity. Either is defensible; the
   decision must be recorded in the crate docs.
6. **`SendGtkWindow`/`SendGtkBox`** — pure GTK plumbing; keep the names only if the Linux `gtk_window`
   / `default_vbox` members stay (they do, returning `Err`), otherwise they are dead types.

---

## Appendix A — evidence index

Reference (all paths relative to `source/tauri/crates/tauri-runtime-cef/src/`):
`window.rs:68,77,86,95,97,105,112,127,138,160,181,201,256,292,299,309-315,317-416,419,423-446,449-462,466,468-498,500-535,537-761,763-1305,1307-1311,1313-1320,1322-1334,1336-1832,1834-1891,1893-1904,1906-1910,1912-1951`;
`window_handle.rs:10-49`; `popup.rs:5-7,20,22-32,34-41,43-49,51-329,331-363,365-467`;
`offscreen.rs:12-14,16-62,64-97,99-118`; `platform/mod.rs:5-54`;
`platform/windows/{mod.rs:5-9,window.rs:28-173,webview.rs:22-233,event_loop.rs:11-24,icon.rs:8-33,monitor.rs:11-31}`;
`platform/macos/{mod.rs:5-22,application.rs:27-274,dock.rs:16-93,event_loop.rs:18-98,fullscreen.rs:39-179,monitor.rs:11-34,progress.rs:14-137,utils.rs:19-76,wake.rs:25-121,webview.rs:16-127,window.rs:32-348}`;
`platform/linux/{mod.rs:5-12,window.rs:20-357,webview.rs:15-169,event_loop.rs:13-48,monitor.rs:14-132,taskbar.rs:9-177,utils.rs:13-153}`;
`lib.rs:18-53`.

Trait: `source/tauri/crates/tauri-runtime/src/lib.rs:816-1190` (`WindowDispatch`),
`source/tauri/crates/tauri-runtime/src/window.rs:665-687` (`RawWindow`),
`source/tauri/crates/tauri-runtime/src/webview.rs:160-190` (`NewWindowFeatures`).

Electron: `source/electron/docs/api/base-window.md` (method headings at 425-1600; `setEnabled` 818,
`isEnabled` 824, `setHasShadow` 1198, `setProgressBar` 1160, `setOverlayIcon` 1180,
`setVisibleOnAllWorkspaces` 1385, `setIgnoreMouseEvents` 1411, `setContentProtection` 1438,
`setFocusable` 1468, `setSimpleFullScreen` 668, `setWindowButtonPosition` 1560,
`setTitleBarOverlay` 1584, `flashFrame` 1042, `getNativeWindowHandle` 1095);
`source/electron/shell/browser/native_window_views.cc:689,709,1112,1132,1165,1196,1251,1267,1284,1349,1397,1437,1452,1609,1618,1718`;
`source/electron/shell/browser/native_window_mac.mm:497,501,824,832,862,870,967,976,999,1132,1182,1191,1200,1246,1291,1294,1500`;
`source/electron/lib/browser/parse-features-string.ts:35-41,100-148,153-183`;
`source/electron/lib/browser/api/web-contents.ts:261-300,316-370`;
`source/electron/lib/browser/guest-window-manager.ts:45-81`;
`source/electron/shell/browser/api/electron_api_web_contents.cc:1612-1660`;
`source/electron/docs/api/structures/web-preferences.md` (`offscreen`),
`source/electron/docs/api/web-contents.md:924-960,1461,2245-2276`.

electron-rs (`c97c052ba2f923912dbeba4b277a45d57091d958`, vendored at `.temp/electron-rs`):
`src/rpc.rs:121-453` (window commands), `src/rpc.rs:457-676` (webview commands),
`src/rpc.rs:716-742` (app commands), `src-ts/runtime/window.ts:246-529`,
`src-ts/runtime/native/appkit.ts:14-31`, `src-ts/runtime/native/cursor.ts:6-30`.
