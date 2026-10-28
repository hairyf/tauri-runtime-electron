// Exercise evidence failures without PowerShell, native input, files removed, or GUI.
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { EventEmitter } from 'node:events'
import { win32 as path } from 'node:path'
import vm from 'node:vm'
import './runtime-evidence.test.mjs'

const source = readFileSync(new URL('./examples.mjs', import.meta.url), 'utf8').replaceAll('\r\n', '\n')
const nativeStart = source.indexOf('function nativeBlocked(')
const nativeEnd = source.indexOf('\nfunction nativeWindow(')
assert.ok(nativeStart >= 0 && nativeEnd > nativeStart)
const writes = []
let killed = false
const context = vm.createContext({
  assert, Buffer, Set, Date, JSON, Error, Object, String, path,
  root: 'D:\\repo', process: { platform: 'win32' }, existsSync: () => true,
  writeFileSync: (file, data) => {
    assert.ok(data.endsWith('\n'), 'JSON evidence ends with a real newline')
    writes.push({ file, observations: JSON.parse(data) })
  },
  clearTimeout() {},
  setTimeout(callback, delay) {
    assert.equal(delay, 60_000)
    queueMicrotask(callback)
    return 1
  },
  spawn(_command, args) {
    const script = Buffer.from(args.at(-1), 'base64').toString('utf16le')
    assert.ok(script.includes(path.join('D:\\repo', 'tests/checks/example-native.ps1')), 'native helper uses its relocated path')
    const child = new EventEmitter()
    child.stdout = new EventEmitter()
    child.stderr = new EventEmitter()
    child.kill = () => { killed = true }
    queueMicrotask(() => {
      child.stderr.emit('data', '[native-phase] owner lookup done\r\n[native-phase] Add-Type start\r\n')
      child.stdout.emit('data', 'partial-json')
    })
    return child
  },
})
vm.runInContext(source.slice(nativeStart, nativeEnd) + '\nthis.runNative = native', context)
const directory = 'D:\\repo\\target\\example-checks\\transparent'
await assert.rejects(context.runNative({ child: { pid: 7 }, directory }, 'List'), error => {
  assert.equal(error.blocked, true)
  assert.match(error.message, /List: native helper timed out after 60000ms; last phase: Add-Type start/)
  assert.match(error.message, /stderr: \[native-phase\] owner lookup done/)
  assert.match(error.message, /stdout: partial-json/)
  return true
})
assert.equal(killed, true)
assert.equal(writes.length, 1)
assert.equal(writes[0].file, path.join(directory, 'native-observations.json'))
const [failure] = writes[0].observations
assert.equal(failure.action, 'List')
assert.equal(failure.phase, 'Add-Type start')
assert.equal(failure.raw, 'partial-json')
assert.equal(failure.ok, false)
assert.match(failure.stderr, /Add-Type start/)

const checkStart = source.indexOf('async function check(name) {')
const cleanupStart = source.indexOf('  assert.equal(path.dirname(path.resolve(directory))', checkStart)
const cleanupEnd = source.indexOf("  const log = path.join(directory, 'run.log')", cleanupStart)
assert.ok(checkStart >= 0 && cleanupStart > checkStart && cleanupEnd > cleanupStart)
const cleanup = source.slice(cleanupStart, cleanupEnd)
const removed = []
const cleanupContext = {
  assert, path, root: 'D:\\repo', directory, realpathSync: value => value,
  readdirSync: () => ['old.png', 'run.log', 'result.json', 'native-observations.json', 'notes.txt', 'nested.png', 'linked.png']
    .map(name => ({ name, isFile: () => !['nested.png', 'linked.png'].includes(name) })),
  rmSync: file => removed.push(file),
}
vm.runInNewContext(cleanup, cleanupContext)
assert.deepEqual(removed, ['old.png', 'run.log', 'result.json', 'native-observations.json'].map(name => path.join(directory, name)))
cleanupContext.realpathSync = () => 'D:\\elsewhere'
assert.throws(() => vm.runInNewContext(cleanup, cleanupContext), /redirected directory/)
cleanupContext.directory = 'D:\\repo\\other'
assert.throws(() => vm.runInNewContext(cleanup, cleanupContext), /inside target\/example-checks/)
assert.equal(removed.length, 4)

