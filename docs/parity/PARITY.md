# Parity with `tauri-runtime-cef`

Reference: `source/tauri/crates/tauri-runtime-cef` at `tauri-runtime-cef 3.0.0-alpha.7`
(`source/tauri` @ `17a527316`, Tauri v3).
Target: this crate on branch `next`.
Checked by: `node tools/parity.mjs` — re-run it; this document explains what it
measures and why the numbers are what they are.

## The claim

An application written against the CEF runtime compiles against this one after
renaming `Cef` to `Electron`, and every call it makes either does the equivalent
work through Electron or reports that Electron cannot do it. Nothing silently
succeeds.

## Result

| tier | measure | result |
| --- | --- | --- |
| 1 | reference files with a counterpart here | **66 / 66** |
| 2 | reference exports present at the crate root | **118 / 118** |
| 3 | `tauri-runtime` trait members implemented | **all** |
| — | documented extra file | 1 (`unsupported.rs`) |
| — | documented extra exports | 15 |
| — | crate-internal deltas | 21 missing, 30 extra, across 22 files |

`node tools/parity.mjs` exits 0 on this state.

## 1. Files

Every reference file has a file of the same name at the same path, with
`cef_impl/` renamed to `electron_impl/` because the bridge replaces CEF's C
callbacks. `src/lib.rs` declares the same module tree in the same order.

One file exists here and not in the reference:

- `src/unsupported.rs` — the reference can do everything CEF exposes, so it never
  needs a place to record what it cannot do. This runtime does, and the module is
  the honest answer: `report(api)` logs `[electron] unsupported, ignored: <api>`
  once per distinct API instead of returning a fake success.

## 2. Exported API

`tools/parity.mjs` reads `src/lib.rs` and expands every `pub use module::Item`
and `pub use module::*`, then compares the two export sets under the rename map.
All 118 reference exports exist here.

### The rename map

Only CEF-specific identifiers change. Everything else keeps its exact spelling.

| reference | target |
| --- | --- |
| `Cef` | `Electron` |
| `CefRuntime<T>` | `ElectronRuntime<T>` |
| `CefRuntimeHandle<T>` | `ElectronRuntimeHandle<T>` |
| `CefWebviewAttributes` | `ElectronWebviewAttributes` |
| `CefWebviewDispatcher<T>` | `ElectronWebviewDispatcher<T>` |
| `CefWindowDispatcher<T>` | `ElectronWindowDispatcher<T>` |
| `CefInitScript` | `ElectronInitScript` |
| `AsCefWebviewAttributes` / `AsCefWebviewDispatcher` / `AsCefWindowOpener` | `AsElectron…` |
| `WebviewBuilderCefExt` / `WebviewCefExt` / `WebviewWindowBuilderCefExt` | `Webview…ElectronExt` |
| `WinitCefApp<T>` / `TauriCefApp<T>` / `TauriCefHelperApp` | `TauriElectronApp<T>` / `TauriElectronHelperApp` |
| `run_cef_helper_process` | `run_electron_helper_process` |
| `cef_entry_point` | `electron_entry_point` |
| `cef_api_version` | `electron_api_version` |
| `cef_impl/` | `electron_impl/` |
| method names carrying `cef` (`as_cef_webview_attributes_mut`, `with_cef_webview`, `from_cef`) | `as_electron_…`, `with_electron_webview`, `from_electron` |

Unchanged: `EventProxy`, `NewWindowOpener`, `NativeWindowEventHandler`,
`SandboxPolicy`, `DebugEnvironment`, `SecretStorage`, `RemoteDebugging`,
`DevToolsPolicy`, `CertificateErrorPolicy`, `ProxyConfig`, `AutoplayPolicy`,
`WebRtcIpHandling`, `DowngradePolicy`, `WindowBuilderWrapper`,
`NativeWindowToken`, `Webview`, `WebviewSnapshot`, `ConsoleMessage`,
`ConsoleMessageLevel`, `ChromeCommandGroup`, `DevToolsProtocol`, `RuntimeStyle`,
`FrameEvent`, `FrameEventKind`, `FrameNavigationState`, `NativeDocumentToken`,
`Offscreen`, `OffscreenView`, `CursorHandler`, `NativeDialogKind`,
`NativeDialogObservation`, `NativeDialogSnapshot`, `NativeDialogToken`,
`DevToolsMessageIdExhausted`, `allocate_devtools_message_id`.

### The 15 extra exports, and why each exists

The reference reaches these through `pub use cef;` or through CEF's own type
system. This crate has no `cef`, so each is a same-shaped replacement.

