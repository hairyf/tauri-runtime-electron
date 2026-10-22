#!/usr/bin/env node
// Run any of Tauri's official examples on the Electron runtime.
//
//   pnpm example <name>          # or: node scripts/example.mjs <name>
//   pnpm example --list
//   pnpm example <name> --build-only
//   pnpm example file-associations -- <file paths>
//
// Each example is staged under .cache/examples/<name>, its runtime is swapped
// for this crate, and the built binary starts it. Examples with a Vite frontend get
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

export function parseArgs(args) {
  const separator = args.indexOf('--')
  const options = separator < 0 ? args : args.slice(0, separator)
  const flags = new Set(options.filter((arg) => arg.startsWith('-')))
  for (const flag of flags) {
    if (!['--list', '--clean', '--no-install', '--build-only', '--debug'].includes(flag)) {
      throw new Error(`unknown option ${flag}; put app arguments after --`)
    }
  }
  const names = options.filter((arg) => !arg.startsWith('-'))
  if (names.length > 1) throw new Error('expected one example name; put app arguments after --')
  if (names[0] && !/^[a-z0-9]+(?:-[a-z0-9]+)*$/.test(names[0])) {
    throw new Error('example name must be a name from --list, not a path')
  }
  return { name: names[0], flags, appArgs: separator < 0 ? [] : args.slice(separator + 1) }
}

let flags = new Set()
let name
let appArgs = []

let viteProc = null
let cargoProc = null
let shuttingDown = false
let tauriFeatures = ['unstable', 'tray-icon', 'image-png', 'image-ico', 'devtools']
let exampleResources = []

export function exampleNames() {
  return fs
    .readdirSync(examplesDir, { withFileTypes: true })
    .filter((entry) => entry.isDirectory() && !entry.isSymbolicLink())
    .map((entry) => entry.name)
    .filter((entry) => entry !== 'cef')
    .filter((entry) => {
      const dir = path.join(examplesDir, entry)
      return (
        fs.existsSync(path.join(dir, 'src-tauri', 'Cargo.toml')) ||
        fs.existsSync(path.join(dir, 'main.rs'))
      )
    })
    .sort()
}

