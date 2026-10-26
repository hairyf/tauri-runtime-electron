// Execute the production API with global and bare-peer sources; Rust compares observations.
import { registerHooks } from 'node:module'

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

async function observe(api) {
  calls.length = 0
  const handles = [
    api.getCurrentWindow(), api.Window.getCurrent(),
    ...(await api.getAllWindows()), ...(await api.Window.getAll()),
    await api.Window.getByLabel('main'), await api.Window.getFocusedWindow(),
  ]
  const factories = handles.map(handle => ({
    label: handle?.label,
    libraryInstance: handle instanceof api.Window,
    upstreamInstance: handle instanceof OfficialWindow,
    options: handle?.options,
  }))
  const current = api.getCurrentWindow()
  const inherited = await current.minimize()
  await current.setEffects({ effects: [api.Effect.Acrylic], state: api.EffectState.Active, radius: 12, color: [10, 20, 30, 40] })
  await current.clearEffects()
  let rejected = null
  try { await current.setEffects({ effects: [api.Effect.Blur] }) }
  catch (error) { rejected = error.message }
  return {
    effectIdentity: api.Effect === Effect,
    effectStateIdentity: api.EffectState === EffectState,
    factories,
    missing: await api.Window.getByLabel('missing'),
    focused: (await api.Window.getFocusedWindow())?.label ?? null,
    inherited,
    calls: structuredClone(calls),
    rejected,
    upstreamMethodUnchanged: originals.every(handle => handle.setEffects === OfficialWindow.prototype.setEffects),
  }
}

const previous = globalThis.__TAURI__
try {
  globalThis.__TAURI__ = { window: upstream, core: { invoke } }
  const global = await observe(await import('../../api/window.js?global-check'))
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
    const peer = await observe(await import('../../api/window.js?peer-check'))
    console.log(JSON.stringify({ global, peer }))
  } finally { hooks.deregister() }
} finally {
  delete globalThis.__windowApiCheck
  if (previous === undefined) delete globalThis.__TAURI__
  else globalThis.__TAURI__ = previous
}
