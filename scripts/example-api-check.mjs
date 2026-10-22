// Exercises the official Svelte views. Missing OS/device evidence stays blocked;
// a screenshot or a resolved setter alone never certifies a native capability.
import nodeAssert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { existsSync, readFileSync, writeFileSync } from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const labelExpression = 'window.__TAURI_INTERNALS__?.metadata?.currentWindow?.label'

function physicalBounds(value, scale) {
  const dip = Math.round(value / scale)
  // Electron's own bounds checks allow one DIP at fractional scales (and odd scales > 2).
  const slack = Math.round(scale) !== scale || (scale > 2 && scale % 2 === 1) ? 1 : 0
  return { dip, expected: Math.round(dip * scale), min: Math.round((dip - slack) * scale), max: Math.round((dip + slack) * scale) }
}

export default async function apiChecks(test, { assert = nodeAssert, click, fill, until, native, nativeWindow }) {
  const page = await test.page('main')
  test.coverage ??= []
  test.limitations ??= []
  const coverage = test.coverage
  const limitations = test.limitations
  const save = () => writeFileSync(path.join(test.directory, 'api-functional-coverage.json'), JSON.stringify({ coverage, limitations, visualReview: 'pending' }, null, 2) + '\n')
  const limit = (view, capability, reason) => { limitations.push({ view, capability, status: 'blocked', reason }); save() }
  const blocked = (message) => { throw Object.assign(new Error(message), { blocked: true }) }
  const section = async (name, action) => {
    try {
      const evidence = await action()
      coverage.push({ name, status: evidence?.partial ? 'partial' : 'passed', evidence })
      if (!evidence?.partial) test.pass(`API ${name}`)
    } catch (error) {
      coverage.push({ name, status: error.blocked ? 'blocked' : 'failed', reason: error.message, ...(error.evidence ? { evidence: error.evidence } : {}) })
      console.log(`  ${error.blocked ? 'BLOCKED' : 'FAIL'} API ${name}: ${error.message}`)
    } finally { save() }
  }

  // Resolve original labels without adding IDs or replacing event handlers.
  // Communication repeats id=request, and Window's Height is a div, not a label.
  const locate = async (text, kind = 'button', scope = 'main') => page.evaluate(`(() => {
    const text = ${JSON.stringify(text)}, kind = ${JSON.stringify(kind)}, scope = ${JSON.stringify(scope)}
    const clean = text => text.replace(/\\s+/g, ' ').trim(), field = kind.startsWith('field:')
    const matches = [...document.querySelectorAll(field ? scope + ' label, ' + scope + ' h4, ' + scope + ' div' : scope + ' ' + kind)]
      .filter(el => clean(field ? [...el.childNodes].filter(n => n.nodeType === 3).map(n => n.textContent).join(' ') : el.textContent) === text)
      .map(el => field ? (el.tagName === 'H4' ? el.parentElement : el).querySelector(kind.slice(6)) : el).filter(Boolean)
    if (matches.length !== 1) throw new Error('Expected one original control: ' + text + '; found ' + matches.length)
    let el = matches[0], parts = []
    while (el && el !== document.body) {
      const siblings = [...el.parentElement.children].filter(n => n.tagName === el.tagName)
      parts.unshift(el.tagName.toLowerCase() + ':nth-of-type(' + (siblings.indexOf(el) + 1) + ')')
      el = el.parentElement
    }
    return 'body > ' + parts.join(' > ')
  })()`)
  const button = async (name) => click(page, await locate(name))
  const view = async (name) => {
    await click(page, await locate(name, 'a', '#sidebar'))
    await until(() => page.evaluate('document.querySelector("main h1")?.textContent'), (text) => text === name)
  }
  const messages = () => page.evaluate(`Array.from(document.querySelectorAll('#console pre'), el => {
    const timestamp = el.querySelector('strong')?.textContent ?? ''
    return el.textContent.slice(timestamp.length).trim()
  })`)
  const clear = async () => {
    await click(page, '#console [role="button"]:has(.i-codicon-clear-all)')
    await until(messages, (values) => values.length === 0)
  }
  const key = async (key, code, windowsVirtualKeyCode, modifiers = 0) => {
    await page.call('Input.dispatchKeyEvent', { type: 'keyDown', key, code, windowsVirtualKeyCode, modifiers })
    await page.call('Input.dispatchKeyEvent', { type: 'keyUp', key, code, windowsVirtualKeyCode, modifiers })
  }
  const editNumber = async (label, value, beforeCommit) => {
    await click(page, await locate(label, 'field:input'))
    await key('a', 'KeyA', 65, process.platform === 'darwin' ? 4 : 2)
    await page.call('Input.insertText', { text: String(value) })
    const requested = await beforeCommit?.()
    await key('Tab', 'Tab', 9) // commit the original bind:value/onchange handler
    return requested
  }
  const windowRead = (method) => page.evaluate(`window.__TAURI__.window.getCurrentWindow().${method}()`)
  const checkbox = async (label, getter, checked) => {
    const selector = await locate(label, 'field:input')
    if (await page.evaluate(`document.querySelector(${JSON.stringify(selector)}).checked`) !== checked) await click(page, selector)
    assert.equal(await page.evaluate(`document.querySelector(${JSON.stringify(selector)}).checked`), checked, `${label}: original checkbox must reflect the click`)
    await until(() => windowRead(getter), (value) => value === checked)
  }
  const originalTitle = await windowRead('title')
  const icon = path.join(root, 'source/tauri/examples/.icons/tray_icon.png')
  const flattenMenu = (items) => items.flatMap((item) => [item, ...flattenMenu(item.children)])
  const nativeMenu = async (window, commandId) => {
    if (process.platform !== 'win32' || !native) blocked('Windows native menu observation is unavailable')
    const current = (await native(test, 'Rect', { WindowHandle: window.hwnd })).window
    const hwnd = String(current.hwnd), owner = current.pid ?? current.processId
    assert.match(hwnd, /^\d+$/)
    assert.ok(Number.isInteger(owner) && owner > 0)
    if (commandId !== undefined) assert.ok(Number.isInteger(commandId) && commandId > 0 && commandId < 0xFFFFFFFF)
    // Only a scoped, validated HWND and an ID just read from its real menu enter
    // this Win32 command path. WM_COMMAND certifies routing, not user selection.
    const source = `$ErrorActionPreference = 'Stop'
Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
public static class ApiMenuProbe {
  public class Item { public string text; public uint id; public bool enabled, check; public Item[] children; }
  [DllImport("user32.dll")] static extern IntPtr GetMenu(IntPtr window);
  [DllImport("user32.dll")] static extern IntPtr GetSubMenu(IntPtr menu, int position);
  [DllImport("user32.dll")] static extern int GetMenuItemCount(IntPtr menu);
  [DllImport("user32.dll")] static extern uint GetMenuItemID(IntPtr menu, int position);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern int GetMenuString(IntPtr menu, uint item, StringBuilder text, int size, uint flags);
  [DllImport("user32.dll")] static extern uint GetMenuState(IntPtr menu, uint item, uint flags);
  [DllImport("user32.dll")] static extern bool IsWindow(IntPtr window);
  [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr window, out uint pid);
  [DllImport("user32.dll", SetLastError=true)] static extern IntPtr SendMessageTimeoutW(IntPtr window, uint message, UIntPtr wparam, IntPtr lparam, uint flags, uint timeout, out UIntPtr result);
  static Item[] Read(IntPtr menu) {
    var result = new List<Item>();
    for (int i = 0; i < GetMenuItemCount(menu); i++) {
      var text = new StringBuilder(2048); GetMenuString(menu, (uint)i, text, text.Capacity, 0x400);
      uint state = GetMenuState(menu, (uint)i, 0x400);
      result.Add(new Item { text = text.ToString(), id = GetMenuItemID(menu, i), enabled = (state & 3) == 0, check = (state & 8) != 0, children = Read(GetSubMenu(menu, i)) });
    }
    return result.ToArray();
  }
  public static Item[] Probe(long handle, uint owner, uint command) {
    IntPtr window = new IntPtr(handle); uint actual; GetWindowThreadProcessId(window, out actual);
    if (!IsWindow(window) || actual != owner) throw new Exception("Native menu window owner changed");
    if (command != 0) { UIntPtr result; if (SendMessageTimeoutW(window, 0x111, new UIntPtr(command), IntPtr.Zero, 2, 1000, out result) == IntPtr.Zero) throw new Exception("Native WM_COMMAND timed out or was refused"); }
    return Read(GetMenu(window));
  }
}
'@
@{ items = @([ApiMenuProbe]::Probe([long]${hwnd}, [uint32]${owner}, [uint32]${commandId ?? 0})) } | ConvertTo-Json -Depth 20 -Compress`
    return new Promise((resolve, reject) => {
      const process = spawn('pwsh', ['-NoProfile', '-NonInteractive', '-EncodedCommand', Buffer.from(source, 'utf16le').toString('base64')], { cwd: root, windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'] })
      let stdout = '', stderr = ''
      const timer = setTimeout(() => { process.kill(); reject(new Error('Native menu probe timed out')) }, 15_000)
      process.stdout.on('data', (data) => { stdout += data })
      process.stderr.on('data', (data) => { stderr += data })
      process.on('error', (error) => { clearTimeout(timer); reject(error) })
      process.on('close', (code) => {
        clearTimeout(timer)
        try { if (code !== 0) throw new Error(stderr.trim() || `native probe exited ${code}`); resolve(JSON.parse(stdout.trim().replace(/^\uFEFF/, ''))) }
        catch (error) { reject(error) }
      })
    })
  }

  await section('Welcome metadata and original isolation frame', async () => {
    await view('Welcome')
    const config = JSON.parse(readFileSync(path.join(root, 'source/tauri/examples/api/src-tauri/tauri.conf.json'), 'utf8'))
    const tauriVersion = /^version = "([^"]+)"/m.exec(readFileSync(path.join(root, 'source/tauri/crates/tauri/Cargo.toml'), 'utf8'))[1]
    const actual = await page.evaluate(`Promise.all([window.__TAURI__.app.getName(), window.__TAURI__.app.getVersion(), window.__TAURI__.app.getTauriVersion(), window.__TAURI__.app.getBundleType()])`)
    assert.deepEqual(actual.slice(0, 3), [config.productName, config.version, tauriVersion])
    const expected = [actual[0], actual[1], actual[2], actual[3] ?? 'Unknown']
    const displayed = await until(() => page.evaluate('Array.from(document.querySelectorAll("main pre code"), el => el.textContent)'), (values) => JSON.stringify(values) === JSON.stringify(expected))
    const isolation = await page.evaluate(`(() => {
      const frame = document.querySelector('iframe#__tauri_isolation__')
      return { pattern: window.__TAURI_INTERNALS__.__TAURI_PATTERN__, hook: typeof window.__TAURI_ISOLATION_HOOK__,
        frame: frame && { src: frame.src, sandbox: frame.getAttribute('sandbox'), display: getComputedStyle(frame).display } }
    })()`)
    assert.equal(isolation.pattern.pattern, 'isolation')
    assert.equal(isolation.pattern.side, 'original')
    assert.equal(isolation.hook, 'undefined')
    assert.ok(isolation.frame && /isolation-/.test(isolation.frame.src))
    assert.equal(isolation.frame.sandbox, 'allow-scripts')
    assert.equal(isolation.frame.display, 'none')
    await test.shot(page, 'api-01-welcome-metadata')
    return { actual, displayed, isolation }
  })

  await section('App theme cycles through original sidebar controls', async () => {
    const initial = await page.evaluate('localStorage.getItem("theme") || "auto"')
    let theme = initial
    const states = []
    try {
      for (let i = 0; i < 3; i++) {
        theme = { auto: 'dark', dark: 'light', light: 'auto' }[theme]
        await click(page, '#sidebar > a[href="##"]')
        const state = await until(() => page.evaluate(`({ selected: localStorage.getItem('theme'), dark: document.documentElement.classList.contains('dark'), systemDark: matchMedia('(prefers-color-scheme: dark)').matches })`), (state) => state.selected === theme && state.dark === (theme === 'auto' ? state.systemDark : theme === 'dark'))
        if (theme !== 'auto') await until(() => windowRead('theme'), (actual) => actual === theme)
        states.push(state)
        await test.shot(page, `api-02-theme-${theme}`)
      }
      assert.equal(theme, initial)
      return states
    } finally {
      // Restore via the same original control if an intermediate assertion failed.
      for (let i = 0; i < 3 && await page.evaluate('localStorage.getItem("theme") || "auto"') !== initial; i++) await click(page, '#sidebar > a[href="##"]')
    }
  })

  await section('Communication Log original event and negative scope', async () => {
    await view('Communication')
    const offset = readFileSync(test.log, 'utf8').length
    await button('Call Log API')
    await until(() => readFileSync(test.log, 'utf8').slice(offset), (text) => text.includes('tauri-click') && text.includes('this payload is optional because we used Option in Rust'))
    const rejected = await page.evaluate(`window.__TAURI__.core.invoke('log_operation', { event: 'not-in-the-capability', payload: 'negative scope check' }).then(() => ({ resolved: true }), error => ({ error }))`)
    assert.deepEqual(rejected, { error: 'not allowed' })
    await test.shot(page, 'api-03-log-control')
    return { originalEvent: 'tauri-click', rejected }
  })
  await section('Communication Request exact Rust JSON', async () => {
    await view('Communication'); await clear(); await button('Call Request (async) API')
    const values = await until(messages, (values) => values.length === 1)
    assert.deepEqual(JSON.parse(values[0]), { message: 'message response' })
    await test.shot(page, 'api-04-request-response')
    return values
  })
  await section('Communication original Echo JSON and supplemental raw bytes', async () => {
    await view('Communication'); await clear(); await button('Echo')
    const values = (await until(messages, (values) => values.length >= 2)).map(JSON.parse)
    assert.equal(values.length, 2)
    assert.deepEqual(values.find(Array.isArray), [1, 2, 3])
    assert.deepEqual(values.find((value) => !Array.isArray(value)), { message: 'Tauri JSON request!' })
    const bytes = [0, 1, 127, 128, 255]
    const raw = await page.evaluate(`window.__TAURI__.core.invoke('echo', new Uint8Array(${JSON.stringify(bytes)})).then(value => {
      if (!(value instanceof ArrayBuffer)) throw new Error('raw echo did not return an ArrayBuffer')
      return Array.from(new Uint8Array(value))
    })`)
    assert.deepEqual(raw, bytes)
    await test.shot(page, 'api-05-echo-json-and-array')
    return { originalUi: values, supplementalRaw: raw }
  })
  await section('Communication original JS event gets one exact Rust reply', async () => {
    await view('Communication'); await clear()
    const offset = readFileSync(test.log, 'utf8').length
    await button('Send event to Rust')
    const values = await until(messages, (values) => values.length > 0)
    assert.equal(values.length, 1)
    const event = JSON.parse(values[0])
    assert.equal(event.event, 'rust-event')
    assert.deepEqual(event.payload, { data: 'something else' })
    assert.ok(Number.isInteger(event.id))
    await until(() => readFileSync(test.log, 'utf8').slice(offset), (text) => text.includes('got js-event with message') && text.includes('this is the payload string'))
    await test.shot(page, 'api-06-rust-event-reply')
    return event
  })
  await section('Communication Spam complete ordered Channel sequence', async () => {
    await view('Communication'); await clear(); await button('Spam')
    const values = (await until(messages, (values) => values.length >= 1000, 60_000)).map(Number)
    assert.deepEqual(values, Array.from({ length: 1000 }, (_, i) => i + 1))
    await test.shot(page, 'api-07-channel-last-values')
    return { count: values.length, first: values[0], last: values.at(-1), values }
  })

  await section('App original Show continuation (platform limitation recorded)', async () => {
    await view('App'); await clear(); await button('Show')
    const values = await until(messages, (values) => values.includes('Shown app'), 7000)
    assert.deepEqual(values, ['Hide app', 'Shown app'])
    await test.shot(page, 'api-08-app-show-continuation')
    return { partial: true, values, osApplicationVisibilityCertified: false }
  })
  limit('App', 'Native application Show/Hide and Dock visibility', process.platform === 'darwin'
    ? 'UI continuation is exercised; this helper has no macOS application/Dock observer. Native visibility is unverified.'
    : 'Tauri app_show/app_hide/set_dock_visibility are successful no-ops outside macOS. The log continuation is not native functionality and is not counted passed.')

  await section('Window title and physical geometry through original forms', async () => {
    await view('Window')
    const original = await page.evaluate(`(async () => { const w = window.__TAURI__.window.getCurrentWindow(); return { title: await w.title(), size: await w.innerSize(), position: await w.outerPosition(), scale: await w.scaleFactor() } })()`)
    const evidence = { original, sizeSteps: [] }
    let stage = 'title'
    try {
      const title = `API original title form ${Date.now()}`
      await fill(page, await locate('Set Window Title', 'field:input'), title)
      await button('Set')
      await until(() => windowRead('title'), (actual) => actual === title)
      const width = Math.round(Math.max(650 * original.scale, original.size.width - 80))
      const height = Math.round(Math.max(450 * original.scale, original.size.height - 50))
      const fields = { width: await locate('Width', 'field:input'), height: await locate('Height', 'field:input') }
      const readSizeFields = () => page.evaluate(`Object.fromEntries(Object.entries(${JSON.stringify(fields)}).map(([axis, selector]) => [axis, document.querySelector(selector).valueAsNumber]))`)
      const matchesSize = (actual, expected) => ['width', 'height'].every(axis => actual[axis] >= expected[axis].min && actual[axis] <= expected[axis].max)
      let expected
      for (const [label, value] of [['Width', width], ['Height', height]]) {
        stage = `commit ${label}`
        const requested = await editNumber(label, value, readSizeFields)
        assert.equal(requested[label.toLowerCase()], value, 'original number field must contain the requested value')
        expected = Object.fromEntries(Object.entries(requested).map(([axis, value]) => [axis, physicalBounds(value, original.scale)]))
        const step = { label, requested, expected, scale: original.scale }
        evidence.sizeSteps.push(step)
        stage = `read ${label} size`
        await until(async () => { step.actual = await windowRead('innerSize'); return step.actual }, (actual) => matchesSize(actual, expected))
        // Resize events refill both bound fields; wait before editing the next axis.
        await until(readSizeFields, (values) => values.width === step.actual.width && values.height === step.actual.height)
      }
      const x = Math.max(0, original.position.x + 24), y = Math.max(0, original.position.y + 24)
      stage = 'position X'
      await editNumber('X', x)
      await until(() => windowRead('outerPosition'), (actual) => Math.abs(actual.x - x) <= 1)
      stage = 'position Y'
      await editNumber('Y', y)
      await until(() => windowRead('outerPosition'), (actual) => Math.abs(actual.x - x) <= 1 && Math.abs(actual.y - y) <= 1)
      evidence.actual = await windowRead('innerSize')
      evidence.scale = await windowRead('scaleFactor')
      assert.equal(evidence.scale, original.scale, 'geometry moved to a different display scale')
      assert.ok(matchesSize(evidence.actual, expected), 'position change must preserve requested content size within the DPI bounds')
      await test.shot(page, 'api-09-window-title-and-geometry')
      let os
      if (process.platform === 'win32' && native && nativeWindow) {
        stage = 'native client rect'
        const target = nativeWindow(await native(test, 'List'), title)
        evidence.nativeClientRect = target.clientRect
        assert.equal(target.visible, true)
        assert.ok(matchesSize(target.clientRect, expected), 'native physical client size must match the last original form request within the DPI bounds')
        if (test.nativeShot) { await test.nativeShot(target, 'api-native-title-and-frame'); os = target }
        else os = await native(test, 'Capture', { WindowHandle: target.hwnd, OutputPath: path.join(test.directory, `api-native-title-and-frame-${Date.now()}.png`) })
      } else limit('Window', 'Native title/frame evidence', 'No native window observer on this platform.')
      return { ...evidence, title, width, height, x, y, os }
    } catch (error) {
      throw Object.assign(error, { message: `${stage}: ${error.message}`, evidence })
    } finally {
      await page.evaluate(`(async () => { const w = window.__TAURI__.window.getCurrentWindow(), original = ${JSON.stringify(original)};
        await w.setTitle(original.title); await w.setSize(new window.__TAURI__.window.PhysicalSize(original.size.width, original.size.height));
        await w.setPosition(new window.__TAURI__.window.PhysicalPosition(original.position.x, original.position.y));
      })()`)
    }
  })
  for (const [label, getter] of [['Resizable', 'isResizable'], ['Maximizable', 'isMaximizable'], ['Minimizable', 'isMinimizable'], ['Closable', 'isClosable'], ['Has decorations', 'isDecorated'], ['Always on top', 'isAlwaysOnTop']]) {
    await section(`Window ${label} original checkbox and backend getter`, async () => {
      await view('Window')
      const original = await windowRead(getter)
      try {
        await checkbox(label, getter, false)
        await test.shot(page, `api-window-${getter}-off`)
        await checkbox(label, getter, true)
        return { off: false, on: true }
      } finally { await checkbox(label, getter, original) }
    })
  }
  for (const [label, getter] of [['Maximized', 'isMaximized'], ['Fullscreen', 'isFullscreen']]) {
    await section(`Window ${label} true/false real state`, async () => {
      await view('Window')
      const original = await windowRead(getter)
      try {
        await checkbox(label, getter, true)
        await test.shot(page, `api-window-${getter}-on`)
        await checkbox(label, getter, false)
        return { on: true, off: false }
      } finally { await checkbox(label, getter, original) }
    })
  }
  for (const [name, getter, initial, transient, expected] of [
    ['Hide', 'isVisible', true, false, ['window is visible', 'window is invisible', 'window is visible']],
    ['Disable', 'isEnabled', true, false, ['window is enabled', 'window is disabled', 'window is enabled']],
    ['Minimize', 'isMinimized', false, true, undefined],
  ]) {
    await section(`Window original ${name} timer restores real state`, async () => {
      await view('Window'); await clear()
      assert.equal(await windowRead(getter), initial)
      try {
        await button(name)
        await until(() => windowRead(getter), (value) => value === transient, 1500)
        await until(() => windowRead(getter), (value) => value === initial, 7000)
        if (expected) await until(messages, (values) => JSON.stringify(values) === JSON.stringify(expected))
        await test.shot(page, `api-window-${name.toLowerCase()}-restored`)
        return { observed: [initial, transient, initial], messages: await messages() }
      } finally {
        // Recovery does not alter a failed verdict for the original timer path.
        await page.evaluate(`(async () => { const w = window.__TAURI__.window.getCurrentWindow(); await w.setEnabled(true); await w.show(); await w.unminimize() })()`)
      }
    })
  }
  await section('Window UI creates a separately addressable child and closes it normally', async () => {
    await view('Window'); await clear()
    const suffix = `functional-${Date.now()}`, label = `main-${suffix}`
    let child
    try {
      await fill(page, 'main input[placeholder="New window label.."]', suffix)
      await button('Create')
      child = await test.page(label)
      await until(messages, (values) => values.includes('webview created'))
      assert.equal(await child.evaluate(labelExpression), label)
      await until(() => child.evaluate('document.querySelector("main h1")?.textContent'), (text) => text === 'Welcome')
      await test.shot(child, 'api-14-ui-created-child')
      const childId = child.target.id
      try { await child.evaluate('window.__TAURI__.window.getCurrentWindow().close()') } // allowed for API main-* capability
      catch (error) { if (!/CDP target closed/.test(error.message)) throw error }
      // A normal close may destroy the renderer before its invoke can resolve.
      // The child disappearing and the original main surviving are the verdict.
      await until(test.pages, (pages) => !pages.some((target) => target.id === childId))
      child = undefined
      assert.equal(await windowRead('title'), originalTitle)
      return { label, mainSurvived: true }
    } finally {
      if (child) { try { await child.evaluate('window.__TAURI__.window.getCurrentWindow().close()') } catch {} }
    }
  })

  const dragMenuItem = async (kind) => {
    if (process.platform !== 'win32' || !native || !nativeWindow) blocked('Original Sortable drag requires the scoped Windows native input helper on this host')
    const sourceSelector = `main .menu-item[data-kind="${kind}"]:not([data-id])`
    const targetSelector = 'main .max-w-5xl > div:last-child'
    let stage = 'locate native owner and original source/drop zone'
    try {
      await page.call('Page.bringToFront')
      const window = nativeWindow(await native(test, 'List'), await windowRead('title'))
      const points = await page.evaluate(`(() => {
        const source = document.querySelector(${JSON.stringify(sourceSelector)}), target = document.querySelector(${JSON.stringify(targetSelector)})
        if (!source || !target) throw new Error('missing original menu item/drop zone')
        const visiblePoint = (el, drop) => {
          const r = el.getBoundingClientRect(), clip = { left: Math.max(0, r.left), top: Math.max(0, r.top), right: Math.min(innerWidth, r.right), bottom: Math.min(innerHeight, r.bottom) }
          for (let p = el.parentElement; p; p = p.parentElement) {
            const style = getComputedStyle(p), b = p.getBoundingClientRect()
            if (/(auto|scroll|hidden|clip)/.test(style.overflowY)) { clip.top = Math.max(clip.top, b.top); clip.bottom = Math.min(clip.bottom, b.bottom) }
            if (/(auto|scroll|hidden|clip)/.test(style.overflowX)) { clip.left = Math.max(clip.left, b.left); clip.right = Math.min(clip.right, b.right) }
          }
          if (clip.right - clip.left < 8 || clip.bottom - clip.top < 8) return null
          const point = { x: drop ? (clip.left + clip.right) / 2 : clip.left + 4, y: drop ? clip.bottom - 4 : (clip.top + clip.bottom) / 2 }
          const hit = document.elementFromPoint(point.x, point.y)
          return (drop ? el.contains(hit) : hit === el) ? point : null
        }
        source.scrollIntoView({ block: 'start' })
        if (!visiblePoint(target, true)) target.scrollIntoView({ block: 'end' })
        const start = visiblePoint(source, false), end = visiblePoint(target, true)
        if (!start || !end) throw new Error('source and drop zone must both be visible; do not drag to a stale or covered coordinate')
        return { start, end, width: innerWidth, height: innerHeight }
      })()`)
      await test.shot(page, `api-menu-${kind.toLowerCase()}-before-native-drag`)
      const current = (await native(test, 'Rect', { WindowHandle: window.hwnd })).window.clientRect
      const physical = (point) => ({ x: Math.round(current.left + point.x * current.width / points.width), y: Math.round(current.top + point.y * current.height / points.height) })
      const start = physical(points.start), end = physical(points.end)
      stage = 'real OS press/move/release on original Sortable source'
      await native(test, 'Drag', { WindowHandle: window.hwnd, StartX: start.x, StartY: start.y, EndX: end.x, EndY: end.y, Expected: 'Any', DurationMs: 1200, Steps: 30, Activate: true })
      // Expected Any only reports input delivery. The original onAdd row is the
      // required functional assertion; never insert a row or call create by hand.
      stage = 'original Sortable onAdd creates one row'
      await until(() => page.evaluate(`document.querySelectorAll('main .menu-item[data-id][data-kind="${kind}"]').length`), (count) => count === 1)
      await page.evaluate(`document.querySelector('main .menu-item[data-id][data-kind="${kind}"]').scrollIntoView({ block: 'center' })`)
      await test.shot(page, `api-menu-${kind.toLowerCase()}-after-native-drag`)
    } catch (error) {
      throw Object.assign(error, { message: `Menu ${kind} native drag, ${stage}: ${error.message}`, evidence: { kind, stage } })
    }
  }
  await section('Menu original native-pointer builder drag, real Win32 items and native command callback', async () => {
    await view('Menu')
    let stage = 'Normal original Sortable insertion by native pointer'
    try {
      assert.equal(await page.evaluate('document.querySelectorAll("main .menu-item[data-id]").length'), 0)
      await dragMenuItem('Normal')
      const text = `Original menu action ${Date.now()}`
      stage = 'Normal text field'
      await fill(page, 'main .menu-item[data-id][data-kind="Normal"] input[title="text"]', text)
      await key('Tab', 'Tab', 9)
      stage = 'Check original Sortable insertion by native pointer'
      await dragMenuItem('Check')
      await click(page, 'main .menu-item[data-id][data-kind="Check"] input[title="checked"]')
      assert.equal(await page.evaluate('document.querySelector("main .menu-item[data-id][data-kind=Check] input[title=checked]").checked'), true)
      assert.equal(await page.evaluate('document.querySelector("main .menu-item[data-id][data-kind=Normal]").dataset.text'), text)
      await test.shot(page, 'api-15-menu-builder')
      stage = 'create menu and enumerate native items'
      await button('Create menu')
      if (process.platform !== 'win32' || !native || !nativeWindow) blocked('Menu builder ran; Win32 native item/query/callback verification is unavailable on this host')
      const target = nativeWindow(await native(test, 'List'), await windowRead('title'))
      const tree = await until(() => nativeMenu(target), (tree) => flattenMenu(tree.items).some((item) => item.text === text))
      const actual = flattenMenu(tree.items)
      const normal = actual.find((item) => item.text === text)
      const check = actual.find((item) => item.text === 'A check menu')
      assert.ok(normal && normal.enabled && Number.isInteger(normal.id) && normal.id > 0 && normal.id < 0xFFFFFFFF)
      assert.ok(check && check.check, 'original checked control must update the real Win32 menu state')
      if (test.nativeShot) await test.nativeShot(target, 'api-native-created-menu')
      stage = 'native Normal command callback'
      await clear()
      await nativeMenu(target, normal.id)
      assert.deepEqual(await until(messages, (values) => values.length > 0), [`Item ${text} clicked`])
      await test.shot(page, 'api-menu-native-command-callback')
      // Edit/remove the live original items, then verify the native menu changed.
      const renamed = `${text} edited`
      stage = 'live Normal text change'
      await fill(page, 'main .menu-item[data-id][data-kind="Normal"] input[title="text"]', renamed)
      await key('Tab', 'Tab', 9)
      await until(() => nativeMenu(target), (tree) => flattenMenu(tree.items).some((item) => item.text === renamed))
      stage = 'live Check checked change'
      await click(page, 'main .menu-item[data-id][data-kind="Check"] input[title="checked"]')
      await until(() => nativeMenu(target), (tree) => flattenMenu(tree.items).find((item) => item.text === 'A check menu')?.check === false)
      stage = 'live Check remove'
      await click(page, 'main .menu-item[data-id][data-kind="Check"] button[title="remove"]')
      const removed = await until(() => nativeMenu(target), (tree) => !flattenMenu(tree.items).some((item) => item.text === 'A check menu'))
      await test.shot(page, 'api-menu-live-edit-and-remove')
      return { originalUiRows: 2, text, tree, nativeRoute: 'Win32 WM_COMMAND using ID enumerated from this HWND; actual popup selection not certified', removed }
    } catch (error) {
      throw Object.assign(error, { message: `Menu ${stage}: ${error.message}`, evidence: error.evidence ?? { stage } })
    } finally { await view('Welcome') } // original onDestroy closes menu/items
  })
  limit('Welcome/Menu', 'Actual popup selection and OS visuals', 'The original builder, real Win32 item/state changes and original Console callback through WM_COMMAND are checked. That command route does not certify a user opening/selecting Context menu/Popup. Actual native keyboard/mouse selection, NativeIcon/Image variants, live reorder and Ctrl+B visibility still need OS input and keyframes; no JS callback is fabricated.')

  await section('Tray original Create/Remove path (OS appearance still blocked)', async () => {
    await view('Tray')
    try {
      assert.ok(existsSync(icon), 'local icon fixture exists')
      await fill(page, 'main input[placeholder="Title"]', 'API functional tray')
      await fill(page, 'main input[placeholder="Tooltip"]', 'API functional tray tooltip')
      await fill(page, 'main input[placeholder="Icon path"]', icon)
      await clear(); await button('Create tray')
      await until(() => page.evaluate('Array.from(document.querySelectorAll("main button"), el => el.textContent.trim())'), (buttons) => buttons.includes('Remove tray'))
      assert.deepEqual(await messages(), [], 'original create catch must not have rendered an error')
      await test.shot(page, 'api-16-tray-created-control')
      await button('Remove tray')
      await until(() => page.evaluate('Array.from(document.querySelectorAll("main button"), el => el.textContent.trim())'), (buttons) => buttons.includes('Create tray') && !buttons.includes('Remove tray'))
      await test.shot(page, 'api-17-tray-remove-control')
      return { partial: true, createPromiseResolved: true, removeInvoked: true, icon, osIconCertified: false }
    } finally { await view('Welcome') } // original onDestroy closes menu/tray
  })
  limit('Tray', 'OS notification area and original Rust tray', 'Need OS observer/input for icon visibility, tooltip, real left/right-click events, menu callbacks, title/template/icon variants and native disappearance after Remove. Rust tray-1 Toggle/New window/Switch Menu/Quit are separate actions. A Remove button is not OS proof.')

  await section('WebRTC actual device video and original unmount cleanup', async () => {
    await clear(); await view('WebRTC')
    let streamObject
    try {
      let observed
      try {
        observed = await until(async () => ({ values: await messages(), video: await page.evaluate(`(() => { const v = document.querySelector('#localVideo'); return {
          supported: Boolean(navigator.mediaDevices?.getUserMedia), attached: Boolean(v?.srcObject),
          width: v?.videoWidth, height: v?.videoHeight, readyState: v?.readyState, time: v?.currentTime,
          tracks: v?.srcObject?.getTracks().map(t => ({ kind: t.kind, state: t.readyState, enabled: t.enabled })) ?? []
        } })()`) }), (state) => !state.video.supported || state.video.attached || state.values.some((value) => value.includes('getUserMedia error:')), 12_000)
      } catch (error) { blocked(`Original camera/mic permission request did not resolve: ${error.message}`) }
      if (!observed.video.attached) blocked(`Original getUserMedia has no real camera/mic stream: ${observed.values.join('; ') || 'media API unavailable'}`)
      const { result } = await page.call('Runtime.evaluate', { expression: 'document.querySelector("#localVideo").srcObject', returnByValue: false })
      streamObject = result.objectId
      assert.ok(streamObject, 'retain the actual stream via CDP, without page globals or fake media')
      assert.ok(observed.video.tracks.some((track) => track.kind === 'video' && track.state === 'live'))
      assert.ok(observed.video.tracks.some((track) => track.kind === 'audio' && track.state === 'live'))
      const playing = await until(() => page.evaluate(`(() => { const v = document.querySelector('#localVideo'); return { width: v.videoWidth, height: v.videoHeight, time: v.currentTime, readyState: v.readyState } })()`), (state) => state.width > 0 && state.height > 0 && state.readyState >= 2 && state.time > observed.video.time + 0.1, 7000)
      // The original view previews locally; retain only numeric/track metadata,
      // never a camera frame, recording, audio sample or uploaded media.
      await view('Welcome')
      await test.shot(page, 'api-18-webrtc-metadata-after-leaving-preview')
      const stopped = await page.call('Runtime.callFunctionOn', { objectId: streamObject, functionDeclaration: 'function() { return this.getTracks().map(t => ({ kind: t.kind, state: t.readyState })) }', returnByValue: true })
      assert.ok(stopped.result.value.every((track) => track.state === 'ended'), 'original WebRTC onDestroy left live tracks; source never assigns mediaStream')
      return { observed, playing, stopped: stopped.result.value }
    } finally {
      // Recovery follows the original teardown verdict; it cannot make it pass.
      if (streamObject) {
        try { await page.call('Runtime.callFunctionOn', { objectId: streamObject, functionDeclaration: 'function() { for (const t of this.getTracks()) t.stop() }', returnByValue: true }) }
        finally { await page.call('Runtime.releaseObject', { objectId: streamObject }) }
      } else {
        await page.evaluate('document.querySelector("#localVideo")?.srcObject?.getTracks().forEach(track => track.stop())')
      }
      await view('Welcome')
    }
  })
  limit('WebRTC', 'Live camera-frame visual review', 'The original local preview is exercised and track state/intrinsic dimensions/currentTime are checked, but no camera frames, microphone samples or recordings are captured or uploaded. The screenshot is taken only after leaving the preview; private live-image visual acceptance remains manual.')
  limit('Window', 'Remaining native controls', 'Not certified: cursor shapes/grab/visibility/ignore-events/physical position; icon variants; focus/attention/z-order, always-on-bottom, shadow/effects/content protection; min/max constraints under user resizing; multi-monitor/simple-fullscreen; taskbar progress visuals. A getter round trip does not prove every native effect.')
  limit('Other', 'External/OS/mobile integration', 'Offline Documentation/GitHub/Source/new-window navigation, file drag/drop, file association OS launch, mobile scenes/native sample values and CLI options are outside this desktop subset. No external links are followed.')

  await section('Welcome original Exit ends the application normally', async () => {
    await view('Welcome')
    await test.shot(page, 'api-19-before-original-exit')
    try { await button('Exit') } catch (error) {
      if (!/target closed|socket|WebSocket|Session closed/i.test(error.message)) throw error
    }
    await until(() => test.child.exitCode, (code) => code !== null, 30_000)
    assert.equal(test.child.exitCode, 0)
    return { exitCode: test.child.exitCode }
  })
  save()
  const failed = coverage.filter((item) => item.status === 'failed')
  if (failed.length) throw new Error(`API failures: ${failed.map((item) => `${item.name}: ${item.reason}`).join('; ')}; see api-functional-coverage.json`)
  if (limitations.length || coverage.some((item) => item.status === 'blocked' || item.status === 'partial')) {
    throw Object.assign(new Error('API testable subset completed; OS/manual/device capabilities remain unverified. See api-functional-coverage.json; this is not full API acceptance.'), { blocked: true })
  }
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  nodeAssert.deepEqual(physicalBounds(1420, 1.5), { dip: 947, expected: 1421, min: 1419, max: 1422 })
  nodeAssert.deepEqual(physicalBounds(1153, 1.5), { dip: 769, expected: 1154, min: 1152, max: 1155 })
  nodeAssert.deepEqual(physicalBounds(1420, 1), { dip: 1420, expected: 1420, min: 1420, max: 1420 })
  console.log('API DPI bounds self-check passed')
}
