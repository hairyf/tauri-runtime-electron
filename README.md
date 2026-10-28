# tauri-runtime-electron

An experimental Tauri 2 runtime that runs windows and webviews in Electron instead of the system WebView.

- Rust owns Tauri's event loop, commands, CSP, and ACL.
- Electron runs windows and webviews in a child process over local JSON-RPC.
- Build-time configuration prepares a private Electron distribution.

`main` targets **Tauri 2 / tauri-runtime 2.12**. `next` tracks **Tauri 3.0.0-alpha** with an incompatible runtime contract. This project is Git-only and not published to crates.io; pin a revision or tag for releases.

## Install

```toml
[dependencies]
tauri = { version = "2", default-features = false }
tauri-runtime-electron = { git = "https://github.com/hairyf/tauri-runtime-electron.git" }

[build-dependencies]
tauri-build = "2"
```

Keep `tauri_build::build()` in the application's `build.rs`, then select the runtime through Tauri's generic builder:

```rust
use tauri_runtime_electron::ElectronRuntime;

fn main() {
    tauri::Builder::<ElectronRuntime>::new()
        .run(tauri::generate_context!())
        .expect("error while running application");
}
```

The build reads `productName`, `identifier`, and `bundle.icon` from the consuming Tauri config, then uses the independent [electron-rs project](https://github.com/hairyf/electron-rs) to prepare Electron `45.0.0` by default. It may use a local install/cache or download the verified official release. Runtime startup never downloads, extracts, brands, or repairs Electron.

## Bundle

Map the complete prepared distribution into the application resources:

```json
{"bundle":{"resources":{"target/electron":"runtime"}}}
```

The output is `target/electron/<platform>-<arch>` with no version directory. Bundle all libraries, resources, metadata, and generated scripts. Startup searches adjacent `runtime` locations and standard Linux package paths; only debug builds can fall back to the build-machine staging path.

For Isolation or a custom protocol, register its scheme before creating the runtime, on the same thread:

```rust
let context = tauri::generate_context!();
if let tauri::Pattern::Isolation { schema, .. } = context.pattern() {
    tauri_runtime_electron::register_custom_protocol_scheme(schema)?;
}
tauri::Builder::<ElectronRuntime>::new().run(context)?;
```

## Electron bridge

Enable `features = ["tauri"]` to access the authenticated client of the current Electron process:

```rust
use std::process::Output;
use tauri_runtime_electron::{main_bridge, ElectronRuntime};

async fn run_task(
    webview: &tauri::Webview<ElectronRuntime>,
) -> Result<Output, Box<dyn std::error::Error>> {
    let bridge = main_bridge(webview).await?;
    let output = tokio::task::spawn_blocking(move || {
        bridge.node(&["-e", "console.log(process.argv[1]);", "--", "hello"])
    }).await??;
    assert!(output.status.success());
    assert_eq!(output.stdout, b"hello\n");
    Ok(output)
}
```

`main_bridge` returns an `Arc` client for the existing Electron process; `ElectronWebviewDispatcher::main_bridge()` is the direct runtime form. The client has two distinct execution paths:

- `bridge.node(args)` synchronously starts a **separate Node process** using the client's Electron binary with `ELECTRON_RUN_AS_NODE=1`. It directly forwards CLI arguments and returns `std::io::Result<std::process::Output>`; there is no RPC, Worker, streaming builder, or same-PID guarantee. Use `spawn_blocking` from async code as above. For eval use `&["-e", script, "--", ...]` with `process.argv.slice(1)`; file entries use `process.argv.slice(2)`. Eval source is ordinary JavaScript: wrap async work in `(async()=>{...})()` and do not use top-level `return`.
- `bridge.run(script)` executes an **async JavaScript function body on the existing Electron main thread**, in the same PID. It accepts only the script, not CLI arguments or a separate argument list. Top-level `await` and `return` are supported; return values are ignored. The body receives `signal`, `require`, `process`, and `console`, but no injected `args`.

```rust
let output = bridge
    .run("await Promise.resolve(); console.log(require('electron').app.isReady());")
    .await?;
assert!(output.status.success());
```

`run` returns an awaitable `Command` supporting `.on_output(...)`, optional `.timeout(...)`, and `.max_output_bytes(...)`. Output preserves status and raw stdout/stderr bytes. Execution has no time limit by default; the default output cap is 16 MiB. Main-thread cancellation cannot interrupt synchronous JavaScript. There is no connected-client `electron(args)` method or generic renderer execution endpoint.

For lower-level acquisition, RPC, events, and command APIs, see [electron-rs](https://github.com/hairyf/electron-rs).

## Native process helpers

`Electron::node(&args)` and `Electron::electron(&args)` run separate processes using the resolved binary. They do not start an RPC bridge; nonzero exits are `Ok(std::process::Output)` with status and raw output preserved.

## Native extensions

- The optional [window effects API](src/window/extension.rs) supports Acrylic, Mica, and Tabbed on Windows 11 22H2 (build 22621)+. Unsupported platforms or effect lists return errors; official Tauri effects are not redirected here.
- Windows attached Tauri menus are mirrored through a Rust UI-thread owner. Native menu integration on macOS/Linux and popup placement remain unverified.
- `with_webview` cannot expose another process's WebView2, GTK, or AppKit objects and returns an error without calling the callback. macOS native window handles and WKWebView data-store operations are unavailable.
- Page `window.open` uses native Chromium popups by default. Use this crate's `new_window` API for `Allow`/`Deny`/`Create`; the official process-local `new_window_handler` remains unsupported.
- Native icons use `.ico` on Windows and `.icns` on macOS. Linux currently supports executable renaming only. Apply release signing and macOS notarization after branding.

## Status

Historical CI evidence for commit [`4fb041d9854a3a2f7a4051f83df530c74082ef8c`](https://github.com/hairyf/tauri-runtime-electron/commit/4fb041d9854a3a2f7a4051f83df530c74082ef8c) recorded **154/192 (80.2%)** applicable official members across Windows, macOS, and Linux. That result is tied to that exact archived commit and does not describe the current build; skipped, failed, and unverified members remain unsupported. See the [method coverage](docs/coverage.md) and [acceptance notes](docs/acceptance.md).

## Examples and checks

```bash
pnpm example helloworld
pnpm test
cargo check --locked --all-targets --all-features
```

Run desktop examples serially. Missing platform prerequisites are reported as blocked or failed; screenshots still need visual review.

[MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE-2.0).
