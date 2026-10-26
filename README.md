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

See the [method coverage](docs/coverage.md) for platform details. It describes implementation paths, not verified capabilities.

- Tauri native menus, callbacks, accelerators and popup attachment are unsupported. Cross-process IDs do not provide local WebView2, GTK or AppKit objects: `with_webview` errors without calling the callback; macOS native window handles and WKWebView data-store operations are unavailable.
- A configured `new_window_handler` fails creation; page `window.open` is denied without opening an external browser. Accepted navigation resumes the original request, preserving POST data and redirects. A subframe veto affects only that frame; Chromium may show an error page there.
- Initialization scripts keep their frame scope; IPC is main-frame-only. `convertFileSrc` still needs `protocol-asset` and an `assetProtocol` scope. Matching resolved `data_directory` paths share a session; incognito webviews have separate in-memory sessions.
- Downloads stay private until complete and first approved by the host, then publish without overwriting. Publication failures retain the staged file and report its path; denial, timeout and destruction clean it up. Exit vetoes/codes are respected: `run_return` returns the code, while `run` exits the Rust process.
- macOS transparent/hidden titles and runtime title-bar-style changes are unsupported. Non-Windows native dragging usually needs `-webkit-app-region: drag`. `DebugEnvironment::Auto` permits diagnostic environment variables only in debug builds. OS composition settings affect native materials; RPC success and CSS `backdrop-filter` do not establish desktop blur.

## Examples and checks

With Node.js 24+, pnpm, Rust and platform dependencies installed:

```bash
git submodule update --init source/tauri source/electron
pnpm example                         # list examples
pnpm example helloworld
pnpm example transparent
pnpm example commands --prepare-only
pnpm check helloworld state
node scripts/coverage.mjs --check
cargo check --locked --all-targets --all-features
cargo test --manifest-path electron-rs/Cargo.toml --locked
```

The [example runner](scripts/example.mjs) adapts copies under `target/example/tauri`, preserving framework sources and original pages/commands/CSP/ACL/Isolation. Run examples serially; `--prepare-only` keeps the generated app. The [checker](scripts/example-check.mjs) saves assertions, logs and original screenshots under `target/example-checks/<name>/`; missing platform/device prerequisites are `blocked`, and both `blocked` and `failed` exit nonzero. Visual review remains `pending` until screenshots are inspected and recorded in `visual-review.json`.

The root `cargo test` includes [window smoke](tests/runtime_smoke.rs) and [exit lifecycle](tests/runtime_lifecycle.rs) checks that need a desktop (Linux can use `xvfb-run`), cover named subsets and report skips. CI configuration is not proof of a passing run or three-platform/native-device validation. Development file arguments do not verify installed file associations. See [acceptance guidance](docs/acceptance.md).

[MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE-2.0).
