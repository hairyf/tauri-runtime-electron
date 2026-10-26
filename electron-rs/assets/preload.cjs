// Tauri's ordinary postMessage bridge and a binary-preserving IPC fetch path.
// The latter needs the native sender: Electron's protocol.handle omits it.
'use strict'

const { contextBridge, ipcRenderer, webFrame } = require('electron')

if (process.isMainFrame) {
contextBridge.exposeInMainWorld('ipc', {
  postMessage(data) {
    ipcRenderer.send('tauri-ipc', String(data))
  },
  fetch(request) {
    return ipcRenderer.invoke('tauri-ipc-fetch', request)
  },
})

webFrame.executeJavaScript(`(${function installIpcFetch() {
  const originalFetch = window.fetch
  window.fetch = function (input, init) {
    // Leave all non-IPC requests with Chromium, including real dev servers.
    const url = new URL(input instanceof Request ? input.url : input, location.href)
    if (url.username || url.password || url.port || !(
      (url.protocol === 'ipc:' && url.hostname === 'localhost') ||
      (/^https?:$/.test(url.protocol) && url.hostname === 'ipc.localhost')
    )) return originalFetch.call(this, input, init)
    const request = new Request(input, init)
    return new Promise((resolve, reject) => {
      const abort = () => reject(request.signal.reason ?? new DOMException('Aborted', 'AbortError'))
      if (request.signal.aborted) return abort()
      request.signal.addEventListener('abort', abort, { once: true })
      ;(async () => {
        const body = await request.arrayBuffer()
        const result = await window.ipc.fetch({
          url: request.url, method: request.method,
          headers: [...request.headers], body: new Uint8Array(body),
        })
        const empty = request.method === 'HEAD' || [204, 205, 304].includes(result.status)
        return new Response(empty ? null : result.body, { status: result.status, headers: result.headers })
      })().then(resolve, reject).finally(() => request.signal.removeEventListener('abort', abort))
    })
  }
}.toString()})()`)
}
