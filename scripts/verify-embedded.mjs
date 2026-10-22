// Real desktop/iframe/guest checks. The parent supplies the scoped t.native helper.
// node scripts/verify-embedded.mjs --self-check runs only the pure checks below.
import assert from 'node:assert/strict'
import { readFileSync, writeFileSync } from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { setTimeout as delay } from 'node:timers/promises'

const json = JSON.stringify
const page = (t, target, fn, value) => t.evaluate(target, `(${fn})(${json(value) ?? ''})`)
const save = (t, name, value) => writeFileSync(path.join(t.dir, `${name}.json`), json(value, null, 2) + '\n')
const rect = ({ x, y, width, height }) => ({ x, y, width, height })
const compact = (source) => source.replace(/\s+/g, '').replace(/;$/, '')
const bytes = (value) => Array.isArray(value) && value.every((n) => Number.isInteger(n) && n >= 0 && n <= 255)
const encrypted = (value) => value && bytes(value.nonce) && value.nonce.length === 12
  && bytes(value.payload) && value.payload.length >= 16 && value.contentType === 'application/json'

function packet(raw, command) {
  try {
    const value = typeof raw === 'string' ? JSON.parse(raw) : raw
    const payload = encrypted(value) ? value : value?.payload
    if (!encrypted(payload)) return null
    // Do not retain the invoke key or transport headers in acceptance evidence.
    return { cmd: value.cmd ?? command, callback: value.callback, error: value.error, payload }
  } catch { return null }
}

function bridgeCallSite(source, location) {
  const offset = source.split('\n').slice(0, location.lineNumber).reduce((n, line) => n + line.length + 1, 0) + location.columnNumber
  const call = /window\.ipc\.postMessage\(\s*([\w$]+)\s*\)/.exec(source.slice(offset))
  assert(call, 'original serialized IPC bridge call site is unavailable')
  const before = source.slice(0, offset + call.index).split('\n')
  return { variable: call[1], location: { scriptId: location.scriptId, lineNumber: before.length - 1, columnNumber: before.at(-1).length } }
}

function nativeFetchCallSite(source, scriptId) {
  const marker = "const bytes = Uint8Array.from(atob(reply.body ?? ''), c => c.charCodeAt(0))"
  const offset = source.indexOf(marker)
  assert(offset >= 0 && source.indexOf(marker, offset + marker.length) < 0, 'original native IPC response call site is unavailable or ambiguous')
  const before = source.slice(0, offset).split('\n')
  return { scriptId, lineNumber: before.length - 1, columnNumber: before.at(-1).length }
}

function quadrants(width, height) {
  return [
    { x: 0, y: 0, width: width / 2, height: height / 2 },
    { x: width / 2, y: 0, width: width / 2, height: height / 2 },
    { x: 0, y: height / 2, width: width / 2, height: height / 2 },
    { x: width / 2, y: height / 2, width: width / 2, height: height / 2 },
  ]
}

function collecting(t) {
  const failures = []
  return {
    check(...args) { try { t.check(...args) } catch (error) { failures.push(error) } },
    finish() { if (failures.length) throw new AggregateError(failures, failures.map((e) => e.message).join('\n')) },
  }
}

async function nativeWindow(t) {
  assert.equal(typeof t.native, 'function', `${t.name} requires the scoped Windows native helper`)
  const windows = await t.native('state')
  assert(Array.isArray(windows), 'native state must return a JSON array')
  const visible = windows.filter((w) => w.visible && w.clientWidth > 0 && w.clientHeight > 0)
  assert.equal(visible.length, 1, `expected one visible own native window; got ${json(visible)}`)
  return visible[0]
}

const desktop = (t, label) => t.native('capture', { OutputPath: path.join(t.dir, `${label}-native.png`) })

// The hit must be the specified element itself. closest() would hide bare-region bugs.
function dragPoint({ number, selector }) {
  const test = document.querySelectorAll('.qa-test-case')[number - 1]
  const element = test?.querySelector(selector)
  if (!element) throw new Error(`QA ${number}: missing ${selector}`)
  element.scrollIntoView({ block: 'center', inline: 'center' })
  const r = element.getBoundingClientRect()
  const left = Math.max(1, r.left + 3), right = Math.min(innerWidth - 2, r.right - 3)
  const top = Math.max(1, r.top + 3), bottom = Math.min(innerHeight - 2, r.bottom - 3)
  for (let y = top; y <= bottom; y += Math.max(1, Math.min(12, (bottom - top) / 4))) {
    for (let x = left; x <= right; x += Math.max(1, Math.min(12, (right - left) / 8))) {
      if (document.elementFromPoint(x, y) !== element) continue
      window.__verifyEmbeddedDrag.expected = element
      window.__verifyEmbeddedDrag.events.length = 0
      return { x, y, dpr: devicePixelRatio, viewport: { width: innerWidth, height: innerHeight },
        target: { tag: element.tagName, role: element.getAttribute('role'), attr: element.getAttribute('data-tauri-drag-region'), text: element.textContent.trim() } }
    }
  }
  throw new Error(`QA ${number}: no visible direct hit for ${selector}`)
}

