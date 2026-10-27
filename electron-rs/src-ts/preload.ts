import { contextBridge, ipcRenderer, webFrame } from 'electron'

if (process.isMainFrame) {
  contextBridge.exposeInMainWorld('ipc', {
    postMessage: (data: any) => ipcRenderer.send('tauri-ipc', String(data)),
    fetch: (req: any) => ipcRenderer.invoke('tauri-ipc-fetch', req),
  })

  webFrame.executeJavaScript(`(${function installIpcFetch() {
    const oFetch = window.fetch
    window.fetch = async function (input, init) {
      const u = new URL(input instanceof Request ? input.url : input, location.href)
      if (u.username || u.password || u.port || !((u.protocol === 'ipc:' && u.hostname === 'localhost') || (/^https?:$/.test(u.protocol) && u.hostname === 'ipc.localhost'))) {
        return oFetch.call(this, input, init)
      }

      const req = new Request(input, init)
      return new Promise((resolve, reject) => {
        const abort = () => reject(req.signal.reason ?? new DOMException('Aborted', 'AbortError'))
        if (req.signal.aborted) return abort()
        req.signal.addEventListener('abort', abort, { once: true })

          ; (async () => {
            const res = await window.ipc.fetch({
              url: req.url, method: req.method,
              headers: [...req.headers], body: new Uint8Array(await req.arrayBuffer()),
            })
            const empty = req.method === 'HEAD' || [204, 205, 304].includes(res.status)
            return new Response(empty ? null : res.body, { status: res.status, headers: res.headers })
          })().then(resolve, reject).finally(() => req.signal.removeEventListener('abort', abort))
      })
    }
  }.toString()})()`)
}