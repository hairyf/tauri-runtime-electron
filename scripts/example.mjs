#!/usr/bin/env node
// Run any of Tauri's official examples on the Electron runtime.
//
//   pnpm example <name>          # or: node scripts/example.mjs <name>
//   pnpm example --list
//
// Each example is staged under .cache/examples/<name>, its runtime is swapped
// for this crate, and `cargo run` starts it. Examples with a Vite frontend get
// the dev server on :1420; the rest embed their `frontendDist` at build time.
// Ctrl+C stops the app, cargo and Vite.

import { spawn, spawnSync } from 'node:child_process'
import fs from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const examplesDir = path.join(repoRoot, 'source', 'tauri', 'examples')
const cacheRoot = path.join(repoRoot, '.cache', 'examples')
const PORT = 1420

const args = process.argv.slice(2)
const flags = new Set(args.filter((arg) => arg.startsWith('--')))
const name = args.find((arg) => !arg.startsWith('--'))

let viteProc = null
let cargoProc = null
let shuttingDown = false
let tauriFeatures = ['unstable', 'tray-icon', 'image-png', 'image-ico', 'devtools']

function listExamples() {
  const names = fs
    .readdirSync(examplesDir, { withFileTypes: true })
    .filter((entry) => entry.isDirectory())
    .map((entry) => entry.name)
    .filter((entry) => entry !== 'cef' && entry !== 'README.md')
    .filter((entry) => {
      const dir = path.join(examplesDir, entry)
      return (
        fs.existsSync(path.join(dir, 'src-tauri', 'Cargo.toml')) ||
        fs.existsSync(path.join(dir, 'main.rs'))
      )
    })
  console.log(`usage: pnpm example <name>\n\nrunnable examples:\n  ${names.join('\n  ')}`)
}

function fail(message) {
  console.error(`\n[example] ${message}`)
  kill(viteProc)
  kill(cargoProc)
  process.exit(1)
}

function read(file) {
  // Windows checkouts hand back CRLF, and every rewrite below matches on LF.
  return fs.readFileSync(file, 'utf8').replaceAll('\r\n', '\n')
}

function write(file, contents) {
  fs.mkdirSync(path.dirname(file), { recursive: true })
  fs.writeFileSync(file, contents)
}

function copyTree(from, to) {
  fs.mkdirSync(path.dirname(to), { recursive: true })
  fs.cpSync(from, to, { recursive: true })
}

function ensureExampleSource() {
  if (fs.existsSync(path.join(examplesDir, 'api', 'package.json'))) return
  console.log('[example] source/tauri is missing; running `git submodule update --init source/tauri`')
  const result = spawnSync('git', ['submodule', 'update', '--init', 'source/tauri'], {
    cwd: repoRoot,
    stdio: 'inherit',
  })
  if (result.status !== 0) fail('could not check out source/tauri (are you offline?)')
}

function stageConf(confPath, useVite) {
  const conf = JSON.parse(read(confPath))
  conf.$schema = 'https://schema.tauri.app/config/3'
  conf.identifier = `com.tauri.${conf.identifier.replace(/[^a-z0-9.]/gi, '-')}`
  conf.productName = `${conf.productName ?? name} (Electron)`
  conf.build.beforeDevCommand = ''
  conf.build.beforeBuildCommand = ''
  if (useVite) {
    conf.build.devUrl = `http://localhost:${PORT}`
  } else {
    delete conf.build.devUrl
  }
  // The CLI plugin is not registered by these apps, isolation needs a runtime
  // feature this one does not have, and the IPC transport is outside the CSP.
  delete conf.plugins
  delete conf.app.security.pattern
  conf.app.security.csp = null
  conf.bundle.active = false
  conf.bundle.icon =
    conf.bundle.icon
      ?.map((icon) => path.basename(icon))
      .filter((icon) => fs.existsSync(path.join(path.dirname(confPath), '..', '.icons', icon))) ??
    []
  conf.bundle.resources = { 'target/electron': 'runtime' }
  write(confPath, JSON.stringify(conf, null, 2) + '\n')
  // tauri-build refuses features the config does not allow, so follow the config.
  tauriFeatures = ['unstable', 'tray-icon', 'image-png', 'image-ico', 'devtools']
  if (conf.app?.security?.assetProtocol?.enable) tauriFeatures.push('protocol-asset')
  if (conf.app?.macOSPrivateApi) tauriFeatures.push('macos-private-api')
}