const dragCases = [
  [1, 'default', '.qa-testable', false],
  [2, 'bare-self', '.qa-testable', true], [2, 'bare-child', 'span', false],
  [3, 'deep-self', '.qa-testable', true], [3, 'deep-child', '.nested-content', true],
  [4, 'false-self', '.qa-testable', false], [4, 'false-child', 'span', false],
  [5, 'button-parent', '.qa-testable', true], [5, 'button-blocks', 'button', false],
  [6, 'input-parent', '.qa-testable', true], [6, 'input-blocks', 'input', false],
  [7, 'deep-text', '.nested-content', true], [7, 'deep-button-blocks', 'button', false], [7, 'deep-link-blocks', 'a', false],
  [8, 'plain-text', '.nested-content', true],
  [9, 'deep-parent', '.qa-testable', true], [9, 'false-child-blocks', '.nested-content', false],
  [10, 'doubleclick-region-drag', '.qa-testable', true],
  [11, 'role-button-blocks', '[role="button"]', false], [11, 'role-link-blocks', '[role="link"]', false],
  [11, 'role-checkbox-blocks', '[role="checkbox"]', false], [11, 'text-between-roles', '.nested-content > div:last-child', true],
  [12, 'explicit-button', 'button', true], [12, 'explicit-link', 'a', true],
  [13, 'explicit-deep-button', 'button', true], [13, 'explicit-deep-child', '.nested-content', true],
  [14, 'explicit-false-button', 'button', false],
  [15, 'explicit-role-button', '[role="button"]', true], [15, 'explicit-deep-role-child', '[role="tab"] span', true],
  [16, 'bare-role-parent', '.qa-testable', true], [16, 'role-tab-blocks', '[role="tab"]', false],
  [16, 'role-switch-blocks', '[role="switch"]', false], [16, 'role-option-blocks', '[role="option"]', false],
]

function dragListeners() {
  const listeners = getEventListeners(document)
  return { url: location.href, ready: document.readyState, isTauri: window.isTauri,
    metadata: window.__TAURI_INTERNALS__?.metadata ?? null,
    invoke: typeof window.__TAURI_INTERNALS__?.invoke, bridge: typeof window.ipc?.postMessage,
    listeners: Object.fromEntries(['mousedown', 'mouseup'].map((type) => [type, (listeners[type] ?? []).map((entry) => ({
      useCapture: entry.useCapture, passive: entry.passive, once: entry.once,
      scriptId: entry.scriptId, lineNumber: entry.lineNumber, columnNumber: entry.columnNumber,
      source: String(entry.listener),
    }))])) }
}

async function directToggleProbe(t, target) {
  const evidence = { command: 'plugin:window|internal_toggle_maximize', before: await nativeWindow(t) }
  const toggle = async () => {
    try { return { status: 'fulfilled', value: await t.evaluate(target, `window.__TAURI_INTERNALS__.invoke('plugin:window|internal_toggle_maximize')`) } }
    catch (error) { return { status: 'rejected', error: error.message } }
  }
  evidence.invoke = await toggle()
  await delay(150)
  evidence.after = await nativeWindow(t)
  if (json(rect(evidence.before)) !== json(rect(evidence.after))) {
    evidence.restore = await toggle()
    await delay(150)
    evidence.restored = await nativeWindow(t)
    evidence.restoredOriginalBounds = json(rect(evidence.before)) === json(rect(evidence.restored))
  }
  return evidence
}

