# tauri-runtime-electron

A `tauri-runtime` implementation backed by Electron instead of the system webview. The Rust process keeps the Tauri event loop and `invoke` handler, spawning an Electron main process as a child and driving it over a loopback JSON-RPC socket.

> **Branches:** `main` targets the Tauri 2 line (`tauri-runtime` 2.x). `next` tracks
> Tauri 3.0.0-alpha. These branches have different runtime contracts; this document
> describes the Tauri 2 implementation and its explicit native-object boundaries.

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

This runtime uses the official Tauri 2 generic runtime traits without upstream
source changes. Those traits have no native menu attachment or window-effects
delegation hooks. This crate's optional `window` API delegates effects to Electron
through its own Tauri plugin; official generic effects remain undelegated, and
Tauri's native `muda` menus remain unsupported. See the window API below.

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

### 2. Window API and native effects

Enable the optional `tauri` feature to use `tauri_runtime_electron::window::Window`
and the effects plugin. It depends on official Tauri with `default-features = false`
and `unstable`; it does not enable Wry or patch Tauri:

```toml
[dependencies]
tauri = { version = "2", default-features = false }
tauri-runtime-electron = { path = "../tauri-runtime-electron", features = ["tauri"] }

[build-dependencies]
tauri-build = "2"
```

The Rust `Window` wraps `tauri::Window<ElectronRuntime>`, converts from official
`Window` and `WebviewWindow` handles, and keeps official methods through `Deref`.
The module re-exports official `Color`, `Effect`, `EffectState`, `WindowEffectsConfig`
and `EffectsBuilder`. For example, given an existing official window:

```rust
use tauri_runtime_electron::{
    window::{Effect, EffectsBuilder, Window},
    ElectronRuntime,
};

fn update_effects(window: tauri::Window<ElectronRuntime>) -> tauri::Result<()> {
    let window = Window::from(window);
    window.set_effects(EffectsBuilder::new().effect(Effect::Acrylic).build())?;
    window.clear_effects()
}
```

`Window::set_effects<E: Into<Option<WindowEffectsConfig>>>` and `clear_effects`
use Electron; the free `window::set_effects(&official_window, config)` and
`window::clear_effects(&official_window)` functions accept official handles directly.
Calls run through the selected window dispatcher's `run_on_main_thread` task, bound
to that window's serial and weak Electron connection for the task's lifetime. No
global current runtime or upstream downcast is used.

For frontend calls, register the plugin on the application's builder:

```rust
tauri::Builder::<tauri_runtime_electron::ElectronRuntime>::new()
    .plugin(tauri_runtime_electron::window::init())
    .run(tauri::generate_context!())
    .expect("error while running application");
```

Declare its command with the supported inlined-plugin build API in the application's
`build.rs` so Tauri generates the permission metadata:

```rust
fn main() {
    tauri_build::try_build(tauri_build::Attributes::new().plugin(
        "electron-window",
        tauri_build::InlinedPlugin::new().commands(&["set_effects"]),
    ))
    .expect("failed to build application");
}
```

Grant the command explicitly in the application's capability (add permissions for
other official window methods as needed):

```json
{
  "identifier": "main-window",
  "windows": ["main"],
  "permissions": ["electron-window:allow-set-effects"]
}
```

The TypeScript entry is `tauri-runtime-electron/api/window`. The package is currently
`private` and unpublished: link a local checkout as a frontend dependency and provide
its `@tauri-apps/api` v2 peer. The repository's unbundled transparent demo instead
copies `api/window.js` to its generated app's `window-api.js` and imports
`./window-api.js` with `withGlobalTauri` enabled.

```ts
import { Effect, getCurrentWindow } from 'tauri-runtime-electron/api/window'

const window = getCurrentWindow()
await window.setEffects({ effects: [Effect.Acrylic] })
await window.clearEffects()
```

The JS `Window` extends the official class, retains its window methods and exports
`Effect`/`EffectState`; its current/all/by-label/focused factories return this crate's
`Window`. `setEffects` and `clearEffects` invoke the async
`plugin:electron-window|set_effects` command through normal Tauri IPC and ACL checks.
Import this entry or use the Rust wrapper/free functions to reach the extension;
official `@tauri-apps/api/window` and `tauri::Window` effects are not redirected.

Only material selection through `effects` is supported: the first supported
`Acrylic`, `Mica` or `Tabbed` entry maps to Electron's `window.backgroundMaterial`
RPC on Windows 11 22H2 (build 22621) or later. A list with no supported effect or an
unsupported platform returns an error. As in the official API, `state`, `radius`
and `interactive` apply only on macOS, and `color` has no effect on Windows 11;
these fields are ignored by this Windows backend. Clearing restores the configured
background color, including alpha, across repeated enable/clear calls. Native menu
callbacks, accelerators and popup attachment remain unsupported. OS settings can
limit the visible effect; successful RPC is separate from desktop compositing review.

