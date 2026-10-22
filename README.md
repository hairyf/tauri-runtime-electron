# tauri-runtime-electron

A `tauri-runtime` implementation backed by Electron instead of the system webview. The Rust process keeps the Tauri event loop and `invoke` handler, spawning an Electron main process as a child and driving it over a loopback JSON-RPC socket.

> **Branches:** `main` targets the Tauri 2 line (`tauri-runtime` 2.x). `next` tracks
> Tauri 3.0.0-alpha and additionally wires the runtime-init attributes, new-window
> decisions, download/permission handlers and process-termination events.

| Component | Version |
| --- | --- |
| `tauri-runtime-electron` | `0.1.0` |
| `tauri-runtime` | [2.12](https://crates.io/crates/tauri-runtime) |
| Electron | [45](https://www.electronjs.org/docs/latest/api/app) |

---

## Quick Start & Configuration

### 1. Rust Integration

Tauri 2 selects the runtime through the builder's type parameter:

```rust
tauri::Builder::<tauri_runtime_electron::ElectronRuntime>::new()
    .run(tauri::generate_context!())
```

Because Tauri 2 has no runtime init attributes, the knobs travel as environment
variables: `TAURI_DEV_URL` (dev-server origin override; the runtime already uses
the URL Tauri resolves from `devUrl`), `TAURI_RUNTIME_ELECTRON_ICON` (window icon)
and `TAURI_RUNTIME_ELECTRON_IDENTIFIER` (AppUserModelID). Everything else (Electron
binary, main script, IPC port) is auto-detected or defaults.


### 2. Tauri Bundle Configuration

To bundle the Electron runtime with your application without triggering downloads on first launch, add the staged target directory to your `tauri.conf.json`:

```json
{
  "bundle": {
    "resources": { "target/electron": "runtime" }
  }
}

```

---

## Runtime Resolution Order

Consumers do not need to manually ship or configure an Electron payload. The runtime locates the executable using the following order:

1. `TAURI_RUNTIME_ELECTRON_BINARY` / `Electron::binary(...)` (Explicit executable)
2. `TAURI_RUNTIME_ELECTRON_ELECTRON_DIR` (Unpacked distribution)
3. `electron/` or `resources/electron/` beside the app executable
4. `node_modules/electron/dist` at or above the workspace
5. The staged copy recorded by `build.rs` at compile time
6. An unpacked copy in `ELECTRON_CACHE` (downloads automatically if missing)

---

## Layout & Architecture

The file layout mirrors `crates/tauri-runtime-cef` for side-by-side consistency:

| This Crate | `tauri-runtime-cef` | Electron Mapping |
| --- | --- | --- |
| `runtime.rs` | `runtime.rs` | `Runtime`/`RuntimeHandle` RPC bridge |
| `window.rs` | `window.rs` | `BrowserWindow` methods |
| `webview.rs` | `webview.rs` | `webContents` (`<webview>` guests) |
| `window_builder.rs` | `window_builder.rs` | Unchanged shape |
| `devtools.rs` | `devtools.rs` | Open/close/query CDP IDs |
| `environment.rs` | `environment.rs` | Policy (`SSLKEYLOGFILE`, etc.) |
| `electron_impl/` | `cef_impl/` | IPC bridge: socket, handshake, RPC |

---

## Notes

* **Release Builds**: Ensure `devUrl` is absent from `tauri.conf.json` so Tauri embeds frontend assets.
* **Environment**: `DebugEnvironment::Auto` honors Chromium flags in debug builds and restricts them in release builds.
* **Custom Menu**: `electron/main.cjs` automatically builds a default context menu for every `webContents` (link/editing/clipboard, history, reload, inspect).
* **Native Window**: `Electron` hands its Win32/macOS/X11 handle back to Tauri, so host-side code that needs one works unchanged: application and window menus, window effects (`Mica`/`Acrylic`, vibrancy), `set_decorations` at runtime, `start_dragging`, always-on-bottom and `window.hwnd()`. Public globals stay unavailable: `with_webview` gets `()` and `Window::gtk_window`/`default_vbox` are not implemented.
* **Asset Protocol**: the `ipc`, `tauri` and `asset` schemes are registered as privileged before Electron starts, so `convertFileSrc` + `fetch` work once the app enables the `protocol-asset` feature and an `assetProtocol` scope.
* **Multiple Windows**: every `WebviewWindowBuilder` becomes its own Electron `BrowserWindow` with its own webview serial, so extra windows are created, closed and tracked exactly like the main one (closing a child keeps the app alive unless it was the last window).
* **New Window Requests**: `on_new_window` runs on the Rust side, so a page calling `window.open` can be handled by returning `NewWindowResponse::Create` (navigate that window) or `Allow` (Electron opens a plain browser window). Electron can only answer `setWindowOpenHandler` synchronously, so the call is denied up front and the page's `window.open` returns `null` — slower but the only shape that lets Tauri own the decision.

---

## Semver & License

* **Semver**: Follows [Semantic Versioning 2.0](https://semver.org/). Major version signifies the intended Tauri version.
* **License**: MIT or MIT/Apache 2.0 where applicable. (c) 2026 tauri-runtime-electron contributors.