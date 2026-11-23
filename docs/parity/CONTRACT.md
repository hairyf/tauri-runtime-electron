# Rewrite contract — `tauri-runtime-electron` mirroring `tauri-runtime-cef`

Read this before touching any file. It is binding.

## 0. The one rule

`src/` must be a **file-for-file mirror** of `source/tauri/crates/tauri-runtime-cef/src/`.
Same file names, same module tree, same public items, same support level. Only the
backend changes: CEF's C callbacks become `electron-rs` JSON-RPC calls and events.

Where the reference has no Electron equivalent, the item **still exists** with the
reference's exact signature, and its body reports through `crate::unsupported::report`
or returns the reference's own "cannot" answer. It never silently succeeds.

## 1. Rename map

| reference | target |
| --- | --- |
| `Cef` | `Electron` |
| `CefRuntime<T>` | `ElectronRuntime<T>` |
| `CefRuntimeHandle<T>` | `ElectronRuntimeHandle<T>` |
| `CefWebviewAttributes` | `ElectronWebviewAttributes` |
| `CefWebviewDispatcher<T>` | `ElectronWebviewDispatcher<T>` |
| `CefWindowDispatcher<T>` | `ElectronWindowDispatcher<T>` |
| `CefInitScript` | `ElectronInitScript` |
| `AsCefWebviewAttributes` | `AsElectronWebviewAttributes` |
| `AsCefWebviewDispatcher` | `AsElectronWebviewDispatcher` |
| `AsCefWindowOpener` | `AsElectronWindowOpener` |
| `WebviewBuilderCefExt` | `WebviewBuilderElectronExt` |
| `WebviewCefExt` | `WebviewElectronExt` |
| `WebviewWindowBuilderCefExt` | `WebviewWindowBuilderElectronExt` |
| `WinitCefApp<T>` | `TauriElectronApp<T>` |
| `run_cef_helper_process` | `run_electron_helper_process` |
| `cef_entry_point` (macro) | `electron_entry_point` |
| `cef_impl/` | `electron_impl/` |
| `TauriCef*Handler` | `TauriElectron*Handler` |

Everything else keeps its **exact** spelling: `EventProxy`, `NewWindowOpener`,
`NativeWindowEventHandler`, `SandboxPolicy`, `DebugEnvironment`, `SecretStorage`,
`RemoteDebugging`, `DevToolsPolicy`, `CertificateErrorPolicy`, `ProxyConfig`,
`AutoplayPolicy`, `WebRtcIpHandling`, `DowngradePolicy`, `WindowBuilderWrapper`,
`NativeWindowToken`, `Webview`, `WebviewSnapshot`, `ConsoleMessage`,
`ConsoleMessageLevel`, `ChromeCommandGroup`, `DevToolsProtocol`, `RuntimeStyle`,
`FrameEvent`, `FrameEventKind`, `FrameNavigationState`, `NativeDocumentToken`,
`Offscreen`, `OffscreenView`, `CursorHandler`, `NativeDialogKind`,
`NativeDialogObservation`, `NativeDialogSnapshot`, `NativeDialogToken`,
`DevToolsMessageIdExhausted`, `allocate_devtools_message_id`.

## 2. CEF types must not leak

The reference leaks CEF types into its public API. Each is replaced by an
Electron-native equivalent of the same shape:

| reference item | target replacement |
| --- | --- |
| `pub use cef;` | `pub use electron_rs;` (the bridge crate, so apps can name its types) |
| `cef::Settings` in `with_settings` | `SettingsCallback = dyn FnOnce(&mut ElectronSettings)` where `ElectronSettings` is our own struct in `runtime.rs` |
| `cef::ContentSettingTypes` / `ContentSettingValues` in `default_content_setting` | `ContentSettingType` / `ContentSettingValue` enums in `runtime.rs` |
| `LogSeverity` / `LogItems` | `LogSeverity` / `LogItems` enums in `runtime.rs` |
| `cef::sys::CEF_API_VERSION_LAST` | `ELECTRON_API_VERSION_LAST` const in `runtime.rs` |
| `winit::window::Window` in `NativeWindowEventHandler` | `NativeWindow` — our own opaque handle type in `platform/mod.rs` |
| `dyn ActiveEventLoop` in `NativeWindowEventHandler` | `&NativeEventLoop` — our own opaque type in `platform/mod.rs` |
| `WinitWindowEvent` | `NativeWindowEvent` — our own enum in `platform/mod.rs` mirroring the winit variants the reference observes |

## 3. Architecture

