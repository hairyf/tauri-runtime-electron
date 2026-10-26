# Handoff

Electron preparation and process communication now live in the independent [electron-rs crate](../electron-rs/README.md). The Tauri adapter keeps its ordinary generic builder and application `tauri_build::build()` call. No Electron-specific setup environment variables are required or read.

## Build and runtime flow

- [build_support.rs](../build_support.rs) discovers the consuming application, uses Tauri's parser for base/platform config, then applies the CLI's `TAURI_CONFIG` merge patch. `productName` falls back to the consuming Cargo package name; native icons come from `bundle.icon` (`.ico` on Windows, `.icns` on macOS).
- [build.rs](../build.rs) uses the pinned Electron `DEFAULT_VERSION`, prepares a private copy under `target/electron/<platform>-<arch>`, and records validated identity for the adapter. Source/cache lookup and verified official-release downloads happen only during the build. Linux passes only app-name metadata to native branding; its identifier is forwarded internally to startup.
- Keep the resource map `{"target/electron":"runtime"}` and package the complete distribution, including scripts and metadata. Changing the Cargo target directory changes the source root, not the `runtime` destination.
- [runtime.rs](../src/runtime.rs) searches bundled adjacent paths and Linux product-name paths (`../lib/<productName>/runtime`, `$APPDIR/usr/lib/<productName>/runtime`, `/usr/lib/<productName>/runtime`). Only debug builds may use the recorded staging path. Missing binaries/scripts fail immediately; startup never downloads, extracts, brands or repairs Electron.
- [ipc.rs](../electron-rs/src/ipc.rs) owns process lifetime, bounded startup/handshake, buffered initial events, timed RPC, subscriptions and shutdown. The builder embeds its own main/preload assets; callers supply no script path.

## Configuration discovery boundary

A dependency build script is not given the selected dependent application's manifest or full Tauri configuration. Cargo's `OUT_DIR` identifies the target tree, while the build-script working directory and `CARGO_MANIFEST_DIR` identify this dependency. `TAURI_CONFIG` is a CLI override patch, not a complete configuration or directory pointer.

Discovery therefore uses bounded hints: absolute inherited `INIT_CWD`, absolute inherited `PWD`, then the parent of the derived target root. It locates nearby Cargo manifests, asks offline `cargo metadata --no-deps` for workspace packages, and accepts one package containing Tauri config. Multiple application configs in a candidate workspace fail explicitly; there is no recursive application scan or arbitrary first-app selection. Standalone library builds with no discovered app stage unbranded Electron.

Standard single-app layouts are supported. Multiple applications sharing one target output, stale inherited directory hints, or arbitrary external target directories without invocation hints cannot provide exact consumer selection. Cargo also does not fingerprint this dependency by the outer `-p` selection. Separate target directories are needed for independent branded builds; one builder owns each output. Exact parity for every Cargo invocation needs an explicit upstream application-context handoff, not another Electron environment knob.

The repository [example runner](../scripts/example.mjs) sets standard `INIT_CWD` to the selected generated app automatically, while retaining the working directory needed for upstream Cargo configuration. Ordinary consumers do not copy this runner setup.

## Validation at handoff

Checked on Windows on 2026-10-26:

- `cargo check --all-targets --all-features --offline` passed.
- `cargo test --all-features --locked --offline --lib --test build_config --test acquisition` passed: 11 unit tests, one config-merge/identity regression and one mandatory staged-Electron launch check.
- Strict Clippy passed for both the root crate and independent crate with `--locked --offline -- -D warnings`.
- Earlier in this extraction, root full tests passed, including exit lifecycle and the 92-check real Electron window smoke. Those desktop checks preceded the final automatic-config changes and were not repeated.
- Earlier Node protocol/layout/download/policy/window/evidence checks passed. Script syntax and generated coverage are checked again before committing.

The independent crate's previous full unit run had seven passes, one failure before builder execution when Windows denied temporary fixture-directory creation, and one ignored native PE smoke. The added acquisition checksum regression passed separately. Temporary permission changes used during diagnosis were restored; the fixture failure is not a passing test and must be reproduced on an unrestricted development/CI machine. See the ignored smoke in [pe.rs](../electron-rs/src/pe.rs) for opt-in native executable branding verification. No macOS hardware execution or production signing/notarization was verified here.

## Follow-up

1. Run the complete independent-crate suite on a machine where Rust tests can create temporary directories; run its opt-in PE smoke on a temporary copy of Electron.
2. Exercise a packaged release with configured product name and icon on each native platform, including Linux installed/AppImage resource paths. Cross-target compilation and pure path tests do not establish installed-package behavior.
3. Check multi-app/shared-target discovery before extending supported layouts. Do not infer a product name from the application identifier or silently scan unrelated directories.
4. Apply production executable signing and macOS notarization after branding. Development ad-hoc signing is not release signing.

Original staged skill-management changes and untracked contents in the Tauri submodule are outside this implementation commit and remain in the working tree.
