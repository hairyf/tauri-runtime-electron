// Exercise the library's native effects, then the original transparency and window actions.
// Renderer styles and successful IPC alone do not certify the native visual result.
import { readFileSync } from 'node:fs'
import { release } from 'node:os'

export default async function transparentChecks(test, { assert, click, until, native, nativeWindow, nativeWindows }) {
  const config = JSON.parse(readFileSync(new URL('../../example/transparent/tauri.conf.json', import.meta.url), 'utf8'))
  assert.equal(config.app.withGlobalTauri, true)
  assert.equal(config.app.windows.length, 1)
  const configured = config.app.windows[0]
  for (const [key, value] of Object.entries({ label: 'main', title: 'Transparent Window', width: 760, height: 560, center: true, resizable: false, transparent: true, decorations: false, shadow: false, backgroundColor: '#00000000' })) {
    assert.equal(configured[key], value, `transparent window config: ${key}`)
  }
  const permissions = config.app.security.capabilities.flatMap(capability => capability.permissions)
  assert.ok(!permissions.includes('core:window:allow-set-effects'))
  assert.ok(permissions.includes('electron-window:allow-set-effects'))
  assert.ok(config.build.frontendDist.includes('window-api.js'))

  const page = await test.page('main')
  const read = () => page.evaluate(`(() => {
    const root = document.documentElement, body = document.body
    const panel = document.querySelector('#panel'), slider = document.querySelector('#opacity')
    const transparency = document.querySelector('#transparency'), blur = document.querySelector('#blur')
    const style = getComputedStyle(panel), rootStyle = getComputedStyle(root)
    const controls = [...document.querySelectorAll('h1, p, label, output, button, input')]
    const unfaded = controls.every(el => {
      for (let node = el; node; node = node.parentElement) {
        const s = getComputedStyle(node)
        if (s.opacity !== '1' || s.filter !== 'none') return false
      }
      return true
    })
    return {
      title: document.title, size: [innerWidth, innerHeight], scale: devicePixelRatio,
      backgrounds: [rootStyle.backgroundColor, getComputedStyle(body).backgroundColor],
      padding: ['Top', 'Right', 'Bottom', 'Left'].map(side => getComputedStyle(body)['padding' + side]),
      background: style.backgroundColor, alpha: Number(rootStyle.getPropertyValue('--panel-alpha')),
      backdrop: style.backdropFilter,
      transparent: transparency.checked, slider: slider.value, range: [slider.type, slider.min, slider.max],
      output: document.querySelector('#opacity-value').value,
      disabled: [slider.disabled, blur.disabled], blur: blur.checked,
      busy: blur.getAttribute('aria-busy') === 'true',
      drag: document.querySelector('#drag-region').hasAttribute('data-tauri-drag-region'),
      unfaded, text: controls.filter(el => el.id !== 'status' && el.matches('h1, p, button')).map(el => ({ text: el.textContent, color: getComputedStyle(el).color })),
      status: document.querySelector('#status').textContent
    }
  })()`)
  const initial = await until(read, value => value.output === '72%' && !value.busy)
  assert.equal(initial.title, 'Transparent Window')
  assert.ok(initial.size.every(value => value > 0) && initial.scale > 0, 'renderer has a visible viewport and valid pixel scale')
  assert.deepEqual(initial.range, ['range', '15', '100'])
  assert.equal(initial.drag, true)
  let window
  if (process.platform === 'win32') {
    window = nativeWindow(await nativeWindows(test, ['Transparent Window']), 'Transparent Window')
    assert.ok(window.visible && !window.minimized && !window.maximized)
  }
  const nativeShot = async (name) => {
    if (!window) return
    try { await test.nativeShot(window, name) }
    catch (error) {
      if (!error.blocked) throw error
      test.limitations.push(`Native transparent-window keyframe unavailable: ${error.message}`)
    }
  }
  const supported = !initial.disabled[1]
  const [major, , build] = release().split('.').map(Number)
  if (process.platform === 'win32' && (major > 10 || (major === 10 && build >= 22621))) {
    assert.equal(supported, true, `native effects must initialize on supported Windows: ${initial.status}`)
  }
  assert.equal(initial.blur, false, 'startup must clear any native effect left by reload')
  if (supported) {
    assert.equal(initial.status, '透明层已就绪')
    await nativeShot('00-effects-clear-before')
    // Invoke the exported library API, never the upstream generic setEffects path.
    for (const effect of ['Acrylic', 'Mica', 'Tabbed']) {
      await page.evaluate(`(async () => {
        const { getCurrentWindow, Effect } = await import('./window-api.js')
        await getCurrentWindow().setEffects({ effects: [Effect.${effect}] })
      })()`)
      await test.shot(page, `00-effects-${effect.toLowerCase()}-renderer`)
      await nativeShot(`00-effects-${effect.toLowerCase()}-native`)
      await page.evaluate(`(async () => {
        const { getCurrentWindow } = await import('./window-api.js')
        await getCurrentWindow().clearEffects()
        await getCurrentWindow().clearEffects()
      })()`)
      await nativeShot(`00-effects-${effect.toLowerCase()}-cleared`)
    }
    const rejected = await page.evaluate(`(async () => {
      const { getCurrentWindow, Effect } = await import('./window-api.js')
      try { await getCurrentWindow().setEffects({ effects: [Effect.Blur] }); return null }
      catch (error) { return String(error) }
    })()`)
    assert.match(rejected ?? '', /unsupported|not supported|不支持/i, 'unsupported effects must reject explicitly')
    for (let cycle = 0; cycle < 2; cycle++) {
      await click(page, '#blur')
      await until(read, value => !value.busy && value.blur && value.status.includes('Acrylic'))
      await nativeShot(`00-toggle-${cycle}-enabled`)
      await click(page, '#blur')
      await until(read, value => !value.busy && !value.blur && value.status === '透明层已就绪')
      await nativeShot(`00-toggle-${cycle}-cleared`)
    }
    // Queue competing requests in one turn: the final native command must clear.
    await page.evaluate(`(() => {
      const input = document.querySelector('#blur')
      for (const checked of [true, false, true, false]) {
        input.checked = checked
        input.dispatchEvent(new Event('change', { bubbles: true }))
      }
    })()`)
    await until(read, value => !value.busy && !value.blur && value.status === '透明层已就绪')
    await nativeShot('00-queued-toggles-cleared')
    await click(page, '#blur')
    await until(read, value => !value.busy && value.blur)
    await click(page, '#reset')
    await until(read, value => !value.busy && !value.blur && value.status === '透明层已就绪')
    await nativeShot('00-reset-cleared')
    test.pass('library setEffects/clearEffects: Acrylic/Mica/Tabbed, explicit unsupported rejection, repeated restores, serialized toggles and reset')
    test.limitations.push('Native effect before/after screenshots require visual review; successful IPC and renderer CSS do not alone certify the system backdrop.')
  } else {
    assert.match(initial.status, /不可用|失败/)
    test.limitations.push(`Native effects unsupported or failed; the disabled control displays the actual error: ${initial.status}`)
  }

  const appearance = async ({ slider = '72', transparent = true } = {}) => {
    const alpha = transparent ? Number(slider) / 100 : 1
    const value = await until(read, value => !value.busy && value.slider === slider && value.transparent === transparent && value.alpha === alpha && value.output === `${Math.round(alpha * 100)}%`)
    assert.deepEqual(value.size, initial.size, 'appearance updates and reload do not resize the fixed window')
    assert.deepEqual(value.backgrounds, ['rgba(0, 0, 0, 0)', 'rgba(0, 0, 0, 0)'], 'html and body remain transparent in every state')
    assert.deepEqual(value.padding, ['24px', '24px', '24px', '24px'])
    const channels = value.background.match(/[\d.]+/g)?.map(Number)
    assert.deepEqual(channels?.slice(0, 3), [18, 27, 45], 'only the panel background alpha changes')
    assert.equal(channels?.[3] ?? 1, alpha)
    assert.deepEqual(value.disabled, [!transparent, !supported])
    assert.equal(value.blur, false, 'native effects remain cleared during alpha checks')
    assert.equal(value.backdrop, 'none')
    assert.equal(value.unfaded, true, 'text and controls must not inherit element opacity or a fading filter')
    assert.deepEqual(value.text, initial.text, 'text, button labels and their colors remain unchanged')
    assert.equal(value.status, initial.status, 'appearance changes do not change the native effect status')
    return value
  }
  const key = async (key, windowsVirtualKeyCode) => {
    await page.call('Input.dispatchKeyEvent', { type: 'keyDown', key, windowsVirtualKeyCode })
    await page.call('Input.dispatchKeyEvent', { type: 'keyUp', key, windowsVirtualKeyCode })
  }
  const slider = async (value) => {
    await click(page, '#opacity')
    await key(value === 100 ? 'End' : 'Home', value === 100 ? 35 : 36)
    if (value !== 100) for (let number = 15; number < value; number++) await key('ArrowRight', 39)
  }
  await appearance()
  await test.shot(page, '01-transparent-default')
  await slider(15)
  await appearance({ slider: '15' })
  await test.shot(page, '02-opacity-minimum')
  await slider(50)
  await appearance({ slider: '50' })
  await test.shot(page, '03-opacity-middle')
  await slider(100)
  await appearance({ slider: '100' })
  await test.shot(page, '04-opacity-maximum')
  await slider(50)
  await appearance({ slider: '50' })
  await click(page, '#transparency')
  await appearance({ slider: '50', transparent: false })
  await test.shot(page, '05-transparency-disabled')
  await click(page, '#transparency')
  await appearance({ slider: '50' })
  await test.shot(page, '06-transparency-restored')
  // Reload with nondefault values, so resetting is not a vacuous check.
  await click(page, '#transparency')
  await appearance({ slider: '50', transparent: false })
  if (supported) {
    await click(page, '#blur')
    await until(read, value => !value.busy && value.blur)
    await nativeShot('06-native-effect-before-reload')
  }
  await page.call('Page.reload')
  await until(() => page.evaluate('document.readyState === "complete" && Boolean(window.__TAURI__?.window)'), Boolean)
  await appearance()
  await test.shot(page, '07-defaults-after-reload')
  await nativeShot(supported ? '07-native-effect-cleared-after-reload' : '07-native-effects-unavailable-after-reload')
  test.pass(`original transparent page: minimum/middle/maximum alpha and output, disable/restore, unfaded controls and reload restores defaults${supported ? ' and clears native effects' : '; native effect rejection remains visible'}`)

  if (process.platform === 'win32') {
    // Native client pixels are rounded independently of the CSS viewport at fractional DPI.
    for (const [index, axis] of ['width', 'height'].entries()) {
      assert.ok(Math.abs(window.clientRect[axis] - initial.size[index] * initial.scale) <= initial.scale, `${axis}: native client size matches the renderer within one CSS pixel`)
    }
    await nativeShot('10-native-before-drag')
    const point = await page.evaluate(`(() => {
      const el = document.querySelector('#drag-region'), r = el.getBoundingClientRect()
      const x = r.x + r.width / 2, y = r.y + r.height / 2
      if (document.elementFromPoint(x, y) !== el) throw new Error('drag pointer must hit the original region')
      return { x, y, width: innerWidth, height: innerHeight }
    })()`)
    const current = (await native(test, 'Rect', { WindowHandle: window.hwnd })).window
    const start = {
      x: Math.round(current.clientRect.left + point.x * current.clientRect.width / point.width),
      y: Math.round(current.clientRect.top + point.y * current.clientRect.height / point.height),
    }
    // Stay inside the painted panel: fully transparent margins may hit-test the desktop.
    const moved = await native(test, 'Drag', { WindowHandle: window.hwnd, StartX: start.x, StartY: start.y, EndX: start.x + 36, EndY: start.y + 20, Expected: 'Moved', Activate: true, DurationMs: 800 })
    assert.equal(moved.assertion.passed, true)
    assert.deepEqual([moved.after.width, moved.after.height], [moved.before.width, moved.before.height])
    await appearance()
    await test.shot(page, '11-renderer-after-native-drag')
    await nativeShot('12-native-after-drag')
    test.pass('runner-owned transparent native window moves by the physical mouse delta without resizing')
  } else {
    test.limitations.push('BLOCKED: Windows native transparent-window snapshots, physical drag and minimized-state observation are unavailable on this platform; renderer checks do not certify those native behaviors.')
  }
  // Minimize last: this example deliberately has no unminimize/setFocus grants.
  await click(page, '#minimize')
  if (window) {
    await until(() => native(test, 'Rect', { WindowHandle: window.hwnd }), value => value.window.minimized)
    assert.equal(await page.evaluate('document.querySelector("#status").textContent'), initial.status)
    test.pass('original minimize button produces the native minimized state')
  }
  // DOM activation still exercises the original close handler while minimized;
  // CDP mouse input/bringToFront cannot be relied on to restore this window.
  await page.evaluate('document.querySelector("#close").click()').catch(error => {
    if (!/CDP target closed/.test(error.message)) throw error
  })
  await until(() => test.child.exitCode, code => code !== null, 20_000)
  assert.equal(test.child.exitCode, 0, 'original close button exits the runner normally')
  test.pass('original close handler closes the last window and exits zero without a restore permission')
}