| export | replaces |
| --- | --- |
| `runtime::ELECTRON_API_VERSION_LAST` | `cef::sys::CEF_API_VERSION_LAST` |
| `runtime::ElectronSettings` | `cef::Settings` (the `with_settings` callback's argument) |
| `runtime::LogSeverity`, `runtime::LogItems` | `cef::LogSeverity`, `cef::LogItems` |
| `runtime::ContentSettingType`, `runtime::ContentSettingValue` | `cef::ContentSettingTypes`, `cef::ContentSettingValues` |
| `webview::ElectronBrowserSettings` | `cef::BrowserSettings` (the `with_browser_settings` callback's argument) |
| `webview::ElectronWebviewHandle` | `cef::Browser` (`Webview::browser()`) |
| `offscreen::RenderHandler`, `offscreen::CursorType`, `offscreen::CursorInfo` | `cef::RenderHandler`, `cef::CursorType`, `cef::CursorInfo` |
| `runtime::binary`, `runtime::identifier`, `runtime::startup_timeout` | no counterpart: CEF is linked into the process, Electron is a separate distribution the runtime has to find and launch |
| `webview::web_contents_id` | no counterpart: names the guest's `webContents.id`, which Electron reports and CEF has no equivalent of |

`pub use cef;` becomes `pub use electron_rs;`, so an application can still name
the engine's own types.

## 3. Trait members

Every member of `WindowDispatch`, `WebviewDispatch`, `Runtime`, `RuntimeHandle`,
`RuntimeInitAttrs`, `WindowBuilder`, `WindowBuilderBase` and `EventLoopProxy`
that the reference implements is implemented here, with the same `#[cfg]`
gating. `tools/parity.mjs` reads the impl blocks on both sides and compares the
member sets; it reports no gaps.

## 4. Crate-internal deltas

21 items the reference has and this crate does not, and 30 it does not have and
this crate does, across 22 files. None is reachable from outside the crate; each
is one of three things:

**CEF interface plumbing.** `TauriCefDisplayHandler`, `UriSchemeHandlerFactory`,
`WebRequestHandler`, `WebResourceHandler`, `TauriRenderProcessHandler`,
`WebDragDropResourceRequestHandler`, `TauriCefBrowserProcessHandler`,
`PopupClientFactory` and friends implement CEF's C++ interfaces. Electron pushes
named events over a JSON-RPC socket instead of calling back into C, so the
equivalent responsibility lives in `electron_impl/client/*`'s event routing
rather than in a handler struct. `TauriElectron*Handler` structs exist where the
runtime genuinely has state to keep (permissions, downloads, command chords).

**winit event-loop machinery.** `AppState`, `WinitDragDropState`,
`AfterWindowCreationCallback`, `wait_for_creation`, `wake_event_loop`,
`close_window`, `request_window_close`, `run_on_main_thread`,
`EventLoopMessage`, `TauriElectronApp` and `CefWinitApplication` exist because
the reference *runs* the event loop. Electron owns its loop in another process,
so the runtime's loop is a drain over the bridge's event receiver and most of
that machinery has nothing to do. `TauriElectronApp` survives as the run-loop
state; the rest does not.

**CEF load sequencing.** `INITIAL_LOAD_URL`, `PendingInitialLoads`,
`load_initial_url_after_registering_initialization_scripts`,
`add_dev_tools_observer`, `build_browser_child`, `build_and_attach_webview`'s
CEF half exist because CEF needs a placeholder document, a deferred first
navigation and a CDP session to get initialization scripts and `devtools`
working. `webview.create` takes the real URL with its initialization scripts
already attached, so there is no placeholder, no second history entry and no
CDP session to observe.

**AppKit subclassing.** `CefWinitApplication`, `CefAppDelegateIvars`,
`shared_application`, `last_dock_show` subclass `NSApplication` so CEF's event
scopes work. Electron's main process owns the application object, so the macOS
bootstrap only has to leave the process able to talk to it.

**X11/GTK helpers.** `with_x11`, `atom`, `with_cef_display`, `default_vbox` need
a live X connection or a GTK widget tree, and the runtime's process has neither.

## 5. Support level

The reference's own gaps are reproduced, not papered over:

- `WindowBuilderWrapper::focusable` and `no_redirection_bitmap` are no-ops in the
  reference. Here `focusable` is implemented (`window.focusable`) because Electron
  has the API; `no_redirection_bitmap` stays a no-op because it is a Windows
  redirection-bitmap flag with no Electron equivalent.
- `Runtime::set_activate_ignoring_other_apps` is unimplemented in the reference.
  Here it forwards to `app.focus({ steal })`, which is the closest Electron offers.
- `RuntimeHandle::fetch_data_store_identifiers` / `remove_data_store` call back
  with an empty list / `Ok(())` in the reference. They do the same here, for a
  different reason: Electron keys sessions by partition name, not by an opaque
  data-store identifier.

Everything else either does the equivalent work or calls
`crate::unsupported::report`. The per-item verdicts are in
[`spec-runtime.md`](spec-runtime.md) §7, [`spec-window.md`](spec-window.md) §7,
[`spec-webview.md`](spec-webview.md) §6 and [`spec-modules.md`](spec-modules.md)
§13.

## 6. What is not proven

- **Windows is the only platform built and tested here.** macOS and Linux compile
  paths are written and `#[cfg]`-gated, but they were not compiled on this host;
  the CI matrix is what proves them.
- **No real Electron window was opened.** The 139 unit tests cover the state
  machines, the payload mapping, the cookie and frame-navigation rules and the
  downgrade policy. The RPC round trips they exercise are mocked at the boundary.
  A smoke run against a real Electron process is the next step, and the bridge
  it would drive is tested in `electron-rs`'s own suite.
