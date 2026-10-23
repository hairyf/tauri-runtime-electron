// The original page owns every appearance update and window action; screenshots
// record its real states, not a replacement page or a synthetic desktop backdrop.
import { readFileSync } from 'node:fs'

export default async function transparentChecks(test, { assert, click, until, native, nativeWindow, nativeWindows }) {
  const config = JSON.parse(readFileSync(new URL('../example/transparent/tauri.conf.json', import.meta.url), 'utf8'))
  assert.equal(config.app.withGlobalTauri, true)
  assert.equal(config.app.windows.length, 1)
  const configured = config.app.windows[0]
  for (const [key, value] of Object.entries({ label: 'main', title: 'Transparent Window', width: 760, height: 560, center: true, resizable: false, transparent: true, decorations: false, shadow: false, backgroundColor: '#00000000' })) {
    assert.equal(configured[key], value, `transparent window config: ${key}`)
  }

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
      blurVariable: rootStyle.getPropertyValue('--panel-blur').trim(), backdrop: style.backdropFilter,
      blurSupported: CSS.supports('backdrop-filter', 'blur(16px)'),
      transparent: transparency.checked, slider: slider.value, range: [slider.type, slider.min, slider.max],
      output: document.querySelector('#opacity-value').value,
      disabled: [slider.disabled, blur.disabled], blur: blur.checked,
      drag: document.querySelector('#drag-region').hasAttribute('data-tauri-drag-region'),
      unfaded, text: controls.filter(el => el.id !== 'status' && el.matches('h1, p, button')).map(el => ({ text: el.textContent, color: getComputedStyle(el).color })),
      effect: document.querySelector('#status').dataset.effect,
      status: document.querySelector('#status').textContent
    }
  })()`)
  const initial = await until(read, value => value.output === '72%')
  assert.equal(initial.title, 'Transparent Window')
  assert.ok(initial.size.every(value => value > 0) && initial.scale > 0, 'renderer has a visible viewport and valid pixel scale')
  assert.deepEqual(initial.range, ['range', '15', '100'])
  assert.equal(initial.drag, true)
  if (!initial.blurSupported) test.limitations.push('CSS backdrop-filter is unsupported in this renderer; the CSS variable toggle is checked, but no blur rendering is certified.')
  test.limitations.push('Native Acrylic request completion and on/off desktop keyframes are recorded on Windows; OS composition and blur strength still require visual review, not just CSS assertions.')

  const appearance = async ({ slider = '72', transparent = true, blur = false } = {}) => {
    const alpha = transparent ? Number(slider) / 100 : 1
    const value = await until(read, value => value.slider === slider && value.transparent === transparent && value.blur === blur && value.alpha === alpha && value.output === `${Math.round(alpha * 100)}%` && value.effect === (transparent && blur ? (process.platform === 'win32' ? 'on' : 'unsupported') : 'off'))
    assert.deepEqual(value.size, initial.size, 'appearance updates and reload do not resize the fixed window')
    assert.deepEqual(value.backgrounds, ['rgba(0, 0, 0, 0)', 'rgba(0, 0, 0, 0)'], 'html and body remain transparent in every state')
    assert.deepEqual(value.padding, ['24px', '24px', '24px', '24px'])
    const channels = value.background.match(/[\d.]+/g)?.map(Number)
    assert.deepEqual(channels?.slice(0, 3), [18, 27, 45], 'only the panel background alpha changes')
    assert.equal(channels?.[3] ?? 1, alpha)
    assert.deepEqual(value.disabled, [!transparent, !transparent])
    assert.equal(value.blurVariable, transparent && blur ? 'blur(16px)' : 'none')
    assert.equal(value.backdrop, transparent && blur && value.blurSupported ? 'blur(16px)' : 'none')
    assert.equal(value.unfaded, true, 'text and controls must not inherit element opacity or a fading filter')
    assert.deepEqual(value.text, initial.text, 'text, button labels and their colors remain unchanged')
    assert.equal(value.status, transparent && blur
      ? (process.platform === 'win32' ? '已请求 Windows 原生背景模糊' : '当前平台未启用原生模糊，仅应用 CSS 效果')
      : '透明层已就绪', 'native effect request completes without a permission or window error')
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
  if (process.platform === 'win32') {
    const error = await page.evaluate(`window.__TAURI__.window.getCurrentWindow()
      .setEffects({ effects: [window.__TAURI__.window.Effect.Blur] })
      .then(() => null, error => String(error))`)
    assert.equal(error, 'runtime error: failed to create window', 'unsupported native effects reject through the runtime error contract instead of silently reporting success')
    await page.evaluate(`(async () => {
      const { getCurrentWindow, Effect } = window.__TAURI__.window
      await getCurrentWindow().setEffects({ effects: [Effect.Acrylic] })
      await getCurrentWindow().clearEffects()
    })()`)
    test.pass('standard Tauri Acrylic apply/clear completes; unsupported Blur rejects through the runtime error contract')
  }
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
  if (process.platform === 'win32') await test.nativeShot(nativeWindow(await nativeWindows(test, ['Transparent Window']), 'Transparent Window'), '04-native-blur-off')
  await click(page, '#blur')
  await appearance({ slider: '50', blur: true })
  await test.shot(page, '05-blur-enabled')
  if (process.platform === 'win32') await test.nativeShot(nativeWindow(await nativeWindows(test, ['Transparent Window']), 'Transparent Window'), '05-native-blur-on')
  await click(page, '#transparency')
  await appearance({ slider: '50', transparent: false, blur: true })
  await test.shot(page, '06-transparency-disabled')
  await click(page, '#transparency')
  await appearance({ slider: '50', blur: true })
  await test.shot(page, '07-slider-and-blur-restored')
  await click(page, '#blur')
  await appearance({ slider: '50' })
  await test.shot(page, '08-blur-reset')
  // Reload with both controls nondefault, so resetting is not a vacuous check.
  await click(page, '#blur')
  await click(page, '#transparency')
  await appearance({ slider: '50', transparent: false, blur: true })
  await page.call('Page.reload')
  await until(() => page.evaluate('document.readyState === "complete" && Boolean(window.__TAURI__?.window)'), Boolean)
  await appearance()
  await test.shot(page, '09-defaults-after-reload')
  test.pass('original transparent page: minimum/middle/maximum alpha and output, disable/restore, unfaded controls, native effect request toggle/reset and reload defaults')

  let window
  if (process.platform === 'win32') {
    window = nativeWindow(await nativeWindows(test, ['Transparent Window']), 'Transparent Window')
    assert.ok(window.visible && !window.minimized && !window.maximized)
    // Native client pixels are rounded independently of the CSS viewport at fractional DPI.
    for (const [index, axis] of ['width', 'height'].entries()) {
      assert.ok(Math.abs(window.clientRect[axis] - initial.size[index] * initial.scale) <= initial.scale, `${axis}: native client size matches the renderer within one CSS pixel`)
    }
    const nativeShot = async (name) => {
      try { await test.nativeShot(window, name) }
      catch (error) {
        if (!error.blocked) throw error
        test.limitations.push(`Native transparent-window keyframe unavailable: ${error.message}`)
      }
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