export async function verifyDrag(t) {
  const target = t.main, results = [], checks = collecting(t), rendererEvents = []
  await t.wait(target, `document.querySelectorAll('.qa-test-case').length`, (n) => n === 16)
  t.check('drag source QA count', await t.evaluate(target, `document.querySelectorAll('.qa-test-case').length`), 16)
  const cdp = await observer(target, (event) => {
    if (event.method === 'Runtime.exceptionThrown' || event.method === 'Log.entryAdded'
      || (event.method === 'Runtime.consoleAPICalled' && ['error', 'warning'].includes(event.params.type))) rendererEvents.push(event)
  })
  try {
    await Promise.all([cdp.send('Runtime.enable'), cdp.send('Log.enable')])
    save(t, 'drag-initial-listeners', await cdp.send('Runtime.evaluate', {
      expression: `(${dragListeners})()`, includeCommandLineAPI: true, returnByValue: true,
    }))
    await nativeWindow(t)
    await t.native('place')
    await t.capture(target, 'drag-before')
    save(t, 'drag-before-native-state', await desktop(t, 'drag-before'))
    await t.evaluate(target, `(() => {
      const state = { expected: null, events: [] }
      state.listener = (event) => state.events.push({ type: event.type, trusted: event.isTrusted,
        matches: event.target === state.expected, detail: event.detail, x: event.clientX, y: event.clientY,
        tag: event.target.tagName, role: event.target.getAttribute?.('role'), attr: event.target.getAttribute?.('data-tauri-drag-region') })
      for (const type of ['mousedown', 'dblclick', 'focusin']) document.addEventListener(type, state.listener, { capture: true, passive: true })
      window.__verifyEmbeddedDrag = state
    })()`)
    for (const [number, name, selector, shouldMove] of dragCases) {
      await t.native('place')
      const point = await page(t, target, dragPoint, { number, selector })
      const label = `drag-qa${String(number).padStart(2, '0')}-${name}`
      const before = await nativeWindow(t)
      checks.check(`${label} CSS-to-physical client dimensions`,
        Math.abs(point.viewport.width * point.dpr - before.clientWidth) <= 2 * point.dpr
          && Math.abs(point.viewport.height * point.dpr - before.clientHeight) <= 2 * point.dpr, true)
      await t.capture(target, `${label}-before`)
      const motion = await t.native('drag', { X: Math.round(point.x * point.dpr), Y: Math.round(point.y * point.dpr), Dx: 60, Dy: 35 })
      assert(motion?.before && motion?.after, 'native drag must return before/after states')
      const events = await t.evaluate(target, 'window.__verifyEmbeddedDrag.events')
      const dx = motion.after.x - motion.before.x, dy = motion.after.y - motion.before.y
      const evidence = { number, name, selector, shouldMove, point, motion, dx, dy, events }
      results.push(evidence)
      save(t, `${label}-motion`, evidence)
      await t.capture(target, `${label}-after`)
      checks.check(`${label} receives real native mousedown on exact target`,
        events.some((e) => e.type === 'mousedown' && e.trusted && e.matches), true)
      checks.check(`${label} preserves window size`, [motion.after.width, motion.after.height], [motion.before.width, motion.before.height])
      checks.check(`${label} ${shouldMove ? 'moves native window by desktop drag' : 'does not move native window'}`,
        shouldMove ? Math.abs(dx - 60) <= 12 && Math.abs(dy - 35) <= 12 : dx === 0 && dy === 0, true)
      if (['bare-self', 'deep-child', 'button-blocks', 'input-blocks'].includes(name)) await desktop(t, label)
    }

    for (const [number, selector, name] of [[1, '.qa-testable', 'default'], [5, 'button', 'button'], [6, 'input', 'input']]) {
      await t.native('place')
      const point = await page(t, target, dragPoint, { number, selector })
      const motion = await t.native('doubleclick', { X: Math.round(point.x * point.dpr), Y: Math.round(point.y * point.dpr) })
      const events = await t.evaluate(target, 'window.__verifyEmbeddedDrag.events')
      save(t, `drag-doubleclick-${name}`, { point, motion, events })
      checks.check(`doubleclick ${name} reaches exact non-region target`, events.some((e) => e.type === 'mousedown' && e.trusted && e.matches && e.detail === 2), true)
      checks.check(`doubleclick ${name} does not maximize or move`, rect(motion.after), rect(motion.before))
      await t.capture(target, `drag-doubleclick-${name}-after`)
    }

    await t.native('place')
    let point = await page(t, target, dragPoint, { number: 10, selector: '.qa-testable' })
    await t.capture(target, 'drag-doubleclick-region-before')
    await desktop(t, 'drag-doubleclick-region-before')
    const maximized = await t.native('doubleclick', { X: Math.round(point.x * point.dpr), Y: Math.round(point.y * point.dpr) })
    const events = await t.evaluate(target, 'window.__verifyEmbeddedDrag.events')
    save(t, 'drag-doubleclick-maximized-state', { point, maximized, events })
    await t.capture(target, 'drag-doubleclick-maximized')
    await desktop(t, 'drag-doubleclick-maximized')
    checks.check('QA10 native doubleclick reaches bare region itself', events.some((e) => e.type === 'mousedown' && e.trusted && e.matches && e.detail === 2), true)
    checks.check('QA10 native doubleclick actually enlarges native client area',
      maximized.after.clientWidth > maximized.before.clientWidth + 40 && maximized.after.clientHeight > maximized.before.clientHeight + 40, true)
    point = await page(t, target, dragPoint, { number: 10, selector: '.qa-testable' })
    const restored = await t.native('doubleclick', { X: Math.round(point.x * point.dpr), Y: Math.round(point.y * point.dpr) })
    save(t, 'drag-doubleclick-restored-state', { point, restored })
    checks.check('QA10 second native doubleclick restores original dimensions',
      [restored.after.width, restored.after.height], [maximized.before.width, maximized.before.height])
    await t.capture(target, 'drag-doubleclick-restored')
    await desktop(t, 'drag-doubleclick-restored')
    save(t, 'drag-all-cases', results)
    checks.finish()
  } catch (error) {
    try { save(t, 'drag-direct-toggle', await directToggleProbe(t, target)) }
    catch (probeError) { save(t, 'drag-direct-toggle', { error: probeError.message }) }
    throw error
  } finally {
    try {
      await t.evaluate(target, `(() => {
        const state = window.__verifyEmbeddedDrag
        if (state) for (const type of ['mousedown', 'dblclick', 'focusin']) document.removeEventListener(type, state.listener, true)
        delete window.__verifyEmbeddedDrag
      })()`)
    } finally {
      try { save(t, 'drag-renderer-events', rendererEvents) }
      finally { cdp.close() }
    }
  }
}