function listExamples() {
  console.log(`usage: pnpm example <name> [--build-only] [-- app arguments]\n\nrunnable examples:\n  ${exampleNames().join('\n  ')}\n\ncef: CEF-specific APIs cannot be swapped to Electron`)
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

export function configure(conf, useVite) {
  conf.$schema = 'https://schema.tauri.app/config/3'
  conf.identifier = `com.tauri.${conf.identifier.replace(/[^a-z0-9.]/gi, '-')}`
  conf.productName = `${conf.productName ?? conf.identifier} (Electron)`
  conf.build ??= {}
  conf.build.beforeDevCommand = ''
  conf.build.beforeBuildCommand = ''
  if (useVite) conf.build.devUrl = `http://localhost:${PORT}`
  else delete conf.build.devUrl
  conf.bundle ??= {}
  conf.bundle.active = false
  return conf
}

function stageConf(confPath, useVite) {
  const conf = configure(JSON.parse(read(confPath)), useVite)
  conf.bundle.icon = (conf.bundle.icon ?? [])
    .map((icon) => path.basename(icon))
    .filter((icon) => fs.existsSync(path.join(path.dirname(confPath), icon)))
  // Keep security policy and resource targets exactly as the example declared.
  // The runtime build script already stages Electron into the shared target.
  exampleResources = conf.bundle.resources ?? []
  write(confPath, JSON.stringify(conf, null, 2) + '\n')
  // tauri-build checks both dependencies against the security configuration.
  tauriFeatures = ['unstable', 'tray-icon', 'image-png', 'image-ico', 'devtools']
  if (conf.app?.security?.assetProtocol?.enable) tauriFeatures.push('protocol-asset')
  if (conf.app?.security?.pattern?.use === 'isolation') tauriFeatures.push('isolation')
}

export function replaceRuntime(original) {
  // Tauri alpha omits these builder registrations from RuntimeInitArgs.
  const schemes = [...original.matchAll(/\.register_(?:asynchronous_)?uri_scheme_protocol\(\s*"([a-zA-Z][a-zA-Z0-9+.-]*)"/g)]
    .map((match) => match[1])
  const runtime = 'tauri_runtime_electron::Electron::default()' + [...new Set(schemes)]
    .map((scheme) => `.custom_scheme(${JSON.stringify(scheme)})`).join('')
  return original.replaceAll('tauri_runtime_wry::Wry::default()', runtime)
}

// Keep each delayed Window action bound to the window selected at click time.
export function patchWindowTimers(source) {
  return source.replace(/(^  (?:async )?function (?:hide|disable|minimize)\(\) \{)([^]*?)(^  \})/gm,
    (_, head, body, tail) => `${head}\n    const target = selectedWebview${body
      .replaceAll('selectedWebview', 'target')
      .replace('setTimeout(target.unminimize, 2000)', 'setTimeout(() => target.unminimize(), 2000)')}${tail}`)
}

function patchRuntime(file) {
  const original = read(file)
  // `main.rs` frequently only calls into `lib.rs`, which is where the builder is.
  if (!original.includes('tauri_runtime_wry')) return
  const source = replaceRuntime(original)
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

export function appManifest(original, exampleName, features) {
  const tauri = `tauri = { version = "=3.0.0-alpha.5", features = ${JSON.stringify(features)} }`
  const build = `tauri-build = { version = "=3.0.0-alpha.4", features = ${JSON.stringify(['codegen', ...features.filter((feature) => feature === 'isolation')])} }`
  const runtime = `tauri-runtime-electron = { path = ${JSON.stringify(repoRoot.replaceAll('\\', '/'))} }`
  // Keep upstream lib names, plugin dependencies, target tables and cfg features;
  // only workspace/runtime dependencies need to become standalone dependencies.
  return original.split(/(?=^\[[^\n]+\])/m).map((section) => {
    const heading = section.match(/^\[([^\n]+)\]/)?.[1]
    if (/^(?:dev-)?dependencies\.tauri(?:-runtime-(?:wry|cef))?$/.test(heading ?? '')) return ''
    if (heading === 'package') return section.replace(/^name\s*=.*$/m, `name = ${JSON.stringify(exampleName)}`)
    if (heading === 'build-dependencies') {
      return section.replace(/^tauri-build\s*=\s*\{[^}]*\}/m, build)
    }
    if (heading === 'dependencies') {
      return section.replace(/^tauri(?:-runtime-(?:wry|cef))?\s*=\s*\{[^}]*\}[^\S\n]*\n?/gm, '')
        .replace('[dependencies]', `[dependencies]\n${tauri}\n${runtime}`)
    }
    if (heading === 'features') {
      return section.replace(/^default\s*=.*$/m, 'default = []')
        .replace(/^(wry|cef)\s*=.*$/gm, '$1 = []')
    }
    return section
  }).join('') + `\n[[bin]]\nname = ${JSON.stringify(exampleName)}\npath = "src/main.rs"\n`
}

function manifest(rustSources, extraDeps, binPath) {
  const source = rustSources.map((file) => read(file)).join('\n')
  const deps = [
    'serde = { version = "1", features = ["derive"] }',
    'serde_json = "1"',
    'log = "0.4"',
    'tauri-utils = "~3.0.0-alpha.4"',
  ]
  if (/\bhttp::/.test(source)) deps.push('http = "1"')
  if (/\bhttp_range::/.test(source)) deps.push('http-range = "0.1"')
  if (/\bpercent_encoding::/.test(source)) deps.push('percent-encoding = "2"')
  if (/\bgetrandom::/.test(source)) deps.push('getrandom = "0.3"')
  if (/\burl::/.test(source)) deps.push('url = "2"')
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
tauri-build = { version = "=3.0.0-alpha.4", features = ${JSON.stringify(['codegen', ...tauriFeatures.filter((feature) => feature === 'isolation')])} }

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
  // tauri-build looks for these next to the manifest (and under `icons/`) when
  // it makes the Windows resource file or the macOS bundle; the examples keep
  // them one level up in `examples/.icons`.
  for (const icon of ['icon.ico', 'icon.icns', '32x32.png', '128x128.png', '128x128@2x.png']) {
    const from = path.join(examplesDir, '.icons', icon)
    if (!fs.existsSync(from)) continue
    copyTree(from, path.join(crateDir, icon))
    copyTree(from, path.join(crateDir, 'icons', icon))
  }
}

function stageFlat(dir, dst) {
  // `main.rs` at the example root: give it the crate manifest Tauri expects.
  const entries = fs.readdirSync(dir)
  for (const entry of entries) copyTree(path.join(dir, entry), path.join(dst, entry))
  const mainRs = path.join(dst, 'main.rs')
  patchRuntime(mainRs)
  copyCrateIcons(dst)
  stageConf(path.join(dst, 'tauri.conf.json'), false)
  write(path.join(dst, 'build.rs'), BUILD_RS)
  write(path.join(dst, 'Cargo.toml'), manifest([mainRs], [], 'main.rs'))
  return dst
}

function stageApp(dir, dst) {
  for (const entry of fs.readdirSync(dir)) copyTree(path.join(dir, entry), path.join(dst, entry))
  const windowView = path.join(dst, 'src', 'views', 'Window.svelte')
  if (name === 'api' && fs.existsSync(windowView)) write(windowView, patchWindowTimers(read(windowView)))
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
  }
  copyCrateIcons(tauriDir)
  stageConf(path.join(tauriDir, 'tauri.conf.json'), hasVite(dst))
  const cargo = path.join(tauriDir, 'Cargo.toml')
  write(cargo, appManifest(read(cargo), crateName(), tauriFeatures))
  const packageFile = path.join(dst, 'package.json')
  if (fs.existsSync(packageFile)) {
    const pkg = JSON.parse(read(packageFile))
    if (pkg.dependencies?.['@tauri-apps/api']) {
      pkg.dependencies['@tauri-apps/api'] = '3.0.0-alpha.3'
      write(packageFile, JSON.stringify(pkg, null, 2) + '\n')
    }
  }
  return tauriDir
}

