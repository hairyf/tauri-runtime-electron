# tauri-runtime-electron

A `tauri-runtime` implementation backed by Electron instead of the system webview. The Rust process keeps the Tauri event loop and `invoke` handler, spawning an Electron main process as a child and driving it over a loopback JSON-RPC socket.

| Component | Version |
| --- | --- |
| `tauri-runtime-electron` | `0.1.0` |
| `tauri-runtime` | [3.0.0-alpha.4](https://crates.io/crates/tauri-runtime) |
| Electron | [45](https://www.electronjs.org/docs/latest/api/app) |

---

## Quick Start & Configuration

### 0. Add the Dependency

The crate is not published yet; depend on the `next` branch, which tracks Tauri 3:

```toml
[dependencies]
tauri-runtime-electron = { git = "https://github.com/hairyf/tauri-runtime-electron", branch = "next" }
```

`main` tracks the Tauri 2 line and is not what Tauri 3 applications want. Pin a commit (`rev = "…"`) for reproducible builds.

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

On Windows, each window has a hidden owner on the Rust UI thread so muda can
track context menus. Its menu bar is mirrored into Electron's `Menu` API;
clicks return to muda so Tauri handles callbacks and check-item state. Changes
to labels, enabled state and menu attachment are synchronized by the runtime.

`Window::hwnd()` / `window_handle()` return this helper on Windows. Third-party
Win32 effects using that handle affect the helper; effects declared in
`tauri.conf.json` and the runtime's window operations use the Electron window.
Parent and owner options translate helper handles back to their visible window.

Window theme overrides apply to that window's Chromium pages and guests; Windows
also updates the visible DWM caption. macOS/Linux native chrome remains app-wide.
App theme changes still follow Electron's app-wide `nativeTheme` behavior.

Tauri 3 alpha does not pass builder-registered custom protocols in its startup
arguments. Privilege them before Electron starts, alongside their Tauri handler:

```rust
.runtime(tauri_runtime_electron::Electron::default().custom_scheme("stream"))
```

`convertFileSrc` uses registered `http://<scheme>.localhost` (or HTTPS) aliases
for resources. Chromium needs an HTTP-family URL to read the total length of
partial media responses. Handlers still receive their original custom-scheme URI,
headers and body. App, IPC and isolation origins keep their direct scheme URLs;
HTTP aliases cannot invoke IPC.

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

`--clean` wipes the staged copy first, `--build-only` compiles without opening
windows, and `--no-install` skips the frontend install. Pass application arguments
after `--`, for example `pnpm example file-associations -- "C:/files/hello world.txt"`.
Ctrl+C stops the app, cargo and Vite. The original isolation pattern, CSP,
capabilities and resource declarations are retained; the streaming fixture is
fetched over HTTPS and checked for an MP4 header before use. The staged API
example fixes upstream delayed hide/disable/minimize handlers to retain their
original target and method receiver. Its web fonts load through browser CSS
imports so a font server timeout cannot stop Vite; the original font families
and CSS fallbacks remain. Reference submodule files are unchanged.

For reproducible actions and keyframes on a Windows desktop:

```bash
pnpm verify:examples                  # all 13 examples
pnpm verify:examples commands state   # selected examples
node scripts/verify-runtime.mjs       # offline helper regressions, not UI proof
```

The acceptance driver uses real application controls, checks Rust responses,
window events, native drags/resizes, isolation ciphertext and decoded video
frames, and saves screenshots plus JSON evidence under `.cache/acceptance`.
Every screenshot starts with `inspected: false`; automated assertions alone do
not establish a visual pass. Missing external pages, unavailable devices and
native features outside a check's scope are recorded as `UNVERIFIED`.
The CI Windows desktop job uploads the complete evidence even when a case fails;
the three-platform smoke jobs remain separate from this desktop acceptance.

---

## Semver & License

* **Semver**: Follows [Semantic Versioning 2.0](https://semver.org/). Major version signifies the intended Tauri version.
* **License**: MIT or MIT/Apache 2.0 where applicable. (c) 2026 tauri-runtime-electron contributors.
