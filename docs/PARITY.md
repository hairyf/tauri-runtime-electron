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

Two files are the Electron-specific layer, which the mirror cannot express and which
is purely additive — an application that only uses the Tauri API never touches it:

| file | what it adds |
| --- | --- |
| `src/command_bridge.rs` | `ElectronClient`, the running `electron-rs` client: run a Node script, call an RPC the Tauri API does not wrap. `init()` registers it as managed state; `current_client()` reads it from a runtime task. |

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

## The extra exports

Eleven replace a CEF type the reference reaches through `pub use cef;` or through
CEF's own type system. The rest are the Electron-specific layer: things CEF needs no
equivalent of, because it is linked into the process and this runtime is not.

| export | why |
| --- | --- |
| `runtime::ELECTRON_API_VERSION_LAST` | replaces `cef::sys::CEF_API_VERSION_LAST` |
| `runtime::ElectronSettings` | replaces `cef::Settings` |
| `runtime::LogSeverity`, `runtime::LogItems` | replace `cef::LogSeverity`, `cef::LogItems` |
| `runtime::ContentSettingType`, `runtime::ContentSettingValue` | replace `cef::ContentSettingTypes`, `cef::ContentSettingValues` |
| `webview::ElectronBrowserSettings` | replaces `cef::BrowserSettings` |
| `webview::ElectronWebviewHandle` | replaces `cef::Browser` |
| `offscreen::RenderHandler`, `offscreen::CursorType`, `offscreen::CursorInfo` | replace `cef::RenderHandler`, `cef::CursorType`, `cef::CursorInfo` |
| `runtime::binary`, `runtime::identifier`, `runtime::startup_timeout` | no counterpart: CEF is linked into the process, Electron is a separate distribution to find and launch |
| `runtime::custom_scheme`, `runtime::custom_schemes` | no counterpart: CEF takes its scheme list from `RuntimeInitArgs`, Electron needs the schemes before it starts |
| `webview::web_contents_id` | no counterpart: names the guest's `webContents.id` |
| `command_bridge::{ElectronClient, current_client, init}` | no counterpart: the raw client, for work no `tauri-runtime` call can express |
| `electron_rs::{Command, CommandChunk, OutputStream}` | re-exported so an application can name what `ElectronClient::run` returns |

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
- `Runtime::run_return` returns `0`, as the reference's does — its own comment says
  the exit code is not available from the loop.

Everything else either does the equivalent work or calls
`crate::unsupported::report`.

## What is not proven

- **Windows is the only platform built and tested here.** macOS and Linux are
  written and `#[cfg]`-gated; CI is what proves them.
- **The smoke test covers the window and webview surface, not all 118 exports.**
  It proves handshake, displays, cursor, window creation, seven setters through
  their getters, `eval_script_with_callback`, `url`, `bounds`, `zoom`,
  `can_go_back` and destroy. The rest is covered by the unit tests against a
  mocked RPC boundary.
- **`examples/hello` covers one Tauri application, not the API.** It proves the
  window, the asset protocol, the IPC bootstrap and one command round trip.
