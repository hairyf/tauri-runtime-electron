// Run complete production main/preload code with small Electron fakes; no GUI/network.
// Run: node scripts/webview-policy-check.mjs
import assert from 'node:assert/strict'
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { createRequire } from 'node:module'
import { EventEmitter } from 'node:events'
import vm from 'node:vm'

const require = createRequire(import.meta.url)
const electronDirectory = fileURLToPath(new URL('../electron/', import.meta.url))
const root = fs.mkdtempSync(path.join(os.tmpdir(), 'electron-webview-policy-check-'))
const messages = []
const timers = new Set()
const contentsById = new Map()
const ipcListeners = new Map()
const ipcHandlers = new Map()
const external = []
let nextContents = 0
function makeProtocol() {
  const handlers = new Map()
  return {
    handlers,
    registerSchemesAsPrivileged() {},
    handle(scheme, callback) {
      assert(!handlers.has(scheme), `duplicate protocol handler: ${scheme}`)
      handlers.set(scheme, callback)
    },
  }
}
function makeSession() {
  const target = new EventEmitter()
  target.protocol = makeProtocol()
  target.forwarded = []
  target.fetch = function (request, options) {
    assert.equal(this, target)
    assert.equal(options.bypassCustomProtocolHandlers, true)
    target.forwarded.push(request)
    return Promise.resolve(new Response('ordinary HTTP'))
  }
  target.navigationInstalls = 0
  target.permissionInstalls = 0
  target.requestEndInstalls = 0
  target.webRequest = {
    onBeforeRequest(callback) {
      target.navigationInstalls++
      target.beforeRequest = callback
    },
    onCompleted(callback) { target.requestEndInstalls++; target.completed = callback },
    onErrorOccurred(callback) { target.requestEndInstalls++; target.errorOccurred = callback },
  }
  target.setPermissionRequestHandler = () => { target.permissionInstalls++ }
  return target
}
const defaultSession = makeSession()
const pathSessions = new Map()
const partitionSessions = new Map()
const pathCalls = []
const session = {
  defaultSession,
  fromPath(directory) {
    pathCalls.push(directory)
    if (!pathSessions.has(directory)) pathSessions.set(directory, makeSession())
    return pathSessions.get(directory)
  },
  fromPartition(partition) {
    assert(!partition.startsWith('persist:'), 'incognito must not persist')
    if (!partitionSessions.has(partition)) partitionSessions.set(partition, makeSession())
    return partitionSessions.get(partition)
  },
}
class Contents extends EventEmitter {
  constructor() {
    super()
    this.id = ++nextContents
    this.loads = []
    this.stops = 0
    this.mainFrame = { url: 'https://page.invalid/', origin: 'https://page.invalid' }
    contentsById.set(this.id, this)
  }
  isDestroyed() { return false }
  stop() { this.stops++ }
  setWindowOpenHandler(callback) { this.openWindow = callback }
  loadURL(url) { this.loads.push(url); return Promise.resolve() }
  close() { this.emit('destroyed') }
}
class Window extends EventEmitter {
  constructor(options) {
    super()
    this.options = options
    this.webContents = new Contents()
    this.contentView = { addChildView() {}, removeChildView() {} }
  }
  isDestroyed() { return false }
  loadURL(url) { return this.webContents.loadURL(url) }
  getContentBounds() { return { x: 0, y: 0, width: 800, height: 600 } }
}
class ChildView {
  constructor(options) { this.options = options; this.webContents = new Contents() }
  setBounds(bounds) { this.bounds = bounds }
  getBounds() { return this.bounds }
}
const app = new EventEmitter()
app.whenReady = () => new Promise(() => {})
const electron = {
  app, session, BrowserWindow: Window, WebContentsView: ChildView,
  protocol: defaultSession.protocol,
  net: { fetch: (request, options) => defaultSession.fetch(request, options) },
  ipcMain: {
    on: (name, callback) => ipcListeners.set(name, callback),
    handle: (name, callback) => ipcHandlers.set(name, callback),
  },
  webContents: { fromId: (id) => contentsById.get(id) },
  shell: { openExternal: (...args) => external.push(args) },
}
const context = vm.createContext({
  require: (name) => name === 'electron' ? electron
    : name === 'node:fs' ? { ...fs, appendFileSync() {} }
    : name === 'node:os' ? { ...os, tmpdir: () => root } : require(name),
  __dirname: electronDirectory,
  process: { argv: ['node', 'main.cjs', '--port', '1'], env: {} },
  Buffer, URL, Request, Response, Headers, Uint8Array, console,
  setTimeout(callback, delay) {
    assert.equal(delay, 15_000)
    const timer = { callback, unref() { this.unreferenced = true } }
    timers.add(timer)
    return timer
  },
  clearTimeout: (timer) => timers.delete(timer),
  testSocket: { write(line) {
    const message = JSON.parse(line)
    messages.push(message)
    if (message.event === 'protocol.request') {
      context.handle('protocol.respond', { id: message.params.id, status: 200, body: '' })
    }
  } },
})
try {
  vm.runInContext(`${fs.readFileSync(path.join(electronDirectory, 'main.cjs'), 'utf8')}
;socket = testSocket; installPolicyHandlers();`, context)
  const runtime = vm.runInContext('({ handle, sessionFor, installPolicyHandlers, navigationRequests, windows, childWebviews })', context)
  const plain = (value) => JSON.parse(JSON.stringify(value))
  const scripts = [
    { script: 'globalThis.mainScript = (globalThis.mainScript ?? 0) + 1', mainFrameOnly: true },
    { script: 'globalThis.everyFrameScript = (globalThis.everyFrameScript ?? 0) + 1', mainFrameOnly: false },
  ]
  await runtime.handle('window.create', {
    windowId: 1, trafficLightPosition: { x: 12, y: 18 }, tabbingIdentifier: 'editor-tabs',
    webview: { webviewId: 10, url: 'https://page.invalid/', initScripts: scripts, policy: { navigation: true } },
  })
  const window = runtime.windows.get(1)
  assert.deepEqual(plain(window.options.trafficLightPosition), { x: 12, y: 18 })
  assert.equal(window.options.tabbingIdentifier, 'editor-tabs')
  await runtime.handle('webview.create', {
    windowId: 1, webviewId: 20, url: 'https://child.invalid/', initScripts: scripts,
    policy: { navigation: true },
  })
  const child = runtime.childWebviews.get(20).view
  await runtime.handle('window.create', {
    windowId: 2, webview: { webviewId: 30, url: 'https://no-handler.invalid/' },
  })
  const noHandler = runtime.windows.get(2).webContents

  // Both request forms route by the actual contents, never the parent window.
  let nextRequest = 0
  function begin(contents, resourceType = 'mainFrame', byId = false, target = defaultSession, requestId = ++nextRequest) {
    const replies = []
    const details = Object.freeze({
      ...(byId ? { webContentsId: contents.id } : { webContents: contents }),
      id: requestId, resourceType, url: 'https://submit.invalid/form', method: 'POST',
      uploadData: [{ bytes: Buffer.from('original POST body') }],
    })
    const before = messages.length
    const stops = contents.stops
    target.beforeRequest(details, (reply) => {
      if (reply.cancel) assert.equal(contents.stops, stops + (resourceType === 'mainFrame' ? 1 : 0),
        'only main-frame denial stops the page before native cancellation')
      replies.push(plain(reply))
    })
    return { details, replies, request: messages[before]?.params }
  }
  const mainNavigation = begin(window.webContents)
  const childNavigation = begin(child.webContents, 'subFrame', true)
  assert.equal(mainNavigation.request.webviewId, 10)
  assert.equal(childNavigation.request.webviewId, 20)
  assert.notEqual(mainNavigation.request.id, childNavigation.request.id)
  assert.equal(mainNavigation.request.url, mainNavigation.details.url)
  assert.deepEqual(mainNavigation.replies, [], 'request waits for the host')
  assert.equal(window.webContents.listenerCount('destroyed'), 1)
  assert([...timers].every((timer) => timer.unreferenced))
  const answer = (navigation, allow) => runtime.handle('webview.navigationResponse', { id: navigation.request.id, allow })
  answer(childNavigation, false)
  assert.deepEqual(childNavigation.replies, [{ cancel: true }])
  assert.equal(child.webContents.stops, 0, 'subframe deny never stops its page')
  const deniedIframe = begin(window.webContents, 'subFrame')
  answer(deniedIframe, false)
  assert.deepEqual(deniedIframe.replies, [{ cancel: true }])
  assert.equal(window.webContents.stops, 0, 'iframe denial must not abort the pending top navigation')
  assert.deepEqual(mainNavigation.replies, [], 'child decision cannot answer the main page')
  answer(mainNavigation, true)
  assert.deepEqual(mainNavigation.replies, [{ cancel: false }])
  answer(mainNavigation, false)
  answer(childNavigation, true)
  assert.equal(mainNavigation.replies.length, 1, 'duplicate decisions are ignored')
  assert.equal(childNavigation.replies.length, 1)
  assert.equal(window.webContents.listenerCount('destroyed'), 0)
  assert.equal(child.webContents.listenerCount('destroyed'), 0)
  assert.equal(timers.size, 0)
  assert.equal(mainNavigation.details.method, 'POST')
  assert.equal(mainNavigation.details.uploadData[0].bytes.toString(), 'original POST body')
  assert.deepEqual(window.webContents.loads, ['https://page.invalid/'], 'never replay navigation with loadURL')
  assert.deepEqual(child.webContents.loads, ['https://child.invalid/'])

  for (const navigation of [
    begin({ id: 999 }, 'mainFrame', true), begin(noHandler),
    begin(window.webContents, 'xhr'), begin(child.webContents, 'image', true),
  ]) {
    assert.deepEqual(navigation.replies, [{}])
    assert.equal(navigation.request, undefined, 'unknown/no-handler/non-navigation requests bypass policy')
  }
  const invalidAllow = begin(window.webContents)
  answer(invalidAllow, 'true')
  assert.deepEqual(invalidAllow.replies, [{ cancel: true }], 'only boolean true allows')
  const failedStop = begin(window.webContents)
  const stop = window.webContents.stop
  window.webContents.stop = function () { this.stops++; throw new Error('stop failed') }
  try { assert.throws(() => answer(failedStop, false), /stop failed/) }
  finally { window.webContents.stop = stop }
  assert.deepEqual(failedStop.replies, [{ cancel: true }], 'stop failure must not bypass native cancellation')
  assert.equal(runtime.navigationRequests.size, 0)
  assert.equal(timers.size, 0)
  for (const reason of ['destroyed', 'timeout']) {
    const navigation = begin(child.webContents)
    const timer = [...timers][0]
    if (reason === 'destroyed') child.webContents.emit('destroyed')
    else timer.callback()
    assert.deepEqual(navigation.replies, [{ cancel: true }])
    assert.equal(runtime.navigationRequests.size, 0)
    assert.equal(child.webContents.listenerCount('destroyed'), 0)
    assert.equal(timers.size, 0)
    answer(navigation, true)
    timer.callback()
    assert.equal(navigation.replies.length, 1, 'late response/timeout must not answer twice')
  }
  for (const event of ['completed', 'errorOccurred']) {
    const stale = begin(window.webContents)
    const timer = [...timers][0]
    defaultSession[event]({ id: stale.details.id + 1000 })
    assert(runtime.navigationRequests.has(stale.request.id), 'unrelated completion must not retire a request')
    const stops = window.webContents.stops
    defaultSession[event]({ id: stale.details.id })
    assert.equal(runtime.navigationRequests.size, 0)
    assert.equal(window.webContents.listenerCount('destroyed'), 0)
    assert.equal(timers.size, 0)
    const newer = begin(window.webContents)
    answer(stale, false)
    timer.callback()
    assert.equal(window.webContents.stops, stops, 'late decisions/timeouts never stop newer navigation')
    assert.deepEqual(stale.replies, [], 'ended native request needs no stale callback')
    assert.deepEqual(newer.replies, [])
    answer(newer, true)
    assert.deepEqual(newer.replies, [{ cancel: false }])
  }
  for (const contents of [window.webContents, child.webContents, noHandler]) {
    for (const url of ['https://external.invalid/', 'mailto:user@example.invalid', 'file:///private']) {
      assert.deepEqual(plain(contents.openWindow({ url })), { action: 'deny' })
    }
  }
  assert.deepEqual(external, [], 'window.open must never launch a system browser')

  // Execute each generated preload in both frame kinds, including its real page-world scripts.
  for (const options of [window.options, child.options]) {
    const preferences = options.webPreferences
    assert.equal(preferences.contextIsolation, true)
    assert.equal(preferences.sandbox, true)
    assert.equal(preferences.nodeIntegration, false)
    assert.equal(preferences.nodeIntegrationInSubFrames, true)
    const preload = fs.readFileSync(preferences.preload, 'utf8')
    for (const isMainFrame of [true, false]) {
      const sent = []
      const invoked = []
      const page = vm.createContext({ window: { fetch() {} }, location: { href: 'https://page.invalid/' }, URL, Request })
      vm.runInNewContext(preload, {
        require(name) {
          assert.equal(name, 'electron')
          return {
            contextBridge: { exposeInMainWorld: (name, value) => { page.window[name] = value } },
            ipcRenderer: {
              send: (...args) => sent.push(args),
              invoke: (...args) => { invoked.push(args); return Promise.resolve('reply') },
            },
            webFrame: { executeJavaScript: (script) => vm.runInContext(script, page) },
          }
        },
        process: { isMainFrame }, console,
      })
      assert.equal(page.mainScript, isMainFrame ? 1 : undefined)
      assert.equal(page.everyFrameScript, 1)
      assert.equal(Boolean(page.window.ipc), isMainFrame, 'only main frame gets the IPC bridge')
      if (isMainFrame) {
        page.window.ipc.postMessage(42)
        assert.deepEqual(sent, [['tauri-ipc', '42']])
        const request = { body: Uint8Array.of(1, 2) }
        assert.equal(await page.window.ipc.fetch(request), 'reply')
        assert.deepEqual(invoked, [['tauri-ipc-fetch', request]])
      }
    }
  }
  const ipc = ipcListeners.get('tauri-ipc')
  const sender = window.webContents
  const beforeIpc = messages.length
  ipc({ sender, senderFrame: sender.mainFrame }, 'main message')
  assert.deepEqual(messages.at(-1), {
    event: 'ipc.message', params: { webviewId: 10, url: sender.mainFrame.url, body: 'main message' },
  })
  const opaque = { url: 'data:text/html,opaque', origin: 'null' }
  for (const senderFrame of [null, { ...sender.mainFrame }, opaque]) {
    ipc({ sender, senderFrame }, 'not main')
    await assert.rejects(() => ipcHandlers.get('tauri-ipc-fetch')({ sender, senderFrame }, {}), /unknown IPC sender/)
  }
  ipc({ sender: { id: 999, mainFrame: sender.mainFrame }, senderFrame: sender.mainFrame }, 'unknown')
  ipc({ sender: { id: sender.id, mainFrame: opaque }, senderFrame: opaque }, 'opaque')
  assert.equal(messages.length, beforeIpc + 1, 'main process also rejects non-main/unknown/opaque IPC')

  assert.equal(runtime.sessionFor(), undefined)
  runtime.handle('protocol.register', { scheme: 'tauri' })
  const directory = path.join(root, 'profiles', 'shared')
  const shared = runtime.sessionFor({ dataDirectory: directory })
  assert.equal(runtime.sessionFor({ dataDirectory: path.join(directory, '..', 'shared') }), shared)
  assert.equal(runtime.sessionFor({ dataDirectory: path.relative(process.cwd(), directory) }), shared)
  assert(pathCalls.every((value) => path.isAbsolute(value)), 'fromPath gets resolved absolute paths')
  const separate = runtime.sessionFor({ dataDirectory: path.join(root, 'profiles', 'separate') })
  assert.notEqual(shared, separate)
  const privateOne = runtime.sessionFor({ incognito: true, dataDirectory: directory })
  const privateTwo = runtime.sessionFor({ incognito: true, dataDirectory: directory })
  assert.notEqual(privateOne, privateTwo)
  assert.notEqual(privateOne, shared)
  assert.equal(partitionSessions.size, 2)
  runtime.handle('protocol.register', { scheme: 'asset' })
  runtime.handle('protocol.register', { scheme: 'asset' })
  const future = runtime.sessionFor({ dataDirectory: path.join(root, 'profiles', 'future') })
  const otherSession = begin(window.webContents, 'mainFrame', false, shared)
  const sameId = begin(window.webContents, 'mainFrame', false, separate, otherSession.details.id)
  shared.errorOccurred({ id: otherSession.details.id })
  assert(!runtime.navigationRequests.has(otherSession.request.id))
  assert(runtime.navigationRequests.has(sameId.request.id), 'request retirement is scoped to its session')
  answer(sameId, true)
  assert.deepEqual(sameId.replies, [{ cancel: false }])
  answer(otherSession, false)
  assert.deepEqual(otherSession.replies, [])
  for (const target of [defaultSession, shared, separate, privateOne, privateTwo, future]) {
    runtime.installPolicyHandlers(target)
    assert.equal(target.navigationInstalls, 1)
    assert.equal(target.requestEndInstalls, 2)
    assert.equal(target.permissionInstalls, 1)
    assert.equal(target.listenerCount('will-download'), 1)
    assert.deepEqual([...target.protocol.handlers.keys()].sort(), ['asset', 'http', 'https', 'tauri'])
    await target.protocol.handlers.get('tauri')(new Request('tauri://localhost/first'))
    assert.equal(messages.at(-1).params.uri, 'tauri://localhost/first')
    await target.protocol.handlers.get('http')(new Request('http://asset.localhost/second'))
    assert.equal(messages.at(-1).params.uri, 'asset://localhost/second')
    assert.equal(messages.at(-1).params.scheme, 'asset')
    const beforeFallback = messages.length
    for (const url of ['https://remote.invalid/', 'http://tauri.localhost:1420/', 'http://unregistered.localhost/']) {
      const request = new Request(url)
      const response = await target.protocol.handlers.get(new URL(url).protocol.slice(0, -1))(request)
      assert.equal(await response.text(), 'ordinary HTTP')
      assert.equal(target.forwarded.at(-1), request, 'ordinary HTTP uses this session and the original request')
    }
    assert.equal(target.forwarded.length, 3, 'custom-session fallback must not use the default network session')
    assert.equal(messages.length, beforeFallback)
  }
  assert.equal(runtime.navigationRequests.size, 0)
  assert.equal(timers.size, 0)
  console.log('ok: navigation routing/POST/cleanup, deny popups, frame-scoped preload/IPC, session isolation/protocols/one-time policies, traffic-light/tabbing options')
} finally {
  app.emit('will-quit')
  // root was created by this script; remove only its own isolated test data.
  fs.rmSync(root, { recursive: true, force: true })
}
