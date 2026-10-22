#!/usr/bin/env node
/**
 * Runs Tauri's official api example on this runtime.
 *
 * The example lives in the `source/tauri` submodule and is written against wry.
 * This copies the tauri workspace into `target/api-example/`, patches the copy
 * onto this runtime, runs it, and deletes the copy — the submodule is only ever
 * read, so it ends exactly as clean as it started.
 *
 *   node scripts/api-example.mjs
 *
 * The example is the broadest test we have: it drives menus, tray, dialogs,
 * events, channels, plugins and most of the webview API through the runtime.
 * Its frontend is not checked in, so build it once first:
 *
 *   cd source/tauri/examples/api && pnpm install && pnpm build
 */
import { spawnSync } from 'node:child_process'
import { cpSync, existsSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import path from 'node:path'

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const tauri = path.join(root, 'source', 'tauri')
const example = path.join(tauri, 'examples', 'api')
const workspace = path.join(root, 'target', 'api-example', 'tauri')
const app = path.join(workspace, 'examples', 'api', 'src-tauri')

if (!existsSync(path.join(example, 'dist', 'index.html'))) {
  console.error(
    `[api-example] the example's frontend is not built: ${path.join(example, 'dist', 'index.html')} is missing.\n` +
      '              run `cd source/tauri/examples/api && pnpm install && pnpm build` first.',
  )
  process.exit(1)
}

function patch(file, from, to) {
  // Submodule files are checked out with CRLF; anchor on LF and put the endings
  // back so an edit cannot silently miss.
  const text = readFileSync(file, 'utf8')
  const crlf = text.includes('\r\n')
  const normalized = crlf ? text.replaceAll('\r\n', '\n') : text
  if (!normalized.includes(from)) {
    throw new Error(`${path.relative(root, file)}: cannot find ${JSON.stringify(from)}`)
  }
  const patched = normalized.replaceAll(from, to)
  writeFileSync(file, crlf ? patched.replaceAll('\n', '\r\n') : patched)
}

function dropIsolation() {
  // Isolation delegates to `with_webview`, which an Electron guest cannot
  // honour, so both its feature and its config block go.
  patch(path.join(app, 'Cargo.toml'), '  "isolation",\n', '')
  const config = path.join(app, 'tauri.conf.json')
  const parsed = JSON.parse(readFileSync(config, 'utf8'))
  delete parsed.app?.security?.pattern
  writeFileSync(config, `${JSON.stringify(parsed, null, 2)}\n`)
}

let status = 1
try {
  rmSync(workspace, { force: true, recursive: true })
  console.log(`[api-example] copying ${path.relative(root, tauri)} to ${path.relative(root, workspace)}`)
  cpSync(tauri, workspace, { recursive: true })

  patch(
    path.join(workspace, 'Cargo.toml'),
    'tauri-plugin = { path = "./crates/tauri-plugin" }',
    `tauri-plugin = { path = "./crates/tauri-plugin" }
# The example builds \`tauri\` from this workspace while tauri-runtime-electron
# asks crates.io for these two: patch them onto the workspace paths so both
# sides see the same \`Runtime\` trait.
tauri-runtime = { path = "./crates/tauri-runtime" }
tauri-utils = { path = "./crates/tauri-utils" }`,
  )
  // `default-features` keeps `wry`, which would compile a second runtime;
  // `tray-icon` is what the example's tray page needs from that default set.
  patch(
    path.join(app, 'Cargo.toml'),
    'default-features = true\nfeatures = [\n',
    `default-features = false
features = [
  "tray-icon",
`,
  )
  patch(
    path.join(app, 'Cargo.toml'),
    'tauri-plugin-automation = { version = "0.2", optional = true }',
    `tauri-plugin-automation = { version = "0.2", optional = true }

tauri-runtime-electron = { path = ${JSON.stringify(root.replaceAll('\\', '/'))} }`,
  )
  patch(
    path.join(app, 'src', 'lib.rs'),
    'tauri::Builder::default()',
    'tauri::Builder::<tauri_runtime_electron::ElectronRuntime>::default()',
  )
  dropIsolation()

  console.log('[api-example] running Tauri’s api example on tauri-runtime-electron')
  // `prod` embeds the built frontend through the custom protocol, which is what
  // makes it run without the vite dev server.
  const run = spawnSync(
    'cargo',
    ['run', '--manifest-path', path.join(app, 'Cargo.toml'), '--features', 'prod'],
    {
      stdio: 'inherit',
      env: { ...process.env, CARGO_TARGET_DIR: path.join(root, 'target') },
    },
  )
  status = run.status ?? 1
} finally {
  rmSync(workspace, { force: true, recursive: true })
  console.log('[api-example] removed the patched copy; source/tauri was only read')
}
process.exit(status)