function hasVite(dir) {
  return fs.existsSync(path.join(dir, 'vite.config.js'))
}

function stage() {
  ensureExampleSource()
  if (name === 'cef') fail('cef uses CEF-specific extension APIs and cannot be swapped to Electron')
  if (!exampleNames().includes(name)) {
    listExamples()
    fail(`unknown example ${name}`)
  }
  const source = path.join(examplesDir, name)
  const dst = path.resolve(cacheRoot, name)
  if (path.dirname(dst) !== cacheRoot || fs.lstatSync(source).isSymbolicLink()) {
    fail('example paths must stay inside the worktree')
  }
  if (fs.existsSync(dst) && fs.lstatSync(dst).isSymbolicLink()) fail('staging directory must not be a symlink')
  if (flags.has('--clean')) fs.rmSync(dst, { recursive: true, force: true })
  fs.mkdirSync(dst, { recursive: true })
  console.log(`[example] staging ${path.relative(repoRoot, dst)}`)

  // Examples reference the shared icon set through `../.icons`.
  copyTree(path.join(examplesDir, '.icons'), path.join(cacheRoot, '.icons'))

  if (fs.existsSync(path.join(source, 'main.rs'))) return { crate: stageFlat(source, dst), frontend: dst }
  return { crate: stageApp(source, dst), frontend: dst }
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

function stageResources(crate, binDir) {
  // `cargo run` leaves bundle resources alone (the CLI copies them), so put them
  // beside the binary the way `tauri dev` would.
  for (const pattern of exampleResources) {
    const relative = pattern.replace(/[*?].*$/, '').replace(/\/+$/, '')
    const from = path.join(crate, relative)
    if (!fs.existsSync(from)) continue
    const to = path.join(binDir, relative)
    if (fs.statSync(from).isDirectory()) {
      copyTree(from, to)
    } else {
      fs.mkdirSync(path.dirname(to), { recursive: true })
      fs.copyFileSync(from, to)
    }
  }
}

function prepareVideo(crate) {
  if (name !== 'streaming') return
  const file = path.join(crate, 'streaming_example_test_video.mp4')
  const valid = (candidate) => {
    if (!fs.existsSync(candidate)) return false
    const fd = fs.openSync(candidate, 'r')
    try {
      const header = Buffer.alloc(12)
      return fs.readSync(fd, header) === 12 && header.toString('ascii', 4, 8) === 'ftyp'
    } finally { fs.closeSync(fd) }
  }
  if (valid(file)) return
  // Keep upstream's Big Buck Bunny film; W3 hosts the full film if Google denies access.
  const url = read(path.join(crate, 'main.rs')).match(/"(http:\/\/commondatastorage[^\"]+\.mp4)"/)?.[1]
  if (!url) fail('upstream streaming video URL changed')
  const temp = file + '.part'
  for (const source of [url.replace('http:', 'https:'), 'https://media.w3.org/2010/05/bunny/movie.mp4']) {
    console.log(`[example] downloading streaming fixture ${source}`)
    try {
      const result = spawnSync('curl', ['--fail', '--location', '--retry', '2', '--max-time', '180', '--output', temp, source], { stdio: 'inherit' })
      if (result.error || result.status !== 0 || !valid(temp)) continue
      fs.renameSync(temp, file)
      return
    } finally { fs.rmSync(temp, { force: true }) }
  }
  fail('streaming fixture download failed or is not an MP4 file')
}

function runApp(crate) {
  console.log(`\n[example] Tauri ${name} example on the Electron runtime\n`)
  prepareVideo(crate)
  // One target directory for every example: dependencies are compiled once
  // instead of once per example (a full tauri build is ~2 GB).
  const targetDir = path.join(cacheRoot, 'target')
  const build = spawnSync('cargo', ['build'], {
    cwd: crate,
    stdio: 'inherit',
    env: { ...process.env, CARGO_TARGET_DIR: targetDir },
  })
  if (build.status !== 0) fail('cargo build failed')
  const binDir = path.join(targetDir, 'debug')
  stageResources(crate, binDir)
  const binary = path.join(binDir, crateName() + (process.platform === 'win32' ? '.exe' : ''))
  if (flags.has('--build-only')) {
    console.log(`[example] built ${binary}`)
    return
  }
  cargoProc = spawn(binary, appArgs, {
    cwd: crate,
    stdio: 'inherit',
    detached: process.platform !== 'win32',
  })
  cargoProc.on('error', (error) => fail(`could not start the app: ${error.message}`))
  cargoProc.on('exit', (code, signal) => {
    if (!shuttingDown) {
      console.log(`[example] app exited (${signal ?? code})`)
      shutdown(code ?? 0)
    }
  })
}

async function main() {
  ;({ flags, name, appArgs } = parseArgs(process.argv.slice(2)))
  if (flags.has('--list') || !name) {
    listExamples()
    return
  }
  const { crate, frontend } = stage()
  if (!flags.has('--build-only') && hasVite(frontend)) {
    if (!flags.has('--no-install')) installDeps(frontend)
    await assertPortFree()
    startVite(frontend)
    await waitForVite()
  }
  runApp(crate)
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  process.on('SIGINT', () => shutdown(0))
  process.on('SIGTERM', () => shutdown(0))
  if (process.platform !== 'win32') process.on('SIGHUP', () => shutdown(0))
  process.on('exit', () => {
    kill(viteProc)
    kill(cargoProc)
  })
  main().catch((error) => {
    if (flags.has('--debug')) console.error(error)
    fail(error.message)
  })
}
