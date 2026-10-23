// Execute the production dispatch/layout code with native objects replaced by
// small stateful doubles: no Electron process, socket, GUI, or network.
// Run: node scripts/webview-layout-check.mjs
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { EventEmitter } from 'node:events'
import vm from 'node:vm'

const require = createRequire(import.meta.url)
let nextId = 0
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
  openDevTools() { this.devtools = true }
  closeDevTools() { this.devtools = false }
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
    app: { on() {}, whenReady: () => new Promise(() => {}) },
    BrowserWindow: Window, WebContentsView: View,
    ipcMain: { on() {}, handle() {} }, protocol: { registerSchemesAsPrivileged() {} },
    screen: {
      getDisplayMatching: (bounds) => ({ scaleFactor: bounds.scale }),
      // A secondary display has a physical origin distinct from DIP-origin * scale.
      getCursorScreenPoint: () => ({ x: 2100, y: 40 }),
      getDisplayNearestPoint: () => ({ scaleFactor: 2 }),
      dipToScreenPoint: ({ x, y }) => ({ x: 1920 + (x - 1920) * 2, y: y * 2 }),
      screenToDipPoint: ({ x, y }) => ({ x: 1920 + (x - 1920) / 2, y: y / 2 }),
      dipToScreenRect: (_window, rect) => ({
        x: 1920 + (rect.x - 1920) * 2, y: rect.y * 2,
        width: rect.width * 2, height: rect.height * 2,
      }),
    },
  } : name === 'node:fs' ? { appendFileSync() {} }
    : name === 'node:os' ? { tmpdir: () => '.', release: () => '10.0.22631' } : require(name),
  process: { argv: ['node', 'main.cjs', '--port', '1'], env: {}, platform: 'win32' },
  Buffer, URL, console, setTimeout, clearTimeout, __dirname: new URL('../electron', import.meta.url).pathname,
})
vm.runInContext(readFileSync(new URL('../electron/main.cjs', import.meta.url), 'utf8'), context)
const inspect = (expression) => vm.runInContext(expression, context)
const plain = (value) => JSON.parse(JSON.stringify(value))
const call = (method, params) => context.handle(method, params)
await call('window.create', { windowId: 1, width: 800, height: 600 })
const host = inspect('windows.get(1)')
assert.equal(host.options.useContentSize, true)
assert.deepEqual(plain(await call('window.cursorPosition', {})), { x: 2280, y: 80 })
await call('window.position', { windowId: 1, physical: true, x: 2280, y: 80 })
assert.equal(host.content.x, 2100)
assert.equal(host.content.y, 40)
const geometry = plain(inspect('readGeometry(windows.get(1))'))
assert.equal(geometry.bounds.x, 2280)
assert.equal(geometry.bounds.y, 80)
const monitor = plain(inspect(`readDisplay({ scaleFactor: 2,
  bounds: { x: 1920, y: 0, width: 1280, height: 720 },
  workArea: { x: 1920, y: 0, width: 1280, height: 680 } })`))
