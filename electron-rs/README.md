# electron-rs

Prepare Electron at build time and control its process from Rust, without Tauri.

- Download official releases with SHA-256 verification and a shared cache.
- Stage a private distribution with built-in main/preload scripts and optional branding.
- Launch with async typed JSON-RPC, event subscriptions and Node mode.

## Usage

Add the crate as both a normal and build dependency, plus Serde for request types and Tokio for the async runtime:

```toml
[dependencies]
electron-rs = { git = "https://github.com/hairyf/tauri-runtime-electron", branch = "main" }
serde = { version = "1", features = ["derive"] }
tokio = { version = "1", features = ["rt", "time", "macros"] }

[build-dependencies]
electron-rs = { git = "https://github.com/hairyf/tauri-runtime-electron", branch = "main" }
```

Pin a Git revision or tag for release builds. In your application's build script:

```rust
use electron_rs::{ElectronBuilder, Error, DEFAULT_VERSION};

fn main() -> Result<(), Error> {
    let prepared = ElectronBuilder::new(DEFAULT_VERSION)
        .app_name("My App")
        .build()?;
    if let Some(warning) = &prepared.cleanup_warning {
        println!("cargo:warning={warning}");
    }
    println!("cargo:rustc-env=APP_ELECTRON_BINARY={}", prepared.binary.display());
    Ok(())
}
```

`DEFAULT_VERSION` is currently `45.0.0`. `build()` returns `Result<PreparedDistribution, Error>`. Its fields are `directory: PathBuf` and `binary: PathBuf`, both absolute, and `cleanup_warning: Option<Error>`. Reuse those paths for that build invocation. Bundle the entire `prepared.directory`, including libraries, resources, metadata and scripts. The absolute `APP_ELECTRON_BINARY` path above is for local development; resolve packaged resources on the destination machine.

Launch from the prepared runtime directory or executable inside a Tokio runtime with time enabled:

```rust
use std::{error::Error, path::Path, thread};
use electron_rs::{find_binary, Electron, RpcRequest};
use serde::Serialize;

#[derive(Serialize)]
struct SetTheme {
    theme: &'static str,
}

impl RpcRequest for SetTheme {
    type Response = ();
    const METHOD: &'static str = "app.theme";
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    let runtime = Path::new(env!("APP_ELECTRON_BINARY"));
    let binary = find_binary(runtime)?;
    let electron = Electron::new(&binary, 0).await?;
    let events = electron.events()?;
    thread::spawn(move || {
        for event in events {
            eprintln!("{event}");
        }
    });
    electron.call(SetTheme { theme: "system" }).await?;
    let output = electron.node(&["-p", "process.versions.node"])?;
    if output.status.success() {
        println!("{}", String::from_utf8_lossy(&output.stdout));
    } else {
        eprintln!("Node exited {}: {}", output.status, String::from_utf8_lossy(&output.stderr));
    }
    electron.shutdown();
    Ok(())
}
```

`find_binary` accepts an executable, a distribution directory or a resource root containing `<platform>-<arch>`. It resolves renamed executables from staged metadata and checks executable headers for the target OS and CPU architecture. Launching needs the prepared scripts; **it never downloads, extracts, brands or repairs files**. The crate owns [main.cjs](assets/main.cjs) and [preload.cjs](assets/preload.cjs); consumers supply neither assets nor a `main_script` argument.

## Command execution

| Method on `Arc<Electron>` | Execution | Result |
| --- | --- | --- |
| `node(&args)` | Separate process with `ELECTRON_RUN_AS_NODE=1` | Synchronous `std::process::Output` |
| `electron(&args)` | Separate normal Electron process; removes `ELECTRON_RUN_AS_NODE` | Synchronous `std::process::Output` |
| `node_in_main(body, &args).await` | Short-lived Node Worker in the connected Electron process | `CommandOutput` |
| `electron_in_main(body, &args).await` | Current Electron main JavaScript thread | `CommandOutput` |

