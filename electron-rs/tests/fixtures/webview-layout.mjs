// Observe the real main-process code with native objects replaced by doubles.
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { EventEmitter } from 'node:events'
import { MessageChannel, receiveMessageOnPort } from 'node:worker_threads'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import vm from 'node:vm'

const require = createRequire(import.meta.url)
let nextId = 0
let ready
const timers = new Set()
const tasks = []
const transport = new EventEmitter()
transport.setEncoding = () => {}
transport.write = () => {}
transport.destroy = () => { transport.destroyed = true; transport.emit('close') }
// Run both production IPC branches, with only the native Worker/socket replaced.
// Message framing and main-thread queueing still come from ipc.cjs itself.
const ipcSource = readFileSync(new URL('../../assets/ipc.cjs', import.meta.url), 'utf8')
class SocketWorker extends EventEmitter {
  constructor(_filename, { workerData }) {
    super()
    this.channel = workerData.channel
    vm.runInNewContext(ipcSource, {
      require: (name) => name === 'node:worker_threads' ? { isMainThread: false, workerData }
        : name === 'node:net' ? { connect: () => transport } : require(name),
      process: new EventEmitter(),
    })
  }
  terminate() { this.channel.close(); return Promise.resolve(0) }
}
const ipcModule = { exports: {} }
vm.runInNewContext(ipcSource, {
  require: (name) => name === 'node:worker_threads'
    ? { Worker: SocketWorker, MessageChannel, receiveMessageOnPort, isMainThread: true } : require(name),
  module: ipcModule, __filename: fileURLToPath(new URL('../../assets/ipc.cjs', import.meta.url)),
  setImmediate: (callback) => tasks.push(callback), performance,
})
const allContents = []
const allWindows = []
const faults = { viewConstructor: false, applyWebviewConfig: false, attach: false, show: false, windowConfig: false }
const nativeTheme = Object.assign(new EventEmitter(), {
  themeSource: 'system', shouldUseDarkColors: false, shouldUseHighContrastColors: false,
})
const displays = [
  { id: 1, label: 'primary', scaleFactor: 1,
    bounds: { x: 0, y: 0, width: 1920, height: 1080 },
    workArea: { x: 0, y: 0, width: 1920, height: 1040 } },
  { id: 2, label: 'secondary', scaleFactor: 2,
    bounds: { x: 1920, y: 0, width: 1280, height: 720 },
    workArea: { x: 1920, y: 0, width: 1280, height: 680 } },
]
const screen = Object.assign(new EventEmitter(), {
  getAllDisplays: () => displays,
  getPrimaryDisplay: () => displays[0],
  getDisplayMatching: (bounds) => ({ scaleFactor: bounds.scale }),
  // A secondary display's physical origin differs from DIP-origin * scale.
  getCursorScreenPoint: () => ({ x: 2100, y: 40 }),
  getDisplayNearestPoint: () => ({ scaleFactor: 2 }),
  dipToScreenPoint: ({ x, y }) => ({ x: 1920 + (x - 1920) * 2, y: y * 2 }),
  screenToDipPoint: ({ x, y }) => ({ x: 1920 + (x - 1920) / 2, y: y / 2 }),
  dipToScreenRect: (_window, rect) => ({
    x: 1920 + (rect.x - 1920) * 2, y: rect.y * 2,
    width: rect.width * 2, height: rect.height * 2,
  }),
})
class Contents extends EventEmitter {
  id = ++nextId
  destroyed = false
  calls = []
  inserted = []
  removed = []
  styles = new Map()
  nextStyle = 0
  mainFrame = { executeJavaScript: (script) => this.executeJavaScript(script) }
  debugger = Object.assign(new EventEmitter(), {
    attached: false,
    commands: [],
    isAttached: () => this.debugger.attached,
    attach: () => { this.debugger.attached = true },
    detach: () => { this.debugger.attached = false; this.debugger.emit('detach') },
    sendCommand: (method, params) => {
      if (!this.debugger.attached) return Promise.reject(new Error('debugger is not attached'))
      this.debugger.commands.push({ method, params })
      if (method === 'Emulation.setEmulatedMedia') {
        this.emulatedTheme = params.features.find((feature) => feature.name === 'prefers-color-scheme')?.value
      }
      return Promise.resolve()
    },
  })
  isDestroyed() { return this.destroyed }
  setWindowOpenHandler(handler) { this.openHandler = handler }
  setUserAgent(value) {
    if (faults.applyWebviewConfig) throw new Error('webview config failed')
    this.calls.push({ method: 'setUserAgent', value }); this.userAgent = value
  }
  loadURL(url) { this.calls.push({ method: 'loadURL', value: url }); this.url = url; return Promise.resolve() }
  getURL() { return this.url ?? '' }
  getLastWebPreferences() { return {} }
  executeJavaScript(script) {
    this.script = script
    // Electron waits for did-stop-loading when a webContents has never loaded.
    if (!this.getURL()) return new Promise(() => {})
    if (script.includes('matchMedia(')) {
      this.mediaQueries = (this.mediaQueries ?? 0) + 1
      return Promise.resolve(this.pageDark ?? (this.emulatedTheme
        ? this.emulatedTheme === 'dark' : nativeTheme.shouldUseDarkColors))
    }
    return Promise.resolve(this.id)
  }
  insertCSS(css, options) {
    if (!this.getURL()) return Promise.reject(new Error('no render frame'))
    const key = `css-${++this.nextStyle}`
    const rule = { css, ...options }
    this.inserted.push(rule)
    this.styles.set(key, rule)
    return Promise.resolve(key)
  }
  removeInsertedCSS(key) { this.removed.push(key); this.styles.delete(key); return Promise.resolve() }
  navigate(url) { this.url = url; this.styles.clear(); this.emit('dom-ready') }
  reload() { this.reloaded = true }
  focus() { this.focused = true }
  setZoomFactor(value) { this.zoom = value }
  openDevTools() { this.devtools = true; this.devToolsWebContents = {}; this.emit('devtools-opened') }
  closeDevTools() { this.devtools = false; this.devToolsWebContents = null; this.emit('devtools-closed') }
  isDevToolsOpened() { return this.devtools ?? false }
  close() { this.destroyed = true; this.emit('destroyed') }
  constructor() { super(); allContents.push(this) }
}
class View {
  visible = true
  constructor(options = {}) {
    if (faults.viewConstructor) throw new Error('view constructor failed')
    this.options = options
    this.webContents = options.webContents ?? new Contents()
    this.bounds = { x: 0, y: 0, width: 0, height: 0 }
  }
  setBounds(bounds) { this.bounds = { ...bounds } }
  getBounds() { return { ...this.bounds } }
  setVisible(value) { this.visible = value }
  getVisible() { return this.visible }
  setBackgroundColor(value) { this.color = value }
}
class Window extends EventEmitter {
  children = []
  visible = true
  destroyed = false
  scale = 2
  frame = { width: 0, height: 0 }
  parentCalls = []
  nativeCalls = []
  contentView = {
    addChildView: (view) => {
      if (faults.attach) throw new Error('attach failed')
      this.children.push(view)
    },
    removeChildView: (view) => { this.children = this.children.filter((item) => item !== view) },
  }
  constructor(options) {
    super()
    allWindows.push(this)
    this.options = options
    this.content = { x: -200, y: 100, width: options.width ?? 800, height: options.height ?? 600 }
    this.maximizable = options.maximizable ?? true
    this.minimizable = options.minimizable ?? true
    this.closable = options.closable ?? true
  }
  getBackgroundColor() { return this.color ?? this.options.backgroundColor ?? '#ffffff' }
  setBackgroundColor(color) { this.color = color }
  setBackgroundMaterial(material) { this.material = material; this.color = material === 'none' ? '#ffffff' : '#00000000' }
  getNormalBounds() { return this.getBounds() }
  getContentBounds() { return { ...this.content } }
  getContentSize() { return [this.content.width, this.content.height] }
  getBounds() { return { ...this.content, width: this.content.width + this.frame.width,
    height: this.content.height + this.frame.height, scale: this.scale } }
  setBounds(bounds) {
    if (bounds.x !== undefined) this.content.x = bounds.x
    if (bounds.y !== undefined) this.content.y = bounds.y
    if (bounds.width !== undefined) this.content.width = bounds.width - this.frame.width
    if (bounds.height !== undefined) this.content.height = bounds.height - this.frame.height
  }
  setContentSize(width, height) { this.content.width = width; this.content.height = height }
  setMinimumSize(width, height) { this.minimum = [width, height] }
  setMaximumSize(width, height) { this.maximum = [width, height] }
  setPosition(x, y) { this.content.x = x; this.content.y = y }
  setParentWindow(parent) { this.parent = parent; this.parentCalls.push(parent?.options.title ?? null) }
  setMenu(menu) { this.menu = menu; this.frame.height = menu ? 58 : 38 }
  setAutoHideMenuBar(value) { this.autoHideMenuBar = value }
  setMenuBarVisibility(value) { this.menuBarVisible = value }
  isDestroyed() { return this.destroyed }
  isVisible() { return this.visible }
  isMaximized() { return false }
  isMinimized() { return false }
  isFocused() { return true }
  isFullScreen() { return false }
  isSimpleFullScreen() { return false }
  isResizable() { return true }
  isMaximizable() { return this.maximizable }
  isMinimizable() { return this.minimizable }
  isClosable() { return this.closable }
  setMaximizable(value) { this.nativeCalls.push({ method: 'setMaximizable', value }); this.maximizable = value }
  setMinimizable(value) { this.nativeCalls.push({ method: 'setMinimizable', value }); this.minimizable = value }
  setClosable(value) { this.nativeCalls.push({ method: 'setClosable', value }); this.closable = value }
  isAlwaysOnTop() { return false }
  getTitle() { return this.options.title ?? '' }
  focus() { this.focused = true }
  show() {
    if (faults.show) throw new Error('window show failed')
    this.visible = true
  }
  showInactive() { this.visible = true; this.focused = false }
  hide() { this.visible = false }
  close() {
    this.closeRequested = true
    this.lastClosePrevented = false
    this.emit('close', { preventDefault: () => { this.lastClosePrevented = true } })
    if (!this.lastClosePrevented) this.destroy()
  }
  destroy() { this.destroyed = true; this.emit('closed') }
  resize(width, height) { this.content.width = width; this.content.height = height; this.emit('resize') }
}
const context = vm.createContext({
  require: (name) => name === 'electron' ? {
    app: { on() {}, quit() {}, whenReady: () => new Promise((resolve) => { ready = resolve }) },
    Menu: { setApplicationMenu() {}, buildFromTemplate: (template) => ({ template }) },
    nativeTheme, screen,
    session: { defaultSession: { on() {}, setPermissionRequestHandler() {}, webRequest: { onCompleted() {}, onErrorOccurred() {}, onBeforeRequest() {} } } },
    BaseWindow: Window, WebContentsView: View,
    ipcMain: { on() {}, handle() {} }, protocol: { registerSchemesAsPrivileged() {} },
  } : name === 'node:net' ? { connect: () => transport }
    : name === 'node:fs' ? { appendFileSync() {}, readFileSync(fd) { assert.equal(fd, 0); return '0123456789abcdef'.repeat(4) + '\n' } }
    : name === 'node:os' ? { tmpdir: () => '.', release: () => '10.0.22631' }
      : name === './ipc.cjs' ? ipcModule.exports : require(name),
  process: { argv: ['node', 'main.cjs', '--port', '1'], env: {}, platform: 'win32' },
  Buffer, URL, console,
  setTimeout(callback, delay) { const timer = { callback, delay }; timers.add(timer); return timer },
  clearTimeout: (timer) => timers.delete(timer),
  setImmediate: (callback) => tasks.push(callback),
  __dirname: fileURLToPath(new URL('../../assets/', import.meta.url)),
})
vm.runInContext(readFileSync(new URL('../../assets/main.cjs', import.meta.url), 'utf8'), context)
const inspect = (expression) => vm.runInContext(expression, context)
const call = (method, params) => context.handle(method, params)
const errorFrom = async (action) => { try { await action(); return null } catch (error) { return error.message } }
const settle = async () => {
  await Promise.resolve()
  while (tasks.length) await tasks.shift()()
  await new Promise(setImmediate)
}
const mainViewOf = (window) => inspect('mainViews').get(window)?.view
const mainContentsOf = (window) => context.mainContents(window)
const cursorStyles = (contents) => [...(contents?.styles.values() ?? [])]
const constraintsOf = (window) => ({ min: window.minimum, max: window.maximum })
const preferencesOf = (options) => {
  const { javascript, devTools, backgroundThrottling, userAgent, webviewTag } = options.webPreferences
  return { javascript, devTools, backgroundThrottling, userAgent: userAgent ?? null, webviewTag }
}
const createHost = async (main = false) => {
  await call('window.create', main
    ? { windowId: 2, width: 640, height: 480, webview: { webviewId: 20, url: 'tauri://localhost/main' } }
    : { windowId: 1, width: 800, height: 600 })
  return inspect(`windows.get(${main ? 2 : 1})`)
}
const createChildren = async () => {
  const host = await createHost()
  const urls = ['tauri://localhost/', 'https://github.com/tauri-apps/tauri', 'https://tauri.app', 'https://twitter.com/TauriApps']
  for (let index = 0; index < urls.length; index++) {
    await call('webview.create', {
      windowId: 1, webviewId: index + 10, url: urls[index], autoResize: true,
      bounds: { position: { x: index % 2 * 400, y: Math.floor(index / 2) * 300, physical: false },
        size: { width: 400, height: 300, physical: false } },
      policy: { permission: true },
    })
  }
  return host
}

