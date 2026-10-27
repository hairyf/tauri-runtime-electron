// Observe the complete production assets with native doubles; Rust owns policy assertions.
import assert from 'node:assert/strict'
import fs from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { createRequire } from 'node:module'
import { EventEmitter } from 'node:events'
import vm from 'node:vm'
import { createRuntime } from './runtime-modules.mjs'

const require = createRequire(new URL('../../assets/main.cjs', import.meta.url))
const electronDirectory = fileURLToPath(new URL('../../assets/', import.meta.url))
const root = path.join(electronDirectory, 'virtual-webview-policy')
const generatedPreloads = new Map()
let nextDirectory = 0
// Keep production's generated preloads in memory; only the real source assets are read from disk.
const nativeFs = {
  appendFileSync() {},
  mkdtempSync: (prefix) => `${prefix}${++nextDirectory}`,
  readFileSync: (file, encoding) => generatedPreloads.get(file) ?? fs.readFileSync(file, encoding),
  writeFileSync: (file, source) => generatedPreloads.set(file, source),
  rmSync(directory) {
    for (const file of generatedPreloads.keys()) {
      if (path.dirname(file) === directory) generatedPreloads.delete(file)
    }
  },
}
const messages = []
const timers = new Set()
const contentsById = new Map()
const lifecycleDestroyedListeners = new Set()
const ipcListeners = new Map()
const ipcHandlers = new Map()
const external = []
let nextContents = 0
let nextSession = 0
function makeSession() {
  const target = new EventEmitter()
  target.id = ++nextSession
  const handlers = new Map()
  target.protocolCalls = []
  target.protocol = {
    handlers,
    registerSchemesAsPrivileged() {},
    handle(scheme, callback) {
      target.protocolCalls.push(scheme)
      handlers.set(scheme, callback)
    },
  }
  target.forwarded = []
  target.fetch = function (request, options) {
    target.forwarded.push({ request, options, receiver: this === target })
    return Promise.resolve(new Response('ordinary HTTP'))
  }
  target.navigationInstalls = 0
  target.permissionInstalls = 0
  target.requestEndInstalls = 0
  target.webRequest = {
    onBeforeRequest(callback) { target.navigationInstalls++; target.beforeRequest = callback },
    onCompleted(callback) { target.requestEndInstalls++; target.completed = callback },
    onErrorOccurred(callback) { target.requestEndInstalls++; target.errorOccurred = callback },
  }
  target.setPermissionRequestHandler = (callback) => {
    target.permissionInstalls++
    target.permissionRequest = callback
  }
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
    if (!partitionSessions.has(partition)) partitionSessions.set(partition, makeSession())
    return partitionSessions.get(partition)
  },
}
class Contents extends EventEmitter {
  constructor(options = {}) {
    super()
    this.session = options.webPreferences?.session ?? defaultSession
    this.id = ++nextContents
    this.loads = []
    this.stops = 0
    this.destroyed = false
    this.mainFrame = { url: 'https://page.invalid/', origin: 'https://page.invalid', executeJavaScript: async () => false }
    this.debugger = new EventEmitter()
    contentsById.set(this.id, this)
  }
  isDestroyed() { return this.destroyed }
  stop() { this.stops++ }
  setWindowOpenHandler(callback) { this.openWindow = callback }
  loadURL(url) { this.url = url; this.loads.push(url); return Promise.resolve() }
  getURL() { return this.url ?? '' }
  close() { this.destroyed = true; this.emit('destroyed') }
}
class Window extends EventEmitter {
  constructor(options) {
    super()
    this.options = options
    this.bounds = { x: 0, y: 0, width: options.width, height: options.height }
    const views = []
    this.contentView = {
      setBackgroundColor() {},
      children: views,
      addChildView(view) { views.push(view) },
      removeChildView(view) { const index = views.indexOf(view); if (index >= 0) views.splice(index, 1) },
    }
  }
  get webContents() { return this.contentView.children[0]?.webContents }
  show() {}
  isDestroyed() { return false }
  loadURL(url) { return this.webContents.loadURL(url) }
  getBounds() { return { ...this.bounds } }
  getContentBounds() { return { ...this.bounds } }
  setContentSize(width, height) { Object.assign(this.bounds, { width, height }) }
  setMinimumSize() {}
  setMaximumSize() {}
}
class ChildView {
  constructor(options) { this.options = options; this.webContents = new Contents(options) }
  setBackgroundColor(color) { this.color = color }
  setBounds(bounds) { this.bounds = bounds }
  getBounds() { return this.bounds }
}
const app = new EventEmitter()
app.whenReady = () => new Promise(() => {})
const electron = {
  app, session, BrowserWindow: Window, BaseWindow: Window, WebContentsView: ChildView,
  screen: Object.assign(new EventEmitter(), { getDisplayMatching: () => ({ scaleFactor: 1 }) }),
  nativeTheme: Object.assign(new EventEmitter(), { shouldUseDarkColors: false }),
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
  process: { argv: ['node', 'main.cjs', '--port', '1'], env: {}, platform: 'win32' },
  Buffer, URL, Request, Response, Headers, Uint8Array, console,
  setTimeout(callback, delay) {
    const timer = { callback, delay, unreferenced: false, unref() { this.unreferenced = true } }
    timers.add(timer)
    return timer
  },
  clearTimeout: (timer) => timers.delete(timer),
})
const runtime = createRuntime(context, electron, {
  require: (name) => name === 'electron' ? electron : name === 'node:fs' ? nativeFs
    : name === 'node:os' ? { tmpdir: () => root } : require(name),
  send(message) {
    messages.push(message)
    if (message.event === 'protocol.request') runtime.handle('protocol.respond', { id: message.params.id, status: 200, body: '' })
  },
  request: (event, params) => ({ action: 'deny' }),
})
context.handle = runtime.handle
const plain = (value) => JSON.parse(JSON.stringify(value))
let window
let child
let noHandler
let nextRequest = 0
function begin(contents, resourceType = 'mainFrame', byId = false, target = defaultSession, requestId = ++nextRequest) {
  const replies = []
  const stopsAtReply = []
  const details = Object.freeze({
    ...(byId ? { webContentsId: contents.id } : { webContents: contents }),
    id: requestId, resourceType, url: 'https://submit.invalid/form', method: 'POST',
    uploadData: [{ bytes: Buffer.from('original POST body') }],
  })
  const before = messages.length
  const stops = contents.stops ?? 0
  target.beforeRequest(details, (reply) => {
    stopsAtReply.push((contents.stops ?? 0) - stops)
    replies.push(plain(reply))
  })
  return { details, replies, stopsAtReply, request: messages[before]?.params ?? null }
}
const answer = (navigation, allow) => runtime.handle('webview.navigationResponse', { id: navigation.request.id, allow })
const navigationState = (navigation) => plain({
  request: navigation.request, replies: navigation.replies, stopsAtReply: navigation.stopsAtReply,
  url: navigation.details.url, method: navigation.details.method,
  body: navigation.details.uploadData[0].bytes.toString(),
})
const cleanup = () => ({
  pending: runtime.state.navigationRequests.size,
  timers: timers.size,
  destroyedListeners: [window.webContents, child.webContents].map((contents) =>
    contents.listeners('destroyed').filter((listener) => !lifecycleDestroyedListeners.has(listener)).length),
})
function navigation() {
  const main = begin(window.webContents)
  const subframe = begin(child.webContents, 'subFrame', true)
  const waiting = {
    main: navigationState(main), child: navigationState(subframe),
    timers: [...timers].map(({ delay, unreferenced }) => ({ delay, unreferenced })),
    cleanup: cleanup(),
  }
  answer(subframe, false)
  const iframe = begin(window.webContents, 'subFrame')
  answer(iframe, false)
  const denied = {
    child: navigationState(subframe), iframe: navigationState(iframe),
    main: navigationState(main), stops: [window.webContents.stops, child.webContents.stops],
  }
  answer(main, true)
  answer(main, false)
  answer(subframe, true)
  const decided = { main: navigationState(main), child: navigationState(subframe), cleanup: cleanup() }
  const bypass = [
    begin({ id: 999 }, 'mainFrame', true), begin(noHandler.webContents),
    begin(window.webContents, 'xhr'), begin(child.webContents, 'image', true),
  ].map(navigationState)
  const invalidAllow = begin(window.webContents)
  answer(invalidAllow, 'true')
  const failedStop = begin(window.webContents)
  const stop = window.webContents.stop
  window.webContents.stop = function () { this.stops++; throw new Error('stop failed') }
  let stopError = null
  try { answer(failedStop, false) }
  catch (error) { stopError = error.message }
  finally { window.webContents.stop = stop }
  const rejected = { invalidAllow: navigationState(invalidAllow), failedStop: navigationState(failedStop), stopError, cleanup: cleanup() }
  const lifecycle = []
  for (const reason of ['timeout', 'destroyed']) {
    const pending = begin(child.webContents)
    const timer = [...timers][0]
    if (reason === 'destroyed') child.webContents.close()
    else timer.callback()
    const finished = { navigation: navigationState(pending), cleanup: cleanup() }
    const stops = child.webContents.stops
    answer(pending, true)
    timer.callback()
    lifecycle.push({ reason, finished, afterLate: navigationState(pending), lateStops: child.webContents.stops - stops })
  }
  const retired = []
  for (const event of ['completed', 'errorOccurred']) {
    const stale = begin(window.webContents)
    const timer = [...timers][0]
    defaultSession[event]({ id: stale.details.id + 1000 })
    const unrelatedPending = runtime.state.navigationRequests.has(stale.request.id)
    const stops = window.webContents.stops
    defaultSession[event]({ id: stale.details.id })
    const ended = cleanup()
    const newer = begin(window.webContents)
    answer(stale, false)
    timer.callback()
    const newerWaiting = navigationState(newer)
    const lateStops = window.webContents.stops - stops
    answer(newer, true)
    retired.push({ event, unrelatedPending, ended, stale: navigationState(stale), newerWaiting, newer: navigationState(newer), lateStops })
  }
  const popups = []
  for (const contents of [window.webContents, child.webContents, noHandler.webContents]) {
    for (const url of ['https://external.invalid/', 'mailto:user@example.invalid', 'file:///private']) {
      popups.push({ url, reply: contents.openWindow({ url }) })
    }
  }
  return {
    waiting, denied, decided, bypass, rejected, lifecycle, retired, popups, external,
    loads: [window.webContents.loads, child.webContents.loads], cleanup: cleanup(),
  }
}
async function preload() {
  const frames = []
  for (const [owner, options] of [['main', window.options], ['child', child.options]]) {
    const preferences = options.webPreferences
    const source = nativeFs.readFileSync(preferences.preload, 'utf8')
    for (const isMainFrame of [true, false]) {
      const sent = []
      const invoked = []
      const page = vm.createContext({ window: { fetch() {} }, location: { href: 'https://page.invalid/' }, URL, Request })
      const required = new Set()
      vm.runInNewContext(source, {
        require(name) {
          assert.equal(name, 'electron', 'sandboxed preload dependencies remain Electron-only')
          required.add(name)
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
      let fetchReply = null
      let fetchCall = null
      if (page.window.ipc) {
        page.window.ipc.postMessage(42)
        const request = { body: Uint8Array.of(1, 2) }
        fetchReply = await page.window.ipc.fetch(request)
        fetchCall = {
          channel: invoked[0]?.[0], body: Array.from(invoked[0]?.[1]?.body ?? []),
          sameRequest: invoked[0]?.[1] === request, calls: invoked.length,
        }
      }
      frames.push({
        owner, isMainFrame, required: [...required],
        preferences: {
          contextIsolation: preferences.contextIsolation, sandbox: preferences.sandbox,
          nodeIntegration: preferences.nodeIntegration, nodeIntegrationInSubFrames: preferences.nodeIntegrationInSubFrames,
        },
        mainScript: page.mainScript ?? null, everyFrameScript: page.everyFrameScript ?? null,
        hasIpc: Boolean(page.window.ipc), sent, fetchCall, fetchReply,
      })
    }
  }
  const ipc = ipcListeners.get('tauri-ipc')
  const sender = window.webContents
  const before = messages.length
  ipc({ sender, senderFrame: sender.mainFrame }, 'main message')
  const opaque = { url: 'data:text/html,opaque', origin: 'null' }
  const fetchErrors = []
  for (const senderFrame of [null, { ...sender.mainFrame }, opaque]) {
    ipc({ sender, senderFrame }, 'not main')
    try { await ipcHandlers.get('tauri-ipc-fetch')({ sender, senderFrame }, {}); fetchErrors.push(null) }
    catch (error) { fetchErrors.push(error.message) }
  }
  ipc({ sender: { id: 999, mainFrame: sender.mainFrame }, senderFrame: sender.mainFrame }, 'unknown')
  ipc({ sender: { id: sender.id, mainFrame: opaque }, senderFrame: opaque }, 'opaque')
  return {
    frames, fetchErrors, messages: messages.slice(before).filter(message => message.event === 'ipc.message'),
    plainPreload: noHandler.options.webPreferences.preload === path.join(electronDirectory, 'preload.cjs'),
    trafficLightPosition: window.options.trafficLightPosition, tabbingIdentifier: window.options.tabbingIdentifier,
  }
}
function permissions() {
  const decisions = []
  for (const [permission, details, response, contents] of [
    ['media', { mediaTypes: ['audio', 'video'] }, 'allow', window.webContents],
    ['media', { mediaTypes: ['audio'] }, 'deny', child.webContents],
    ['media', undefined, 'default', window.webContents],
    ['geolocation', undefined, true, child.webContents],
    ['notifications', undefined, 'allow', window.webContents],
    ['clipboard-read', undefined, 'allow', child.webContents],
    ['display-capture', undefined, 'allow', window.webContents],
    ['midi', undefined, 'allow', child.webContents],
    ['midiSysex', undefined, 'allow', window.webContents],
    ['sensors', undefined, 'allow', child.webContents],
  ]) {
    const replies = []
    const before = messages.length
    defaultSession.permissionRequest(contents, permission, (reply) => replies.push(reply), details)
    const request = messages[before]?.params ?? null
    const waiting = [...replies]
    if (request) {
      runtime.handle('webview.permissionResponse', { id: request.id, response })
      runtime.handle('webview.permissionResponse', { id: request.id, response: response === 'allow' ? 'deny' : 'allow' })
    }
    decisions.push({ permission, request, waiting, response, replies })
  }
  const bypass = []
  for (const [contents, permission] of [[noHandler.webContents, 'geolocation'], [{ id: 999 }, 'media'], [window.webContents, 'fullscreen']]) {
    const before = messages.length
    const replies = []
    defaultSession.permissionRequest(contents, permission, (reply) => replies.push(reply), {})
    bypass.push({ request: messages[before]?.params ?? null, replies })
  }
  runtime.handle('webview.permissionResponse', { id: 999, response: 'allow' })
  return { decisions, bypass, pending: runtime.state.permissionRequests.size, timers: timers.size }
}
async function sessions() {
  const defaultSelection = runtime.state.sessionFor() ?? null
  runtime.handle('protocol.register', { scheme: 'tauri' })
  const directory = path.join(root, 'profiles', 'shared')
  const shared = runtime.state.sessionFor({ dataDirectory: directory })
  const aliases = [
    runtime.state.sessionFor({ dataDirectory: path.join(directory, '..', 'shared') }).id,
    runtime.state.sessionFor({ dataDirectory: path.relative(process.cwd(), directory) }).id,
  ]
  const separate = runtime.state.sessionFor({ dataDirectory: path.join(root, 'profiles', 'separate') })
  const privateOne = runtime.state.sessionFor({ incognito: true, dataDirectory: directory })
  const privateTwo = runtime.state.sessionFor({ incognito: true, dataDirectory: directory })
  runtime.handle('protocol.register', { scheme: 'asset' })
  runtime.handle('protocol.register', { scheme: 'asset' })
  const future = runtime.state.sessionFor({ dataDirectory: path.join(root, 'profiles', 'future') })
  const otherSession = begin(window.webContents, 'mainFrame', false, shared)
  const sameId = begin(window.webContents, 'mainFrame', false, separate, otherSession.details.id)
  shared.errorOccurred({ id: otherSession.details.id })
  const remaining = [...runtime.state.navigationRequests.keys()]
  answer(sameId, true)
  answer(otherSession, false)
  const targets = []
  for (const target of [defaultSession, shared, separate, privateOne, privateTwo, future]) {
    runtime.state.installPolicyHandlers(target)
    let before = messages.length
    await target.protocol.handlers.get('tauri')(new Request('tauri://localhost/first'))
    const tauri = messages.slice(before).find(message => message.event === 'protocol.request')
    before = messages.length
    await target.protocol.handlers.get('http')(new Request('http://asset.localhost/second'))
    const asset = messages.slice(before).find(message => message.event === 'protocol.request')
    before = messages.length
    const fallback = []
    for (const url of ['https://remote.invalid/', 'http://tauri.localhost:1420/', 'http://unregistered.localhost/']) {
      const request = new Request(url)
      const response = await target.protocol.handlers.get(new URL(url).protocol.slice(0, -1))(request)
      const forwarded = target.forwarded.at(-1)
      fallback.push({
        url, body: await response.text(), sameRequest: forwarded?.request === request,
        receiver: forwarded?.receiver ?? null, options: forwarded?.options ?? null,
      })
    }
    targets.push({
      id: target.id, navigationInstalls: target.navigationInstalls, requestEndInstalls: target.requestEndInstalls,
      permissionInstalls: target.permissionInstalls, downloadListeners: target.listenerCount('will-download'),
      protocols: [...target.protocol.handlers.keys()].sort(), protocolCalls: target.protocolCalls.toSorted(),
      tauri, asset, fallback, forwarded: target.forwarded.length, protocolMessagesDuringFallback: messages.length - before,
    })
  }
  const observed = {
    defaultSelection, shared: shared.id, aliases, separate: separate.id, private: [privateOne.id, privateTwo.id],
    partitions: [...partitionSessions.keys()], pathCalls: pathCalls.map((value) => ({ path: value, absolute: path.isAbsolute(value) })),
    retirement: { remaining, retired: navigationState(otherSession), active: navigationState(sameId) },
    targets, cleanup: cleanup(),
  }
  assert.equal(runtime.state.sessionFor(null), undefined, 'null main webview selects no custom session')
  await runtime.handle('window.create', { windowId: 3, webview: null })
  assert.equal(runtime.windows.get(3).native.contentView.children.length, 0, 'null main webview creates a bare window')
  await runtime.handle('webview.create', { windowId: 3, webviewId: 40, incognito: false, dataDirectory: null })
  assert.equal(runtime.webviews.get(40).contents.session, defaultSession, 'a bare window child uses the default session')
  await runtime.handle('window.create', { windowId: 4, webview: { webviewId: 41, dataDirectory: directory } })
  assert.equal(runtime.webviews.get(41).contents.session, shared)
  let webviewId = 41
  for (const [options, expected] of [
    [{}, shared],
    [{ incognito: false, dataDirectory: null }, shared],
    [{ dataDirectory: path.join(root, 'profiles', 'separate') }, separate],
  ]) {
    await runtime.handle('webview.create', { windowId: 4, webviewId: ++webviewId, ...options })
    assert.equal(runtime.webviews.get(webviewId).contents.session, expected, 'explicit child profile overrides; absent or Rust-default settings inherit')
  }
  const privateChildren = []
  const pathsBeforePrivate = pathCalls.length
  for (let index = 0; index < 2; index++) {
    await runtime.handle('webview.create', { windowId: 4, webviewId: ++webviewId, incognito: true, dataDirectory: directory })
    const selected = runtime.webviews.get(webviewId).contents.session
    for (const other of [defaultSession, shared, separate, ...privateChildren]) assert.notEqual(selected, other, 'explicit incognito never inherits or shares a session')
    privateChildren.push(selected)
  }
  assert.equal(pathCalls.length, pathsBeforePrivate, 'incognito still takes precedence over the child data directory')
  return observed
}
try {
  runtime.state.installPolicyHandlers(defaultSession)
  const scripts = [
    { script: 'globalThis.mainScript = (globalThis.mainScript ?? 0) + 1', mainFrameOnly: true },
    { script: 'globalThis.everyFrameScript = (globalThis.everyFrameScript ?? 0) + 1', mainFrameOnly: false },
  ]
  await runtime.handle('window.create', {
    windowId: 1, trafficLightPosition: { x: 12, y: 18 }, tabbingIdentifier: 'editor-tabs',
    webview: { webviewId: 10, url: 'https://page.invalid/', initScripts: scripts, policy: { navigation: true, permission: true } },
  })
  runtime.state.installPolicyHandlers(defaultSession)
  window = runtime.windows.get(1).native
  await runtime.handle('webview.create', {
    windowId: 1, webviewId: 20, url: 'https://child.invalid/', initScripts: scripts, policy: { navigation: true, permission: true },
  })
  child = runtime.webviews.get(20).view
  await runtime.handle('window.create', { windowId: 2, webview: { webviewId: 30, url: 'https://no-handler.invalid/' } })
  noHandler = runtime.windows.get(2).native
  // Policy cleanup counts request listeners separately from theme/ownership lifecycle listeners.
  for (const contents of [window.webContents, child.webContents]) {
    for (const listener of contents.listeners('destroyed')) lifecycleDestroyedListeners.add(listener)
  }
  const scenarios = { navigation, preload, permissions, sessions }
  const scenario = scenarios[process.argv[2]]
  if (typeof scenario !== 'function') throw new Error(`unknown webview scenario: ${process.argv[2]}`)
  console.log(JSON.stringify(await scenario()))
} finally {
  app.emit('will-quit')
}