The native methods also exist on `ElectronBuilder`, and as `node(binary, args)` / `electron(binary, args)` free functions. They do not start an RPC bridge. Both preserve argument boundaries and return nonzero exits as `Ok(Output)`; process creation and collection failures are I/O errors.

The shared variants reuse the authenticated connection. Their source is an **async function body**, with `args: string[]`, `signal: AbortSignal`, CommonJS `require`, `process` and `console` available. `require` resolves from the main process's current directory. A JSON return value is separate from stdout/stderr:

```rust
use electron_rs::{Electron, OutputStream};
use std::{io::Write, time::Duration};

async fn commands(electron: &std::sync::Arc<Electron>) -> Result<(), electron_rs::Error> {
    let output = electron.node_in_main(
        "console.log(args[0]); process.stderr.write('progress'); return { answer: 42 };",
        &["one argument with spaces"],
    )
    .timeout(Duration::from_secs(10))
    .on_output(|chunk| {
        match chunk.stream {
            OutputStream::Stdout => std::io::stdout().write_all(&chunk.data),
            OutputStream::Stderr => std::io::stderr().write_all(&chunk.data),
        }.expect("forward command output");
    })
    .await?;
    assert!(output.success());
    assert_eq!(output.result["answer"], 42);

    let main = electron.electron_in_main(
        "return { ready: require('electron').app.isReady(), pid: process.pid };", &[],
    ).await?;
    assert_eq!(main.result["ready"], true);
    Ok(())
}
```

`CommandOutput` contains `code: i32`, raw `stdout` / `stderr: Vec<u8>` and `result: serde_json::Value`. Byte chunks retain stream identity and an increasing sequence; text decoding is the caller's choice. Callbacks run on the awaiting task, never the RPC reader, and all queued output is delivered before completion. Command logs bypass general event queues. Keep callbacks short: a full bounded output queue fails and cancels that command rather than silently dropping data or stopping Electron.

A script exception returns code `1` and its stack on stderr; exit codes follow native `std::process::ExitStatus::code()` representation (signed 32-bit on Windows, 8-bit on POSIX). Server deadlines return `124`; cancellation/disconnect cleanup stops Workers, and explicit cancellation returns `130`. Transport, invalid input, non-JSON return values and output-limit failures are errors. Defaults are 30 seconds and 16 MiB of combined stdout/stderr; `.timeout(...)` accepts whole milliseconds from 1 to 2,147,483,647, and `.max_output_bytes(...)` accepts 1 byte to 256 MiB. Logs are bounded, collected even without a callback, and never silently truncated. Dropping a pending future sends best-effort cancellation on its Tokio runtime; the server deadline remains the fallback if that runtime is gone.

Worker tasks finish when their Node event loop and output pipes finish. Main-thread tasks finish when their async body settles, so await their asynchronous work; task-owned timers are then cleared. Main-thread exit handling is task-scoped, and cancellation is cooperative: it cannot preempt synchronous infinite JavaScript. These are trusted Rust-side execution APIs, not a JavaScript sandbox or a renderer endpoint; Electron tasks share application globals. The connected process must have reached `app.whenReady()` before using Electron APIs, as the bundled bridge does.

## Build options

Call `build()` in a build script. It resolves the output once, prepares any changes privately, and returns the published paths. Both `binary()` and `directory()` return `Result<PathBuf, Error>` and resolve using the current environment without changing files. Prefer the returned `PreparedDistribution` when reusing the result of a particular build.

The default layout is `<target-root>/electron/<platform>-<arch>`, with **no version directory**. The target root comes from the consuming build script's `OUT_DIR`, then `CARGO_TARGET_DIR`, then offline `cargo metadata` for the enclosing project, including its workspace and configured target directory. Outside a Cargo project it falls back to `cwd/target`. An invalid Cargo context returns an error. `output(path)` replaces the full default path; a relative path resolves against the current directory.

