// Offline wiring regression; Electron media rendering/native captions require desktop verification.
// Run: node scripts/verify-theme.mjs
import assert from 'node:assert/strict'
import { EventEmitter } from 'node:events'
import { readFileSync } from 'node:fs'
import vm from 'node:vm'

const source = readFileSync(new URL('../electron/main.cjs', import.meta.url), 'utf8')
const messages = []
const nativeTheme = Object.assign(new EventEmitter(), { themeSource: 'system', systemDark: false })
Object.defineProperty(nativeTheme, 'shouldUseDarkColors', {
  get: () => nativeTheme.themeSource === 'dark' || (nativeTheme.themeSource === 'system' && nativeTheme.systemDark),
})
const windows = new Map()
const sandbox = { nativeTheme, windows, process: { env: {} }, send: (message) => messages.push(message), log() {} }
vm.createContext(sandbox)
const themeCode = source.slice(source.indexOf('const windowThemeState ='), source.indexOf('async function createWindow('))
assert.ok(themeCode.includes("'Emulation.setEmulatedMedia'"))
vm.runInContext(themeCode, sandbox)
vm.runInContext(source.match(/^function findWindow\([^]*?^\}/m)[0], sandbox)
vm.runInContext(source.match(/^function handle\([^]*?^\}/m)[0], sandbox)

let nextId = 1
function contents() {
  const target = Object.assign(new EventEmitter(), { id: nextId++, override: null, commands: [] })
  let attached = false
  target.isDestroyed = () => false
  target.getURL = () => 'tauri://localhost/'
  target.isDevToolsOpened = () => false
  target.debugger = Object.assign(new EventEmitter(), {
    isAttached: () => attached,
    attach() { assert.equal(attached, false); attached = true },
    detach() { attached = false; target.override = null; target.debugger.emit('detach', {}, 'target closed') },
    async sendCommand(method, params) {
      assert.equal(attached, true)
      assert.equal(method, 'Emulation.setEmulatedMedia')
      target.commands.push(JSON.parse(JSON.stringify(params)))
      target.override = params.features[0]?.value ?? null
    },
  })
  target.mainFrame = { executeJavaScript: async (code) => {
    assert.equal(code, "matchMedia('(prefers-color-scheme: dark)').matches")
    return target.override ? target.override === 'dark' : nativeTheme.shouldUseDarkColors
  } }
  return target
}
function window(windowId, theme) {
  const window = Object.assign(new EventEmitter(), { webContents: contents(), isDestroyed: () => false })
  windows.set(windowId, window)
  sandbox.installWindowTheme(window, windowId, theme)
  return window
}
const query = async (windowId) => (await sandbox.handle('window.themeQuery', { windowId })).theme
const setTheme = (windowId, theme) => sandbox.handle('window.theme', { windowId, theme })
const settle = () => new Promise((resolve) => setImmediate(resolve))

const first = window(1, 'dark')
const sibling = window(2)
await settle()
assert.equal(await query(1), 'dark', 'creation config must apply a real Chromium override')
assert.equal(await query(2), 'light', 'a sibling must remain at app preference')
const getURL = sibling.webContents.getURL
sibling.webContents.getURL = () => ''
assert.equal(await query(2), 'light', 'an uncommitted page must not block creation callbacks')
sibling.webContents.getURL = getURL
assert.equal(nativeTheme.themeSource, 'system', 'per-window theme must not mutate nativeTheme')
await setTheme(1, 'light')
await sandbox.handle('app.theme', { theme: 'dark' })
nativeTheme.emit('updated')
assert.equal(await query(1), 'light', 'explicit window theme must survive an app override')
assert.equal(await query(2), 'dark', 'app override still applies to unforced siblings')
await setTheme(1, 'system')
assert.equal(await query(1), 'dark', 'clearing window override must inherit the app theme')
assert.equal(first.webContents.debugger.isAttached(), false, 'clear must release our debugger')
assert.deepEqual(first.webContents.commands.at(-1), { features: [] })
await sandbox.handle('app.theme', { theme: 'system' })
assert.equal(await query(1), 'light')
assert.equal(await query(2), 'light')
await Promise.all([setTheme(1, 'dark'), setTheme(1, 'light')])
assert.equal(await query(1), 'light', 'concurrent calls must preserve request order')
const sendCommand = first.webContents.debugger.sendCommand
let resume
first.webContents.debugger.sendCommand = async (...args) => {
  await new Promise((resolve) => { resume = resolve })
  return sendCommand(...args)
}
const changing = setTheme(1, 'dark')
await settle()
const guest = contents()
first.webContents.emit('did-attach-webview', {}, guest)
await settle()
assert.equal(guest.override, 'dark', 'a guest attached during an in-flight setter inherits the new override')
resume()
await changing
first.webContents.debugger.sendCommand = sendCommand
assert.equal(guest.override, 'dark')
assert.equal(sibling.webContents.commands.length, 0)
first.webContents.emit('dom-ready')
await settle()
assert.equal(await query(1), 'dark', 'navigation keeps the target session and its override')
first.webContents.debugger.detach()
await settle()
assert.equal(await query(1), 'light', 'getter must report live media after debugger detach')
assert.equal(first.webContents.debugger.isAttached(), false, 'detach must not trigger a reattach loop')
first.webContents.emit('devtools-closed')
await settle()
assert.equal(await query(1), 'dark', 'DevTools close can restore a suspended override')
await setTheme(1, 'system')
assert.equal(guest.override, null)
sibling.webContents.debugger.attach()
await assert.rejects(setTheme(2, 'dark'), /existing debugger/)
assert.equal(sibling.webContents.debugger.isAttached(), true, 'never detach somebody else\'s debugger')
assert.equal(sibling.webContents.commands.length, 0, 'never send commands through another owner')
assert.throws(() => setTheme(1, 'bogus'), /invalid window theme/)
await settle()
assert.ok(messages.some(({ event, params }) => event === 'window.themeChanged' && params.windowId === 1 && params.theme === 'dark' && params.dark === true))
first.emit('closed')
sibling.emit('closed')
assert.equal(nativeTheme.listenerCount('updated'), 0)
console.log('theme: offline isolation, config, app/override/clear, guests, detach and ownership checks passed')
