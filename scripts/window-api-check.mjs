// Node 24+, no Electron, GUI, installed peer, or third-party test dependencies.
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { registerHooks } from 'node:module'
import { setImmediate } from 'node:timers/promises'
import vm from 'node:vm'

const calls = []
const Effect = { Acrylic: 'acrylic', Mica: 'mica', Tabbed: 'tabbed', Blur: 'blur' }
const EffectState = { Active: 'active', Inactive: 'inactive', FollowsWindowActiveState: 'followsWindowActiveState' }
class OfficialWindow {
  constructor(label, options) { this.label = label; this.options = options }
  async isFocused() { return this.label === 'focused' }
  async minimize() { return 'inherited-minimize' }
  async setEffects() { throw Error('upstream generic effects must not be used') }
}
const originals = [new OfficialWindow('main'), new OfficialWindow('focused')]
const upstream = { Window: OfficialWindow, Effect, EffectState, getCurrentWindow: () => originals[0], getAllWindows: async () => originals }
const invoke = async (command, args) => {
  calls.push({ command, args })
  if (args.value?.effects?.includes('blur')) throw Error('unsupported effect: blur')
}
const previous = globalThis.__TAURI__
globalThis.__TAURI__ = { window: upstream, core: { invoke } }
const api = await import('../api/window.js?global-check')
assert.equal(api.Effect, Effect)
assert.equal(api.EffectState, EffectState)
for (const handle of [api.getCurrentWindow(), api.Window.getCurrent(), ...(await api.getAllWindows()), ...(await api.Window.getAll()), await api.Window.getByLabel('main'), await api.Window.getFocusedWindow()]) {
  assert.ok(handle instanceof api.Window)
  assert.ok(handle instanceof OfficialWindow)
  assert.deepEqual(handle.options, { skip: true }, 'existing handles must never create another native window')
}
assert.equal(await api.Window.getByLabel('missing'), null)
assert.equal((await api.Window.getFocusedWindow()).label, 'focused')
const current = api.getCurrentWindow()
assert.equal(await current.minimize(), 'inherited-minimize')
const effects = { effects: [Effect.Acrylic], state: EffectState.Active, radius: 12, color: [10, 20, 30, 40] }
await current.setEffects(effects)
await current.clearEffects()
assert.deepEqual(calls, [
  { command: 'plugin:electron-window|set_effects', args: { label: 'main', value: effects } },
  { command: 'plugin:electron-window|set_effects', args: { label: 'main', value: null } },
])
await assert.rejects(current.setEffects({ effects: [Effect.Blur] }), /unsupported effect/)
assert.equal(originals[0].setEffects, OfficialWindow.prototype.setEffects, 'official handles must not be patched')

// Exercise bare peer imports without installing dependencies.
delete globalThis.__TAURI__
globalThis.__windowApiCheck = { upstream, invoke }
const peerModules = {
  '@tauri-apps/api/window': 'export const { Window, Effect, EffectState, getCurrentWindow, getAllWindows } = globalThis.__windowApiCheck.upstream',
  '@tauri-apps/api/core': 'export const { invoke } = globalThis.__windowApiCheck',
}
const hooks = registerHooks({ resolve(specifier, context, nextResolve) {
  if (specifier in peerModules) return { url: `data:text/javascript,${encodeURIComponent(peerModules[specifier])}`, shortCircuit: true }
  return nextResolve(specifier, context)
} })
try {
  const peerApi = await import('../api/window.js?peer-check')
  assert.ok(peerApi.getCurrentWindow() instanceof peerApi.Window)
  assert.equal(peerApi.Effect, Effect)
  await peerApi.getCurrentWindow().clearEffects()
} finally {
  hooks.deregister()
  delete globalThis.__windowApiCheck
  if (previous === undefined) delete globalThis.__TAURI__
  else globalThis.__TAURI__ = previous
}

// Run the actual example module body against a minimal DOM and delayed native API.
const source = readFileSync(new URL('../example/transparent/main.js', import.meta.url), 'utf8')
assert.match(source, /^import .* from '\.\/window-api\.js'/)
const body = source.replace(/^import [^\n]+\n/, '')
let nativeEffect = null
let active = 0
let maxActive = 0
let unsupported = false
const commands = []
const native = async (value) => {
  active++
  maxActive = Math.max(active, maxActive)
  commands.push(value)
  try {
    await setImmediate()
    if (unsupported) throw Error('Unsupported platform: requires Windows 11 build 22621+')
    nativeEffect = value
  } finally { active-- }
}
function loadPage() {
  const ids = ['transparency', 'opacity', 'blur', 'reset', 'blur-note', 'status', 'opacity-value', 'minimize', 'close']
  const elements = Object.fromEntries(ids.map(id => [id, {
    checked: id === 'transparency', value: id === 'opacity' ? '72' : '', disabled: ['blur', 'reset'].includes(id), textContent: '', listeners: {}, attributes: {},
    addEventListener(type, listener) { this.listeners[type] = listener },
    setAttribute(name, value) { this.attributes[name] = value },
  }]))
  const styles = {}
  vm.runInNewContext(body, {
    document: { querySelector: selector => elements[selector.slice(1)], documentElement: { style: { setProperty: (key, value) => { styles[key] = value } } } },
    Effect,
    getCurrentWindow: () => ({ setEffects: value => native(value), clearEffects: () => native(null), minimize: async () => {}, close: async () => {} }),
  })
  return { elements, styles }
}
async function idle(page) {
  for (let step = 0; step < 100; step++) {
    if (page.elements.blur.attributes['aria-busy'] === 'false') return
    await setImmediate()
  }
  assert.fail('example effects queue did not settle')
}
const page = loadPage()
await idle(page)
assert.deepEqual(commands.slice(0, 3).map(value => value?.effects?.[0] ?? null), [null, 'acrylic', null])
assert.equal(page.elements.blur.disabled, false)
assert.equal(nativeEffect, null)
for (const checked of [true, false, true, false]) {
  page.elements.blur.checked = checked
  page.elements.blur.listeners.change()
}
await idle(page)
assert.equal(maxActive, 1, 'native effect commands must serialize')
assert.equal(nativeEffect, null)
assert.equal(page.elements.blur.checked, false)
page.elements.blur.checked = true
page.elements.blur.listeners.change()
await idle(page)
assert.equal(nativeEffect.effects[0], 'acrylic')
page.elements.opacity.value = '15'
page.elements.opacity.listeners.input()
assert.equal(page.styles['--panel-alpha'], '0.15')
assert.equal(nativeEffect.effects[0], 'acrylic', 'panel alpha must not change the native effect')
page.elements.reset.listeners.click()
await idle(page)
assert.equal(nativeEffect, null)
assert.equal(page.elements.opacity.value, '72')
assert.equal(page.styles['--panel-alpha'], '0.72')
page.elements.blur.checked = true
page.elements.blur.listeners.change()
await idle(page)
const reloaded = loadPage()
await idle(reloaded)
assert.equal(nativeEffect, null, 'reload must clear the previous native effect')
unsupported = true
const unavailable = loadPage()
await idle(unavailable)
assert.equal(unavailable.elements.blur.disabled, true)
assert.equal(unavailable.elements.blur.checked, false)
assert.match(unavailable.elements.status.textContent, /Unsupported platform: requires Windows 11 build 22621\+/)
assert.notEqual(unavailable.elements.status.textContent, '透明层已就绪')
console.log('window-api: PASS (global/peer exports, handle factories, inherited methods, exact IPC, rejected errors, serialized toggles, reset/reload, independent alpha, unsupported UI; mocked native only)')
