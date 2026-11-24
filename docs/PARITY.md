# Parity with `tauri-runtime-cef`

Reference: `source/tauri/crates/tauri-runtime-cef` at `tauri-runtime-cef 3.0.0-alpha.7`.
Checked by `node tools/parity.mjs`, which CI runs on every push.

## Result

| tier | measure | result |
| --- | --- | --- |
| files | reference files with a counterpart | **66 / 66** |
| exported API | reference exports present at the crate root | **118 / 118** |
| trait members | `tauri-runtime` members implemented | **all** |

One file exists here and not in the reference: `src/unsupported.rs`. The reference
can do everything CEF exposes, so it never needs a place to record what it cannot
do. This runtime does, and the module is the honest answer — `report(api)` logs
`[electron] unsupported, ignored: <api>` once instead of returning a fake success.

## Rename map

Only CEF-specific identifiers change.

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
| `as_cef_*`, `with_cef_webview`, `from_cef` | `as_electron_*`, `with_electron_webview`, `from_electron` |

Unchanged: `EventProxy`, `NewWindowOpener`, `NativeWindowEventHandler`,
`SandboxPolicy`, `DebugEnvironment`, `SecretStorage`, `RemoteDebugging`,
`DevToolsPolicy`, `CertificateErrorPolicy`, `ProxyConfig`, `AutoplayPolicy`,
`WebRtcIpHandling`, `DowngradePolicy`, `WindowBuilderWrapper`, `NativeWindowToken`,
`Webview`, `WebviewSnapshot`, `ConsoleMessage`, `ConsoleMessageLevel`,
`ChromeCommandGroup`, `DevToolsProtocol`, `RuntimeStyle`, `FrameEvent`,
`FrameEventKind`, `FrameNavigationState`, `NativeDocumentToken`, `Offscreen`,
`OffscreenView`, `CursorHandler`, `NativeDialogKind`, `NativeDialogObservation`,
`NativeDialogSnapshot`, `NativeDialogToken`, `DevToolsMessageIdExhausted`,
`allocate_devtools_message_id`.

## The 15 extra exports

The reference reaches these through `pub use cef;` or through CEF's type system.
This crate has no `cef`, so each is a same-shaped replacement.

| export | replaces |
| --- | --- |
| `runtime::ELECTRON_API_VERSION_LAST` | `cef::sys::CEF_API_VERSION_LAST` |
| `runtime::ElectronSettings` | `cef::Settings` |
| `runtime::LogSeverity`, `runtime::LogItems` | `cef::LogSeverity`, `cef::LogItems` |
| `runtime::ContentSettingType`, `runtime::ContentSettingValue` | `cef::ContentSettingTypes`, `cef::ContentSettingValues` |
| `webview::ElectronBrowserSettings` | `cef::BrowserSettings` |
| `webview::ElectronWebviewHandle` | `cef::Browser` |
| `offscreen::RenderHandler`, `offscreen::CursorType`, `offscreen::CursorInfo` | `cef::RenderHandler`, `cef::CursorType`, `cef::CursorInfo` |
| `runtime::binary`, `runtime::identifier`, `runtime::startup_timeout` | no counterpart: CEF is linked into the process, Electron is a separate distribution to find and launch |
| `webview::web_contents_id` | no counterpart: names the guest's `webContents.id` |

`pub use cef;` becomes `pub use electron_rs;`, so an application can still name the
engine's own types.

## Crate-internal deltas

21 items the reference has and this crate does not, and 30 the other way, across 22
files. None is reachable from outside the crate. Each is one of:

**CEF interface plumbing.** `TauriCefDisplayHandler`, `UriSchemeHandlerFactory`,
`WebRequestHandler`, `TauriRenderProcessHandler` and friends implement CEF's C++
interfaces. Electron pushes named events over a socket instead, so the equivalent
responsibility lives in `electron_impl/client/*`'s event routing rather than in a
handler struct.

**winit event-loop machinery.** `AppState`, `WinitDragDropState`,
`wait_for_creation`, `wake_event_loop`, `close_window`, `EventLoopMessage` and
`CefWinitApplication` exist because the reference *runs* the event loop. Electron
owns its loop in another process, so this runtime's loop is a drain over the
bridge's event receiver.

**CEF load sequencing.** `INITIAL_LOAD_URL`, `PendingInitialLoads`,
`add_dev_tools_observer`, `build_browser_child` exist because CEF needs a
placeholder document, a deferred first navigation and a CDP session to get
initialization scripts working. `webview.create` takes the real URL with its scripts
already attached.

**X11/GTK and AppKit helpers.** `with_x11`, `atom`, `default_vbox`, `shared_application`
need a live X connection, a GTK widget tree or an `NSApplication` subclass. This
runtime's process has none of them.

## Support level

The reference's own gaps are reproduced, not papered over:

- `no_redirection_bitmap` stays a no-op — it is a Windows redirection-bitmap flag
  with no Electron equivalent.
- `RuntimeHandle::fetch_data_store_identifiers` / `remove_data_store` call back with
  an empty list / `Ok(())`, as the reference does, because Electron keys sessions by
  partition name rather than an opaque data-store identifier.
- `set_activate_ignoring_other_apps` is unimplemented in the reference; here it
  forwards to `app.focus({ steal })`, the closest Electron offers.

Everything else either does the equivalent work or calls
`crate::unsupported::report`.

## What is not proven

- **Windows is the only platform built and tested here.** macOS and Linux are
  written and `#[cfg]`-gated; CI is what proves them.
- **The smoke test covers the window and webview surface, not all 118 exports.**
  It proves handshake, displays, cursor, window creation, seven setters through
  their getters, `eval_script_with_callback`, `url`, `bounds`, `zoom`,
  `can_go_back` and destroy. The rest is covered by the 139 unit tests against a
  mocked RPC boundary.
