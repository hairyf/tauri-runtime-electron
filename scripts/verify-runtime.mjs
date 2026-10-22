// Offline helper regression checks. Event/DOM models below do not prove Electron UI behavior.
// Run: node scripts/verify-runtime.mjs
import assert from 'node:assert/strict'
import { EventEmitter } from 'node:events'
import { readFileSync } from 'node:fs'
import path from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'
import vm from 'node:vm'

const source = readFileSync(new URL('../electron/main.cjs', import.meta.url), 'utf8')
const root = fileURLToPath(new URL('..', import.meta.url))
const preloadPath = path.join(root, '.temp', 'preload # % sample.cjs')
const timers = new Set()
const sandbox = {
  URL, pathToFileURL, process: { env: {} }, readyDocuments: new WeakSet(),
  webviews: new Map(), webviewByContents: new Map(), guestContentsIds: new Map(),
  handlerPolicy: new Map(), innerWidth: 800, innerHeight: 600,
  screen: { getDisplayMatching: () => ({ scaleFactor: 2 }) },
  writeInitPreload: () => preloadPath, log() {}, send() {},
  setTimeout(callback, delay) {
    const timer = setTimeout(callback, delay)
    timers.add(timer)
    return timer
  },
  clearTimeout(timer) { clearTimeout(timer); timers.delete(timer) },
}
vm.createContext(sandbox)
for (const name of [
  'trackDocument', 'pageReady', 'placeGuest', 'guestBounds', 'scaleOf', 'toDip',
  'webviewUrl', 'attachWebviewElement', 'recordPolicy', 'policyFor', 'wireWebviewEvents',
  'readWebviewBounds', 'findWebview',
]) {
  const declaration = source.match(new RegExp(`^(?:async )?function ${name}\\([^]*?^\\}`, 'm'))?.[0]
  assert.ok(declaration, `missing runtime helper: ${name}`)
  vm.runInContext(declaration, sandbox, { filename: `electron/main.cjs:${name}` })
}
const plain = (value) => JSON.parse(JSON.stringify(value))
async function bounded(promise) {
  let timer
  try {
    return await Promise.race([
      promise,
      new Promise((_, reject) => { timer = setTimeout(() => reject(new Error('helper did not settle')), 200) }),
    ])
  } finally { clearTimeout(timer) }
}
function host(overrides = {}) {
  const state = { url: 'about:blank', loading: false, destroyed: false, ...overrides }
  const contents = Object.assign(new EventEmitter(), {
    getURL: () => state.url,
    isLoading: () => state.loading,
    isLoadingMainFrame: () => state.loading,
    isDestroyed: () => state.destroyed,
    executeJavaScript() { throw new Error('use the ready main frame, not the WebContents loading gate') },
  })
  const window = {
    webContents: contents, isDestroyed: () => state.destroyed,
    getContentSize: () => [800, 600],
    getContentBounds: () => ({ x: 80, y: 60, width: 800, height: 600 }),
    getBounds: () => ({ x: 80, y: 60, width: 800, height: 600 }),
  }
  sandbox.trackDocument(contents)
  return { state, contents, window }
}

