# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and this crate aims to
follow [Semantic Versioning](https://semver.org/).

## \[0.1.0] - 2026-10-22

### New Features

- `build.rs` acquires an Electron runtime without a Node toolchain, following
  `cef-dll-sys`'s approach for CEF: honour `TAURI_RUNTIME_ELECTRON_ELECTRON_DIR`,
  reuse a `node_modules/electron` install, then the `ELECTRON_CACHE` cache, then
  download from `ELECTRON_MIRROR` verified against `SHASUMS256.txt`, and stage it
  under `target/electron/<platform>-<arch>` (archives chosen from `TARGET`, so
  cross builds work). Packaged apps find the staged copy beside the executable
  (`electron/` or `resources/electron/`).
- `tauri-runtime-electron`: a `tauri-runtime` implementation that spawns an
  Electron main process as a child of the Rust process and drives it over a
  loopback JSON-RPC socket (`electron_impl/ipc.rs`), with
  `Electron::default()` usable directly in `tauri::Builder::runtime`.
- Windows and webviews are created through `window.create`/`webview.create`
  RPCs; the first webview of a window is its page, and every later one is
  attached as an Electron `<webview>` element, so it is an out-of-process iframe
  and host-page DOM can cover it.
- Tauri IPC over the `ipc://` custom scheme: `window.__TAURI_INTERNALS__` is
  provided by Tauri's initialization script, `ipc://` requests are bridged to
  `PendingWebview::ipc_handler` callbacks, and the request `Origin` header is
  rewritten to the page origin so Tauri's ACL treats the call as local.
- Custom scheme protocol handlers (`tauri://`, plugin schemes) are registered
  before the window loads and answered from the Rust run loop, so production
  builds serve embedded assets without a dev server.
- Window dispatch: size/position/min-max constraints, resizable, maximizable,
  minimizable, closable, maximize/unmaximize, minimize/unminimize, fullscreen,
  always-on-top, skip-taskbar, focusable, content-protected, center, request
  attention, shadow, icon, overlay icon, progress bar, badge count, background
  colour, theme, title, show/hide/focus/close/destroy, and display/monitor queries.
- Webview dispatch: navigate, reload, history (back/forward/can-go-*), zoom,
  eval (with callback), bounds/size/position, show/hide/focus, print, close,
  cookies (get/set/delete/clear browsing data), background colour, and DevTools
  open/close/query.
- Window, webview, page-load, title, new-window and process-termination event
  handlers, including `render-process-gone` mapped to
  `WebContentProcessTermination`.
- `DebugEnvironment` policy for Chromium diagnostic environment variables,
  accept-language detection (`locale.rs`), Chromium switch merging plus the
  dangerous-switch warning (`switches.rs`), and window-feature parsing for
  `window.open` (`popup.rs`).
- Electron side (`electron/main.cjs`, `electron/preload.cjs`): F12 / Ctrl+Shift+I
  DevTools shortcut, native context menu, application menu removed, window icon
  from `bundle.icon`, and a `<webview>` element per guest webview.
- Application-level switches (`set_activation_policy`, `set_dock_visibility`,
  `show`, `hide`, and activating the app on macOS) now reach Electron's `app`
  module instead of returning a silent success, and `webview.set_auto_resize`
  forwards to `webContents.setAutoResize`.
- `download_handler` and `permission_request_handler` are wired through a
  decision protocol: the webview's creation payload tells Electron which handlers
  exist, Electron forwards `will-download` / `setPermissionRequestHandler` events
  to the run loop, and the answers arrive asynchronously. A requested download
  destination is honoured by moving the finished file, and a denied request
  cancels (`permission` falling back to Electron's own deny).

### Bug Fixes
- Webview history RPCs use `webContents.navigationHistory` instead of the
  deprecated `canGoBack()`/`goBack()` shortcuts.
- Fix guest webviews never running their initialization scripts: the scripts ran
  from a `dom-ready` listener registered after that event had already fired, so
  `window.__TAURI_INTERNALS__` was missing and `invoke` failed. Guests now run
  them right after the awaited `dom-ready`.
- Fix a guest webview stealing the IPC scheme: custom-scheme handlers answer for
  the whole session and Electron cannot report the requesting webview, so the
  first webview (the window's page) keeps scheme ownership instead of the last
  registered one.
- Fix the `Object has been destroyed` exception when a window closes: the
  `closed` handler no longer reads `window.webContents` after teardown.
- Fix `asset not found: index.html` under `tauri dev`: `Electron::apply_config`
  now falls back to `build.devUrl` in debug builds.
- Fix a deadlock when the first window loads a `tauri://` URL: `loadWebview` no
  longer awaits `loadURL`, so the Rust run loop can answer `protocol.request`.
- Fix the IPC handler lookup missing registered handlers: the handler map is
  shared through `Rc<RefCell<…>>` instead of being cloned per dispatcher.
- Fix `Runtime::cursor_position` returning a `(0, 0)` stub; it now asks
  Electron's `screen.getCursorScreenPoint`.
