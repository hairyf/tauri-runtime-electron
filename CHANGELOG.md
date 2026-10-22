# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and this crate aims to
follow [Semantic Versioning](https://semver.org/).

## \[0.1.0] - 2026-10-22

### Notes

- `main` targets the Tauri 2 line (`tauri-runtime` 2.x) and is selected with
  `tauri::Builder::<ElectronRuntime>::new()`; Tauri 2 has no runtime init
  attributes, so configuration is read from `TAURI_DEV_URL`,
  `TAURI_RUNTIME_ELECTRON_ICON` and `TAURI_RUNTIME_ELECTRON_IDENTIFIER`.
  New-window requests and renderer process termination are logged rather than
  handed to a Tauri callback, because Tauri 2 only exposes them through
  WebView2/webkit handles (or on macOS/iOS). The `next` branch keeps the
  Tauri 3.0.0-alpha port with those hooks wired.

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

- Packaged apps can ship the staged runtime with a single
  `"resources": { "target/electron": "runtime" }` entry in `tauri.conf.json`;
  the resolver picks up `runtime/<platform>-<arch>` beside the executable.

- Window handling that needs a native handle now goes through the real one:
  `WindowDispatch::window_handle` asks Electron for its HWND/NSView/X11 id, so
  Tauri's own menus (`set_menu`), runtime `set_decorations`, `start_dragging`,
  always-on-bottom and `window.hwnd()` work unchanged, and window effects such as
  Mica/Acrylic apply.
- The `ipc`, `tauri` and `asset` schemes are always registered as privileged
  before Electron starts, so the asset protocol (`convertFileSrc` + `fetch`)
  works without extra wiring once the app enables `protocol-asset`.

### Bug Fixes
- Every window keeps its own webview serial, and secondary windows no longer
  inherit the first window's browser window or event handlers.
- A page's new-window request is answered on the Rust side: `Create` navigates
  the requested window and `Allow` opens an Electron window instead of relying
  on Electron's synchronous decision.
- Packaged apps show the Tauri icon instead of Electron's: the icon is read out of
  the executable's `RT_GROUP_ICON`/`RT_ICON` resources into a temporary `.ico`.
  The executable cannot be passed to `BrowserWindow` directly, and
  `app.getFileIcon` answers with the shell's generic application icon.
- `main.cjs`/`preload.cjs` are refreshed on every build, so a staged or cached
  copy can no longer run stale main-process code.
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
