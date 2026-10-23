#!/usr/bin/env node
// Runs the original pages, asserts their own UI/IPC, and records unmodified
// keyframes for visual review. A successful assertion is not a visual verdict.
//   pnpm check helloworld state
//   pnpm check                         # all local and official examples, sequentially
import assert from 'node:assert/strict'
import { spawn, spawnSync } from 'node:child_process'
import { closeSync, existsSync, mkdirSync, openSync, readFileSync, readdirSync, realpathSync, rmSync, writeFileSync } from 'node:fs'
import { createServer } from 'node:net'
import path from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'
import apiChecks from './example-api-check.mjs'
import transparentChecks from './example-transparent-check.mjs'

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const artifacts = path.join(root, 'target', 'example-checks')
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms))
const labelExpression = 'window.__TAURI_INTERNALS__?.metadata?.currentWindow?.label'

async function until(read, accept = Boolean, timeout = 15_000) {
  const deadline = Date.now() + timeout
  let value
  let error
  do {
    try {
      value = await read()
      if (accept(value)) return value
      error = undefined
    } catch (reason) {
      if (reason.fatal) throw reason
      error = reason
    }
    await sleep(100)
  } while (Date.now() < deadline)
  throw new Error(`condition timed out: ${error?.message ?? JSON.stringify(value)}`)
}

/** The Chromium shipped by this runtime is the only browser this test needs. */
class Session {
  next = 0
  pending = new Map()
  events = []

  static async connect(target) {
    const socket = new WebSocket(target.webSocketDebuggerUrl)
    await new Promise((resolve, reject) => {
      const timer = setTimeout(() => { socket.close(); reject(new Error('CDP connect timed out')) }, 5000)
      socket.onopen = () => { clearTimeout(timer); resolve() }
      socket.onerror = () => { clearTimeout(timer); reject(new Error(`cannot attach to ${target.url}`)) }
    })
    const session = new Session()
    session.socket = socket
    session.target = target
    socket.onmessage = ({ data }) => {
      const message = JSON.parse(data)
      if (!message.id) { session.events.push(message); return }
      const pending = session.pending.get(message.id)
      if (!pending) return
      session.pending.delete(message.id)
      clearTimeout(pending.timer)
      if (message.error) pending.reject(new Error(message.error.message))
      else pending.resolve(message.result)
    }
    socket.onclose = () => {
      for (const { timer, reject } of session.pending.values()) {
        clearTimeout(timer)
        reject(new Error('CDP target closed'))
      }
      session.pending.clear()
    }
    await session.call('Runtime.enable')
    await session.call('Page.enable')
    return session
  }

  call(method, params = {}, timeout = 20_000, sessionId) {
    return new Promise((resolve, reject) => {
      const id = ++this.next
      const timer = setTimeout(() => {
        this.pending.delete(id)
        reject(new Error(`${method} timed out`))
      }, timeout)
      this.pending.set(id, { resolve, reject, timer })
      try { this.socket.send(JSON.stringify({ id, method, params, ...(sessionId ? { sessionId } : {}) })) }
      catch (error) { clearTimeout(timer); this.pending.delete(id); reject(error) }
    })
  }

  async evaluate(expression) {
    const { result, exceptionDetails } = await this.call('Runtime.evaluate', {
      expression, awaitPromise: true, returnByValue: true,
    })
    if (exceptionDetails) throw new Error(exceptionDetails.exception?.description ?? result?.description ?? JSON.stringify(exceptionDetails))
    return result?.value
  }

  close() { this.socket.close() }
}

async function click(page, selector) {
  await page.call('Page.bringToFront')
  const point = await page.evaluate(`(() => {
    const element = document.querySelector(${JSON.stringify(selector)})
    if (!element || element.disabled) throw new Error('not actionable: ' + ${JSON.stringify(selector)})
    element.scrollIntoView({ block: 'center' })
    const rect = element.getBoundingClientRect()
    if (!rect.width || !rect.height) throw new Error('not visible')
    return { x: rect.x + rect.width / 2, y: rect.y + rect.height / 2 }
  })()`)
  await page.call('Input.dispatchMouseEvent', { type: 'mousePressed', button: 'left', clickCount: 1, ...point })
  await page.call('Input.dispatchMouseEvent', { type: 'mouseReleased', button: 'left', clickCount: 1, ...point })
}

async function fill(page, selector, text) {
  await page.call('Page.bringToFront')
  await page.evaluate(`(() => { const el = document.querySelector(${JSON.stringify(selector)}); el.focus(); el.select() })()`)
  await page.call('Input.insertText', { text })
}

function nativeBlocked(message) {
  throw Object.assign(new Error(`BLOCKED: ${message}`), { blocked: true })
}