try {
  // A committed DOM stays usable while images/frames are still loading.
  const ready = host({ loading: true })
  ready.contents.emit('dom-ready')
  await bounded(sandbox.pageReady(ready.window))
  assert.equal(ready.contents.isLoading(), true)
  for (const details of [
    { isMainFrame: false, isSameDocument: false },
    { isMainFrame: true, isSameDocument: true },
  ]) {
    ready.contents.emit('did-start-navigation', details)
    await bounded(sandbox.pageReady(ready.window))
  }
  ready.contents.emit('did-start-navigation', { isMainFrame: true, isSameDocument: false })
  let settled = false
  const navigating = sandbox.pageReady(ready.window).then(() => { settled = true })
  await Promise.resolve()
  assert.equal(settled, false, 'new main-frame navigation must invalidate the old DOM')
  ready.contents.emit('dom-ready')
  await bounded(navigating)

  // !isLoading() alone cannot establish readiness before the first navigation.
  const fresh = host({ url: '' })
  settled = false
  const firstDocument = sandbox.pageReady(fresh.window).then(() => { settled = true })
  await Promise.resolve()
  assert.equal(settled, false, 'a never-navigated host has no committed document')
  fresh.state.url = 'about:blank'
  fresh.contents.emit('dom-ready')
  await bounded(firstDocument)

  const dead = host({ destroyed: true })
  await assert.rejects(bounded(Promise.resolve().then(() => sandbox.pageReady(dead.window))), /destroy/i)
  const loading = host({ url: '', loading: true })
  const destroyed = assert.rejects(bounded(sandbox.pageReady(loading.window)), /destroy/i)
  loading.state.destroyed = true
  loading.contents.emit('destroyed')
  await destroyed
  const failing = host({ url: '', loading: true })
  const failed = assert.rejects(bounded(sandbox.pageReady(failing.window)), /ERR_NAME_NOT_RESOLVED|fail/i)
  failing.contents.emit('did-fail-load', {}, -105, 'ERR_NAME_NOT_RESOLVED', 'tauri://localhost/', true)
  await failed
  assert.equal(timers.size, 0, 'readiness must clean up its timers')

  // Run the real attachment helper and its generated renderer code, with no guest load completion.
  let navigated
  const guest = Object.assign(new EventEmitter(), {
    id: 901, isDestroyed: () => false, setWindowOpenHandler() {},
    loadURL(url) {
      assert.equal(sandbox.guestContentsIds.get(7), this.id, 'map before guest navigation')
      assert.equal(sandbox.webviewByContents.get(this.id), 7)
      assert.equal(this.listenerCount('did-start-loading'), 1, 'wire lifecycle before navigation')
      navigated = url
      return new Promise(() => {})
    },
  })
  let element
  ready.contents.mainFrame = {
    async executeJavaScript(code) {
      return vm.runInNewContext(code, {
        innerWidth: 800, innerHeight: 600,
        document: {
          createElement() {
            return { style: {}, dataset: {}, attributes: {}, setAttribute(name, value) { this.attributes[name] = value } }
          },
          getElementById(id) { assert.equal(id, element.id); return element },
          body: { appendChild(value) { element = value; queueMicrotask(() => ready.contents.emit('did-attach-webview', {}, guest)) } },
        },
      })
    },
  }
  const bounds = { position: { x: 400, y: 300, physical: true }, size: { width: 800, height: 600, physical: true } }
  assert.deepEqual(plain(sandbox.guestBounds(ready.window, bounds)), { x: 200, y: 150, width: 400, height: 300 })
  const logical = { position: { x: 200, y: 150, physical: false }, size: { width: 400, height: 300, physical: false } }
  assert.deepEqual(plain(sandbox.guestBounds(ready.window, logical)), { x: 200, y: 150, width: 400, height: 300 })
  assert.deepEqual(plain(sandbox.guestBounds(ready.window, null)), { x: 0, y: 0, width: 800, height: 600 })
  assert.deepEqual(plain(await bounded(sandbox.attachWebviewElement(ready.window, {
    webviewId: 7, url: 'tauri://localhost/', bounds, autoResize: true,
    policy: { permission: true, download: true },
  }))), { webviewId: 7 })
  assert.equal(new URL(element.attributes.preload, 'about:blank').protocol, 'file:')
  assert.equal(fileURLToPath(element.attributes.preload), preloadPath)
  assert.match(element.attributes.preload, /%23/)
  assert.match(element.attributes.preload, /%25/)
  assert.equal(element.src, 'about:blank')
  assert.equal(navigated, 'tauri://localhost/index.html')
  assert.equal(sandbox.guestContentsIds.get(7), guest.id)
  assert.equal(sandbox.webviewByContents.get(guest.id), 7)
  assert.deepEqual(plain(sandbox.policyFor(guest)), { permission: true, download: true })
  assert.equal(element.style.left, '25%')
  assert.equal(element.style.top, '25%')
  assert.equal(element.style.width, '50%')
  assert.equal(element.style.height, '50%')
  element.getBoundingClientRect = () => ({ x: 200, y: 150, width: 400, height: 300 })
  const measured = await bounded(sandbox.readWebviewBounds(7))
  assert.deepEqual(plain(measured.bounds), { x: 400, y: 300, width: 800, height: 600 })
  assert.deepEqual(plain(measured.contentBounds), { x: 0, y: 0, width: 1600, height: 1200 })
  sandbox.placeGuest(element, null, false)
  assert.equal(element.style.left, '200px')
  assert.equal(element.style.width, '400px')
  sandbox.placeGuest(element, { x: 10, y: 20, width: 100, height: 80 })
  assert.equal(element.style.left, '10px')
  assert.equal(element.style.height, '80px')
  sandbox.placeGuest(element, null, true)
  assert.equal(element.style.left, '25%')
  assert.equal(element.dataset.tauriAutoResize, 'true')
  assert.equal(sandbox.webviewUrl('tauri://localhost/?q=1#anchor'), 'tauri://localhost/index.html?q=1#anchor')
  for (const url of ['isolation://localhost/', 'https://example.com/', 'about:blank', 'tauri://localhost/child.html']) {
    assert.equal(sandbox.webviewUrl(url), url)
  }
  assert.equal(timers.size, 0, 'attachment must clean up its timer')

  // Execute the real native IPC callback: only Electron's sender/frame chooses identity.
  const sent = []
  let onIpc
  sandbox.ipcMain = { on(channel, callback) { assert.equal(channel, 'tauri-ipc'); onIpc = callback } }
  sandbox.send = (message) => sent.push(plain(message))
  const registration = source.match(/^ipcMain\.on\('tauri-ipc', \([^]*?^\}\)/m)?.[0]
  assert.ok(registration, 'missing native IPC registration')
  vm.runInContext(registration, sandbox)
  const sibling = { id: 902, mainFrame: { url: 'https://sibling.example/page' }, getURL() { assert.fail('use senderFrame.url') } }
  sandbox.webviewByContents.set(sibling.id, 8)
  const body = JSON.stringify({ webviewId: 7, url: 'tauri://localhost/', payload: { value: 42 } })
  onIpc({ sender: sibling, senderFrame: sibling.mainFrame }, body)
  assert.deepEqual(sent, [{ event: 'ipc.message', params: { webviewId: 8, url: sibling.mainFrame.url, body } }])
  for (const [sender, senderFrame, data] of [
    [{ ...sibling, id: 999 }, sibling.mainFrame, body],
    [sibling, { url: sibling.mainFrame.url }, body], [sibling, null, body],
    [sibling, sibling.mainFrame, {}],
  ]) onIpc({ sender, senderFrame }, data)
  assert.equal(sent.length, 1, 'reject unknown senders, subframes, absent frames and non-string bodies')

  // Exercise the real authenticated protocol validator; a renderer cannot choose its identity.
  Object.assign(sandbox, { Headers, Buffer, Uint8Array })
  vm.runInContext(source.match(/^function nativeProtocolRequest\([^]*?^\}/m)[0], sandbox)
  const request = { url: 'ipc://localhost/echo', method: 'POST', headers: [['origin', 'tauri://localhost'], ['x-channel-id', '17']], body: new Uint8Array([4, 5]) }
  const validated = plain(sandbox.nativeProtocolRequest({ sender: sibling, senderFrame: sibling.mainFrame }, request))
  assert.equal(validated.webviewId, 8)
  assert.equal(validated.pageUrl, sibling.mainFrame.url)
  assert.equal(validated.headers.origin, 'https://sibling.example')
  assert.equal(validated.headers['x-channel-id'], '17')
  assert.equal(validated.body, 'BAU=')
  assert.equal(validated.uri, 'ipc://localhost/echo')
  for (const url of ['https://ipc.localhost/echo', 'ipc://other/echo', 'http://ipc.localhost:123/echo', 'ipc://user@localhost/echo']) {
    assert.throws(() => sandbox.nativeProtocolRequest({ sender: sibling, senderFrame: sibling.mainFrame }, { ...request, url }))
  }
  for (const event of [{ sender: sibling, senderFrame: {} }, { sender: { id: 999 }, senderFrame: sibling.mainFrame }]) {
    assert.throws(() => sandbox.nativeProtocolRequest(event, request))
  }
  assert.throws(() => sandbox.nativeProtocolRequest({ sender: sibling, senderFrame: sibling.mainFrame }, { ...request, method: 'GET' }))

  // Run the real upstream template with the Rust-selected local fetch. MIME must
  // distinguish Raw bytes from valid JSON arrays without changing global fetch.
  const rustSource = readFileSync(new URL('../src/runtime.rs', import.meta.url), 'utf8')
  const rust = rustSource.match(/^fn native_ipc_script\([^]*?^\}/m)?.[0]
  const marker = JSON.parse(rust.match(/let marker = ("[^"]+");/)[1])
  const replacement = rustSource.match(/const NATIVE_IPC_FETCH: &str = r#"([^]*?)"#;/)[1]
  const signatures = [...rust.matchAll(/script\.contains\(("[^"]+")\)/g)].map((match) => JSON.parse(match[1]))
  const upstream = readFileSync(new URL('../source/tauri/crates/tauri/scripts/ipc-protocol.js', import.meta.url), 'utf8')
  for (const text of [marker, ...signatures]) assert.equal(upstream.split(text).length - 1, 1, `upstream IPC signature changed: ${text}`)
  const native = upstream.replace(marker, replacement)
  assert.equal(native.replace(replacement, marker), upstream, 'preserve invoke-key, encryption and fallback code')
  const requests = [], callbacks = []
  let reply
  const window = { __TAURI_INTERNALS__: {
    convertFileSrc: (cmd) => `ipc://localhost/${encodeURIComponent(cmd)}`,
    runCallback: (id, value) => callbacks.push({ id, value }),
  }, ipc: { postMessage() { assert.fail('typed requests must not use lossy fallback') }, async fetch(value) { requests.push(value); return reply } } }
  const context = vm.createContext({
    window, Headers, Response, Uint8Array, atob, ArrayBuffer,
    __TEMPLATE_invoke_key__: 'test-key', __TEMPLATE_os_name__: 'windows',
    __TEMPLATE_fetch_channel_data_command__: 'plugin:channel|fetch',
    __RAW_process_ipc_message_fn__: (payload) => ({ contentType: typeof payload === 'string' ? 'application/json' : 'application/octet-stream', data: payload }),
    fetch() { assert.fail('native IPC must not use unattributed custom-protocol fetch') },
  })
  vm.runInContext(native, context)
  const response = async (payload, type, bytes, ok = true, cmd = 'echo') => {
    reply = { status: 200, headers: { 'content-type': type, 'tauri-response': ok ? 'ok' : 'error' }, body: Buffer.from(bytes).toString('base64') }
    const count = callbacks.length
    window.__TAURI_INTERNALS__.postMessage({ cmd, callback: 1, error: 2, payload, options: { headers: { 'x-channel-id': '17' } } })
    await bounded((async () => { while (callbacks.length === count) await new Promise(resolve => setImmediate(resolve)) })())
    const request = requests.at(-1)
    assert.equal(new Headers(request.headers).get('tauri-invoke-key'), 'test-key')
    assert.equal(new Headers(request.headers).get('x-channel-id'), '17')
    assert.equal(callbacks.at(-1).id, ok ? 1 : 2)
    return callbacks.at(-1).value
  }
  for (const size of [0, 1023, 1024, 8193]) {
    const bytes = Buffer.alloc(size, 171)
    const raw = await response('{}', 'application/octet-stream', bytes)
    assert(raw instanceof ArrayBuffer)
    assert.deepEqual(Buffer.from(raw), bytes)
  }
  assert.deepEqual(await response('{}', 'application/json', '[0,255]'), [0, 255])
  assert.equal(await response('{}', 'application/json', '"denied"', false), 'denied')
  const view = new Uint8Array([9, 1, 2, 9]).subarray(1, 3)
  await response(view, 'text/plain', 'ok', true, 'plugin:channel|fetch')
  assert.deepEqual([...requests.at(-1).body], [1, 2], 'retain view offset and length')
  await response([3, 4], 'text/plain', 'ok')
  assert.deepEqual([...requests.at(-1).body], [3, 4])
  console.log('PASS runtime helpers: readiness, guest geometry and authenticated typed IPC including raw bytes, JSON arrays, errors and channel headers (offline; no Electron UI proof)')
} finally {
  for (const timer of timers) clearTimeout(timer)
}