assert.equal(monitor.position.x, 1920, 'monitor origin is converted natively, not multiplied by scale')
assert.equal(monitor.size.width, 2560)
assert.equal(monitor.workArea.size.height, 1360)
const urls = ['tauri://localhost/', 'https://github.com/tauri-apps/tauri', 'https://tauri.app', 'https://twitter.com/TauriApps']
for (let index = 0; index < 4; index++) {
  const x = index % 2 * 400
  const y = Math.floor(index / 2) * 300
  await call('webview.create', {
    windowId: 1, webviewId: index + 10, url: urls[index], autoResize: true,
    bounds: { position: { x, y, physical: false }, size: { width: 400, height: 300, physical: false } },
    policy: { permission: true },
  })
}
assert.equal(host.children.length, 4)
assert.equal(host.webContents.url, undefined, 'first child must not replace the host main page')
assert.equal(new Set(host.children.map((view) => view.webContents.id)).size, 4)
for (let index = 0; index < 4; index++) {
  const view = host.children[index]
  assert.equal(view.webContents.url, urls[index])
  assert.deepEqual(plain(view.options.webPreferences), {
    contextIsolation: true, sandbox: true, nodeIntegration: false,
    nodeIntegrationInSubFrames: true,
    preload: view.options.webPreferences.preload,
  })
  assert.equal(inspect(`webviewByContents.get(${view.webContents.id})`), index + 10)
  assert.deepEqual(plain(await call('webview.bounds', { webviewId: index + 10 })).bounds, {
    x: index % 2 * 800, y: Math.floor(index / 2) * 600, width: 800, height: 600,
  })
}
host.resize(1001, 701)
for (let index = 0; index < 4; index++) {
  const left = Math.round(index % 2 * 1001 / 2)
  const top = Math.round(Math.floor(index / 2) * 701 / 2)
  assert.deepEqual(host.children[index].getBounds(), {
    x: left, y: top, width: index % 2 ? 1001 - left : 501, height: index < 2 ? 351 : 701 - top,
  })
}
host.resize(800, 600)
assert.deepEqual(host.children[3].getBounds(), { x: 400, y: 300, width: 400, height: 300 }, 'resizing back must not accumulate rounding drift')
// Position and size may independently use logical/physical units; partial
// setters preserve the other half and need no racy getter round trip from Rust.
await call('webview.setBounds', { webviewId: 10,
  position: { x: -30, y: 20, physical: false },
  size: { width: 400, height: 200, physical: true },
})
assert.deepEqual(host.children[0].getBounds(), { x: -30, y: 20, width: 200, height: 100 })
await call('webview.setBounds', { webviewId: 10, size: { width: 310, height: 190, physical: false } })
assert.deepEqual(host.children[0].getBounds(), { x: -30, y: 20, width: 310, height: 190 })
await call('webview.setBounds', { webviewId: 10, position: { x: 120, y: 80, physical: true } })
assert.deepEqual(host.children[0].getBounds(), { x: 60, y: 40, width: 310, height: 190 })
await call('webview.setBounds', { webviewId: 10,
  position: { x: 100, y: 50, physical: false },
  size: { width: 200, height: 100, physical: false },
})
assert.deepEqual(host.children[0].getBounds(), { x: 100, y: 50, width: 200, height: 100 })
assert.deepEqual(plain(await call('webview.bounds', { webviewId: 10 })).bounds, { x: 200, y: 100, width: 400, height: 200 })
assert.throws(() => call('webview.setBounds', { webviewId: 10, size: { width: -1, height: 10 } }), /invalid child/)
assert.throws(() => call('webview.setBounds', { webviewId: 10, position: { x: NaN, y: 10 } }), /invalid child/)
host.resize(1600, 1200)
assert.deepEqual(host.children[0].getBounds(), { x: 200, y: 100, width: 400, height: 200 }, 'manual bounds must update auto-resize proportions')
await call('webview.setAutoResize', { webviewId: 10, autoResize: false })
host.resize(800, 600)
assert.deepEqual(host.children[0].getBounds(), { x: 200, y: 100, width: 400, height: 200 })
await call('webview.setAutoResize', { webviewId: 10, autoResize: true })
host.resize(1600, 1200)
assert.deepEqual(host.children[0].getBounds(), { x: 400, y: 200, width: 800, height: 400 })
const first = host.children[0]
await call('webview.eval', { webviewId: 10, script: 'child-script' })
assert.equal(first.webContents.script, 'child-script')
assert.equal(host.webContents.script, undefined)
// Window cursor settings reach all four native documents, not just the empty host.
for (const cursor of ['pointer', 'none', 'default']) {
  const timer = setTimeout(() => { throw new Error('cursor dispatch waited on the unloaded host') }, 1000)
  try { await call('window.cursor', { windowId: 1, cursor }) }
  finally { clearTimeout(timer) }
  const script = `document.documentElement.style.cursor = ${JSON.stringify(cursor)}`
  assert.equal(host.webContents.script, undefined, 'never execute JavaScript in an unloaded non-business host')
  assert.equal(host.children.length, 4)
  assert(host.children.every((view) => view.webContents.script === script))
}
await call('webview.reload', { webviewId: 10 })
await call('webview.setZoom', { webviewId: 10, scaleFactor: 1.25 })
await call('webview.openDevtools', { webviewId: 10 })
assert.equal(await call('webview.isDevtoolsOpen', { webviewId: 10 }), true)
// Native lifecycle methods can reenter Node; invoke them in the socket callback,
// before its microtask checkpoint. Async results and both error paths still reply.
const replies = []
const originalSend = context.send
context.send = (message) => replies.push(plain(message))
try {
  const close = context.onLine(JSON.stringify({ id: 1, method: 'webview.closeDevtools', params: { webviewId: 10 } }))
  assert.equal(first.webContents.devtools, false, 'close must run before the socket callback returns')
  await close
  assert.deepEqual(replies.pop(), { id: 1, result: null })
  await context.onLine(JSON.stringify({ id: 2, method: 'webview.isDevtoolsOpen', params: { webviewId: 10 } }))
  assert.deepEqual(replies.pop(), { id: 2, result: false })
  await context.onLine(JSON.stringify({ id: 3, method: 'missing' }))
  assert.match(replies.pop().error, /unknown method missing/)
  const execute = first.webContents.executeJavaScript
  try {
    first.webContents.executeJavaScript = () => Promise.reject(new Error('eval rejected'))
    await context.onLine(JSON.stringify({ id: 4, method: 'webview.eval', params: { webviewId: 10, script: '' } }))
    assert.deepEqual(replies.pop(), { id: 4, error: 'eval rejected' })
  } finally { first.webContents.executeJavaScript = execute }
  await context.onLine(JSON.stringify({ id: 5, method: 'webview.eval', params: { webviewId: 10, script: first.webContents.script } }))
  assert.deepEqual(replies.pop(), { id: 5, result: first.webContents.id })
  await context.onLine('invalid JSON')
  await context.onLine(JSON.stringify({ method: 'webview.closeDevtools' }))
  assert.equal(replies.length, 0, 'invalid requests produce no uncorrelated replies')
} finally { context.send = originalSend }
assert.equal(first.webContents.reloaded, true)
assert.equal(first.webContents.zoom, 1.25)
await call('webview.visibility', { webviewId: 10, visible: false })
assert.equal((await call('webview.bounds', { webviewId: 10 })).visible, false)
assert.equal(host.visible, true)
await call('webview.visibility', { webviewId: 10, visible: true })
await call('webview.focus', { webviewId: 10 })
assert.equal(first.webContents.focused, true)
await call('webview.backgroundColor', { webviewId: 10, color: '#123456' })
assert.equal(first.color, '#123456')
await call('webview.close', { webviewId: 10 })
assert.equal(first.webContents.isDestroyed(), true)
assert.equal(host.children.length, 3)
assert.equal(inspect(`webviewByContents.has(${first.webContents.id})`), false)
assert.throws(() => call('webview.eval', { webviewId: 10, script: '' }), /unknown webview/)
const remaining = [...host.children]
host.destroy()
assert(remaining.every((view) => view.webContents.isDestroyed()), 'closing host must release child webContents')
assert.equal(inspect('childWebviews.size'), 0)
assert.equal(inspect('webviews.size'), 0)
assert.equal(inspect('handlerPolicy.size'), 0)