// t.command opens a fresh socket per request. Isolation needs one retained CDP
// session to observe original console calls, iframe contexts and transport bytes.
async function observer(target, onEvent) {
  const socket = new WebSocket(target.webSocketDebuggerUrl), pending = new Map()
  let next = 1
  await new Promise((resolve, reject) => {
    const timer = setTimeout(() => { socket.close(); reject(new Error('isolation observer connection timed out')) }, 5000)
    socket.onopen = () => { clearTimeout(timer); resolve() }
    socket.onerror = () => { clearTimeout(timer); reject(new Error('isolation observer connection failed')) }
  })
  socket.onmessage = ({ data }) => {
    const message = JSON.parse(data)
    if (message.id) {
      const entry = pending.get(message.id)
      if (!entry) return
      clearTimeout(entry.timer); pending.delete(message.id)
      if (message.error) entry.reject(new Error(message.error.message))
      else entry.resolve(message.result)
    } else onEvent(message)
  }
  socket.onclose = () => {
    for (const entry of pending.values()) { clearTimeout(entry.timer); entry.reject(new Error('isolation observer disconnected')) }
    pending.clear()
  }
  return {
    send(method, params = {}, sessionId) {
      const id = next++
      return new Promise((resolve, reject) => {
        const timer = setTimeout(() => { pending.delete(id); reject(new Error(`${method} observation timed out`)) }, 15000)
        pending.set(id, { resolve, reject, timer })
        socket.send(json({ id, method, params, ...(sessionId ? { sessionId } : {}) }))
      })
    },
    close() { socket.close() },
  }
}

const isolationFrame = `(() => {
  const frame = document.querySelector('iframe#__tauri_isolation__')
  return { pattern: window.__TAURI_INTERNALS__?.__TAURI_PATTERN__,
    frame: frame && { src: frame.src, sandbox: [...frame.sandbox].sort(), id: frame.id } }
})()`

