// Drives the running app over the Chrome DevTools Protocol to verify the
// runtime end to end: the `<webview>` guest renders, `invoke` round-trips, and
// the guest can be covered by page content.
// Usage: pnpm dev + cargo run with TAURI_RUNTIME_ELECTRON_ARGS="--remote-debugging-port=9222"
import { writeFileSync } from 'node:fs'

const list = await (await fetch('http://127.0.0.1:9222/json/list')).json()
const page = list.find((t) => t.type === 'page' && t.url.includes('localhost:1420'))
if (!page) throw new Error('page target not found')

const ws = new WebSocket(page.webSocketDebuggerUrl)
let nextId = 0
const pending = new Map()
ws.onmessage = (event) => {
  const message = JSON.parse(event.data)
  pending.get(message.id)?.(message)
  pending.delete(message.id)
}
await new Promise((resolve) => (ws.onopen = resolve))

const send = (method, params = {}) =>
  new Promise((resolve) => {
    const id = ++nextId
    pending.set(id, resolve)
    ws.send(JSON.stringify({ id, method, params }))
  })
const evaluate = async (expression) => {
  const response = await send('Runtime.evaluate', {
    expression,
    awaitPromise: true,
    returnByValue: true,
  })
  if (response.error || response.result?.exceptionDetails) {
    throw new Error(JSON.stringify(response.result?.exceptionDetails ?? response.error))
  }
  return response.result.result.value
}

const state = await evaluate(`({
  internals: typeof window.__TAURI_INTERNALS__,
  ipc: typeof window.ipc?.postMessage,
  webviewTag: !!document.querySelector('#embedded'),
})`)
console.log('state', state)

await evaluate(`(async () => {
  document.querySelector('#greet-form')?.dispatchEvent(
    new Event('submit', { bubbles: true, cancelable: true })
  )
})()`)
await new Promise((resolve) => setTimeout(resolve, 1500))
console.log('greet', await evaluate(`document.querySelector('#greet-result')?.textContent`))

console.log('guest', await evaluate(`document.querySelector('#embedded')?.getWebContentsId?.()`))

await evaluate(`(() => {
  const cover = document.createElement('div')
  cover.id = 'cover'
  cover.textContent = 'covered by the host page'
  cover.style.cssText = 'position:fixed;inset:0;background:#1e40af;color:white;display:grid;place-items:center;font:24px system-ui;z-index:99999'
  document.body.appendChild(cover)
})()`)
await new Promise((resolve) => setTimeout(resolve, 300))
const shot = await send('Page.captureScreenshot', { format: 'png' })
writeFileSync('probe-covered.png', Buffer.from(shot.result.data, 'base64'))
console.log('screenshot', 'probe-covered.png')
ws.close()
