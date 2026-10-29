# tauri-runtime-electron

An experimental Tauri 2 runtime that runs windows and webviews in Electron instead of the system WebView.

- Rust owns Tauri's event loop, commands, CSP, and ACL
- Electron runs windows and webviews in a child process over local JSON-RPC
- Build-time configuration prepares a private Electron distribution

`main` targets **Tauri 2 / tauri-runtime 2.12**; `next` tracks **Tauri 3.0.0-alpha** with an incompatible runtime contract. Git-only and not on crates.io — pin a revision or tag for releases.

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

- the build reads `productName`, `identifier`, and `bundle.icon` from the Tauri config, then prepares Electron `45.0.0` by default through the independent [electron-rs](https://github.com/hairyf/electron-rs) project, from a local cache or the verified official download
- startup never downloads, extracts, brands, or repairs Electron

## Bundle

Map the complete prepared distribution into the application resources:

```json
{"bundle":{"resources":{"target/electron":"runtime"}}}
```

- output is `target/electron/<platform>-<arch>`, no version directory; bundle every library and generated script
- startup searches adjacent `runtime` paths and standard Linux package locations; only debug builds fall back to the build-machine staging path

For Isolation or a custom protocol, register its scheme before creating the runtime, on the same thread:

```rust
let context = tauri::generate_context!();
if let tauri::Pattern::Isolation { schema, .. } = context.pattern() {
    tauri_runtime_electron::register_custom_protocol_scheme(schema)?;
}
tauri::Builder::<ElectronRuntime>::new().run(context)?;
```

## Electron client

Enable `features = ["tauri"]` and register `.plugin(tauri_runtime_electron::init())` to manage this app's authenticated Electron client as `ElectronClient` State:

```rust
use std::process::Output;
use tauri_runtime_electron::{ElectronClient, ElectronRuntime};

async fn run_task(electron: tauri::State<'_, ElectronClient>) -> std::io::Result<Output> {
    let output = electron.inner().clone()
        .node(&["-e", "console.log(process.argv[1]);", "--", "hello"])?
        .wait_with_output()
        .await?;
    assert_eq!(output.stdout, b"hello\n");
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
| `client.node(args)` | Separate native Node process, `ELECTRON_RUN_AS_NODE=1`, CLI args forwarded, no RPC or Worker | `std::io::Result<tokio::process::Child>` |
| `client.run(script)` | Async JS function body on the existing main thread, same PID | Awaitable `Command` → `std::process::Output` |

- `node` spawns immediately, without `.await` or `spawn_blocking`; read, wait, or kill through native `Child`; eval uses `&["-e", script, "--", ...]` with `process.argv.slice(1)`, file entries `slice(2)`; source is ordinary JavaScript, so wrap async work in `(async()=>{...})()` and skip top-level `return`
- `run` takes only the script, returning an awaitable `Command` with `.on_output(...)`, optional `.timeout(...)`, and `.max_output_bytes(...)` (16 MiB default); there is no connected-client `electron(args)` method or generic renderer endpoint; body, output, and cancellation semantics match `electron_rs::Electron::run` — see [electron-rs](https://github.com/hairyf/electron-rs)
- the configuration builder `Electron::node(args)` resolves the binary and spawns immediately, like root `electron_rs::node(binary, args)`; `Electron::electron(args)` stays synchronous; neither starts an RPC bridge

## Native extensions

- optional [window effects](src/window/extension.rs): Acrylic, Mica, Tabbed on Windows 11 22H2 (22621)+; unsupported platforms or effect lists error out
- Windows attached menus are mirrored through a Rust UI-thread owner; macOS/Linux native menus and popup placement unverified
- `with_webview` cannot expose another process's WebView2, GTK, or AppKit objects; it errors instead of calling back
- `window.open` uses native Chromium popups; use `new_window` for `Allow`/`Deny`/`Create`, not the process-local `new_window_handler`
- icons: `.ico` on Windows, `.icns` on macOS, executable renaming only on Linux; sign and notarize after branding

## Status

Commit [`4fb041d`](https://github.com/hairyf/tauri-runtime-electron/commit/4fb041d9854a3a2f7a4051f83df530c74082ef8c) CI recorded **154/192 (80.2%)** applicable official members on Windows, macOS, and Linux — that snapshot covers only that archived commit, not the current build. Skipped, failed, and unverified members remain unsupported; see [method coverage](docs/coverage.md) and [acceptance notes](docs/acceptance.md).

## Examples and checks

```bash
pnpm install --frozen-lockfile
pnpm example                 # Tauri CLI: Vite + desktop dashboard
pnpm --dir example dev        # Vite frontend only (no desktop IPC)
pnpm --dir example tauri build --debug --no-bundle # compile desktop app
pnpm check                   # automated dashboard checks with screenshots
pnpm test:examples transparent # relocated test fixture, not a public example
pnpm test
cargo check --locked --all-targets --all-features
```

The single [example package](example/package.json) follows the reference Tauri layout and lists every member in [method coverage](docs/coverage.md). It shows live readback assertions separately from archived CI evidence, with search, status filters, rerun and JSON export. Unexecuted members stay unverified; use the Rust runtime suites for the wider runtime/platform contract. The frontend uses React, Tailwind CSS and strict TypeScript. Standard scripts are `pnpm --dir example dev` (Vite), `pnpm --dir example build` (frontend assets) and `pnpm --dir example tauri dev` / `tauri build` (desktop). The [Vite config](example/vite.config.ts) generates the API inventory from the same contract and evidence as the coverage document, without a custom launcher. Run `pnpm --dir example typecheck` for TypeScript validation.

Run desktop checks serially; missing prerequisites are reported as blocked or failed, and screenshots still need visual review. Results are saved under `target/example-checks/`; the [transparent fixture](tests/assets/transparent/tauri.conf.json) preserves its dedicated effects and native-drag checks. The old script-based upstream example runner has been removed.

Before deleting or merging any check, read the [test suite map](docs/test-improvements.md): `tests/checks` is the evidence driver for `src/bin`, not an API test duplicate.

[MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE-2.0).
