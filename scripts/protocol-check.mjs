// Pure Node regression: no Electron process, GUI, socket, or network.
// Run: node scripts/protocol-check.mjs
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import vm from 'node:vm'

const require = createRequire(import.meta.url)
const handlers = new Map()
const requests = []
const messages = []
const listeners = new Map()
const invokeHandlers = new Map()
const forwarded = []
const csp = "default-src 'none'; frame-src http://isolation-test.localhost"
let reply = {
  status: 200,
  headers: { 'Content-Security-Policy': csp, 'Content-Type': 'text/plain' },
  body: Buffer.from('resolved locally').toString('base64'),
}
const electron = {
  app: { on() {}, whenReady: () => new Promise(() => {}) },
  ipcMain: {
    on(name, callback) { listeners.set(name, callback) },
    handle(name, callback) { invokeHandlers.set(name, callback) },
  },
  protocol: {
    registerSchemesAsPrivileged() {},
    handle(scheme, handler) {
      assert(!handlers.has(scheme), `duplicate protocol handler: ${scheme}`)
      handlers.set(scheme, handler)
    },
  },
  net: {
    fetch(request, options) {
      assert.equal(options.bypassCustomProtocolHandlers, true)
      forwarded.push(request)
      return Promise.resolve(new Response('ordinary HTTP'))
    },
  },
}
const context = vm.createContext({
  require: (name) => name === 'electron' ? electron
    : name === 'node:fs' ? { appendFileSync() {} } : require(name),
  process: { argv: ['node', 'main.cjs', '--port', '1'], env: {} },
  Buffer, URL, Request, Response, Headers, Uint8Array, console,
  testSocket: {
    write(line) {
      const message = JSON.parse(line)
      if (message.event === 'ipc.message') {
        messages.push(message.params)
        return
      }
      assert.equal(message.event, 'protocol.request')
      requests.push(message.params)
      context.handle('protocol.respond', { id: message.params.id, ...reply })
    },
  },
})
vm.runInContext(`${readFileSync(new URL('../electron-rs/assets/main.cjs', import.meta.url), 'utf8')}
;socket = testSocket`, context)

const timeout = setTimeout(() => { throw new Error('protocol regression timed out') }, 3000)
for (const scheme of ['tauri', 'ipc', 'asset', 'isolation-test']) {
  context.handle('protocol.register', { scheme })
}
context.handle('protocol.register', { scheme: 'tauri' })

for (const transport of ['http', 'https']) {
  const request = new Request(`${transport}://tauri.localhost/assets/a%20b.js?v=1`, {
    method: 'POST', body: 'payload', headers: { Origin: `${transport}://tauri.localhost` },
  })
  const response = await handlers.get(transport)(request)
  assert.equal(request.url, `${transport}://tauri.localhost/assets/a%20b.js?v=1`)
  assert.equal(response.headers.get('Content-Security-Policy'), csp)
  assert.equal(await response.text(), 'resolved locally')
  const sent = requests.at(-1)
  assert.equal(sent.scheme, 'tauri')
  assert.equal(sent.uri, 'tauri://localhost/assets/a%20b.js?v=1')
  assert.equal(sent.method, 'POST')
  assert.equal(sent.headers.origin, `${transport}://tauri.localhost`)
  assert.equal(Buffer.from(sent.body, 'base64').toString(), 'payload')
}

// Chromium's initiator wins over a header, including opaque sandbox origins.
for (const origin of ['http://tauri.localhost', 'https://remote.example', 'null']) {
  const request = new Request('http://asset.localhost/file', {
    headers: { Origin: 'http://wrong.localhost' },
  })
  request.initiatorOrigin = origin
  await handlers.get('http')(request)
  assert.equal(requests.at(-1).headers.origin, origin)
}
const frame = new Request('http://isolation-test.localhost/')
await handlers.get('http')(frame)
assert.equal(requests.at(-1).scheme, 'isolation-test')
assert.equal(requests.at(-1).uri, 'isolation-test://localhost/')
assert.equal(requests.at(-1).headers.origin, undefined, 'do not fabricate a local origin')