/** Bind OS observations to this runner's Electron descendant, never another repo window. */
async function native(test, action, options = {}) {
  const helper = path.join(root, 'scripts/example-native.ps1')
  if (process.platform !== 'win32' || !existsSync(helper)) nativeBlocked('Windows native window observation is unavailable')
  const literal = (value) => `'${String(value).replaceAll("'", "''")}'`
  const args = Object.entries({ Action: action, RepositoryPath: root, ...options })
    .map(([key, value]) => typeof value === 'boolean' ? (value ? `-${key}` : '') : `-${key} ${literal(value)}`).join(' ')
  const lookup = test.nativePid ? `$ownerId = ${test.nativePid}` : `
    [Console]::Error.WriteLine('[native-phase] owner lookup start')
    $rows = @(Get-CimInstance Win32_Process)
    $parents = @{}
    foreach ($row in $rows) { $parents[[int] $row.ProcessId] = [int] $row.ParentProcessId }
    $owners = @(foreach ($row in $rows) {
      if ([string] $row.CommandLine -notmatch '(?:^|\\s)--remote-debugging-port=\\d+(?:\\s|$)' -or [string] $row.CommandLine -match '(?:^|\\s)--type=') { continue }
      $ancestor = [int] $row.ProcessId
      $seen = @{}
      while ($ancestor -and -not $seen.ContainsKey($ancestor)) {
        if ($ancestor -eq ${test.child.pid}) { $row; break }
        $seen[$ancestor] = $true
        $ancestor = [int] $parents[$ancestor]
      }
    })
    [Console]::Error.WriteLine('[native-phase] owner lookup done')
    if ($owners.Count -eq 0) { '{"ok":true,"windows":[]}'; exit 0 }
    if ($owners.Count -ne 1) { throw 'Ambiguous Electron descendants of this runner' }
    $ownerId = [int] $owners[0].ProcessId
  `
  const source = `$ErrorActionPreference = 'Stop'; try { ${lookup}; & ${literal(helper)} -ProcessId $ownerId ${args} } catch { @{ ok = $false; error = $_.Exception.Message } | ConvertTo-Json -Compress; exit 1 }`
  const { value, exitCode, stderr, raw } = await new Promise((resolve, reject) => {
    const child = spawn('pwsh', ['-NoProfile', '-NonInteractive', '-EncodedCommand', Buffer.from(source, 'utf16le').toString('base64')], { cwd: root, windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'] })
    let stdout = ''
    let stderr = ''
    const failure = (message) => {
      const phase = [...stderr.matchAll(/^\[native-phase\] (.+)$/gm)].at(-1)?.[1] ?? 'pwsh startup'
      return Object.assign(new Error(`${action}: ${message}; last phase: ${phase}; stderr: ${stderr.trim() || '(empty)'}; stdout: ${stdout.trim() || '(empty)'}`), { stderr, raw: stdout, phase })
    }
    const timer = setTimeout(() => { child.kill(); reject(failure('native helper timed out after 60000ms')) }, 60_000)
    child.stdout.on('data', (data) => { stdout += data })
    child.stderr.on('data', (data) => { stderr += data })
    child.on('error', (error) => { clearTimeout(timer); reject(failure(error.message)) })
    child.on('close', (code) => {
      clearTimeout(timer)
      try {
        const value = JSON.parse(stdout.trim().replace(/^\uFEFF/, ''))
        resolve({ value, exitCode: code, stderr, raw: stdout })
      } catch (error) { reject(failure(error.message)) }
    })
  }).catch((error) => {
    test.nativeObservations ??= []
    test.nativeObservations.push({ observedAt: new Date().toISOString(), action, ok: false, error: error.message, phase: error.phase, stderr: error.stderr, raw: error.raw })
    writeFileSync(path.join(test.directory, 'native-observations.json'), `${JSON.stringify(test.nativeObservations, null, 2)}\n`)
    nativeBlocked(error.message)
  })
  if (value.windows?.length) {
    const owners = new Set(value.windows.map((window) => window.pid))
    assert.equal(owners.size, 1, 'native windows must share the runner-owned Electron PID')
    test.nativePid = value.windows[0].pid
  }
  test.nativeObservations ??= []
  test.nativeObservations.push({ observedAt: new Date().toISOString(), action, ...value, exitCode, stderr, ...(!value.ok ? { raw } : {}) })
  writeFileSync(path.join(test.directory, 'native-observations.json'), `${JSON.stringify(test.nativeObservations, null, 2)}\n`)
  if (value.assertion && value.assertion.expected !== 'Any') assert.equal(value.assertion.passed, true, `${action}: ${JSON.stringify(value)}`)
  if (exitCode !== 0 || !value.ok) nativeBlocked(`${action}: ${value.error || stderr.trim() || `native helper exited ${exitCode}`}`)
  return value
}

function nativeWindow(snapshot, title) {
  const matches = snapshot.windows.filter((window) => window.title === title && window.className === 'Chrome_WidgetWin_1')
  assert.equal(matches.length, 1, `expected one runner-owned native window titled ${JSON.stringify(title)}`)
  return matches[0]
}

async function nativeWindows(test, titles, timeout = 15_000) {
  const deadline = Date.now() + timeout
  do {
    const snapshot = await native(test, 'List')
    if (titles.every((title) => snapshot.windows.some((window) => window.title === title && window.className === 'Chrome_WidgetWin_1'))) return snapshot
    await sleep(100)
  } while (Date.now() < deadline)
  nativeBlocked(`could not observe the original native windows: ${titles.join(', ')}`)
}

const checks = {
  api: (test) => apiChecks(test, { assert, click, fill, until, native, nativeWindow }),
  transparent: (test) => transparentChecks(test, { assert, click, until, native, nativeWindow, nativeWindows }),
  'file-associations': async (test) => {
    if (process.platform === 'darwin') nativeBlocked('macOS open-file delivery requires an installed bundle and real open-file event')
    const page = await test.page()
    await until(() => page.evaluate('window.openedFiles'), Array.isArray)
    await test.shot(page, '01-files-delivered')
    const expected = [test.fileArgument, test.fileArgument]
    assert.deepEqual(await page.evaluate('window.openedFiles'), expected, 'native argv and file URL survive; flags and non-file URLs are ignored')
    assert.equal(await page.evaluate('document.querySelector("#files").textContent'), expected.join('\n'))
    await page.call('Page.reload')
    await until(() => page.evaluate('document.querySelector("#files").textContent'), (text) => text === expected.join('\n'))
    await test.shot(page, '02-initialization-survives-reload')
    test.pass('original argv parser delivers real existing path/file URL and original page renders exact paths after reload')
    test.limitations.push('Operating-system file association registration/double-click requires an installed bundle; this run proves delivery and display only.')
  },
  streaming: async (test) => {
    const page = await test.page()
    const videoState = () => page.evaluate(`(() => { const v = document.querySelector('video'); return { ready: v.readyState, width: v.videoWidth, height: v.videoHeight, duration: v.duration, time: v.currentTime, paused: v.paused, error: v.error && { code: v.error.code, message: v.error.message } } })()`)
    const loaded = await until(videoState, (value) => value.error || (value.ready >= 2 && value.width > 0 && value.duration > 0), 30_000)
    assert.equal(loaded.error, null, `real MP4 decoder: ${JSON.stringify(loaded.error)}`)
    await page.evaluate('document.querySelector("video").pause()')
    const bytes = readFileSync(path.join(root, 'source/electron/spec/fixtures/cat-spin.mp4'))
    // Observe the media request itself; the original CSP permits media-src stream
    // while intentionally disallowing script connect-src stream.
    await page.call('Network.enable')
    await page.call('Network.setCacheDisabled', { cacheDisabled: true })
    await page.call('Page.reload')
    await until(videoState, (value) => value.ready >= 2 && value.width > 0, 30_000)
    const partial = await until(() => page.events.filter((event) => event.method === 'Network.responseReceived').map((event) => event.params.response).find((response) => response.url.includes('streaming_example_test_video.mp4') && response.status === 206))
    const range = Object.entries(partial.headers).find(([key]) => key.toLowerCase() === 'content-range')?.[1]
    assert.match(range ?? '', new RegExp(`^bytes \\d+-\\d+/${bytes.length}$`))
    test.protocolObservations = { url: partial.url, status: partial.status, headers: partial.headers, fixtureBytes: bytes.length }
    await page.evaluate('document.querySelector("video").pause()')
    await test.shot(page, '01-video-decoded-paused')
    await click(page, 'video')
    await page.evaluate('document.querySelector("video").play()')
    const start = (await videoState()).time
    await until(videoState, (value) => value.time > start + 0.25 || (value.time < start && !value.paused), 15_000)
    await test.shot(page, '02-video-playing')
    await page.evaluate('document.querySelector("video").pause()')
    const seek = Math.min(loaded.duration * 0.6, loaded.duration - 0.1)
    await page.evaluate(`document.querySelector('video').currentTime = ${seek}`)
    await until(videoState, (value) => Math.abs(value.time - seek) < 0.12 && value.ready >= 2)
    await test.shot(page, '03-video-seeked')
    test.pass(`original media element decodes ${loaded.width}x${loaded.height} MP4, plays, pauses and seeks through the Rust stream protocol (${bytes.length} fixture bytes)`)
    test.limitations.push('Original streaming CSP blocks script connect-src to stream; HTTP edge-range assertions require the independent protocol regression, not CSP relaxation.')
  },
  multiwebview: async (test) => {
    await test.page('main')
    // The empty BrowserWindow host is also a CDP target. Only identify a child
    // by its injected webview label or Chromium's committed/failed-navigation URL.
    const requested = ['http://tauri.localhost/', 'https://github.com/tauri-apps/tauri', 'https://tauri.app/', 'https://twitter.com/TauriApps']
    let observed = []
    let children
    try {
      children = await until(async () => {
        observed = []
        for (const target of (await test.targets()).filter((target) => target.url && target.url !== 'about:blank')) {
          const page = await test.session(target)
          const { frameTree: { frame } } = await page.call('Page.getFrameTree')
          const value = await page.evaluate('({ label: window.__TAURI_INTERNALS__?.metadata?.currentWebview?.label, url: location.href, title: document.title, width: innerWidth, height: innerHeight, text: document.body?.innerText.slice(0, 400) ?? "" })')
          const requestedUrl = frame.unreachableUrl || frame.url
          const index = requested.indexOf(requestedUrl)
          const label = /^main[1-4]$/.test(value.label ?? '') ? value.label : index < 0 ? undefined : `main${index + 1}`
          observed.push({ targetId: target.id, targetUrl: target.url, frame, ...value, label })
          if (label) observed[observed.length - 1].page = page
        }
        return observed.filter((item) => item.page)
      }, (items) => items.length === 4 && new Set(items.map((item) => item.label)).size === 4, 60_000)
    } catch (error) {
      writeFileSync(path.join(test.directory, 'navigation-observations.json'), `${JSON.stringify(observed, (key, value) => key === 'page' ? undefined : value, 2)}\n`)
      nativeBlocked(`original child navigation identities did not settle: ${error.message}`)
    }
    writeFileSync(path.join(test.directory, 'navigation-observations.json'), `${JSON.stringify(observed, (key, value) => key === 'page' ? undefined : value, 2)}\n`)
    assert.deepEqual(children.map((item) => item.label).sort(), ['main1', 'main2', 'main3', 'main4'])
    assert.ok(children.every((item) => item.width === 400 && item.height === 300), 'four original 400x300 native children')
    // This example never sets a title; select its sole visible native parent
    // inside the already validated Electron descendant instead of inventing one.
    const snapshot = await until(() => native(test, 'List'), (value) => value.windows.filter((window) => window.className === 'Chrome_WidgetWin_1' && window.visible).length === 1)
    const window = snapshot.windows.find((window) => window.className === 'Chrome_WidgetWin_1' && window.visible)
    await test.nativeShot(window, '01-four-native-quadrants')
    const failures = []
    for (const child of children) {
      await test.shot(child.page, `02-${child.label}`, false)
      if (child.label === 'main1') assert.ok(child.text.includes('Welcome to Tauri!'))
      else if (!child.title || /^(chrome-error:|about:blank)/.test(child.url) || /ERR_|This site can.t be reached/.test(child.text)) failures.push(`${child.label}: ${child.url} ${child.text}`)
    }
    const start = { x: window.rect.right - 2, y: window.rect.bottom - 2 }
    const resize = await native(test, 'Drag', { WindowHandle: window.hwnd, StartX: start.x, StartY: start.y, EndX: start.x - 150, EndY: start.y - 120, Activate: true })
    assert.ok(resize.after.width < resize.before.width - 100 && resize.after.height < resize.before.height - 70, 'real native resize must shrink parent')
    for (const child of children) {
      await until(() => child.page.evaluate('({ width: innerWidth, height: innerHeight })'), (value) => value.width < child.width - 30 && value.height < child.height - 20)
    }
    await test.nativeShot(window, '03-quadrants-after-native-resize')
    assert.equal(await children.find((child) => child.label === 'main1').page.evaluate('document.querySelector("h1").textContent'), 'Welcome to Tauri!')
    test.pass('four independent native WebContentsViews preserve original labels, quadrants and auto-resize with the parent')
    if (failures.length) nativeBlocked(`original remote pages did not load: ${failures.join('; ')}`)
  },
  drag: async (test) => {
    const page = await test.page()
    const window = nativeWindow(await nativeWindows(test, ['Welcome to Tauri!']), 'Welcome to Tauri!')
    const cases = [
      [0, '', false, 'no-attribute'],
      [1, '', true, 'bare-direct'], [1, 'span', false, 'bare-child'],
      [2, '.nested-content', true, 'deep-child'],
      [3, '', false, 'disabled'],
      [4, 'button', false, 'button-blocks'], [5, 'input', false, 'input-blocks'],
      [6, 'button', false, 'deep-button-blocks'], [6, 'a', false, 'deep-link-blocks'],
      [7, '.nested-content', true, 'deep-text'], [8, '.nested-content', false, 'nested-disabled'],
      [10, '[role="button"]', false, 'role-button-blocks'],
      [10, '[role="link"]', false, 'role-link-blocks'],
      [10, '[role="checkbox"]', false, 'role-checkbox-blocks'],
      [11, 'button', true, 'explicit-button'], [11, 'a', true, 'explicit-link'],
      [12, '.nested-content', true, 'explicit-deep-button'],
      [13, '', false, 'explicit-disabled-button'],
      [14, '[role="button"]', true, 'explicit-role'], [14, '[role="tab"] span', true, 'explicit-deep-role'],
      [15, '[role="tab"]', false, 'bare-role-child-blocks'],
      [15, '[role="switch"]', false, 'switch-child-blocks'], [15, '[role="option"]', false, 'option-child-blocks'],
    ]
    const point = async (index, descendant) => {
      await page.call('Page.bringToFront')
      const element = await page.evaluate(`(() => {
        const section = document.querySelectorAll('.qa-test-case')[${index}]
        const el = ${descendant ? `section.querySelector(${JSON.stringify(descendant)})` : "section.querySelector('.qa-testable')"}
        if (!el) throw new Error('missing original drag test element')
        el.scrollIntoView({ block: 'center' })
        const r = el.getBoundingClientRect()
        const x = ${descendant ? 'r.x + r.width / 2' : 'r.x + 10'}
        const y = ${descendant ? 'r.y + r.height / 2' : 'r.y + 10'}
        if (document.elementFromPoint(x, y) !== el) throw new Error('pointer must land on original target itself')
        return { x, y, width: innerWidth }
      })()`)
      const current = (await native(test, 'Rect', { WindowHandle: window.hwnd })).window
      const scale = current.clientRect.width / element.width
      return { x: Math.round(current.clientRect.left + element.x * scale), y: Math.round(current.clientRect.top + element.y * scale) }
    }
    let direction = -1
    for (const [index, descendant, moves, label] of cases) {
      const start = await point(index, descendant)
      await test.nativeShot(window, `${label}-before`)
      const dx = direction * 36, dy = direction * 20
      await native(test, 'Drag', { WindowHandle: window.hwnd, StartX: start.x, StartY: start.y, EndX: start.x + dx, EndY: start.y + dy, Expected: moves ? 'Moved' : 'Stationary', Activate: true, DurationMs: 800 })
      if (moves) direction *= -1
      await test.nativeShot(window, `${label}-after`)
      assert.ok(await page.evaluate('Boolean(window.__TAURI_INTERNALS__.invoke)'), 'renderer and IPC stay alive after native mouse release')
      test.pass(`real OS drag ${label}: ${moves ? 'window moved by pointer delta' : 'window stayed still'}`)
    }
    const start = await point(9, '')
    await test.nativeShot(window, 'double-click-before')
    await native(test, 'DoubleClick', { WindowHandle: window.hwnd, StartX: start.x, StartY: start.y, Activate: true })
    await until(() => native(test, 'Rect', { WindowHandle: window.hwnd }), (value) => value.window.maximized)
    await test.nativeShot(window, 'double-click-maximized')
    test.pass('real native double click on the original region maximizes the window')
  },
  commands: async (test) => {
    const page = await test.page()
    await until(() => page.evaluate('document.querySelectorAll("#container button").length'), (count) => count === 29)
    await test.shot(page, '01-command-controls')
    const returnsValue = new Set([
      'borrow_cmd', 'simple_command_with_result', 'future_simple_command_with_return_snake',
      'future_simple_command_with_result_snake', 'force_async_snake', 'force_async_with_result_snake',
      'simple_command_with_result_snake', 'stateful_command_with_result_snake',
      'stateful_command_with_result', 'async_simple_command_with_result',
      'future_simple_command_with_return', 'future_simple_command_with_result',
      'async_stateful_command_with_result',
    ])
    const missingAllowed = new Set(['window_label', 'stateful_command', 'async_stateful_command', 'command_arguments_wild', 'renamed_command_new', 'renamed_command_in_mod_new'])
    const rawBytes = [...readFileSync(path.join(root, 'source/tauri/examples/commands/README.md'))]
    // innerText normalizes CRLF; the separate ArrayBuffer assertion keeps every byte.
    const raw = Buffer.from(rawBytes).toString('utf8').replaceAll('\r\n', '\n')
    for (let index = 0; index < 29; index++) {
      const name = await page.evaluate(`document.querySelectorAll('#container button')[${index}].textContent.slice(4)`)
      const renamed = name === 'renamed_command' || name === 'renamed_command_in_mod'
      const expected = renamed ? `Err(Command ${name} not found)` : name === 'raw_request' ? `Ok(${raw})` : returnsValue.has(name) ? 'Ok(value)' : 'Ok(null)'
      // Observe both fresh responses: many buttons return the same text, so
      // reading the previous command's result would make a no-op pass.
      const rendered = page.evaluate(`new Promise(resolve => {
        const nodes = ['#response', '#response-optional'].map(selector => document.querySelector(selector))
        const updated = new Set()
        const observer = new MutationObserver(records => {
          for (const record of records) updated.add(nodes.findIndex(node => node === record.target || node.contains(record.target)))
          if (updated.has(0) && updated.has(1)) {
            observer.disconnect()
            resolve(nodes.map(node => node.innerText))
          }
        })
        for (const node of nodes) observer.observe(node, { childList: true, subtree: true, characterData: true })
      })`)
      await click(page, `#container button:nth-child(${index + 1})`)
      const [response, optional] = await rendered
      assert.equal(response, expected, name)
      assert.ok(((text) => {
        if (renamed) return text === expected
        if (name === 'raw_request') return text === `Ok(${raw})`
        if (missingAllowed.has(name)) return text === 'Ok(null)'
        if (name.startsWith('stateful_command_with_result')) return text === 'Err(FooError)'
        if (name === 'async_stateful_command_with_result') return text === 'Ok()'
        return text.startsWith('Err(invalid args')
      })(optional), `${name} without arguments: ${optional}`)
      test.pass(`${name}: original button checks provided and missing arguments`)
      await test.shot(page, `command-${String(index + 1).padStart(2, '0')}-${name}`)
    }
    assert.deepEqual(await page.evaluate(`(async () => [...new Uint8Array(await window.__TAURI__.core.invoke('raw_request'))])()`), rawBytes, 'raw response preserves exact README bytes, including CRLF')
    const value = await page.evaluate(`(async () => {
      const invoke = window.__TAURI__.core.invoke
      const result = {}
      for (const name of ['force_async', 'force_async_with_result', 'borrow_cmd_async']) result[name] = await invoke(name, { theArgument: 'borrowed' })
      for (const name of ['cmd', 'invoke', 'message', 'resolver']) result[name] = await invoke(name, { argument: 'reserved-name' })
      for (const name of ['simple_command_with_result', 'force_async_with_result', 'force_async_with_result_snake']) {
        try { await invoke(name, { theArgument: '', the_argument: '' }); result[name + 'Error'] = 'unexpected success' }
        catch (error) { result[name + 'Error'] = error }
      }
      return result
    })()`)
    assert.deepEqual(value, {
      force_async: 'borrowed', force_async_with_result: 'borrowed', borrow_cmd_async: 'borrowed',
      cmd: null, invoke: null, message: null, resolver: null,
      simple_command_with_resultError: 'FooError', force_async_with_resultError: 'FooError', force_async_with_result_snakeError: 'FooError',
    })
    test.pass('commands absent from the page: borrowed, forced-async, reserved names and error propagation')
  },
  isolation: async (test) => {
    const page = await test.page()
    const frame = await until(() => page.evaluate(`(() => {
      const frame = document.querySelector('iframe')
      return frame && { src: frame.src, sandbox: frame.getAttribute('sandbox') }
    })()`))
    assert.match(frame.src, /isolation/)
    assert.ok(frame.sandbox?.includes('allow-scripts'))
    assert.equal(await page.evaluate('typeof window.__TAURI_ISOLATION_HOOK__'), 'undefined')
    // The sandbox has its own origin/process. Attach its actual CDP session;
    // the top page's console cannot prove what ran inside that frame.
    await page.call('Target.setAutoAttach', { autoAttach: true, waitForDebuggerOnStart: false, flatten: true })
    const isolated = await until(() => page.events.find((event) => event.method === 'Target.attachedToTarget' && event.params.targetInfo.type === 'iframe' && event.params.targetInfo.url.startsWith(frame.src)))
    const isolatedId = isolated.params.sessionId
    await page.call('Runtime.enable', {}, 20_000, isolatedId)
    const hook = await page.call('Runtime.evaluate', { expression: 'typeof window.__TAURI_ISOLATION_HOOK__', returnByValue: true }, 20_000, isolatedId)
    assert.equal(hook.result.value, 'function', 'original hook lives in the real sandbox frame')
    await test.shot(page, '01-isolated-page-before-ping')
    await click(page, '#ping')
    await until(() => page.evaluate('document.querySelector("#pong").textContent'), (text) => /^ok: \d+$/.test(text))
    await until(() => page.events.filter((event) => event.method === 'Runtime.consoleAPICalled' && event.sessionId === isolatedId), (messages) => messages.some((event) => event.params.args.some((arg) => arg.value === 'hook')))
    test.protocolObservations = { isolatedFrame: frame, hookType: hook.result.value, authenticatedSandboxConsole: true }
    assert.match(readFileSync(test.log, 'utf8'), /ping: Instant/)
    test.pass('original isolation iframe and hook process encrypted invoke; Rust handles ping')
    await test.shot(page, '02-isolated-ping-returned')
  },
  helloworld: async (test) => {
    const page = await test.page()
    await test.shot(page, '01-form')
    await fill(page, '#name', 'World')
    await click(page, '#form button')
    await until(() => page.evaluate('document.querySelector("#message").textContent'), (value) => value === 'Hello World, You have been greeted from Rust!')
    test.pass('submit the original form and render Rust greet response')
    await test.shot(page, '02-greeting')
  },
  state: async (test) => {
    const page = await test.page()
    const count = () => page.evaluate('document.querySelector("#counter").textContent')
    await until(count, (value) => value === '0')
    await test.shot(page, '01-initial')
    await click(page, '#increment-btn')
    await until(count, (value) => value === '1')
    await click(page, '#increment-btn')
    await until(count, (value) => value === '2')
    await test.shot(page, '02-incremented')
    await page.call('Page.reload')
    await until(count, (value) => value === '2')
    test.pass('Rust managed state survives a real page reload')
    await test.shot(page, '03-persisted-after-reload')
    await click(page, '#decrement-btn')
    await until(count, (value) => value === '1')
    await click(page, '#reset-btn')
    await until(count, (value) => value === '0')
    test.pass('increment, decrement and reset update the original counter UI')
    await test.shot(page, '04-reset')
  },
  resources: async (test) => {
    const page = await test.page()
    const value = await until(() => page.evaluate('({ path: document.querySelector("#path").textContent, content: document.querySelector("#content").textContent })'), (value) => value.content.includes('SPDX'))
    assert.match(value.path, /assets[\\/]index\.js$/)
    assert.equal(value.content, readFileSync(path.join(root, 'source/tauri/examples/resources/src-tauri/assets/index.js'), 'utf8'))
    test.pass('original page resolves and reads the exact bundled resource bytes')
    await test.shot(page, '01-resource-content')
    await page.call('Page.reload')
    await until(() => page.evaluate('document.querySelector("#content").textContent'), (text) => text === value.content)
    await test.shot(page, '02-resource-after-reload')
  },
  multiwindow: async (test) => {
    if (process.platform !== 'win32') nativeBlocked('multiwindow requires native OS title/visibility/WM_CLOSE observation on this platform')
    const main = await test.page('Main')
    const secondary = await test.page('Secondary')
    const third = await test.page('Third')
    assert.equal((await test.pages()).length, 3)
    const titles = ['Tauri - Main', 'Tauri - Secondary', 'Tauri - Third']
    const initial = await nativeWindows(test, titles)
    const original = titles.map((title) => nativeWindow(initial, title))
    assert.equal(new Set(original.map((window) => window.hwnd)).size, 3, 'three separate native windows')
    assert.ok(original.every((window) => window.visible && !window.minimized), 'all original windows are visible')
    await test.shot(main, '01-main')
    await test.shot(secondary, '02-secondary')
    await test.shot(third, '03-third-from-rust')
    const readMessages = (page) => page.evaluate('document.querySelector("#messages-view").textContent')
    const send = async (page, label, message) => {
      await fill(page, '#send-message', message)
      await fill(page, '#send-label', label)
      await click(page, '#send-message-form button')
    }
    const [mainBefore, secondaryBefore, thirdBefore] = await Promise.all([main, secondary, third].map(readMessages))
    const token = `Main to Secondary: ${Date.now()}`
    await send(main, 'Secondary', token)
    const received = await until(() => readMessages(secondary), (text) => text !== secondaryBefore && text.includes(token))
    assert.equal(received.split(token).length - 1, 1, 'target receives exactly once')
    assert.equal(await readMessages(main), mainBefore, 'sender must not receive its own targeted message')
    assert.equal(await readMessages(third), thirdBefore, 'third must not receive another window target')
    await secondary.evaluate('document.querySelector("#messages-view").scrollIntoView({ block: "center" })')
    await test.shot(secondary, '04-cross-window-received')
    const reply = `Secondary reply: ${Date.now()}`
    await send(secondary, 'Main', reply)
    const answered = await until(() => readMessages(main), (text) => text !== mainBefore && text.includes(reply))
    assert.equal(answered.split(reply).length - 1, 1, 'reply received exactly once')
    assert.equal(await readMessages(secondary), received, 'reply sender must not receive its own reply')
    assert.equal(await readMessages(third), thirdBefore, 'third remains a separate target')
    await main.evaluate('document.querySelector("#messages-view").scrollIntoView({ block: "center" })')
    await test.shot(main, '05-reply-received')
    await fill(main, '#new-label', 'Fourth')
    const fourthTitle = `Created from the official form ${Date.now()}`
    await fill(main, '#new-title', fourthTitle)
    await click(main, '#new-window-form button')
    const fourth = await test.page('Fourth')
    assert.equal((await test.pages()).length, 4)
    const created = nativeWindow(await nativeWindows(test, [fourthTitle]), fourthTitle)
    assert.ok(created.visible && !original.some((window) => window.hwnd === created.hwnd), 'UI creates a new native window with the requested title')
    const fourthMessage = `Fourth receives: ${Date.now()}`
    await send(main, 'Fourth', fourthMessage)
    await until(() => readMessages(fourth), (text) => text.includes(fourthMessage))
    await fourth.evaluate('document.querySelector("#messages-view").scrollIntoView({ block: "center" })')
    await test.shot(fourth, '06-window-created-from-ui')
    const closing = await native(test, 'Close', { WindowHandle: created.hwnd })
    assert.equal(closing.closeRequested, true, 'WM_CLOSE must be posted')
    await until(test.pages, (pages) => pages.length === 3)
    const remaining = await until(() => native(test, 'List'), (snapshot) => !snapshot.windows.some((window) => window.hwnd === created.hwnd))
    for (const window of original) assert.ok(remaining.windows.some((candidate) => candidate.hwnd === window.hwnd && candidate.visible), `original window ${window.title} survives normal child close`)
    assert.equal(await readMessages(main), answered)
    test.pass('three original native windows including Rust Third, exact targeted request/reply, original UI creates a fourth and WM_CLOSE removes only that window')
  },
  splashscreen: async (test) => {
    if (process.platform !== 'win32') nativeBlocked('splashscreen requires native OS visibility observation on this platform')
    // Start OS observation as soon as CDP is available; never pause/rewrite the
    // original main-page timer or bring the initially hidden main to the front.
    await until(test.pages, (pages) => pages.length > 0, 240_000)
    const [, initial] = await Promise.all([
      (async () => { const splash = await test.page('splashscreen'); await test.shot(splash, '01-visible-splash', false) })(),
      nativeWindows(test, ['Tauri', 'Splashscreen'], 5000),
    ])
    const hiddenMain = nativeWindow(initial, 'Tauri')
    const visibleSplash = nativeWindow(initial, 'Splashscreen')
    if (hiddenMain.visible || !visibleSplash.visible) nativeBlocked('the original initial hidden-main/visible-splash state was not observed before the two-second timer')
    assert.equal(hiddenMain.minimized, false, 'main is hidden, not minimized')
    assert.equal(visibleSplash.minimized, false)
    const main = await test.page('main')
    await until(test.pages, (pages) => pages.length === 1)
    const final = await until(() => native(test, 'List'), (snapshot) => !snapshot.windows.some((window) => window.hwnd === visibleSplash.hwnd) && snapshot.windows.some((window) => window.hwnd === hiddenMain.hwnd && window.visible && !window.minimized))
    assert.equal(nativeWindow(final, 'Tauri').hwnd, hiddenMain.hwnd, 'the same initially hidden main becomes visible')
    assert.equal(await main.evaluate('document.querySelector("h1").textContent'), 'This is the main window!')
    await test.shot(main, '02-main-after-original-timer')
    test.pass('native observation proves main starts hidden, original two-second timer destroys splash and reveals the same main window')
  },
  'run-return': async (test) => {
    const page = await test.page()
    await test.shot(page, '01-before-normal-close')
    const window = nativeWindow(await nativeWindows(test, ['Welcome to Tauri!']), 'Welcome to Tauri!')
    assert.ok(window.visible && !window.minimized)
    const closing = await native(test, 'Close', { WindowHandle: window.hwnd })
    assert.equal(closing.closeRequested, true, 'post a real WM_CLOSE without forcing the app to exit')
    await until(() => test.child.exitCode, (code) => code !== null, 20_000)
    assert.equal(test.child.exitCode, 0)
    assert.equal(readFileSync(test.log, 'utf8').split('I run after exit').length - 1, 1)
    test.pass('normal last-window WM_CLOSE returns from app.run_return exactly once and exits zero')
  },
}

async function freePort() {
  const server = createServer()
  await new Promise((resolve, reject) => { server.once('error', reject); server.listen(0, '127.0.0.1', resolve) })
  const port = server.address().port
  await new Promise((resolve) => server.close(resolve))
  return port
}

async function check(name) {
  const directory = path.join(artifacts, name)
  mkdirSync(directory, { recursive: true })
  assert.equal(path.dirname(path.resolve(directory)), path.resolve(root, 'target/example-checks'), 'evidence cleanup must stay inside target/example-checks')
  assert.equal(realpathSync(directory), path.resolve(directory), 'evidence cleanup must not follow a redirected directory')
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    if (entry.isFile() && (entry.name.endsWith('.png') || ['native-observations.json', 'result.json', 'run.log'].includes(entry.name))) {
      rmSync(path.join(directory, entry.name))
    }
  }
  const log = path.join(directory, 'run.log')
  const output = openSync(log, 'w')
  const port = await freePort()
  const fileArgument = path.join(directory, 'sample file 中文.png')
  const args = []
  if (name === 'file-associations') {
    writeFileSync(fileArgument, readFileSync(path.join(root, 'source/tauri/examples/.icons/32x32.png')))
    args.push('--', fileArgument, pathToFileURL(fileArgument).href, '--ignored-flag', 'https://example.invalid/not-a-file')
  }
  const child = spawn(process.execPath, [path.join(root, 'scripts/example.mjs'), name, ...args], {
    cwd: root, stdio: ['ignore', output, output], detached: process.platform !== 'win32',
    env: { ...process.env, TAURI_RUNTIME_ELECTRON_ARGS: `${process.env.TAURI_RUNTIME_ELECTRON_ARGS ?? ''} --remote-debugging-address=127.0.0.1 --remote-debugging-port=${port}` },
  })
  const sessions = new Map()
  const report = { name, platform: process.platform, started: new Date().toISOString(), checks: [], frames: [], assertions: 'failed', visualReview: 'pending' }
  child.on('error', (error) => { report.error = error.message })
  const targets = async () => {
    const response = await fetch(`http://127.0.0.1:${port}/json/list`, { signal: AbortSignal.timeout(1500) })
    assert.ok(response.ok, 'CDP endpoint failed')
    return (await response.json()).filter((target) => ['page', 'webview'].includes(target.type) && !target.url.startsWith('devtools:'))
  }
  const pages = async () => (await targets()).filter((target) => /^(tauri:|https?:|file:|data:)/.test(target.url))
  async function session(target) {
    if (!sessions.has(target.id)) sessions.set(target.id, await Session.connect(target))
    return sessions.get(target.id)
  }
  const test = {
    child, log, directory, targets, pages, session, fileArgument, limitations: [], coverage: [],
    pass(message) { report.checks.push(message); console.log(`  ok   ${message}`) },
    async page(label) {
      return until(async () => {
        if (report.error || child.exitCode !== null) throw Object.assign(new Error(report.error ?? `runner exited ${child.exitCode}; see ${log}`), { fatal: true })
        for (const target of await pages()) {
          const page = await session(target)
          if (await page.evaluate('document.readyState === "complete" && Boolean(window.__TAURI_INTERNALS__?.invoke)') && (!label || await page.evaluate(labelExpression) === label)) return page
        }
      }, Boolean, name === 'api' ? 1_200_000 : 900_000)
    },
    async nativeShot(window, key) {
      const file = path.join(directory, `${key}.png`)
      if (existsSync(file)) rmSync(file)
      const capture = await native(test, 'Capture', { WindowHandle: window.hwnd, OutputPath: file, Activate: true })
      report.frames.push({ file: path.relative(root, file).replaceAll('\\', '/'), title: capture.window.title, capture: capture.capture, window: capture.window })
      console.log(`  frame ${path.relative(root, file)} (native)`)
    },
    async shot(page, key, foreground = true) {
      if (foreground) await page.call('Page.bringToFront')
      const { data } = await page.call('Page.captureScreenshot', { format: 'png', captureBeyondViewport: false })
      const file = path.join(directory, `${key}.png`)
      writeFileSync(file, Buffer.from(data, 'base64'))
      const state = await page.evaluate(`({ label: ${labelExpression}, url: location.href, text: document.body.innerText.slice(0, 8000), size: [innerWidth, innerHeight] })`)
      report.frames.push({ file: path.relative(root, file).replaceAll('\\', '/'), ...state })
      console.log(`  frame ${path.relative(root, file)}`)
    },
  }
  try {
    if (!checks[name]) throw new Error('functional checks not implemented; not a pass')
    await checks[name](test)
    assert.ok(report.frames.length, 'no visual evidence was captured')
    report.assertions = 'passed'
  } catch (error) {
    report.error = error.message
    if (error.blocked) report.assertions = 'blocked'
    report.rendererErrors = [...sessions.values()].flatMap((page) => page.events
      .filter((event) => event.method === 'Runtime.exceptionThrown' || (event.method === 'Runtime.consoleAPICalled' && event.params.type === 'error'))
      .map((event) => ({ target: page.target.url, ...event })))
    for (const page of sessions.values()) {
      try { await test.shot(page, `failure-${report.frames.length + 1}`, false) } catch {}
    }
    console.error(`  FAIL ${report.error}`)
  } finally {
    // Normal window closure first, so Cargo and the runner can restore the copy.
    for (const page of sessions.values()) {
      try { await page.evaluate('window.__TAURI__?.window?.getCurrentWindow().close()') } catch {}
    }
    const deadline = Date.now() + 8000
    while (child.exitCode === null && Date.now() < deadline) await sleep(100)
    if (child.exitCode === null) {
      if (process.platform === 'win32') spawnSync('taskkill', ['/PID', String(child.pid), '/T', '/F'], { stdio: 'ignore' })
      else { try { process.kill(-child.pid, 'SIGKILL') } catch {} }
      await until(() => child.exitCode, (code) => code !== null, 5000).catch(() => {})
      // A hard-killed runner cannot release its own lock. Never remove a lock
      // another runner acquired in the meantime.
      const lock = path.join(root, 'target/example.lock')
      if (existsSync(lock)) {
        try {
          if (JSON.parse(readFileSync(lock, 'utf8')).pid === child.pid) rmSync(lock)
        } catch {}
      }
    }
    for (const page of sessions.values()) page.close()
    closeSync(output)
    report.finished = new Date().toISOString()
    report.limitations = test.limitations
    report.coverage = test.coverage
    report.protocolObservations = test.protocolObservations
    writeFileSync(path.join(directory, 'result.json'), `${JSON.stringify(report, null, 2)}\n`)
  }
  return report
}

const available = [...new Set(['source/tauri/examples', 'example'].flatMap((directory) =>
  existsSync(path.join(root, directory)) ? readdirSync(path.join(root, directory), { withFileTypes: true })
    .filter((entry) => entry.isDirectory() && /^[a-z][a-z0-9-]*$/.test(entry.name))
    .filter((entry) => existsSync(path.join(root, directory, entry.name, 'main.rs')) || existsSync(path.join(root, directory, entry.name, 'src-tauri/Cargo.toml')))
    .map((entry) => entry.name) : [],
))]
const requested = process.argv.slice(2)
assert.ok(requested.every((name) => available.includes(name)), `unknown example; choose ${available.join(', ')}`)
const results = []
for (const name of requested.length ? requested : available) {
  console.log(`[check] ${name}`)
  results.push(await check(name))
}
mkdirSync(artifacts, { recursive: true })
writeFileSync(path.join(artifacts, 'results.json'), `${JSON.stringify(results, null, 2)}\n`)
for (const result of results) console.log(`${result.assertions === 'passed' ? 'ASSERTIONS' : 'FAIL'} ${result.name}: ${result.error ?? 'passed; screenshots await visual review'}`)
process.exitCode = results.some((result) => result.assertions !== 'passed') ? 1 : 0
