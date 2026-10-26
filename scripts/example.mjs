#!/usr/bin/env node
/**
 * Run a local or official Tauri example on Electron without modifying the submodule.
 *
 *   pnpm example                         # list examples
 *   pnpm example commands
 *   pnpm example commands --prepare-only # keep the generated project; no cargo/GUI
 *   pnpm example file-associations -- path/to/image.png
 *
 * All runs share target/example/tauri to reuse Cargo artifacts. An exclusive
 * lock protects that copy until cargo (and its application) has exited.
 */
import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import {
  closeSync, cpSync, existsSync, ftruncateSync, lstatSync, mkdirSync, openSync,
  readdirSync, readFileSync, rmSync, statSync, writeFileSync, writeSync,
} from 'node:fs'
import { fileURLToPath } from 'node:url'
import path from 'node:path'

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const tauri = path.join(root, 'source', 'tauri')
const [name, ...args] = process.argv.slice(2)
const prepareOnly = args[0] === '--prepare-only'
if (prepareOnly) args.shift()
if (args[0] === '--') args.shift()

/** Crates imported by snippets, originally supplied by the tauri crate itself. */
const extraDependencies = {
  commands: { serde: '{ version = "1", features = ["derive"] }' },
  streaming: { http: '"1"', 'http-range': '"0.1"', getrandom: '"0.3"', 'percent-encoding': '"2"' },
}
// Preserve the pinned Tauri defaults while selecting Electron instead of Wry.
const tauriFeatures = ['compression', 'common-controls-v6', 'dynamic-acl', 'x11', 'dbus']

function examples() {
  return [...new Set([path.join(root, 'example'), path.join(tauri, 'examples')].flatMap((directory) =>
    existsSync(directory) ? readdirSync(directory, { withFileTypes: true })
      .filter((entry) => entry.isDirectory() && /^[a-z][a-z0-9-]*$/.test(entry.name))
      .filter((entry) => existsSync(path.join(directory, entry.name, 'src-tauri', 'Cargo.toml')) || existsSync(path.join(directory, entry.name, 'main.rs')))
      .map((entry) => entry.name) : [],
  ))].sort()
}

/** Keep file operations inside their expected tree; never follow a junction. */
function localPath(base, file) {
  const relative = path.relative(base, file)
  if (path.isAbsolute(relative) || relative.split(path.sep).includes('..')) {
    throw new Error(`path escapes ${base}: ${file}`)
  }
  let current = base
  for (const part of ['', ...relative.split(path.sep).filter(Boolean)]) {
    current = path.join(current, part)
    if (existsSync(current) && lstatSync(current).isSymbolicLink()) {
      throw new Error(`refusing symlink/junction: ${current}`)
    }
  }
  return file
}

const validName = /^[a-z][a-z0-9-]*$/.test(name ?? '')
const localSource = path.join(root, 'example', name ?? '')
const local = validName && (existsSync(path.join(localSource, 'main.rs')) || existsSync(path.join(localSource, 'src-tauri/Cargo.toml')))
const source = local ? localSource : path.join(tauri, 'examples', name ?? '')
const full = /^[a-z][a-z0-9-]*$/.test(name ?? '') && existsSync(path.join(source, 'src-tauri', 'Cargo.toml'))
const copy = path.join(root, 'target', 'example')
const workspace = path.join(copy, 'tauri')
// Snippets open ../../examples/<name>/tauri.conf.json relative to their crate.
const app = path.join(workspace, 'examples', name ?? '', ...(full ? ['src-tauri'] : []))
const manifest = path.join(app, 'Cargo.toml')
const lock = path.join(root, 'target', 'example.lock')
let lockFd
let child

function record(command = null) {
  ftruncateSync(lockFd, 0)
  writeSync(lockFd, `${JSON.stringify({ pid: process.pid, childPid: child?.pid ?? null, name, command })}\n`, 0, 'utf8')
}

function alive(pid) {
  if (!Number.isSafeInteger(pid) || pid <= 0) return false
  try {
    process.kill(pid, 0)
    return true
  } catch (error) {
    return error.code !== 'ESRCH'
  }
}