const sessionStart = source.indexOf('  async function session(target) {')
const sessionEnd = source.indexOf('  const test = {', sessionStart)
assert.ok(sessionStart >= 0 && sessionEnd > sessionStart)
let connects = 0
await vm.runInNewContext(`${source.slice(sessionStart, sessionEnd)}\n(async () => { await session({ id: 'page' }); await session({ id: 'page' }) })()`, {
  sessions: new Map(), Session: { connect: async () => { connects++; return {} } },
})
assert.equal(connects, 1, 'reuse the CDP connection for each target')

const checksStart = source.indexOf('const checks = {')
const checksEnd = source.indexOf('\nasync function freePort()', checksStart)
assert.ok(checksStart >= 0 && checksEnd > checksStart)
const checks = vm.runInNewContext(`${source.slice(checksStart, checksEnd)}\nchecks`, {
  assert, transparentChecks() {}, until: async (read, accept, timeout) => {
    assert.equal(timeout, 120_000)
    const value = await read()
    assert.equal(accept(value), true, 'dashboard must complete before assertions')
    return value
  },
})
assert.deepEqual(Object.keys(checks).sort(), ['coverage', 'transparent'])
async function runCoverage(coverage) {
  const shots = []
  const passes = []
  const page = { evaluate: async expression => {
    assert.equal(expression, 'window.__coverage')
    return coverage
  } }
  const test = {
    page: async label => { assert.equal(label, 'main'); return page },
    shot: async (target, key) => { assert.equal(target, page); shots.push(key) },
    pass: message => passes.push(message), limitations: [],
  }
  return { test, shots, passes, run: checks.coverage(test) }
}
const passed = await runCoverage({ done: true, results: [{ id: 'ipc', status: 'pass', detail: 'round trip' }, { id: 'native', status: 'skip', detail: 'manual' }] })
await passed.run
assert.equal(passed.test.coverage.results.length, 2)
assert.deepEqual(passed.shots, ['01-coverage-complete'])
assert.deepEqual(passed.passes, ['ipc: round trip'])
assert.deepEqual(passed.test.limitations, ['native: manual'])
const failed = await runCoverage({ done: true, results: [{ id: 'ipc', status: 'fail', detail: 'wrong response' }] })
await assert.rejects(failed.run, /dashboard failures: ipc: wrong response/)
assert.deepEqual(failed.shots, ['01-coverage-complete'], 'retain completed evidence before rejecting failures')
assert.equal(failed.test.coverage.results[0].status, 'fail')
const startupFailed = await runCoverage({ done: true, error: 'startup failed', results: [] })
await assert.rejects(startupFailed.run, /dashboard error: startup failed/)
assert.deepEqual(startupFailed.shots, ['01-coverage-complete'])
const notApplicable = await runCoverage({ done: true, results: [{ id: 'native', status: 'na', detail: 'platform restriction' }] })
await notApplicable.run
assert.deepEqual(notApplicable.test.limitations, ['native: platform restriction'])
const empty = await runCoverage({ done: true, results: [] })
await assert.rejects(empty.run, /nonempty results/)
const malformed = await runCoverage({ done: true, results: [{ id: 'ipc', status: 'unknown' }] })
await assert.rejects(malformed.run, /unknown dashboard status/)

