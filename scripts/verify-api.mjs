import { readFileSync } from 'node:fs'
import { verifyStartupTray } from './verify-native-cases.mjs'
import { observer } from './verify-embedded.mjs'
const json = JSON.stringify
const consoleRows = `Array.from(document.querySelectorAll('#console pre'), e => e.textContent.replace(/^\\[[^\\]]+\\]:\\s*/, '').trim())`
const currentWindow = 'window.__TAURI__.window.getCurrentWindow()'
const heading = (text) => `[...document.querySelectorAll('main h4')].find(e => e.textContent.trim() === ${json(text)})`
const field = (text) => `[...document.querySelectorAll('main label, main div')].find(e => [...e.childNodes].filter(n => n.nodeType === Node.TEXT_NODE).map(n => n.textContent).join(' ').trim() === ${json(text)} && e.querySelector('input'))?.querySelector('input')`
const windowToggles = [
  ['Resizable', 'isResizable', true], ['Maximizable', 'isMaximizable', true],
  ['Minimizable', 'isMinimizable', true], ['Closable', 'isClosable', true],
  ['Has decorations', 'isDecorated', true], ['Maximized', 'isMaximized', false],
  ['Fullscreen', 'isFullscreen', false],
]
const geometry = `Promise.all([${currentWindow}.outerPosition(), ${currentWindow}.outerSize(), window.__TAURI__.window.currentMonitor()]).then(([p,s,m]) => ({ x:p.x, y:p.y, width:s.width, height:s.height, workArea:m?.workArea }))`
const centered = ({ x, y, width, height, workArea }) => Boolean(workArea)
  && Math.abs(x + width / 2 - workArea.position.x - workArea.size.width / 2) <= 2
  && Math.abs(y + height / 2 - workArea.position.y - workArea.size.height / 2) <= 2
