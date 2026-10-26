// Exercise evidence failures without PowerShell, native input, files removed, or GUI.
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { EventEmitter } from 'node:events'
import { win32 as path } from 'node:path'
import vm from 'node:vm'

const source = readFileSync(new URL('./examples.mjs', import.meta.url), 'utf8').replaceAll('\r\n', '\n')
const nativeStart = source.indexOf('function nativeBlocked(')
const nativeEnd = source.indexOf('\nfunction nativeWindow(')
assert.ok(nativeStart >= 0 && nativeEnd > nativeStart)
const writes = []
let killed = false
const context = vm.createContext({
  assert, Buffer, Set, Date, JSON, Error, Object, String, path,
  root: 'D:\\repo', process: { platform: 'win32' }, existsSync: () => true,
  writeFileSync: (file, data) => writes.push({ file, observations: JSON.parse(data) }),
  clearTimeout() {},
  setTimeout(callback, delay) {
    assert.equal(delay, 60_000)
    queueMicrotask(callback)
    return 1
  },
  spawn() {
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
for (const name of ['streaming', 'helloworld']) {
  const calls = []
  await vm.runInNewContext(`${source.slice(sessionStart, sessionEnd)}\n(async () => { await session({ id: 'page' }); await session({ id: 'page' }) })()`, {
    name, sessions: new Map(), Session: { connect: async () => ({ call: async method => calls.push(method) }) },
  })
  assert.deepEqual(calls, name === 'streaming' ? ['Network.enable', 'Media.enable'] : [])
}
const diagnosticsStart = source.indexOf("    if (name === 'streaming') {\n      report.streamingDiagnostics")
const diagnosticsEnd = source.indexOf('    for (const page of sessions.values()) {', diagnosticsStart)
assert.ok(diagnosticsStart >= 0 && diagnosticsEnd > diagnosticsStart)
const events = [
  { method: 'Media.playerErrorsRaised', params: { errors: [{ errorType: 'pipeline_error' }] } },
  { method: 'Network.loadingFailed', params: { errorText: 'net::ERR_FAILED' } },
  { method: 'Runtime.consoleAPICalled' },
]
const report = { error: 'original streaming timeout' }
await vm.runInNewContext(`(async () => { ${source.slice(diagnosticsStart, diagnosticsEnd)} })()`, {
  name: 'streaming', report, Promise,
  sessions: new Map([
    ['live', { target: { url: 'tauri://localhost/' }, evaluate: async () => ({ src: 'stream://localhost/video.mp4', ready: 0, network: 2 }), events }],
    ['closed', { target: { url: 'tauri://localhost/closed' }, evaluate: async () => { throw new Error('target closed') }, events: [] }],
  ]),
})
assert.equal(report.error, 'original streaming timeout')
assert.equal(report.streamingDiagnostics[0].video.src, 'stream://localhost/video.mp4')
assert.equal(report.streamingDiagnostics[0].video.network, 2)
assert.deepEqual(report.streamingDiagnostics[0].events, events.slice(0, 2))
assert.equal(report.streamingDiagnostics[1].video.diagnosticError, 'target closed')
console.log('native timeout diagnostics, scoped evidence cleanup, and streaming failure evidence passed')
