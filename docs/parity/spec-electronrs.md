# `electron-rs` capability surface — implementation contract

Status: reference specification for the Electron backend of the Tauri v3 runtime.
Subject: the `electron-rs` crate checked out at `E:\projects\electron-rs`, branch `main`, commit `c97c052`.
Scope of evidence: `src/lib.rs`, `src/ipc.rs`, `src/rpc.rs`, `src/command.rs`, `src/acquisition.rs`, `src/metadata.rs`, `src/error.rs`, `build.rs`, `src-ts/main.ts`, `src-ts/preload.ts`, and everything under `src-ts/runtime/`, `src-ts/socket/`, `src-ts/utils/`.

Every statement below is read from those sources. Where the Rust and TypeScript sides disagree, the disagreement is recorded as a numbered defect (`D1`–`D6` in §1.7) and referenced wherever it matters, rather than smoothed over.

---

## 0. Wire protocol (needed by every later section)

One TCP connection, `127.0.0.1`, newline-delimited JSON, UTF-8. Rust is the **listener**; Electron is the **client**. Electron owns the connection from inside a `worker_threads` Worker (`src-ts/socket/ipc.ts:196-249`) so that the main thread can block on `Atomics.wait` during native policy callbacks without starving the socket.

**Rust → Electron** (`src/ipc.rs:367-411`), one JSON object per line:

```jsonc
{ "id": <u64>, "method": "<string>", "params": <any> }            // ordinary RPC
{ "id": <u64>, "method": "...", "params": ..., "asynchronous": true }  // RpcRequest::deferred()
{ "id": <u64>, "method": "...", "params": ..., "notification": true }  // RpcRequest::notification()
```

`id` comes from one `AtomicU64` per `Electron` (`next_id`), shared with command task IDs.

**Electron → Rust** (`src-ts/socket/transport.ts:59-71`, `src-ts/socket/ipc.ts:102-146`), one JSON object per line:

```jsonc
{ "event": "<string>", "params": { ... } }                 // one-way notification
{ "event": "<string>", "params": { ... , "id": <n> } }     // blocking policy request (no top-level id)
{ "id": <u64>, "result": <any> }                           // RPC reply
{ "id": <u64>, "error": "<string>" }                       // RPC failure (string only)
```

Rust's dispatcher (`src/ipc.rs:547-572`) decides by shape:

1. `message["event"] == "command.output"` **or** `message["method"] == "command.output"` → routed to the command-output channel, never published as an event.
2. Otherwise, if `message["id"]` is a `u64` → resolves the pending RPC with `result` (or `Error::Rpc { method, message }` when `error` is a string).
3. Otherwise → published to every event subscriber as an opaque `serde_json::Value`.

Blocking policy requests therefore arrive at Rust as **events** whose `params.id` is the JS-side request number; Rust answers them with an ordinary RPC (`webview.newWindowResponse`, `webview.navigationResponse`, `webview.downloadResponse`, `webview.permissionResponse`, `protocol.respond`) that the blocked main thread processes re-entrantly (`src-ts/socket/ipc.ts:161-177`, whitelist at `src-ts/socket/transport.ts:5-18`).

---

## 1. Rust public API

### 1.1 Crate root — `src/lib.rs`

Re-exports (`src/lib.rs:6-21`):

```rust
mod acquisition; mod command; mod error; mod ipc; mod metadata;
pub mod rpc;
#[cfg(windows)] mod pe;

pub use command::{Command, CommandChunk, OutputStream};
pub use ipc::{Electron, Event, RpcRequest};
pub use rpc::{app::SetTheme, Theme};
pub use error::{ElectronError, Error};
```

Constants and free functions:

```rust
pub const DEFAULT_VERSION: &str = "44.7.0";

pub fn platform() -> String;                     // "win32" | "darwin" | "linux"
pub fn arch() -> String;                         // "x64" | "arm64" | "ia32" | "armv7l"
pub fn executable() -> String;                   // "electron.exe" | "Electron.app/Contents/MacOS/Electron" | "electron"

pub fn electron_binary(path: impl AsRef<Path>) -> Result<PathBuf, Error>;
#[macro_export] macro_rules! electron_binary { ($path:expr $(,)?) => { ... } }

pub fn node(binary: impl AsRef<Path>) -> tokio::process::Command;
pub fn electron(binary: impl AsRef<Path>) -> tokio::process::Command;
```

`platform()`, `arch()`, `executable()` are thin wrappers over the private `acquisition::DistributionTarget` (`src/acquisition.rs:15-119`); all three derive from `TARGET` (falling back to the host triple) and are string-matched rather than validated, so an unknown CPU string is returned verbatim by `arch()` and any non-Windows/non-Apple OS is reported as `"linux"`. The validated path is `DistributionTarget::from_triple`, which is private and rejects unsupported pairs.

Types:

```rust
#[derive(Debug)]
pub struct PreparedDistribution {
  pub directory: PathBuf,
  pub binary: PathBuf,
  pub cleanup_warning: Option<Error>,
}

#[derive(Debug, Clone, Default)]
pub struct Metadata {
  pub app_name: Option<String>,
  pub identifier: Option<String>,
  pub icon: Option<PathBuf>,            // Windows .ico or macOS .icns
  pub host_executable: Option<PathBuf>, // Windows-only icon/version donor
}

#[derive(Debug, Clone)]
pub struct ElectronBuilder { /* private */ }
```

`ElectronBuilder` methods (all consuming-`self` fluent except the resolvers):

```rust
pub fn new(version: impl Into<String>) -> Self;
pub fn path(mut self, path: impl Into<PathBuf>) -> Self;            // ordered search path, may start with "{resources}"
pub fn resources_dir(mut self, path: impl Into<PathBuf>) -> Self;   // resolves "{resources}"
pub fn output(mut self, path: impl Into<PathBuf>) -> Self;          // default target/electron/<platform>-<arch>
pub fn app_name(mut self, name: impl Into<String>) -> Self;
pub fn identifier(mut self, identifier: impl Into<String>) -> Self;
pub fn icon(mut self, path: impl Into<PathBuf>) -> Self;
pub fn host_executable(mut self, path: impl Into<PathBuf>) -> Self;
pub fn metadata(mut self, metadata: Metadata) -> Self;
pub fn pe(mut self, enabled: bool) -> Self;

pub fn build(&self) -> Result<PreparedDistribution, Error>;
pub fn binary(&self) -> Result<PathBuf, Error>;
pub fn directory(&self) -> Result<PathBuf, Error>;
pub fn node(&self) -> io::Result<tokio::process::Command>;
pub fn electron(&self) -> io::Result<tokio::process::Command>;
```

`pe_enabled()` (`src/lib.rs:96-102`) resolves `pe` as: explicit value → `PROFILE=debug` ⇒ `false` → `PROFILE=release` ⇒ `true` → `!cfg!(debug_assertions)`.

### 1.2 `src/error.rs`

```rust
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ElectronError {
  IpcTimeout { timeout_duration: Duration },
  ChecksumMismatch { expected: String, actual: String },
  ChannelDisconnected,
  Rpc { method: String, message: String },
  InvalidInput(String),
  Io(#[from] std::io::Error),
  Json(#[from] serde_json::Error),
  Utf16(#[from] std::string::FromUtf16Error),
  #[cfg(windows)] Windows(#[from] windows::core::Error),
  Context { message: String, #[source] source: Box<dyn std::error::Error + Send + Sync> },
}

pub type Error = ElectronError;

impl From<String> for ElectronError;   // → InvalidInput
impl From<&str> for ElectronError;     // → InvalidInput
```

`with_source`/`context` are `pub(crate)`. Display strings (they are part of the observable contract, `src/error.rs:7-31`):

| variant | message |
| --- | --- |
| `IpcTimeout` | `Electron RPC timed out after {timeout_duration:?}` |
| `ChecksumMismatch` | `Electron checksum mismatch: expected {expected}, got {actual}` |
| `ChannelDisconnected` | `Electron IPC channel disconnected` |
| `Rpc` | ``Electron RPC `{method}` failed: {message}`` |
| `InvalidInput` | `{0}` |
| `Io` | `I/O operation failed: {0}` |
| `Json` | `invalid JSON: {0}` |
| `Utf16` | `invalid UTF-16: {0}` |
| `Windows` | `Windows operation failed: {0}` |
| `Context` | `{message}: {source}` |

### 1.3 `src/ipc.rs`

```rust
pub type Event = serde_json::Value;

pub trait RpcRequest: Serialize + Send + 'static {
  type Response: DeserializeOwned + Send + 'static;
  const METHOD: &'static str;
  fn method(&self) -> &'static str { Self::METHOD }
  fn timeout(&self) -> Duration { Duration::from_secs(5) }
  fn deferred(&self) -> bool { false }
  fn notification(&self) -> bool { false }
}

pub struct Electron { /* private */ pub version: String }

impl Electron {
  pub async fn new(binary: impl AsRef<Path>, port: u16) -> Result<Arc<Self>, Error>;
  pub async fn with_args(
    binary: impl AsRef<Path>,
    port: u16,
    args: &[String],
    timeout: Duration,
  ) -> Result<Arc<Self>, Error>;

  pub fn events(&self) -> Result<std::sync::mpsc::Receiver<Event>, Error>;
  pub fn on(&self, handler: impl Fn(Event) + Send + 'static);
  pub fn subscribe(&self) -> std::sync::mpsc::Receiver<Event>;

  pub async fn call<R: RpcRequest>(&self, request: R) -> Result<R::Response, Error>;

  pub fn node(&self) -> tokio::process::Command;
  pub fn run(self: &Arc<Self>, script: &str) -> crate::Command;
  pub fn shutdown(&self);
}

impl std::fmt::Debug for Electron;   // { binary, version }
impl Drop for Electron;              // calls shutdown()
```

Crate-internal but load-bearing for `Command` (`src/command.rs` calls them):

```rust
pub(crate) async fn call_with_timeout<R: RpcRequest>(&self, request: R, timeout: Option<Duration>) -> Result<R::Response, Error>;
pub(crate) fn command_task_id(&self) -> Result<String, Error>;      // "command-<n>"
pub(crate) fn register_command(&self, task_id: String, route: CommandRoute) -> Result<(), Error>;
pub(crate) fn remove_command(&self, task_id: &str);
```

Behavioural facts a consumer must rely on:

* `new(binary, port)` = `with_args(binary, port, &[], Duration::from_secs(15))`; `port == 0` binds an ephemeral port and the actual port is what Electron receives.
* `with_args` runs the whole blocking startup on `spawn_blocking`; a zero timeout is rejected (`"Electron startup timeout must be positive"`).
* `version` is the string Electron reported in the handshake (`/params/electron`), or `"unknown"` when absent.
* `events()` yields the **initial** receiver exactly once (error `"Electron events receiver already taken"` afterwards); it is buffered from launch with capacity 1024. On overflow the event is dropped and a `log::warn!` is emitted — the channel is deliberately *not* closed because it drives the runtime run loop.
* `subscribe()` creates an additional 1024-slot channel; on overflow that subscription is closed (and logged), other subscriptions and RPC continue. Subscribing after disconnect returns an already-closed receiver.
* `on(handler)` = `subscribe()` + a dedicated `electron-events` thread; a blocked handler never blocks the RPC reader.
* `call()` uses `RpcRequest::timeout()` (default 5 s). `notification()` requests write the line and return `Ok(Value::Null)` without registering a pending entry.
* `shutdown()` is idempotent: marks closed, fails all command routes with `ChannelDisconnected`, fails all pending RPCs with `ChannelDisconnected`, best-effort `try_send({"event":"ipc.closed"})` to every subscriber, `shutdown(Both)` on the socket, drops the Windows job object (kills the whole process tree), then `kill()` + `wait()` on the child.
* The reader thread (`electron-rpc`) calls `shutdown()` on EOF, so Electron dying tears the client down.
* Event capacity constants: `EVENT_CAPACITY = 1024`, `STDERR_TAIL_LINES = 32`, `STDERR_TAIL_LINE_CHARS = 400`, `STDERR_TAIL_SETTLE = 250 ms`.

### 1.4 `src/command.rs`

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputStream { Stdout, Stderr }

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandChunk { pub sequence: u64, pub stream: OutputStream, pub data: Vec<u8> }

#[must_use = "await the command to execute it"]
pub struct Command { /* private */ }

