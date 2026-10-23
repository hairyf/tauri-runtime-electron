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

The crate is not published to crates.io, so depend on the repository:

```bash
cargo add --git https://github.com/hairyf/tauri-runtime-electron
```

```toml
[dependencies]
tauri-runtime-electron = { git = "https://github.com/hairyf/tauri-runtime-electron" }
```

Pin `rev = "<sha>"` (or a tag) once you ship something, and point `path` at a
checkout while working on the runtime itself. `main` is the Tauri 2 line;
`branch = "next"` tracks Tauri 3.0.0-alpha.

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

For isolation or another application-defined protocol, register its scheme on the
same thread before creating the runtime. Electron requires secure schemes before
its `ready` event; keep and pass the original Tauri context:

```rust
let context = tauri::generate_context!();
if let tauri::Pattern::Isolation { schema, .. } = context.pattern() {
    tauri_runtime_electron::register_custom_protocol_scheme(schema)?;
}
tauri::Builder::<tauri_runtime_electron::ElectronRuntime>::new().run(context)?;
```

The example runner does this automatically. Registration is consumed by the next
runtime on that thread; Tauri still supplies the original protocol handlers, CSP
and ACL checks.


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
| [`webview.rs`](src/webview.rs) | `webview.rs` | Main-page `webContents` and native `WebContentsView` children |
| `window_builder.rs` | `window_builder.rs` | Unchanged shape |
| `devtools.rs` | `devtools.rs` | Open/close/query CDP IDs |
| `environment.rs` | `environment.rs` | Policy (`SSLKEYLOGFILE`, etc.) |
| `electron_impl/` | `cef_impl/` | IPC bridge: socket, handshake, RPC |

---

## Coverage

Not every `tauri-runtime` trait method is wired to Electron. [docs/coverage.md](docs/coverage.md)
lists each one, what this crate does instead, and what `tauri-runtime-wry` does for the
same method. It is generated from source (`node scripts/coverage.mjs --write`) and CI
fails when it goes stale.

That page records implementation paths, not functional results. `cargo test` runs
[the desktop smoke test](src/bin/smoke.rs) against a real window and page. Its named
checks cover a subset: title, geometry and minimum size, selected window states,
theme, visibility, cursor, zoom, script evaluation, navigation, IPC, page lifecycle,
devtools, an idle connection and close. Platform-specific checks report skips; see
[the coverage notes](docs/coverage.md) and test output for gaps. CI runs this smoke
suite on Windows, macOS and Linux (under `xvfb-run`). Each platform also runs
`helloworld`, `state`, `resources`, `commands`, `isolation` and `streaming` through
the functional checker, then uploads logs, assertions and original keyframes as
`example-checks-<platform>` artifacts. The other seven examples and native menu,
tray, input and device checks need separate evidence. CI leaves visual review pending.

### Running examples

Initialize the pinned sources with `git submodule update --init source/tauri source/electron`,
then use Node.js 24+, pnpm and the Rust/system dependencies for the current desktop:

```bash
pnpm example <name>
pnpm example transparent
pnpm example commands --prepare-only
pnpm example file-associations -- /absolute/path/to/image.png
pnpm check helloworld state
pnpm check transparent
pnpm check                    # attempt all 14 examples, sequentially
```

`pnpm example` with no name lists `api`, `commands`, `drag`, `file-associations`,
`helloworld`, `isolation`, `multiwebview`, `multiwindow`, `resources`, `run-return`,
`splashscreen`, `state`, `streaming` and the local `transparent` demo.

[The transparent demo](example/transparent/index.html) uses a frameless transparent
window, an explicit zero-alpha native background, transparent HTML/body and a separate
panel background alpha, following the desktop shell's layer separation. Its controls
adjust panel opacity (15–100%), toggle transparency/background blur, and minimize or
close the window; the header is draggable. On Windows the blur toggle calls Tauri's
native `setEffects`/`clearEffects` API with `Effect.Acrylic`. The generated Tauri copy
forwards effects to Electron's `setBackgroundMaterial`, which also synchronizes Chromium's
compositor; applying an accent policy to its HWND alone is insufficient. This requires
Windows 11 22H2 or later. Clearing restores the original background instead of Electron's
white default. Acrylic affects the entire window, including its margins, and
OS version/accessibility/transparency settings can limit its appearance. Other platforms
report native blur as unavailable in this demo. The window is
fixed-size because transparent-window resizing is platform-dependent. The check records
appearance and window-control assertions separately from desktop-compositing visual review;
other compositors/platforms need their own validation.

