#!/usr/bin/env node
/**
 * Runs one of Tauri's official examples on this runtime.
 *
 *   pnpm example api
 *   pnpm example resources
 *   pnpm example file-associations <args...>
 *
 * The examples live in the `source/tauri` submodule and are written against wry,
 * so this copies the tauri workspace into `target/examples/<name>/`, patches the
 * copy onto this runtime, runs it, and deletes the copy again: the submodule is
 * only ever read.
 *
 * Examples whose frontend is not checked in (only `api`) need it built first:
 *
 *   cd source/tauri/examples/api && pnpm install && pnpm build
 */
import { spawnSync } from 'node:child_process'
import { cpSync, existsSync, readdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import path from 'node:path'

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const tauri = path.join(root, 'source', 'tauri')
const [name, ...args] = process.argv.slice(2)

/** Every example that carries its own Cargo project. */
function examples() {
  return readdirSync(path.join(tauri, 'examples'), { withFileTypes: true })
    .filter((entry) => entry.isDirectory())
    .map((entry) => entry.name)
    .filter((entry) => existsSync(path.join(tauri, 'examples', entry, 'src-tauri', 'Cargo.toml')))
}

if (!name || !examples().includes(name)) {
  console.error(
    `[example] usage: pnpm example <name> [args...]\n          buildable examples: ${examples().join(', ')}`,
  )
  process.exit(1)
}

const source = path.join(tauri, 'examples', name)
const workspace = path.join(root, 'target', 'examples', name, 'tauri')
const app = path.join(workspace, 'examples', name, 'src-tauri')

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

/** The example's frontend has to exist before tauri-build embeds it. */
function checkFrontend() {
  const config = JSON.parse(readFileSync(path.join(source, 'src-tauri', 'tauri.conf.json'), 'utf8'))
  const dist = config.build?.frontendDist
  if (!dist) return
  const missing = (Array.isArray(dist) ? dist : [dist])
    .map((entry) => path.join(source, 'src-tauri', entry))
    .filter((entry) => !existsSync(entry) && !existsSync(path.join(entry, 'index.html')))
  if (missing.length) {
    throw new Error(
      `the ${name} example's frontend is not built: ${missing.map((entry) => path.relative(root, entry)).join(', ')} is missing.\n` +
        `              run \`cd source/tauri/examples/${name} && pnpm install && pnpm build\` first.`,
    )
  }
}

/** Tauri's default features pull in `wry`, which this runtime replaces. */
function withoutWry(manifest) {
  const line = readFileSync(manifest, 'utf8')
    .split(/\r?\n/)
    .find((candidate) => /^tauri\s*=\s*\{/.test(candidate))
  if (!line) throw new Error(`${name}: no \`tauri = { .. }\` dependency in ${path.relative(root, manifest)}`)
  if (line.includes('default-features = true')) {
    patch(manifest, line, line.replace('default-features = true', 'default-features = false'))
  } else if (!line.includes('default-features')) {
    patch(manifest, line, line.replace('{ ', '{ default-features = false, '))
  }
}

let status = 1
let ran = false
try {
  checkFrontend()
  rmSync(path.dirname(workspace), { force: true, recursive: true })
  console.log(`[example] copying ${path.relative(root, tauri)} to ${path.relative(root, workspace)}`)
  cpSync(tauri, workspace, { recursive: true })

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
  const config = path.join(app, 'tauri.conf.json')
  const parsed = JSON.parse(readFileSync(config, 'utf8'))
  delete parsed.app?.security?.pattern
  writeFileSync(config, `${JSON.stringify(parsed, null, 2)}\n`)

  // Tauri's default features pull in `wry`, which this runtime replaces, and
  // `Builder::default` only exists without it.
  withoutWry(path.join(app, 'Cargo.toml'))
  const sources = readdirSync(path.join(app, 'src')).filter((file) => file.endsWith('.rs'))
  const entry = sources.find((file) =>
    readFileSync(path.join(app, 'src', file), 'utf8').includes('tauri::Builder::default()'),
  )
  if (!entry) throw new Error(`${name}: no \`tauri::Builder::default()\` to point at this runtime`)
  patch(
    path.join(app, 'src', entry),
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
    rmSync(path.dirname(workspace), { force: true, recursive: true })
  } else {
    console.error(`[example] the run failed; the patched copy is at ${path.relative(root, workspace)}`)
  }
}
process.exit(status)
