#!/usr/bin/env node
/**
 * Runs one of Tauri's official examples on this runtime.
 *
 *   pnpm example            # list what can run
 *   pnpm example api
 *   pnpm example multiwindow --some-arg
 *
 * The examples live in the `source/tauri` submodule and are written against wry,
 * so this copies the tauri workspace into `target/examples/<name>/tauri`,
 * patches the copy onto this runtime, runs it, and deletes the copy again: the
 * submodule is only ever read.
 *
 * Examples that ship no Cargo project (the ones whose `main.rs` is a doc
 * snippet) get a generated one, and an example whose frontend is not checked in
 * has it built with `pnpm install && pnpm build` inside the copy.
 */
import { spawnSync } from 'node:child_process'
import { cpSync, existsSync, mkdirSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import path from 'node:path'

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const tauri = path.join(root, 'source', 'tauri')
const [name, ...args] = process.argv.slice(2)

/** Crates a snippet `main.rs` imports but that the generated project cannot infer. */
const extraDependencies = {
  commands: { serde: '{ version = "1", features = ["derive"] }' },
  streaming: { http: '"1"', 'http-range': '"0.1"' },
}

/** Every example directory, with or without its own Cargo project. */
function examples() {
  return readdirSync(path.join(tauri, 'examples'), { withFileTypes: true })
    .filter((entry) => entry.isDirectory())
    .map((entry) => entry.name)
    .filter((entry) => {
      const source = path.join(tauri, 'examples', entry)
      return (
        existsSync(path.join(source, 'src-tauri', 'Cargo.toml')) || existsSync(path.join(source, 'main.rs'))
      )
    })
}

if (!name || !examples().includes(name)) {
  console.error(
    `[example] usage: pnpm example <name> [args...]\n          examples: ${examples().join(', ')}`,
  )
  process.exit(1)
}

const source = path.join(tauri, 'examples', name)
const full = existsSync(path.join(source, 'src-tauri', 'Cargo.toml'))
// One copy for every example: the path stays put, so switching examples only
// rebuilds the app, not the whole tauri workspace.
const copy = path.join(root, 'target', 'example')
const workspace = path.join(copy, 'tauri')
// A snippet's `main.rs` opens `../../examples/<name>/tauri.conf.json`, so its
// crate sits in the example directory itself instead of a `src-tauri` child.
const app = path.join(workspace, 'examples', name, ...(full ? ['src-tauri'] : []))

/** Patches are loud: an anchor that misses throws instead of quietly doing nothing. */
function patch(file, from, to) {
  if (!existsSync(file)) throw new Error(`${path.relative(root, file)}: no such file`)
  // Submodule files are checked out with CRLF; anchor on LF and put it back.
  const text = readFileSync(file, 'utf8')
  const crlf = text.includes('\r\n')
  const normalized = crlf ? text.replaceAll('\r\n', '\n') : text
  if (!normalized.includes(from)) {
    throw new Error(`${path.relative(root, file)}: cannot find ${JSON.stringify(from)}`)
  }
  const patched = normalized.replaceAll(from, to)
  writeFileSync(file, crlf ? patched.replaceAll('\n', '\r\n') : patched)
}

/** Tauri's default features pull in `wry`, which this runtime replaces. */
function withoutWry(manifest) {
  const lines = readFileSync(manifest, 'utf8').split(/\r?\n/)
  const inline = lines.find((candidate) => /^tauri\s*=\s*\{/.test(candidate))
  if (inline) {
    if (inline.includes('default-features = true')) {
      patch(manifest, inline, inline.replace('default-features = true', 'default-features = false'))
    } else if (!inline.includes('default-features')) {
      patch(manifest, inline, inline.replace('{ ', '{ default-features = false, '))
    }
    return
  }
  // The api example spells the same thing as a `[dependencies.tauri]` table.
  const start = lines.indexOf('[dependencies.tauri]')
  if (start === -1) throw new Error(`${name}: no tauri dependency in ${path.relative(root, manifest)}`)
  const end = lines.findIndex((line, index) => index > start && line.startsWith('['))
  const table = lines.slice(start, end === -1 ? undefined : end)
  const feature = table.find((line) => line.startsWith('default-features'))
  if (feature) {
    patch(manifest, feature, feature.replace('true', 'false'))
  } else {
    patch(manifest, '[dependencies.tauri]', '[dependencies.tauri]\ndefault-features = false')
  }
}

/** Builds the example's frontend inside the copy when it is not checked in. */
function ensureFrontend() {
  const config = JSON.parse(readFileSync(path.join(app, 'tauri.conf.json'), 'utf8'))
  const dist = config.build?.frontendDist
  if (!dist) return
  const missing = () =>
    (Array.isArray(dist) ? dist : [dist])
      .map((entry) => path.join(app, entry))
      // A checked-in directory with nothing in it is not a frontend: Electron
      // would open a window onto `asset not found: index.html`.
      .filter((entry) => !existsSync(entry) || (statSync(entry).isDirectory() && !existsSync(path.join(entry, 'index.html'))))
  if (!missing().length) return

  const project = path.dirname(app)
  if (!existsSync(path.join(project, 'package.json'))) {
    throw new Error(
      `the ${name} example has no frontend to run: ${missing().map((entry) => path.relative(root, entry)).join(', ')} is missing and there is no package.json to build it from`,
    )
  }
  for (const script of ['install', 'build']) {
    console.log(`[example] building the ${name} frontend: pnpm ${script}`)
    const result = spawnSync('pnpm', [script], {
      cwd: project,
      shell: process.platform === 'win32',
      stdio: 'inherit',
    })
    if (result.status !== 0) throw new Error(`pnpm ${script} failed in ${path.relative(root, project)}`)
  }
  if (missing().length) {
    throw new Error(`the ${name} example's build did not produce ${missing().map((entry) => path.relative(root, entry)).join(', ')}`)
  }
}

/** Gives a doc-snippet example the Cargo project it never had. */
function generateProject() {
  const dependencies = Object.entries(extraDependencies[name] ?? {})
    .map(([crate, version]) => `${crate} = ${version}`)
    .join('\n')
  // Snippets reach for `tauri_utils` directly; the path dependency keeps them on
  // the same crate the workspace patches.
  const utils = readdirSync(source).some(
    (file) => file.endsWith('.rs') && readFileSync(path.join(source, file), 'utf8').includes('tauri_utils'),
  )
    ? 'tauri-utils = { path = "../../crates/tauri-utils" }\n'
    : ''
  writeFileSync(
    path.join(app, 'Cargo.toml'),
    `[package]
name = ${JSON.stringify(name)}
version = "0.1.0"
edition = "2021"

[[bin]]
name = ${JSON.stringify(name)}
path = "main.rs"

[workspace]

[build-dependencies]
tauri-build = { path = "../../crates/tauri-build", default-features = false }

[dependencies]
tauri = { path = "../../crates/tauri", default-features = false }
${utils}${dependencies}
tauri-runtime-electron = { path = ${JSON.stringify(root.replaceAll('\\', '/'))} }

[patch.crates-io]
tauri-runtime = { path = "../../crates/tauri-runtime" }
tauri-utils = { path = "../../crates/tauri-utils" }
`,
  )
  writeFileSync(path.join(app, 'build.rs'), 'fn main() {\n  tauri_build::build()\n}\n')
  for (const file of readdirSync(source)) {
    if (file.endsWith('.rs') || file === 'index.html' || file === 'tauri.conf.json') {
      writeFileSync(path.join(app, file), readFileSync(path.join(source, file)))
    }
  }
}

let status = 1
let ran = false
try {
  // A pristine copy, timestamped the way the submodule is: cargo treats an
  // identical tree as unchanged, so only the example's own crate recompiles
  // between runs instead of the whole tauri workspace.
  const base = path.join(root, 'target', 'example-base')
  if (!existsSync(base) || statSync(base, { bigint: false }).mtimeMs < statSync(path.join(tauri, 'Cargo.toml')).mtimeMs) {
    rmSync(base, { force: true, recursive: true })
    cpSync(tauri, base, { preserveTimestamps: true, recursive: true })
  }
  rmSync(copy, { force: true, recursive: true })
  console.log(`[example] copying ${path.relative(root, tauri)} to ${path.relative(root, workspace)}`)
  cpSync(base, workspace, { preserveTimestamps: true, recursive: true })

  if (full) {
    // The example builds `tauri` from this workspace while this runtime asks
    // crates.io for these two: one `Runtime` trait needs one path for both.
    patch(
      path.join(workspace, 'Cargo.toml'),
      'tauri-plugin = { path = "./crates/tauri-plugin" }',
      `tauri-plugin = { path = "./crates/tauri-plugin" }
tauri-runtime = { path = "./crates/tauri-runtime" }
tauri-utils = { path = "./crates/tauri-utils" }`,
    )
    writeFileSync(
      path.join(app, 'Cargo.toml'),
      `${readFileSync(path.join(app, 'Cargo.toml'), 'utf8')}
[dependencies.tauri-runtime-electron]
path = ${JSON.stringify(root.replaceAll('\\', '/'))}
`,
    )
    // Isolation delegates to `with_webview`, which an Electron guest cannot
    // honour; only some examples use it.
    if (readFileSync(path.join(app, 'Cargo.toml'), 'utf8').includes('"isolation"')) {
      patch(path.join(app, 'Cargo.toml'), '  "isolation",\n', '')
    }
    withoutWry(path.join(app, 'Cargo.toml'))
    ensureFrontend()
  } else {
    generateProject()
  }

  const config = path.join(app, 'tauri.conf.json')
  const parsed = JSON.parse(readFileSync(config, 'utf8'))
  delete parsed.app?.security?.pattern
  writeFileSync(config, `${JSON.stringify(parsed, null, 2)}\n`)

  // `Builder::default` only exists without the `wry` feature, so every entry
  // point names the runtime instead.
  const entries = full
    ? readdirSync(path.join(app, 'src')).map((file) => path.join(app, 'src', file))
    : readdirSync(app).map((file) => path.join(app, file))
  const entry = entries
    .filter((file) => file.endsWith('.rs'))
    .find((file) => readFileSync(file, 'utf8').includes('tauri::Builder::default()'))
  if (!entry) throw new Error(`${name}: no \`tauri::Builder::default()\` to point at this runtime`)
  patch(
    entry,
    'tauri::Builder::default()',
    'tauri::Builder::<tauri_runtime_electron::ElectronRuntime>::new()',
  )

  // `prod` embeds the frontend through the custom protocol, so it runs without
  // the vite dev server; examples that ship their html do not need it.
  const features = readFileSync(path.join(app, 'Cargo.toml'), 'utf8').includes('\nprod = [')
    ? ['--features', 'prod']
    : []
  console.log(`[example] running the ${name} example on tauri-runtime-electron`)
  ran = true
  const run = spawnSync(
    'cargo',
    ['run', '--manifest-path', path.join(app, 'Cargo.toml'), ...features, ...(args.length ? ['--', ...args] : [])],
    { stdio: 'inherit', env: { ...process.env, CARGO_TARGET_DIR: path.join(root, 'target') } },
  )
  status = run.status ?? 1
} catch (error) {
  console.error(`[example] ${error.message}`)
} finally {
  // Keep the copy when the run itself failed, so the build log is inspectable.
  if (status === 0 || !ran) {
    rmSync(copy, { force: true, recursive: true })
  } else {
    console.error(`[example] the run failed; the patched copy is at ${path.relative(root, workspace)}`)
  }
}
process.exit(status)