[The runner](scripts/example.mjs) discovers local demos under `example/` as well as
official examples, copying the selected local source into the same generated workspace.
It copies [the Tauri sources](source/tauri) into `target/example/tauri`. Three examples already
have Cargo projects: `api`, `file-associations` and `resources`. The other eleven get
generated manifests and build scripts. The copy selects `ElectronRuntime`, adds
explicit runtime parameters and snippet dependencies, enables `unstable` for
`multiwebview`, and repairs Windows drive-path parsing for file-association arguments.
It removes only `wry` from Tauri's default features; all other defaults, original CSP,
ACL and isolation configuration remain enabled. The original pages and command
handlers stay in use.

All examples share one locked workspace and must run sequentially. Successful runs
remove the copy; failed runs retain it for inspection. `--prepare-only` keeps the copy,
prints the manifest path and starts no Cargo process or GUI. [The source submodules](.gitmodules)
are read only. Missing API frontend assets are built from the local API package and
original Vite sources using pnpm with the frozen lockfile; initial dependencies may
need network. `streaming` copies the real [cat-spin.mp4 fixture](source/electron/spec/fixtures/cat-spin.mp4)
from the pinned Electron sources into its generated app and runs there, avoiding the
upstream video's large download.

[The functional checker](scripts/example-check.mjs) operates the original UI, checks
results and captures keyframes. Its native observation helper currently requires
Windows, PowerShell 7 and an interactive desktop; missing native support is recorded
as `blocked`. Remote pages, camera/microphone permission and devices, and packaged OS
file associations have their own prerequisites. A dev run with file arguments does
not verify installer registration or double-click associations.

Each attempted example writes evidence under `target/example-checks/<name>/`:

- `result.json`: functional assertions, limitations, checks and frame paths;
- `run.log`: build and application output;
- `native-observations.json`: native window observations, when requested;
- original PNG captures: page or native-window keyframes, without replacement UI.

Functional assertions and visual review are separate. The checker leaves
`visualReview` as `pending`; review the original PNGs and record per-frame verdicts
and notes in `visual-review.json` in the same directory. `assertions: passed` covers
only the checks recorded for that run. `blocked` and `failed` are not passes, and the
command exits nonzero for either. These commands do not establish that all 14
examples passed, or that each example has been validated on all three desktop OSes.

---

## Notes

* **Release Builds**: Ensure `devUrl` is absent from `tauri.conf.json` so Tauri embeds frontend assets.
* **Environment**: `DebugEnvironment::Auto` honors Chromium flags in debug builds and restricts them in release builds.
* **Custom Menu**: `electron/main.cjs` automatically builds a default context menu for every `webContents` (link/editing/clipboard, history, reload, inspect).
* **Native Window**: Electron returns its Win32/macOS/X11 handle, but the window belongs to the Electron process. Windows decorations, drag/resize, cursor positioning and z-order have native implementations; macOS supports cursor positioning. Dragging elsewhere requires a `-webkit-app-region: drag` region. Cursor icon and visibility apply CSS to each page. Native effects and other integrations need separate validation; `with_webview` receives `()`, and GTK accessors are not implemented.
* **Native Menus**: Tauri/muda can attach and query a Windows menu on Electron's HWND. Menu-bar command callbacks and accelerators are not bridged across the two processes; the original native popup path is not validated. The API example's menu callback check currently fails. Returning a native handle does not make these integrations fully functional.
* **Asset Protocol**: the `ipc`, `tauri` and `asset` schemes are registered as privileged before Electron starts, so `convertFileSrc` + `fetch` work once the app enables the `protocol-asset` feature and an `assetProtocol` scope.
* **Multiple Windows**: every `WebviewWindowBuilder` becomes its own Electron `BrowserWindow` with its own webview serial, so extra windows are created, closed and tracked exactly like the main one (closing a child keeps the app alive unless it was the last window).
* **New Window Requests**: `on_new_window` runs on the Rust side, so a page calling `window.open` can be handled by returning `NewWindowResponse::Create` (navigate that window) or `Allow` (Electron opens a plain browser window). Electron can only answer `setWindowOpenHandler` synchronously, so the call is denied up front and the page's `window.open` returns `null` — slower but the only shape that lets Tauri own the decision.

---

## Semver & License

* **Semver**: Follows [Semantic Versioning 2.0](https://semver.org/). Major version signifies the intended Tauri version.
* **License**: MIT or MIT/Apache 2.0 where applicable. (c) 2026 tauri-runtime-electron contributors.