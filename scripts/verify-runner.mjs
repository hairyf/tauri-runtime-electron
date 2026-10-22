// Offline runner regression checks; no staging, builds, downloads, or live-app proof.
import assert from 'node:assert/strict'
import fs from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import vm from 'node:vm'
import { appManifest, configure, parseArgs } from './example.mjs'

const read = (relative) => fs.readFileSync(new URL(`../source/tauri/examples/${relative}`, import.meta.url), 'utf8').replaceAll('\r\n', '\n')
const json = (relative) => JSON.parse(read(relative))
const allFlags = ['--list', '--clean', '--no-install', '--build-only', '--debug']
assert.deepEqual(parseArgs([]), { name: undefined, flags: new Set(), appArgs: [] })
assert.deepEqual(parseArgs(['api', ...allFlags]), { name: 'api', flags: new Set(allFlags), appArgs: [] })
for (const name of ['../api', '..\\api', '/api', 'C:\\api', 'api/main.rs', 'api\\main.rs', '.', 'api..']) {
  assert.throws(() => parseArgs([name]), /not a path|expected one example/, name)
}
for (const flag of ['--unknown', '-x', '--build-only=true']) {
  assert.throws(() => parseArgs(['api', flag]), /unknown option/, flag)
}
assert.throws(() => parseArgs(['api', 'resources']), /expected one example name/)
const appArgs = ['C:\\hello world.txt', '--clean', '--unknown', '../api', '--', '文件.txt']
assert.deepEqual(parseArgs(['file-associations', '--debug', '--', ...appArgs]), {
  name: 'file-associations', flags: new Set(['--debug']), appArgs,
})

for (const file of ['isolation/tauri.conf.json', 'api/src-tauri/tauri.conf.json', 'resources/src-tauri/tauri.conf.json', 'file-associations/src-tauri/tauri.conf.json']) {
  const original = json(file)
  // Exercise inline permission allow/deny scopes with the official capability fixture.
  original.app.security.capabilities = ['main', 'run-app'].map((name) => json(`api/src-tauri/capabilities/${name}.json`))
  for (const useVite of [false, true]) {
    const configured = configure(structuredClone(original), useVite)
    const { devUrl, ...build } = original.build
    assert.deepEqual(configured.app, original.app, `${file}: isolation, CSP and permission scopes`)
    assert.deepEqual(configured.plugins, original.plugins, `${file}: plugin configuration`)
    assert.deepEqual(configured.bundle, { ...original.bundle, active: false }, `${file}: resources and targets`)
    assert.deepEqual(configured.build, {
      ...build, beforeDevCommand: '', beforeBuildCommand: '',
      ...(useVite ? { devUrl: 'http://localhost:1420' } : {}),
    }, `${file}: frontend configuration`)
    assert.equal(configured.identifier, `com.tauri.${original.identifier.replace(/[^a-z0-9.]/gi, '-')}`)
    assert.equal(configured.productName, `${original.productName ?? configured.identifier} (Electron)`)
    assert.equal(configured.$schema, 'https://schema.tauri.app/config/3')
  }
}

