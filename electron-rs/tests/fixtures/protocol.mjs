// Native doubles only: Rust checks observations from the real main process code.
import { readFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import vm from 'node:vm'
import { runtimeModules } from './runtime-modules.mjs'

const require = createRequire(new URL('../../assets/main.cjs', import.meta.url))
const handlers = new Map()
const registrations = []
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
      registrations.push(scheme)
      handlers.set(scheme, handler)
    },
  },
  net: {
    fetch(request, options) {
      forwarded.push({ request, options })
      return Promise.resolve(new Response('ordinary HTTP'))
    },
  },
}
const context = vm.createContext({
  require: name => name === 'electron' ? electron
    : name === 'node:fs' ? { appendFileSync() {} } : require(name),
  process: { argv: ['node', 'main.cjs', '--port', '1'], env: {} },
  Buffer, URL, Request, Response, Headers, Uint8Array, console,
  testSocket: {
    send(message) {
      if (message.event === 'ipc.message') messages.push(message.params)
      else {
        requests.push(message)
        context.handle('protocol.respond', { id: message.params.id, ...reply })
      }
    },
  },
})
context.require = runtimeModules(context, context.require)
vm.runInContext(`${readFileSync(new URL('../../assets/main.cjs', import.meta.url), 'utf8')}
;state.socket = testSocket`, context)
for (const scheme of ['tauri', 'ipc', 'asset', 'isolation-test', 'tauri']) {
  context.handle('protocol.register', { scheme })
}
const lastRequest = () => {
  const { event, params: { id, ...params } } = requests.at(-1)
  return { event, ...params, body: [...Buffer.from(params.body, 'base64')] }
}
const responseData = async response => ({
  status: response.status, headers: Object.fromEntries(response.headers),
  body: [...new Uint8Array(await response.arrayBuffer())],
})
const errorOf = async action => {
  try { await action(); return null } catch (error) { return error.message }
}
let result
switch (process.argv[2]) {
  case 'http': {
    const virtual = []
    for (const transport of ['http', 'https']) {
      const request = new Request(`${transport}://tauri.localhost/assets/a%20b.js?v=1`, {
        method: 'POST', body: 'payload', headers: { Origin: `${transport}://tauri.localhost` },
      })
      const response = await handlers.get(transport)(request)
      virtual.push({ url: request.url, sent: lastRequest(), response: await responseData(response) })
    }
    const origins = []
    for (const origin of ['http://tauri.localhost', 'https://remote.example', 'null']) {
      const request = new Request('http://asset.localhost/file', { headers: { Origin: 'http://wrong.localhost' } })
      request.initiatorOrigin = origin
      await handlers.get('http')(request)
      origins.push(lastRequest().headers.origin)
    }
    await handlers.get('http')(new Request('http://isolation-test.localhost/'))
    const isolation = lastRequest()
    await handlers.get('asset')(new Request('asset://localhost/file.png'))
    const asset = lastRequest()
    const count = requests.length
    const ordinary = []
    for (const url of [
      'http://localhost:1420/', 'http://127.0.0.1:1420/',
      'http://tauri.localhost:1420/', 'https://tauri.localhost:8443/',
      'http://unregistered.localhost/', 'http://tauri.localhost.attacker.example/',
      'https://example.com/',
    ]) {
      const request = new Request(url)
      const response = await handlers.get(new URL(url).protocol.slice(0, -1))(request)
      ordinary.push({ body: await response.text(), sameRequest: forwarded.at(-1).request === request,
        options: forwarded.at(-1).options })
    }
    result = { virtual, origins, isolation, asset, ordinary, forwardedToRust: requests.length - count, registrations }
    break
  }
  case 'ipc': {
    const beforeIpc = requests.length
    const anonymous = []
    for (const [scheme, url] of [['ipc', 'ipc://localhost/ping'], ['http', 'http://ipc.localhost/ping']]) {
      anonymous.push((await handlers.get(scheme)(new Request(url))).type)
    }
    const anonymousRequests = requests.length - beforeIpc
    const senderEvents = []
    for (const [contentsId, webviewId, url] of [
      [11, 1, 'http://tauri.localhost/'], [12, 2, 'tauri://localhost/second'], [13, 3, 'https://remote.example/'],
    ]) {
      const frame = { url, origin: url.startsWith('tauri:') ? 'tauri://localhost' : new URL(url).origin }
      const sender = { id: contentsId, mainFrame: frame }
      senderEvents.push({ sender, senderFrame: frame })
      context.senderId = contentsId
      context.webviewId = webviewId
      vm.runInContext('state.webviewByContents.set(senderId, webviewId)', context)
      listeners.get('tauri-ipc')({ sender, senderFrame: frame }, 'ping')
    }
    const acceptedMessages = messages.slice()
    const mainFrame = { url: 'http://tauri.localhost/', origin: 'http://tauri.localhost' }
    const opaque = { url: 'data:text/html,opaque', origin: 'null' }
    const rejectedSenders = [
      { sender: { id: 11, mainFrame }, senderFrame: { ...mainFrame } },
      { sender: { id: 11, mainFrame }, senderFrame: null },
      { sender: { id: 999, mainFrame }, senderFrame: mainFrame },
      { sender: { id: 11, mainFrame: opaque }, senderFrame: opaque },
    ]
    for (const event of rejectedSenders) listeners.get('tauri-ipc')(event, 'ping')
    listeners.get('tauri-ipc')({ sender: { id: 11, mainFrame }, senderFrame: mainFrame }, { body: 'ping' })
    const raw = Uint8Array.of(0, 1, 127, 128, 255)
    reply = { status: 206, body: Buffer.from(raw).toString('base64'), headers: {
      'Content-Security-Policy': csp, 'Content-Type': 'application/octet-stream', 'Tauri-Response': 'ok', 'X-Test': 'binary',
    } }
    const requestData = (url = 'http://ipc.localhost/echo?v=1') => ({
      url, method: 'POST', body: raw,
      headers: [['Content-Type', 'application/octet-stream'], ['Tauri-Invoke-Key', 'test-key'],
        ['Origin', 'http://forged.localhost'], ['X-Test', 'request']],
      webviewId: 0xffff_ffff, origin: 'http://forged.localhost',
    })
    const fetchIpc = invokeHandlers.get('tauri-ipc-fetch')
    const binary = []
    for (const [index, event] of senderEvents.entries()) {
      const url = ['http://ipc.localhost/echo?v=1', 'ipc://localhost/echo?v=1', 'https://ipc.localhost/echo?v=1'][index]
      const response = await fetchIpc(event, requestData(url))
      binary.push({ sent: lastRequest(), typedBytes: response.body instanceof Uint8Array,
        response: await responseData(new Response(response.body, { status: response.status, headers: response.headers })) })
    }
    const beforeRejected = requests.length
    const invalidUrls = []
    for (const url of [
      'http://localhost/', 'http://127.0.0.1/', 'https://example.com/', 'file:///local',
      'ipc://attacker.example/ping', 'ipc://localhost:1/ping',
      'http://ipc.localhost:1420/ping', 'https://ipc.localhost:8443/ping',
      'http://ipc.localhost.attacker.example/ping', 'http://user:pass@ipc.localhost/ping',
    ]) invalidUrls.push(await errorOf(() => fetchIpc(senderEvents[0], requestData(url))))
    const invalidSenders = []
    for (const event of rejectedSenders) invalidSenders.push(await errorOf(() => fetchIpc(event, requestData())))
    const invalidPayloads = []
    for (const data of [
      { ...requestData(), method: 'GET' }, { ...requestData(), body: raw.buffer },
      { ...requestData(), body: [...raw] }, { ...requestData(), body: 'not binary' },
    ]) invalidPayloads.push(await errorOf(() => fetchIpc(senderEvents[0], data)))
    result = { anonymous, anonymousRequests, acceptedMessages, rejectedMessages: messages.length - acceptedMessages.length,
      binary, invalidUrls, invalidSenders, invalidPayloads, rejectedRequests: requests.length - beforeRejected }
    break
  }
  case 'bodies': {
    result = []
    for (const method of ['GET', 'HEAD']) {
      for (const status of [200, 204, 205, 304]) {
        for (const body of ['', 'must be suppressed']) {
          reply = { status, body: Buffer.from(body).toString('base64'), headers: {
            'Content-Security-Policy': csp, 'Content-Type': 'text/plain',
            'Content-Length': '18', 'Transfer-Encoding': 'chunked', ETag: '"unchanged"',
          } }
          const response = await handlers.get('tauri')(new Request('tauri://localhost/no-body', { method }))
          const nullBody = response.body === null
          result.push({ method, status, input: body, nullBody, response: await responseData(response) })
        }
      }
    }
    break
  }
  default: throw Error('unknown protocol scenario')
}
process.stdout.write(JSON.stringify(result))