impl Command {
  pub fn on_output(mut self, callback: impl FnMut(CommandChunk) + Send + 'static) -> Self;
  pub fn timeout(mut self, timeout: Duration) -> Self;
  pub fn max_output_bytes(mut self, bytes: usize) -> Self;
}

impl IntoFuture for Command {
  type Output = Result<std::process::Output, Error>;
  type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send + 'static>>;
}
```

Limits: timeout must be `1..=2_147_483_647` whole milliseconds; `max_output_bytes` must be `1..=256 MiB` (default 16 MiB); the output queue holds 256 chunks; the RPC response deadline is `timeout + 1 s` when a timeout is set, unbounded otherwise. `command.execute` params on the wire: `{ taskId, script, timeoutMs: Option<u64>, maxOutputBytes, stream }`; the reply is `{ code: i32, stdout: base64, stderr: base64 }`; streamed chunks arrive as `command.output` notifications `{ taskId, chunks: [{ sequence, stream, data(base64) }] }`. Dropping a pending `Command` future spawns `command.cancel { taskId }` as a notification.

### 1.5 `src/acquisition.rs` and `src/metadata.rs`

Both modules are private (`mod acquisition; mod metadata;`), so **none** of their items are part of the public API. The observable contract they create is documented in §5. Only `acquisition::target_triple()` is `pub` inside a private module, hence unreachable.

### 1.6 `src/rpc.rs` — supporting types

```rust
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme { Dark, Light, #[default] System }              // "dark" | "light" | "system"

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ColorScheme { Dark, Light }                          // "dark" | "light"

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Point { pub x: i32, pub y: i32 }

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LogicalPosition { pub x: f64, pub y: f64 }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Size { pub width: u32, pub height: u32 }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rectangle { pub x: i32, pub y: i32, pub width: u32, pub height: u32 }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]      // camelCase
pub struct Display {
  pub name: String,
  pub position: Rectangle,
  pub size: Size,
  pub work_area: WorkArea,
  pub scale_factor: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkArea { pub position: Rectangle, pub size: Size }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DisplayList { pub displays: Vec<Display>, pub primary: Display }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Color { Css(String), Rgb([u8; 3]), Rgba([u8; 4]) }
```

### 1.7 Known Rust-vs-JavaScript discrepancies

These are defects in the current crate, not design choices; an implementer must not copy them.

| # | Where | Discrepancy |
| --- | --- | --- |
| D1 | `rpc::app::OpenPath` (`src/rpc.rs:740`) vs `src-ts/runtime/commands.ts:211` | Rust declares `Response = String`; the JS handler is `shell.openPath(...).then(() => null)`, i.e. always `null`. Deserialising `null` into `String` fails, so `Electron::call(app::OpenPath { .. })` returns `Error::Json`. Either the JS must return the string or the Rust response must be `()`. |
| D2 | `rpc::window::SetBackgroundColor` (`src/rpc.rs:434`) vs `webview::SetBackgroundColor` (`src/rpc.rs:664`) | The window variant takes `Option<Color>` (string or `[u8;3]`/`[u8;4]`), the webview variant takes `Option<String>`. The JS accepts both shapes for both (`cssColor`), so the webview type is narrower than the bridge. |
| D3 | `rpc::window::Config` (`src/rpc.rs:285-357`) | `windowEffects`, `titleBarOverlay`, `tabbingIdentifier`, `javascriptDisabled`, `devtools`, `backgroundThrottling`, `userAgent`, `authenticatedWebHostSession`, `browserHost` are read by the JS (`src-ts/runtime/factory.ts:213-262`, `src-ts/runtime/window.ts:21-32`) but are **not typed**; they travel through `#[serde(flatten)] additional: BTreeMap<String, Value>`. |
| D4 | `rpc::webview::Config` (`src/rpc.rs:527-562`) | Same for `guestHost`, `authenticatedWebHostSession`, `browserHost`, `ipcOrigin`, `authenticatedWebHost` (read at `src-ts/runtime/factory.ts:145-175`, `src-ts/runtime/webview.ts:97-152`). |
| D5 | `rpc::window::SetBadgeCount` (`src/rpc.rs:432`) | Lives in the `window` module but mutates the application badge (`app.setBadgeCount`), as the doc comment admits. |
| D6 | `RpcRequest::notification()` | No request in `rpc.rs` sets it; the only notification in the crate is the internal `command::Cancel`. |

### 1.8 `src/rpc.rs` — every request struct

All request structs are generated by the `requests!` macro (`src/rpc.rs:20-41`), so they uniformly derive `Debug, Clone, PartialEq, Serialize, Deserialize` with `#[serde(rename_all = "camelCase")]`, and implement `RpcRequest` with `METHOD` set to the literal below. Fields marked *skip-if-none* are `#[serde(skip_serializing_if = "Option::is_none")]`; fields marked *explicit null* are not skipped and serialise as `null`.

`Flags` legend: `—` = plain (5 s timeout, not deferred, not a notification); `D` = `deferred()` returns `true` (adds `"asynchronous": true` to the wire request); `T(n)` = `timeout()` returns `n` seconds. **No request in this module is a notification.**

Total: 108 request structs (58 `window`, 31 `webview`, 4 `browser`, 5 `protocol`, 10 `app`). The crate asserts this set equals `src-ts/runtime/commands.ts` minus the two internal `command.*` entries (`src/rpc.rs:762-794`).

#### 1.8.1 `rpc::window`

Supporting types (`src/rpc.rs:121-357`):

```rust
#[serde(rename_all = "camelCase")]
pub enum ConstraintAxis { MinWidth, MinHeight, MaxWidth, MaxHeight }

#[serde(untagged)]
pub enum SizeConstraint { Logical(f64), Scaled { value: f64, physical: bool } }

pub type SizeConstraints = BTreeMap<ConstraintAxis, Option<SizeConstraint>>;  // missing = unchanged, None = clear

#[serde(rename_all = "camelCase")]
pub struct WindowGeometry {
  pub scale_factor: f64, pub bounds: Rectangle, pub content_bounds: Rectangle,
  pub content_constraints: BTreeMap<ConstraintAxis, f64>,
  pub maximized: bool, pub minimized: bool, pub focused: bool, pub visible: bool,
  pub fullscreen: bool, pub resizable: bool, pub always_on_top: bool, pub decorated: bool,
  pub title: String, pub maximizable: bool, pub minimizable: bool, pub closable: bool,
}

pub struct Created { pub window_id: u32 }
pub struct NativeWindowHandle { pub handle: u64, pub process_id: u32 }
pub struct ThemeState { pub theme: ColorScheme }
pub enum BackgroundMaterial { None, Acrylic, Mica, Tabbed }        // lowercase
pub enum ProgressState { None, Normal, Paused, Error }             // lowercase
pub enum TitleBarStyle { Visible, Transparent, Overlay }           // lowercase
pub struct Icon { pub width: u32, pub height: u32, pub rgba: Vec<u8> }
pub struct CreationIcon { pub width: u32, pub height: u32, pub bgra: String }   // base64 BGRA
pub enum MenuItemType { Separator, Checkbox }                      // lowercase, wire key "type"
pub struct MenuItem {
  pub kind: Option<MenuItemType>,   // "type"
  pub role: Option<String>, pub label: Option<String>, pub enabled: Option<bool>,
  pub accelerator: Option<String>, pub checked: Option<bool>, pub command_id: Option<u32>,
  pub submenu: Option<Vec<MenuItem>>,
}
#[serde(untagged)] pub enum PreventOverflow { Enabled(bool), Margin { width: Option<f64>, height: Option<f64> } }
pub struct AppDetails { pub app_id: String, pub relaunch_command: Option<String>, pub relaunch_display_name: Option<String> }
pub struct Config { /* 34 typed fields + additional */ }
```

`window::Config` fields (`src/rpc.rs:285-357`), all `Option<…>` and skip-if-none except `additional`:
`title: String`, `width: f64`, `height: f64`, `x: f64`, `y: f64`, `min_width: SizeConstraint`, `min_height: SizeConstraint`, `max_width: SizeConstraint`, `max_height: SizeConstraint`, `transparent: bool`, `decorations: bool`, `visible: bool`, `focus: bool`, `focusable: bool`, `center: bool`, `maximized: bool`, `fullscreen: bool`, `resizable: bool`, `maximizable: bool`, `minimizable: bool`, `closable: bool`, `always_on_top: bool`, `skip_taskbar: bool`, `shadow: bool`, `background_color: Color`, `title_bar_style: TitleBarStyle`, `title_bar_hidden: bool`, `hidden_title: bool`, `traffic_light_position: LogicalPosition`, `visible_on_all_workspaces: bool`, `content_protected: bool`, `prevent_overflow: PreventOverflow`, `theme: Theme`, `webview_tag: bool`, plus `#[serde(flatten)] additional: BTreeMap<String, Value>`.

| Struct | Method | Params (wire) | Response | Flags |
| --- | --- | --- | --- | --- |
| `Create` | `window.create` | `windowId: u32`, `window: Config` (*serde default*, so omittable), `webview?: webview::Config`, `sizeConstraints?: SizeConstraints`, `newWindowRequest?: u64`, `deferShow?: bool`, `windowIcon?: CreationIcon`, `icon?: String`, `appDetails?: AppDetails` | `window::Created` = `{ windowId }` | `T(60)` |
| `Ready` | `window.ready` | `windowId: u32` | `()` | `D` |
| `SetTitle` | `window.setTitle` | `windowId: u32`, `title: String` | `()` | — |
| `Geometry` | `window.geometry` | `windowId: u32` | `WindowGeometry` | — |
| `SetSize` | `window.size` | `windowId: u32`, `width: f64`, `height: f64`, `physical: bool` | `()` | — |
| `SetPosition` | `window.position` | `windowId: u32`, `x: f64`, `y: f64`, `physical: bool` | `()` | — |
| `SetMinSize` | `window.minSize` | `windowId: u32`, `width?: f64`, `height?: f64`, `physical: bool` | `()` | — |
| `SetMaxSize` | `window.maxSize` | `windowId: u32`, `width?: f64`, `height?: f64`, `physical: bool` | `()` | — |
| `SetSizeConstraints` | `window.sizeConstraints` | `windowId: u32`, `constraints: SizeConstraints` | `()` | — |
| `SetResizable` | `window.resizable` | `windowId: u32`, `resizable: bool` | `()` | — |
| `SetMaximizable` | `window.maximizable` | `windowId: u32`, `maximizable: bool` | `()` | — |
| `SetMinimizable` | `window.minimizable` | `windowId: u32`, `minimizable: bool` | `()` | — |
| `SetClosable` | `window.closable` | `windowId: u32`, `closable: bool` | `()` | — |
| `Maximize` | `window.maximize` | `windowId: u32` | `()` | — |
| `Unmaximize` | `window.unmaximize` | `windowId: u32` | `()` | — |
| `Minimize` | `window.minimize` | `windowId: u32` | `()` | — |
| `Unminimize` | `window.unminimize` | `windowId: u32` | `()` | `D`, `T(15)` |
| `SetFullscreen` | `window.fullscreen` | `windowId: u32`, `fullscreen: bool`, `x?: f64`, `y?: f64` | `()` | `D`, `T(25)` |
| `SetAlwaysOnTop` | `window.alwaysOnTop` | `windowId: u32`, `alwaysOnTop: bool` | `()` | — |
| `SetCursor` | `window.cursor` | `windowId: u32`, `cursor: String` | `()` | `D` |
| `SetCursorIcon` | `window.setCursorIcon` | `windowId: u32`, `cursor: String` | `()` | `D` |
| `SetCursorVisible` | `window.setCursorVisible` | `windowId: u32`, `visible: bool` | `()` | `D` |
| `SetParentWindow` | `window.setParentWindow` | `windowId: u32`, `parentId: Option<u32>` (**explicit null** = detach) | `()` | — |
| `SetMenu` | `window.setMenu` | `windowId: u32`, `menu: Option<Vec<MenuItem>>` (**explicit null** = remove; `[]` = empty menu) | `()` | — |
| `SetSkipTaskbar` | `window.skipTaskbar` | `windowId: u32`, `skip: bool` | `()` | — |
| `SetFocusable` | `window.focusable` | `windowId: u32`, `focusable: bool` | `()` | — |
| `SetContentProtected` | `window.contentProtected` | `windowId: u32`, `protected: bool` | `()` | — |
| `Center` | `window.center` | `windowId: u32` | `()` | — |
| `RequestAttention` | `window.requestAttention` | `windowId: u32`, `flash: bool` | `()` | — |
| `Show` | `window.show` | `windowId: u32` | `()` | — |
| `Hide` | `window.hide` | `windowId: u32` | `()` | — |
| `Focus` | `window.focus` | `windowId: u32` | `()` | — |
| `NativeHandle` | `window.nativeHandle` | `windowId: u32` | `NativeWindowHandle` = `{ handle: u64, processId: u32 }` | — |
| `Close` | `window.close` | `windowId: u32` | `()` | — |
| `Destroy` | `window.destroy` | `windowId: u32` | `()` | — |
| `SetVisibleOnAllWorkspaces` | `window.visibleOnAllWorkspaces` | `windowId: u32`, `value: bool` | `()` | — |
| `SetIcon` | `window.icon` | `windowId: u32`, `icon: Icon` = `{ width, height, rgba }` | `()` | — |
| `SetEnabled` | `window.enabled` | `windowId: u32`, `enabled: bool` | `()` | — |
| `IsEnabled` | `window.enabledQuery` | `windowId: u32` | `bool` | — |
| `SetSimpleFullscreen` | `window.simpleFullscreen` | `windowId: u32`, `fullscreen: bool` | `()` | — |
| `SetProgressBar` | `window.progressBar` | `windowId: u32`, `state: ProgressState`, `progress: f64` | `()` | — |
| `SetShadow` | `window.shadow` | `windowId: u32`, `value: bool` | `()` | — |
| `SetDecorations` | `window.decorations` | `windowId: u32`, `value: bool` | `()` | — |
| `SetBadgeCount` | `window.badgeCount` | `count: u32` (**no `windowId`**) | `()` | — |
| `SetOverlayIcon` | `window.overlayIcon` | `windowId: u32`, `icon: Option<Icon>` (**explicit null**), `description: String` | `()` | — |
| `SetBackgroundColor` | `window.backgroundColor` | `windowId: u32`, `color: Option<Color>` (**explicit null**) | `()` | — |
| `SetBackgroundMaterial` | `window.backgroundMaterial` | `windowId: u32`, `material: BackgroundMaterial` | `()` | — |
| `SetTheme` | `window.theme` | `windowId: u32`, `theme: Theme` | `()` | `D` |
| `IgnoreCursorEvents` | `window.ignoreCursorEvents` | `windowId: u32`, `ignore: bool` | `()` | — |
| `QueryTheme` | `window.themeQuery` | `windowId: u32` | `ThemeState` = `{ theme: "dark" \| "light" }` | `D` |
| `Displays` | `window.displays` | `{}` | `DisplayList` | — |
| `CursorPosition` | `window.cursorPosition` | `{}` | `Point` = `{ x: i32, y: i32 }` | — |
| `CurrentDisplay` | `window.currentDisplay` | `windowId: u32` | `Display` | — |
| `DisplayForPoint` | `window.displayForPoint` | `x: f64`, `y: f64` | `Option<Display>` (`null` when no display contains the point) | — |
| `SetTitleBarStyle` | `window.titleBarStyle` | `windowId: u32`, `style: TitleBarStyle` | `()` | `D` |
| `SetTrafficLight` | `window.trafficLight` | `windowId: u32`, `x: f64`, `y: f64`, `physical: bool` | `()` | — |
| `StartDrag` | `window.startDrag` | `windowId: u32` | `()` | — |
| `PopupMenu` | `window.popupMenu` | `windowId: u32`, `menu: Vec<MenuItem>`, `x?: f64`, `y?: f64` | `Option<u32>` (chosen `commandId`, `null` if dismissed) | `D`, `T(300)` |

#### 1.8.2 `rpc::webview`

Supporting types (`src/rpc.rs:457-636`):

```rust
pub struct Created { pub webview_id: u32 }
pub struct Snapshot { pub scale_factor: f64, pub bounds: Rectangle, pub content_bounds: Rectangle, pub visible: bool }
pub struct Position { pub x: f64, pub y: f64, pub physical: Option<bool> }
pub struct Extent { pub width: f64, pub height: f64, pub physical: Option<bool> }
pub struct BoundsInput { pub position: Option<Position>, pub size: Option<Extent>, pub physical: Option<bool> }
pub struct Policy { pub navigation: Option<bool>, pub download: Option<bool>, pub permission: Option<bool> }
#[serde(untagged)] pub enum InitScript { Source(String), Detailed { script: String, main_frame_only: bool /* "mainFrameOnly" */ } }
pub struct Config { pub webview_id: u32, pub url: String, /* + 17 optional fields + additional */ }
pub enum NewWindowAction { Allow, Deny }              // lowercase
pub struct NewWindowDecision { pub action: NewWindowAction, pub window_id: Option<u32> }
pub enum PermissionDecision { Allow, Deny, Default }  // lowercase
pub enum SameSite { Unspecified, NoRestriction, Lax, Strict }   // snake_case
pub struct Cookie { name, value, domain?, host_only?, path?, secure?, http_only?, session, expiration_date?, same_site }
pub struct CookieInit { url, name, value, domain?, path?, secure?, http_only?, same_site?, expiration_date? }
```

`webview::Config` optional fields (`src/rpc.rs:527-562`): `auto_resize: bool`, `bounds: BoundsInput`, `policy: Policy`, `incognito: bool`, `data_directory: String`, `user_agent: String`, `init_scripts: Vec<InitScript>`, `preload_scripts: Vec<String>`, `background_color: Color`, `transparent: bool`, `accept_first_mouse: bool`, `javascript_disabled: bool`, `devtools: bool`, `background_throttling: String`, plus `#[serde(flatten)] additional`.

| Struct | Method | Params (wire) | Response | Flags |
| --- | --- | --- | --- | --- |
| `Create` | `webview.create` | `windowId: u32` + `#[serde(flatten)] Config` ⇒ `webviewId`, `url`, and the optional fields above | `webview::Created` = `{ webviewId }` | `D` |
| `Url` | `webview.url` | `webviewId: u32` | `String` | — |
| `Eval` | `webview.eval` | `webviewId: u32`, `script: String`, `documentGeneration?: u64` | `serde_json::Value` | `D` |
| `NewWindowResponse` | `webview.newWindowResponse` | `id: u64`, `response: NewWindowDecision` | `()` | — |
| `NavigationResponse` | `webview.navigationResponse` | `id: u64`, `allow: bool` | `()` | — |
| `DownloadResponse` | `webview.downloadResponse` | `id: u64`, `allow: bool`, `destination?: String` | `()` | — |
| `PermissionResponse` | `webview.permissionResponse` | `id: u64`, `response: PermissionDecision` | `()` | — |
| `Navigate` | `webview.navigate` | `webviewId: u32`, `url: String` | `()` | `D` |
| `Reload` | `webview.reload` | `webviewId: u32` | `()` | `D` |
| `SetAutoResize` | `webview.setAutoResize` | `webviewId: u32`, `autoResize: bool` | `()` | `D` |
| `GoBack` | `webview.goBack` | `webviewId: u32` | `()` | `D` |
| `CanGoBack` | `webview.canGoBack` | `webviewId: u32` | `bool` | `D` |
| `GoForward` | `webview.goForward` | `webviewId: u32` | `()` | `D` |
| `CanGoForward` | `webview.canGoForward` | `webviewId: u32` | `bool` | `D` |
| `OpenDevtools` | `webview.openDevtools` | `webviewId: u32` | `()` | `D`, `T(15)` |
| `CloseDevtools` | `webview.closeDevtools` | `webviewId: u32` | `()` | `D` |
| `IsDevtoolsOpen` | `webview.isDevtoolsOpen` | `webviewId: u32` | `bool` | `D` |
| `SetZoom` | `webview.setZoom` | `webviewId: u32`, `scaleFactor: f64` | `()` | `D` |
| `Reparent` | `webview.reparent` | `webviewId: u32`, `windowId: u32` | `()` | `D` |
| `Bounds` | `webview.bounds` | `webviewId: u32` | `Snapshot` | `D` |
| `SetBounds` | `webview.setBounds` | `webviewId: u32` + `#[serde(flatten)] BoundsInput` ⇒ `position?`, `size?`, `physical?` | `()` | `D` |
| `SetVisibility` | `webview.visibility` | `webviewId: u32`, `visible: bool` | `()` | `D` |
| `SetBackgroundColor` | `webview.backgroundColor` | `webviewId: u32`, `color: Option<String>` (**explicit null**) | `()` | `D` |
| `Focus` | `webview.focus` | `webviewId: u32` | `()` | `D` |
| `Print` | `webview.print` | `webviewId: u32` | `()` | `D` |
| `Capture` | `webview.capture` | `webviewId: u32` | `String` (data URL) | `D` |
| `Close` | `webview.close` | `webviewId: u32` | `()` | `D` |
| `Cookies` | `webview.cookies` | `webviewId: u32`, `url?: String` | `Vec<Cookie>` | `D` |
| `SetCookie` | `webview.setCookie` | `webviewId: u32` + `#[serde(flatten)] CookieInit` | `()` | `D` |
| `DeleteCookie` | `webview.deleteCookie` | `webviewId: u32`, `url: String`, `name: String` | `()` | `D` |
| `ClearBrowsingData` | `webview.clearBrowsingData` | `webviewId: u32` | `()` | `D` |

#### 1.8.3 `rpc::browser`

```rust
pub struct Reservation { pub lease: String, pub partition: String }
```

| Struct | Method | Params (wire) | Response | Flags |
| --- | --- | --- | --- | --- |
| `SetTrustedOrigin` | `webview.trustedOrigin` | `webviewId: u32`, `origin: String` | `()` | `D` |
| `SetTrustedOriginAndNavigate` | `webview.trustedOriginAndNavigate` | `webviewId: u32`, `origin: String`, `url: String` | `()` | `D` |
| `Acquire` | `browser.acquire` | `webviewId: u32`, `workspace: String` | `Reservation` = `{ lease, partition }` | `D` |
| `Release` | `browser.release` | `webviewId: u32`, `lease: String` | `()` | `D` |

Note the namespace split: two of the four live under `webview.*` on the wire.

#### 1.8.4 `rpc::protocol`

```rust
pub struct AuthenticatedHost { pub id: String, pub document_url: String, pub backend_origin: String }
```

| Struct | Method | Params (wire) | Response | Flags |
| --- | --- | --- | --- | --- |
| `AuthenticateHost` | `webHost.authenticate` | `url: String` | `AuthenticatedHost` = `{ id, documentUrl, backendOrigin }` | `D`, `T(60)` |
| `HostCookieHeader` | `webHost.cookieHeader` | `hostId: String` | `String` | `D` |
| `BindHost` | `webHost.bind` | `webviewId: u32`, `hostId: String` | `()` | — |
| `Register` | `protocol.register` | `scheme: String` | `()` | — |
| `Respond` | `protocol.respond` | `id: u64`, `status?: u16`, `headers?: BTreeMap<String, String>`, `body?: String` (base64; ignored for HEAD/204/205/304) | `()` | — |

#### 1.8.5 `rpc::app`

```rust
pub enum ActivationPolicy { Regular, Accessory, Prohibited }   // lowercase
```

| Struct | Method | Params (wire) | Response | Flags |
| --- | --- | --- | --- | --- |
| `SetMenu` | `app.setMenu` | `menu: Option<Vec<window::MenuItem>>` (**explicit null** = no menu) | `()` | `D` |
| `SetBadgeLabel` | `app.badgeLabel` | `label: String` | `()` | `D` |
| `Focus` | `app.focus` | `steal: bool` | `()` | `D` |
| `Show` | `app.show` | `{}` | `()` | `D` |
| `Hide` | `app.hide` | `{}` | `()` | `D` |
| `SetActivationPolicy` | `app.activationPolicy` | `policy: ActivationPolicy` | `()` | `D` |
| `SetDockVisibility` | `app.dockVisibility` | `visible: bool` | `()` | `D` |
| `SetTheme` | `app.theme` | `theme: Theme` | `()` | `D` |
| `OpenExternal` | `shell.openExternal` | `url: String` | `()` | `D` |
| `OpenPath` | `shell.openPath` | `path: String` | `String` — see **D1** | `D` |

The crate enforces that `deferred()` agrees with the JS synchronous whitelist: `deferred == !synchronousMethods.contains(method)` for every built-in (`src/rpc.rs:1097-1120`, whitelist at `src-ts/socket/transport.ts:5-18`).

---

## 2. Event catalogue

### 2.1 Events the JavaScript side sends to Rust

Every entry is a `transport.send(event, params)` call, which becomes one `{ "event": …, "params": … }` line. Types are the TypeScript types at the send site; `number` is a JSON number, `string` a JSON string.

| # | Event | Payload | Sent from |
| --- | --- | --- | --- |
| 1 | `ready` | `{ electron: string, token: string }` — `electron` is `process.versions.electron`; `token` is the 64-hex credential read from stdin | `src-ts/main.ts:85` (the handshake line, §4.4) |
| 2 | `app.menuCommand` | `{ commandId: number }` | `src-ts/main.ts:98` (application menu click) |
| 3 | `app.exitRequested` | `{}` | `src-ts/main.ts:198` (`before-quit` without a closed transport) |
| 4 | `app.reopen` | `{ hasVisibleWindows: boolean }` | `src-ts/main.ts:203` (macOS `activate`) |
| 5 | `ipc.message` | `{ webviewId: number, documentGeneration: number, url: string, body: string }` — `body` is the opaque string the page passed to `window.ipc.postMessage` | `src-ts/main.ts:171` |
| 6 | `download.finished` | `{ webviewId: number, url: string, path: string \| null, success: boolean }` (`DownloadFinished`) | `src-ts/main.ts:175`; also `src-ts/runtime/policy.ts:56` on the `mkdtemp` failure path with `path: null, success: false` |
| 7 | `download.requested` | `{ id: number, webviewId: number, url: string, destination: string }` | `src-ts/runtime/policy.ts:56` |
| 8 | `permission.requested` | `{ id: number, webviewId: number, kind: PermissionKind }` where `PermissionKind = 'geolocation' \| 'notifications' \| 'clipboard-read' \| 'display-capture' \| 'midi' \| 'sensors' \| 'camera' \| 'microphone'` | `src-ts/runtime/policy.ts:62` |
| 9 | `webview.navigationRequested` | `{ id: number, webviewId: number, url: string }` | `src-ts/runtime/policy.ts:48` |
| 10 | `webview.newWindow` | **blocking request** (`transport.request`): `{ request: number, webviewId: number, url: string, frameName: string, features: string, disposition: string, position: { x: number, y: number } \| null, size: { width: number, height: number } \| null, id: number }`. `id` is appended by `IPC.request` (`src-ts/socket/ipc.ts:127`) and is what `webview.newWindowResponse.id` must echo; `request` is what `window.create.newWindowRequest` must echo. Default timeout 15 000 ms. | `src-ts/runtime/policy.ts:72` |
| 11 | `webview.navigationCommitted` | `{ webviewId: number, url: string }` | `src-ts/runtime/factory.ts:295` |
| 12 | `webview.loadStarted` | `{ webviewId: number, url: string }` | `src-ts/runtime/factory.ts:296` |
| 13 | `webview.loadFinished` | `{ webviewId: number, url: string }` | `src-ts/runtime/factory.ts:297` |
| 14 | `webview.loadFailed` | `{ webviewId: number, url: string }` (main-frame failures only; `ERR_ABORTED`/`-3` suppressed) | `src-ts/runtime/factory.ts:298` |
| 15 | `webview.titleChanged` | `{ webviewId: number, title: string }` | `src-ts/runtime/factory.ts:299` |
| 16 | `webview.processGone` | `{ webviewId: number, reason: string, exitCode: number }` | `src-ts/runtime/factory.ts:300` |
| 17 | `webview.closed` | `{ webviewId: number }` | `src-ts/runtime/factory.ts:193, 306` |
| 18 | `webview.attached` | `{ webviewId: number, windowId: number, contentsId: number, url: string }` — emitted when a `<webview>` guest is adopted; `contentsId` is the Electron `webContents.id` | `src-ts/runtime/factory.ts:369` |
| 19 | `window.closeRequested` | `{ windowId: number }` | `src-ts/runtime/factory.ts:278` |
| 20 | `window.closed` | `{ windowId: number }` | `src-ts/runtime/factory.ts:288` |
| 21 | `window.themeChanged` | `{ windowId: number, theme: 'dark' \| 'light', dark: boolean }` (`ThemeChange` spread after `windowId`) | `src-ts/runtime/factory.ts:280` |
| 22 | `window.event` | `{ windowId: number, event: string, …WindowGeometry }`. `event` ∈ `resize`, `move`, `focus`, `blur`, `maximize`, `unmaximize`, `enter-full-screen`, `leave-full-screen`. The geometry tail is exactly `WindowGeometry` (§2.3). | `src-ts/runtime/factory.ts:282-283` |
| 23 | `menu.command` | `{ windowId: number, commandId: number }` — the emitter type is `number \| undefined`, but `menuTemplate` only installs a `click` handler when `commandId` is a validated integer (`src-ts/runtime/window.ts:61-75`), so the value is always a number in practice | `src-ts/runtime/factory.ts:279` |
| 24 | `protocol.request` | `{ id: number, scheme: string, webviewId: number \| undefined, documentGeneration: number \| undefined, uri: string, method: string, headers: Record<string, string>, body: string }` — `body` base64; `headers.origin` is overwritten with the request's initiator origin when present | `src-ts/runtime/protocol.ts:113` |
| 25 | `browser.openRequested` | `{ webviewId: number, lease: string, url: string }` | `src-ts/runtime/browser.ts:136` |
| 26 | `command.output` | `{ taskId: string, chunks: Array<{ sequence: number, stream: 'stdout' \| 'stderr', data: string }> }` — `data` base64, at most 32 chunks per notification, sequences contiguous from 0 across both streams | `src-ts/runtime/execution.ts:203` |

`undefined` values are dropped by `JSON.stringify`, so `webviewId` and `documentGeneration` in `protocol.request` are **absent keys**, not `null`.

### 2.2 Events Rust generates

| Event | Payload | When |
| --- | --- | --- |
| `ipc.closed` | `{}` | Best-effort `try_send` to every subscriber inside `disconnect()` (`src/ipc.rs:543`). It can be **lost** when the 1024-slot buffer is full — consumers must treat channel closure as the authoritative signal. |

`command.output` never reaches event subscribers: it is intercepted before publishing (`src/ipc.rs:548-551`).

### 2.3 `WindowGeometry` payload (shared by `window.event` and `window.geometry`)

```
scaleFactor: number
bounds: { x: number, y: number, width: number, height: number }        // physical pixels
contentBounds: { x, y, width, height }                                 // physical pixels
contentConstraints: { minWidth?: number, minHeight?: number, maxWidth?: number, maxHeight?: number }  // logical; 0 = unconstrained
maximized: boolean, minimized: boolean, focused: boolean, visible: boolean, fullscreen: boolean,
resizable: boolean, alwaysOnTop: boolean, decorated: boolean, title: string,
maximizable: boolean, minimizable: boolean, closable: boolean
```

### 2.4 How a Rust consumer receives events

```rust
// 1. The runtime run-loop channel: taken exactly once, buffered from launch.
let events: std::sync::mpsc::Receiver<serde_json::Value> = electron.events()?;
for event in events.iter() {
    match event["event"].as_str() {
        Some("window.event") => { /* event["params"]["windowId"], ["event"], ["bounds"], … */ }
        Some("ipc.closed") => break,
        _ => {}
    }
}

// 2. Additional bounded channel for a worker.
let rx = electron.subscribe();

// 3. Callback on a dedicated thread (never blocks the RPC reader).
electron.on(|event| { /* … */ });
```

Semantics that matter:

* `events()` errors with `"Electron events receiver already taken"` on the second call. The channel itself is created at launch and buffers up to 1024 events, so a late `events()` call still sees the backlog.
* `subscribe()` after `shutdown()`/EOF returns a receiver that is already disconnected.
* Overflow policy differs per channel: the initial receiver **drops** the event and logs; secondary subscriptions are **closed** and logged. Neither closes the RPC channel.
* Events are `serde_json::Value`; the crate performs **no** validation, so a consumer must treat every field as optional.
* Ordering: one reader thread publishes in wire order; `command.output` notifications are routed to their command route *before* the matching RPC reply wakes (`src/ipc.rs:566`), so a completed `Command` has already delivered all chunks.

---

## 3. Command table

### 3.1 Transport-level rules (`src-ts/socket/transport.ts:73-103`)

1. `id` must be a safe integer, otherwise the request is dropped silently.
2. If the request arrived on a **synchronous** stack (inside a native policy callback) and the method is not in `synchronousMethods` (`src-ts/socket/transport.ts:5-18`), the reply is `{ id, error: "<method> requires the native policy callback to return first" }`.
3. Unknown method → `{ id, error: "unknown method <method>" }`.
4. A thenable result on a synchronous stack → the same "requires the native policy callback to return first" error; the side effects have already happened.
5. Otherwise the reply is `{ id, result: value ?? null }`, or `{ id, error: <message> }` on throw; the error is also `console.error`d.
6. `notification: true` requests get **no** reply at all.

`synchronousMethods` (exact set): `window.create`, `protocol.register`, `protocol.respond`, `webview.url`, `webHost.bind`, `webview.newWindowResponse`, `webview.navigationResponse`, `webview.downloadResponse`, `webview.permissionResponse`, `window.nativeHandle`, `window.setMenu`, `window.setTitle`, `window.geometry`, `window.icon`, `window.size`, `window.position`, `window.minSize`, `window.maxSize`, `window.sizeConstraints`, `window.resizable`, `window.maximizable`, `window.minimizable`, `window.closable`, `window.maximize`, `window.unmaximize`, `window.minimize`, `window.simpleFullscreen`, `window.alwaysOnTop`, `window.setParentWindow`, `window.skipTaskbar`, `window.focusable`, `window.contentProtected`, `window.center`, `window.requestAttention`, `window.show`, `window.hide`, `window.focus`, `window.close`, `window.destroy`, `window.visibleOnAllWorkspaces`, `window.enabled`, `window.enabledQuery`, `window.progressBar`, `window.shadow`, `window.decorations`, `window.badgeCount`, `window.overlayIcon`, `window.backgroundColor`, `window.backgroundMaterial`, `window.ignoreCursorEvents`, `window.displays`, `window.cursorPosition`, `window.currentDisplay`, `window.displayForPoint`, `window.trafficLight`, `window.startDrag`.

Everything else — including every `webview.*` mutator except the five listed — is answered only outside a blocking callback.

### 3.2 Window methods (`src-ts/runtime/commands.ts:62-131`)

`window(p)` = `windows.get(p.windowId)` and **throws** `unknown window <id>` if absent; `maybe(p)` = `windows.peek(p.windowId)` and is a silent no-op when the window is gone.

| Method | Params read | Returns |
| --- | --- | --- |
| `window.create` | `windowId` (safe int, not already registered), `window` (fallback: the whole params object), `appDetails` (copied into the config when absent), `sizeConstraints` (per-axis, overriding the config), `newWindowRequest`, `deferShow`, `windowIcon` `{width,height,bgra}`, `icon` (string path), `webview` (nested `WebviewInit`) | `{ windowId }` |
| `window.ready` | `windowId` | `null` (reveals the window; honours `visible:false`, `focus:false`, `focusable:false` → `showInactive`) |
| `window.setTitle` | `windowId`, `title` | `null` |
| `window.geometry` | `windowId` | `WindowGeometry` (§2.3) |
| `window.size` | `windowId`, `width`, `height`, `physical` | `null` |
| `window.position` | `windowId`, `x`, `y`, `physical` | `null` |
| `window.minSize` | `windowId`, `width`, `height`, `physical` | `null` |
| `window.maxSize` | `windowId`, `width`, `height`, `physical` | `null` |
| `window.sizeConstraints` | `windowId`, `constraints` (fallback: params itself) | `null` |
| `window.resizable` | `windowId`, `resizable` | `null` |
| `window.maximizable` | `windowId`, `maximizable` (`!== false`, so absent ⇒ true) | `null` |
| `window.minimizable` | `windowId`, `minimizable` (`!== false`) | `null` |
| `window.closable` | `windowId`, `closable` (`!== false`) | `null` |
| `window.maximize` / `window.unmaximize` / `window.minimize` | `windowId` | `null` |
| `window.unminimize` | `windowId` | `Promise<null>` — on macOS waits for the `restore` event, 10 s timeout |
| `window.fullscreen` | `windowId`, `fullscreen`, `x`, `y` | `Promise<null>` — with `x`/`y` it re-targets a display, restores bounds on exit |
| `window.alwaysOnTop` | `windowId`, `alwaysOnTop` | `null` |
| `window.cursor` | `windowId`, `cursor` | `Promise<null>` (injects `* { cursor: … !important }` CSS into every webview of the window) |
| `window.setCursorIcon` | `windowId`, `cursor` | `Promise<null>` (identical to `window.cursor`) |
| `window.setCursorVisible` | `windowId`, `visible` | `Promise<null>` (`cursor: none`) |
| `window.setParentWindow` | `windowId`, `parentId` (`null` ⇒ detach) | `null` |
| `window.setMenu` | `windowId`, `menu` (`null` ⇒ remove, array ⇒ build, `{items}` tolerated) | `null` |
| `window.skipTaskbar` | `windowId`, `skip` | `null` |
| `window.focusable` | `windowId`, `focusable` | `null` |
| `window.contentProtected` | `windowId`, `protected` | `null` |
| `window.center` | `windowId` | `null` |
| `window.requestAttention` | `windowId`, `flash` | `null` (`flashFrame`) |
| `window.show` / `window.hide` / `window.focus` / `window.close` / `window.destroy` | `windowId` | `null` (no-op when the window is gone) |
| `window.nativeHandle` | `windowId` | `{ handle: number, processId: number }` |
| `window.visibleOnAllWorkspaces` | `windowId`, `value` | `null` |
| `window.icon` | `windowId`, `icon` `{width,height,rgba}` | `null` |
| `window.enabled` | `windowId`, `enabled` | `null` |
| `window.enabledQuery` | `windowId` | `boolean` |
| `window.simpleFullscreen` | `windowId`, `fullscreen` (fallback `enable`) | `null` |
| `window.progressBar` | `windowId`, `state`, `progress` | `null` |
| `window.shadow` | `windowId`, `value` | `null` |
| `window.decorations` | `windowId`, `value` (fallback `decorations`, absent ⇒ true) | `null` |
| `window.badgeCount` | `count` | `null` (`app.setBadgeCount`) |
| `window.overlayIcon` | `windowId`, `icon`, `description` | `null` |
| `window.backgroundColor` | `windowId`, `color` (CSS string or `[r,g,b(,a)]`) | `null` |
| `window.backgroundMaterial` | `windowId`, `material` (`none\|acrylic\|mica\|tabbed`; Windows 11 22H2+ only, else throws) | `null` |
| `window.theme` | `windowId`, `theme` (`dark\|light\|system`) | `Promise<null>` (CDP `Emulation.setEmulatedMedia`) |
| `window.ignoreCursorEvents` | `windowId`, `ignore` | `null` (`setIgnoreMouseEvents(ignore, {forward:true})`) |
| `window.themeQuery` | `windowId` | `Promise<{ theme: 'dark' \| 'light' }>` |
| `window.displays` | — | `{ displays: DisplayInfo[], primary: DisplayInfo }` |
| `window.cursorPosition` | — | `{ x: number, y: number }` (physical) |
| `window.currentDisplay` | `windowId` | `DisplayInfo` |
| `window.displayForPoint` | `x`, `y` | `DisplayInfo \| null` |
| `window.titleBarStyle` | `windowId`, `style` | `null` |
| `window.trafficLight` | `windowId`, `x`, `y`, `physical` | `null` (macOS only) |
| `window.startDrag` | `windowId` | `null` (macOS `performWindowDragWithEvent`) |
| `window.popupMenu` | `windowId`, `menu`, `x`, `y` | `Promise<number \| null>` — chosen `commandId`; resolves only after the native menu closes |

`DisplayInfo` = `{ name: string, position: Rectangle, size: {width,height}, workArea: { position: Rectangle, size: {width,height} }, scaleFactor: number }` (`src-ts/runtime/display.ts:3-9`).

### 3.3 Webview, browser, protocol, app methods

| Method | Params read | Returns |
| --- | --- | --- |
| `webview.create` | `webviewId` (safe int, unique), `windowId`, `url`, `autoResize`, `bounds`, `policy`, `incognito`, `dataDirectory`, `userAgent`, `initScripts`, `preloadScripts`, `backgroundColor`, `transparent`, `acceptFirstMouse`, `javascriptDisabled`, `devtools`, `backgroundThrottling`, `guestHost`, `authenticatedWebHostSession`, `browserHost` | `{ webviewId }` |
| `browser.acquire` | `webviewId`, `workspace` (non-empty string ≤ 4096) | `{ lease: string, partition: string }` |
| `browser.release` | `webviewId`, `lease` | `Promise<null>` |
| `webview.url` | `webviewId` | `string` |
| `webview.eval` | `webviewId`, `script`, `documentGeneration` (defaults to the current generation) | `Promise<unknown>` — the script's JSON-serialisable result |
| `webview.capture` | `webviewId` | `Promise<string>` — `data:` URL |
| `webview.newWindowResponse` | `id`, `response` `{ action, windowId? }` | `null` — resolves the blocked `setWindowOpenHandler` |
| `webview.navigationResponse` | `id`, `allow` (strict `=== true`) | `null` |
| `webview.downloadResponse` | `id`, `allow`, `destination` | `null` |
| `webview.permissionResponse` | `id`, `response` (`allow\|deny\|default`) | `null` |
| `webview.trustedOrigin` | `webviewId`, `origin` | `Promise<null>` |
| `webview.trustedOriginAndNavigate` | `webviewId`, `origin`, `url` | `Promise<null>` |
| `webview.navigate` | `webviewId`, `url` | `null` |
| `webview.reload` | `webviewId` | `null` |
| `webview.setAutoResize` | `webviewId`, `autoResize` | `null` |
| `webview.goBack` / `webview.goForward` | `webviewId` | `null` |
| `webview.canGoBack` / `webview.canGoForward` | `webviewId` | `boolean` |
| `webview.openDevtools` | `webviewId` | `Promise<null>` (10 s timeout, rejects when DevTools are disabled) |
| `webview.closeDevtools` | `webviewId` | `null` |
| `webview.isDevtoolsOpen` | `webviewId` | `boolean` |
| `webview.setZoom` | `webviewId`, `scaleFactor` (> 0) | `null` |
| `webview.reparent` | `webviewId`, `windowId` | `null` |
| `webview.bounds` | `webviewId` | `{ scaleFactor, bounds, contentBounds, visible }` |
| `webview.setBounds` | `webviewId`, `position` `{x,y,physical?}`, `size` `{width,height,physical?}`, `physical` | `null` |
| `webview.visibility` | `webviewId`, `visible` | `null` |
| `webview.backgroundColor` | `webviewId`, `color` | `null` |
| `webview.focus` | `webviewId` | `null` |
| `webview.print` | `webviewId` | `Promise<void>` → `null` |
| `webview.close` | `webviewId` | `null` |
| `webview.cookies` | `webviewId`, `url` | `Promise<Cookie[]>` |
| `webview.setCookie` | `webviewId` + `url`, `name`, `value`, `domain`, `path`, `secure`, `httpOnly`, `sameSite`, `expirationDate` | `Promise<null>` |
| `webview.deleteCookie` | `webviewId`, `url`, `name` | `Promise<null>` |
| `webview.clearBrowsingData` | `webviewId` | `Promise<null>` (`session.clearStorageData()`) |
| `webHost.authenticate` | `url` (`http:`/`https:`, no credentials) | `Promise<{ id, documentUrl, backendOrigin }>` — requires a `303` with `set-cookie` and a same-origin `location` |
| `webHost.cookieHeader` | `hostId` | `string` |
| `webHost.bind` | `webviewId`, `hostId` | `null` |
| `protocol.register` | `scheme` (`^[a-z][a-z0-9+.-]*$`) | `null` (idempotent) |
| `protocol.respond` | `id`, `status`, `headers`, `body` | `null` |
| `app.setMenu` | `menu` (array or `null`) | `null` |
| `app.badgeLabel` | `label` | `null` (macOS dock only) |
| `app.focus` | `steal` | `null` |
| `app.show` / `app.hide` | — | `null` (macOS only; no-op elsewhere) |
| `app.activationPolicy` | `policy` | `null` (macOS only) |
| `app.dockVisibility` | `visible` | `null` (macOS only) |
| `app.theme` | `theme` (`dark\|light\|system`) | `null` (writes `nativeTheme.themeSource`; on Linux this is the only writer) |
| `shell.openExternal` | `url` | `Promise<null>` |
| `shell.openPath` | `path` | `Promise<null>` — see **D1**: Rust's declared response is `String` |
| `command.execute` | `taskId`, `script`, `timeoutMs`, `maxOutputBytes`, `stream` | `Promise<{ code, stdout(base64), stderr(base64) }>` |
| `command.cancel` | `taskId` | `null` (stops the task with exit code 130) |

### 3.4 Command-side facts an implementer must not miss

* **Identity is never taken from the renderer.** `tauri-ipc`/`tauri-ipc-fetch` resolve the webview from `event.sender.id` and reject unless the frame is the main frame, the origin is non-opaque and `webview.acceptsIpc(frame.url)` (`src-ts/main.ts:152-172`).
* **`window.create` is the only method that can run nested inside a blocked callback** and create a native popup: it requires `newWindowRequest` to match the currently-blocked request and consumes the registered `PopupPlan` (`src-ts/runtime/factory.ts:98-128`).
* **`webview.eval` is generation-guarded**: a stale `documentGeneration` throws rather than evaluating into the wrong document; without the parameter the *current* generation is used, which makes the check a no-op (`src-ts/runtime/webview.ts:240-267`).
* **`webview.close` on a guest plan** that was never attached still emits `webview.closed` (`src-ts/runtime/factory.ts:177-194`).
* **`window.destroy` is the only way to close a window without a `window.closeRequested` round-trip**; `window.close` re-enters the close-request path (§4.6).
* **`app.setMenu` increments a generation counter** so a stale menu closure cannot send `app.menuCommand` (`src-ts/main.ts:94-102`).
* **`command.execute`** rejects duplicate `taskId`s, validates `timeoutMs ∈ 1..=2147483647`, `maxOutputBytes ∈ 1..=268435456`, and traps `process.stdout.write`, `process.stderr.write`, `process.exit`, `process.exitCode`, timers and unhandled rejections for the duration of the script (`src-ts/runtime/execution.ts:47-129, 294-306`).

---

## 4. Startup contract

### 4.1 The only launch API

```rust
Electron::new(binary, port)                                  // = with_args(binary, port, &[], 15s)
Electron::with_args(binary, port, args: &[String], timeout)  // args are appended verbatim
```

`with_args` moves the whole blocking sequence onto `tokio::task::spawn_blocking`; the returned `Arc<Electron>` is fully connected and handshaken. The `timeout` must be non-zero and is a single deadline computed *before* the spawn, so it bounds process start, socket accept and handshake together; it also bounds each individual accepted peer's handshake read.

### 4.2 Process construction (`src/ipc.rs:141-299`)

1. `std::fs::canonicalize(binary)` — the canonical path is what later compares against the staged marker.
2. `main_script(&binary)` (`src/ipc.rs:793-824`): `<binary-dir>/main.cjs`; on macOS also, when the binary lives in `<X>.app/Contents/MacOS/`, `<X>.app/../main.cjs` (i.e. beside the bundle). Not found ⇒ `"staged Electron main.cjs not found beside <binary> or its .app distribution"`.
3. `staged_identity(&binary, &script)` (`src/ipc.rs:826-864`): read `<script-dir>/electron-rs.json`; a missing file means "no identity"; otherwise the marker's `binary` (resolved and containment-checked by `metadata::binary`) must canonicalise to the launched binary (`"<marker> belongs to a different Electron binary"`), `appName`/`identifier` must be string-or-null, and the pair is re-validated with `metadata::validate`.
4. `TcpListener::bind(("127.0.0.1", port))`; `port == 0` ⇒ the OS picks, and the **bound** port is what is passed to Electron. The listener is set non-blocking.
5. `handshake_token()`: 32 CSPRNG bytes (`/dev/urandom` on Unix, `BCryptGenRandom` on Windows) rendered as 64 lowercase hex characters.
6. Spawn:

   ```
   argv   = [ <canonical binary>, <main.cjs>, "--port", "<port>", ...args ]
   stdin  = piped      (credential only)
   stderr = piped      (drained into a 32-line tail, echoed to the parent's stderr)
   stdout = inherited  (NOT piped)
   env    : ELECTRON_RUN_AS_NODE       removed
            ELECTRON_RS_APP_NAME       removed, then set from the staged marker when present
            ELECTRON_RS_IDENTIFIER     removed, then set from the staged marker when present
            TAURI_RUNTIME_ELECTRON_OWNER  set (see below)
   ```

   Windows: `creation_flags(0x0800_0000)` = `CREATE_NO_WINDOW`; immediately after spawn a Job Object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` is created and the child assigned to it, so a host crash kills Electron and all its descendants (`src/ipc.rs:766-791`).

7. `TAURI_RUNTIME_ELECTRON_OWNER` resolution order (`src/ipc.rs:180-194`): the inherited variable → the staged marker's `identifier` → the staged marker's `app_name` → `current_exe().file_stem()`. If all are absent: `"cannot determine the Electron profile owner"`.

8. The credential is written to the child's stdin as one line and stdin is then dropped/closed. The comment is explicit: the credential must never appear in argv, environment, files or diagnostics.

### 4.3 What the JavaScript side requires

| Input | Source | Requirement / effect |
| --- | --- | --- |
| `--port <n>` or `--port=<n>` | argv | **Mandatory.** Missing/falsy ⇒ `console.error('missing --port')` and `process.exit(1)` (`src-ts/main.ts:43-47`). |
| credential | **stdin, fd 0**, one line, trimmed | Must match `/^[a-f0-9]{64}$/`, else `throw new Error('missing Electron IPC credential')` (`src-ts/main.ts:50-51`). Read blocking with `fs.readFileSync(0)`. |
| `--tauri-identifier <id>` | argv | Falls back to `ELECTRON_RS_IDENTIFIER`. Used for `app.setAppUserModelId(identifier)` after ready (`src-ts/main.ts:54, 79`). |
| `ELECTRON_RS_IDENTIFIER` | env | Fallback for the above; set by Rust from the staged marker. |
| `--tauri-schemes a,b,c` | argv | Comma-separated, trimmed, empties dropped. Registered as privileged (`standard, secure, supportFetchAPI, corsEnabled, stream`) **before** `app.whenReady()` (`src-ts/main.ts:57-63`, `src-ts/runtime/protocol.ts:55-58`). Missing ⇒ no custom schemes. |
| `ELECTRON_RS_APP_NAME` | env | `app.setName(appName)` before ready (`src-ts/main.ts:32-33`). |
| `TAURI_RUNTIME_ELECTRON_OWNER` | env | `app.setPath('userData', app.getPath('appData')/<owner>)`, directory created recursively (`src-ts/main.ts:34-39`). This is the only profile isolation mechanism. |

The `arg()` helper (`src-ts/utils/index.ts:10-15`) accepts both `--name value` and `--name=value`.

Linux additionally calls ``app.setDesktopName(`${app.getName()}.desktop`)`` before ready, because GNOME matches the `.desktop` file by `app_id` and the value only takes effect pre-ready (`src-ts/main.ts:66`).

### 4.4 Handshake

Electron → Rust, as the **first line on the socket**:

```json
{"event":"ready","params":{"electron":"44.7.0","token":"<64 hex>"}}
```

Rust side (`read_handshake`, `src/ipc.rs:882-923`):

* Reads up to the first `\n`, hard-capped at 64 KiB (`"Electron handshake exceeds 64 KiB"`).
* Each accepted peer gets `min(overall deadline, now + 500 ms)`; a peer that fails validation is dropped and the accept loop continues, so a stray local connection cannot consume the real client's slot.
* `params.token` must equal the credential (length check plus a constant-time XOR fold — no prefix match, no early exit) **and** `event` must be `"ready"`, else `"unauthenticated Electron handshake"`.
* On success the socket read timeout is cleared and `params.electron` (or `"unknown"`) becomes `Electron::version`.
* The loop also polls `child.try_wait()`; a child that exits first produces `"Electron exited before authenticating (<status>): <binary>"`.
* Any startup failure kills and reaps the child, then appends the captured stderr tail: `"\nELECTRON_STDERR_TAIL: last N Electron stderr line(s):"` followed by `"  <line>"` entries. The tail keeps the last 32 lines, each truncated to 400 characters (with `…`).

Electron connects only after it is ready to authenticate: the Worker thread spins on a `SharedArrayBuffer` gate (`Atomics.wait` on slot 2) that the main thread flips once `app.whenReady()` → `Menu.setApplicationMenu(null)` → `policies.install(session.defaultSession)` has resolved (`src-ts/main.ts:78-87`, `src-ts/socket/ipc.ts:76-95, 207-213`). If that readiness promise rejects, the gate is never opened and the client closes instead.

### 4.5 Post-handshake wiring

* Rust starts the `electron-rpc` reader thread; from then on every line is parsed and dispatched (§0).
* Electron keeps `transport.onClose`, `app.on('before-quit')`, `app.on('will-quit')`, `app.on('window-all-closed')`, `app.on('activate')` and the two `ipcMain` handlers (`tauri-ipc`, `tauri-ipc-fetch`) installed for the process lifetime.
* Electron deliberately does **not** quit when the last window closes (`app.on('window-all-closed', noop)`): window lifetime belongs to Rust.

### 4.6 Shutdown semantics

**Rust → Electron**

`Electron::shutdown()` (`src/ipc.rs:480-494`), also reached from `Drop` and from the reader thread on EOF:

1. `disconnect()`: set `closed`; fail every command route with `ChannelDisconnected`; fail every pending RPC with `ChannelDisconnected`; best-effort `{"event":"ipc.closed"}` to each subscriber.
2. `shutdown_stream.shutdown(Shutdown::Both)` — this is what Electron observes as socket close.
3. Drop the Windows Job Object handle (kills the whole tree).
4. `child.kill()` + `child.wait()`.

**Electron → Rust**

* Socket close (Worker exit, Rust closing, or a fatal protocol error) ⇒ `transport.onClose` ⇒ `protocols.dispose()`, `policies.dispose()`, destroy every window, `app.quit()` (`src-ts/main.ts:180-185`).
* `before-quit` while the transport is still open ⇒ `event.preventDefault()` and `transport.send('app.exitRequested', {})`. Electron does **not** exit on its own initiative; only a closed transport lets the quit proceed (`src-ts/main.ts:195-199`).
* `will-quit` ⇒ `systemTheme.dispose()` and `executions.close()` (which stops every running command with exit code 130).

**Consequences the runtime must design around**

* There is **no exit-code channel** and **no exit veto**: Rust cannot ask Electron to exit with a status, and cannot answer `app.exitRequested`. The only way Rust ends the process is `shutdown()` (kill). §6 lists this as a gap.
* `window.closeRequested` is likewise advisory: Electron always `preventDefault()`s the native `close` event while the transport is open (`src-ts/runtime/factory.ts:284`), so a window only really closes via `window.destroy` or process exit.
* Killing the process (host crash, `shutdown()`) is covered on Windows by the Job Object; on Unix the Electron child is killed directly but its own grandchildren are not.

### 4.7 Failure modes and their messages

| Situation | Error |
| --- | --- |
| `timeout == 0` | `InvalidInput("Electron startup timeout must be positive")` |
| binary missing | `Context("Electron binary <path>", io::Error)` |
| no `main.cjs` | `InvalidInput("staged Electron main.cjs not found beside …")` |
| marker names another binary | `InvalidInput("<marker> belongs to a different Electron binary")` |
| bind failure | `Context("bind IPC", io::Error)` |
| no owner | `InvalidInput("cannot determine the Electron profile owner")` |
| spawn failure | `Context("spawn <binary>", io::Error)` |
| handshake timeout | `InvalidInput("Electron did not authenticate within <timeout>")` |
| child died first | `InvalidInput("Electron exited before authenticating (<status>): <binary>")` |
| bad token / not `ready` | `InvalidInput("unauthenticated Electron handshake")` |
| oversized handshake | `InvalidInput("Electron handshake exceeds 64 KiB")` |

---

## 5. Distribution staging

### 5.1 What `ElectronBuilder::build()` produces

`build()` (`src/lib.rs:111-188`) is a **build-script** operation. It never launches Electron and never patches the shared download in place; every mutation happens in a staging directory that is renamed into place.

Pipeline:

1. `validate_version`: ≤ 100 chars, first char ASCII digit, remaining chars `[A-Za-z0-9.-]`.
2. `DistributionTarget::current()` from `TARGET` (falling back to the host triple); supported pairs are `win32×{x64,arm64,ia32}`, `linux×{x64,arm64,armv7l}`, `darwin×{x64,arm64}`.
3. `pe = pe_enabled()`; `metadata::validate_for(metadata, target, pe)` (name/identifier syntax, icon extension and content, host-executable support per platform).
4. Output directory: explicit `.output(..)` (absolutised against cwd) else `<cargo target dir>/electron/<platform>-<arch>`. The parent is created and legacy `.<name>-*` temporaries from older versions are removed.
5. **Fast path**: if the existing output has consistent version evidence, matching branding, and byte-identical assets, `build()` returns it without writing anything.
6. Source selection, in order: the reusable output itself → each `.path()` (with `{resources}` expanded) → `node_modules/electron/dist` found by walking ancestors of the cwd → the existing output when it is *stock* → the shared download cache → `acquisition::download()`.
7. Guard: source and output must not contain one another (`"Electron source and output must not contain each other"`).
8. Stage: `acquisition::stage_directory(output, "stage")` creates `.<name>-stage-<pid>-<serial>` in the **grandparent** of the output (same filesystem for an atomic rename, outside the resource root the bundler scans).
9. Into the stage: `copy_dir(source, stage)` → `write_assets(stage)` → `compile_native_assets(stage, target)` → `metadata::apply(stage, …)` → write `.electron-rs-version` → compute the staged binary and its path relative to the stage.
10. Commit: `acquisition::replace_directory(stage, output)` renames stage → output, backing up any previous output to `.<name>-previous-<pid>-<serial>`. A failed publish restores the backup; a failed *cleanup* after a successful publish is reported as `PreparedDistribution::cleanup_warning`, not an error.
11. On any error the stage tree is removed and the previous output is untouched.

`PreparedDistribution { directory, binary, cleanup_warning }` is the only supported way to obtain a launchable path from the same build invocation; `binary()` re-resolves without writing but refuses a non-stock distribution when `pe(false)`.

### 5.2 Files in the prepared distribution

* The complete Electron release tree, copied from the source distribution (stock executable, `chrome-sandbox`, `resources/`, `locales/`, frameworks, …).
* **Bridge assets** — every `*.cjs` under `assets/`, at its relative path, plus `appkit.mm` (`build.rs:13-19`; `ASSETS` is generated at Cargo build time from the complete tsdown output tree and embedded with `include_bytes!`). With the current sources that is:

  ```
  main.cjs
  preload.cjs
  runtime/authenticated-host.cjs   runtime/browser.cjs      runtime/commands.cjs
  runtime/display.cjs              runtime/downloads.cjs    runtime/environment.cjs
  runtime/execution.cjs            runtime/factory.cjs      runtime/origin.cjs
  runtime/policy.cjs               runtime/protocol.cjs     runtime/webview.cjs
  runtime/window.cjs
  runtime/native/appkit.cjs        runtime/native/cursor.cjs
  runtime/native/system-theme.cjs  runtime/native/theme.cjs
  socket/ipc.cjs                   socket/table.cjs         socket/transport.cjs
  utils/index.cjs
  _virtual/_rolldown/runtime.cjs
  appkit.mm                        (source, all platforms)
  ```

  `socket/ipc.cjs` must stay a separate file: the Worker bootstraps itself via `new Worker(__filename)` (`src-ts/socket/ipc.ts:82-85`), so inlining it into `main.cjs` would break the channel.
* **`appkit.node`** — macOS only. `compile_native_assets` runs `clang++ -std=c++17 -fobjc-arc -fblocks -bundle -undefined dynamic_lookup -framework AppKit -arch <arm64|x86_64> -mmacosx-version-min=12.0 appkit.mm -o appkit.node` and then `codesign --force --sign -`. Building a darwin target on a non-macOS host fails with `"building the AppKit bridge requires a macOS host with Xcode Command Line Tools"`.
* **`.electron-rs-version`** — the version string, used as version evidence by `acquisition::version_matches`.
* **`electron-rs.json`** — the marker (§5.3).
* With `pe(true)` and `app_name` set: a renamed executable (`<name>.exe`, `<name>`, `<name>.app/Contents/MacOS/<name>`), macOS `Contents/Resources/app/{package.json,index.mjs}` launcher, patched `Info.plist` (`CFBundleExecutable`, `CFBundleName`, `CFBundleDisplayName`, `CFBundleIdentifier`, `CFBundleIconFile`), renamed `Electron Helper*.app` bundles, and a `codesign --force --deep --sign -` pass.

`assets_match` (`src/lib.rs:344-351`) additionally requires `appkit.node` to exist as a regular file on darwin, and every asset to be byte-identical, so a stale or tampered asset triggers a repair build.

### 5.3 The `electron-rs.json` marker

Written atomically (`electron-rs.json.tmp` → rename) by `metadata::apply` (`src/metadata.rs:94-131`):

```jsonc
{
  "version": 2,                          // marker schema version; 1 is read-only legacy
  "target": "win32",                     // "win32" | "darwin" | "linux"
  "arch": "x64",                         // "x64" | "arm64" | "ia32" | "armv7l"
  "electronVersion": "44.7.0",
  "appName": "My App",                   // string or null
  "identifier": "com.example.app",       // string or null
  "binary": "relative/normal/path/to/executable",
  "fingerprint": "0123456789abcdef",
  "pe": true                             // native branding applied?
}
```

Rules enforced when reading it:

* `version` must be `1` or `2`; only `2` can prove a distribution is *stock*.
* `binary` must be a relative, normal, non-empty path with no `\`, `:`, control characters, `.`/`..` components, and must canonicalise to a path inside the distribution (`"metadata marker binary escapes distribution"`).
* `target` must equal the current platform; for version 2 the `arch` must match too.
* `fingerprint` is a `DefaultHasher` digest over platform, arch, `appName`, `identifier`, (on darwin with `pe`, the launcher bytes) and the icon/host-executable contents when `pe` is on.
* `staged_identity` at launch reads **only** `appName` and `identifier` from this file; everything else is build-time evidence.

### 5.4 What `electron_binary()` accepts

`electron_binary(path)` (`src/lib.rs:238-240`, `305-317`) is read-only: it never downloads, launches or repairs. It accepts, in order:

1. **An executable file** — validated by inspecting its own headers (`target.verify_binary`, `src/acquisition.rs:159-248`) and returned as-is. The check is per-format and strict: PE magic + `PE\0\0` + machine (`0x8664`/`0xaa64`/`0x014c`) + optional-header magic consistency; ELF magic + class + endianness + `e_type` executable + `e_machine` (`62`/`183`/`40`); Mach-O thin magic or a universal table whose slices are bounds-checked and whose CPU agrees with each slice. A header that belongs to another OS fails with e.g. `"PE executable requires Windows"`.
2. **A distribution root** — a directory containing `electron-rs.json` or the stock executable name for the target; resolved through `metadata::binary_for` (marker-aware, header-verified).
3. **A resource root** — a directory containing `<platform>-<arch>/`; resolved as `metadata::binary_for(path/<platform>-<arch>)`. Failure message: `"Electron binary not found in <path>; bundle target/electron as runtime (runtime downloads are disabled)"`.

Directory names and markers are never treated as evidence on their own: the executable header check always runs.

### 5.5 The `APP_ELECTRON_BINARY` compile-time contract

```rust
pub fn electron_binary(path: impl AsRef<Path>) -> Result<PathBuf, Error>;

#[macro_export]
macro_rules! electron_binary {
  ($path:expr $(,)?) => {{
    let path: ::core::option::Option<_> = $path;
    match path {
      Some(path) => $crate::electron_binary(path),
      None => match ::core::option_env!("APP_ELECTRON_BINARY") {
        Some(path) => $crate::electron_binary(::std::path::Path::new(path)),
        None => Err($crate::Error::from(
          "APP_ELECTRON_BINARY was not set at compile time; set it in the consuming build.rs or pass Some(path)",
        )),
      },
    }
  }};
}
```

Contract details an implementer must honour:

* `option_env!` is expanded **in the consuming crate**, so `APP_ELECTRON_BINARY` must be set by *that* crate's `build.rs` via `println!("cargo:rustc-env=APP_ELECTRON_BINARY=<absolute path>")`. Setting it in `electron-rs`'s own build script or in the runtime environment has no effect.
* `Some(path)` bypasses the variable entirely and needs no compile-time configuration.
* The argument expression is evaluated exactly once.
* The resulting path flows through `electron_binary()` and therefore through the same header validation; a stale path fails at *runtime*, not at compile time.
* Reference usage: `example/build.rs:5-14` points it at `target/electron/<platform>-<arch>/<executable>`; `example/streaming.rs:14` and `example/rpc.rs:13` consume it as `electron_binary!(None::<&Path>)?`.
* A runtime that wants to accept a user-supplied path should call `electron_binary(path)` directly and only fall back to `electron_binary!(None::<&Path>)` for the bundled default.

---

## 6. Gap list

### 6.1 How this list was derived

The CEF runtime is the reference for what a Tauri v3 runtime must expose. Its exported surface is `source/tauri/crates/tauri-runtime-cef/src/lib.rs`:

```rust
pub use dialog::{NativeDialogKind, NativeDialogObservation, NativeDialogSnapshot, NativeDialogToken};
pub use environment::DebugEnvironment;
pub use offscreen::{CursorHandler, Offscreen, OffscreenView};
pub use devtools::{DevToolsMessageIdExhausted, allocate_devtools_message_id};
pub use frame::{FrameEvent, FrameEventHandler, FrameEventKind};
pub use frame_navigation::{FrameNavigationState, NativeDocumentToken};
pub use cef::sys::CEF_API_VERSION_LAST;
#[cfg(target_os = "macos")] pub use platform::macos::setup_application as prepare_macos_application;
pub use runtime::*;          // Cef, CefRuntime, CefRuntimeHandle, EventProxy, NewWindowOpener,
                             // SecretStorage, SandboxPolicy, RemoteDebugging, DevToolsPolicy,
                             // CertificateErrorPolicy, ProxyConfig, AutoplayPolicy,
                             // WebRtcIpHandling, DowngradePolicy
pub use tauri_ext::*;        // AsCefWebviewDispatcher, AsCefWindowOpener, AsCefWebviewAttributes,
                             // WebviewCefExt, WebviewWindowBuilderCefExt, WebviewBuilderCefExt
pub use tauri_macros::cef_entry_point;
pub use webview::*;          // Webview, WebviewSnapshot, CefWebviewAttributes, CefWebviewDispatcher,
                             // RuntimeStyle, DevToolsProtocol, ChromeCommandGroup, ConsoleMessage,
                             // ConsoleMessageLevel
pub use window::{CefWindowDispatcher, NativeWindowToken};
pub use window_builder::WindowBuilderWrapper;
```

Those types exist to satisfy the runtime-agnostic traits in `source/tauri/crates/tauri-runtime/src/lib.rs`:

* `Runtime<T>` (line 526) and `RuntimeHandle<T>` (line 314).
* `WindowDispatch<T>` (line 816) and `WebviewDispatch<T>` (line 666).
* `EventLoopProxy<T>` (line 443), `RuntimeInitAttrs<T>` (line 515).
* `RunEvent<T>` (line 236), `WindowEvent` / `WebviewEvent` / `DragDropEvent` (`window.rs:30-121`).
* `WindowBuilder` (`window.rs:263-529`) and `WebviewBuilder` (`webview.rs:539-941`).

`Change` legend: **JS** = `src-ts` bridge only; **Rust** = the `electron-rs` type layer only; **Both** = a new RPC/event pair plus a new Rust request type; **Native** = additionally needs a native addon (`appkit.node`-style), because Electron's public API has no equivalent.

### 6.2 Runtime / RuntimeHandle level

| Capability | Tauri v3 need | Electron API | Change |
| --- | --- | --- | --- |
| Exit with a status code | `RuntimeHandle::request_exit(code)` | `app.exit(code)` | **Both** — new `app.exit { code }` request; JS must call `app.exit` (today `app.exitRequested` is one-way and `app.quit()` only happens when the socket dies). |
| Veto / allow an exit request | `RunEvent::ExitRequested { code, api }` with `api.prevent_exit()` | `app.on('before-quit', e => e.preventDefault())` | **Both** — JS already prevents unconditionally; it needs an `app.exitResponse { allow }` command so Rust can decide. |
| User event channel | `EventLoopProxy::send_event(T)` | — (host-side only) | **Rust** — the runtime owns its own channel; `Electron::subscribe()` is the delivery half. |
| Run-loop shape | `run`, `run_return`, `run_iteration`, `RunEvent` stream | — (Electron owns its loop) | **Rust** — adapter that turns `events()` into `RunEvent`s: `Ready` from the handshake, `MainEventsCleared` from a drained event batch, `ExitRequested`/`Exit` from `app.exitRequested`/`ipc.closed`. `Resumed`/`Suspended` are mobile-only in `tauri-runtime` and have no desktop mapping. |
| `display_handle` | `RuntimeHandle::display_handle()` | none | **Native** — needs a platform display handle from an addon. |
| `set_device_event_filter` | `RuntimeHandle::set_device_event_filter` | none | **Rust** — accept and ignore. |
| Data-store enumeration / removal | `fetch_data_store_identifiers`, `remove_data_store` | `session.fromPartition`, `session.clearStorageData()`, `session.flushStorageData()`; no partition enumeration | **Both** — JS must track every partition it creates (`incognito`, `dataDirectory`, authenticated hosts, browser guests) and expose list/remove commands. |
| `custom_scheme_url(scheme, https)` | `RuntimeHandle::custom_scheme_url` | pure string mapping: `https://<scheme>.localhost` on Windows, `<scheme>://localhost` elsewhere | **Rust** — JS already performs the inverse mapping (`src-ts/runtime/protocol.ts:125-139`). |
| Webview (engine) version | `RuntimeHandle::webview_version()` | `process.versions.chrome` | **Both** — `Electron::version` currently carries `process.versions.electron` only; add `chrome` to the `ready` payload. |
| Apply `tauri.conf.json` runtime attributes | `RuntimeInitAttrs::apply_config` | `app.commandLine.appendSwitch`, `app.setPath` | **Rust** — `ElectronBuilder` is build-time; there is no runtime config application path at all. |
| Accessibility support toggle | `set_browsers_accessibility_state` (CEF runtime) | `app.setAccessibilitySupportEnabled(bool)` | **Both** |
| Single instance / second-instance hand-off | Tauri's single-instance behaviour | `app.requestSingleInstanceLock()`, `app.on('second-instance')` | **Both** |

### 6.3 Window: `WindowDispatch` + `WindowBuilder`

| Capability | Tauri v3 need | Electron API | Change |
| --- | --- | --- | --- |
| Close veto | `WindowEvent::CloseRequested { signal_tx }` | `win.on('close', e => e.preventDefault())` | **Both** — `window.closeRequested` exists but Rust cannot answer it; add `window.closeResponse { windowId, allow }` and have `requestClose()` await it. Today only `window.destroy` really closes. |
| Always-on-bottom | `set_always_on_bottom(bool)` | none (`setAlwaysOnTop(false)` only clears the level) | **Both** — approximate, or **Native**. |
| Cursor grab | `set_cursor_grab(bool)` | none | **Native** |
| Cursor position | `set_cursor_position(Position)` | none (`screen.getCursorScreenPoint` is read-only) | **Native** |
| Resize dragging | `start_resize_dragging(ResizeDirection)` | none (only CSS `-webkit-app-region: drag` for moving) | **Native** |
| Raw GTK window / default vbox | `gtk_window()`, `default_vbox()` | `win.getNativeWindowHandle()` for the window only; `win.contentView` has no handle | **Both** — `window.nativeHandle` covers the window; the content view needs an addon. |
| Raw window handle | `window_handle() -> RawWindowHandle` | `win.getNativeWindowHandle()` | **Rust** — map the existing `{ handle, processId }` into `RawWindowHandle`. |
| Owner window (vs parent) | `WindowBuilder::owner(HWND)` | `win.setParentWindow` only (parent, not owner) | **Rust** — accept and ignore, or **Native**. |
| Drag-and-drop toggle | `WindowBuilder::drag_and_drop(bool)` | none | **Both** |
| Platform window class / redirection bitmap / activity / scene | `window_classname`, `no_redirection_bitmap`, `activity_name`, `created_by_activity_name`, `requested_by_scene_identifier` | none | **Rust** — accept and ignore; the getters (`activity_name()`, `scene_identifier()`) return a constant. |
| Window effects | `WindowBuilder::effects(WindowEffectsConfig)` | `backgroundMaterial` (Win11 22H2+), `vibrancy` + `visualEffectState` (macOS) | **Both** — JS only maps `effects[]` → material and drops macOS vibrancy; Rust does not type the field at all (**D3**). |
| Badge with desktop filename | `set_badge_count(count, desktop_filename)` | `app.setBadgeCount(count)` (no filename) | **Rust** — accept and ignore the filename. |
| `titleBarOverlay`, `tabbingIdentifier`, `hiddenTitle` | `WindowBuilder` fields | `titleBarOverlay`, `tabbingIdentifier`, `hiddenInset` | **Rust** — already honoured by JS but untyped (**D3**). |
| Scale-factor change event | `WindowEvent::ScaleFactorChanged` | `screen.on('display-metrics-changed')` + `webContents.devicePixelRatio` | **Both** — no event is emitted today; `window.event` carries the current `scaleFactor` only on resize/move. |
| Drag-and-drop events | `WindowEvent::DragDrop(Enter/Over/Drop/Leave)` | none for incoming drops (only `webContents.startDrag` for outgoing) | **Both** + **Native** for real paths |

### 6.4 Webview: `WebviewDispatch` + `WebviewBuilder`

| Capability | Tauri v3 need | Electron API | Change |
| --- | --- | --- | --- |
| Raw webview handle | `with_webview(FnOnce(RawWebview))` | `webContents.id` (already surfaced as `webview.attached.contentsId`) | **Rust** — map the id to a `RawWebview`; no JS change. |
| Drag-and-drop events | `WebviewEvent::DragDrop` | none | **Both** + **Native** |
| Proxy per webview | `WebviewBuilder::proxy_url` | `session.setProxy({ mode, proxyRules, proxyBypassRules })` | **Both** — `webview::Config` has no proxy field and the JS never calls `setProxy`. |
| Zoom hotkeys | `zoom_hotkeys_enabled(bool)` | `webContents.setVisualZoomLevelLimits`, `setZoomFactor`; Ctrl+wheel is on by default | **Both** |
| Browser extensions | `browser_extensions_enabled(bool)` | `session.loadExtension(path)` | **Both** |
| HTTPS scheme mapping | `use_https_scheme(bool)` | `protocol.registerSchemesAsPrivileged` + `https://<scheme>.localhost` | **Both** |
| Link preview | `allow_link_preview(bool)` | macOS `webContents` link preview | **Both** |
| Navigation domain limiting | `limit_navigations_to_app_bound_domains(bool)` | `session.webRequest.onBeforeRequest` | **Both** — the policy plumbing exists (`webview.navigationRequested`); the flag does not. |
| Scrollbar style | `scroll_bar_style(ScrollBarStyle)` | `--enable-features=OverlayScrollbar` / CSS | **Both** |
| Autofill | `general_autofill_enabled(bool)` | `--disable-features=AutofillServerCommunication`, `session` prefs | **Both** |
| Extra browser switches | `additional_browser_args(&str)` | `app.commandLine.appendSwitch` (before ready) | **Both** |
| Drag-and-drop handler toggle | `disable_drag_drop_handler()` | none | **Both** |
| Clipboard access | `enable_clipboard_access()` | `session.setPermissionRequestHandler` | **JS** — already effectively on: `clipboard-sanitized-write` is auto-allowed (`src-ts/runtime/policy.ts:60`); expose it as a flag for parity. |
| Focused on create | `WebviewBuilder::focused(bool)` | `webContents.focus()` after attach | **JS** |
| Console messages | `CefWebviewAttributes::console_message_handler` / `ConsoleMessage` / `ConsoleMessageLevel` | `webContents.on('console-message')` | **Both** — new `webview.consoleMessage` event. |
| Frame lifecycle | `FrameEvent`, `FrameEventKind`, `FrameEventHandler`, `FrameNavigationState`, `NativeDocumentToken` | `did-frame-navigate`, `did-frame-finish-load`, `frame-created`, `webContents.mainFrame` | **Both** — new `webview.frameEvent` event carrying frame id, kind and URL. |
| DevTools protocol | `WebviewCefExt::send_dev_tools_message`, `on_dev_tools_protocol`, `allocate_devtools_message_id`, `DevToolsMessageIdExhausted` | `webContents.debugger.attach()` / `sendCommand()` / `on('message')` | **Both** — the JS already drives CDP internally for theming (`src-ts/runtime/native/theme.ts:6-20`) but exposes nothing. Note the shared request-id space requirement. |
| Offscreen rendering | `Offscreen`, `OffscreenView`, `CursorHandler` | `webPreferences.offscreen: true`, `webContents.on('paint')`, `setFrameRate`, `beginFrameSubscription` | **Both** |
| Chrome command groups | `ChromeCommandGroup`, `allowed_chrome_commands` | `webContents.on('before-input-event')` + `win.setMenu(null)` | **Both** — JS already suppresses the app menu and intercepts F12; there is no per-group allow-list. |
| Runtime style | `RuntimeStyle` (Alloy/Chrome) | none | **Rust** — accept and ignore. |
| Browser-settings escape hatch | `browser_settings_callback` | `webPreferences` + `app.commandLine` | **Rust** — accept and ignore (no per-browser settings object exists). |

### 6.5 Engine policy and diagnostics (CEF `Cef` builder surface)

All of these are `Cef::*` builder options in `runtime.rs:545-1142`, i.e. process-wide policy that a Tauri v3 runtime is expected to expose. Electron has no RPC for any of them today.

| Capability | Electron API | Change |
| --- | --- | --- |
| Proxy configuration (`ProxyConfig`) | `session.setProxy({mode, proxyRules, proxyBypassRules})` | **Both** |
| Certificate error policy (`CertificateErrorPolicy`) | `app.on('certificate-error')`, `session.setCertificateVerifyProc` | **Both** |
| Sandbox policy (`SandboxPolicy`) | `webPreferences.sandbox` (hardcoded `true` in `factory.ts`), `--no-sandbox` | **Both** |
| Remote debugging (`RemoteDebugging`) | `--remote-debugging-port`, `--remote-debugging-pipe` | **Both** |
| DevTools policy (`DevToolsPolicy`) | `webPreferences.devTools` (per webview only), `--auto-open-devtools-for-tabs` | **JS** for the per-webview half; **Both** for a process-wide policy |
| Locale + accept languages (`locale`, `accept_language_list`) | `--lang`, `--accept-lang`, `app.getLocale()` | **Both** |
| Spell checking (`spell_checking`) | `session.setSpellCheckerEnabled`, `setSpellCheckerLanguages` | **Both** |
| Safe browsing (`safe_browsing`) | `--disable-features=SafeBrowsing*` | **Both** |
| Component updates (`component_updates`) | `--disable-component-update` | **Both** |
| Autoplay policy (`AutoplayPolicy`) | `--autoplay-policy` | **Both** |
| WebRTC IP handling (`WebRtcIpHandling`) | `--force-webrtc-ip-handling-policy` | **Both** |
| JavaScript flags (`javascript_flags`) | `--js-flags` | **Both** |
| Feature toggles (`disable_features`, `enable_features`) | `--disable-features`, `--enable-features` | **Both** |
| Command-line args (`command_line_arg(s)`) | already possible via `Electron::with_args` | **Rust** — surface it as typed runtime options |
| Log file / severity (`log_file`, `log_severity`, `log_items`) | `--enable-logging`, `--log-file`, `--v`, `--vmodule` | **Both** |
| User agent + product (`user_agent`, `user_agent_product`) | `webContents.setUserAgent` (per webview, already wired) / `--user-agent` | **JS** for the global half |
| Session cookies persistence (`persist_session_cookies`) | `session.cookies` + `--restore-last-session` semantics | **Both** |
| Profile / global preferences (`profile_preference(_value)`, `global_preference`) | `session` prefs, `app.commandLine` | **Both** |
| Default content setting (`default_content_setting`) | `session.setPermissionRequestHandler` (already the policy hook) | **JS** |
| Root cache path (`root_cache_path`), profile downgrade (`DowngradePolicy`) | `app.setPath('userData')` (already driven by `TAURI_RUNTIME_ELECTRON_OWNER`), `--disk-cache-dir` | **Both** |
| Secret storage (`SecretStorage`) | `safeStorage.encryptString/decryptString` | **Both** |
| Debug environment policy (`DebugEnvironment`) | `--enable-logging`, `ELECTRON_ENABLE_LOGGING` | **Both** |
| Deep-link schemes (`deep_link_schemes`, `RunEvent::Opened`) | `app.setAsDefaultProtocolClient`, `open-url` (macOS), `second-instance` argv | **Both** |
| Chrome policy id (`chrome_policy_id`) | none | **Rust** — accept and ignore |

### 6.6 OS integration a Tauri v3 app expects

| Capability | Electron API | Change |
| --- | --- | --- |
| Native dialogs (open/save/message/error) | `dialog.showOpenDialog`, `showSaveDialog`, `showMessageBox`, `showErrorBox` | **Both** — CEF models these as `NativeDialogKind` / `NativeDialogToken` / `NativeDialogSnapshot` / `NativeDialogObservation`; Electron has no RPC at all. |
| Clipboard read/write | `clipboard.readText/writeText/readImage/writeImage/readHTML` | **Both** |
| Global shortcuts | `globalShortcut.register/unregister/isRegistered` | **Both** |
| System tray | `Tray`, `Menu`, `nativeImage` | **Both** |
| Notifications | `Notification` | **Both** |
| Power-save blocker | `powerSaveBlocker.start/stop/isStarted` | **Both** |
| Auto-update | `autoUpdater` (Squirrel) | **Both** |
| Screen capture / desktop sources | `desktopCapturer.getSources` | **Both** |
| Isolation pattern (Tauri) | hidden `BrowserWindow` + `webContents.send` message relay | **Both** |
| `webview.eval` in an isolated world | `webContents.executeJavaScriptInIsolatedWorld` | **Both** — current `webview.eval` runs in the main world only. |
| Window icon from a file path | `nativeImage.createFromPath` | **JS** — `window.create.icon` already accepts a path string; `window.icon` only accepts raw RGBA. |

### 6.7 Already at parity — do not rebuild

These `WindowDispatch` / `WebviewDispatch` operations are fully covered today, with the request struct named in §1.8:

* **Window getters**: `scale_factor`, `inner/outer_position`, `inner/outer_size`, `is_fullscreen`, `is_minimized`, `is_maximized`, `is_focused`, `is_decorated`, `is_resizable`, `is_maximizable`, `is_minimizable`, `is_closable`, `is_visible`, `is_enabled`, `is_always_on_top`, `title`, `theme`, `current_monitor`, `primary_monitor`, `monitor_from_point`, `available_monitors` — all in one `window.geometry` / `window.themeQuery` / `window.displays` / `window.currentDisplay` / `window.displayForPoint` round-trip.
* **Window setters**: `set_resizable`, `set_enabled`, `set_maximizable`, `set_minimizable`, `set_closable`, `set_title`, `maximize`, `unmaximize`, `minimize`, `unminimize`, `show`, `hide`, `close`, `destroy`, `set_decorations`, `set_shadow`, `set_always_on_top`, `set_visible_on_all_workspaces`, `set_content_protected`, `set_size`, `set_min_size`, `set_max_size`, `set_size_constraints`, `set_position`, `set_fullscreen`, `set_fullscreen_on_monitor` (via `x`/`y`), `set_simple_fullscreen`, `set_focus`, `set_focusable`, `set_icon`, `set_skip_taskbar`, `set_cursor_visible`, `set_cursor_icon`, `set_ignore_cursor_events`, `start_dragging`, `set_badge_count`, `set_badge_label`, `set_overlay_icon`, `set_progress_bar`, `set_title_bar_style`, `set_traffic_light_position`, `set_theme`, `set_background_color`, `center`, `request_user_attention`.
* **Webview**: `open_devtools`, `close_devtools`, `is_devtools_open`, `url`, `bounds`, `position`/`size` (derivable from `webview.bounds`), `navigate`, `reload`, `go_back`, `can_go_back`, `go_forward`, `can_go_forward`, `print`, `close`, `set_bounds`, `set_size`, `set_position`, `set_focus`, `hide`/`show` (`webview.visibility`), `eval_script` (and `eval_script_with_callback`, because `webview.eval` returns the JSON result), `reparent`, `cookies`, `cookies_for_url`, `set_cookie`, `delete_cookie`, `set_auto_resize`, `set_zoom`, `set_background_color`, `clear_all_browsing_data`.
* **Events**: `RunEvent::Ready` (handshake), `RunEvent::Reopen` (`app.reopen`), `WindowEvent::Resized`/`Moved`/`Focused`/`ThemeChanged`/`Destroyed` (`window.event`, `window.themeChanged`, `window.closed`), `WindowEvent::CloseRequested` (emitted — but not answerable, see §6.3), webview load/navigation/title/crash events, policy callbacks (navigation, download, permission, new-window), custom protocols with a request/response round-trip, IPC (`ipc.message` + `ipc://` fetch), and shared-main-thread script execution with streamed output.

### 6.8 Suggested implementation order

1. **Close the semantic holes that make Tauri's own lifecycle wrong**: `app.exit { code }`, `app.exitResponse { allow }`, `window.closeResponse { windowId, allow }` (§6.2, §6.3). Everything else is additive; these three change existing flows.
2. **Fix D1** (`shell.openPath` response type) and type the fields listed in **D3/D4**, otherwise every new feature keeps leaking through `additional`.
3. **Add the missing event families** (`webview.consoleMessage`, `webview.frameEvent`, `window.scaleFactorChanged`) — they need no native code and unblock diagnostics.
4. **DevTools protocol bridge** (`webview.devToolsMessage` / `webview.devToolsProtocol`) — highest value per unit of work, because the JS side already speaks CDP.
5. **Process-wide policy surface** (§6.5) — mostly `app.commandLine.appendSwitch` before ready plus `session` APIs; one `app.configure` request can carry the whole set.
6. **OS integration** (§6.6) and the **native-addon** items (cursor grab/position, resize dragging, display handle, drag-and-drop paths) last.