| Builder method | Purpose |
| --- | --- |
| `new(version)` | Select the Electron release. |
| `path(path)` | Add an ordered source search path, such as `vendor/electron`. |
| `resources_dir(path)` | Set the root used by paths beginning with `{resources}`, e.g. `{resources}/runtime`. |
| `output(path)` | Replace the entire default output path; no platform/version suffix is added. |
| `app_name(name)` | Set the display name applied to the staged copy. |
| `identifier(identifier)` | Set the application identifier used by desktop integration. |
| `icon(path)` | Set a Windows `.ico` or macOS `.icns` application icon. |
| `host_executable(path)` | Reuse Windows icon/version resources from an existing executable. |
| `metadata(value)` | Replace all branding settings with a `Metadata` value. |

Fluent branding setters update individual fields. `metadata(value)` replaces all fields; setters called afterward override the corresponding fields.

The builder reuses a matching output, source path, local `node_modules/electron/dist` or cached release before downloading from official Electron releases. Every candidate, including the existing output and cache, needs executable headers matching the target OS/architecture and consistent evidence for the requested Electron version. At least one of `version` or `.electron-rs-version` must match; missing evidence, unreadable files or conflicting version files prevent reuse. Downloads require `curl`, a SHA-256 tool and a ZIP extractor, and the extracted distribution receives the same compatibility checks.

Branding changes and repairs to missing or stale scripts use a full private staging copy. Preparation errors leave the previous output intact. Publication moves the previous output to a backup and renames the stage into place; if publication fails, it attempts to restore the backup. A failed restore returns an error naming the backup and output state. Once publication succeeds, failure to remove the backup still returns `Ok(PreparedDistribution)` with `cleanup_warning` identifying the leftover backup. Changed metadata is reapplied from a clean source. Use separate outputs for concurrent builds.

New branding metadata uses the v2 marker, adding `arch` and `electronVersion` to the platform and branding identity. Runtime discovery continues to read v1 markers after checking the executable's OS/architecture; legacy metadata does not qualify as matching v2 branding for build reuse.

`Metadata` has optional `app_name`, `identifier`, `icon` and `host_executable` fields:

- Windows renames the executable and updates PE icon/version resources. `icon` must be `.ico`; `host_executable` can reuse resources from an existing Windows executable, not the current app before it is linked.
- macOS renames the app, executable and helpers, updates plists and accepts `.icns` icons. Native Windows/macOS branding requires a matching host platform.
- Linux supports `app_name` only; other metadata fields return unsupported errors. PNG files are not converted to native icons.

Branding changes existing signatures. Apply release signing and macOS notarization afterward; development ad-hoc signing is not release signing. macOS building and actual execution have not been verified in this environment.

## Process and events

`Electron::new(binary, 0).await` selects a free loopback IPC port and returns `Result<Arc<Electron>, Error>`. `Electron::with_args(binary, port, &args, timeout).await` accepts switches as `&[String]` and a positive startup deadline covering connection and handshake; `new` uses 15 seconds. Startup requires a Tokio runtime, and RPC deadlines require its time driver. `version` contains the Electron handshake version.

- `events()` takes the initial receiver **once**, including notifications buffered since launch. Its queue holds at most 1,024 events, even before the receiver is taken. Drain it or explicitly drop the receiver; a new event arriving while the initial queue is full shuts down Electron and disconnects IPC.
- `on(handler)` invokes a `Send + 'static` callback on a dedicated thread for **future** events. `subscribe()` returns a separate receiver for future events. Each has a 1,024-event queue. Overflow closes only that subscription and writes a diagnostic to stderr; other subscriptions and RPC continue. Buffered events drain before the receiver reports disconnection or the callback is dropped. Event delivery never waits for a slow receiver or callback on the RPC reader.
- `call(request).await` accepts an owned `R: RpcRequest`, serializes it as RPC parameters and returns `Result<R::Response, Error>` without blocking the calling executor. The request implements `Serialize + Send + 'static`, declares `METHOD: &'static str` and sets `Response: DeserializeOwned + Send + 'static`. `method()` defaults to `METHOD`; `timeout()` defaults to five seconds and covers writing and waiting for the response. Override these methods on the request type when needed. Supported methods and notification payloads live in the [RPC handler](src-ts/runtime/commands.ts).
- `RpcRequest::deferred()` defaults to `false` and controls the bridge's `asynchronous` flag. `notification()` also defaults to `false`; when `true`, the call completes after writing without waiting for a reply and deserializes JSON `null`, so use `Response = ()`.
- `shutdown()` closes IPC/subscriptions, wakes pending calls and terminates/reaps Electron. Dropping the final `Arc` also shuts it down. `ipc.closed` is a best-effort final event; it cannot be queued into a full buffer, so consumers must handle channel disconnection. New subscriptions after disconnect are already closed.

