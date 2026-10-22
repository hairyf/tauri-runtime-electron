// Electron side of the probe: speaks newline-delimited JSON with the Rust
// runtime over a loopback TCP socket and turns RPC calls into BrowserWindows.
'use strict'

const { app, BrowserWindow, ipcMain } = require('electron')
const net = require('node:net')
const fs = require('node:fs')
const os = require('node:os')
const path = require('node:path')

const logFile = path.join(os.tmpdir(), 'tauri-runtime-electron.log')
function log(...args) {
  fs.appendFileSync(logFile, `${new Date().toISOString()} ${args.join(' ')}\n`)
}

const port = Number(readArg('--port'))
if (!port) {
  console.error('missing --port')
  process.exit(1)
}

/** @type {import('node:net').Socket | undefined} */
let socket
const windows = new Map()
/** webviewId -> BrowserWindow */
const webviews = new Map()
/** webContents id -> webviewId (for routing `tauri-ipc` messages) */
const webviewByContents = new Map()
/** windowId -> webviewId of the contents that hosts the page */
const mainWebview = new Map()

function readArg(name) {
  const index = process.argv.indexOf(name)
  return index === -1 ? undefined : process.argv[index + 1]
}

function send(message) {
  if (socket && !socket.destroyed) socket.write(`${JSON.stringify(message)}\n`)
}

function serializeError(error) {
  return error && error.message ? error.message : String(error)
}

function createWindow(params) {
  const { windowId } = params
  const window = new BrowserWindow({
    title: params.title ?? 'Electron Runtime',
    width: params.width ?? 800,
    height: params.height ?? 600,
    x: params.x ?? undefined,
    y: params.y ?? undefined,
    center: params.center ?? false,
    resizable: params.resizable ?? true,
    frame: params.decorations ?? true,
    transparent: params.transparent ?? false,
    show: params.visible ?? true,
    webPreferences: {
      contextIsolation: true,
      sandbox: true,
      nodeIntegration: false,
      preload: path.join(__dirname, 'preload.cjs'),
      // Tauri pages embed guests with the Chromium `<webview>` element; Electron
      // disables that tag by default since v5.
      webviewTag: true,
    },
  })
  windows.set(windowId, window)

  window.on('closed', () => {
    windows.delete(windowId)
    mainWebview.delete(windowId)
    webviewByContents.delete(window.webContents.id)
    for (const [webviewId, owner] of webviews) {
      if (owner === window) webviews.delete(webviewId)
    }
    send({ event: 'window.closed', params: { windowId } })
  })
  window.webContents.on('did-finish-load', () => {
    log('loaded', window.webContents.getURL())
  })

  const webview = params.webview
  if (webview) return loadWebview(window, webview)
  return { windowId }
}

function loadWebview(window, webview) {
  const { webviewId, url, initScripts } = webview
  webviews.set(webviewId, window)
  webviewByContents.set(window.webContents.id, webviewId)
  window.webContents.on('dom-ready', () => {
    for (const script of initScripts ?? []) {
      window.webContents.executeJavaScript(script, true).catch(() => {})
    }
  })
  return window.loadURL(url).then(() => ({ windowId: webviewId }))
}

/**
 * Additional webviews on an already loaded window are real Chromium guests:
 * a `<webview>` element appended to the page, exactly how an app would embed
 * one itself.
 */
function attachWebviewElement(window, webview) {
  const { webviewId, url, initScripts } = webview
  webviews.set(webviewId, window)
  const elementId = `tauri-webview-${webviewId}`
  const scripts = (initScripts ?? [])
    .map(
      (script) =>
        `element.addEventListener('dom-ready', () => element.executeJavaScript(${JSON.stringify(script)}, true))`
    )
    .join('\n')
  const code = `(() => {
    const element = document.createElement('webview')
    element.id = ${JSON.stringify(elementId)}
    element.setAttribute('webpreferences', 'contextIsolation=yes,sandbox=yes')
    element.setAttribute('preload', ${JSON.stringify(path.join(__dirname, 'preload.cjs'))})
    element.src = ${JSON.stringify(url)}
    element.style.cssText = 'position:absolute;inset:0;width:100%;height:100%;'
    document.body.appendChild(element)
    ${scripts}
    return element.getWebContentsId()
  })()`
  return window.webContents
    .executeJavaScript(code, true)
    .then((contentsId) => {
      if (typeof contentsId === 'number') webviewByContents.set(contentsId, webviewId)
      return { webviewId }
    })
}

function findWebview(webviewId) {
  const window = webviews.get(webviewId)
  if (!window || window.isDestroyed()) throw new Error(`unknown webview ${webviewId}`)
  return window
}

function handle(method, params) {
  switch (method) {
    case 'window.create':
      return createWindow(params)
    case 'webview.create': {
      const window = windows.get(params.windowId)
      if (!window) throw new Error(`unknown window ${params.windowId}`)
      // The first webview of a window owns the page; later ones are guests.
      if (!mainWebview.has(params.windowId)) {
        mainWebview.set(params.windowId, params.webviewId)
        return loadWebview(window, params)
      }
      return attachWebviewElement(window, params)
    }
    case 'webview.eval': {
      const window = findWebview(params.webviewId)
      return window.webContents.executeJavaScript(params.script, true)
    }
    case 'window.setTitle': {
      const window = windows.get(params.windowId)
      if (window) window.setTitle(params.title)
      return null
    }
    case 'window.show':
      windows.get(params.windowId)?.show()
      return null
    case 'window.hide':
      windows.get(params.windowId)?.hide()
      return null
    case 'window.focus':
      windows.get(params.windowId)?.focus()
      return null
    case 'window.close':
      windows.get(params.windowId)?.close()
      return null
    case 'window.destroy':
      windows.get(params.windowId)?.destroy()
      return null
    default:
      throw new Error(`unknown method ${method}`)
  }
}

function onLine(line) {
  let message
  try {
    message = JSON.parse(line)
  } catch {
    log('bad line', line)
    return
  }
  if (typeof message.id !== 'number') {
    log('non-request', line)
    return
  }
  log('request', message.method)

  Promise.resolve()
    .then(() => handle(message.method, message.params ?? {}))
    .then((result) => {
      log('result', message.method, JSON.stringify(result ?? null))
      send({ id: message.id, result: result ?? null })
    })
    .catch((error) => {
      log('error', message.method, serializeError(error))
      send({ id: message.id, error: serializeError(error) })
    })
}

ipcMain.on('tauri-ipc', (event, data) => {
  const webviewId = webviewByContents.get(event.sender.id)
  if (webviewId === undefined) {
    log('ipc message from unknown contents', event.sender.id)
    return
  }
  send({
    event: 'ipc.message',
    params: { webviewId, url: event.sender.getURL(), body: data },
  })
})

app.whenReady().then(() => {
  socket = net.connect(port, '127.0.0.1', () => {
    send({ event: 'ready', params: { electron: process.versions.electron } })
  })

  let buffer = ''
  socket.setEncoding('utf8')
  socket.on('data', (chunk) => {
    buffer += chunk
    let index = buffer.indexOf('\n')
    while (index !== -1) {
      onLine(buffer.slice(0, index))
      buffer = buffer.slice(index + 1)
      index = buffer.indexOf('\n')
    }
  })
  socket.on('close', () => {
    send({ event: 'ipc.closed' })
    app.quit()
  })
  socket.on('error', () => app.quit())
})

// The Rust side owns the lifetime: no window means keep running until the
// socket closes.
app.on('window-all-closed', () => {})