function acquireLock() {
  localPath(root, lock)
  mkdirSync(path.dirname(lock), { recursive: true })
  try {
    lockFd = openSync(lock, 'wx')
  } catch (error) {
    if (error.code !== 'EEXIST') throw error
    let owner
    try { owner = JSON.parse(readFileSync(lock, 'utf8')) } catch { /* Unknown owner: do not overwrite. */ }
    const active = alive(owner?.pid) || alive(owner?.childPid)
    // A hard-killed runner leaves its record behind; both recorded processes
    // being gone is proof that nobody holds the copy, so take the lock over.
    if (active || !Number.isSafeInteger(owner?.pid) || owner.pid <= 0) {
      throw new Error(`${path.relative(root, lock)} ${active ? 'is in use' : 'has an unverified owner'} (runner=${owner?.pid ?? '?'}, child=${owner?.childPid ?? '?'}). Confirm its process tree has exited before removing that lock; no processes were stopped.`)
    }
    console.log(`[example] removing stale ${path.relative(root, lock)} (runner=${owner.pid}, child=${owner.childPid ?? '?'} exited)`)
    rmSync(lock, { force: true })
    lockFd = openSync(lock, 'wx')
  }
  record()
}

/** Await the actual child, retaining its PID for a scheduler and stale-lock checks. */
async function run(command, argv, options = {}) {
  try {
    return await new Promise((resolve, reject) => {
      child = spawn(command, argv, { stdio: 'inherit', ...options })
      child.once('error', (error) => {
        child = undefined
        reject(error)
      })
      child.once('close', (code, signal) => {
        child = undefined
        if (signal) console.error(`[example] ${command} exited on ${signal}`)
        resolve(code ?? 1)
      })
      record(command)
      console.log(`[example] runner PID ${process.pid}; ${command} PID ${child.pid ?? 'not started'}`)
    })
  } finally {
    if (!child) record()
  }
}

/** Patches are loud: a missing anchor fails instead of silently changing nothing. */
function patch(file, from, to) {
  localPath(app, file)
  const text = readFileSync(file, 'utf8')
  const crlf = text.includes('\r\n')
  const normalized = text.replaceAll('\r\n', '\n')
  if (!normalized.includes(from)) {
    throw new Error(`${path.relative(root, file)}: cannot find ${JSON.stringify(from)}`)
  }
  const patched = normalized.replaceAll(from, to)
  writeFileSync(file, crlf ? patched.replaceAll('\n', '\r\n') : patched)
}

function optionalPatch(file, from, to) {
  if (readFileSync(file, 'utf8').replaceAll('\r\n', '\n').includes(from)) patch(file, from, to)
}