const runtimePath = fileURLToPath(new URL('../', import.meta.url)).replaceAll('\\', '/').replace(/\/$/, '')
const features = ['unstable', 'tray-icon', 'image-png', 'image-ico', 'devtools', 'protocol-asset', 'isolation']
const table = (manifest, heading) => manifest.split(/(?=^\[)/m).find((section) => section.startsWith(`${heading}\n`))
for (const name of ['api', 'resources', 'file-associations']) {
  const original = read(`${name}/src-tauri/Cargo.toml`)
  const output = appManifest(original, 'runner_check', features)
  assert.equal(table(output, '[package]'), table(original, '[package]').replace(/^name\s*=.*$/m, 'name = "runner_check"'))
  for (const section of original.split(/(?=^\[)/m).filter((section) => /^\[(lib|target\.)/.test(section))) {
    assert(output.includes(section), `${name}: lib and target tables`)
  }
  for (const line of original.split('\n').filter((line) => /^tauri-plugin-|^(prod|automation)\s*=/.test(line))) {
    assert(output.includes(line), `${name}: plugin names and unrelated features`)
  }
  assert.doesNotMatch(output, /workspace\s*=\s*true|tauri-runtime-(wry|cef)/)
  assert(output.includes(`tauri-runtime-electron = { path = ${JSON.stringify(runtimePath)} }`))
  assert(output.includes(`tauri = { version = "=3.0.0-alpha.5", features = ${JSON.stringify(features)} }`))
  assert(output.includes('tauri-build = { version = "=3.0.0-alpha.4", features = ["codegen","isolation"] }'))
  for (const feature of ['default', 'wry', 'cef']) assert(output.includes(`${feature} = []`))
  assert(output.endsWith('\n[[bin]]\nname = "runner_check"\npath = "src/main.rs"\n'))
}
assert(appManifest(read('resources/src-tauri/Cargo.toml'), 'resources', []).includes('features = ["codegen"]'))
// Exercise the real fixture helper with offline curl/file responses, never real media or app proof.
const runner = fs.readFileSync(new URL('./example.mjs', import.meta.url), 'utf8')
const prepareVideo = runner.match(/^function prepareVideo\([^]*?^\}/m)?.[0]
assert(prepareVideo, 'missing streaming fixture helper')
const videoFile = path.join('streaming-fixture', 'streaming_example_test_video.mp4')
const upstreamVideo = read('streaming/main.rs').match(/"(http:\/\/commondatastorage[^\"]+\.mp4)"/)?.[1]
assert(upstreamVideo, 'upstream streaming fixture changed')
const sources = [upstreamVideo.replace('http:', 'https:'), 'https://media.w3.org/2010/05/bunny/movie.mp4']
const mp4 = Buffer.from('000000186674797069736f6d00000200', 'hex')
const html = Buffer.from('<html>access denied</html>')
for (const { cached, attempts, expected, failed } of [
  { cached: mp4, attempts: [], expected: mp4 },
  { attempts: [{ status: 0, bytes: mp4 }], expected: mp4 },
  { cached: html, attempts: [{ status: 22, bytes: html }, { status: 0, bytes: mp4 }], expected: mp4 },
  { attempts: [{ status: 0, bytes: html }, { status: 0, bytes: mp4 }], expected: mp4 },
  { attempts: [{ error: new Error('curl missing') }, { status: 0, bytes: mp4 }], expected: mp4 },
  { cached: html, attempts: [{ status: 22, bytes: html }, { status: 0, bytes: Buffer.alloc(0) }], expected: html, failed: true },
]) {
  const files = new Map(cached ? [[videoFile, cached]] : [])
  let downloads = 0
  const sandbox = {
    name: 'streaming', path, Buffer, console: { log() {} },
    read(file) { assert.equal(file, path.join('streaming-fixture', 'main.rs')); return read('streaming/main.rs') },
    fail(message) { throw new Error(message) },
    fs: {
      existsSync: (file) => files.has(file), openSync: (file) => file, closeSync() {},
      readSync: (file, header) => files.get(file).copy(header),
      rmSync: (file) => files.delete(file),
      renameSync(from, to) {
        assert.equal(from, videoFile + '.part')
        assert.equal(to, videoFile)
        assert.equal(files.get(from).toString('ascii', 4, 8), 'ftyp', 'validate before atomic rename')
        files.set(to, files.get(from))
        files.delete(from)
      },
    },
    spawnSync(command, args, options) {
      assert.equal(command, 'curl')
      assert.deepEqual(Array.from(args), ['--fail', '--location', '--retry', '2', '--max-time', '180', '--output', videoFile + '.part', sources[downloads]])
      assert.equal(options.stdio, 'inherit')
      assert.equal(files.has(videoFile + '.part'), false, 'remove failed attempt before fallback')
      const attempt = attempts[downloads++]
      assert(attempt, 'unexpected fixture download')
      if (attempt.bytes) files.set(videoFile + '.part', attempt.bytes)
      return attempt
    },
  }
  vm.createContext(sandbox)
  vm.runInContext(prepareVideo, sandbox)
  if (failed) assert.throws(() => sandbox.prepareVideo('streaming-fixture'), /download failed|not an MP4/)
  else sandbox.prepareVideo('streaming-fixture')
  assert.equal(downloads, attempts.length)
  assert.deepEqual(files.get(videoFile), expected)
  assert.equal(files.has(videoFile + '.part'), false, 'never retain a partial fixture')
  sandbox.name = 'api'
  sandbox.prepareVideo('streaming-fixture')
  assert.equal(downloads, attempts.length, 'only streaming downloads a video')
}
console.log('ok: offline runner arguments, configuration preservation, manifest runtime replacement and streaming fixture fallback (no live-app proof)')
