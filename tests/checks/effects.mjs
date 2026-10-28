// Transparent example queue/UI regression with mocked native effects; no Electron or GUI.
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { setImmediate } from 'node:timers/promises'
import vm from 'node:vm'

const Effect = { Acrylic: 'acrylic', Mica: 'mica', Tabbed: 'tabbed', Blur: 'blur' }

// Run the actual example module body against a minimal DOM and delayed native API.
const source = readFileSync(new URL('../assets/transparent/main.js', import.meta.url), 'utf8')
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
console.log('example-effects: PASS (serialized toggles, reset/reload, independent alpha, unsupported UI; mocked native only)')
