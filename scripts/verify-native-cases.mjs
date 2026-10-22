// Native regression helpers; importing this module never launches an app.
import { setTimeout as delay } from 'node:timers/promises'
import path from 'node:path'

const current = 'window.__TAURI__.window.getCurrentWindow()'
const label = 'window.__TAURI_INTERNALS__?.metadata?.currentWindow?.label'
const menu1 = ['Toggle', 'New window', 'Icon 1', 'Icon 2', 'Switch Menu', 'Quit', 'Remove Tray icon']
const menu2 = ['New window', 'Switch Menu', 'Quit', 'Remove Tray icon']

async function observe(description, read, accepts) {
  const deadline = Date.now() + 20000
  let value
  do {
    value = await read()
    if (accepts(value)) return value
    await delay(100)
  } while (Date.now() < deadline)
  throw new Error(`${description}: last native observation ${JSON.stringify(value)}`)
}

function supported(t, scope) {
  if (process.platform === 'win32' && typeof t.native === 'function') return true
  t.note(scope, 'Unverified: the scoped Windows native helper is unavailable on this platform.')
  return false
}

const shot = (t, name, params = {}) => t.native('capture', {
  ...params, OutputPath: path.join(t.dir, `${name}.png`),
})

async function popup(t, item) {
  const menus = await observe(`Popup containing ${item}`, () => t.native('popup'),
    menus => menus.filter(menu => menu.items.some(entry => entry.name === item)).length === 1)
  return menus.find(menu => menu.items.some(entry => entry.name === item))
}

async function trayMenu(t, tooltip) {
  const tray = await t.native('tray-state', { ItemText: tooltip })
  if (!tray.available) throw new Error(`Startup tray ${tooltip} unavailable: ${tray.reason}`)
  await t.native('tray-click', { ItemText: tooltip, Button: 'right' })
  return popup(t, 'Remove Tray icon')
}