export async function verifyIsolation(t) {
  const main = t.main
  const initial = await t.wait(main, isolationFrame, (value) => Boolean(value.frame))
  t.check('isolation active pattern', initial.pattern?.pattern, 'isolation')
  t.check('isolation original iframe sandbox', initial.frame.sandbox, ['allow-scripts'])
  const hookFile = readFileSync(path.join(t.root, 'source/tauri/examples/isolation/isolation-dist/index.js'), 'utf8')
  const expectedHook = hookFile.match(/window\.__TAURI_ISOLATION_HOOK__\s*=\s*([\s\S]*)/)?.[1]
  assert(expectedHook, 'source isolation hook assignment is missing')
  const contexts = new Map(), consoles = [], forwarded = [], transport = [], problems = [], tasks = new Set()
  const bridgeBreakpoints = new Map(), nativeBreakpoints = new Set(), requests = new Map()
  let cdp
  const queue = (promise) => {
    const task = promise.catch((error) => { problems.push(error.message) }).finally(() => tasks.delete(task))
    tasks.add(task)
  }
  const drain = async () => { while (tasks.size) await Promise.all([...tasks]) }
  cdp = await observer(main, (event) => {
    const { method, params, sessionId } = event
    if (method === 'Target.attachedToTarget') {
      queue(Promise.all([cdp.send('Runtime.enable', {}, params.sessionId), cdp.send('Network.enable', {}, params.sessionId)]))
    } else if (method === 'Runtime.executionContextCreated') {
      const context = params.context
      contexts.set(`${sessionId ?? ''}:${context.id}`, { ...context, sessionId })
    } else if (method === 'Runtime.executionContextDestroyed') {
      contexts.delete(`${sessionId ?? ''}:${params.executionContextId}`)
    } else if (method === 'Runtime.consoleAPICalled' && params.args[0]?.value === 'hook') {
      consoles.push({ ...params, sessionId })
    } else if (method === 'Network.requestWillBeSent') {
      const url = new URL(params.request.url)
      if (url.protocol !== 'ipc:' && url.hostname !== 'ipc.localhost') return
      const entry = { channel: 'network', url: url.href, requestId: params.requestId }
      requests.set(`${sessionId ?? ''}:${params.requestId}`, entry)
      queue((async () => {
        const raw = params.request.postData ?? (params.request.hasPostData
          ? (await cdp.send('Network.getRequestPostData', { requestId: params.requestId }, sessionId)).postData : '')
        const value = packet(raw, decodeURIComponent(url.pathname.slice(1)))
        if (value) { entry.value = value; transport.push(entry) }
      })())
    } else if (method === 'Network.responseReceived') {
      const entry = requests.get(`${sessionId ?? ''}:${params.requestId}`)
      if (entry) entry.status = params.response.status
    } else if (method === 'Debugger.paused') {
      queue((async () => {
        try {
          const native = params.hitBreakpoints?.some((id) => nativeBreakpoints.has(id))
          const result = await cdp.send('Debugger.evaluateOnCallFrame', {
            callFrameId: params.callFrames[0].callFrameId,
            expression: native ? `({ raw: options.body, url, status: reply.status })` : `(() => {
              const raw = ${params.hitBreakpoints?.map((id) => bridgeBreakpoints.get(id)).find(Boolean) ?? 'arguments[0]'}
              const value = typeof raw === 'string' ? JSON.parse(raw) : raw
              return { rawString: typeof raw === 'string', value: value && { cmd: value.cmd, callback: value.callback, error: value.error, payload: value.payload } }
            })()`, returnByValue: true,
          }, sessionId)
          if (result.exceptionDetails) throw new Error(result.exceptionDetails.text)
          const snapshot = result.result?.value
          if (native) {
            const value = packet(snapshot?.raw, decodeURIComponent(new URL(snapshot.url).pathname.slice(1)))
            if (value) transport.push({ channel: 'native-protocol', url: snapshot.url, status: snapshot.status, value })
          } else {
            const value = packet(snapshot?.value)
            if (value) (snapshot.rawString ? transport : forwarded).push({ channel: snapshot.rawString ? 'ipc.postMessage' : 'internals.postMessage', value })
          }
        } finally { await cdp.send('Debugger.resume', {}, sessionId) }
      })())
    }
  })
  const evalContext = async (context, expression) => {
    const value = await cdp.send('Runtime.evaluate', { expression, contextId: context.id, returnByValue: true }, context.sessionId)
    if (value.exceptionDetails) throw new Error(value.exceptionDetails.text)
    return value.result?.value
  }
  const hookExpression = `({ url: location.href, title: document.title, source: typeof window.__TAURI_ISOLATION_HOOK__ === 'function' ? window.__TAURI_ISOLATION_HOOK__.toString() : null })`
  let secure
  const runs = []
  try {
    await cdp.send('Target.setAutoAttach', { autoAttach: true, waitForDebuggerOnStart: false, flatten: true })
    await Promise.all([cdp.send('Runtime.enable'), cdp.send('Network.enable'), cdp.send('Debugger.enable')])
    await drain()
    const deadline = Date.now() + 20000
    while (!secure && Date.now() < deadline) {
      for (const context of contexts.values()) {
        if (!context.auxData?.isDefault) continue
        const value = await evalContext(context, hookExpression)
        if (value?.url === initial.frame.src && value.source) { secure = { context, ...value }; break }
      }
      if (!secure) await delay(100)
    }
    assert(secure, 'original hook was not found in the loaded isolation iframe context; sandbox was not changed')
    t.check('isolation iframe runs original source hook', compact(secure.source), compact(expectedHook))
    save(t, 'isolation-original', { initial, hook: { url: secure.url, title: secure.title, source: secure.source } })

    // Observe the original functions. Never replace the hook, bridge or iframe.
    const fn = await cdp.send('Runtime.evaluate', { expression: 'window.__TAURI_INTERNALS__.postMessage', objectGroup: 'verify-embedded' })
    assert(fn.result?.objectId, 'original postMessage is not an observable function')
    await cdp.send('Debugger.setBreakpointOnFunctionCall', { objectId: fn.result.objectId })
    try {
      // contextBridge wrappers can be native. Break at the existing JS bridge
      // call site instead, reading only its already serialized argument.
      const properties = await cdp.send('Runtime.getProperties', { objectId: fn.result.objectId })
      const location = properties.internalProperties?.find((p) => p.name === '[[FunctionLocation]]')?.value?.value
      assert(location?.scriptId, 'original IPC function location is unavailable')
      const { scriptSource } = await cdp.send('Debugger.getScriptSource', { scriptId: location.scriptId })
      try {
        // The native invoke has settled here; options.body still holds the original ciphertext.
        const breakpoint = await cdp.send('Debugger.setBreakpoint', { location: nativeFetchCallSite(scriptSource, location.scriptId) })
        nativeBreakpoints.add(breakpoint.breakpointId)
      } catch (error) { problems.push(`original native IPC response: ${error.message}`) }
      const call = bridgeCallSite(scriptSource, location)
      const breakpoint = await cdp.send('Debugger.setBreakpoint', { location: call.location })
      bridgeBreakpoints.set(breakpoint.breakpointId, call.variable)
    } catch (error) {
      // A successful Network/native response must supply transport evidence if
      // this Chromium cannot expose the original bridge call location.
      problems.push(`original IPC call site: ${error.message}`)
    }
    await t.evaluate(main, `(() => {
      const state = { replies: [] }
      state.listener = (event) => {
        if (event.source !== document.querySelector('iframe#__tauri_isolation__')?.contentWindow || !event.data || typeof event.data !== 'object') return
        const value = event.data
        state.replies.push({ cmd: value.cmd, callback: value.callback, error: value.error, payload: value.payload })
      }
      window.addEventListener('message', state.listener)
      window.__verifyEmbeddedIsolation = state
    })()`)
    t.check('isolation ping initial UI', await t.evaluate(main, `document.querySelector('#pong').textContent`), '')
    await t.capture(main, 'isolation-before')

    for (let index = 0; index < 2; index++) {
      const logStart = (await t.output()).length, consoleStart = consoles.length
      const old = await t.evaluate(main, `document.querySelector('#pong').textContent`)
      await t.click(main, '#ping')
      const text = await t.wait(main, `document.querySelector('#pong').textContent`, (value) => value !== old && /^(ok|error):\s*\d+$/.test(value))
      await drain()
      const hookEvents = consoles.slice(consoleStart).filter((e) => e.executionContextId === secure.context.id && e.sessionId === secure.context.sessionId)
      const hooks = []
      for (const event of hookEvents) {
        const argument = event.args[1]
        if (!argument?.objectId) continue
        const response = await cdp.send('Runtime.callFunctionOn', {
          objectId: argument.objectId, returnByValue: true,
          functionDeclaration: 'function () { return { cmd: this.cmd, callback: this.callback, error: this.error, payload: this.payload } }',
        }, event.sessionId)
        if (response.exceptionDetails) throw new Error(response.exceptionDetails.text)
        hooks.push(response.result.value)
      }
      const hook = hooks.find((value) => value.cmd === 'ping')
      const replies = await t.evaluate(main, 'window.__verifyEmbeddedIsolation.replies')
      const reply = hook && replies.find((value) => value.cmd === 'ping' && value.callback === hook.callback && value.error === hook.error)
      const wire = reply && transport.find((entry) => entry.value.cmd === 'ping' && json(entry.value.payload) === json(reply.payload)
        && (!['network', 'native-protocol'].includes(entry.channel) || entry.status >= 200 && entry.status < 300))
      const forward = reply && forwarded.find((entry) => entry.value.cmd === 'ping' && entry.value.callback === reply.callback && json(entry.value.payload) === json(reply.payload))
      const run = { index, text, hooks, reply, wire, forward, rust: (await t.output()).slice(logStart) }
      runs.push(run)
      save(t, `isolation-ping-${index + 1}-route`, run)
      await t.capture(main, `isolation-ping-${index + 1}-after`)
      t.check(`isolation ping ${index + 1} UI succeeds`, /^ok:\s*\d+$/.test(text), true)
      t.check(`isolation ping ${index + 1} executes Rust handler`, /\bping:/.test(run.rust), true)
      t.check(`isolation ping ${index + 1} calls untouched iframe hook`, Boolean(hook), true)
      t.check(`isolation ping ${index + 1} original hook payload is unchanged`, hook?.payload, {})
      t.check(`isolation ping ${index + 1} iframe returns encrypted bytes`, Boolean(encrypted(reply?.payload)), true)
      t.check(`isolation ping ${index + 1} AES-GCM ciphertext includes authentication tag`, reply?.payload.payload.length, Buffer.byteLength(json(hook.payload)) + 16)
      t.check(`isolation ping ${index + 1} encrypted reply reaches original IPC function`, Boolean(forward), true)
      t.check(`isolation ping ${index + 1} actual transport contains the same ciphertext`, Boolean(wire), true)
    }
    t.check('isolation repeated payload uses fresh nonce', json(runs[0].reply.payload.nonce) !== json(runs[1].reply.payload.nonce), true)
    t.check('isolation repeated payload produces different ciphertext', json(runs[0].reply.payload.payload) !== json(runs[1].reply.payload.payload), true)
    t.check('isolation sandbox and active pattern survive both invokes', await t.evaluate(main, isolationFrame), initial)
    t.check('isolation original hook remains unchanged', (await evalContext(secure.context, hookExpression)).source, secure.source)
    save(t, 'isolation-route', { runs, instrumentation: problems })
    await t.capture(main, 'isolation-after')
  } finally {
    try {
      await t.evaluate(main, `(() => {
        if (window.__verifyEmbeddedIsolation) window.removeEventListener('message', window.__verifyEmbeddedIsolation.listener)
        delete window.__verifyEmbeddedIsolation
      })()`)
    } finally { cdp.close() }
    save(t, 'isolation-observation', { runs, instrumentation: problems })
  }
}