function patchRuntime(file) {
  const source = read(file)
    .replaceAll('tauri_runtime_wry::Wry::default()', 'tauri_runtime_electron::Electron::default()')
    // The examples live inside the Tauri workspace, so they point at the
    // config through a relative path that does not exist once staged.
    .replace(/generate_context!\(\s*"[^"]*"\s*\)/g, 'generate_context!()')
  if (!source.includes('tauri_runtime_electron::Electron')) {
    fail(`could not patch ${path.relative(repoRoot, file)} (upstream example changed?)`)
  }
  write(file, source)
}

function crateName() {
  return name.replace(/[^a-z0-9]+/gi, '_')
}

function manifest(rustSources, extraDeps, binPath) {
  const source = rustSources.map((file) => read(file)).join('\n')
  const deps = ['serde = { version = "1", features = ["derive"] }', 'serde_json = "1"', 'log = "0.4"']
  if (/\bhttp::/.test(source)) deps.push('http = "1"')
  if (/\bhttp_range::/.test(source)) deps.push('http-range = "0.1"')
  const repoPath = repoRoot.replaceAll('\\', '/')
  return `[package]
name = "${crateName()}"
version = "0.1.0"
description = "Tauri's ${name} example running on tauri-runtime-electron"
edition = "2021"

[[bin]]
name = "${crateName()}"
path = "${binPath}"

# The upstream examples switch on this to pick a runtime; this crate ignores it.
[features]
default = []
cef = []

[build-dependencies]
tauri-build = { version = "3.0.0-alpha.4", features = ["codegen"] }

[dependencies]
${deps.join('\n')}
${extraDeps.join('\n')}
tauri = { version = "=3.0.0-alpha.5", features = [${tauriFeatures.map((feature) => `"${feature}"`).join(', ')}] }
tauri-runtime-electron = { path = "${repoPath}" }
`
}

const BUILD_RS = `fn main() {
  tauri_build::try_build(tauri_build::Attributes::new()).expect("failed to run tauri-build");
}
`

function copyCrateIcons(crateDir) {
  // tauri-build looks for these next to the manifest when it makes the Windows
  // resource file / macOS bundle; the examples keep them one level up.
  for (const icon of ['icon.ico', 'icon.icns', '32x32.png', '128x128.png', '128x128@2x.png']) {
    const from = path.join(examplesDir, '.icons', icon)
    if (fs.existsSync(from)) copyTree(from, path.join(crateDir, icon))
  }
}

function stageFlat(dir, dst) {
  // `main.rs` at the example root: give it the crate manifest Tauri expects.
  const entries = fs.readdirSync(dir)
  for (const entry of entries) copyTree(path.join(dir, entry), path.join(dst, entry))
  const mainRs = path.join(dst, 'main.rs')
  patchRuntime(mainRs)
  stageConf(path.join(dst, 'tauri.conf.json'), false)
  copyCrateIcons(dst)
  write(path.join(dst, 'build.rs'), BUILD_RS)
  write(path.join(dst, 'Cargo.toml'), manifest([mainRs], [], 'main.rs'))
}

function stageApp(dir, dst) {
  for (const entry of fs.readdirSync(dir)) copyTree(path.join(dir, entry), path.join(dst, entry))
  const tauriDir = path.join(dst, 'src-tauri')
  const rustSources = ['src/lib.rs', 'src/main.rs']
    .map((file) => path.join(tauriDir, file))
    .filter((file) => fs.existsSync(file))
  if (rustSources.length === 0) fail('no src/lib.rs or src/main.rs in src-tauri')
  for (const file of rustSources) patchRuntime(file)

  // Standalone builds need tauri-build's default manifest, otherwise comctl32
  // v5 loads and TaskDialogIndirect cannot be resolved on startup.
  const buildRs = path.join(tauriDir, 'build.rs')
  if (fs.existsSync(buildRs)) {
    const source = read(buildRs)
      .replace(/use tauri_build::WindowsAttributes;\n/, '')
      .replace(/\s*\.windows_attributes\(WindowsAttributes::new_without_app_manifest\(\)\)/, '')
    if (source.includes('WindowsAttributes')) fail('could not patch src-tauri/build.rs')
    write(buildRs, source)
  }

  const sampleDir = path.join(tauriDir, 'tauri-plugin-sample')
  const extra = []
  if (fs.existsSync(sampleDir)) {
    const sampleManifest = path.join(sampleDir, 'Cargo.toml')
    write(
      sampleManifest,
      read(sampleManifest)
        .replace(
          'tauri = { workspace = true, default-features = false }',
          'tauri = { version = "=3.0.0-alpha.5", default-features = false }',
        )
        .replace(
          'tauri-plugin = { workspace = true, features = ["build"] }',
          'tauri-plugin = { version = "3.0.0-alpha.4", features = ["build"] }',
        ),
    )
    extra.push('tauri-plugin-sample = { path = "./tauri-plugin-sample/" }')
  }
  copyCrateIcons(tauriDir)
  stageConf(path.join(tauriDir, 'tauri.conf.json'), hasVite(dst))
  write(path.join(tauriDir, 'Cargo.toml'), manifest(rustSources, extra, 'src/main.rs'))
}

function hasVite(dir) {
  return fs.existsSync(path.join(dir, 'vite.config.js'))
}

function stage() {
  ensureExampleSource()
  const source = path.join(examplesDir, name)
  if (!fs.existsSync(source)) {
    listExamples()
    process.exit(1)
  }
  const dst = path.join(cacheRoot, name)
  if (flags.has('--clean')) fs.rmSync(dst, { recursive: true, force: true })
  fs.mkdirSync(dst, { recursive: true })
  console.log(`[example] staging ${path.relative(repoRoot, dst)}`)

  // Examples reference the shared icon set through `../.icons`.
  copyTree(path.join(examplesDir, '.icons'), path.join(cacheRoot, '.icons'))

  if (fs.existsSync(path.join(source, 'main.rs'))) stageFlat(source, dst)
  else stageApp(source, dst)
  return dst
}

function installDeps(dir) {
  if (!fs.existsSync(path.join(dir, 'package.json'))) return
  if (fs.existsSync(path.join(dir, 'node_modules'))) return
  console.log('[example] installing frontend dependencies')
  const result = spawnSync('npm install --no-audit --no-fund --prefer-offline', {
    cwd: dir,
    stdio: 'inherit',
    shell: true,
  })
  if (result.status !== 0) fail('npm install failed')
}

async function assertPortFree() {
  try {
    await fetch(`http://localhost:${PORT}/`, { signal: AbortSignal.timeout(1000) })
    fail(`port ${PORT} is already in use; stop whatever is listening there and retry`)
  } catch (error) {
    if (error?.name === 'AbortError') {
      fail(`port ${PORT} is not responding but seems occupied; free it and retry`)
    }
  }
}

function kill(proc) {
  if (!proc || !proc.pid || proc.exitCode !== null) return
  if (process.platform === 'win32') {
    spawnSync('taskkill', ['/pid', String(proc.pid), '/T', '/F'], { stdio: 'ignore' })
  } else {
    try {
      process.kill(-proc.pid, 'SIGTERM')
    } catch {
      try {
        process.kill(proc.pid, 'SIGTERM')
      } catch {}
    }
  }
}

function shutdown(code = 0) {
  if (shuttingDown) return
  shuttingDown = true
  kill(viteProc)
  kill(cargoProc)
  setTimeout(() => process.exit(code), process.platform === 'win32' ? 500 : 100)
}

function startVite(dir) {
  const viteCli = path.join(dir, 'node_modules', 'vite', 'bin', 'vite.js')
  if (!fs.existsSync(viteCli)) fail('frontend dependencies are missing; rerun without --no-install')
  viteProc = spawn(process.execPath, [viteCli, '--port', String(PORT), '--strictPort'], {
    cwd: dir,
    stdio: 'inherit',
    detached: process.platform !== 'win32',
  })
  viteProc.on('error', (error) => fail(`could not start vite: ${error.message}`))
  viteProc.on('exit', (code, signal) => {
    if (!shuttingDown) {
      console.error(`[example] vite exited (${signal ?? code})`)
      shutdown(code ?? 1)
    }
  })
}

async function waitForVite() {
  const deadline = Date.now() + 60_000
  while (Date.now() < deadline) {
    if (viteProc.exitCode !== null) fail('vite exited before the dev server came up')
    try {
      const response = await fetch(`http://localhost:${PORT}/`, { signal: AbortSignal.timeout(1000) })
      if (response.ok) return
    } catch {}
    await new Promise((resolve) => setTimeout(resolve, 250))
  }
  fail(`vite did not answer on port ${PORT} within 60s`)
}

function runApp(cwd) {
  console.log(`\n[example] Tauri ${name} example on the Electron runtime\n`)
  cargoProc = spawn('cargo', ['run'], {
    cwd,
    stdio: 'inherit',
    detached: process.platform !== 'win32',
  })
  cargoProc.on('error', (error) => fail(`could not start cargo: ${error.message}`))
  cargoProc.on('exit', (code, signal) => {
    if (!shuttingDown) {
      console.log(`[example] cargo run exited (${signal ?? code})`)
      shutdown(code ?? 0)
    }
  })
}

async function main() {
  if (flags.has('--list') || !name) {
    listExamples()
    return
  }
  const dst = stage()
  const vite = hasVite(dst)
  if (vite) {
    if (!flags.has('--no-install')) installDeps(dst)
    await assertPortFree()
    startVite(dst)
    await waitForVite()
  }
  runApp(vite ? path.join(dst, 'src-tauri') : dst)
}

process.on('SIGINT', () => shutdown(0))
process.on('SIGTERM', () => shutdown(0))
if (process.platform !== 'win32') process.on('SIGHUP', () => shutdown(0))
process.on('exit', () => {
  kill(viteProc)
  kill(cargoProc)
})

main().catch((error) => fail(error.message))