// Call after API's temporary custom tray has been removed. This removes the
// source-defined startup tray last and leaves the main window visible.
export async function verifyStartupTray(t, main) {
  if (!supported(t, 'Rust startup tray')) return
  const tray = await t.native('tray-state', { ItemText: 'Tauri' })
  if (!tray.available) {
    t.note('Rust startup tray', `Unverified: ${tray.reason || 'the Tauri notification icon is not exposed by shell UIAutomation.'}`)
    return
  }
  const title = await t.evaluate(main, `${current}.title()`)
  const states = await t.native('state', { Title: title })
  const nativeMain = states.find(window => window.visible && window.clientWidth > 0 && window.clientHeight > 0)
  if (!nativeMain || states.filter(window => window.visible).length !== 1) throw new Error('Startup tray needs one unambiguous visible main HWND')
  t.check('Startup tray belongs to source main window', await t.evaluate(main, label), 'main')
  await shot(t, 'api-startup-tray-icon', { Surface: 'tray', ItemText: 'Tauri' })
  let created
  const visibility = async (visible, name) => {
    const windows = await observe(name, () => t.native('state'),
      windows => windows.find(window => window.handle === nativeMain.handle)?.visible === visible)
    t.check(name, windows.find(window => window.handle === nativeMain.handle)?.visible, visible)
    t.check(`${name} agrees with frontend getter`, await t.wait(main, `${current}.isVisible()`, value => value === visible), visible)
  }
  try {
    let menu = await trayMenu(t, 'Tauri')
    t.check('Startup tray source menu labels', menu.items.map(item => item.name), menu1)
    await shot(t, 'api-startup-tray-menu', { Surface: 'popup', ItemText: 'Remove Tray icon' })
    await t.native('menu-click', { ItemText: 'Toggle' })
    await visibility(false, 'Startup tray Toggle hides the real main HWND')

    menu = await trayMenu(t, 'Tauri')
    t.check('Startup tray hidden window relabels Toggle to Show', menu.items.map(item => item.name), ['Show', ...menu1.slice(1)])
    await shot(t, 'api-startup-tray-show', { Surface: 'popup', ItemText: 'Show' })
    await t.native('menu-click', { ItemText: 'Show' })
    await visibility(true, 'Startup tray Show restores the real main HWND')
    menu = await trayMenu(t, 'Tauri')
    t.check('Startup tray shown window relabels Show to Hide', menu.items.map(item => item.name), ['Hide', ...menu1.slice(1)])
    await t.native('menu-click', { ItemText: 'Hide' })
    await visibility(false, 'Startup tray Hide hides the main HWND again')

    await t.native('tray-click', { ItemText: 'Tauri', Button: 'left' })
    await visibility(true, 'Startup tray native left click restores main')
    // Observe before capture/raise: those helpers can restore or focus a window.
    for (let sample = 0; sample < 2; sample++) {
      t.check(`Startup tray left click opens no menu (${sample + 1})`, (await t.native('popup')).length, 0)
    }
    await t.capture(main, 'api-startup-tray-left-restored')
    await shot(t, 'api-startup-tray-left-restored-native', { Title: title })

    await trayMenu(t, 'Tauri')
    await t.native('menu-click', { ItemText: 'New window' })
    created = await t.target(async candidate => candidate.type === 'page' && await t.evaluate(candidate, label) === 'new')
    await t.wait(created, 'document.readyState', value => ['interactive', 'complete'].includes(value))
    t.check('Startup tray New window uses source label', await t.evaluate(created, label), 'new')
    const createdWindows = await observe('Startup tray new native window', () => t.native('state'),
      windows => windows.filter(window => window.handle !== nativeMain.handle && window.title === 'Tauri' && window.visible).length === 1)
    const createdWindow = createdWindows.find(window => window.handle !== nativeMain.handle && window.title === 'Tauri' && window.visible)
    t.check('Startup tray New window uses source native title', createdWindow.title, 'Tauri')
    await t.capture(created, 'api-startup-tray-new-window')
    await shot(t, 'api-startup-tray-new-window-native', { Title: 'Tauri' })
    // The source capability permits main/main-*; issue close from main rather
    // than invoking a window command from the unprivileged new document.
    await t.evaluate(main, `new window.__TAURI__.window.Window('new').close()`)
    await observe('Startup tray new HWND closes', () => t.native('state'),
      windows => !windows.some(window => window.handle === createdWindow.handle))
    await t.target(async candidate => candidate.id === main.id && !(await t.targets()).some(target => target.id === created.id))
    t.check('Startup tray New window closes without main closing', (await t.targets()).some(target => target.id === created.id), false)
    created = undefined

    for (const icon of ['Icon 1', 'Icon 2']) {
      await trayMenu(t, 'Tauri')
      await t.native('menu-click', { ItemText: icon })
      await observe(`${icon} menu closes`, () => t.native('popup'), menus => menus.length === 0)
      await shot(t, `api-startup-tray-${icon === 'Icon 1' ? 'icon-1' : 'icon-2'}`, { Surface: 'tray', ItemText: 'Tauri' })
    }
    t.note('Startup tray icon identity', 'Native Icon 1/Icon 2 selections and shell screenshots are recorded; exact pixels/transparency are not compared or claimed verified.')

    await trayMenu(t, 'Tauri')
    await t.native('menu-click', { ItemText: 'Switch Menu' })
    const switched = await observe('Startup tray tooltip changes to Menu 2', () => t.native('tray-state', { ItemText: 'Menu 2' }), state => state.available)
    t.check('Startup tray Switch Menu changes shell tooltip', switched.available, true)
    menu = await trayMenu(t, 'Menu 2')
    t.check('Startup tray Menu 2 source labels omit icon commands', menu.items.map(item => item.name), ['Show', ...menu2])
    await shot(t, 'api-startup-tray-menu-2', { Surface: 'popup', ItemText: 'Remove Tray icon' })
    await t.native('menu-click', { ItemText: 'Switch Menu' })
    const restored = await observe('Startup tray tooltip returns to Tauri', () => t.native('tray-state', { ItemText: 'Tauri' }), state => state.available)
    t.check('Startup tray Switch Menu restores shell tooltip', restored.available, true)
    menu = await trayMenu(t, 'Tauri')
    t.check('Startup tray Switch Menu restores full source menu', menu.items.map(item => item.name), ['Show', ...menu1.slice(1)])
    await shot(t, 'api-startup-tray-menu-restored', { Surface: 'popup', ItemText: 'Remove Tray icon' })
    t.note('Startup tray Quit', 'Quit is not selected: it exits the API app before subsequent runner checks. Its presence is observed; exit behavior remains unverified.')
    await t.native('menu-click', { ItemText: 'Remove Tray icon' })
    const removed = await observe('Startup tray disappears from shell', () => t.native('tray-state', { ItemText: 'Tauri' }),
      state => !state.available && /matches=0(?:,|\))/.test(state.reason ?? '') && /tray surfaces=[1-9]\d*/.test(state.reason ?? ''))
    t.check('Startup tray Remove Tray icon removes notification icon', removed.available, false)
    await visibility(true, 'Startup tray removal leaves main visible')
  } finally {
    // Recovery is not evidence for the tray action; failed observations still fail.
    const popups = await t.native('popup')
    if (popups.some(menu => menu.items.some(item => item.name === 'Remove Tray icon'))) {
      await t.native('dismiss', { ItemText: 'Remove Tray icon' })
    }
    await t.evaluate(main, `${current}.show()`).catch(error => t.note('Startup tray cleanup', String(error)))
    if (created) await t.evaluate(main, `new window.__TAURI__.window.Window('new').close()`).catch(error => t.note('Startup tray new-window cleanup', String(error)))
  }
}