// Existing WebviewWindow main pages retain their BrowserWindow dispatch path.
await call('window.create', { windowId: 2, width: 640, height: 480, webview: { webviewId: 20, url: 'tauri://localhost/main' } })
const main = inspect('windows.get(2)')
// A native attach failure must still close the newly allocated webContents.
const addChildView = main.contentView.addChildView
main.contentView.addChildView = () => { throw new Error('attach failed') }
assert.throws(() => call('webview.create', { windowId: 2, webviewId: 21, url: 'tauri://localhost/' }), /attach failed/)
assert.equal(allContents.at(-1).isDestroyed(), true)
assert.equal(inspect('childWebviews.has(21) || webviews.has(21)'), false)
main.contentView.addChildView = addChildView
assert.equal(main.children.length, 0)
assert.equal(main.webContents.url, 'tauri://localhost/main')
assert.equal(await call('webview.eval', { webviewId: 20, script: 'main-script' }), main.webContents.id)
await call('window.cursor', { windowId: 2, cursor: 'crosshair' })
assert.equal(main.webContents.script, 'document.documentElement.style.cursor = "crosshair"')
assert.equal(first.webContents.script, 'document.documentElement.style.cursor = "default"', 'cursor changes stay within their owning window')
// Exercise the production effects RPC: clearing must not leave Electron's white
// backing, and a new color during an effect becomes the next restored backing.
await call('window.backgroundColor', { windowId: 2, color: '#00000000' })
await call('window.backgroundMaterial', { windowId: 2, material: 'none' })
assert.equal(main.getBackgroundColor(), '#00000000')
for (const material of ['acrylic', 'mica', 'tabbed']) {
  await call('window.backgroundMaterial', { windowId: 2, material })
  assert.equal(main.material, material)
  assert.equal(main.getBackgroundColor(), '#00000000')
}
await call('window.backgroundMaterial', { windowId: 2, material: 'none' })
assert.equal(main.getBackgroundColor(), '#00000000')
await call('window.backgroundMaterial', { windowId: 2, material: 'acrylic' })
await call('window.backgroundColor', { windowId: 2, color: '#123456' })
await call('window.backgroundMaterial', { windowId: 2, material: 'none' })
assert.equal(main.getBackgroundColor(), '#123456')
assert.throws(() => call('window.backgroundMaterial', { windowId: 2, material: 'blur' }), /invalid background material/)
// AppKit can clear isMiniaturized before its restore animation completes. A
// following hide must not be undone by that animation ordering the window in.
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
assert.equal(restored, false, 'unminimize must wait for the native restore event, not just isMinimized')
finishRestore()
await restoring
assert.equal(main.listenerCount('restore'), 0)
await call('window.hide', { windowId: 2 })
assert.equal((await call('window.geometry', { windowId: 2 })).visible, false)
await call('window.show', { windowId: 2 })
assert.equal((await call('window.geometry', { windowId: 2 })).visible, true)
context.process.platform = 'linux'
assert.throws(() => call('window.backgroundMaterial', { windowId: 2, material: 'acrylic' }), /Windows 11 22H2/)
context.process.platform = 'win32'
await call('webview.visibility', { webviewId: 20, visible: false })
assert.equal(main.visible, false)
await call('webview.close', { webviewId: 20 })
assert.equal(main.closeRequested, true)
console.log('ok: four native child views, URL/IPC ownership, DPI bounds, proportional resizing, dispatch and cleanup; main page compatibility')
