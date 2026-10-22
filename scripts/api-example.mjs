#!/usr/bin/env node
// Boot Tauri's official `examples/api` app on the Electron runtime.
//
//   node scripts/api-example.mjs [--no-install] [--clean]
//
// The example is staged under .cache/api-example so the checkout stays clean.
// Vite runs on :1420, then `cargo run` builds the generated src-tauri crate.
// Ctrl+C stops cargo, the app and Vite.

import { spawn, spawnSync } from 'node:child_process'
import fs from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const exampleDir = path.join(repoRoot, 'source', 'tauri', 'examples', 'api')
const iconsDir = path.join(repoRoot, 'source', 'tauri', 'examples', '.icons')
const cacheDir = path.join(repoRoot, '.cache', 'api-example')
const tauriDir = path.join(cacheDir, 'src-tauri')
const PORT = 1420

const flags = new Set(process.argv.slice(2))
let viteProc = null
let cargoProc = null
let shuttingDown = false

function fail(message) {
  console.error(`\n[api-example] ${message}`)
  shuttingDown = true
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
  fs.rmSync(to, { recursive: true, force: true })
  fs.cpSync(from, to, { recursive: true })
}

function copyFile(from, to) {
  fs.mkdirSync(path.dirname(to), { recursive: true })
  fs.copyFileSync(from, to)
}

function ensureExampleSource() {
  if (fs.existsSync(path.join(exampleDir, 'package.json'))) return
  console.log('[api-example] source/tauri is missing; running `git submodule update --init source/tauri`')
  const result = spawnSync('git', ['submodule', 'update', '--init', 'source/tauri'], {
    cwd: repoRoot,
    stdio: 'inherit',
  })
  if (result.status !== 0 || !fs.existsSync(path.join(exampleDir, 'package.json'))) {
    fail(
      'could not check out source/tauri (are you offline?). ' +
        'Run `git submodule update --init source/tauri` and try again.',
    )
  }
}

function stageFrontend() {
  const entries = [
    'index.html',
    'public',
    'src',
    'svelte.config.js',
    'tsconfig.json',
    'unocss.config.js',
    'vite.config.js',
  ]
  for (const entry of entries) {
    copyTree(path.join(exampleDir, entry), path.join(cacheDir, entry))
  }
  copyTree(iconsDir, path.join(cacheDir, '.icons'))

  const pkgPath = path.join(cacheDir, 'package.json')
  copyFile(path.join(exampleDir, 'package.json'), pkgPath)
  const pkg = JSON.parse(read(pkgPath))
  // The example normally consumes the in-repo `packages/api` build; use the
  // published package with the matching IPC version instead.
  pkg.dependencies['@tauri-apps/api'] = '3.0.0-alpha.3'
  delete pkg.scripts.tauri
  write(pkgPath, JSON.stringify(pkg, null, 2) + '\n')

  const vitePath = path.join(cacheDir, 'vite.config.js')
  const vite = read(vitePath).replace(/allow:\s*\[[^\]]*\]/, "allow: ['.']")
  write(vitePath, vite)
}