// Call on helloworld or the surviving splashscreen main after content checks.
// close:true consumes the final window through the actual native Close item.
export async function verifyDefaultMenu(t, target, { close = false } = {}) {
  if (!supported(t, 'Default native menu')) return
  // Flat examples have no frontend window grants; observe the actual HWND.
  const nativeWindows = await t.native('state')
  const before = nativeWindows.find(window => window.visible && window.clientWidth > 0 && window.clientHeight > 0)
  if (!before || nativeWindows.filter(window => window.visible).length !== 1) throw new Error('Default menu needs one unambiguous visible target HWND')
  const prefix = `${t.name}-default-menu`
  const params = { Title: before.title }
  const bar = await observe('Default menu bar', () => t.native('menubar', params),
    bar => bar.items.filter(item => !item.offscreen).length >= 4)
  t.check('Default native menu top-level source labels', bar.items.filter(item => !item.offscreen).map(item => item.name), ['File', 'Edit', 'Window', 'Help'])
  await shot(t, `${prefix}-bar`, params)
  for (const [name, expected] of [
    ['File', ['Close', 'Exit']],
    ['Edit', ['Undo', 'Redo', 'Cut', 'Copy', 'Paste', 'Select All']],
    ['Window', ['Minimize', 'Maximize', 'Close']],
    ['Help', ['About']],
  ]) {
    await t.native('menubar-click', { ...params, ItemText: name })
    const menu = await popup(t, expected[0])
    t.check(`Default ${name} source labels`, menu.items.map(item => item.name), expected)
    await shot(t, `${prefix}-${name.toLowerCase()}`, { Surface: 'popup', ItemText: expected[0] })
    await t.native('dismiss', { ItemText: expected[0] })
  }

  try {
    await t.native('menubar-click', { ...params, ItemText: 'Window' })
    await popup(t, 'Maximize')
    await t.native('menu-click', { ItemText: 'Maximize' })
    const maximized = await observe('Default Maximize enlarges actual HWND', () => t.native('state'), windows => {
      const window = windows.find(window => window.handle === before.handle)
      return window?.visible && window.width >= before.width && window.height >= before.height && (window.width > before.width || window.height > before.height)
    })
    t.check('Default native Maximize enlarges real HWND', maximized.some(window => window.handle === before.handle && (window.width > before.width || window.height > before.height)), true)
    await t.capture(target, `${prefix}-maximized`)
    await shot(t, `${prefix}-maximized-native`, params)
    await t.native('restore', params)
    await observe('Default menu unmaximize restores native bounds', () => t.native('state'),
      windows => windows.some(window => window.handle === before.handle && window.width === before.width && window.height === before.height))

    await t.native('menubar-click', { ...params, ItemText: 'Window' })
    await popup(t, 'Minimize')
    await t.native('menu-click', { ItemText: 'Minimize' })
    const minimized = await observe('Default Minimize moves actual HWND off desktop', () => t.native('state'), windows => {
      const window = windows.find(window => window.handle === before.handle)
      return window && (window.x <= -30000 || window.y <= -30000 || window.width < before.width || window.height < before.height)
    })
    t.check('Default native Minimize changes real HWND placement', minimized.some(window => window.handle === before.handle && (window.x <= -30000 || window.y <= -30000 || window.width < before.width || window.height < before.height)), true)
    // Native capture raises/restores a minimized window; observe first.
  } finally {
    // Restore the real HWND without widening the source's frontend permissions.
    await t.native('restore', params)
  }
  await observe('Default menu restores native HWND', () => t.native('state'),
    windows => windows.some(window => window.handle === before.handle && window.visible && window.width === before.width && window.height === before.height))
  await t.capture(target, `${prefix}-restored`)
  await shot(t, `${prefix}-restored-native`, params)
  t.note('Default menu remaining actions', 'Native labels and Window Maximize/Minimize are exercised; Edit clipboard/undo actions, About dialog and File Exit are not exercised.')
  if (!close) {
    t.note('Default native Close', 'Close is not selected because the caller retained the target for later checks; pass { close: true } only at the end of this example.')
    return
  }
  await t.native('menubar-click', { ...params, ItemText: 'Window' })
  await popup(t, 'Close')
  await t.native('menu-click', { ItemText: 'Close' })
  const remaining = await observe('Default native Close destroys target HWND', () => t.native('state'),
    windows => !windows.some(window => window.handle === before.handle))
  t.check('Default native Close removes actual target HWND', remaining.some(window => window.handle === before.handle), false)
  t.check('Default native Close exits the final-window example', await t.closed(), 0)
}
