# tauri-runtime-electron

Experimental Electron backend for Tauri, replacing the system WebView.

- Rust keeps the event loop, commands, CSP and ACL.
- Electron runs windows and webviews in a child process over local JSON-RPC.
- The build prepares Electron and app metadata from your existing Tauri config.

`main` targets **Tauri 2 / tauri-runtime 2.12**; `next` tracks **Tauri 3.0.0-alpha** with an incompatible runtime contract. Not published to crates.io; pin a Git revision or tag for releases.

```toml
[dependencies]
tauri = { version = "2", default-features = false }
tauri-runtime-electron = { git = "https://github.com/hairyf/tauri-runtime-electron", branch = "main" }

[build-dependencies]
tauri-build = "2"
```

Keep `tauri_build::build()` in your application's build script, then select the runtime through Tauri's generic builder:

```rust
use tauri_runtime_electron::ElectronRuntime;

fn main() {
    tauri::Builder::<ElectronRuntime>::new()
        .run(tauri::generate_context!())
        .expect("error while running application");
}
```

The crate's [build script](build.rs) merges the base, platform and CLI Tauri config and reads `productName`, `identifier` and `bundle.icon`. It uses the pinned `electron_rs::DEFAULT_VERSION` (currently `45.0.0`), a local install/cache or the official Electron release download with SHA-256 verification. No Electron environment variables or extra application builder call are needed. Automatic config discovery covers standard single-app layouts; ambiguous app selection fails explicitly. See the [discovery limits](docs/handoff.md) for workspaces and external target directories.

For Isolation or a custom protocol, register its scheme **before creating the runtime, on the same thread**, and keep the original context:

```rust
let context = tauri::generate_context!();
if let tauri::Pattern::Isolation { schema, .. } = context.pattern() {
    tauri_runtime_electron::register_custom_protocol_scheme(schema)?;
}
// Register other schemes here, e.g. register_custom_protocol_scheme("my-scheme")?.
tauri::Builder::<ElectronRuntime>::new().run(context)?;
```

## Bundling

```json
{"bundle":{"resources":{"target/electron":"runtime"}}}
```

Output is `target/electron/<platform>-<arch>`, such as `target/electron/win32-x64`, with **no version directory**. If your target root differs, adjust the source path and keep the destination `runtime`. Bundle the entire distribution, including libraries, metadata and generated scripts.

Native icons use `.ico` on Windows and `.icns` on macOS; Linux currently supports executable renaming only. Apply release signing/notarization after branding. macOS building and execution have not been verified in this environment.

Startup locates prepared files beside the app (`runtime`, `resources/runtime` or macOS `Resources/runtime`) and standard Linux package locations derived from `productName`: `../lib/<productName>/runtime`, `$APPDIR/usr/lib/<productName>/runtime` and `/usr/lib/<productName>/runtime`. Only debug builds can fall back to the build machine's staging path. **Startup never downloads, extracts or patches Electron.** Missing runtime files or scripts are errors.

For use without Tauri, see the separate [electron-rs README](electron-rs/README.md).

<a id="2-window-api-and-native-effects"></a>
<details>
<summary>Optional native window effects</summary>

Enable `features = ["tauri"]` on the runtime dependency to use its [window extension](src/window/extension.rs):

```rust
use tauri_runtime_electron::window::{Effect, EffectsBuilder, Window};
fn effects(window: tauri::Window<tauri_runtime_electron::ElectronRuntime>) -> tauri::Result<()> {
    let window = Window::from(window);
    window.set_effects(EffectsBuilder::new().effect(Effect::Acrylic).build())?;
    window.clear_effects()
}
```

For frontend calls, register `.plugin(tauri_runtime_electron::window::init())` on the app builder and declare the inline plugin in the application's build script:

```rust
fn main() {
    tauri_build::try_build(tauri_build::Attributes::new().plugin(
        "electron-window",
        tauri_build::InlinedPlugin::new().commands(&["set_effects"]),
    )).expect("failed to build application");
}
```

In a capability, grant `electron-window:allow-set-effects` to the intended windows, for example `{"identifier":"main-window","windows":["main"],"permissions":["electron-window:allow-set-effects"]}`.

Link the private, unpublished frontend package from this checkout with its `@tauri-apps/api` v2 peer:

```ts
import { Effect, getCurrentWindow } from 'tauri-runtime-electron/api/window'
const window = getCurrentWindow()
await window.setEffects({ effects: [Effect.Acrylic] })
await window.clearEffects()
```

Calls use normal Tauri IPC/ACL. Only Acrylic, Mica and Tabbed on Windows 11 22H2 (build 22621)+ are supported; the first supported entry wins. Unsupported platforms/lists return errors; clearing restores background color and alpha. `state`, `radius`, `interactive` and `color` have no effect here. Official Tauri window effects are not redirected to this extension.

</details>

## Limits

See the [method coverage](docs/coverage.md) for each member's purpose and exact-commit CI results. Unverified, skipped and failed members do not count as supported.