// The host stages about:blank until trusted IPC routing is installed. The
// runtime's tauriSource records configured navigation; getURL is the actual doc.
const guestSources = [
  ['main1', 'tauri://localhost/index.html'],
  ['main2', 'https://github.com/tauri-apps/tauri'],
  ['main3', 'https://tauri.app/'],
  ['main4', 'https://twitter.com/TauriApps'],
]

function hostState() {
  return { width: innerWidth, height: innerHeight, dpr: devicePixelRatio,
    guests: [...document.querySelectorAll('webview')].map((element) => {
      const r = element.getBoundingClientRect()
      return { id: element.id, src: element.dataset.tauriSource, srcAttribute: element.getAttribute('src'), url: element.getURL(), contentsId: element.getWebContentsId(), loading: element.isLoading(),
        bounds: { x: r.x, y: r.y, width: r.width, height: r.height } }
    }) }
}

function documentState() {
  return { url: location.href, title: document.title, ready: document.readyState,
    label: window.__TAURI_INTERNALS__?.metadata?.currentWebview?.label ?? null,
    windowLabel: window.__TAURI_INTERNALS__?.metadata?.currentWindow?.label ?? null,
    text: document.body?.innerText?.slice(0, 16000) ?? '',
    headings: [...document.querySelectorAll('h1')].map((e) => e.textContent.trim()),
    width: innerWidth, height: innerHeight, dpr: devicePixelRatio }
}

function externalStatus(label, doc) {
  const text = `${doc.title}\n${doc.text}`
  if (doc.url.startsWith('chrome-error:') || /ERR_[A-Z_]+|site can.t be reached|no internet|check your internet|connection timed out/i.test(text)) return 'network-gap'
  if (/sign in to (?:github|x|twitter)|log in to (?:x|twitter)|javascript is not available|something went wrong|verify you are human|checking your browser|access denied|too many requests|rate limit|don.t miss what.s happening/i.test(text)) return 'login-or-network-wall'
  let url
  try { url = new URL(doc.url) } catch { return 'unverified-document' }
  if (doc.ready !== 'complete' || doc.text.trim().length < 200) return 'unverified-document'
  if (label === 'main2' && url.hostname === 'github.com' && url.pathname.replace(/\/$/, '') === '/tauri-apps/tauri'
    && /tauri-apps\/tauri/i.test(doc.title) && /tauri-apps/i.test(doc.text) && /README|Code|Releases/i.test(doc.text)) return 'repository-content'
  if (label === 'main3' && url.hostname === 'tauri.app' && /tauri/i.test(doc.title) && doc.headings.some((h) => h.length > 0) && /tauri/i.test(doc.text)) return 'tauri-site-content'
  if (label === 'main4' && ['twitter.com', 'x.com'].includes(url.hostname) && /^\/TauriApps\/?$/i.test(url.pathname)
    && /@TauriApps/i.test(doc.text) && /tauri/i.test(doc.title)) return 'profile-content'
  return 'unverified-document'
}