const diagnosticsStart = source.indexOf("    if (name === 'coverage') {\n      // A timeout/closed target")
const diagnosticsEnd = source.indexOf('    for (const page of sessions.values()) {', diagnosticsStart)
assert.ok(diagnosticsStart >= 0 && diagnosticsEnd > diagnosticsStart)
const report = { error: 'original dashboard timeout' }
await vm.runInNewContext(`(async () => { ${source.slice(diagnosticsStart, diagnosticsEnd)} })()`, {
  name: 'coverage', report, Promise,
  sessions: new Map([
    ['live', { target: { url: 'tauri://localhost/' }, evaluate: async () => ({ done: false, results: [{ id: 'ipc', status: 'fail' }] }) }],
    ['closed', { target: { url: 'tauri://localhost/closed' }, evaluate: async () => { throw new Error('target closed') } }],
  ]),
})
assert.equal(report.error, 'original dashboard timeout')
assert.equal(report.coverageDiagnostics[0].coverage.done, false)
assert.equal(report.coverageDiagnostics[0].coverage.results[0].status, 'fail')
assert.equal(report.coverageDiagnostics[1].coverage.diagnosticError, 'target closed')
assert.ok(source.includes("requested.length ? requested : ['coverage']"), 'default check is the dashboard')
const launchEnd = source.indexOf('  const sessions = new Map()', checkStart)
assert.ok(launchEnd > checkStart)
assert.ok(source.includes("import { createRequire } from 'node:module'"))
for (const platform of ['win32', 'linux']) {
  for (const name of ['coverage', 'transparent']) {
    const cwd = path.join('D:\\repo', name === 'transparent' ? 'tests/assets/transparent' : 'example')
    const cli = 'D:\\repo\\node_modules\\@tauri-apps\\cli\\tauri.js'
    const child = { pid: 7 }
    let resolved = false
    let launched = false
    const run = vm.runInNewContext(`${source.slice(checkStart, launchEnd)}\n  return child\n}\ncheck`, {
      assert, path, root: 'D:\\repo', artifacts: 'D:\\repo\\target\\example-checks',
      process: { platform, execPath: 'C:\\Program Files\\nodejs\\node.exe', env: {
        INIT_CWD: 'D:\\stale', CARGO_TARGET_DIR: 'D:\\stale-target', TAURI_RUNTIME_ELECTRON_ARGS: '--enable-logging', PRESERVED: 'yes',
      } },
      mkdirSync() {}, realpathSync: value => value, readdirSync: () => [], openSync: () => 23,
      freePort: async () => 9222,
      createRequire(packageJson) {
        assert.equal(name, 'coverage', 'transparent does not need the JavaScript CLI')
        assert.equal(packageJson, path.join(cwd, 'package.json'))
        return { resolve(specifier) {
          assert.equal(specifier, '@tauri-apps/cli/tauri.js')
          resolved = true
          return cli
        } }
      },
      spawn(command, args, options) {
        launched = true
        assert.equal(command, name === 'transparent' ? 'cargo' : 'C:\\Program Files\\nodejs\\node.exe')
        assert.deepEqual(Array.from(args), name === 'transparent'
          ? ['run', '--locked', '--manifest-path', path.join(cwd, 'Cargo.toml')]
          : [cli, 'dev', '--no-watch'])
        assert.equal(options.cwd, cwd)
        assert.equal(options.env.INIT_CWD, cwd)
        assert.equal(options.env.CARGO_TARGET_DIR, 'D:\\repo\\target')
        assert.equal(options.env.PRESERVED, 'yes')
        assert.equal(options.env.TAURI_RUNTIME_ELECTRON_ARGS, '--enable-logging --remote-debugging-address=127.0.0.1 --remote-debugging-port=9222')
        assert.deepEqual(Array.from(options.stdio), ['ignore', 23, 23])
        assert.equal(options.shell, undefined, 'launch without shell interpolation or Windows cmd shims')
        assert.equal(options.detached, platform !== 'win32', 'retain runner-owned process-tree cleanup')
        return child
      },
    })
    assert.equal(await run(name), child, 'CDP and native checks track the spawned CLI/Cargo process')
    assert.equal(launched, true)
    assert.equal(resolved, name === 'coverage')
  }
}
console.log('native diagnostics, evidence cleanup, CDP reuse, dashboard failures, retained diagnostics and standard app launches passed')
