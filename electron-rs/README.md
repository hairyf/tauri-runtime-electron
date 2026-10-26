# electron-rs

Prepare Electron at build time and control its process from Rust, without Tauri.

- Download official releases with SHA-256 verification and a shared cache.
- Stage a private distribution with built-in main/preload scripts and optional branding.
- Launch with synchronous JSON-RPC, event subscriptions and Node mode.

## Usage

Add the crate as both a normal and build dependency:

```toml
[dependencies]
electron-rs = { git = "https://github.com/hairyf/tauri-runtime-electron", branch = "main" }
serde_json = "1"

[build-dependencies]
electron-rs = { git = "https://github.com/hairyf/tauri-runtime-electron", branch = "main" }
```

Pin a Git revision or tag for release builds. In your application's build script:

```rust
use electron_rs::{ElectronBuilder, Metadata, DEFAULT_VERSION};

fn main() -> Result<(), String> {
    let builder = ElectronBuilder::new(DEFAULT_VERSION)
        .metadata(Metadata {
            app_name: Some("My App".into()),
            ..Default::default()
        });
    builder.build()?;
    let binary = builder.binary()?;
    println!("cargo:rustc-env=APP_ELECTRON_BINARY={}", binary.display());
    Ok(())
}
```

`DEFAULT_VERSION` is currently `45.0.0`. The output is `target/electron/<platform>-<arch>` (or beneath `CARGO_TARGET_DIR`), with **no version directory**. Bundle the entire distribution, including libraries, resources, metadata and scripts. The absolute `APP_ELECTRON_BINARY` path above is for local development; resolve packaged resources on the destination machine.

Launch from the prepared runtime directory or executable:

```rust
use std::{path::Path, time::Duration};
use electron_rs::{find_binary, Electron};
use serde_json::json;

fn connect(runtime: &Path) -> Result<(), String> {
    let binary = find_binary(runtime)?;
    let electron = Electron::new(&binary, 0)?;
    let _events = electron.events()?;
    electron.on(|event| eprintln!("{event}"));
    electron.call("app.theme", json!({ "theme": "system" }), Duration::from_secs(5))?;
    let output = electron.node(&["-p", "process.versions.node"])?;
    println!("{}", String::from_utf8_lossy(&output.stdout));
    electron.shutdown();
    Ok(())
}
```

`find_binary` accepts an executable, a distribution directory or a resource root containing `<platform>-<arch>`. It also resolves renamed executables from staged metadata. Launching needs the prepared scripts; **it never downloads, extracts, brands or repairs files**. The crate owns [main.cjs](assets/main.cjs) and [preload.cjs](assets/preload.cjs); consumers supply neither assets nor a `main_script` argument.

## Build options

Call `build()` in a build script; `binary()` and `directory()` only resolve the prepared output. `ElectronBuilder::node(args)` runs that binary in Node mode without starting the RPC bridge.

| Builder method | Purpose |
| --- | --- |
| `new(version)` | Select the Electron release. |
| `path(path)` | Add an ordered source search path, such as `vendor/electron`. |
| `resources_dir(path)` | Set the root used by paths beginning with `{resources}`, e.g. `{resources}/runtime`. |
| `output(path)` | Replace the entire default output path; no platform/version suffix is added. |
| `metadata(value)` | Apply optional app identity to the staged copy. |

The builder reuses a matching output, source path, local `node_modules/electron/dist` or cached release before downloading from official Electron releases. Downloads require `curl`, a SHA-256 tool and a ZIP extractor. Branding applies to a private copy; changed metadata is reapplied from a clean source. Use separate outputs for concurrent builds.

`Metadata` has optional `app_name`, `identifier`, `icon` and `host_executable` fields:

- Windows renames the executable and updates PE icon/version resources. `icon` must be `.ico`; `host_executable` can reuse resources from an existing Windows executable, not the current app before it is linked.
- macOS renames the app, executable and helpers, updates plists and accepts `.icns` icons. Native Windows/macOS branding requires a matching host platform.
- Linux supports `app_name` only; other metadata fields return unsupported errors. PNG files are not converted to native icons.

Branding changes existing signatures. Apply release signing and macOS notarization afterward; development ad-hoc signing is not release signing. macOS building and actual execution have not been verified in this environment.

## Process and events

`Electron::new(binary, 0)` selects a free loopback IPC port and returns `Arc<Electron>`. `with_args(binary, port, &args, timeout)` adds `Vec<String>` switches and a positive startup deadline covering connection and handshake; `new` uses 15 seconds. `version` contains the Electron handshake version.

- `events()` takes the initial receiver **once**, including notifications buffered since launch.
- `on(handler)` invokes a `Send + 'static` callback on a dedicated thread for **future** events. `subscribe()` returns a separate receiver for future events.
- `call(method, params, timeout)` sends synchronous RPC with a response timeout. Supported methods and notification payloads live in the bundled [RPC handler](assets/main.cjs#L856).
- `node(args)` starts a separate `ELECTRON_RUN_AS_NODE` process and returns its output.
- `shutdown()` closes IPC/subscriptions, wakes pending calls and terminates/reaps Electron. Dropping the final `Arc` also shuts it down; disconnection emits `ipc.closed`.

The bundled bridge uses sandboxed, isolated renderers and main-frame IPC. Standalone callers own host policy decisions; Tauri CSP/ACL are provided by the Tauri adapter. Keep IPC and executable/resource paths within your application's trust boundary.

## Checks

From the repository root:

```bash
cargo test --manifest-path electron-rs/Cargo.toml --locked
node --check electron-rs/assets/main.cjs
node --check electron-rs/assets/preload.cjs
```

The crate's tests cover staged assets/resource mapping, metadata validation and pipelined handshake/event/RPC cleanup. They do not establish three-platform desktop behavior or release-signing correctness. For the Tauri adapter and desktop checks, see the [root README](../README.md).

[MIT](../LICENSE-MIT) or [Apache-2.0](../LICENSE-APACHE-2.0).
