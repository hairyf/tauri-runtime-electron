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
- `ElectronWindowBuilder` now stores the complete `WindowConfig` instead of
  dropping its arguments: `title`, `inner_size`, `min|max_inner_size`,
  `resizable`, `maximizable`, `minimizable`, `closable`, `fullscreen`,
  `decorations`, `transparent`, `visible`, `focusable`, `always_on_top`,
  `always_on_bottom`, `skip_taskbar`, `content_protected`,
  `visible_on_all_workspaces`, `prevent_overflow`, `theme`, `background_color`,
  `shadow`, `user_agent`, `devtools`, `incognito`, `window_effects` and the
  macOS title bar/traffic light options all reach the `BrowserWindow`, and the
  window icon travels as raw BGRA pixels.
- Cursor control goes through real APIs: `set_cursor_grab` clips the Win32
  cursor, `set_cursor_position` moves it, and `set_cursor_icon`/
  `set_cursor_visible` inject a page-wide CSS cursor. `start_resize_dragging`
  starts a native resize loop from the window edge.
- `docs/coverage.md` (generated by `node scripts/coverage.mjs`) lists every
  `tauri-runtime` trait member and how this crate handles it, and exits non-zero
  when a member would silently swallow a request. Unsupported calls log
  `[electron] unsupported, ignored: <api>` once instead of returning a fake
  success, and unsupported builder options are reported per window.
- `examples/smoke.rs` opens real windows and asserts every setter through its
  getter; CI runs it on Windows, macOS and headless Linux (xvfb), and now also
  triggers on the `next` branch.
- `pnpm example <name>` stages and launches any of Tauri's own examples on this
  runtime (manifest, `build.rs` and asset paths generated on the way), so the
  real API surface can be exercised by hand without touching `source/tauri`.

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
- Fix frameless secondary windows showing a native title bar next to the app's
  own: the window builder dropped `decorations`, so `frame` fell back to `true`.
- `is_decorated`, `is_maximizable`, `is_minimizable` and `is_closable` report the
  live window state instead of a hard-coded `true`, and `closable: false` is now
  enforced by the close handler.
- Fix the content area being a couple of pixels taller than the requested
  `inner_size` right after creation.
- Fix context menus never appearing: `Window::popup_menu` handed muda the
  Electron window, which belongs to another process, so `TrackPopupMenu`
  returned at once and drew nothing. Tauri now gets a 1x1 window owned by the
  thread that tracks the menu; a menu *bar* still has no visible effect, because
  Electron draws that from its own menu API.
- Fix every app sharing one Chromium profile (`%APPDATA%\Electron`), which made
  two Tauri apps fight over the same cache lock until the renderer crashed: the
  profile is now `appData/<identifier>` and the identifier reaches `main.cjs`
  instead of being dropped on the way to it.
- Give macOS fullscreen a fallback to simple fullscreen, so `set_fullscreen`
  still takes effect where AppKit refuses a space switch (VMs, CI runners).