let result
switch (process.argv[2]) {
  case 'geometry': {
    const host = await createHost()
    const cursor = await call('window.cursorPosition', {})
    await call('window.position', { windowId: 1, physical: true, x: 2280, y: 80 })
    result = {
      useContentSize: host.options.useContentSize, cursor, nativePosition: host.getContentBounds(),
      geometry: await call('window.geometry', { windowId: 1 }),
      monitor: inspect(`readDisplay({ scaleFactor: 2,
        bounds: { x: 1920, y: 0, width: 1280, height: 720 },
        workArea: { x: 1920, y: 0, width: 1280, height: 680 } })`),
    }
    break
  }
  case 'monitors': {
    // Containment and DIP conversions are unit checks with injected displays,
    // never runtime-evidence for a real monitor/API implementation.
    screen.dipToScreenRect = (_window, rect) => rect.x < 1920 ? { ...rect }
      : { x: 1920 + (rect.x - 1920) * 2, y: rect.y * 2, width: rect.width * 2, height: rect.height * 2 }
    const query = async (points) => Promise.all(points.map(async ([x, y]) =>
      (await call('window.displayForPoint', { x, y }))?.name ?? null))
    const physical = await query([[0, 0], [1919, 1079], [1920, 0], [4479, 1439],
      [4480, 0], [1920, 1440], [-1, 0], [900, 1080]])
    const secondary = await call('window.displayForPoint', { x: 4000, y: 1000 })
    const invalid = await errorFrom(() => call('window.displayForPoint', { x: NaN, y: 0 }))
    const logical = {}
    for (const platform of ['darwin', 'linux']) {
      context.process.platform = platform
      logical[platform] = await query([[0, 0], [1919, 1079], [1920, 0], [3199, 719],
        [3200, 0], [2000, 720], [-1, 0], [900, 1080]])
    }
    result = { physical, secondary, logical, invalid }
    const contained = ['primary', 'primary', 'secondary', 'secondary', null, null, null, null]
    assert.deepEqual(physical, contained)
    for (const platform of ['darwin', 'linux']) assert.deepEqual(logical[platform], contained)
    assert.equal(secondary.name, 'secondary')
    assert.equal(secondary.position.x, 1920)
    assert.equal(secondary.size.width, 2560)
    assert.equal(invalid, 'invalid monitor point')
    break
  }
  case 'native-state': {
    const host = await createHost()
    // Force disagreement with requested/default WeakMap flags. Queries must
    // inspect native getters, not echo the last request.
    host.maximizable = host.minimizable = host.closable = false
    const flags = async () => {
      const { maximizable, minimizable, closable } = await call('window.geometry', { windowId: 1 })
      return { maximizable, minimizable, closable }
    }
    const contrary = await flags()
    const states = []
    const requests = [false, true, false, true]
    for (const wanted of requests) {
      await call('window.closable', { windowId: 1, closable: wanted })
      await call('window.maximizable', { windowId: 1, maximizable: wanted })
      await call('window.minimizable', { windowId: 1, minimizable: wanted })
      states.push(await flags())
    }
    host.maximizable = host.minimizable = host.closable = false
    result = { contrary, states, afterNativeChange: await flags(), calls: host.nativeCalls }
    const disabled = { maximizable: false, minimizable: false, closable: false }
    assert.deepEqual(contrary, disabled)
    assert.deepEqual(result.afterNativeChange, disabled)
    assert.deepEqual(states, requests.map((value) => ({ maximizable: value, minimizable: value, closable: value })))
    assert.deepEqual(host.nativeCalls, requests.flatMap((value) => [
      { method: 'setClosable', value }, { method: 'setMaximizable', value }, { method: 'setMinimizable', value },
    ]))
    break
  }
  case 'layout': {
    const host = await createChildren()
    const children = []
    for (const [index, view] of host.children.entries()) {
      children.push({ contentsId: view.webContents.id, url: view.webContents.url,
        preferences: { ...view.options.webPreferences, preload: path.basename(view.options.webPreferences.preload) },
        owner: inspect(`webviewByContents.get(${view.webContents.id})`),
        bounds: (await call('webview.bounds', { webviewId: index + 10 })).bounds })
    }
    host.resize(1001, 701)
    const oddBounds = host.children.map((view) => view.getBounds())
    host.resize(800, 600)
    result = { hostUrl: mainContentsOf(host)?.getURL() ?? null, hostHasWebContents: 'webContents' in host,
      contentsCount: allContents.length, children, oddBounds,
      resetBounds: host.children.map((view) => view.getBounds()) }
    break
  }
  case 'bounds': {
    const host = await createChildren()
    const first = host.children[0]
    const steps = []
    for (const bounds of [
      { position: { x: -30, y: 20, physical: false }, size: { width: 400, height: 200, physical: true } },
      { size: { width: 310, height: 190, physical: false } },
      { position: { x: 120, y: 80, physical: true } },
      { position: { x: 100, y: 50, physical: false }, size: { width: 200, height: 100, physical: false } },
    ]) {
      await call('webview.setBounds', { webviewId: 10, ...bounds })
      steps.push(first.getBounds())
    }
    const physical = (await call('webview.bounds', { webviewId: 10 })).bounds
    const errors = []
    for (const bounds of [{ size: { width: -1, height: 10 } }, { position: { x: NaN, y: 10 } }]) {
      errors.push(await errorFrom(() => call('webview.setBounds', { webviewId: 10, ...bounds })))
    }
    host.resize(1600, 1200)
    const manualResize = first.getBounds()
    await call('webview.setAutoResize', { webviewId: 10, autoResize: false })
    host.resize(800, 600)
    const disabledResize = first.getBounds()
    await call('webview.setAutoResize', { webviewId: 10, autoResize: true })
    host.resize(1600, 1200)
    result = { steps, physical, errors, manualResize, disabledResize, enabledResize: first.getBounds() }
    break
  }
  case 'dispatch': {
    const host = await createChildren()
    const first = host.children[0]
    await call('webview.eval', { webviewId: 10, script: 'child-script' })
    const evalScripts = { child: first.webContents.script, host: mainContentsOf(host)?.script ?? null }
    const cursors = []
    for (const cursor of ['pointer', 'none', 'default']) {
      await call('window.cursor', { windowId: 1, cursor })
      cursors.push({ host: cursorStyles(mainContentsOf(host)),
        children: host.children.map((view) => cursorStyles(view.webContents)) })
    }
    await call('webview.reload', { webviewId: 10 })
    await call('webview.setZoom', { webviewId: 10, scaleFactor: 1.25 })
    await call('webview.visibility', { webviewId: 10, visible: false })
    const hidden = { child: (await call('webview.bounds', { webviewId: 10 })).visible, host: host.visible }
    await call('webview.visibility', { webviewId: 10, visible: true })
    await call('webview.focus', { webviewId: 10 })
    await call('webview.backgroundColor', { webviewId: 10, color: '#123456' })
    const child = { reloaded: first.webContents.reloaded, zoom: first.webContents.zoom,
      focused: first.webContents.focused, visible: first.visible, color: first.color }
    await call('webview.close', { webviewId: 10 })
    const closed = { destroyed: first.webContents.isDestroyed(), children: host.children.length,
      mapped: inspect(`webviewByContents.has(${first.webContents.id})`),
      error: await errorFrom(() => call('webview.eval', { webviewId: 10, script: '' })) }
    const remaining = [...host.children]
    host.destroy()
    result = { evalScripts, cursors, hidden, child, closed,
      remainingDestroyed: remaining.map((view) => view.webContents.isDestroyed()),
      childWebviews: inspect('childWebviews.size'), webviews: inspect('webviews.size'), policies: inspect('handlerPolicy.size') }
    break
  }
  case 'devtools': {
    const host = await createChildren()
    const contents = host.children[0].webContents
    const lifecycleEvents = ['devtools-opened', 'devtools-closed', 'destroyed']
    const initialListeners = lifecycleEvents.map((event) => contents.listenerCount(event))
    let openCalls = 0
    contents.openDevTools = () => { openCalls++; contents.devtools = true }
    let opened = false
    const opening = call('webview.openDevtools', { webviewId: 10 })
    opening.then(() => { opened = true })
    const before = { samePending: call('webview.openDevtools', { webviewId: 10 }) === opening,
      visible: await call('webview.isDevtoolsOpen', { webviewId: 10 }), opened, openCalls }
    contents.devToolsWebContents = {}
    contents.emit('devtools-opened')
    await opening
    await call('webview.openDevtools', { webviewId: 10 })
    const after = { opened, openCalls,
      listeners: lifecycleEvents.map((event, index) => contents.listenerCount(event) - initialListeners[index]) }
    const destroyed = new Contents()
    destroyed.openDevTools = () => {}
    const interrupted = context.openDevtools(destroyed)
    destroyed.emit('destroyed')
    result = { before, after, destroyedError: await errorFrom(() => interrupted),
      destroyedPending: inspect('openingDevtools').has(destroyed), timers: timers.size }
    break
  }
  case 'rpc': {
    const host = await createChildren()
    await settle()
    const contents = host.children[0].webContents
    contents.openDevTools()
    const replies = []
    context.send = (message) => replies.push(message)
    const message = (id, method) => ({ id, method, params: { webviewId: 10 } })
    context.onRequest(message(1, 'webview.closeDevtools'))
    const closedSynchronously = contents.devtools
    context.onRequest(message(2, 'webview.isDevtoolsOpen'))
    context.onRequest({ id: 3, method: 'missing' })
    const execute = contents.executeJavaScript
    contents.executeJavaScript = () => Promise.reject(new Error('eval rejected'))
    context.onRequest({ id: 4, method: 'webview.eval', params: { webviewId: 10, script: '' } })
    await settle()
    contents.executeJavaScript = execute
    context.onRequest({ id: 5, method: 'webview.eval', params: { webviewId: 10, script: 'rpc-script' } })
    await settle()
    const directReplies = replies.splice(0)
    context.onRequest({ method: 'webview.closeDevtools' })
    const invalidReplies = replies.splice(0)
    ready()
    await settle()
    let closeCalls = 0
    let reading = false
    const closeDuringRead = []
    contents.closeDevTools = () => {
      closeDuringRead.push(reading)
      closeCalls++
      transport.emit('data', `${JSON.stringify({ id: 8, method: 'webview.isDevtoolsOpen', params: { webviewId: 10 } })}\n`)
    }
    const request = (id, method) => JSON.stringify({ id, method, params: { webviewId: 10 } })
    reading = true
    transport.emit('data', `${request(6, 'webview.closeDevtools')}\n${request(7, 'webview.isDevtoolsOpen')}`)
    transport.emit('data', '\n')
    reading = false
    const beforeTasks = { closeCalls, replies: replies.length }
    await settle()
    await settle()
    inspect('socket').close()
    result = { closedSynchronously, directReplies, invalidReplies, contentsId: contents.id,
      beforeTasks, closeCalls, closeDuringRead, framedReplies: replies }
    break
  }
  case 'main-window': {
    const host = await createChildren()
    const first = host.children[0]
    await call('window.cursor', { windowId: 1, cursor: 'default' })
    const unrelatedCursor = cursorStyles(first.webContents)
    await call('webview.close', { webviewId: 10 })
    host.destroy()
    const main = await createHost(true)
    const view = mainViewOf(main)
    const contents = mainContentsOf(main)
    const addChildView = main.contentView.addChildView
    main.contentView.addChildView = () => { throw new Error('attach failed') }
    const attachError = await errorFrom(() => call('webview.create', { windowId: 2, webviewId: 21, url: 'tauri://localhost/' }))
    main.contentView.addChildView = addChildView
    const evalResult = await call('webview.eval', { webviewId: 20, script: 'main-script' })
    await call('window.cursor', { windowId: 2, cursor: 'crosshair' })
    await call('webview.visibility', { webviewId: 20, visible: false })
    const visible = view.visible
    const hostVisible = main.visible
    await call('webview.close', { webviewId: 20 })
    result = { attachError, failedContentsDestroyed: allContents.at(-1).isDestroyed(),
      failedChildMapped: inspect('childWebviews.has(21) || webviews.has(21)'), children: main.children.length,
      url: contents.url, evalResult, contentsId: contents.id,
      script: contents.script, cursor: cursorStyles(contents), unrelatedCursor,
      visible, hostVisible, closeRequested: main.closeRequested ?? false,
      contentsDestroyed: contents.isDestroyed(), mainMapped: inspect('childWebviews.has(20) || webviews.has(20)'),
      hostAlive: !main.isDestroyed(), hostHasWebContents: 'webContents' in main }
    break
  }
  case 'effects': {
    const main = await createHost(true)
    await call('window.backgroundColor', { windowId: 2, color: '#00000000' })
    await call('window.backgroundMaterial', { windowId: 2, material: 'none' })
    const initialClear = main.getBackgroundColor()
    const materials = []
    for (const material of ['acrylic', 'mica', 'tabbed']) {
      await call('window.backgroundMaterial', { windowId: 2, material })
      materials.push({ material: main.material, color: main.getBackgroundColor() })
    }
    await call('window.backgroundMaterial', { windowId: 2, material: 'none' })
    const cleared = main.getBackgroundColor()
    await call('window.backgroundMaterial', { windowId: 2, material: 'acrylic' })
    await call('window.backgroundColor', { windowId: 2, color: '#123456' })
    await call('window.backgroundMaterial', { windowId: 2, material: 'none' })
    const changed = main.getBackgroundColor()
    const invalid = await errorFrom(() => call('window.backgroundMaterial', { windowId: 2, material: 'blur' }))
    context.process.platform = 'linux'
    result = { initialClear, materials, cleared, changed, invalid,
      unsupported: await errorFrom(() => call('window.backgroundMaterial', { windowId: 2, material: 'acrylic' })) }
    break
  }
  case 'configuration': {
    await call('window.create', { windowId: 30, title: 'ignored flat title', width: 999,
      window: { title: 'Nested window', width: 640, height: 480, acceptFirstMouse: true,
        backgroundColor: [10, 20, 30, 128], titleBarStyle: 'overlay',
        userAgent: 'config-agent', javascriptDisabled: false, devtools: true, backgroundThrottling: 'enabled' },
      webview: { webviewId: 300, url: 'tauri://localhost/main', userAgent: 'main-agent',
        javascriptDisabled: true, devtools: false, backgroundThrottling: 'disabled' } })
    const main = inspect('windows.get(30)')
    await call('webview.create', { windowId: 30, webviewId: 301, url: 'tauri://localhost/child',
      userAgent: 'child-agent', javascriptDisabled: true, devtools: false, backgroundThrottling: 'disabled' })
    const child = inspect('childWebviews.get(301).view')
    await call('webview.create', { windowId: 30, webviewId: 302, url: 'tauri://localhost/defaults' })
    await call('window.create', { windowId: 31,
      window: { userAgent: 'fallback-agent', javascriptDisabled: true, devtools: false, backgroundThrottling: 'disabled' },
      webview: { webviewId: 310, url: 'tauri://localhost/fallback', userAgent: null,
        javascriptDisabled: null, devtools: null, backgroundThrottling: null } })
    const fallback = inspect('windows.get(31)')
    await call('window.create', { windowId: 32, title: 'Flat window', width: 360, height: 240,
      acceptFirstMouse: true, backgroundColor: '#123456', titleBarHidden: true })
    const flat = inspect('windows.get(32)')
    const titleStyles = []
    for (const [index, config] of [{ titleBarStyle: 'transparent' }, { hiddenTitle: true }, {}].entries()) {
      await call('window.create', { windowId: 33 + index, window: config })
      titleStyles.push(inspect(`windows.get(${33 + index}).options.titleBarStyle`))
    }
    result = {
      nested: { title: main.options.title, width: main.options.width, height: main.options.height,
        useContentSize: main.options.useContentSize, acceptFirstMouse: main.options.acceptFirstMouse,
        backgroundColor: main.options.backgroundColor, titleBarStyle: main.options.titleBarStyle },
      main: { preferences: preferencesOf(mainViewOf(main).options), calls: mainContentsOf(main).calls },
      child: { preferences: preferencesOf(child.options), calls: child.webContents.calls },
      defaults: preferencesOf(inspect('childWebviews.get(302).view.options')),
      fallback: { preferences: preferencesOf(mainViewOf(fallback).options), calls: mainContentsOf(fallback).calls },
      flat: { title: flat.options.title, width: flat.options.width, height: flat.options.height,
        acceptFirstMouse: flat.options.acceptFirstMouse, backgroundColor: flat.options.backgroundColor,
        titleBarStyle: flat.options.titleBarStyle }, titleStyles,
    }
    break
  }
  case 'constraints': {
    await call('window.create', { windowId: 40,
      window: { width: 800, height: 600, minWidth: 240, minHeight: 180, maxWidth: 1000, maxHeight: 800 } })
    const host = inspect('windows.get(40)')
    const creation = constraintsOf(host)
    host.frame = { width: 16, height: 38 }
    host.emit('resize')
    await settle()
    const framed = constraintsOf(host)
    await call('window.size', { windowId: 40, width: 100, height: 2000 })
    const clamped = host.getContentSize()
    await call('window.minSize', { windowId: 40, height: 300, physical: false })
    const heightOnlyMin = constraintsOf(host)
    await call('window.maxSize', { windowId: 40, height: 900, physical: true })
    const heightOnlyMax = constraintsOf(host)
    await call('window.sizeConstraints', { windowId: 40,
      minWidth: { value: 300, physical: false }, minHeight: { value: 400, physical: true },
      maxWidth: { value: 1000, physical: true }, maxHeight: { value: 800, physical: true } })
    const atomic = constraintsOf(host)
    const nativeConstraints = (await call('window.geometry', { windowId: 40 })).contentConstraints
    await call('window.size', { windowId: 40, width: 2400, height: 100, physical: true })
    const physicalClamped = host.getContentSize()
    await call('window.setMenu', { windowId: 40, menu: [{ label: 'File', enabled: true, commandId: 1 }] })
    await settle()
    const menu = constraintsOf(host)
    await call('window.setMenu', { windowId: 40, menu: null })
    await settle()
    const menuRemoved = constraintsOf(host)
    host.frame = { width: 0, height: 0 }
    await call('window.decorations', { windowId: 40, value: false })
    await settle()
    const undecorated = constraintsOf(host)
    host.frame = { width: 16, height: 38 }
    await call('window.decorations', { windowId: 40, value: true })
    await settle()
    const decorated = constraintsOf(host)
    host.scale = 1.25
    host.frame = { width: 12, height: 30 }
    screen.emit('display-metrics-changed', {}, { scaleFactor: 1.25 }, ['scaleFactor'])
    await settle()
    const dpi = constraintsOf(host)
    const nativeDpi = await call('window.geometry', { windowId: 40 })
    await call('window.sizeConstraints', { windowId: 40,
      minWidth: null, minHeight: null, maxWidth: null, maxHeight: null })
    const cleared = constraintsOf(host)
    await call('window.minSize', { windowId: 40, height: 120, physical: false })
    await call('window.maxSize', { windowId: 40, height: 300, physical: false })
    const restoredHeightOnly = constraintsOf(host)
    await call('window.minSize', { windowId: 40 })
    await call('window.maxSize', { windowId: 40 })
    result = { creation, framed, clamped, heightOnlyMin, heightOnlyMax, atomic, physicalClamped,
      menu, menuRemoved, undecorated, decorated, dpi, cleared, restoredHeightOnly, nativeConstraints,
      nativeDpi: { scaleFactor: nativeDpi.scaleFactor, constraints: nativeDpi.contentConstraints },
      nativeCleared: (await call('window.geometry', { windowId: 40 })).contentConstraints,
      legacyCleared: constraintsOf(host) }
    break
  }
  case 'cursors': {
    const host = await createChildren()
    const other = await createHost(true)
    const first = host.children[0].webContents
    await call('window.setCursorIcon', { windowId: 1, cursor: 'crosshair' })
    const chosen = host.children.map((view) => cursorStyles(view.webContents))
    await call('window.setCursorVisible', { windowId: 1, visible: false })
    const hidden = host.children.map((view) => cursorStyles(view.webContents))
    await call('window.setCursorIcon', { windowId: 1, cursor: 'pointer' })
    const chosenWhileHidden = host.children.map((view) => cursorStyles(view.webContents))
    await call('webview.create', { windowId: 1, webviewId: 14, url: 'tauri://localhost/new' })
    host.children[4].webContents.emit('dom-ready')
    await settle()
    const inheritedHidden = cursorStyles(host.children[4].webContents)
    await call('window.setCursorVisible', { windowId: 1, visible: true })
    const visible = host.children.map((view) => cursorStyles(view.webContents))
    first.navigate('tauri://localhost/reloaded')
    await settle()
    const restored = cursorStyles(first)
    await call('webview.create', { windowId: 1, webviewId: 15, url: 'tauri://localhost/later' })
    host.children[5].webContents.emit('dom-ready')
    await settle()
    const inheritedVisible = cursorStyles(host.children[5].webContents)
    await call('window.cursor', { windowId: 1, cursor: 'wait' })
    const legacy = host.children.map((view) => cursorStyles(view.webContents))
    result = { chosen, hidden, chosenWhileHidden, inheritedHidden, visible, restored, inheritedVisible, legacy,
      host: mainContentsOf(host)?.inserted ?? [], other: mainContentsOf(other).inserted, removed: first.removed.length }
    break
  }
  case 'parent-menu': {
    await call('window.create', { windowId: 50, title: 'Parent' })
    await call('window.create', { windowId: 51, title: 'Child' })
    const host = inspect('windows.get(51)')
    await call('window.setParentWindow', { windowId: 51, parentId: 50 })
    const attached = host.parent === inspect('windows.get(50)')
    await call('window.setParentWindow', { windowId: 51, parentId: null })
    const detached = host.parent === null
    const unknown = await errorFrom(() => call('window.setParentWindow', { windowId: 51, parentId: 999 }))
    const self = await errorFrom(() => call('window.setParentWindow', { windowId: 51, parentId: 51 }))
    const events = []
    context.send = (message) => events.push(message)
    await call('window.setMenu', { windowId: 51, menu: [{ label: 'File', enabled: true, submenu: [
      { label: 'Open', enabled: true, accelerator: 'CmdOrCtrl+O', commandId: 71 },
      { label: 'Tools', enabled: false, submenu: [
        { label: 'Pinned', enabled: true, checked: true, commandId: 72 },
        { type: 'separator' }, { label: 'Disabled', enabled: false, checked: false, commandId: 73 },
      ] },
    ] }] })
    const file = host.menu.template[0]
    const open = file.submenu[0]
    const pinned = file.submenu[1].submenu[0]
    const template = JSON.parse(JSON.stringify(host.menu.template))
    open.click()
    pinned.click()
    const clicks = events.splice(0)
    const visibility = { autoHide: host.autoHideMenuBar, visible: host.menuBarVisible }
    await call('window.setMenu', { windowId: 51, menu: null })
    const cleared = host.menu === null
    host.destroy()
    events.length = 0
    open.click()
    pinned.click()
    result = { attached, detached, unknown, self, parentCalls: host.parentCalls,
      template, clicks, visibility, cleared, afterClose: events }
    break
  }
  case 'themes': {
    const host = await createChildren()
    const main = await createHost(true)
    await settle()
    const beforeListeners = nativeTheme.listenerCount('updated')
    await call('window.theme', { windowId: 1, theme: 'dark' })
    await call('window.theme', { windowId: 2, theme: 'dark' })
    const dark = await call('window.themeQuery', { windowId: 2 })
    await call('window.theme', { windowId: 2, theme: 'light' })
    const light = await call('window.themeQuery', { windowId: 2 })
    const children = host.children.map((view) => view.webContents.debugger.commands.at(-1))
    const mainCommand = mainContentsOf(main).debugger.commands.at(-1)
    mainContentsOf(main).pageDark = true
    const pageQuery = await call('window.themeQuery', { windowId: 2 })
    const pageQueries = mainContentsOf(main).mediaQueries
    delete mainContentsOf(main).pageDark
    await call('webview.create', { windowId: 1, webviewId: 14, url: 'tauri://localhost/themed' })
    await settle()
    const inherited = host.children[4].webContents.debugger.commands.at(-1)
    const inheritedDark = host.children[4].webContents.emulatedTheme
    await call('window.theme', { windowId: 1, theme: null })
    const cleared = host.children.map((view) => ({
      command: view.webContents.debugger.commands.at(-1), attached: view.webContents.debugger.isAttached(),
    }))
    const invalid = await errorFrom(() => call('window.theme', { windowId: 1, theme: 'sepia' }))
    nativeTheme.shouldUseDarkColors = true
    nativeTheme.emit('updated')
    await settle()
    const system = await call('window.themeQuery', { windowId: 1 })
    const stillLight = await call('window.themeQuery', { windowId: 2 })
    const events = []
    context.send = (message) => events.push(message)
    host.destroy()
    const afterFirstClose = nativeTheme.listenerCount('updated')
    main.destroy()
    const afterClose = nativeTheme.listenerCount('updated')
    await settle()
    events.length = 0
    nativeTheme.emit('updated')
    await settle()
    result = { beforeListeners, dark, light, children, mainCommand, pageQuery, pageQueries,
      inherited, inheritedDark, cleared, invalid, system, stillLight, nativeSource: nativeTheme.themeSource,
      afterFirstClose, afterClose, afterCloseEvents: events }
    break
  }
  case 'native-policy': {
    inspect('socket = {closed:false}')
    const events = []
    context.send = (message) => {
      if (inspect('socket && !socket.closed')) events.push(JSON.parse(JSON.stringify(message)))
    }
    const main = await createHost(true)
    const mainView = mainViewOf(main)
    await call('webview.create', { windowId: 2, webviewId: 21, url: 'about:blank' })
    const child = inspect('childWebviews.get(21).view')
    const roots = {
      mainOptions: main.options.webPreferences.webviewTag,
      mainView: mainView.options.webPreferences.webviewTag,
      child: child.options.webPreferences.webviewTag,
    }

    inspect('activePopupRequest = 7')
    inspect('activePopupContents = {session:undefined}')
    await call('window.create', {
      windowId: 3, newWindowRequest: 7,
      webview: { webviewId: 30, url: 'about:blank' },
    })
    const staged = inspect('pendingPopups.get(7)')
    inspect('pendingPopups.delete(7)')
    staged.attach(new Contents())
    const popup = staged.window
    const view = mainViewOf(popup)
    const guest = view.webContents
    Object.assign(roots, {
      popupOptions: popup.options.webPreferences.webviewTag,
      popupView: view.options.webPreferences.webviewTag,
    })
    Object.defineProperty(view, 'webContents', {
      get: () => guest.isDestroyed() ? undefined : guest,
    })
    assert.deepEqual(roots, {
      mainOptions: false, mainView: false, child: false,
      popupOptions: false, popupView: false,
    })
    events.length = 0
    assert.doesNotThrow(() => guest.close())
    const nativeDestroy = {
      guestDestroyed: guest.isDestroyed(),
      viewContentsCleared: view.webContents === undefined,
      shellDestroyed: popup.isDestroyed(),
      windowMapped: inspect('windows.has(3)'),
      childMapped: inspect('childWebviews.has(30)'),
      webviewMapped: inspect('webviews.has(30)'),
      mainMapped: inspect('mainViews').has(popup),
      events: events.splice(0),
    }
    assert.deepEqual(nativeDestroy, {
      guestDestroyed: true, viewContentsCleared: true, shellDestroyed: true,
      windowMapped: false, childMapped: false, webviewMapped: false, mainMapped: false,
      events: [
        { event: 'webview.closed', params: { webviewId: 30 } },
        { event: 'window.closed', params: { windowId: 3 } },
      ],
    })

    inspect('activePopupRequest = 8')
    inspect('activePopupContents = {session:undefined}')
    await call('window.create', {
      windowId: 4, newWindowRequest: 8,
      webview: { webviewId: 31, url: 'about:blank' },
    })
    const stagedExplicit = inspect('pendingPopups.get(8)')
    inspect('pendingPopups.delete(8)')
    stagedExplicit.attach(new Contents())
    const popupExplicit = stagedExplicit.window
    const explicitView = mainViewOf(popupExplicit)
    const explicitGuest = explicitView.webContents
    Object.defineProperty(explicitView, 'webContents', {
      get: () => explicitGuest.isDestroyed() ? undefined : explicitGuest,
    })
    events.length = 0
    await call('webview.close', { webviewId: 31 })
    const explicitClose = {
      guestDestroyed: explicitGuest.isDestroyed(),
      viewContentsCleared: explicitView.webContents === undefined,
      shellDestroyed: popupExplicit.isDestroyed(),
      windowMapped: inspect('windows.has(4)'),
      childMapped: inspect('childWebviews.has(31)'),
      webviewMapped: inspect('webviews.has(31)'),
      mainMapped: inspect('mainViews').has(popupExplicit),
      events: events.splice(0),
    }
    assert.deepEqual(explicitClose, {
      guestDestroyed: true, viewContentsCleared: true, shellDestroyed: false,
      windowMapped: true, childMapped: false, webviewMapped: false, mainMapped: false,
      events: [{ event: 'webview.closed', params: { webviewId: 31 } }],
    })

    await call('window.create', { windowId: 5, window: { closable: false } })
    const noSocket = inspect('windows.get(5)')
    inspect('socket = undefined')
    events.length = 0
    noSocket.close()
    const noSocketClose = {
      prevented: noSocket.lastClosePrevented, destroyed: noSocket.isDestroyed(), events: events.splice(0),
    }
    inspect('socket = {closed:true}')
    await call('window.create', { windowId: 6, window: { closable: false } })
    const closedSocket = inspect('windows.get(6)')
    events.length = 0
    closedSocket.close()
    const closedSocketClose = {
      prevented: closedSocket.lastClosePrevented, destroyed: closedSocket.isDestroyed(), events: events.splice(0),
    }
    assert.deepEqual(noSocketClose, { prevented: false, destroyed: true, events: [] })
    assert.deepEqual(closedSocketClose, { prevented: false, destroyed: true, events: [] })
    result = { roots, nativeDestroy, explicitClose, noSocketClose, closedSocketClose }
    break
  }
  case 'linux-state': {
    context.process.platform = 'linux'
    inspect('socket = {closed:false}')
    await call('window.create', { windowId: 60,
      window: { maximizable: false, minimizable: false, closable: false } })
    const host = inspect('windows.get(60)')
    const state = async () => {
      const { maximizable, minimizable, closable } = await call('window.geometry', { windowId: 60 })
      return { maximizable, minimizable, closable }
    }
    const initial = await state()
    const events = []
    context.send = (message) => events.push(message)
    host.close()
    const vetoed = { prevented: host.lastClosePrevented, events: events.splice(0) }
    for (const method of ['maximizable', 'minimizable', 'closable']) {
      await call(`window.${method}`, { windowId: 60, [method]: true })
    }
    const enabled = await state()
    host.close()
    const requested = { prevented: host.lastClosePrevented, events: events.splice(0) }
    for (const method of ['maximizable', 'minimizable', 'closable']) {
      await call(`window.${method}`, { windowId: 60, [method]: false })
    }
    const disabled = await state()
    await call('window.close', { windowId: 60 })
    const vetoedAgain = { prevented: host.lastClosePrevented, events: events.splice(0) }
    await call('window.destroy', { windowId: 60 })
    const closedEvents = events.splice(0)
    await call('window.create', { windowId: 61, window: { closable: false } })
    const disconnected = inspect('windows.get(61)')
    inspect('socket.closed = true')
    disconnected.close()
    result = { initial, vetoed, enabled, requested, disabled, vetoedAgain,
      destroyed: host.isDestroyed(), closedEvents,
      disconnected: { prevented: disconnected.lastClosePrevented, destroyed: disconnected.isDestroyed() } }
    break
  }
  case 'lifecycle': {
    const host = await createHost()
    await call('webview.create', { windowId: 1, webviewId: 16, url: 'tauri://localhost/fixed', autoResize: false })
    const child = host.children[0]
    const initialBounds = child.getBounds()
    host.resize(1200, 900)
    const resizedBounds = child.getBounds()
    let finishLoad
    let loaded = false
    child.webContents.loadURL = (url) => {
      child.webContents.url = url
      return new Promise((resolve) => { finishLoad = () => { loaded = true; resolve() } })
    }
    const navigating = call('webview.navigate', { webviewId: 16, url: 'tauri://localhost/next' })
    const navigation = { acknowledged: navigating === null, loaded,
      url: child.webContents.getURL(), hostUrl: mainContentsOf(host)?.getURL() ?? null }
    finishLoad()
    await navigating
    context.process.platform = 'darwin'
    const initialFullscreenListeners = ['enter-full-screen', 'leave-full-screen'].map((event) => host.listenerCount(event))
    const nativeCalls = []
    const simpleCalls = []
    let native = false
    host.setFullScreen = (value) => nativeCalls.push(value)
    host.setSimpleFullScreen = (value) => simpleCalls.push(value)
    host.isFullScreen = () => native
    let acknowledged = false
    const entering = call('window.fullscreen', { windowId: 1, fullscreen: true }).then(() => { acknowledged = true })
    await settle()
    const beforeEnter = { acknowledged, fullscreen: (await call('window.geometry', { windowId: 1 })).fullscreen,
      simpleCalls: [...simpleCalls] }
    native = true
    host.emit('enter-full-screen')
    await entering
    const entered = (await call('window.geometry', { windowId: 1 })).fullscreen
    acknowledged = false
    const leaving = call('window.fullscreen', { windowId: 1, fullscreen: false }).then(() => { acknowledged = true })
    await settle()
    const beforeLeave = { acknowledged, fullscreen: (await call('window.geometry', { windowId: 1 })).fullscreen }
    native = false
    host.emit('leave-full-screen')
    await leaving
    const exited = (await call('window.geometry', { windowId: 1 })).fullscreen
    const reentering = call('window.fullscreen', { windowId: 1, fullscreen: true })
    native = true
    host.emit('enter-full-screen')
    await reentering
    await call('window.fullscreen', { windowId: 1, fullscreen: true })
    const nativeFullscreen = (await call('window.geometry', { windowId: 1 })).fullscreen
    native = false
    const timeout = errorFrom(() => call('window.fullscreen', { windowId: 1, fullscreen: true }))
    const timer = [...timers].find((item) => item.delay === 10_000)
    timer.callback()
    const timeoutError = await timeout
    const closing = errorFrom(() => call('window.fullscreen', { windowId: 1, fullscreen: true }))
    host.destroy()
    host.isFullScreen = host.isSimpleFullScreen = host.setSimpleFullScreen = () => { throw new Error('closed window accessed') }
    const closedError = await closing
    result = { initialBounds, resizedBounds, navigation, beforeEnter, beforeLeave, entered, exited, nativeFullscreen,
      nativeCalls, simpleCalls, timeoutDelay: timer.delay, timeoutError, closedError, timers: timers.size,
      transitionListeners: ['enter-full-screen', 'leave-full-screen'].map((event, index) => host.listenerCount(event) - initialFullscreenListeners[index]) }
    break
  }
  case 'restore': {
    const main = await createHost(true)
    context.process.platform = 'darwin'
    let finishRestore
    main.isMinimized = () => true
    main.restore = () => {
      main.isMinimized = () => false
      finishRestore = () => { main.visible = true; main.emit('restore') }
    }
    let restored = false
    const restoring = Promise.resolve(call('window.unminimize', { windowId: 2 })).then(() => { restored = true })
    await Promise.resolve()
    const beforeEvent = { restored, minimized: main.isMinimized() }
    finishRestore()
    await restoring
    await call('window.hide', { windowId: 2 })
    const hidden = (await call('window.geometry', { windowId: 2 })).visible
    await call('window.show', { windowId: 2 })
    result = { beforeEvent, restored, restoreListeners: main.listenerCount('restore'),
      hidden, shown: (await call('window.geometry', { windowId: 2 })).visible, timers: timers.size }
    break
  }
  default: throw new Error(`unknown scenario ${process.argv[2]}`)
}
for (const view of inspect('childWebviews').values()) {
  assert.equal(view.view.options.webPreferences.webviewTag, false, 'native views must not enable DOM webview popups')
}
console.log(JSON.stringify(result))
