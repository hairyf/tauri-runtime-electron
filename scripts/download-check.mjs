// Run production download handling with mocked Electron; no GUI or network.
// Run: node scripts/download-check.mjs
import assert from 'node:assert/strict'
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import { createRequire } from 'node:module'
import { EventEmitter } from 'node:events'
import vm from 'node:vm'

const require = createRequire(import.meta.url)
const root = fs.mkdtempSync(path.join(os.tmpdir(), 'electron-download-check-'))
const messages = []
const logs = []
const timers = new Set()
const target = new EventEmitter()
target.webRequest = { onBeforeRequest() {}, onCompleted() {}, onErrorOccurred() {} }
target.setPermissionRequestHandler = () => {}
let copyingFails = false
let stagingFails = false
let copies = 0
let insideCallback = false
const context = vm.createContext({
  require: (name) => name === 'electron' ? {
    app: { getPath: () => root, on() {}, whenReady: () => new Promise(() => {}) },
    ipcMain: { on() {}, handle() {} },
    protocol: { registerSchemesAsPrivileged() {} },
    session: { defaultSession: target },
  } : name === 'node:fs' ? {
    ...fs,
    appendFileSync: (_file, line) => logs.push(line),
    mkdtempSync(prefix) {
      if (stagingFails) throw new Error('staging denied')
      return fs.mkdtempSync(prefix)
    },
    // Renaming must not be needed, including on a different destination volume.
    renameSync() { throw Object.assign(new Error('cross device'), { code: 'EXDEV' }) },
    copyFileSync(source, destination, flags) {
      copies++
      assert.equal(flags, fs.constants.COPYFILE_EXCL)
      if (copyingFails) throw Object.assign(new Error('disk full'), { code: 'ENOSPC' })
      fs.copyFileSync(source, destination, flags)
    },
  } : name === 'node:os' ? { ...os, tmpdir: () => root } : require(name),
  process: { argv: ['node', 'main.cjs', '--port', '1'], env: {} },
  Buffer, URL, console,
  setTimeout(callback, delay) {
    assert.equal(delay, 30_000)
    const timer = { callback, unref() {} }
    timers.add(timer)
    return timer
  },
  clearTimeout: (timer) => timers.delete(timer),
  testSocket: { write: (line) => messages.push(JSON.parse(line)) },
})
vm.runInContext(`${fs.readFileSync(new URL('../electron-rs/assets/main.cjs', import.meta.url), 'utf8')}
;socket = testSocket; installPolicyHandlers();
webviewByContents.set(7, 70); handlerPolicy.set(70, { download: true });`, context)
const inspect = (expression) => vm.runInContext(expression, context)
class Item extends EventEmitter {
  savePath = ''
  getURL() { return 'https://download.invalid/file' }
  getFilename() { return 'suggested.bin' }
  getSavePath() { return this.savePath }
  setSavePath(value) {
    assert(insideCallback, 'setSavePath must remain inside will-download')
    this.savePath = value
  }
  cancel() { this.cancelled = true; this.emit('done', {}, 'cancelled') }
}
const begin = (savePath = '') => {
  const contents = new EventEmitter()
  contents.id = 7
  const item = new Item()
  item.savePath = savePath
  const event = { preventDefault() { this.prevented = true } }
  const before = messages.length
  insideCallback = true
  try { target.emit('will-download', event, item, contents) }
  finally { insideCallback = false }
  return { item, contents, event, request: messages[before]?.params }
}
const answer = (download, allow, destination) => context.handle('webview.downloadResponse', {
  id: download.request.id, allow, ...(destination === undefined ? {} : { destination }),
})
const done = (download, state = 'completed') => download.item.emit('done', {}, state)
const stage = (download, data = 'download bytes') => fs.writeFileSync(download.item.savePath, data)
const finished = () => messages.filter((message) => message.event === 'download.finished')
const expectFinished = (success, destination = null) => {
  assert.deepEqual(finished().at(-1).params, {
    webviewId: 70, url: 'https://download.invalid/file', path: destination, success,
  })
}
const expectClean = (download) => assert(!fs.existsSync(path.dirname(download.item.savePath)))
try {
  const late = begin()
  const early = begin()
  assert.notEqual(path.dirname(late.item.savePath), path.dirname(early.item.savePath))
  assert.equal(late.request.destination, path.join(root, 'suggested.bin'))
  assert.notEqual(late.request.destination, late.item.savePath, 'host sees suggested final path, not staging')
  stage(late)
  done(late)
  assert.equal(finished().length, 0, 'done must wait for the host')
  const final = path.join(root, 'late.bin')
  answer(late, true, final)
  expectFinished(true, final)
  assert.equal(fs.readFileSync(final, 'utf8'), 'download bytes')
  expectClean(late)
  answer(late, false)
  assert.equal(finished().length, 1, 'late duplicates cannot finish twice')

  const first = path.join(root, 'early.bin')
  answer(early, true, first)
  assert.equal(timers.size, 0, 'decision timer must not cancel a long accepted download')
  answer(early, false, path.join(root, 'ignored.bin'))
  assert(!early.item.cancelled, 'first host response wins')
  assert.equal(finished().length, 1, 'host response must wait for done')
  stage(early)
  done(early)
  expectFinished(true, first)
  expectClean(early)

  for (const completed of [false, true]) {
    const denied = begin()
    stage(denied)
    if (completed) done(denied)
    answer(denied, false)
    expectFinished(false)
    expectClean(denied)
    assert.equal(denied.item.cancelled, completed ? undefined : true)
  }
  for (const state of ['interrupted', 'cancelled']) {
    const failed = begin()
    stage(failed)
    done(failed, state)
    const before = finished().length
    assert(inspect(`downloadRequests.has(${failed.request.id})`))
    answer(failed, true, path.join(root, `${state}.bin`))
    assert.equal(finished().length, before + 1)
    expectFinished(false)
    expectClean(failed)
  }

  const existing = path.join(root, 'existing.bin')
  fs.writeFileSync(existing, 'original bytes')
  const collision = begin(existing)
  assert.equal(collision.request.destination, existing)
  stage(collision)
  answer(collision, true)
  done(collision)
  expectFinished(false)
  assert.equal(fs.readFileSync(existing, 'utf8'), 'original bytes', 'never clobber a destination')
  assert.equal(fs.readFileSync(collision.item.savePath, 'utf8'), 'download bytes', 'keep failed publish recoverable')
  assert(logs.some((line) => line.includes(collision.item.savePath)))

  const failedPublish = begin()
  stage(failedPublish)
  answer(failedPublish, true, path.join(root, 'disk-full.bin'))
  copyingFails = true
  done(failedPublish)
  copyingFails = false
  expectFinished(false)
  assert(fs.existsSync(failedPublish.item.savePath), 'publication failure must not delete complete data')
  assert(!fs.existsSync(path.join(root, 'disk-full.bin')))
  assert.equal(copies, 4, 'both successful copies plus collision and simulated I/O failure')

  for (const destination of ['', 42, 'bad\0path']) {
    const invalid = begin()
    stage(invalid)
    answer(invalid, true, destination)
    expectFinished(false)
    expectClean(invalid)
  }
  for (const completed of [false, true]) {
    const destroyed = begin()
    stage(destroyed)
    if (completed) done(destroyed)
    destroyed.contents.emit('destroyed')
    expectFinished(false)
    expectClean(destroyed)
  }
  const delayedCancel = begin()
  stage(delayedCancel)
  delayedCancel.item.cancel = () => { delayedCancel.item.cancelled = true }
  delayedCancel.contents.emit('destroyed')
  expectClean(delayedCancel)
  const beforeLateDone = finished().length
  // Model bytes/file handles appearing after the initial cancellation cleanup.
  fs.mkdirSync(path.dirname(delayedCancel.item.savePath))
  stage(delayedCancel)
  done(delayedCancel, 'cancelled')
  expectClean(delayedCancel)
  assert.equal(finished().length, beforeLateDone, 'late cancelled done cannot emit another result')

  const timeout = begin()
  stage(timeout)
  done(timeout)
  const timer = [...timers][0]
  assert(timer)
  timer.callback()
  expectFinished(false)
  expectClean(timeout)
  answer(timeout, true, path.join(root, 'too-late.bin'))
  assert(!fs.existsSync(path.join(root, 'too-late.bin')))
  assert.equal(inspect('downloadRequests.size'), 0)
  assert.equal(timers.size, 0)

  stagingFails = true
  const refused = begin()
  stagingFails = false
  assert.equal(refused.event.prevented, true)
  expectFinished(false)
  assert.equal(inspect('downloadRequests.size'), 0)
  console.log('ok: download ordering, first decision, denial/failure cleanup, exclusive cross-device publication, recoverable errors, destroy and timeout')
} finally {
  // root was created by this script; remove only its own isolated test data.
  fs.rmSync(root, { recursive: true, force: true })
}