function stageRust() {
  const upstream = path.join(exampleDir, 'src-tauri')
  for (const entry of ['src', 'capabilities', 'permissions', 'locales']) {
    copyTree(path.join(upstream, entry), path.join(tauriDir, entry))
  }
  // The upstream workspace strips the app manifest and re-adds a test-only one
  // (its build.rs even mentions STATUS_ENTRYPOINT_NOT_FOUND). Standalone builds
  // need tauri-build's default manifest, otherwise comctl32 v5 loads and
  // TaskDialogIndirect cannot be resolved on startup.
  const buildRsPath = path.join(tauriDir, 'build.rs')
  const buildRs = read(path.join(upstream, 'build.rs'))
    .replace(/use tauri_build::WindowsAttributes;\n/, '')
    .replace(/\s*\.windows_attributes\(WindowsAttributes::new_without_app_manifest\(\)\)/, '')
  if (buildRs.includes('WindowsAttributes')) {
    fail('could not patch src-tauri/build.rs (upstream example changed?)')
  }
  write(buildRsPath, buildRs)
  copyTree(path.join(upstream, 'tauri-plugin-sample'), path.join(tauriDir, 'tauri-plugin-sample'))

  // Swap only the runtime; the example's run(), commands, plugins and handlers stay.
  const libPath = path.join(tauriDir, 'src', 'lib.rs')
  const lib = read(libPath).replace(
    'tauri_runtime_wry::Wry::default()',
    'tauri_runtime_electron::Electron::default()',
  )
  if (!lib.includes('tauri_runtime_electron::Electron')) {
    fail('could not patch src-tauri/src/lib.rs (upstream example changed?)')
  }
  write(libPath, lib)

  // The example expects .icons two levels above src-tauri; we keep it inside the cache.
  const trayPath = path.join(tauriDir, 'src', 'tray.rs')
  write(trayPath, read(trayPath).replaceAll('../../.icons/', '../.icons/'))

  const sampleManifestPath = path.join(tauriDir, 'tauri-plugin-sample', 'Cargo.toml')
  const sampleManifest = read(sampleManifestPath)
    .replace(
      'tauri = { workspace = true, default-features = false }',
      'tauri = { version = "=3.0.0-alpha.5", default-features = false }',
    )
    .replace(
      'tauri-plugin = { workspace = true, features = ["build"] }',
      'tauri-plugin = { version = "3.0.0-alpha.4", features = ["build"] }',
    )
  if (sampleManifest.includes('workspace = true')) {
    fail('could not patch tauri-plugin-sample/Cargo.toml (upstream example changed?)')
  }
  write(sampleManifestPath, sampleManifest)

  const repoPath = repoRoot.replaceAll('\\', '/')
  write(
    path.join(tauriDir, 'Cargo.toml'),
    `[package]
name = "api"
version = "0.1.0"
description = "Tauri API example running on tauri-runtime-electron"
edition = "2021"

[lib]
name = "api_lib"
crate-type = ["staticlib", "cdylib", "rlib"]

[build-dependencies]
tauri-build = { version = "3.0.0-alpha.4", features = ["codegen"] }

[dependencies]
serde_json = "1"
serde = { version = "1", features = ["derive"] }
log = "0.4"
tauri-plugin-sample = { path = "./tauri-plugin-sample/" }
tauri-plugin-log = "3.0.0-alpha.1"
tauri = { version = "=3.0.0-alpha.5", features = ["unstable", "tray-icon", "image-png", "image-ico", "protocol-asset"] }
tauri-runtime-electron = { path = "${repoPath}" }
`,
  )

  const conf = JSON.parse(read(path.join(upstream, 'tauri.conf.json')))
  conf.$schema = 'https://schema.tauri.app/config/3'
  conf.identifier = 'com.tauri.api.electron-example'
  conf.build.devUrl = `http://localhost:${PORT}`
  conf.build.beforeDevCommand = ''
  conf.build.beforeBuildCommand = 'npm run build'
  delete conf.build.removeUnusedCommands
  delete conf.app.security.pattern // the Electron runtime does not implement isolation
  conf.app.security.csp = null // the Electron IPC transport is outside the upstream CSP
  delete conf.plugins // the CLI plugin is not registered by this app
  conf.bundle.active = false
  conf.bundle.icon = conf.bundle.icon.map((icon) => icon.replace('../../.icons/', '../.icons/'))
  conf.bundle.resources = { 'target/electron': 'runtime' }
  write(path.join(tauriDir, 'tauri.conf.json'), JSON.stringify(conf, null, 2) + '\n')
}

function stage() {
  ensureExampleSource()
  if (flags.has('--clean')) fs.rmSync(cacheDir, { recursive: true, force: true })
  fs.mkdirSync(cacheDir, { recursive: true })
  console.log(`[api-example] staging ${path.relative(repoRoot, cacheDir)}`)
  stageFrontend()
  stageRust()
}

function installDeps() {
  console.log('[api-example] installing frontend dependencies')
  const result = spawnSync('npm install --no-audit --no-fund --prefer-offline', {
    cwd: cacheDir,
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

function startVite(viteCli) {
  viteProc = spawn(process.execPath, [viteCli, '--port', String(PORT), '--strictPort'], {
    cwd: cacheDir,
    stdio: 'inherit',
    detached: process.platform !== 'win32',
  })
  viteProc.on('error', (error) => fail(`could not start vite: ${error.message}`))
  viteProc.on('exit', (code, signal) => {
    if (!shuttingDown) {
      console.error(`[api-example] vite exited (${signal ?? code})`)
      shutdown(code ?? 1)
    }
  })
}

async function waitForVite() {
  const deadline = Date.now() + 60_000
  while (Date.now() < deadline) {
    if (viteProc.exitCode !== null) fail('vite exited before the dev server came up')
    try {
      const response = await fetch(`http://localhost:${PORT}/`, {
        signal: AbortSignal.timeout(1000),
      })
      if (response.ok) return
    } catch {}
    await new Promise((resolve) => setTimeout(resolve, 250))
  }
  fail(`vite did not answer on port ${PORT} within 60s`)
}

function runApp() {
  console.log(`\n[api-example] Tauri API example — http://localhost:${PORT}\n`)
  cargoProc = spawn('cargo', ['run'], {
    cwd: tauriDir,
    stdio: 'inherit',
    detached: process.platform !== 'win32',
  })
  cargoProc.on('error', (error) => fail(`could not start cargo: ${error.message}`))
  cargoProc.on('exit', (code, signal) => {
    if (!shuttingDown) {
      console.log(`[api-example] cargo run exited (${signal ?? code})`)
      shutdown(code ?? 0)
    }
  })
}

async function main() {
  stage()
  if (!flags.has('--no-install')) installDeps()

  const viteCli = path.join(cacheDir, 'node_modules', 'vite', 'bin', 'vite.js')
  if (!fs.existsSync(viteCli)) {
    fail('frontend dependencies are missing; rerun without --no-install to install them')
  }

  await assertPortFree()
  startVite(viteCli)
  await waitForVite()
  runApp()
}

process.on('SIGINT', () => shutdown(0))
process.on('SIGTERM', () => shutdown(0))
if (process.platform !== 'win32') process.on('SIGHUP', () => shutdown(0))
process.on('exit', () => {
  kill(viteProc)
  kill(cargoProc)
})

main().catch((error) => fail(error.message))