export async function verifyMultiwebview(t) {
  const checks = collecting(t)
  const host = await t.target(async (target) => {
    if (!target.webSocketDebuggerUrl || target.url.startsWith('devtools:')) return false
    try { return await t.evaluate(target, `document.querySelectorAll('webview').length === 4`) }
    catch { return false }
  })
  const expression = `(${hostState})()`
  await t.wait(host, expression, (value) => value.guests.length === 4 && value.guests.every((g) => g.contentsId > 0 && g.bounds.width > 0 && g.bounds.height > 0))
  try {
    await t.wait(host, expression, (value) => value.guests.every((g) => !g.loading), 45000)
  } catch (error) {
    t.note('multiwebview external loading did not settle', error.message)
  }
  const initial = await page(t, host, hostState)
  const native = await nativeWindow(t)
  checks.check('multiwebview source logical client size', [initial.width, initial.height], [800, 600])
  checks.check('multiwebview native physical client size matches CSS and DPR',
    [native.clientWidth, native.clientHeight], [Math.round(initial.width * initial.dpr), Math.round(initial.height * initial.dpr)])
  const candidates = []
  for (const target of await t.targets()) {
    if (!target.webSocketDebuggerUrl || target.id === host.id || target.url.startsWith('devtools:')) continue
    try { candidates.push({ target, doc: await page(t, target, documentState) }) }
    catch (error) { candidates.push({ target, error: error.message }) }
  }
  const guests = []
  for (const [label, source] of guestSources) {
    const element = initial.guests.find((g) => new URL(g.src).href === source)
    checks.check(`multiwebview ${label} configured source is preserved`, Boolean(element), true)
    if (!element) continue
    const candidate = candidates.find((c) => c.doc?.label === label)
      ?? candidates.find((c) => [element.src, element.url].includes(c.target.url) || [element.src, element.url].includes(c.doc?.url))
    checks.check(`multiwebview ${label} has a real guest CDP target`, Boolean(candidate?.doc), true)
    if (!candidate?.doc) continue
    checks.check(`multiwebview ${label} actual injected label`, candidate.doc.label, label)
    checks.check(`multiwebview ${label} belongs to native main window`, candidate.doc.windowLabel, 'main')
    if (label === 'main1') {
      checks.check('multiwebview main1 real local document', candidate.doc.headings, ['Welcome to Tauri!'])
    } else {
      checks.check(`multiwebview ${label} external URL has no appended entry file`,
        !new URL(element.src).pathname.endsWith('/index.html') && !new URL(candidate.doc.url).pathname.endsWith('/index.html'), true)
      const status = externalStatus(label, candidate.doc)
      if (status.endsWith('-content')) checks.check(`multiwebview ${label} real document content`, status,
        { main2: 'repository-content', main3: 'tauri-site-content', main4: 'profile-content' }[label])
      else t.note(`multiwebview ${label} external content`, `${status}: ${json({ url: candidate.doc.url, title: candidate.doc.title, ready: candidate.doc.ready, text: candidate.doc.text.slice(0, 1200) })}`)
    }
    guests.push({ label, element, ...candidate })
    await t.capture(candidate.target, `multiwebview-${label}-initial`)
  }
  checks.check('multiwebview four distinct guest targets', new Set(guests.map((g) => g.target.id)).size, 4)
  checks.check('multiwebview four distinct native guest contents', new Set(initial.guests.map((g) => g.contentsId)).size, 4)

  const verifyBounds = async (state, phase) => {
    const expected = quadrants(state.width, state.height)
    for (const [index, [label, source]] of guestSources.entries()) {
      const guest = state.guests.find((g) => new URL(g.src).href === source)
      checks.check(`multiwebview ${phase} ${label} exact CSS quadrant`, guest?.bounds, expected[index])
      const target = guests.find((g) => g.label === label)?.target
      if (!target) continue
      const size = await t.wait(target, '({ width: innerWidth, height: innerHeight })',
        (value) => value.width === expected[index].width && value.height === expected[index].height, 5000)
        .catch(() => t.evaluate(target, '({ width: innerWidth, height: innerHeight })'))
      checks.check(`multiwebview ${phase} ${label} real guest viewport follows bounds`, size,
        { width: expected[index].width, height: expected[index].height })
    }
  }
  save(t, 'multiwebview-layout-initial', { host: initial, native, guests: guests.map(({ label, element, target, doc }) => ({ label, element, targetId: target.id, doc })) })
  await t.capture(host, 'multiwebview-composite-initial')
  await desktop(t, 'multiwebview-composite-initial')
  await verifyBounds(initial, 'initial')

  // The source has no resize control. Resize its own desktop window, then let
  // the source's auto_resize behavior update the real guests without DOM edits.
  const width = Math.round(640 * initial.dpr), height = Math.round(480 * initial.dpr)
  const resize = await t.native('resize', { Width: width, Height: height })
  assert(resize?.before && resize?.after, 'native resize must return before/after states')
  checks.check('multiwebview native resize changes physical client dimensions',
    [resize.after.clientWidth, resize.after.clientHeight], [width, height])
  const resized = await t.wait(host, expression, (value) => value.width === 640 && value.height === 480)
  await verifyBounds(resized, 'resized')
  save(t, 'multiwebview-layout-resized', { host: resized, resize })
  await t.capture(host, 'multiwebview-composite-resized')
  await desktop(t, 'multiwebview-composite-resized')
  for (const guest of guests) {
    save(t, `multiwebview-${guest.label}-resized-document`, await page(t, guest.target, documentState))
    await t.capture(guest.target, `multiwebview-${guest.label}-resized`)
  }
  checks.finish()
}