```
tauri-runtime-electron (this crate)
  └── electron_rs::Electron          process ownership, socket, handshake, event stream
        └── Electron main process   src-ts/ command table (window.*, webview.*, app.*, protocol.*)
```

- `electron_impl/ipc.rs` — the **only** place that enters the async client. It owns
  the adapter thread, the Tokio runtime, `call(method, params, timeout)` and the
  event receiver. Everything else calls `context.ipc.call(..)` synchronously.
- `electron_impl/client/*` — the equivalent of CEF's client handlers: they consume
  the `electron-rs` event stream and turn it into Tauri events. One file per CEF
  handler file, same names.
- `electron_impl/request_handler.rs` — custom-scheme requests (`protocol.request` /
  `protocol.respond`), the equivalent of CEF's `UriSchemeHandlerFactory`.
- `electron_impl/request_context.rs` — session/partition and preference management.
- `electron_impl/cookie.rs` — cookie manager.
- `electron_impl/preferences.rs` — Chromium preference plumbing.
- `external_message_pump/*` — the run loop that drains `electron-rs` events. CEF
  needs a per-OS message pump; Electron owns its own loop, so these files drive the
  cross-platform event drain and keep the reference's public shape.

## 4. Existing RPC wire contract (already implemented in the Electron main process)

These methods exist and are proven. Reuse the payload shapes verbatim.

**window.\*** `create`, `destroy`, `close`, `setTitle`, `geometry`, `size`, `position`,
`minSize`, `maxSize`, `sizeConstraints`, `resizable`, `maximizable`, `minimizable`,
`closable`, `maximize`, `unmaximize`, `minimize`, `unminimize`, `simpleFullscreen`,
`fullscreen`, `alwaysOnTop`, `setParentWindow`, `skipTaskbar`, `focusable`, `focus`,
`contentProtected`, `center`, `requestAttention`, `show`, `hide`, `visibleOnAllWorkspaces`,
`enabled`, `enabledQuery`, `progressBar`, `shadow`, `decorations`, `badgeCount`,
`overlayIcon`, `backgroundColor`, `ignoreCursorEvents`, `displays`, `currentDisplay`,
`displayForPoint`, `cursorPosition`, `setCursorIcon`, `setCursorVisible`, `trafficLight`,
`startDrag`, `icon`, `nativeHandle`, `theme`, `themeQuery`, `setMenu`

**webview.\*** `create`, `close`, `navigate`, `reload`, `goBack`, `goForward`,
`canGoBack`, `canGoForward`, `url`, `bounds`, `setBounds`, `visibility`, `focus`,
`eval`, `print`, `setZoom`, `backgroundColor`, `cookies`, `setCookie`, `deleteCookie`,
`clearBrowsingData`, `openDevtools`, `closeDevtools`, `isDevtoolsOpen`, `setAutoResize`,
`reparent`, `capture`, `downloadResponse`, `permissionResponse`

**app.\*** `activationPolicy`, `badgeLabel`, `dockVisibility`, `focus`, `hide`, `show`, `theme`

**protocol.\*** `register`, `respond`

**Events consumed** `ready`, `ipc.closed`, `ipc.message`, `window.event`,
`window.closed`, `window.closeRequested`, `window.themeChanged`, `webview.loadStarted`,
`webview.loadFinished`, `webview.titleChanged`, `webview.newWindow`,
`webview.processGone`, `download.requested`, `download.finished`,
`permission.requested`, `protocol.request`, `menu.command`

Anything not on this list needs a new RPC in `electron-rs` (see §5).

## 5. `electron-rs` is ours to extend

`E:\projects\electron-rs` is the bridge crate. When a capability is missing:

1. Add the Rust request type in `src/rpc.rs` (or the method name in `ipc.rs`).
2. Add the JavaScript handler in `src-ts/runtime/commands.ts` and the domain module.
3. `pnpm run build` regenerates `assets/`, which `build.rs` embeds.
4. Commit and push to `origin/main`.

**Constraints:** additive only. `tauri-runtime-electron` v2 pins rev `38b7761` and
must keep building; do not change or remove any existing request, method, event name,
payload field or public type. New methods, new event names and new optional payload
fields are fine.

## 6. Style

- 2-space indent, no tabs. `rustfmt` defaults otherwise.
- Doc comments explain **why**, in the reference's voice. Keep the reference's
  comments where the reasoning still holds, and rewrite the ones that name CEF.
- No `TODO`. If something cannot be done, say so in the body and report it.
- Every file starts with:
  ```
  // Copyright 2019-2024 Tauri Programme within The Commons Conservancy
  // SPDX-License-Identifier: Apache-2.0
  // SPDX-License-Identifier: MIT
  ```