/** Build real upstream frontend sources, including the local @tauri-apps/api. */
async function ensureFrontend(config) {
  const dist = config.build?.frontendDist
  if (!dist) return
  if (typeof dist === 'string' && /^https?:\/\//.test(dist)) return
  const paths = (Array.isArray(dist) ? dist : [dist]).map((entry) => {
    if (typeof entry !== 'string' || !entry || path.isAbsolute(entry)) {
      throw new Error(`${name}: invalid frontendDist path ${JSON.stringify(entry)}`)
    }
    return localPath(workspace, path.resolve(app, entry))
  })
  const missing = () => paths.filter((entry) =>
    !existsSync(entry) || (statSync(entry).isDirectory() && !existsSync(path.join(entry, 'index.html'))),
  )
  if (!missing().length) return
  const project = full ? path.dirname(app) : app
  if (!existsSync(path.join(project, 'package.json'))) {
    throw new Error(`${name}: missing frontend ${missing().join(', ')}; no package.json to build it`)
  }
  const pnpm = async (argv, cwd = project) => {
    console.log(`[example] pnpm ${argv.join(' ')}`)
    const code = await run('pnpm', argv, { cwd, shell: process.platform === 'win32' })
    if (code !== 0) throw new Error(`pnpm ${argv.join(' ')} failed (${code}) in ${path.relative(root, cwd)}`)
  }
  if (name === 'api') {
    // Installing the whole upstream workspace also installs unrelated mobile/E2E tooling.
    await pnpm(['--filter', '@tauri-apps/api', '--filter', 'api', 'install', '--frozen-lockfile'], workspace)
    await pnpm(['--filter', '@tauri-apps/api', 'build'], workspace)
  } else {
    await pnpm(['install', ...(process.env.CI ? ['--frozen-lockfile'] : [])])
  }
  await pnpm(['build'])
  if (missing().length) throw new Error(`${name}: build did not produce ${missing().join(', ')}`)
}

function generateProject(config) {
  const dependencies = Object.entries(extraDependencies[name] ?? {})
    .map(([crate, version]) => `${crate} = ${version}`).join('\n')
  const isolation = config.app?.security?.pattern?.use === 'isolation'
  const features = [...tauriFeatures, 'custom-protocol', ...(isolation ? ['isolation'] : []), ...(name === 'multiwebview' ? ['unstable'] : [])]
  const utils = readdirSync(source).some((file) =>
    file.endsWith('.rs') && readFileSync(path.join(source, file), 'utf8').includes('tauri_utils'),
  ) ? 'tauri-utils = { path = "../../crates/tauri-utils" }\n' : ''
  writeFileSync(manifest, `[package]
name = ${JSON.stringify(name)}
version = "0.1.0"
edition = "2021"

[[bin]]
name = ${JSON.stringify(name)}
path = "main.rs"

[workspace]

[build-dependencies]
tauri-build = { path = "../../crates/tauri-build", features = ${JSON.stringify(isolation ? ['isolation'] : [])} }

[dependencies]
tauri = { path = "../../crates/tauri", default-features = false, features = ${JSON.stringify(features)} }
${utils}${dependencies}
tauri-runtime-electron = { path = ${JSON.stringify(root.replaceAll('\\', '/'))}${name === 'transparent' ? ', features = ["tauri"]' : ''} }

[patch.crates-io]
tauri = { path = "../../crates/tauri" }
tauri-runtime = { path = "../../crates/tauri-runtime" }
tauri-utils = { path = "../../crates/tauri-utils" }
`)
  writeFileSync(path.join(app, 'build.rs'), name === 'transparent' ? `fn main() {
  tauri_build::try_build(tauri_build::Attributes::new().plugin(
    "electron-window",
    tauri_build::InlinedPlugin::new().commands(&["set_effects"]),
  )).expect("failed to build transparent example");
}
` : 'fn main() {\n  tauri_build::build()\n}\n')
}

let status = 1
try {
  if (!name || !examples().includes(name) || (prepareOnly && args.length)) {
    throw new Error(`usage: pnpm example <name> [--prepare-only | [--] args...]\n          examples: ${examples().join(', ')}`)
  }
  localPath(root, source)
  acquireLock()
  // ponytail: one shared copy serializes examples; use per-run workspaces if parallel GUI runs are needed.
  // No mtime-only base cache: any source/config/asset change must reach the next run.
  for (const directory of [copy, path.join(root, 'target', 'example-base')]) {
    localPath(root, directory)
    rmSync(directory, { force: true, recursive: true, maxRetries: 5, retryDelay: 300 })
  }
  console.log(`[example] copying ${path.relative(root, tauri)} to ${path.relative(root, workspace)}`)
  cpSync(tauri, workspace, {
    preserveTimestamps: true,
    recursive: true,
    filter: (file) => {
      if (path.relative(tauri, file).split(path.sep).some((part) => ['.git', 'node_modules', 'target'].includes(part))) return false
      if (lstatSync(file).isSymbolicLink()) throw new Error(`refusing source symlink/junction: ${file}`)
      return true
    },
  })
  if (local) {
    cpSync(source, path.join(workspace, 'examples', name), {
      recursive: true,
      filter: (file) => {
        if (lstatSync(file).isSymbolicLink()) throw new Error(`refusing source symlink/junction: ${file}`)
        return !path.relative(source, file).split(path.sep).some((part) => ['node_modules', 'target'].includes(part))
      },
    })
  }

  if (name === 'transparent') {
    cpSync(path.join(root, 'api', 'window.js'), path.join(app, 'window-api.js'))
  }

  const originalConfig = path.join(source, ...(full ? ['src-tauri'] : []), 'tauri.conf.json')
  const config = JSON.parse(readFileSync(originalConfig, 'utf8'))
  if (full) {
    // Isolate the app workspace so type unification never edits the framework.
    const text = readFileSync(manifest, 'utf8').replaceAll('\r\n', '\n')
    const dependency = text.includes('[dependencies.tauri]\n')
      ? '[dependencies.tauri]\nworkspace = true\ndefault-features = true\nfeatures = ['
      : text.match(/^tauri = \{ workspace = true, default-features = true(?:, features = \[)?/m)?.[0]
    assert.ok(dependency, `${name}: missing Tauri workspace dependency`)
    patch(manifest, dependency, dependency.replace('default-features = true', 'default-features = false')
      + (dependency.endsWith('[') ? `\n  ${tauriFeatures.map((feature) => JSON.stringify(feature)).join(', ')},` : `, features = ${JSON.stringify(tauriFeatures)}`))
    const workspaceManifest = readFileSync(path.join(workspace, 'Cargo.toml'), 'utf8')
    const inherited = workspaceManifest.replaceAll('\r\n', '\n').match(/^\[workspace\.dependencies\]\n([\s\S]*?)(?=^\[)/m)?.[1]
    assert.ok(inherited, 'missing upstream workspace dependencies')
    writeFileSync(manifest, `${readFileSync(manifest, 'utf8')}
[dependencies.tauri-runtime-electron]
path = ${JSON.stringify(root.replaceAll('\\', '/'))}

[workspace]

[workspace.dependencies]
${inherited.replaceAll('path = "./crates/', 'path = "../../../crates/')}
[patch.crates-io]
tauri = { path = "../../../crates/tauri" }
tauri-runtime = { path = "../../../crates/tauri-runtime" }
tauri-utils = { path = "../../../crates/tauri-utils" }
`)
  } else {
    generateProject(config)
  }

  const entries = readdirSync(full ? path.join(app, 'src') : app)
    .filter((file) => file.endsWith('.rs'))
    .map((file) => path.join(app, ...(full ? ['src'] : []), file))
  const entry = entries.find((file) => readFileSync(file, 'utf8').includes('tauri::Builder::default()'))
  if (!entry) throw new Error(`${name}: no tauri::Builder::default() entry point`)
  const runtime = 'tauri_runtime_electron::ElectronRuntime'
  patch(entry, 'tauri::Builder::default()', `tauri::Builder::<${runtime}>::new()`)
  optionalPatch(entry, ': AppHandle)', `: AppHandle<${runtime}>)`)
  optionalPatch(entry, ': AppHandle,', `: AppHandle<${runtime}>,`)
  optionalPatch(entry, ': Window)', `: Window<${runtime}>)`)
  optionalPatch(entry, '-> tauri::Context {', `-> tauri::Context<${runtime}> {`)
  if (config.app?.security?.pattern?.use === 'isolation') {
    // Preserve the exact Context/macro. Its random isolation scheme must be
    // privileged before Electron starts, including on macOS/Linux native URLs.
    const contexts = readFileSync(entry, 'utf8').replaceAll('\r\n', '\n').match(/tauri::(?:generate_context!\([\s\S]*?\)|tauri_build_context!\(\))/g) ?? []
    assert.equal(contexts.length, 1, `${name}: expected one original Context macro`)
    const [context] = contexts
    patch(entry, context, `{
      let context = ${context};
      if let tauri::Pattern::Isolation { schema, .. } = context.pattern() {
        tauri_runtime_electron::register_custom_protocol_scheme(schema)
          .expect("invalid isolation protocol scheme");
      }
      context
    }`)
  }
  if (name === 'file-associations') {
    // Upstream parses a Windows drive prefix as a URL scheme, losing native file argv.
    patch(entry, 'if let Ok(url) = url::Url::parse(&maybe_file) {', `if std::path::Path::new(&maybe_file).is_absolute() {
            files.push(PathBuf::from(maybe_file));
          } else if let Ok(url) = url::Url::parse(&maybe_file) {`)
  }

  if (name === 'api') {
    // Upstream reads this flag without tracking it. Invalidate an earlier build
    // made without the workspace config, then keep future Cargo caching correct.
    patch(path.join(app, 'build.rs'), 'fn main() {', 'fn main() {\n  println!("cargo:rerun-if-env-changed=__TAURI_WORKSPACE__");')
    // Tauri 2 hands on_new_window a native WebView2/WKWebView/GTK opener. This
    // runtime rejects such a handler at creation instead of fabricating one,
    // which would fail the setup hook, so drop the demo window and its counter.
    patch(entry, `        use std::sync::atomic::{AtomicU64, Ordering};

        let app_ = app.handle().clone();
        let created_window_count = AtomicU64::new(0);
`, '        // Electron cannot supply the native opener `on_new_window` requires.\n')
    patch(entry, `          .menu(tauri::menu::Menu::default(app.handle())?)
          .on_new_window(move |url, features| {
            println!("new window requested: {url:?} {features:?}");

            let number = created_window_count.fetch_add(1, Ordering::Relaxed);

            let builder = WebviewWindowBuilder::new(
              &app_,
              format!("new-{number}"),
              tauri::WebviewUrl::External("about:blank".parse().unwrap()),
            )
            .window_features(features)
            .on_document_title_changed(|window, title| {
              window.set_title(&title).unwrap();
            })
            .title(url.as_str());

            let window = builder.build().unwrap();
            tauri::webview::NewWindowResponse::Create { window }
          });
`, '          .menu(tauri::menu::Menu::default(app.handle())?);\n')
  }
  await ensureFrontend(config)
  if (name === 'streaming') {
    // Reuse the pinned Electron test video: real media, without a 150 MB download.
    const video = path.join(root, 'source/electron/spec/fixtures/cat-spin.mp4')
    if (!existsSync(video)) throw new Error('missing Electron video fixture; run git submodule update --init source/electron')
    cpSync(video, path.join(app, 'streaming_example_test_video.mp4'))
  }
  // Framework manifests and runtime/effect implementations stay byte-identical.
  for (const file of [
    'Cargo.toml',
    'crates/tauri/Cargo.toml',
    'crates/tauri/src/app.rs',
    'crates/tauri/src/vibrancy/mod.rs',
    'crates/tauri/src/window/mod.rs',
    'crates/tauri-runtime/Cargo.toml',
    'crates/tauri-runtime/src/lib.rs',
    'crates/tauri-utils/Cargo.toml',
  ]) {
    assert.deepEqual(readFileSync(path.join(workspace, file)), readFileSync(path.join(tauri, file)),
      `framework file must be copied unchanged: ${file}`)
  }
  // Runnable preparation checks: security configuration and real upstream assets stay intact.
  assert.equal(readFileSync(path.join(app, 'tauri.conf.json'), 'utf8'), readFileSync(originalConfig, 'utf8'))
  if (config.app?.security?.pattern?.use === 'isolation') {
    assert.match(readFileSync(manifest, 'utf8'), /"isolation"/)
    const isolationDir = localPath(workspace, path.resolve(app, config.app.security.pattern.options.dir))
    assert.ok(existsSync(path.join(isolationDir, 'index.html')), 'missing real isolation frontend')
  }

  if (prepareOnly) {
    console.log(`[example] prepared manifest: ${manifest}`)
    status = 0
  } else {
    console.log(`[example] running ${name} on tauri-runtime-electron`)
    // Static assets, never a hidden Vite server. Pass application arguments without a shell.
    status = await run('cargo', ['run', '--manifest-path', manifest, '--features', 'tauri/custom-protocol',
      ...(args.length ? ['--', ...args] : [])], {
      // Cargo reads .cargo/config.toml from cwd, not --manifest-path. The
      // upstream workspace enables the Windows Common Controls v6 manifest here.
      cwd: name === 'streaming' ? app : workspace,
      env: { ...process.env, INIT_CWD: app, CARGO_TARGET_DIR: path.join(root, 'target') },
    })
  }
} catch (error) {
  console.error(`[example] ${error.message}`)
} finally {
  if (lockFd !== undefined) {
    try {
      // A failed preparation/run is evidence, not disposable. Hard kills leave the lock too.
      if (status === 0 && !prepareOnly && !child) {
        localPath(root, copy)
        rmSync(copy, { force: true, recursive: true, maxRetries: 5, retryDelay: 300 })
      } else if (status !== 0) {
        console.error(`[example] retained patched workspace: ${workspace}`)
      }
    } catch (error) {
      status = 1
      console.error(`[example] cleanup failed: ${error.message}`)
    } finally {
      closeSync(lockFd)
      if (!child) rmSync(lock, { force: true })
    }
  }
}
process.exitCode = status
