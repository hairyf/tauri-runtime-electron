# tauri-runtime-electron

[![status](https://img.shields.io/badge/Status-Experimental-orange.svg)](#)
[![tauri](https://img.shields.io/badge/tauri-3.0.0--alpha.5-purple.svg)](https://tauri.app)
[![electron](https://img.shields.io/badge/electron-45-47848f.svg)](https://www.electronjs.org)

| Component             | Version                                                                                                       |
| --------------------- | ------------------------------------------------------------------------------------------------------------- |
| tauri-runtime-electron | 0.1.0                                                                                                         |
| tauri-runtime         | [3.0.0-alpha.4](https://crates.io/crates/tauri-runtime)                                                       |
| Electron              | [45](https://www.electronjs.org/docs/latest/api/app)                                                          |

## About Tauri

Tauri is a polyglot and generic system that is very composable and allows engineers to make a wide variety of applications. It is used for building applications for Desktop Computers using a combination of Rust tools and HTML rendered in a Webview. Apps built with Tauri can ship with any number of pieces of an optional JS API / Rust API so that webviews can control the system via message passing. In fact, developers can extend the default API with their own functionality and bridge the Webview and Rust-based backend easily.

Tauri apps can have custom menus and have tray-type interfaces. They can be updated, and are managed by the user's operating system as expected. They are very small, because they use the system's webview. They do not ship a runtime, since the final binary is compiled from rust. This makes the reversing of Tauri apps not a trivial task.

## This module

This crate is a `tauri-runtime` implementation backed by Electron instead of the
system webview. The Rust process keeps the Tauri event loop and the `invoke`
handler; it spawns an Electron main process as a child and drives it over a
loopback JSON-RPC socket. Electron owns the windows, the webviews and the
Chromium content layer, so a `<webview>` is a real OOPIF that the host page's DOM
can cover — the behaviour Tauri's native child webview cannot provide.

```rust
tauri::Builder::default()
  .runtime(tauri_runtime_electron::Electron::default())
  .run(tauri::generate_context!())
```

None of the exposed API of this crate is stable, and it may break semver
compatibility in the future. The major version only signifies the intended Tauri version.

To learn more about the details of how all of these pieces fit together, please consult the
[layout table](#layout) below and the [Tauri architecture](https://github.com/tauri-apps/tauri/blob/dev/ARCHITECTURE.md) document.

### Layout

The file layout mirrors `crates/tauri-runtime-cef` (Tauri's `feat/cef` branch) so
the two runtimes can be read side by side. A clone of that branch lives in
`source/tauri` for reference; it is not part of the crate.

| this crate | `tauri-runtime-cef` | Electron mapping |
| --- | --- | --- |
| `runtime.rs` | `runtime.rs` | `Runtime`/`RuntimeHandle` are the same; windows and webviews are created by `window.create`/`webview.create` RPCs |
| `window.rs` | `window.rs` | `BrowserWindow` methods, DIP↔physical conversion in `electron/main.cjs` |
| `webview.rs` | `webview.rs` | `webContents`; guests are `<webview>` elements, not `CefBrowser` |
| `window_builder.rs` | `window_builder.rs` | unchanged shape (attributes are collected, not applied natively) |
| `window_handle.rs` | `window_handle.rs` | monitors/locale/cursor come from `screen`/`nativeTheme` |
| `devtools.rs` | `devtools.rs` | CDP ids belong to Electron; only open/close/query remain |
| `environment.rs` | `environment.rs` | same policy (`SSLKEYLOGFILE`, `NODE_OPTIONS`, …) |
| `locale.rs` | `locale.rs` | user language → `--lang` |
| `popup.rs` | `popup.rs` | `setWindowOpenHandler` denies, Tauri's handler decides |
| `platform/mod.rs` | `platform/mod.rs` | Electron reports work areas, so no native `MonitorExt` shim |
| `switches.rs` | `switches.rs` | Chromium switch merging and the dangerous-flag warning |
| `tauri_ext.rs` | `tauri_ext.rs` | `impl From<Electron> for DynRuntimeInitAttrs<T>` |
| `electron_impl/` | `cef_impl/` | the IPC bridge: socket, handshake, request/response |

Modules that exist only because CEF exposes native handles have no
counterpart here: `cef_impl/*` (replaced by `electron_impl`), `dialog.rs`
(DevTools Page-domain observation of native JS dialogs), `frame.rs` /
`frame_navigation.rs` (CEF frame/document tokens), `sandbox.rs` (CEF sandbox
settings), `external_message_pump/` (CEF pumps its own loop; Electron's runs in
its main process), and `macros.rs` (`cef_entry_point`).

### Notes

- Release builds need `devUrl` absent from `tauri.conf.json` so Tauri embeds the
  frontend assets; with `devUrl` present the `tauri://` protocol handler has no
  asset to serve. Debug builds pick `build.devUrl` up automatically, which is
  what `tauri dev` relies on.
- `Runtime::with_webview` hands the closure `()`, because Electron owns the
  native handles and does not expose them to the host process.
- `DebugEnvironment::Auto` honours Chromium's diagnostic environment variables
  in debug builds and refuses them in release builds.
- `on_web_content_process_terminate_handler` is wired to Electron's
  `render-process-gone` event and runs asynchronously.
- `download_handler` and `permission_request_handler` are wired through a
  decision protocol: at webview creation the runtime sends Electron which
  handlers exist, and Electron forwards `will-download` /
  `setPermissionRequestHandler` events to the Rust run loop, which answers
  asynchronously. `DownloadEvent::Requested` can still rewrite the destination;
  because Electron only honours `setSavePath` before the first byte, the file is
  moved to the requested path when the download finishes. `event.preventDefault()`
  maps to a cancelled download.
- `web_resource_request_handler` needs no runtime support: Tauri applies it
  inside its own `tauri://` handler, and that handler is served through the
  protocol bridge like any other custom scheme.
- Electron ships no default context menu, so `electron/main.cjs` builds one for
  every `webContents` (link/editing/clipboard actions, history, reload, inspect).

### Acquiring the runtime

`build.rs` stages an Electron binary under `target/electron/<platform>-<arch>` the
way `tauri-runtime-cef` stages CEF through `cef-dll-sys`, so a build without Node
still runs. Resolution order:

1. `TAURI_RUNTIME_ELECTRON_ELECTRON_DIR` — an unpacked distribution to use as-is.
2. A `node_modules/electron/dist` install found at or above the workspace.
3. An unpacked copy in `ELECTRON_CACHE` (default `%LOCALAPPDATA%/electron/Cache`
   on Windows, `~/.cache/electron` elsewhere).
4. A download, verified against the release's `SHASUMS256.txt`.

`ELECTRON_MIRROR` and `ELECTRON_CUSTOM_DIR` point downloads at a mirror, and
`TAURI_RUNTIME_ELECTRON_DOWNLOAD=0` skips the step entirely. Archives are selected
from Cargo's `TARGET` triple, so cross builds fetch the matching platform. A
failed download only warns; the runtime then falls back to `electron` on `PATH`.

The staged directory also receives `main.cjs` and `preload.cjs`, and at runtime
those are looked up next to the binary. A packaged app only has to ship the staged
directory as `electron/` beside its executable (or as `resources/electron/`); the
runtime prefers that over anything in the source tree.

## Semver

**tauri-runtime-electron** intends to follow [Semantic Versioning 2.0](https://semver.org/).

## Licenses

Code: (c) 2026 tauri-runtime-electron contributors.

MIT or MIT/Apache 2.0 where applicable.