Event receivers use blocking `std::sync::mpsc` iteration; drain them on a separate thread as in the example. `shutdown()` and Node mode also remain synchronous. Both `ElectronBuilder::node(args)` and `Electron::node(args)` start a separate `ELECTRON_RUN_AS_NODE` process without starting an RPC bridge and return `std::io::Result<std::process::Output>`. A nonzero exit is `Ok(Output)`: inspect `status`, `stdout` and `stderr`. Errors report binary resolution, process creation or output collection failures. Both methods use the same launcher and set `CREATE_NO_WINDOW` on Windows.

Preparation, discovery and RPC return `electron_rs::ElectronError`, a non-exhaustive enum also exported as the compatibility alias `electron_rs::Error`. It implements `Display` and `std::error::Error`, with matchable variants including `IpcTimeout`, `ChannelDisconnected`, `Rpc`, `ChecksumMismatch` and `InvalidInput`. Its display text includes context and the cause; `source()` retains the underlying error, including I/O, JSON, UTF-16 and Windows errors where applicable. Use `Error` for operations returning crate errors or `Box<dyn std::error::Error>` when combining crate and I/O errors, as in the examples.

The bundled bridge uses sandboxed, isolated renderers and main-frame IPC. Standalone callers own host policy decisions; Tauri CSP/ACL are provided by the Tauri adapter. Keep IPC and executable/resource paths within your application's trust boundary.

## Checks

From the repository root, with Node.js 24+, Rust and pnpm installed:

```bash
pnpm run test:bridge:regressions
node --check electron-rs/assets/main.cjs
node --check electron-rs/assets/preload.cjs
```

The crate's unit tests cover staged assets/resource mapping, metadata validation and pipelined handshake/event/RPC cleanup. Its [integration tests](tests/) keep protocol/IPC, downloads, child layout and webview policy with the code they exercise. Rust owns the assertions; small Node VM fixtures execute the production JavaScript with timer/native doubles. These regressions need no Electron process, display or source submodules.

The separate [real bridge tests](tests/bridge.rs) are ignored by default. Run preparation first:

```bash
pnpm run test:bridge:prepare
```

This uses the actual pinned Electron release, exercises builder/resource mapping, branding, asset repair, repeated preparation and Node mode, and keeps a private distribution under `target/electron-bridge/<platform>-<arch>`. Acquisition may download Electron. On Linux, configure the staged sandbox **after preparation**, adjusting the architecture if needed:

```bash
sudo chown root:root target/electron-bridge/linux-x64/chrome-sandbox
sudo chmod 4755 target/electron-bridge/linux-x64/chrome-sandbox
```

Then run with a desktop available:

```bash
pnpm run test:bridge
# Linux with Xvfb and openbox installed:
xvfb-run --auto-servernum --server-args='-screen 0 1280x1024x24' sh -c 'openbox & sleep 1; pnpm run test:bridge'
```

The real test checks handshake, events/subscriptions/callbacks, Node success/failure, loaded-page preload and binary protocol IPC, native window/child webview operations and shutdown. It has bounded waits and removes only its own staging output; the uniquely named Electron profile under the user's app-data directory remains. Run preparation and execution serially because they share one staging path. These tests cover named public API paths and RPCs; they do not establish every native effect, three-platform execution or release-signing correctness. CI invokes both steps on Windows, macOS and Linux. For the Tauri adapter and example checks, see the [root README](../README.md#examples-and-checks).

[MIT](../LICENSE-MIT) or [Apache-2.0](../LICENSE-APACHE-2.0).