### 3. Tauri Bundle Configuration

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
theme, visibility, cursor, zoom, script evaluation, navigation veto/frame isolation,
frame-scoped initialization, profile sharing/isolation, IPC, page lifecycle, devtools,
an idle connection and close. A separate real-runtime test covers setup-time exit,
vetoes and exit codes. Platform-specific checks report skips; see
[the coverage notes](docs/coverage.md) and test output for gaps. CI is configured to run this smoke
suite on Windows, macOS and Linux (under `xvfb-run`). The workflow also configures
`helloworld`, `state`, `resources`, `commands`, `isolation` and `streaming` through
the functional checker and uploads logs, assertions and original keyframes as
`example-checks-<platform>` artifacts. Windows also runs the transparent-window
check. Node regressions cover protocols/authenticated IPC, child layout/dispatch,
downloads, webview policy/initialization and the library window API/effects queue
(`window-api-check`) on all three platforms. The build job uses
`cargo check --locked --all-targets --all-features`, including the optional Tauri
integration. These configured jobs are not a claim that the current revision has
passed remote CI. Native Tauri menus remain unsupported; tray, other input and
device checks need separate evidence. CI leaves visual review pending.

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
pnpm check                    # attempt all discovered examples, sequentially
```

`pnpm example` with no name lists `api`, `commands`, `drag`, `file-associations`,
`helloworld`, `isolation`, `multiwebview`, `multiwindow`, `resources`, `run-return`,
`splashscreen`, `state`, `streaming` and the local `transparent` demo.

[The transparent demo](example/transparent/index.html) uses a frameless transparent
window, an explicit zero-alpha native background, transparent HTML/body and a separate
panel background alpha, following the desktop shell's layer separation. Its controls
adjust panel opacity (15–100%), toggle transparency and Acrylic, reset the appearance,
and minimize or close the window; the header is draggable. The frontend imports this
crate's window API and uses its plugin to call Electron's `setBackgroundMaterial`
on Windows 11 build 22621+. The page probes support, serializes effects changes and
clears native effects on reset/reload; errors are displayed and the blur control stays
disabled when unsupported. Repeated clear calls restore the configured transparent
background. Panel opacity is independent of the native material. CSS `backdrop-filter`
only filters page content and does not establish desktop blur. The window is fixed-size
because transparent-window resizing is platform-dependent. The check records appearance
and window-control assertions separately from desktop compositing visual review;
other compositors/platforms need their own validation.

[The runner](scripts/example.mjs) discovers local demos under `example/` as well as
official examples, copying the selected local source into the same generated workspace.
It copies [the Tauri sources](source/tauri) into `target/example/tauri`. Three examples already
have Cargo projects: `api`, `file-associations` and `resources`. The remaining snippets get
generated manifests and build scripts. The copy selects `ElectronRuntime`, adds
explicit runtime parameters and snippet dependencies, enables `unstable` for
`multiwebview`, and repairs Windows drive-path parsing for file-association arguments.
Generated app manifests select Tauri's non-Wry default features explicitly; original
CSP, ACL and isolation configuration remain enabled. The original pages and command
handlers stay in use. The runner asserts byte-identical copies of the upstream root
manifest and seven key framework manifest/source files, including the runtime trait,
app, window and effects implementations.

All examples share one locked workspace and must run sequentially. Successful runs
remove the copy; failed runs retain it for inspection. `--prepare-only` keeps the copy,
prints the manifest path and starts no Cargo process or GUI. [The source submodules](.gitmodules)
are read only. The runner adapts generated example projects without adding menu or
effects hooks to the framework. For `transparent`, it enables this crate's `tauri`
feature, copies the frontend entry to the generated app's `window-api.js` and declares
the inlined `electron-window` command permission; the example registers the plugin
and explicitly grants `electron-window:allow-set-effects`. Missing API frontend assets
are built from the local API package and original Vite sources using pnpm with the
frozen lockfile; initial dependencies may
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
command exits nonzero for either. These commands do not establish that all
examples passed, or that each example has been validated on all three desktop OSes.

---

## Notes

* **Release Builds**: Ensure `devUrl` is absent from `tauri.conf.json` so Tauri embeds frontend assets.
* **Environment**: `DebugEnvironment::Auto` honors Chromium flags in debug builds and restricts them in release builds.
* **Custom Menu**: `electron/main.cjs` automatically builds a default context menu for every `webContents` (editing/clipboard, history, reload, inspect). It does not launch external links behind the host policy.
* **Native Window**: Win32 and X11 IDs identify windows owned by the Electron process; they do not grant local native-object access. Local WebView2 interfaces, GTK widgets and AppKit/WKWebView objects cannot be fabricated from those IDs or child-process pointers. On macOS, `window_handle` returns `HandleError::Unavailable` rather than exposing a child-process `NSView` pointer. Missing local GTK windows/widgets return errors instead of panicking. `with_webview` returns an error without invoking the callback: there is no local WebView2, WKWebView or GTK webview object. Windows decorations, drag/resize, cursor positioning and z-order have native implementations; macOS supports cursor positioning. Dragging elsewhere requires a `-webkit-app-region: drag` region. Cursor icon and visibility apply CSS to each page. WebView2-specific interop is unsupported.
* **Native Menus and Effects**: The official generic runtime traits expose no native menu attachment or effects delegation hooks. Tauri's `muda` and `window-vibrancy` paths run in the Rust process and are not delegated to Electron. Native menu callbacks/accelerators and popup attachment remain unsupported; Electron's own context menu is separate. This crate's Rust `window` extension and `tauri-runtime-electron/api/window` use a normal Tauri plugin to apply supported Electron background materials without framework patches or ACL bypass. The inherent `ElectronWindowDispatcher::set_window_effects` helper also remains available; official generic effects do not call it.
* **macOS Title Bar**: `TitleBarStyle::Overlay` maps to Electron's `titleBarStyle: 'hidden'`; logical traffic-light positions and `tabbing_identifier` map to native BrowserWindow options. `TitleBarStyle::Transparent` and `hidden_title: true` fail creation explicitly because Electron has no matching public API that preserves Tauri's semantics. Runtime title-bar-style changes remain unsupported. Apple WKWebView data-store enumeration and removal return errors without invoking their callbacks; Electron sessions are not WKWebView stores.
* **Asset Protocol**: the `ipc`, `tauri` and `asset` schemes are registered as privileged before Electron starts, so `convertFileSrc` + `fetch` work once the app enables the `protocol-asset` feature and an `assetProtocol` scope.
* **Multiple Windows**: every `WebviewWindowBuilder` becomes its own Electron `BrowserWindow` with its own webview serial, so extra windows are created, closed and tracked exactly like the main one (closing a child keeps the app alive unless it was the last window).
* **New Window Requests**: A configured generic `new_window_handler` fails webview creation explicitly: its opener requires a real local WebView2/WKWebView/GTK object that this runtime cannot supply. It does not receive a fake callback object. Page `window.open` requests are denied by default and are not automatically sent to `shell.openExternal`.
* **Navigation**: Generic navigation decisions pause the original request in Electron's `onBeforeRequest`; allowing it resumes that request without replacing it with a fresh `loadURL`, preserving POST data and redirect behavior. Denied subframe requests are cancelled without stopping the parent or sibling frames, but Chromium may replace that iframe with an error document; retaining its previous document is not guaranteed. This does not provide native-opener callbacks.
* **Downloads**: Host-controlled downloads use a private staging directory until both completion and the first host decision arrive. Publishing copies exclusively to the destination, including across volumes, without overwriting existing files. A failed publish reports failure and retains completed bytes at the staged path recorded in the log; denial, timeout and destruction clean their staging data.
* **Initialization Scripts and IPC**: Initialization-script frame scope is retained through sandboxed subframe preload: main-frame-only scripts stay in the main frame and all-frame scripts also run in subframes. The Tauri IPC bridge remains main-frame-only.
* **Profiles**: `data_directory` is resolved and supplied to Electron's `Session.fromPath`, rather than converted to a named partition. The same resolved path shares a session. Incognito webviews use unique in-memory sessions, not the persistent directory.
* **Windows DPI**: Screen-space cursor, window and monitor coordinates use Electron's native DIP/physical conversions; multiplying global origins by one display's scale is incorrect on mixed-DPI desktops.
* **Lifecycle**: Exit requests honor the callback veto and requested exit code; `run_return` returns that code, while `run` exits the Rust process with it. macOS reopen events and supported process-termination events are forwarded under the runtime trait's platform cfg gates.

---

## Semver & License

* **Semver**: Follows [Semantic Versioning 2.0](https://semver.org/). Major version signifies the intended Tauri version.
* **License**: MIT or MIT/Apache 2.0 where applicable. (c) 2026 tauri-runtime-electron contributors.