// Native custom schemes still work, without installing another HTTP handler.
await handlers.get('asset')(new Request('asset://localhost/file.png'))
assert.equal(requests.at(-1).uri, 'asset://localhost/file.png')
const count = requests.length
for (const url of [
  'http://localhost:1420/', 'http://127.0.0.1:1420/',
  'http://tauri.localhost:1420/', 'https://tauri.localhost:8443/',
  'http://unregistered.localhost/', 'http://tauri.localhost.attacker.example/',
  'https://example.com/',
]) {
  const request = new Request(url)
  assert.equal(await (await handlers.get(new URL(url).protocol.slice(0, -1))(request)).text(), 'ordinary HTTP')
  assert.equal(forwarded.at(-1), request, 'delegate the original request, not just its URL')
}
assert.equal(requests.length, count, 'ordinary HTTP must not reach a Tauri protocol')
assert.equal(handlers.size, 6)
// Invokes need the authenticated sender. No session-global protocol owner,
// renderer-provided header or forged origin may select another webview.
const beforeIpc = requests.length
const senderEvents = []
assert.equal((await handlers.get('ipc')(new Request('ipc://localhost/ping'))).type, 'error')
assert.equal((await handlers.get('http')(new Request('http://ipc.localhost/ping'))).type, 'error')
assert.equal(requests.length, beforeIpc)
for (const [contentsId, webviewId, url] of [
  [11, 1, 'http://tauri.localhost/'],
  [12, 2, 'tauri://localhost/second'],
  [13, 3, 'https://remote.example/'],
]) {
  const frame = { url, origin: new URL(url).origin }
  // Node's URL treats registered custom schemes as opaque; Chromium does not.
  if (url.startsWith('tauri:')) frame.origin = 'tauri://localhost'
  const sender = { id: contentsId, mainFrame: frame }
  senderEvents.push({ sender, senderFrame: frame })
  context.senderId = contentsId
  context.webviewId = webviewId
  vm.runInContext('webviewByContents.set(senderId, webviewId)', context)
  listeners.get('tauri-ipc')({ sender, senderFrame: frame }, 'ping')
  assert.deepEqual(JSON.parse(JSON.stringify(messages.at(-1))), { webviewId, url, body: 'ping' })
}
const countMessages = messages.length
const mainFrame = { url: 'http://tauri.localhost/', origin: 'http://tauri.localhost' }
for (const event of [
  { sender: { id: 11, mainFrame }, senderFrame: { ...mainFrame } },
  { sender: { id: 11, mainFrame }, senderFrame: null },
  { sender: { id: 999, mainFrame }, senderFrame: mainFrame },
]) listeners.get('tauri-ipc')(event, 'ping')
const opaque = { url: 'data:text/html,opaque', origin: 'null' }
listeners.get('tauri-ipc')({ sender: { id: 11, mainFrame: opaque }, senderFrame: opaque }, 'ping')
listeners.get('tauri-ipc')({ sender: { id: 11, mainFrame }, senderFrame: mainFrame }, { body: 'ping' })
assert.equal(messages.length, countMessages, 'reject unknown senders, child/opaque frames and non-string payloads')
// Exercise the real main-process binary fetch handler, without protocol-owner guesses.
const fetchIpc = invokeHandlers.get('tauri-ipc-fetch')
assert.equal(typeof fetchIpc, 'function')
const raw = Uint8Array.of(0, 1, 127, 128, 255)
const replyHeaders = {
  'Content-Security-Policy': csp,
  'Content-Type': 'application/octet-stream',
  'Tauri-Response': 'ok',
  'X-Test': 'binary',
}
reply = { status: 206, headers: replyHeaders, body: Buffer.from(raw).toString('base64') }
const requestData = (url = 'http://ipc.localhost/echo?v=1') => ({
  url, method: 'POST', body: raw,
  headers: [['Content-Type', 'application/octet-stream'], ['Tauri-Invoke-Key', 'test-key'],
    ['Origin', 'http://forged.localhost'], ['X-Test', 'request']],
  webviewId: 0xffff_ffff, origin: 'http://forged.localhost',
})
for (const [index, event] of senderEvents.entries()) {
  const url = ['http://ipc.localhost/echo?v=1', 'ipc://localhost/echo?v=1', 'https://ipc.localhost/echo?v=1'][index]
  const result = await fetchIpc(event, requestData(url))
  const sent = requests.at(-1)
  assert.equal(sent.scheme, 'ipc')
  assert.equal(sent.webviewId, index + 1, 'bind owner to native sender, ignoring renderer identity')
  assert.equal(sent.uri, 'ipc://localhost/echo?v=1')
  assert.equal(sent.method, 'POST')
  assert.equal(sent.headers.origin, event.senderFrame.origin, 'never promote a remote origin to local')
  assert.equal(sent.headers['tauri-invoke-key'], 'test-key')
  assert.equal(sent.headers['content-type'], 'application/octet-stream')
  assert.equal(sent.headers['x-test'], 'request')
  assert.deepEqual([...Buffer.from(sent.body, 'base64')], [...raw])
  assert.equal(result.status, 206)
  assert.ok(result.body instanceof Uint8Array)
  assert.deepEqual([...result.body], [...raw])
  const response = new Response(result.body, { status: result.status, headers: result.headers })
  assert.equal(response.headers.get('Content-Security-Policy'), csp)
  assert.equal(response.headers.get('Tauri-Response'), 'ok')
  assert.equal(response.headers.get('content-type'), 'application/octet-stream')
  assert.equal(response.headers.get('x-test'), 'binary')
  assert.deepEqual([...new Uint8Array(await response.arrayBuffer())], [...raw])
}
const beforeRejectedFetches = requests.length
for (const url of [
  'http://localhost/', 'http://127.0.0.1/', 'https://example.com/',
  'file:///local', 'ipc://attacker.example/ping', 'ipc://localhost:1/ping',
  'http://ipc.localhost:1420/ping', 'https://ipc.localhost:8443/ping',
  'http://ipc.localhost.attacker.example/ping', 'http://user:pass@ipc.localhost/ping',
]) await assert.rejects(() => fetchIpc(senderEvents[0], requestData(url)), /invalid IPC request/)
for (const event of [
  { sender: senderEvents[0].sender, senderFrame: null },
  { sender: senderEvents[0].sender, senderFrame: { ...mainFrame } },
  { sender: { id: 999, mainFrame }, senderFrame: mainFrame },
  { sender: { id: 11, mainFrame: opaque }, senderFrame: opaque },
]) await assert.rejects(() => fetchIpc(event, requestData()), /unknown IPC sender/)
for (const data of [
  { ...requestData(), method: 'GET' },
  { ...requestData(), body: raw.buffer },
  { ...requestData(), body: [...raw] },
  { ...requestData(), body: 'not binary' },
]) await assert.rejects(() => fetchIpc(senderEvents[0], data), /invalid IPC request/)
assert.equal(requests.length, beforeRejectedFetches, 'rejected fetches must never reach Rust')
// HEAD preserves representation metadata but sends no payload. Null-body status
// codes must pass null (an empty Buffer is still rejected by Response).
for (const method of ['GET', 'HEAD']) {
  for (const status of [200, 204, 205, 304]) {
    for (const body of ['', Buffer.from('must be suppressed').toString('base64')]) {
      reply = {
        status, body,
        headers: { 'Content-Security-Policy': csp, 'Content-Type': 'text/plain',
          'Content-Length': '18', 'Transfer-Encoding': 'chunked', ETag: '"unchanged"' },
      }
      const response = await handlers.get('tauri')(new Request('tauri://localhost/no-body', { method }))
      assert.equal(response.status, status)
      assert.equal(response.headers.get('Content-Security-Policy'), csp)
      assert.equal(response.headers.get('content-type'), 'text/plain')
      assert.equal(response.headers.get('etag'), '"unchanged"')
      if (method === 'HEAD' || status !== 200) {
        assert.equal(response.body, null)
        assert.equal((await response.arrayBuffer()).byteLength, 0)
      } else {
        assert.equal(await response.text(), Buffer.from(body, 'base64').toString())
      }
      assert.equal(response.headers.get('content-length'), [204, 205].includes(status) ? null : '18')
      assert.equal(response.headers.get('transfer-encoding'), [204, 205].includes(status) ? null : 'chunked')
    }
  }
}
clearTimeout(timeout)
console.log('ok: virtual protocols/CSP, ordinary HTTP, authenticated owners/origins, binary IPC bytes and HEAD/null-body responses')