- Windows attached Tauri menus are mirrored to Electron; commands and accelerators dispatch through a Rust UI-thread menu owner. Menu changes synchronize during the event pump. Native menu integration on macOS/Linux and popup placement remain unverified. Cross-process IDs do not provide local WebView2, GTK or AppKit objects: `with_webview` errors without calling the callback; macOS native window handles and WKWebView data-store operations are unavailable.
- Page `window.open` defaults to native Chromium popups, retaining the original `WindowProxy`, opener, POST data and lifecycle. Use this crate's [new-window API](src/new_window.rs) for `Allow`/`Deny`/`Create`; the Tauri facade requires `features = ["tauri"]`. For `Create`, build inside `with_opener(features.opener(), || ...)` and return `NewWindowResponse::create(&window)`. Official `new_window_handler` still fails explicitly because its process-local WebView handles cannot cross processes; the original API example is not stripped or rewritten to hide that limitation. Accepted navigation resumes the original request; a subframe veto affects only that frame.
- Initialization scripts keep their frame scope; IPC is main-frame-only. `convertFileSrc` still needs `protocol-asset` and an `assetProtocol` scope. Matching resolved `data_directory` paths share a session; incognito webviews have separate in-memory sessions.
- Downloads stay private until complete and first approved by the host, then publish without overwriting. Publication failures retain the staged file and report its path; denial, timeout and destruction clean it up. Exit vetoes/codes are respected: `run_return` returns the code, while `run` exits the Rust process.
- Configured title-bar styles and hidden titles map to Electron window options; runtime title-bar-style changes remain unsupported. Non-Windows native dragging usually needs `-webkit-app-region: drag`. `DebugEnvironment::Auto` permits diagnostic environment variables only in debug builds. OS composition settings affect native materials; RPC success and CSS `backdrop-filter` do not establish desktop blur.

## Examples and checks

With Node.js 24+, pnpm, Rust and platform dependencies installed:

```bash
git submodule update --init source/tauri source/electron
pnpm example                         # list examples
pnpm example helloworld
pnpm example transparent
pnpm example commands --prepare-only
pnpm test
cargo check --locked --all-targets --all-features
```

Tests follow code ownership:

- [electron-rs tests](electron-rs/tests/) own acquisition, metadata and bridge behavior. Protocol/IPC, downloads, child layout and webview policy stay here, with Rust assertions over small Node VM fixtures executing the production JavaScript against native doubles.
- [Root tests](tests/) own the Tauri adapter, library window API and examples. JavaScript checks live in [tests/checks](tests/checks/), and [tests/common](tests/common/mod.rs) supplies the root Rust helper. Module unit tests stay beside their implementation under [src](src/).
- [scripts](scripts/) contains environment/native helpers and the example launch entry. It does not own test assertions.

[package.json](package.json) provides the shared entry points used by CI:

| Command | Scope and prerequisites |
| --- | --- |
| `pnpm test` | Bridge regressions, adapter unit/build/API/document tests and effects/evidence checks. No desktop is needed; the root build script still prepares Electron and the coverage check reads the Tauri submodule. |
| `pnpm run test:bridge:prepare` | Stage a private real Electron distribution under `target/electron-bridge`, exercise builder/resource/branding/Node operations and keep it for the next command. |
| `pnpm run test:bridge` | Use that prepared distribution for handshake, subscriptions, loaded-page preload/protocol IPC, native window/child webview RPCs and shutdown. Requires a desktop; see the [bridge setup](electron-rs/README.md#checks) for Linux sandbox permissions. |
| `pnpm run test:runtime` | All root Cargo tests, including acquisition, [window smoke](tests/runtime_smoke.rs) and [exit lifecycle](tests/runtime_lifecycle.rs). Requires a desktop; Linux uses Xvfb and a window manager. |
| `pnpm run test:examples helloworld state` | Build and check the selected original examples serially; `pnpm check` is an alias. Requires a desktop and example dependencies. |
| `pnpm run test:coverage` | Check that the generated method-coverage document is current; regenerate with `node tests/checks/coverage.mjs --write`. |

The [example runner](scripts/example.mjs) adapts copies under `target/example/tauri`, preserving framework sources and original pages/commands/CSP/ACL/Isolation. Run examples serially; `--prepare-only` keeps the generated app. The [checker](tests/checks/examples.mjs) saves assertions, logs and original screenshots under `target/example-checks/<name>/`; missing platform/device prerequisites are `blocked`, and both `blocked` and `failed` exit nonzero. Visual review remains `pending` until screenshots are inspected and recorded in `visual-review.json`.

The real bridge tests are explicitly ignored by default and run separately in CI. Desktop tests cover named subsets and report platform skips; the coverage test checks generated documentation. CI configuration is not proof of a passing run or three-platform/native-device validation. Development file arguments do not verify installed file associations. See [acceptance guidance](docs/acceptance.md).

[MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE-2.0).
