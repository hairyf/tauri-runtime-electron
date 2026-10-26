// Observe production download handling without Electron, network, or live timers.
import { constants, readFileSync } from 'node:fs'
import path from 'node:path'
import { createRequire } from 'node:module'
import { EventEmitter } from 'node:events'
import vm from 'node:vm'

const require = createRequire(import.meta.url)
const root = path.resolve('download-fixture')
const files = new Map()
const directories = new Set()
let nextDirectory = 0
const messages = []
const logs = []
const timers = new Set()
const timerDelays = []
const copyFlags = []
const cleanupOptions = []
const savePathDuringCallback = []
const target = new EventEmitter()
target.webRequest = { onBeforeRequest() {}, onCompleted() {}, onErrorOccurred() {} }
target.setPermissionRequestHandler = () => {}
let copyingFails = false
let stagingFails = false
let renames = 0
let insideCallback = false
const context = vm.createContext({
  require: (name) => name === 'electron' ? {
    app: { getPath: () => root, on() {}, whenReady: () => new Promise(() => {}) },
    ipcMain: { on() {}, handle() {} },
    protocol: { registerSchemesAsPrivileged() {} },
    session: { defaultSession: target },
  } : name === 'node:fs' ? {
    constants,
    appendFileSync: (_file, line) => logs.push(line),
    mkdtempSync(prefix) {
      if (stagingFails) throw new Error('staging denied')
      const directory = `${prefix}${++nextDirectory}`
      directories.add(directory)
      return directory
    },
    rmSync(directory, options) {
      cleanupOptions.push(options ?? null)
      directories.delete(directory)
      for (const file of files.keys()) {
        if (file.startsWith(`${directory}${path.sep}`)) files.delete(file)
      }
    },
    // A destination may be on another volume; publication must not need rename.
    renameSync() { renames++; throw Object.assign(new Error('cross device'), { code: 'EXDEV' }) },
    copyFileSync(source, destination, flags) {
      copyFlags.push(flags)
      if (copyingFails) throw Object.assign(new Error('disk full'), { code: 'ENOSPC' })
      if (!files.has(source)) throw Object.assign(new Error('missing source'), { code: 'ENOENT' })
      if ((flags & constants.COPYFILE_EXCL) && files.has(destination)) {
        throw Object.assign(new Error('destination exists'), { code: 'EEXIST' })
      }
      files.set(destination, files.get(source))
    },
  } : name === 'node:os' ? { tmpdir: () => root } : require(name),
  process: { argv: ['node', 'main.cjs', '--port', '1'], env: {} },
  Buffer, URL, console,
  setTimeout(callback, delay) {
    timerDelays.push(delay)
    const timer = { callback, unref() {} }
    timers.add(timer)
    return timer
  },
  clearTimeout: (timer) => timers.delete(timer),
  testSocket: { write: (line) => messages.push(JSON.parse(line)) },
})
vm.runInContext(`${readFileSync(new URL('../../assets/main.cjs', import.meta.url), 'utf8')}
;socket = testSocket; installPolicyHandlers();
webviewByContents.set(7, 70); handlerPolicy.set(70, { download: true });`, context)
const inspect = (expression) => vm.runInContext(expression, context)
class Item extends EventEmitter {
  savePath = ''
  getURL() { return 'https://download.invalid/file' }
  getFilename() { return 'suggested.bin' }
  getSavePath() { return this.savePath }
  setSavePath(value) { savePathDuringCallback.push(insideCallback); this.savePath = value }
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
const stage = (download) => files.set(download.item.savePath, 'download bytes')
const relative = (file) => file === null ? null : path.relative(root, file).split(path.sep).join('/')
const finished = () => messages.filter((message) => message.event === 'download.finished')
  .map(({ params }) => ({ ...params, path: relative(params.path) }))
const clean = (download) => !directories.has(path.dirname(download.item.savePath)) && !files.has(download.item.savePath)
const bytes = (file) => files.get(file) ?? null

let result
switch (process.argv[2]) {
  case 'ordering': {
    const late = begin()
    const early = begin()
    const distinctStages = path.dirname(late.item.savePath) !== path.dirname(early.item.savePath)
    const suggestedDestination = relative(late.request.destination)
    const stagingExposed = late.request.destination === late.item.savePath
    stage(late)
    done(late)
    const finishedBeforeDecision = finished().length
    const final = path.join(root, 'late.bin')
    answer(late, true, final)
    const lateResult = finished().at(-1)
    const lateBytes = bytes(final)
    const lateClean = clean(late)
    answer(late, false)
    const finishedAfterDuplicate = finished().length
    const first = path.join(root, 'early.bin')
    answer(early, true, first)
    const timersAfterAccept = timers.size
    answer(early, false, path.join(root, 'ignored.bin'))
    const cancelledAfterDuplicate = early.item.cancelled ?? false
    const finishedBeforeDone = finished().length
    stage(early)
    done(early)
    result = { distinctStages, suggestedDestination, stagingExposed, finishedBeforeDecision,
      lateResult, lateBytes, lateClean, finishedAfterDuplicate, timersAfterAccept,
      cancelledAfterDuplicate, finishedBeforeDone, earlyResult: finished().at(-1),
      earlyBytes: bytes(first), earlyClean: clean(early), ignoredExists: files.has(path.join(root, 'ignored.bin')),
      finishedCount: finished().length }
    break
  }
  case 'denial-and-failure': {
    const denied = []
    for (const completed of [false, true]) {
      const download = begin()
      stage(download)
      if (completed) done(download)
      answer(download, false)
      denied.push({ result: finished().at(-1), clean: clean(download), cancelled: download.item.cancelled ?? null })
    }
    const failures = []
    for (const state of ['interrupted', 'cancelled']) {
      const download = begin()
      stage(download)
      done(download, state)
      const before = finished().length
      const pendingBeforeDecision = inspect(`downloadRequests.has(${download.request.id})`)
      answer(download, true, path.join(root, `${state}.bin`))
      failures.push({ state, pendingBeforeDecision, finishedDelta: finished().length - before,
        result: finished().at(-1), clean: clean(download) })
    }
    result = { denied, failures, finishedCount: finished().length }
    break
  }
  case 'publication-errors': {
    const existing = path.join(root, 'existing.bin')
    files.set(existing, 'original bytes')
    const collision = begin(existing)
    stage(collision)
    answer(collision, true)
    done(collision)
    const collisionResult = { requestedDestination: relative(collision.request.destination),
      result: finished().at(-1), originalBytes: bytes(existing), stagedBytes: bytes(collision.item.savePath),
      loggedStage: logs.some((line) => line.includes(collision.item.savePath)) }
    const failed = begin()
    stage(failed)
    const destination = path.join(root, 'disk-full.bin')
    answer(failed, true, destination)
    copyingFails = true
    done(failed)
    copyingFails = false
    result = { collision: collisionResult, failed: { result: finished().at(-1),
      stagedBytes: bytes(failed.item.savePath), destinationExists: files.has(destination) },
      finishedCount: finished().length }
    break
  }
  case 'invalid-destinations': {
    const invalid = []
    for (const destination of ['', 42, 'bad\0path']) {
      const download = begin()
      stage(download)
      answer(download, true, destination)
      invalid.push({ destination, result: finished().at(-1), clean: clean(download) })
    }
    result = { invalid, finishedCount: finished().length }
    break
  }
  case 'destruction': {
    const destroyed = []
    for (const completed of [false, true]) {
      const download = begin()
      stage(download)
      if (completed) done(download)
      download.contents.emit('destroyed')
      destroyed.push({ result: finished().at(-1), clean: clean(download) })
    }
    const delayed = begin()
    stage(delayed)
    delayed.item.cancel = () => { delayed.item.cancelled = true }
    delayed.contents.emit('destroyed')
    const cleanBeforeLateDone = clean(delayed)
    const before = finished().length
    // Model bytes appearing after the native cancellation releases file handles.
    directories.add(path.dirname(delayed.item.savePath))
    stage(delayed)
    done(delayed, 'cancelled')
    result = { destroyed, cleanBeforeLateDone, cleanAfterLateDone: clean(delayed),
      lateFinishedDelta: finished().length - before, finishedCount: finished().length }
    break
  }
  case 'timeout': {
    const download = begin()
    stage(download)
    done(download)
    const pendingBeforeTimeout = inspect('downloadRequests.size')
    const finishedBeforeTimeout = finished().length
    const timer = [...timers][0]
    const timerPresent = timer !== undefined
    timer?.callback()
    answer(download, true, path.join(root, 'too-late.bin'))
    result = { pendingBeforeTimeout, finishedBeforeTimeout, timerPresent, result: finished().at(-1),
      clean: clean(download), lateDestinationExists: files.has(path.join(root, 'too-late.bin')),
      finishedCount: finished().length }
    break
  }
  case 'staging-error': {
    stagingFails = true
    const refused = begin()
    result = { prevented: refused.event.prevented ?? false, result: finished().at(-1),
      requestedCount: messages.filter((message) => message.event === 'download.requested').length,
      finishedCount: finished().length }
    break
  }
  default: throw new Error(`unknown scenario ${process.argv[2]}`)
}
console.log(JSON.stringify({ ...result, copyFlags, cleanupOptions, renames, timerDelays, savePathDuringCallback,
  pending: inspect('downloadRequests.size'), timers: timers.size }))
