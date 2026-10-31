# tauri-runtime-electron

An experimental Tauri 2 runtime that runs windows and webviews in Electron instead of the system WebView.

- Rust owns Tauri's event loop, commands, CSP, and ACL
- Electron runs windows and webviews in a child process over local JSON-RPC
- Build-time configuration prepares a private Electron distribution

`main` targets **Tauri 2 / tauri-runtime 2.12**; `next` tracks **Tauri 3.0.0-alpha** with an incompatible contract. Git-only, not on crates.io — pin a revision or tag.

## Install

```toml
[dependencies]
tauri = { version = "2", default-features = false }
tauri-runtime-electron = { git = "https://github.com/hairyf/tauri-runtime-electron.git" }

[build-dependencies]
tauri-runtime-electron = { git = "https://github.com/hairyf/tauri-runtime-electron.git" }
tauri-build = "2"
```

Keep `tauri_build::build()` in `build.rs`, then select the runtime through Tauri's generic builder:

```rust
use tauri_runtime_electron::ElectronRuntime;

fn main() {
    tauri::Builder::<ElectronRuntime>::new()
        .run(tauri::generate_context!())
        .expect("error while running application");
}
```

The build reads `productName`, `identifier`, and `bundle.icon` from the Tauri config, then prepares Electron `45.0.0` by default through [electron-rs](https://github.com/hairyf/electron-rs), from a local cache or the verified official download. Startup never downloads, extracts, brands, or repairs Electron.

## Bundle

Map the prepared distribution into the application resources:

```json
{"bundle":{"resources":{"target/electron":"runtime"}}}
```

- Output is `target/electron/<platform>-<arch>`, no version directory; bundle every library and generated script.
- Startup searches adjacent `runtime` paths and standard Linux package locations; only debug builds fall back to the build-machine staging path.

For Isolation or a custom protocol, register its scheme before creating the runtime, on the same thread:

```rust
let context = tauri::generate_context!();
if let tauri::Pattern::Isolation { schema, .. } = context.pattern() {
    tauri_runtime_electron::register_custom_protocol_scheme(schema)?;
}
tauri::Builder::<ElectronRuntime>::new().run(context)?;
```

## `<webview>` tag

Pages may embed `<webview>`; it is enabled per window by default (`WindowConfig.webviewTag`, disable with `false`).

A tag guest is created by Chromium, not by this runtime, so it is adopted rather than attached:

- `will-attach-webview` normalises the guest's preferences: the runtime preload, `contextIsolation`, `sandbox`, no `nodeIntegration`.
- `did-attach-webview` registers the guest in the webview registry and applies the window's navigation, download, permission, and `window.open` policies. Its IPC reaches Rust as `ipc.message` with the guest's own webview id.
- Chromium owns the guest's view: sizing comes from CSS. `set_bounds`, `reparent`, `set_visible`, and `set_background_color` do not apply to it, and closing the page closes the guest like any other webview.

For official `@tauri-apps/api` invoke/listen, register the guest from Rust before assigning the tag's `src`:

```rust
let guest = tauri_runtime_electron::with_guest(host.label(), &guest_label, || {
  window.add_child(
    tauri::WebviewBuilder::new(&guest_label, tauri::WebviewUrl::External(url.clone())),
    tauri::LogicalPosition::new(0.0, 0.0),
    tauri::LogicalSize::new(1.0, 1.0),
  )
})?;
```

The ordinary Tauri builder registers the guest label, initialization scripts, IPC handler and event target. Electron reserves its native id instead of creating a second view. A tag in the named host consumes that reservation only when its complete `src` URL matches; the host must still be on its configured origin. No `name` attribute is needed. Registration crosses Tauri's UI-thread dispatch and is consumed once. The guest keeps its real URL, dispatcher and callbacks; it does not inherit host initialization scripts or permissions. Set guest-specific initialization and navigation/download/permission callbacks on its builder.

Each IPC message/fetch is checked against the registered guest origin, and navigation outside that origin is blocked. Closing the managed guest cancels an unused reservation or closes the attached contents; closing its host clears reservations. An unregistered tag has only the plain runtime preload and cannot borrow the host's Tauri handler or IPC protocol. `RuntimeContext::on_guest_ipc` remains available for an explicit raw IPC handler.

Tauri ACL still applies to the guest label and real URL. An HTTP guest at a different origin is remote to Tauri even when it uses loopback; `local: true` alone cannot authorize it. The application must grant the exact trusted URL and guest label using official capabilities before loading it. The runtime grants no capability and never rewrites an external URL to a local origin.

## Electron client

Enable `features = ["tauri"]` and register `.plugin(tauri_runtime_electron::init())` to expose this app's authenticated client as `ElectronClient` state:

```rust
use std::process::Output;
use tauri_runtime_electron::{ElectronClient, ElectronRuntime};

async fn run_task(electron: tauri::State<'_, ElectronClient>) -> std::io::Result<Output> {
    let output = electron.inner().clone()
        .node(&["-e", "console.log(process.argv[1]);", "--", "hello"])?
        .wait_with_output()
        .await?;
    assert_eq!(output.stdout, b"hello\\n");
    Ok(output)
}

fn main() {
    tauri::Builder::<ElectronRuntime>::new()
        .plugin(tauri_runtime_electron::init())
        .run(tauri::generate_context!())
        .expect("error while running application");
}
```

`ElectronClient` is this runtime's `Arc<electron_rs::Electron>`, registered before user setup — windowless apps included, no renderer or webview lookup.

| API | Runs in | Returns |
| --- | --- | --- |
| `client.node(args)` | Separate native Node process, `ELECTRON_RUN_AS_NODE=1`, no RPC or Worker | `std::io::Result<tokio::process::Child>` |
| `client.run(script)` | Async JS function body on the existing main thread, same PID | Awaitable `Command` → `std::process::Output` |

- `node` spawns immediately — no `.await`, no `spawn_blocking` — and forwards CLI args; `-e` scripts read `process.argv.slice(1)`, files `slice(2)`. Use the native `Child` to read, wait, or kill.
- `run` takes only the script and returns an awaitable `Command` with `.on_output(...)`, `.timeout(...)`, and `.max_output_bytes(...)` (16 MiB default). It is not an `electron(args)` launcher and exposes no generic renderer endpoint; wrap async work in `(async()=>{...})()`.
- `Electron::node(args)` resolves the binary and spawns immediately; `Electron::electron(args)` stays synchronous. Neither starts an RPC bridge.

## Limits

- Window effects (Acrylic, Mica, Tabbed) need Windows 11 22H2 (22621)+; unsupported platforms or effect lists error out.
- Windows attached menus are mirrored through a Rust UI-thread owner; macOS/Linux native menus and popup placement are unverified.
- `with_webview` cannot expose another process's WebView2, GTK, or AppKit objects; it errors instead of calling back.
- `window.open` uses native Chromium popups; use `new_window` for `Allow`/`Deny`/`Create`, not the process-local `new_window_handler`.
- Icons: `.ico` on Windows, `.icns` on macOS, executable renaming only on Linux. Sign and notarize after branding.

## Status

[`4fb041d`](https://github.com/hairyf/tauri-runtime-electron/commit/4fb041d9854a3a2f7a4051f83df530c74082ef8c) recorded **154/192 (80.2%)** official members on Windows, macOS, and Linux — that snapshot covers only that archived commit, not the current build. Skipped, failed, and unverified members stay unsupported; see [method coverage](docs/coverage.md) and [acceptance notes](docs/acceptance.md).

## Checks

```bash
pnpm install --frozen-lockfile
pnpm example                    # Tauri CLI: Vite + desktop dashboard
pnpm --dir example dev          # Vite frontend only (no desktop IPC)
pnpm --dir example tauri build --debug --no-bundle
pnpm check                      # dashboard checks with screenshots
pnpm test                       # Node regressions + cargo test
cargo check --locked --all-targets --all-features
```

- The [example package](example/package.json) follows the reference Tauri layout and lists every member in [method coverage](docs/coverage.md); live readback assertions stay separate from archived CI evidence. Use the Rust runtime suites for the wider platform contract.
- Checks run serially; missing prerequisites report as blocked or failed, and screenshots still need visual review. Output lands in `target/example-checks/`.
- Before deleting or merging a check, read the [test suite map](docs/test-improvements.md): `tests/checks` is the evidence driver for `src/bin`, not an API test duplicate.

[MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE-2.0).