// Offline helper checks only: no application startup, staging or UI proof.
if (process.argv.includes('--self-check')) {
  const rows = Function('document', 'return ' + consoleRows)({
    querySelectorAll: () => ['[1:23:45 AM]: {"message":"ok"}', '[13:23:45]: 1000'].map(textContent => ({ textContent }))
  })
  if (json(rows) !== json(['{"message":"ok"}', '1000'])) throw new Error('Console timestamp parsing failed: ' + json(rows))
  const bounds = { x:-1020, y:160, width:800, height:600, workArea:{ position:{ x:-1580, y:40 }, size:{ width:1920, height:840 } } }
  if (!centered(bounds) || centered({ ...bounds, y:180 }) || centered({ ...bounds, workArea:null })) throw new Error('Window centering geometry check failed')
  const viewport = { left:300, right:1000, top:107, bottom:535 }
  const source = { getBoundingClientRect: () => ({ left:320, right:560, top:90, bottom:125 }) }
  let scrolled = false, obscured = false
  const dest = { scrollIntoView: () => { scrolled = true }, getBoundingClientRect: () => ({ left:320, right:968, top:518, bottom:614 }), contains: element => element === dest }
  const placeholder = { children:[], textContent:'Drag and drop the menu items above to start building', parentElement:dest }
  const document = {
    querySelector: selector => selector === 'main h1' ? { nextElementSibling:{ getBoundingClientRect:() => viewport } } : selector.includes(':not') ? source : null,
    querySelectorAll: () => [{ children:[placeholder], textContent:placeholder.textContent }, placeholder],
    elementFromPoint: (x, y) => obscured ? null : y < 125 ? source : dest,
  }
  const points = Function('document', `return (${menuDragPoints})('Normal', ${menuDragPoint})`)(document)
  if (!scrolled || points.from.y <= 107 || points.to.y !== 534) throw new Error('Menu drag must use the leaf placeholder dropzone and visible content bounds')
  obscured = true
  let rejected = false
  try { Function('document', `return (${menuDragPoints})('Normal', ${menuDragPoint})`)(document) } catch { rejected = true }
  if (!rejected) throw new Error('Menu drag must reject obscured points')
  console.log('API console, geometry and menu drag self-check passed (no UI verification performed)')
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
async function verifyWindowControls(t, main, child, childSize, scale) {
  const childTitle = await t.evaluate(child, `${currentWindow}.title()`)
  const mainPosition = await t.evaluate(main, `${currentWindow}.outerPosition().then(p => [p.x,p.y])`)
  const originalPosition = await t.evaluate(child, `${currentWindow}.outerPosition().then(p => [p.x,p.y])`)
  const originalTheme = await t.evaluate(child, `${currentWindow}.theme()`)
  const mainMedia = await t.evaluate(main, `matchMedia('(prefers-color-scheme: dark)').matches`)
  const workArea = (await t.evaluate(child, geometry)).workArea
  const failures = []
  const attempt = async (operation) => {
    try { await operation() }
    catch (error) {
      failures.push(error)
      try {
        await t.capture(main, `api-window-failure-${failures.length}-controls`)
        await t.capture(child, `api-window-failure-${failures.length}-child`)
      } catch (captureError) { failures.push(captureError) }
    }
  }
  const sizeExpression = `${currentWindow}.innerSize().then(s => [s.width,s.height])`
  const positionExpression = `${currentWindow}.outerPosition().then(p => [p.x,p.y])`
  const capture = async (name) => {
    await t.capture(main, `api-window-${name}-controls`)
    await t.capture(child, `api-window-${name}-child`)
    if (process.platform === 'win32' && t.native) await t.native('capture', { Title:childTitle, OutputPath:`${t.dir}/api-window-${name}-native.png` })
  }
  const dimensions = async (width, height) => {
    await setValue(t, main, field('Width'), width)
    await setValue(t, main, field('Height'), height, true)
  }
  const position = async ([x,y]) => {
    await setValue(t, main, field('X'), x)
    await setValue(t, main, field('Y'), y, true)
  }
  const displayedPosition = async (expected) => {
    const expression = `(() => {
      const title = [...document.querySelectorAll('main div')].find(e => e.textContent.trim() === 'Outer Position')
      return title ? [...title.parentElement.querySelectorAll('span')].map(e => Number(e.textContent.split(':')[1])) : []
    })()`
    t.check('Window move event updates displayed outer position', await t.wait(main, expression, value => json(value) === json(expected)), expected)
  }
  try {
    await attempt(async () => {
      const moved = [originalPosition[0] + Math.round(24 * scale), originalPosition[1] + Math.round(24 * scale)]
      await position(moved)
      t.check('Window X/Y controls move selected child', await t.wait(child, positionExpression, value => json(value) === json(moved)), moved)
      await displayedPosition(moved)
      t.check('Window child move leaves main position unchanged', await t.evaluate(main, positionExpression), mainPosition)
      await capture('moved')
    })
    await attempt(async () => {
      await t.button(main, 'Center')
      const bounds = await t.wait(child, geometry, centered)
      t.check('Window Center aligns child with monitor work area', centered(bounds), true)
      await displayedPosition([bounds.x,bounds.y])
      await capture('centered')
    })

    for (const [button, getter, active, restore, rows] of [
      ['Minimize', 'isMinimized', true, 'unminimize()', null],
      ['Hide', 'isVisible', false, 'show()', ['window is visible','window is invisible','window is visible']],
      ['Disable', 'isEnabled', false, 'setEnabled(true)', ['window is enabled','window is disabled','window is enabled']],
    ]) {
      await attempt(async () => {
        await clear(t, main)
        try {
          await t.button(main, button)
          // Observe the real transient before any cold native helper can consume its 2s interval.
          await assertWindow(t, child, `Window ${button} enters native transient state`, getter, active)
          await assertWindow(t, main, `Window ${button} leaves controller usable`, getter, !active)
          await t.capture(main, `api-window-${button.toLowerCase()}-observed`)
          await assertWindow(t, child, `Window ${button} source timer restores child`, getter, !active)
          if (rows) await expectRows(t, main, `Window ${button} source observations`, rows)
          await capture(`${button.toLowerCase()}-restored`)
        } finally {
          // Recovery is cleanup only; the source timer must pass before this setter runs.
          await t.evaluate(child, `${currentWindow}.${restore}`)
        }
      })
    }

    for (const [label, getter, initial] of windowToggles) {
      if (process.platform === 'linux' && ['Maximizable','Minimizable'].includes(label)) {
        t.note(`Window ${label}`, 'The pinned Tauri API does not support native maximize/minimize button state on Linux.')
        continue
      }
      if (label === 'Has decorations' && process.platform !== 'win32') {
        t.note('Window decorations', 'Changing an existing BrowserWindow frame is implemented only on Windows by this runtime.')
        continue
      }
      await attempt(async () => {
        const control = field(label)
        await assertWindow(t, child, `Window ${label} initial native state`, getter, initial)
        try {
          await t.click(main, await selector(t, main, control))
          await assertWindow(t, child, `Window ${label} checkbox changes selected child`, getter, !initial)
          t.check(`Window ${label} checkbox reflects changed state`, await t.evaluate(main, `(${control}).checked`), !initial)
          if (label === 'Maximized') {
            const size = await t.wait(child, sizeExpression, value => value[0] > childSize[0] || value[1] > childSize[1])
            t.check('Window Maximized enlarges child client area', size[0] > childSize[0] || size[1] > childSize[1], true)
          }
          if (label === 'Fullscreen') {
            const monitor = await t.evaluate(child, 'window.__TAURI__.window.currentMonitor()')
            const expected = [monitor.size.width,monitor.size.height]
            const size = await t.wait(child, `${currentWindow}.outerSize().then(s => [s.width,s.height])`, value => json(value) === json(expected))
            t.check('Window Fullscreen covers selected monitor', size, expected)
          }
          await capture(label.toLowerCase().replaceAll(' ', '-'))
        } finally {
          if (await t.evaluate(main, `(${control}).checked`) !== initial) await t.click(main, await selector(t, main, control))
          await assertWindow(t, child, `Window ${label} restored`, getter, initial)
        }
        await capture(`${label.toLowerCase().replaceAll(' ', '-')}-restored`)
      })
    }

    await attempt(async () => {
      try {
        for (const [previous, next] of [['auto','dark'],['dark','light'],['light','auto']]) {
          await t.button(main, `Switch Theme (${previous})`)
          await assertWindow(t, child, `Window Switch Theme ${next} changes selected child`, 'theme', next === 'auto' ? originalTheme : next)
          const dark = next === 'auto' ? originalTheme === 'dark' : next === 'dark'
          t.check(`Window ${next} theme reaches real child media query`, await t.wait(child, `matchMedia('(prefers-color-scheme: dark)').matches`, value => value === dark), dark)
          t.check('Window child theme leaves controller media query unchanged', await t.evaluate(main, `matchMedia('(prefers-color-scheme: dark)').matches`), mainMedia)
          await capture(`theme-${next}`)
        }
      } finally {
        try {
          for (const theme of ['dark','light']) {
            if (await t.evaluate(main, `Array.from(document.querySelectorAll('main button')).some(e => e.textContent.trim() === ${json(`Switch Theme (${theme})`)})`)) await t.button(main, `Switch Theme (${theme})`)
          }
        } finally { await t.evaluate(child, `${currentWindow}.setTheme(null)`) }
      }
    })

    // Source requires max width >800 and max height >400; fit the request and native frame.
    const maxWidth = Math.min(900, Math.floor(workArea.size.width / scale) - 64)
    const maxHeight = Math.min(560, Math.floor(workArea.size.height / scale) - 96)
    for (const [kind, width, height, delta] of [['Min',600,440,-40], ['Max',maxWidth,maxHeight,24]]) {
      if (kind === 'Max' && (width <= 800 || height <= 400)) {
        t.note('Window maximum size', 'Monitor work area cannot fit the source-required maximum dimensions plus the native frame.')
        continue
      }
      await attempt(async () => {
        const expected = [Math.round(width * scale),Math.round(height * scale)]
        const requested = [Math.round((width + delta) * scale),Math.round((height + delta) * scale)]
        try {
          await setValue(t, main, field(`${kind} width`), width)
          await setValue(t, main, field(`${kind} height`), height, true)
          await dimensions(...requested)
          t.check(`Window ${kind} inner size constrains out-of-range request`, await t.wait(child, sizeExpression, value => json(value) === json(expected)), expected)
          await displayedSize(t, main, expected)
          await capture(`${kind.toLowerCase()}imum-size`)
          await setValue(t, main, field(`${kind} width`), '')
          await setValue(t, main, field(`${kind} height`), '', true)
          await dimensions(...requested)
          t.check(`Window clearing ${kind} constraint permits previous out-of-range size`, await t.wait(child, sizeExpression, value => json(value) === json(requested)), requested)
          await displayedSize(t, main, requested)
          await capture(`${kind.toLowerCase()}-constraint-cleared`)
        } finally {
          await setValue(t, main, field(`${kind} width`), '')
          await setValue(t, main, field(`${kind} height`), '', true)
          await dimensions(...childSize)
          t.check(`Window ${kind} cleanup restores child size`, await t.wait(child, sizeExpression, value => json(value) === json(childSize)), childSize)
          await displayedSize(t, main, childSize)
        }
      })
    }
    await capture('constraints-restored')
  } finally {
    await position(originalPosition)
    t.check('Window child original position restored', await t.wait(child, positionExpression, value => json(value) === json(originalPosition)), originalPosition)
  }
  if (failures.length) throw new AggregateError(failures, failures.map(error => error.message).join('\n'))
}
async function popup(t, target, item) {
  let menus = []
  await t.wait(target, 'true', () => (menus = t.native('popup')).some(menu => menu.items.some(entry => entry.name === item)))
  return menus.find(menu => menu.items.some(entry => entry.name === item))
}
function menuDragPoint(rect, viewport, end = false) {
  const x = Math.max(rect.left + 12, viewport.left + 1)
  const top = Math.max(rect.top + 1, viewport.top + 1)
  const bottom = Math.min(rect.bottom - 8, viewport.bottom - 1)
  if (x >= Math.min(rect.right, viewport.right) || top > bottom) throw new Error('MenuBuilder drag control is outside content viewport')
  return { x, y: end ? bottom : (top + bottom) / 2 }
}
function menuDragPoints(kind, point) {
  const item = document.querySelector(`.menu-item[data-kind="${kind}"]:not([data-id])`)
  const dest = document.querySelector('.menu-item[data-id]')?.parentElement ?? [...document.querySelectorAll('main div')].find(e => !e.children.length && e.textContent.trim() === 'Drag and drop the menu items above to start building')?.parentElement
  if (!item || !dest) throw new Error('MenuBuilder drag controls missing')
  dest.scrollIntoView({ block: 'nearest' })
  const viewport = document.querySelector('main h1').nextElementSibling.getBoundingClientRect()
  const from = point(item.getBoundingClientRect(), viewport), to = point(dest.getBoundingClientRect(), viewport, true)
  if (document.elementFromPoint(from.x, from.y) !== item || !dest.contains(document.elementFromPoint(to.x, to.y))) throw new Error('MenuBuilder drag point is obscured')
  return { from, to }
}
async function addMenuItems(t, target) {
  for (const kind of ['Normal', 'Check']) {
    const points = await t.evaluate(target, `(${menuDragPoints})(${json(kind)}, ${menuDragPoint})`)
    let drag
    const cdp = await observer(target, event => { if (event.method === 'Input.dragIntercepted') drag = event.params.data })
    try {
      await cdp.send('Input.setInterceptDrags', { enabled:true })
      await cdp.send('Input.dispatchMouseEvent', { type:'mouseMoved', ...points.from })
      await cdp.send('Input.dispatchMouseEvent', { type:'mousePressed', button:'left', buttons:1, clickCount:1, ...points.from })
      for (let step=1; step<=12 && !drag; step++) await cdp.send('Input.dispatchMouseEvent', { type:'mouseMoved', button:'left', buttons:1, x:points.from.x+(points.to.x-points.from.x)*step/12, y:points.from.y+(points.to.y-points.from.y)*step/12 })
      await t.wait(target, 'true', () => Boolean(drag))
      // Replay Chromium's real DragData, not synthetic DOM events or builder mutations.
      for (const type of ['dragEnter', 'dragOver', 'drop']) await cdp.send('Input.dispatchDragEvent', { type, ...points.to, data:drag })
      await t.wait(target, `Boolean(document.querySelector('.menu-item[data-id][data-kind="${kind}"]'))`, Boolean)
    } finally {
      try { await cdp.send('Input.cancelDragging') }
      finally {
        try { await cdp.send('Input.dispatchMouseEvent', { type:'mouseReleased', button:'left', clickCount:1, ...points.to }) }
        finally {
          try { await cdp.send('Input.setInterceptDrags', { enabled:false }) }
          finally { cdp.close() }
        }
      }
    }
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
  t.note('Tray remaining scope', 'Creation, tooltip, native menu callbacks/check state and removal are exercised when shell UIAutomation exposes the icon; icon appearance and template/title remain unverified.')
  await navigate(t, main, 'Welcome')
  await verifyStartupTray(t, main)
}
export async function verifyApi(t) {
  const main = t.main
  t.note('WebRTC', 'Not exercised: camera/microphone permissions, real media tracks and video playback require a device-aware test; fake streams are not evidence.')
  t.note('Remaining Window scope', 'Cursor grab/visibility/icon/position/ignore-events, window icon appearance, focusable/attention, always-on-bottom stacking, content protection, progress/taskbar appearance, effects and monitor-specific/simple fullscreen remain unverified. Getter checks for button flags do not prove clicking native titlebar buttons.')
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
  let windowError
  try {
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
    try {
      await t.click(main, await selector(t, main, top))
      await assertWindow(t, child, 'Window checkbox enables native always-on-top', 'isAlwaysOnTop', true)
      t.check('Window topmost checkbox reflects enabled control', await t.evaluate(main, `(${top}).checked`), true)
      await t.capture(main, 'api-window-child-controls')
      await t.capture(child, 'api-window-child-resized')
    } finally {
      if (await t.evaluate(main, `(${top}).checked`)) await t.click(main, await selector(t, main, top))
      await assertWindow(t, child, 'Window checkbox disables native always-on-top', 'isAlwaysOnTop', false)
    }
    await verifyWindowControls(t, main, child, childSize, scale)
    t.check('Window child controls leave main physical size unchanged', await t.evaluate(main, `${currentWindow}.innerSize().then(s => [s.width,s.height])`), originalSize)
  } catch (error) { windowError = error }
  finally {
    try {
      await setValue(t, main, selected, mainLabel, true)
      await setValue(t, main, titleInput, originalTitle)
      await t.button(main, 'Set')
      await assertWindow(t, main, 'Window main title restored through Set control', 'title', originalTitle)
    } finally {
      // Close only this verifier-created child, even after a failed state assertion.
      await t.command(child, 'Page.close', {})
    }
  }
  const closed = await t.wait(main,
    `window.__TAURI__.window.getAllWindows().then(windows => windows.map(w => w.label))`,
    (labels) => !labels.includes(childLabel))
  t.check('Window child is removed after cleanup', closed.includes(childLabel), false)
  await navigate(t, main, 'Welcome')
  try {
    await verifyNative(t, main)
    await t.capture(main, 'api-final-welcome')
  } catch (error) {
    if (windowError) throw new AggregateError([windowError,error], `${windowError.message}\n${error.message}`)
    throw error
  }
  if (windowError) throw windowError
}
