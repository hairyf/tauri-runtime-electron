import { readFileSync } from 'node:fs'
const json = JSON.stringify
const consoleRows = `Array.from(document.querySelectorAll('#console pre'), e => e.textContent.replace(/^\\[[^\\]]+\\]:\\s*/, '').trim())`
const currentWindow = 'window.__TAURI__.window.getCurrentWindow()'
const heading = (text) => `[...document.querySelectorAll('main h4')].find(e => e.textContent.trim() === ${json(text)})`
const field = (text) => `[...document.querySelectorAll('main label, main div')].find(e => [...e.childNodes].filter(n => n.nodeType === Node.TEXT_NODE).map(n => n.textContent).join(' ').trim() === ${json(text)} && e.querySelector('input'))?.querySelector('input')`
// node scripts/verify-api.mjs --self-check tests console parsing without starting an app.
if (process.argv.includes('--self-check')) {
  const rows = Function('document', 'return ' + consoleRows)({
    querySelectorAll: () => ['[1:23:45 AM]: {"message":"ok"}', '[13:23:45]: 1000'].map(textContent => ({ textContent }))
  })
  if (json(rows) !== json(['{"message":"ok"}', '1000'])) throw new Error('Console timestamp parsing failed: ' + json(rows))
  console.log('API console parser self-check passed (no UI verification performed)')
}
// Resolve existing, text-labelled controls without adding attributes or replacing DOM.
async function selector(t, target, expression) {
  return t.evaluate(target, `(() => {
    let element = ${expression}
    if (!element) throw new Error(${json(`Missing control: ${expression}`)})
    const parts = []
    while (element) {
      if (element.id) { parts.unshift('#' + CSS.escape(element.id)); break }
      parts.unshift(element.localName + ':nth-child(' + ([...element.parentNode.children].indexOf(element) + 1) + ')')
      element = element.parentElement
    }
    return parts.join(' > ')
  })()`)
}
async function setValue(t, target, control, value, change = false) {
  await t.evaluate(target, `(() => {
    const element = ${control}
    if (!element) throw new Error(${json(`Missing input: ${control}`)})
    element.focus()
    element.value = ${json(String(value))}
    element.dispatchEvent(new Event('input', { bubbles: true }))
    ${change ? "element.dispatchEvent(new Event('change', { bubbles: true }))" : ''}
  })()`)
}
async function navigate(t, target, name) {
  const link = `[...document.querySelectorAll('aside a')].find(e => e.textContent.trim() === ${json(name)})`
  await t.wait(target, `Boolean(${link})`, Boolean)
  if (await t.evaluate(target, 'innerWidth < 640')) {
    await t.click(target, '#sidebarToggle')
    await t.wait(target, `document.querySelector('aside').getBoundingClientRect().x`, x => Math.abs(x) < 1)
  }
  await t.click(target, await selector(t, target, link))
  await t.wait(target, `document.querySelector('main h1')?.textContent.trim()`, (value) => value === name)
  await t.evaluate(target, "document.querySelector('main h1').nextElementSibling.scrollTop = 0")
}
async function clear(t, target) {
  // The console's actual Clear control is an icon-only role=button, not effects' Clear.
  await t.click(target, '#console [role="button"]:has(.i-codicon-clear-all)')
  await t.wait(target, consoleRows, (rows) => rows.length === 0)
}
async function expectRows(t, target, label, expected) {
  const rows = await t.wait(target, consoleRows, (value) => value.length >= expected.length)
  t.check(label, rows.map((row) => { try { return JSON.parse(row) } catch { return row } }), expected)
}
async function assertWindow(t, target, label, property, expected) {
  const expression = `${currentWindow}.${property}()`
  const value = await t.wait(target, expression, (actual) => json(actual) === json(expected))
  t.check(label, value, expected)
}
async function displayedSize(t, target, expected) {
  const expression = `(() => {
    const title = [...document.querySelectorAll('main div')].find(e => e.textContent.trim() === 'Inner Size')
    return title ? [...title.parentElement.querySelectorAll('span')].map(e => Number(e.textContent.split(':')[1])) : []
  })()`
  t.check('Window resize event updates displayed physical size',
    await t.wait(target, expression, (value) => json(value) === json(expected)), expected)
}
async function popup(t, target, item) {
  let menus = []
  await t.wait(target, 'true', () => (menus = t.native('popup')).some(menu => menu.items.some(entry => entry.name === item)))
  return menus.find(menu => menu.items.some(entry => entry.name === item))
}
async function addMenuItems(t, target) {
  for (const kind of ['Normal', 'Check']) {
    const source = `.menu-item[data-kind="${kind}"]:not([data-id])`
    const points = await t.evaluate(target, `(() => {
      const item = document.querySelector(${json(source)})
      const dest = document.querySelector('.menu-item[data-id]')?.parentElement ?? [...document.querySelectorAll('main div')].find(e => e.textContent.trim() === 'Drag and drop the menu items above to start building')?.parentElement
      if (!item || !dest) throw new Error('MenuBuilder drag controls missing')
      dest.scrollIntoView({ block: 'nearest' })
      const a = item.getBoundingClientRect(), b = dest.getBoundingClientRect()
      if (a.top < 0 || a.bottom > innerHeight || b.bottom > innerHeight) throw new Error('MenuBuilder drag controls are outside viewport')
      return { from: { x:a.x+12, y:a.y+a.height/2 }, to: { x:b.x+16, y:b.bottom-8 } }
    })()`)
    await t.command(target, 'Input.dispatchMouseEvent', { type:'mouseMoved', ...points.from })
    await t.command(target, 'Input.dispatchMouseEvent', { type:'mousePressed', button:'left', clickCount:1, ...points.from })
    for (let step=1; step<=12; step++) await t.command(target, 'Input.dispatchMouseEvent', { type:'mouseMoved', button:'left', buttons:1, x:points.from.x+(points.to.x-points.from.x)*step/12, y:points.from.y+(points.to.y-points.from.y)*step/12 })
    await t.command(target, 'Input.dispatchMouseEvent', { type:'mouseReleased', button:'left', clickCount:1, ...points.to })
    await t.wait(target, `Boolean(document.querySelector('.menu-item[data-id][data-kind="${kind}"]'))`, Boolean)
  }
}
async function verifyNative(t, main) {
  if (process.platform !== 'win32' || !t.native) { t.note('Native popup/tray', 'Windows native helper is unavailable on this platform.'); return }
  const shot = (surface, item, name) => t.native('capture', { Surface:surface, ItemText:item, OutputPath:`${t.dir}/${name}.png` })
  t.native('raise')
  await t.button(main, 'Context menu')
  let menu = await popup(t, main, 'Tauri is awesome!')
  t.check('Welcome native popup item labels', menu.items.map(i => i.name), ['Tauri is awesome!', 'Do something', 'Copy'])
  const checked = menu.items.find(i => i.name === 'Tauri is awesome!').checked
  if (!['On','Off'].includes(checked)) throw new Error('Native check state is not exposed')
  shot('popup', 'Tauri is awesome!', 'api-native-welcome-popup')
  t.native('menu-click', { ItemText:'Tauri is awesome!' })
  await t.button(main, 'Context menu')
  menu = await popup(t, main, 'Tauri is awesome!')
  t.check('Welcome native check item toggles', menu.items.find(i => i.name === 'Tauri is awesome!').checked, checked === 'On' ? 'Off' : 'On')
  t.native('menu-click', { ItemText:'Do something' })
  await navigate(t, main, 'Menu')
  await addMenuItems(t, main)
  await clear(t, main)
  await t.button(main, 'Popup')
  menu = await popup(t, main, 'A menu item')
  t.check('MenuBuilder popup reflects actual dragged items', menu.items.map(i => i.name), ['A menu item','A check menu'])
  shot('popup', 'A menu item', 'api-native-custom-popup')
  t.native('menu-click', { ItemText:'A menu item' })
  await expectRows(t, main, 'Native normal item callback reaches Svelte', ['Item A menu item clicked'])
  await clear(t, main)
  await t.button(main, 'Popup'); await popup(t, main, 'A check menu')
  t.native('menu-click', { ItemText:'A check menu' })
  await expectRows(t, main, 'Native check item callback reaches Svelte', ['Item A check menu clicked'])
  t.check('Native check selection updates builder checkbox', await t.wait(main, `document.querySelector('.menu-item[data-id][data-kind="Check"] input[type="checkbox"]')?.checked`, Boolean), true)
  await clear(t, main)
  await t.button(main, 'Create menu')
  let bar
  await t.wait(main, 'true', () => (bar = t.native('menubar')).items.some(item => item.name === 'app' && !item.offscreen))
  t.check('Window menu appears on visible app HWND', bar.items.filter(item => item.name === 'app' && item.enabled && !item.offscreen).map(item => item.name), ['app'])
  shot('window', '', 'api-native-menu-bar')
  t.native('menubar-click', { ItemText:'app' })
  menu = await popup(t, main, 'A menu item')
  t.check('Visible menu bar opens source-defined submenu', menu.items.map(item => item.name), ['A menu item','A check menu'])
  shot('popup', 'A menu item', 'api-native-menu-bar-open')
  t.native('menu-click', { ItemText:'A menu item' })
  await expectRows(t, main, 'Visible menu bar normal callback reaches Svelte', ['Item A menu item clicked'])
  await clear(t, main)
  t.native('menubar-click', { ItemText:'app' }); await popup(t, main, 'A check menu')
  t.native('menu-click', { ItemText:'A check menu' })
  await expectRows(t, main, 'Visible menu bar check callback reaches Svelte', ['Item A check menu clicked'])
  t.check('Visible menu bar check updates builder state', await t.wait(main, `document.querySelector('.menu-item[data-id][data-kind="Check"] input[type="checkbox"]')?.checked`, value => value === false), false)
  t.native('menubar-click', { ItemText:'app' }); menu = await popup(t, main, 'A check menu')
  t.check('Visible menu bar mirrors changed native check state', menu.items.find(item => item.name === 'A check menu').checked, 'Off')
  shot('popup', 'A check menu', 'api-native-menu-bar-unchecked')
  t.native('dismiss', { ItemText:'A check menu' })
  await navigate(t, main, 'Tray')
  await addMenuItems(t, main)
  const tooltip = `API verification ${Date.now()}`
  await setValue(t, main, `document.querySelector('input[placeholder="Tooltip"]')`, tooltip)
  await t.button(main, 'Create tray')
  await t.wait(main, `Array.from(document.querySelectorAll('button')).some(e => e.textContent.trim() === 'Remove tray')`, Boolean)
  const tray = t.native('tray-state', { ItemText:tooltip })
  if (!tray.available) t.note('Tray OS interactions', tray.reason)
  else {
    t.check('Tray unique tooltip is exposed by Windows shell', tray.name, tooltip)
    shot('tray', tooltip, 'api-native-tray-icon')
    await clear(t, main)
    t.native('tray-click', { ItemText:tooltip, Button:'right' })
    menu = await popup(t, main, 'A menu item')
    t.check('Tray menu contains actual dragged items', menu.items.map(i => i.name), ['A menu item','A check menu'])
    shot('popup', 'A menu item', 'api-native-tray-menu')
    t.native('menu-click', { ItemText:'A menu item' })
    const rows = await t.wait(main, consoleRows, rows => rows.includes('Item A menu item clicked'))
    t.check('Tray native menu callback reaches Svelte', rows.includes('Item A menu item clicked'), true)
    t.native('tray-click', { ItemText:tooltip, Button:'left' })
    await popup(t, main, 'A check menu')
    t.native('menu-click', { ItemText:'A check menu' })
    await t.wait(main, consoleRows, rows => rows.includes('Item A check menu clicked'))
    t.check('Tray left-click native check toggles builder state', await t.wait(main, `document.querySelector('.menu-item[data-id][data-kind="Check"] input[type="checkbox"]')?.checked`, Boolean), true)
  }
  await t.button(main, 'Remove tray')
  await t.wait(main, `Array.from(document.querySelectorAll('button')).some(e => e.textContent.trim() === 'Create tray')`, Boolean)
  if (tray.available) t.check('Removed tray disappears from Windows shell', t.native('tray-state', { ItemText:tooltip }).available, false)
  t.note('Tray remaining scope', 'Creation, tooltip, native menu callbacks/check state and removal are exercised when shell UIAutomation exposes the icon; icon appearance, template/title and Rust startup tray commands remain unverified.')
  await navigate(t, main, 'Welcome')
}
export async function verifyApi(t) {
  const main = t.main
  t.note('WebRTC', 'Not exercised: camera/microphone permissions, real media tracks and video playback require a device-aware test; fake streams are not evidence.')
  t.note('Other Window controls', 'Only title, create-window, physical size, resize-event UI and always-on-top state are exercised; other window, cursor, effects and taskbar operations are not verified.')
  t.note('OS stacking', 'Always-on-top is checked through the native getter after real checkbox clicks; actual desktop z-order is not visually verified by page screenshots.')
  const config = JSON.parse(readFileSync(new URL('../.cache/examples/api/src-tauri/tauri.conf.json', import.meta.url), 'utf8'))
  const cargo = readFileSync(new URL('../source/tauri/crates/tauri/Cargo.toml', import.meta.url), 'utf8')
  const tauriVersion = cargo.match(/^version\s*=\s*"([^"]+)"/m)?.[1]
  if (!tauriVersion) throw new Error('Missing Tauri version in upstream manifest')
  const identity = `[...document.querySelectorAll('main pre code')].map(e => e.textContent.trim())`
  await navigate(t, main, 'Welcome')
  const values = await t.wait(main, identity, (value) => value.length === 4 && value[0] !== 'Unknown' && value[2] !== '1.0.0')
  t.check('Welcome app name from staged configuration', values[0], config.productName)
  t.check('Welcome app version from staged configuration', values[1], config.version)
  t.check('Welcome Tauri framework version', values[2], tauriVersion)
  // Version's initial UI placeholder is also 1.0.0: independently query the real backend.
  t.check('App native identity getters', await t.evaluate(main,
    'Promise.all([window.__TAURI__.app.getName(), window.__TAURI__.app.getVersion(), window.__TAURI__.app.getTauriVersion(), window.__TAURI__.app.getIdentifier()])'),
    [config.productName, config.version, tauriVersion, config.identifier])
  const bundle = await t.evaluate(main, 'window.__TAURI__.app.getBundleType()')
  t.check('Welcome bundle type matches native read-only query', values[3], bundle || 'Unknown')
  t.note('Bundle packaging', 'The example is launched unbundled; bundle-type display consistency does not verify an installer or packaged distribution.')
  await t.capture(main, 'api-welcome-identity')
  await navigate(t, main, 'Communication')
  let start = (await t.output()).length
  await t.button(main, 'Call Log API')
  await t.wait(main, 'document.readyState',
    () => t.output().slice(start).includes('tauri-click Some("this payload is optional because we used Option in Rust")'))
  t.check('Communication Log API reaches Rust logger',
    (await t.output()).slice(start).includes('tauri-click Some("this payload is optional because we used Option in Rust")'), true)
  await clear(t, main)
  await t.button(main, 'Call Request (async) API')
  await expectRows(t, main, 'Communication structured request response', [{ message: 'message response' }])
  t.check('Communication endpoint and body reach Rust', (await t.output()).slice(start).includes('dummy endpoint arg RequestBody { id: 5, name: "test" }'), true)
  await t.capture(main, 'api-communication-request')
  await clear(t, main)
  await t.button(main, 'Echo')
  const echo = await t.wait(main, consoleRows, (rows) => rows.length >= 2)
  t.check('Communication JSON and binary echo', echo.map(JSON.parse).sort((a, b) => Number(Array.isArray(a)) - Number(Array.isArray(b))),
    [{ message: 'Tauri JSON request!' }, [1, 2, 3]])
  await t.capture(main, 'api-communication-echo')
  await clear(t, main)
  start = (await t.output()).length
  await t.button(main, 'Send event to Rust')
  const events = await t.wait(main, consoleRows, (rows) => rows.some((row) => row.includes('rust-event')))
  const replies = events.map(JSON.parse).filter((event) => event.event === 'rust-event')
  t.check('Communication event round trip reply', replies.map((event) => event.payload), [{ data: 'something else' }])
  await t.wait(main, 'document.readyState', () => t.output().slice(start).includes('got js-event with message'))
  t.check('Communication event payload reaches Rust', (await t.output()).slice(start).includes('this is the payload string'), true)
  await t.capture(main, 'api-communication-event')
  await clear(t, main)
  await t.button(main, 'Spam')
  const channel = await t.wait(main, consoleRows, (rows) => rows.length >= 1000, 60000)
  t.check('Communication channel delivers all 1000 messages in order', channel.map(Number), Array.from({ length: 1000 }, (_, i) => i + 1))
  await t.capture(main, 'api-communication-channel')
  await navigate(t, main, 'App')
  await t.capture(main, 'api-app-controls')
  t.note('App Show/Hide/Dock', process.platform === 'darwin'
    ? 'Not exercised: macOS application visibility and dock require OS-level observation; console acknowledgements alone are not proof.'
    : 'Not applicable on this platform: upstream Show, Hide and dock-visibility commands operate only on macOS; their non-macOS no-op acknowledgements are not passes.')
  await navigate(t, main, 'Window')
  const originalTitle = await t.evaluate(main, `${currentWindow}.title()`)
  const originalSize = await t.evaluate(main, `${currentWindow}.innerSize().then(s => [s.width, s.height])`)
  const titleInput = `${heading('Set Window Title')}?.parentElement.querySelector('input')`
  const selected = `${heading('Selected Window')}?.parentElement.querySelector('select')`
  const mainLabel = await t.evaluate(main, 'window.__TAURI_INTERNALS__.metadata.currentWindow.label')
  const newLabel = `verify-${Date.now()}`
  const childLabel = `main-${newLabel}`
  const title = 'API verifier: main title'
  await setValue(t, main, titleInput, title)
  await t.button(main, 'Set')
  await assertWindow(t, main, 'Window main title changed through Set control', 'title', title)
  await t.capture(main, 'api-window-main-title')
  await clear(t, main)
  await setValue(t, main, `document.querySelector('input[placeholder="New window label.."]')`, newLabel)
  await t.button(main, 'Create')
  await expectRows(t, main, 'Window creation emits created event', ['webview created'])
  const child = await t.target(async (target) => target.type === 'page'
    && (await t.evaluate(target, 'window.__TAURI_INTERNALS__?.metadata?.currentWindow?.label')) === childLabel)
  t.check('Window new child has requested label', await t.evaluate(child, 'window.__TAURI_INTERNALS__.metadata.currentWindow.label'), childLabel)
  await t.wait(child, `document.querySelector('main h1')?.textContent.trim()`, (value) => value === 'Welcome')
  await t.capture(child, 'api-window-child-created')
  const initialChildSize = await t.evaluate(child, `${currentWindow}.innerSize().then(s => [s.width, s.height])`)
  await setValue(t, main, selected, childLabel, true)
  await t.wait(main, `(${selected})?.value`, (value) => value === childLabel)
  await t.wait(main, `[Number((${field('Width')})?.value), Number((${field('Height')})?.value)]`,
    (value) => json(value) === json(initialChildSize))
  await setValue(t, main, titleInput, 'API verifier: child title')
  await t.button(main, 'Set')
  await assertWindow(t, child, 'Window selected child title changes', 'title', 'API verifier: child title')
  await assertWindow(t, main, 'Window child title does not change main', 'title', title)
  const scale = await t.evaluate(child, `${currentWindow}.scaleFactor()`)
  const childSize = [Math.round(720 * scale), Math.round(520 * scale)]
  await setValue(t, main, field('Width'), childSize[0])
  await setValue(t, main, field('Height'), childSize[1], true)
  const size = await t.wait(child, `${currentWindow}.innerSize().then(s => [s.width, s.height])`, (value) => json(value) === json(childSize))
  t.check('Window size controls set child physical dimensions', size, childSize)
  await displayedSize(t, main, childSize)
  t.check('Window child resize does not change main', await t.evaluate(main, `${currentWindow}.innerSize().then(s => [s.width, s.height])`), originalSize)
  const top = field('Always on top')
  t.check('Window always-on-top starts false', await t.evaluate(child, `${currentWindow}.isAlwaysOnTop()`), false)
  await t.click(main, await selector(t, main, top))
  await assertWindow(t, child, 'Window checkbox enables native always-on-top', 'isAlwaysOnTop', true)
  t.check('Window topmost checkbox reflects enabled control', await t.evaluate(main, `(${top}).checked`), true)
  await t.capture(main, 'api-window-child-controls')
  await t.capture(child, 'api-window-child-resized')
  await t.click(main, await selector(t, main, top))
  await assertWindow(t, child, 'Window checkbox disables native always-on-top', 'isAlwaysOnTop', false)
  await setValue(t, main, selected, mainLabel, true)
  await setValue(t, main, titleInput, originalTitle)
  await t.button(main, 'Set')
  await assertWindow(t, main, 'Window main title restored through Set control', 'title', originalTitle)
  // Native close is cleanup only; creation and all state changes above use app controls.
  await t.command(child, 'Page.close', {})
  const closed = await t.wait(main,
    `window.__TAURI__.window.getAllWindows().then(windows => windows.map(w => w.label))`,
    (labels) => !labels.includes(childLabel))
  t.check('Window child is removed after cleanup', closed.includes(childLabel), false)
  await navigate(t, main, 'Welcome')
  await verifyNative(t, main)
  await t.capture(main, 'api-final-welcome')
}
