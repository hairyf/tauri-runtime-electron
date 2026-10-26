// Observe the real main-process code with native objects replaced by doubles.
import { readFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { EventEmitter } from 'node:events'
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
const allContents = []
class Contents extends EventEmitter {
  id = ++nextId
  destroyed = false
  isDestroyed() { return this.destroyed }
  setWindowOpenHandler(handler) { this.openHandler = handler }
  loadURL(url) { this.url = url; return Promise.resolve() }
  getURL() { return this.url ?? '' }
  executeJavaScript(script) {
    this.script = script
    // Electron waits for did-stop-loading when a webContents has never loaded.
    return this.getURL() ? Promise.resolve(this.id) : new Promise(() => {})
  }
  reload() { this.reloaded = true }
  focus() { this.focused = true }
  setZoomFactor(value) { this.zoom = value }
  openDevTools() { this.devtools = true; this.devToolsWebContents = {}; this.emit('devtools-opened') }
  closeDevTools() { this.devtools = false; this.devToolsWebContents = null; this.emit('devtools-closed') }
  isDevToolsOpened() { return this.devtools ?? false }
  close() { this.destroyed = true }
  constructor() { super(); allContents.push(this) }
}
class View {
  webContents = new Contents()
  visible = true
  constructor(options) { this.options = options }
  setBounds(bounds) { this.bounds = { ...bounds } }
  getBounds() { return { ...this.bounds } }
  setVisible(value) { this.visible = value }
  getVisible() { return this.visible }
  setBackgroundColor(value) { this.color = value }
}
class Window extends EventEmitter {
  webContents = new Contents()
  children = []
  visible = true
  destroyed = false
  scale = 2
  contentView = {
    addChildView: (view) => this.children.push(view),
    removeChildView: (view) => { this.children = this.children.filter((item) => item !== view) },
  }
  constructor(options) {
    super()
    this.options = options
    this.content = { x: -200, y: 100, width: options.width, height: options.height }
  }
  getBackgroundColor() { return this.color ?? this.options.backgroundColor ?? '#ffffff' }
  setBackgroundColor(color) { this.color = color }
  setBackgroundMaterial(material) { this.material = material; this.color = material === 'none' ? '#ffffff' : '#00000000' }
  getContentBounds() { return { ...this.content } }
  getBounds() { return { ...this.content, scale: this.scale } }
  setPosition(x, y) { this.content.x = x; this.content.y = y }
  isDestroyed() { return this.destroyed }
  isVisible() { return this.visible }
  isMaximized() { return false }
  isMinimized() { return false }
  isFocused() { return true }
  isFullScreen() { return false }
  isResizable() { return true }
  isAlwaysOnTop() { return false }
  getTitle() { return this.options.title ?? '' }
  loadURL(url) { return this.webContents.loadURL(url) }
  focus() { this.focused = true }
  show() { this.visible = true }
  hide() { this.visible = false }
  close() { this.closeRequested = true }
  destroy() { this.destroyed = true; this.emit('closed') }
  resize(width, height) { this.content.width = width; this.content.height = height; this.emit('resize') }
}
const context = vm.createContext({
  require: (name) => name === 'electron' ? {
    app: { on() {}, whenReady: () => new Promise((resolve) => { ready = resolve }) },
    Menu: { setApplicationMenu() {} },
    session: { defaultSession: { on() {}, setPermissionRequestHandler() {}, webRequest: { onCompleted() {}, onErrorOccurred() {}, onBeforeRequest() {} } } },
    BrowserWindow: Window, WebContentsView: View,
    ipcMain: { on() {}, handle() {} }, protocol: { registerSchemesAsPrivileged() {} },
    screen: {
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
    },
  } : name === 'node:net' ? { connect: () => transport }
    : name === 'node:fs' ? { appendFileSync() {} }
    : name === 'node:os' ? { tmpdir: () => '.', release: () => '10.0.22631' } : require(name),
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
    result = { hostUrl: host.webContents.url ?? null, children, oddBounds,
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
    const evalScripts = { child: first.webContents.script, host: host.webContents.script ?? null }
    const cursors = []
    for (const cursor of ['pointer', 'none', 'default']) {
      await call('window.cursor', { windowId: 1, cursor })
      cursors.push({ host: host.webContents.script ?? null, children: host.children.map((view) => view.webContents.script) })
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
      listeners: ['devtools-opened', 'devtools-closed', 'destroyed'].map((event) => contents.listenerCount(event)) }
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
    const contents = host.children[0].webContents
    contents.openDevTools()
    const replies = []
    context.send = (message) => replies.push(message)
    const close = context.onLine(JSON.stringify({ id: 1, method: 'webview.closeDevtools', params: { webviewId: 10 } }))
    const closedSynchronously = contents.devtools
    await close
    await context.onLine(JSON.stringify({ id: 2, method: 'webview.isDevtoolsOpen', params: { webviewId: 10 } }))
    await context.onLine(JSON.stringify({ id: 3, method: 'missing' }))
    const execute = contents.executeJavaScript
    contents.executeJavaScript = () => Promise.reject(new Error('eval rejected'))
    await context.onLine(JSON.stringify({ id: 4, method: 'webview.eval', params: { webviewId: 10, script: '' } }))
    contents.executeJavaScript = execute
    await context.onLine(JSON.stringify({ id: 5, method: 'webview.eval', params: { webviewId: 10, script: 'rpc-script' } }))
    const directReplies = replies.splice(0)
    await context.onLine('invalid JSON')
    await context.onLine(JSON.stringify({ method: 'webview.closeDevtools' }))
    const invalidReplies = replies.splice(0)
    ready()
    await Promise.resolve()
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
    while (tasks.length) await tasks.shift()()
    result = { closedSynchronously, directReplies, invalidReplies, contentsId: contents.id,
      beforeTasks, closeCalls, closeDuringRead, framedReplies: replies }
    break
  }
  case 'main-window': {
    const host = await createChildren()
    const first = host.children[0]
    await call('window.cursor', { windowId: 1, cursor: 'default' })
    await call('webview.close', { webviewId: 10 })
    host.destroy()
    const main = await createHost(true)
    const addChildView = main.contentView.addChildView
    main.contentView.addChildView = () => { throw new Error('attach failed') }
    const attachError = await errorFrom(() => call('webview.create', { windowId: 2, webviewId: 21, url: 'tauri://localhost/' }))
    main.contentView.addChildView = addChildView
    const evalResult = await call('webview.eval', { webviewId: 20, script: 'main-script' })
    await call('window.cursor', { windowId: 2, cursor: 'crosshair' })
    await call('webview.visibility', { webviewId: 20, visible: false })
    const visible = main.visible
    await call('webview.close', { webviewId: 20 })
    result = { attachError, failedContentsDestroyed: allContents.at(-1).isDestroyed(),
      failedChildMapped: inspect('childWebviews.has(21) || webviews.has(21)'), children: main.children.length,
      url: main.webContents.url, evalResult, contentsId: main.webContents.id,
      script: main.webContents.script, unrelatedScript: first.webContents.script, visible, closeRequested: main.closeRequested }
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
console.log(JSON.stringify(result))