if (process.argv.includes('--self-check') && process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  assert.deepEqual(quadrants(800, 600), [
    { x: 0, y: 0, width: 400, height: 300 }, { x: 400, y: 0, width: 400, height: 300 },
    { x: 0, y: 300, width: 400, height: 300 }, { x: 400, y: 300, width: 400, height: 300 },
  ])
  assert.deepEqual(quadrants(641, 481)[3], { x: 320.5, y: 240.5, width: 320.5, height: 240.5 })
  const payload = { nonce: Array(12).fill(1), payload: Array(18).fill(2), contentType: 'application/json' }
  assert(encrypted(payload))
  assert(!encrypted({ ...payload, nonce: [1] }))
  assert(!encrypted({ ...payload, payload: [256] }))
  assert.deepEqual(packet(json({ cmd: 'ping', payload, __TAURI_INVOKE_KEY__: 'excluded' })), { cmd: 'ping', callback: undefined, error: undefined, payload })
  assert.equal(packet('not JSON'), null)
  assert.equal(packet(json({ cmd: 'ping', payload: {} })), null)
  assert.equal(packet(json(payload), 'ping').cmd, 'ping')
  assert.deepEqual(bridgeCallSite('const ignored = 0;\nfunction send() {\n  window.ipc.postMessage(data)\n}', { scriptId: '7', lineNumber: 1, columnNumber: 0 }),
    { variable: 'data', location: { scriptId: '7', lineNumber: 2, columnNumber: 2 } })
  assert.throws(() => bridgeCallSite('function send() {}', { scriptId: '7', lineNumber: 0, columnNumber: 0 }))
  const nativeFetch = readFileSync(new URL('../src/runtime.rs', import.meta.url), 'utf8').replaceAll('\r\n', '\n').match(/const NATIVE_IPC_FETCH: &str = r#"([^]*?)"#;/)?.[1]
  assert(nativeFetch, 'missing runtime native IPC fetch helper')
  const nativeSite = nativeFetchCallSite(nativeFetch, '8')
  assert.equal(nativeSite.scriptId, '8')
  assert.equal(nativeFetch.split('\n')[nativeSite.lineNumber].slice(nativeSite.columnNumber), "const bytes = Uint8Array.from(atob(reply.body ?? ''), c => c.charCodeAt(0))")
  assert.deepEqual(nativeFetchCallSite(nativeFetch.replaceAll('\n', '\r\n'), '8'), nativeSite)
  assert.throws(() => nativeFetchCallSite('function send() {}', '8'))
  assert.throws(() => nativeFetchCallSite(`${nativeFetch}\n${nativeFetch}`, '8'))
  assert.equal(compact('(payload) => { return payload };'), '(payload)=>{returnpayload}')
  assert.deepEqual([...new Set(dragCases.map(([number]) => number))], Array.from({ length: 16 }, (_, i) => i + 1))
  for (const outcome of ['changed', 'unchanged', 'rejected']) {
    const original = { x: 120, y: 80, width: 816, height: 639, clientWidth: 800, clientHeight: 600, visible: true }
    let maximized = false, calls = 0
    const probe = await directToggleProbe({ name: 'drag',
      async native(action) {
        assert.equal(action, 'state')
        return [maximized ? { ...original, x: 0, y: 0, width: 1920, height: 1080 } : original]
      },
      async evaluate(_target, expression) {
        assert.equal(expression, `window.__TAURI_INTERNALS__.invoke('plugin:window|internal_toggle_maximize')`)
        calls++
        if (outcome === 'rejected') throw new Error('invoke denied')
        if (outcome === 'changed') maximized = !maximized
        return null
      },
    }, {})
    assert.equal(probe.invoke.status, outcome === 'rejected' ? 'rejected' : 'fulfilled')
    assert.equal(calls, outcome === 'changed' ? 2 : 1)
    assert.equal(maximized, false, 'diagnostic must restore the original window')
    if (outcome === 'changed') assert.equal(probe.restoredOriginalBounds, true)
    else assert.equal(probe.restore, undefined)
  }
  assert.equal(externalStatus('main2', { url: 'chrome-error://chromewebdata/', title: '', text: 'ERR_NAME_NOT_RESOLVED' }), 'network-gap')
  assert.equal(externalStatus('main4', { url: 'https://x.com/i/flow/login', title: 'Log in to X', text: 'Log in to X' }), 'login-or-network-wall')
  assert.equal(externalStatus('main3', { url: 'https://tauri.app/', title: 'Tauri', text: 'Tauri '.repeat(40), ready: 'complete', headings: ['Tauri'] }), 'tauri-site-content')
  console.log('Embedded verifier self-check passed (no apps launched; no UI verification performed)')
}
