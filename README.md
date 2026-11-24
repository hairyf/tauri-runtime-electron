# tauri-runtime-electron

> A [Tauri](https://tauri.app) runtime that uses Electron as its webview engine.

`tauri-runtime` implementation backed by Electron instead of the system webview.
The Rust process keeps the Tauri event loop and `invoke` handler; an Electron main
process owns every window and page, driven over a local socket by
[`electron-rs`](https://github.com/hairyf/electron-rs).

It is a file-for-file mirror of [`tauri-runtime-cef`](https://github.com/tauri-apps/tauri/tree/v3/crates/tauri-runtime-cef):
same modules, same public API, same support level. Rename `Cef` to `Electron` and an
application built against the CEF runtime compiles against this one.

| | |
| --- | --- |
| `tauri-runtime` | 3.0.0-alpha.5 |
| Electron | 44.7.0 |
| Branch | `next` (Tauri 3). `main` tracks Tauri 2 |

## Features

- Drop-in for `tauri-runtime-cef`: `EventProxy`, `SandboxPolicy`, `DowngradePolicy`,
  `NativeDocumentToken`, `ChromeCommandGroup`, `WindowBuilderWrapper` and the rest
  keep their exact names
- Every window and webview API works through Electron, not a stub
- Anything Electron cannot do reports `[electron] unsupported, ignored: <api>` once
  instead of returning a success it did not earn
- No `tauri.conf.json` changes: the crate's build script prepares the Electron
  distribution and the bundler ships it
- Real smoke test: launches Electron and proves every setter through its getter

## Install

Not published; depend on the branch:

```toml
[dependencies]
tauri-runtime-electron = { git = "https://github.com/hairyf/tauri-runtime-electron", branch = "next" }
```

```rust
tauri::Builder::default()
  .runtime(tauri_runtime_electron::Electron::default())
  .run(tauri::generate_context!())
```

## Configuration

`Electron` mirrors `Cef`'s builder method for method.

```rust
Electron::default()
  .user_agent("MyApp/1.0")
  .sandbox(SandboxPolicy::Required)
  .remote_debugging(RemoteDebugging::Port { port: 9222, allowed_origins: vec![] })
  .proxy(ProxyConfig::Direct)
  .autoplay(AutoplayPolicy::UserGestureRequired)
```

### Custom protocols

Tauri 3 does not pass builder-registered protocols in its startup arguments, so
privilege them before Electron starts:

```rust
Electron::default().custom_scheme("stream")
```

`convertFileSrc` uses registered `http://<scheme>.localhost` aliases, because
Chromium needs an HTTP-family URL to report the length of partial media responses.

### Bundling

Nothing to configure. The build script reports the prepared distribution with
`cargo:runtime=electron` and `cargo:electron_dir=…`, the same contract
`tauri-runtime-cef` uses, and the bundler copies it to `resources/electron`.

The bundler support is a patch against Tauri v3, kept in
[`docs/tauri-electron-runtime-bundling.patch`](docs/tauri-electron-runtime-bundling.patch):

```bash
git -C source/tauri am ../docs/tauri-electron-runtime-bundling.patch
```

## Verifying

```bash
cargo test                      # 139 unit tests, RPC boundary mocked
cargo test --test smoke -- --nocapture   # a real Electron process
node tools/parity.mjs           # compares this crate against tauri-runtime-cef
```

`tools/parity.mjs` is the contract check: every reference file, every exported item
and every `tauri-runtime` trait member has to line up. CI runs it on every push.

The smoke test is what proves the bridge — it launches Electron, creates a window
with a live document, and reads every setter back through its getter. Linux needs a
display server, so CI runs it under `xvfb-run`.

## Layout

| This crate | `tauri-runtime-cef` | Electron mapping |
| --- | --- | --- |
| `runtime.rs` | `runtime.rs` | `Runtime`/`RuntimeHandle`, the `Electron` builder |
| `window.rs` | `window.rs` | `BaseWindow` |
| `webview.rs` | `webview.rs` | `WebContentsView` |
| `window_builder.rs` | `window_builder.rs` | Unchanged shape |
| `electron_impl/ipc.rs` | `cef_impl/ipc.rs` | The `electron-rs` bridge |
| `electron_impl/client/` | `cef_impl/client/` | Event routing where CEF had C callbacks |

## License

MIT OR Apache-2.0
