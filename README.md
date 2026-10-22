# tauri-runtime-electron

A `tauri-runtime` implementation backed by Electron instead of the system webview. The Rust process keeps the Tauri event loop and `invoke` handler, spawning an Electron main process as a child and driving it over a loopback JSON-RPC socket.

| Component | Version |
| --- | --- |
| `tauri-runtime-electron` | `0.1.0` |
| `tauri-runtime` | [3.0.0-alpha.4](https://crates.io/crates/tauri-runtime) |
| Electron | [45](https://www.electronjs.org/docs/latest/api/app) |

---

## Quick Start & Configuration

### 1. Rust Integration

Initialize the runtime with `tauri::Builder::default()`:

```rust
tauri::Builder::default()
    .runtime(tauri_runtime_electron::Electron::default())
    .run(tauri::generate_context!())

```

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

## Menus & Native Handles

Context menus work: `Window::popup_menu` draws the same native menu wry would,
because Tauri is handed a 1x1 window owned by the thread that tracks the menu
(muda can only pop a menu up on a window of that thread, and Electron's window
belongs to another process). Two consequences are worth knowing:

* A **menu bar** (`Window::set_menu`) has no visible effect: Tauri attaches it to
  that helper window, while Electron draws its menu bar from its own `Menu` API.
* On Windows, `Window::hwnd()` / `window_handle()` return the helper window, not
  the Electron one, so third-party Win32 tweaks fed with that handle (window
  vibrancy, custom DWM calls) do nothing. Effects declared in `tauri.conf.json`
  are unaffected: they travel as window config, not through the handle.

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

## Verifying Coverage

Two checks exist so "it compiles" is never mistaken for "it works":

```bash
node scripts/coverage.mjs   # writes docs/coverage.md, exits 1 on silent no-ops
cargo run --example smoke   # opens real windows, asserts every setter via its getter
```

`docs/coverage.md` lists every `tauri-runtime` trait member next to how this
crate handles it, classified as implemented, accepted then ignored,
platform-limited, refused, or missing. Anything the runtime cannot do is logged
as `[electron] unsupported, ignored: <api>` on first use instead of returning a
fake success. Linux needs a display server, so CI runs the smoke example under
`xvfb-run`.

The smoke example asserts ~80 behaviours against two real windows and one guest
webview: every setter that has a getter round-trips through it, every other
setter must at least reach Electron without an IPC error, and a third window is
closed to prove `run_iteration` actually delivers the destroy event. Behaviour
that only exists on macOS or Windows (maximize, minimize, fullscreen,
decorations) is asserted there and skipped under xvfb, which has no window
manager.

`source/tauri` and `source/electron` are pinned submodules kept for reading the
upstream implementations; they are not built.

## Running Tauri's Own Examples

```bash
pnpm example api        # or: node scripts/example.mjs api
pnpm example --list     # every example that can be staged this way
```

The script stages `source/tauri/examples/<name>` under `.cache/examples/<name>`,
swaps its runtime for this one, generates the manifest and `build.rs` the
upstream workspace normally supplies, and launches it — so any of Tauri's
examples can be poked at by hand. Examples with a Vite frontend (`api`) get the
dev server on port 1420; the rest keep their `frontendDist` assets.

`--clean` wipes the staged copy first, `--no-install` skips the frontend
install, `--keep` leaves everything staged after exit. Ctrl+C stops the app,
cargo and Vite.

---

## Semver & License

* **Semver**: Follows [Semantic Versioning 2.0](https://semver.org/). Major version signifies the intended Tauri version.
* **License**: MIT or MIT/Apache 2.0 where applicable. (c) 2026 tauri-runtime-electron contributors.
