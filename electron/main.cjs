// Electron side of the probe: speaks newline-delimited JSON with the Rust
// runtime over a loopback TCP socket and turns RPC calls into BrowserWindows.
'use strict'

const {
  app,
  BrowserWindow,
  WebContentsView,
  ipcMain,
  Menu,
  nativeImage,
  nativeTheme,
  net: electronNet,
  protocol,
  screen,
  session,
  shell,
  webContents,
} = require('electron')
const net = require('node:net')
const fs = require('node:fs')
const os = require('node:os')
const path = require('node:path')

// Electron keys its profile by app name, and that name is "Electron" for every
// app built on this runtime: without this, two running apps would share one
// profile, cache and lock.
const owner = process.env.TAURI_RUNTIME_ELECTRON_OWNER
if (owner) app.setPath('userData', path.join(app.getPath('appData'), owner))

const logFile = path.join(os.tmpdir(), 'tauri-runtime-electron.log')
function log(...args) {
  fs.appendFileSync(logFile, `${new Date().toISOString()} ${args.join(' ')}\n`)
}

/** F12 / Ctrl+Shift+I toggle the DevTools of the focused renderer. */
function installDevtoolsShortcut(contents) {
  contents.on('before-input-event', (event, input) => {
    if (input.type !== 'keyDown') return
    const f12 = input.key === 'F12'
    const inspect = input.control && input.shift && input.key.toLowerCase() === 'i'
    if (!f12 && !inspect) return
    event.preventDefault()
    if (contents.isDevToolsOpened()) contents.closeDevTools()
    else contents.openDevTools({ mode: 'detach' })
    setTimeout(() => log('devtools', contents.id, contents.isDevToolsOpened()), 300)
  })
}

/** Right-click menu; Electron ships none, the BrowserWindow only has a page. */
function installContextMenu(contents) {
  contents.on('context-menu', (_event, params) => {
    const items = []
    if (params.linkURL) {
      items.push({
        label: 'Open Link in Browser',
        click: () => shell.openExternal(params.linkURL),
      })
    }
    if (params.isEditable) {
      items.push(
        { role: 'cut', label: 'Cut', enabled: params.editFlags.canCut },
        { role: 'copy', label: 'Copy', enabled: params.editFlags.canCopy },
        { role: 'paste', label: 'Paste', enabled: params.editFlags.canPaste },
        { role: 'selectAll', label: 'Select All' }
      )
    } else if (params.selectionText.trim()) {
      items.push({ role: 'copy', label: 'Copy' })
    }
    if (items.length) items.push({ type: 'separator' })
    items.push(
      { label: 'Back', enabled: contents.navigationHistory.canGoBack(), click: () => contents.navigationHistory.goBack() },
      { label: 'Forward', enabled: contents.navigationHistory.canGoForward(), click: () => contents.navigationHistory.goForward() },
      { role: 'reload', label: 'Reload' },
      { type: 'separator' },
      { label: 'Inspect Element', click: () => contents.inspectElement(params.x, params.y) }
    )
    try {
      const menu = Menu.buildFromTemplate(items)
      // `fromWebContents` is null for a `<webview>` guest, so fall back to the
      // window the guest belongs to (or the focused one).
      const owner = BrowserWindow.fromWebContents(contents) ?? BrowserWindow.getFocusedWindow()
      menu.popup({ ...(owner ? { window: owner } : {}) })
    } catch (error) {
      log('context-menu error', serializeError(error))
    }
  })
}

const port = Number(readArg('--port'))
if (!port) {
  console.error('missing --port')
  process.exit(1)
}

// The Rust runtime passes the app identifier so the window looks like the Tauri
// app instead of Electron itself; the window icon travels with the creation
// request (the runtime extracts it from the executable when it is bundled).
const appIdentifier = readArg('--tauri-identifier') || undefined

// Tauri's custom schemes (`tauri://localhost`, plugin protocols) must be
// privileged before the app is ready, so the Rust runtime passes them on the
// command line.
const tauriSchemes = (readArg('--tauri-schemes') ?? '')
  .split(',')
  .map((scheme) => scheme.trim())
  .filter(Boolean)
protocol.registerSchemesAsPrivileged(
  tauriSchemes.map((scheme) => ({
    scheme,
    privileges: {
      standard: true,
      secure: true,
      supportFetchAPI: true,
      corsEnabled: true,
      stream: true,
    },
  }))
)

let localhostProtocolsInstalled = false

/** Keep WebView2's virtual HTTP(S) origin: isolation and CSP depend on it. */
function installLocalhostProtocols() {
  if (localhostProtocolsInstalled) return
  for (const transport of ['http', 'https']) {
    protocol.handle(transport, (request) => {
      const url = new URL(request.url)
      const match = /^([a-z][a-z0-9+.-]*)\.localhost$/.exec(url.hostname)
      // A dev server on a real port, or any unregistered host, stays ordinary HTTP.
      if (url.port || !match || !registeredSchemes.has(match[1])) {
        return electronNet.fetch(request, { bypassCustomProtocolHandlers: true })
      }
      // Only the resolver sees the native scheme, just like Wry's WebView2 adapter.
      const uri = `${match[1]}://localhost${url.pathname}${url.search}`
      return dispatchProtocol(match[1], request, uri)
    })
  }
  localhostProtocolsInstalled = true
}

/** id -> resolve(Response); a `protocol.request` awaiting `protocol.respond`. */
const protocolRequests = new Map()
let nextProtocolId = 1
const registeredSchemes = new Set()

async function dispatchProtocol(scheme, request, uri = request.url, webviewId) {
  // Electron's session-global protocol callback has no authenticated sender.
  // IPC uses the narrow preload fetch bridge below; never guess its owner.
  if (scheme === 'ipc' && webviewId === undefined) return Response.error()
  const id = nextProtocolId++
  const body = Buffer.from(await request.arrayBuffer()).toString('base64')
  const headers = {}
  for (const [name, value] of request.headers) headers[name] = value
  // Chromium's initiator is not page-controlled; preserve opaque origins too.
  if (typeof request.initiatorOrigin === 'string') headers.origin = request.initiatorOrigin
  const response = new Promise((resolve) => protocolRequests.set(id, resolve))
  send({
    event: 'protocol.request',
    params: { id, scheme, webviewId, uri, method: request.method, headers, body },
  })
  const reply = await response
  log('protocol', scheme, request.method, request.url, reply.status)
  if (reply.status >= 400) {
    log('protocol-error', request.url, Buffer.from(reply.body ?? '', 'base64').toString().slice(0, 300))
  }
  const status = reply.status ?? 200
  const empty = request.method === 'HEAD' || [204, 205, 304].includes(status)
  const responseHeaders = new Headers(reply.headers ?? {})
  if (status === 204 || status === 205) {
    responseHeaders.delete('content-length')
    responseHeaders.delete('transfer-encoding')
  }
  return new Response(empty ? null : Buffer.from(reply.body ?? '', 'base64'), {
    status,
    headers: responseHeaders,
  })
}

/** @type {import('node:net').Socket | undefined} */
let socket
const windows = new Map()
// Windows Tauri accepted to close; everything else holds the close.
const closeAllowed = new Set()
/** webviewId -> BrowserWindow */
const webviews = new Map()
/** webContents id -> webviewId (for routing `tauri-ipc` messages) */
const webviewByContents = new Map()
/** webviewId -> native child view and its resize reference (main pages stay on BrowserWindow). */
const childWebviews = new Map()
/** webviewId -> the out-of-band callbacks the runtime registered */
const handlerPolicy = new Map()
/** request id -> DownloadItem awaiting the runtime's answer */
const downloadRequests = new Map()
/** request id -> where Electron writes now vs where the runtime wants it */
const downloadPaths = new Map()
/** request id -> Electron permission callback awaiting the runtime */
const permissionRequests = new Map()
let nextDownloadRequest = 0
let nextPermissionRequest = 0

/** Electron's permission names, mapped onto the kinds Tauri can name. */
function permissionKind(permission, details) {
  if (permission === 'media') {
    const types = details?.mediaTypes ?? []
    return types.includes('video') ? 'camera' : 'microphone'
  }
  return {
    geolocation: 'geolocation',
    notifications: 'notifications',
    'clipboard-read': 'clipboard-read',
    'display-capture': 'display-capture',
    midi: 'midi',
    midiSysex: 'midi',
    sensors: 'sensors',
  }[permission]
}

function policyFor(contents) {
  const webviewId = contents && webviewByContents.get(contents.id)
  return webviewId === undefined ? undefined : handlerPolicy.get(webviewId)
}

function recordPolicy(webviewId, policy) {
  if (!policy) return
  if (policy.download || policy.permission) handlerPolicy.set(webviewId, policy)
}

/**
 * Downloads and permission prompts are decided by the Rust run loop, so they are
 * forwarded to it and answered later instead of blocking the main thread.
 */
function installPolicyHandlers() {
  session.defaultSession.on('will-download', (_event, item, contents) => {
    log('download.begin', contents && contents.id, item.getURL(), 'policy', JSON.stringify(policyFor(contents)))
    const webviewId = contents && webviewByContents.get(contents.id)
    const policy = webviewId === undefined ? undefined : handlerPolicy.get(webviewId)
    if (!policy || !policy.download) return
    const id = ++nextDownloadRequest
    downloadRequests.set(id, item)
    // Electron only starts writing once a save path exists; the runtime's answer
    // may replace this provisional one before the first byte is written.
    if (!item.getSavePath()) item.setSavePath(path.join(os.tmpdir(), `electron-download-${id}.bin`))
    downloadPaths.set(id, { provisional: item.getSavePath(), finalPath: item.getSavePath() })
    item.once('done', (_done, state) => {
      const entry = downloadPaths.get(id)
      downloadPaths.delete(id)
      if (entry && entry.finalPath !== entry.provisional) {
        // `setSavePath` only takes effect before the first byte, so the data sits
        // in the provisional file; move it once the download has finished.
        if (state === 'completed' && fs.existsSync(entry.provisional)) {
          try {
            fs.renameSync(entry.provisional, entry.finalPath)
            log('download.moved', entry.provisional, entry.finalPath)
          } catch (error) {
            log('download.move failed', serializeError(error))
          }
        } else if (fs.existsSync(entry.provisional)) {
          fs.rmSync(entry.provisional, { force: true })
        }
      }
      log('download.done', state, entry ? entry.finalPath : item.getSavePath())
      const path = state === 'completed' ? (entry ? entry.finalPath : item.getSavePath()) : null
      send({
        event: 'download.finished',
        params: { webviewId, url: item.getURL(), path, success: state === 'completed' },
      })
    })
    // The runtime answers through `webview.downloadResponse`. Electron keeps the
    // original path when the answer arrives after the first byte.
    send({
      event: 'download.requested',
      params: { id, webviewId, url: item.getURL(), destination: item.getSavePath() },
    })
  })

  session.defaultSession.setPermissionRequestHandler((contents, permission, callback, details) => {
    const policy = policyFor(contents)
    const kind = permissionKind(permission, details)
    if (!policy || !policy.permission || !kind) {
      // Nothing registered for this webview; Electron's own default is to deny.
      callback(false)
      return
    }
    const id = ++nextPermissionRequest
    permissionRequests.set(id, callback)
    send({ event: 'permission.requested', params: { id, webviewId: webviewIdFor(contents), kind } })
  })
}

function webviewIdFor(contents) {
  return contents ? webviewByContents.get(contents.id) : undefined
}

function readArg(name) {
  const index = process.argv.indexOf(name)
  if (index !== -1) return process.argv[index + 1]
  const prefixed = process.argv.find((arg) => arg.startsWith(`${name}=`))
  return prefixed?.slice(name.length + 1)
}

function send(message) {
  if (socket && !socket.destroyed) socket.write(`${JSON.stringify(message)}\n`)
}

function serializeError(error) {
  return error && error.message ? error.message : String(error)
}

async function createWindow(params) {
  const { windowId } = params
  // Windows canonicalized paths arrive with a verbatim `\\?\` prefix that
  // Electron's image loader cannot open.
  const icon =
    typeof params.icon === 'string' ? params.icon.replace(/^\\\\\?\\/, '') : params.icon
  log('window.create', JSON.stringify({ windowId, icon: icon ?? null }))
  // Creation-time fields are the only chance Electron gets: it ignores most of
  // them afterwards, so a field missing here is a config value silently lost.
  // linux ignores maximizable/minimizable/closable/focusable, like Tauri does.
  const creation = {
    minWidth: params.minWidth,
    minHeight: params.minHeight,
    maxWidth: params.maxWidth,
    maxHeight: params.maxHeight,
    fullscreen: params.fullscreen ?? false,
    maximizable: params.maximizable ?? true,
    minimizable: params.minimizable ?? true,
    closable: params.closable ?? true,
    focusable: params.focusable ?? true,
    skipTaskbar: params.skipTaskbar ?? false,
    alwaysOnBottom: params.alwaysOnBottom ?? false,
    visibleOnAllWorkspaces: params.visibleOnAllWorkspaces ?? false,
    contentProtection: params.contentProtected ?? false,
    hasShadow: params.shadow ?? true,
    ...(params.backgroundColor ? { backgroundColor: params.backgroundColor } : {}),
    ...(params.titleBarHidden ? { titleBarStyle: 'hidden' } : {}),
    ...(params.titleBarOverlay ? { titleBarOverlay: true } : {}),
  }
  for (const key of Object.keys(creation)) {
    if (creation[key] === null) delete creation[key]
  }
  const window = new BrowserWindow({
    title: params.title ?? 'Electron Runtime',
    // Tauri sizes windows by their content, and `window.size` below resizes the
    // content too: without this the frame would eat the first few pixels.
    useContentSize: true,
    width: params.width ?? 800,
    height: params.height ?? 600,
    x: params.x ?? undefined,
    y: params.y ?? undefined,
    center: params.center ?? false,
    resizable: params.resizable ?? true,
    frame: params.decorations ?? true,
    transparent: params.transparent ?? false,
    show: params.visible ?? true,
    alwaysOnTop: params.alwaysOnTop ?? false,
    ...creation,
    autoHideMenuBar: true,
    ...(icon ? { icon } : {}),
    webPreferences: {
      contextIsolation: true,
      sandbox: true,
      nodeIntegration: false,
      preload: params.webview
        ? writeInitPreload(params.webview.webviewId, params.webview.initScripts)
        : path.join(__dirname, 'preload.cjs'),
      // Tauri pages embed guests with the Chromium `<webview>` element; Electron
      // disables that tag by default since v5.
      webviewTag: true,
    },
  })
  windows.set(windowId, window)
  // Electron has no constructor option for the maximized state.
  if (params.maximized) window.maximize()
  // Electron has no decorations getter, so remember what the window was created
  // with; `window.decorations` updates it when the Win32 styles are toggled.
  window.__tauriDecorations = params.decorations ?? true
  // Read this now: by the time `closed` fires the window and its webContents are
  // destroyed, and `window.webContents.id` throws "Object has been destroyed".
  const contentsId = window.webContents.id

  // Electron destroys the window on `close`; Tauri decides instead (an app may
  // `prevent_close` and keep the window around, e.g. hiding to the tray), so
  // hold the close and let the runtime answer with `window.destroy`.
  window.on('close', (event) => {
    if (closeAllowed.has(windowId)) return
    event.preventDefault()
    send({ event: 'window.closeRequested', params: { windowId } })
  })

  window.on('closed', () => {
    closeAllowed.delete(windowId)
    windows.delete(windowId)
    webviewByContents.delete(contentsId)
    for (const [webviewId, owner] of webviews) {
      if (owner !== window) continue
      const child = childWebviews.get(webviewId)
      if (child) {
        childWebviews.delete(webviewId)
        webviewByContents.delete(child.view.webContents.id)
        if (!child.view.webContents.isDestroyed()) child.view.webContents.close()
      }
      handlerPolicy.delete(webviewId)
      webviews.delete(webviewId)
    }
    send({ event: 'window.closed', params: { windowId } })
  })
  const emit = (event, extra = {}) => {
    if (window.isDestroyed()) return
    const geometry = readGeometry(window)
    send({ event: 'window.event', params: { windowId, event, ...geometry, ...extra } })
  }
  window.on('resize', () => resizeChildWebviews(window))
  for (const event of ['resize', 'move', 'focus', 'blur', 'maximize', 'unmaximize', 'enter-full-screen', 'leave-full-screen']) {
    window.on(event, () => emit(event))
  }
  window.webContents.on('did-finish-load', () => {
    log('loaded', window.webContents.getURL())
  })

  const webview = params.webview
  if (webview) return loadWebview(window, webview)
  return { windowId }
}

/** Forwards the webview lifecycle events Tauri's runtime handlers expect. */
function wireWebviewEvents(contents, webviewId) {
  if (!contents || contents.isDestroyed()) return
  contents.on('did-start-loading', () => {
    send({ event: 'webview.loadStarted', params: { webviewId, url: contents.getURL() } })
  })
  contents.on('did-finish-load', () => {
    send({ event: 'webview.loadFinished', params: { webviewId, url: contents.getURL() } })
  })
  contents.on('page-title-updated', (_event, title) => {
    send({ event: 'webview.titleChanged', params: { webviewId, title } })
  })
  contents.on('render-process-gone', (_event, details) => {
    send({
      event: 'webview.processGone',
      params: { webviewId, reason: details.reason, exitCode: details.exitCode },
    })
  })
  // `setWindowOpenHandler` can only answer synchronously, but the Rust run loop
  // is what decides — so deny here and let Tauri open its own window.
  contents.setWindowOpenHandler(({ url, features }) => {
    // Web pages open in the system browser, like they do under wry. Doing it
    // here rather than from the Tauri process keeps the launch in the process
    // that owns the window: Windows only lets a foreground app hand the
    // foreground to the browser it opens.
    if (/^(https?|mailto|tel):/i.test(url)) {
      shell.openExternal(url)
      return { action: 'deny' }
    }
    send({ event: 'webview.newWindow', params: { webviewId, url, features: features ?? '' } })
    return { action: 'deny' }
  })
}

/**
 * Writes a preload that installs the IPC bridge and runs Tauri's initialization
 * scripts in the page's main world.
 *
 * Tauri's scripts (`window.__TAURI_INTERNALS__`) must exist before the page's own
 * modules run, so they cannot be executed from `dom-ready`; a preload runs ahead
 * of every page script. The bridge goes through `contextBridge` because the
 * preload itself is sandboxed.
 */
function writeInitPreload(webviewId, initScripts) {
  const scripts = initScripts ?? []
  if (!scripts.length) return path.join(__dirname, 'preload.cjs')
  const file = path.join(os.tmpdir(), `tauri-runtime-electron-preload-${webviewId}.cjs`)
  const source = `${fs.readFileSync(path.join(__dirname, 'preload.cjs'), 'utf8')}
for (const source of ${JSON.stringify(scripts)}) {
  try {
    webFrame.executeJavaScript(source)
  } catch (error) {
    console.error('init script failed', error)
  }
}
`
  fs.writeFileSync(file, source)
  return file
}

function loadWebview(window, webview) {
  const { webviewId, url } = webview
  webviews.set(webviewId, window)
  webviewByContents.set(window.webContents.id, webviewId)
  recordPolicy(webviewId, webview.policy)
  wireWebviewEvents(window.webContents, webviewId)
  // Do not await: the page may need a `protocol.request` round trip to the Rust
  // runtime, which only starts pumping events after this call returns.
  window.loadURL(url).catch((error) => log('loadURL failed', url, serializeError(error)))
  return { windowId: webviewId }
}

/** Native children do not depend on a host page, its CSP, or its load finishing. */
function createChildWebview(window, webview) {
  const { webviewId, url, initScripts } = webview
  const view = new WebContentsView({
    webPreferences: {
      contextIsolation: true,
      sandbox: true,
      nodeIntegration: false,
      preload: writeInitPreload(webviewId, initScripts),
    },
  })
  const child = { view, autoResize: webview.autoResize || !webview.bounds }
  childWebviews.set(webviewId, child)
  webviews.set(webviewId, window)
  try {
    window.contentView.addChildView(view)
    webviewByContents.set(view.webContents.id, webviewId)
    recordPolicy(webviewId, webview.policy)
    wireWebviewEvents(view.webContents, webviewId)
    const content = window.getContentBounds()
    const position = webview.bounds?.position ?? { x: 0, y: 0 }
    const size = webview.bounds?.size ?? content
    setChildBounds(window, child, {
      x: toDip(window, position, 'x'), y: toDip(window, position, 'y'),
      width: toDip(window, size, 'width'), height: toDip(window, size, 'height'),
    })
    // Never wait for navigation here: local protocols need the Rust loop that
    // is currently waiting for this creation call to return.
    view.webContents.loadURL(url).catch((error) => log('loadURL failed', url, serializeError(error)))
    return { webviewId }
  } catch (error) {
    closeChildWebview(webviewId)
    throw error
  }
}

function rememberChildBounds(window, child) {
  child.reference = { ...child.view.getBounds(), parent: window.getContentBounds() }
}

function setChildBounds(window, child, bounds) {
  if (!Object.values(bounds).every(Number.isFinite) || bounds.width < 0 || bounds.height < 0) {
    throw new Error('invalid child webview bounds')
  }
  child.view.setBounds(bounds)
  rememberChildBounds(window, child)
}

/** Match Wry: both position and size scale relative to the parent's content. */
function resizeChildWebviews(window) {
  const parent = window.getContentBounds()
  if (parent.width <= 0 || parent.height <= 0) return
  for (const [id, child] of childWebviews) {
    if (webviews.get(id) !== window || !child.autoResize) continue
    const reference = child.reference
    if (reference.parent.width <= 0 || reference.parent.height <= 0) {
      rememberChildBounds(window, child)
      continue
    }
    const x = Math.round(reference.x * parent.width / reference.parent.width)
    const y = Math.round(reference.y * parent.height / reference.parent.height)
    // Scale edges so adjacent views still meet when the parent has odd dimensions.
    child.view.setBounds({
      x, y,
      width: Math.round((reference.x + reference.width) * parent.width / reference.parent.width) - x,
      height: Math.round((reference.y + reference.height) * parent.height / reference.parent.height) - y,
    })
  }
}

function closeChildWebview(webviewId) {
  const child = childWebviews.get(webviewId)
  if (!child) return
  const window = webviews.get(webviewId)
  if (window && !window.isDestroyed()) window.contentView.removeChildView(child.view)
  webviewByContents.delete(child.view.webContents.id)
  handlerPolicy.delete(webviewId)
  childWebviews.delete(webviewId)
  webviews.delete(webviewId)
  if (!child.view.webContents.isDestroyed()) child.view.webContents.close()
}

/** Route every dispatch to its own native child or the BrowserWindow main page. */
function contentsFor(webviewId) {
  const window = findWebview(webviewId)
  const contents = childWebviews.get(webviewId)?.view.webContents ?? window.webContents
  if (contents.isDestroyed()) throw new Error(`destroyed webview ${webviewId}`)
  return contents
}

function readWebviewBounds(webviewId) {
  const window = findWebview(webviewId)
  const child = childWebviews.get(webviewId)
  const scaleFactor = scaleOf(window)
  const content = window.getContentBounds()
  const rect = child ? child.view.getBounds() : { ...content, x: 0, y: 0 }
  const toPhysical = (value) => Math.round(value * scaleFactor)
  return {
    scaleFactor,
    // Webview positions are relative to their parent, never screen coordinates.
    bounds: { x: toPhysical(rect.x), y: toPhysical(rect.y), width: toPhysical(rect.width), height: toPhysical(rect.height) },
    contentBounds: { x: toPhysical(content.x), y: toPhysical(content.y), width: toPhysical(content.width), height: toPhysical(content.height) },
    visible: window.isVisible() && (!child || child.view.getVisible()),
  }
}

function findWebview(webviewId) {
  const window = webviews.get(webviewId)
  if (!window || window.isDestroyed()) throw new Error(`unknown webview ${webviewId}`)
  return window
}

function findWindow(windowId) {
  const window = windows.get(windowId)
  if (!window || window.isDestroyed()) throw new Error(`unknown window ${windowId}`)
  return window
}

function scaleOf(window) {
  return screen.getDisplayMatching(window.getBounds()).scaleFactor || 1
}

/** Tauri monitors are physical pixels, Electron displays are DIPs. */
function readDisplay(display) {
  const toPhysical = (rect) => ({
    x: Math.round(rect.x * display.scaleFactor),
    y: Math.round(rect.y * display.scaleFactor),
    width: Math.round(rect.width * display.scaleFactor),
    height: Math.round(rect.height * display.scaleFactor),
  })
  return {
    name: display.label,
    position: toPhysical(display.bounds),
    size: {
      width: toPhysical(display.bounds).width,
      height: toPhysical(display.bounds).height,
    },
    workArea: {
      position: toPhysical(display.workArea),
      size: {
        width: toPhysical(display.workArea).width,
        height: toPhysical(display.workArea).height,
      },
    },
    scaleFactor: display.scaleFactor,
  }
}

/**
 * Tauri hands over raw RGBA pixels; Electron's bitmap buffers are BGRA on the
 * platforms we ship, so swap the red and blue channels before handing them
 * over. Returns null when the payload has no icon.
 */
function imageFromPayload(payload) {
  if (!payload) return null
  const { width, height, rgba } = payload
  if (!Array.isArray(rgba) || !width || !height) return null
  const bitmap = Buffer.from(rgba)
  for (let index = 0; index + 3 < bitmap.length; index += 4) {
    const red = bitmap[index]
    bitmap[index] = bitmap[index + 2]
    bitmap[index + 2] = red
  }
  return nativeImage.createFromBitmap(bitmap, { width, height })
}

function readGeometry(window) {
  const scaleFactor = scaleOf(window)
  const toPhysical = (rect) => ({
    x: Math.round(rect.x * scaleFactor),
    y: Math.round(rect.y * scaleFactor),
    width: Math.round(rect.width * scaleFactor),
    height: Math.round(rect.height * scaleFactor),
  })
  return {
    scaleFactor,
    bounds: toPhysical(window.getBounds()),
    contentBounds: toPhysical(window.getContentBounds()),
    maximized: window.isMaximized(),
    minimized: window.isMinimized(),
    focused: window.isFocused(),
    visible: window.isVisible(),
    fullscreen: window.isFullScreen(),
    resizable: window.isResizable(),
    alwaysOnTop: window.isAlwaysOnTop(),
    decorated: window.__tauriDecorations ?? true,
    title: window.getTitle(),
    // Electron only ships these getters on macOS and Windows.
    maximizable: window.isMaximizable?.() ?? true,
    minimizable: window.isMinimizable?.() ?? true,
    closable: window.isClosable?.() ?? true,
  }
}

/** Tauri sizes and positions are physical pixels; Electron's are DIPs. */
function toDip(window, params, key) {
  const value = params[key]
  if (typeof value !== 'number') return undefined
  return params.physical ? Math.round(value / scaleOf(window)) : Math.round(value)
}

function setWindowSize(window, params) {
  const width = toDip(window, params, 'width')
  const height = toDip(window, params, 'height')
  if (width === undefined || height === undefined) return
  window.setContentSize(Math.max(1, width), Math.max(1, height))
}

function setWindowPosition(window, params) {
  const x = toDip(window, params, 'x')
  const y = toDip(window, params, 'y')
  if (x === undefined || y === undefined) return
  window.setPosition(x, y)
}

function handle(method, params) {
  switch (method) {
    case 'window.create':
      return createWindow(params)
    case 'webview.create': {
      const window = windows.get(params.windowId)
      if (!window) throw new Error(`unknown window ${params.windowId}`)
      return createChildWebview(window, params)
    }
    case 'webview.eval': {
      return contentsFor(params.webviewId).executeJavaScript(params.script, true)
    }
    case 'window.setTitle': {
      const window = windows.get(params.windowId)
      if (window) window.setTitle(params.title)
      return null
    }
    case 'window.geometry':
      return readGeometry(findWindow(params.windowId))
    case 'window.size':
      setWindowSize(findWindow(params.windowId), params)
      return null
    case 'window.position':
      setWindowPosition(findWindow(params.windowId), params)
      return null
    case 'window.minSize': {
      const window = findWindow(params.windowId)
      if (!params.width) return window.setMinimumSize(0, 0)
      return window.setMinimumSize(
        toDip(window, params, 'width') ?? 0,
        toDip(window, params, 'height') ?? 0,
      )
    }
    case 'window.maxSize': {
      const window = findWindow(params.windowId)
      if (!params.width) return window.setMaximumSize(0, 0)
      return window.setMaximumSize(
        toDip(window, params, 'width') ?? 0,
        toDip(window, params, 'height') ?? 0,
      )
    }
    case 'window.resizable':
      findWindow(params.windowId).setResizable(params.resizable)
      return null
    case 'window.maximizable':
      findWindow(params.windowId).setMaximizable(params.maximizable)
      return null
    case 'window.minimizable':
      findWindow(params.windowId).setMinimizable(params.minimizable)
      return null
    case 'window.closable':
      findWindow(params.windowId).setClosable(params.closable)
      return null
    case 'window.maximize':
      findWindow(params.windowId).maximize()
      return null
    case 'window.unmaximize':
      findWindow(params.windowId).unmaximize()
      return null
    case 'window.minimize':
      findWindow(params.windowId).minimize()
      return null
    case 'window.unminimize':
      findWindow(params.windowId).restore()
      return null
    case 'window.fullscreen':
      findWindow(params.windowId).setFullScreen(params.fullscreen)
      return null
    case 'window.alwaysOnTop':
      findWindow(params.windowId).setAlwaysOnTop(params.alwaysOnTop)
      return null
    case 'window.cursor': {
      // Each native child has its own document; a host page's CSS cannot reach it.
      const window = findWindow(params.windowId)
      const contents = []
      for (const [id, owner] of webviews) {
        if (owner === window) contents.push(contentsFor(id))
      }
      const script = `document.documentElement.style.cursor = ${JSON.stringify(params.cursor)}`
      return Promise.all(contents.filter((item) => !item.isDestroyed()).map((item) =>
        item.executeJavaScript(script),
      )).then(() => null)
    }
    case 'window.skipTaskbar':
      findWindow(params.windowId).setSkipTaskbar(params.skip)
      return null
    case 'window.focusable':
      findWindow(params.windowId).setFocusable(params.focusable)
      return null
    case 'window.contentProtected':
      findWindow(params.windowId).setContentProtection(params.protected)
      return null
    case 'window.center':
      findWindow(params.windowId).center()
      return null
    case 'window.requestAttention':
      findWindow(params.windowId).flashFrame(Boolean(params.flash))
      return null
    case 'webview.downloadResponse': {
      const item = downloadRequests.get(params.id)
      downloadRequests.delete(params.id)
      const entry = downloadPaths.get(params.id)
      if (!item) return null
      if (!params.allow) {
        item.cancel()
        if (entry) {
          if (fs.existsSync(entry.provisional)) fs.rmSync(entry.provisional, { force: true })
          downloadPaths.delete(params.id)
        }
        return null
      }
      if (entry && params.destination) entry.finalPath = params.destination
      try {
        if (params.destination) item.setSavePath(params.destination)
        log('download.answer', item.getState(), item.getSavePath())
      } catch (error) {
        log('download.answer failed', serializeError(error))
        item.cancel()
      }
      return null
    }
    case 'webview.permissionResponse': {
      const callback = permissionRequests.get(params.id)
      permissionRequests.delete(params.id)
      // `default` keeps Electron's own behaviour, which is to deny.
      if (callback) callback(params.response === 'allow')
      return null
    }
    case 'webview.navigate':
      return contentsFor(params.webviewId).loadURL(params.url).then(() => null)
    case 'webview.openUrl': {
      // `NewWindowResponse::Allow`: the engine, not Tauri, owns this window.
      const popup = new BrowserWindow({
        width: params.width ?? 800,
        height: params.height ?? 600,
        webPreferences: { contextIsolation: true, sandbox: true, nodeIntegration: false },
      })
      popup.loadURL(params.url).catch((error) => log('popup load failed', serializeError(error)))
      return { windowId: popup.id }
    }
    case 'webview.reload':
      contentsFor(params.webviewId).reload()
      return null
    case 'webview.setAutoResize': {
      const window = findWebview(params.webviewId)
      const child = childWebviews.get(params.webviewId)
      if (child) {
        child.autoResize = Boolean(params.autoResize)
        rememberChildBounds(window, child)
      }
      return null
    }
    case 'webview.goBack':
      // canGoBack()/goBack() on webContents are deprecated; the history object is not.
      if (contentsFor(params.webviewId).navigationHistory.canGoBack()) {
        contentsFor(params.webviewId).navigationHistory.goBack()
      }
      return null
    case 'webview.canGoBack':
      return contentsFor(params.webviewId).navigationHistory.canGoBack()
    case 'webview.goForward':
      if (contentsFor(params.webviewId).navigationHistory.canGoForward()) {
        contentsFor(params.webviewId).navigationHistory.goForward()
      }
      return null
    case 'webview.canGoForward':
      return contentsFor(params.webviewId).navigationHistory.canGoForward()
    case 'webview.openDevtools':
      contentsFor(params.webviewId).openDevTools({ mode: 'detach' })
      return null
    case 'webview.closeDevtools':
      contentsFor(params.webviewId).closeDevTools()
      return null
    case 'webview.isDevtoolsOpen':
      return contentsFor(params.webviewId).isDevToolsOpened()
    case 'webview.setZoom':
      contentsFor(params.webviewId).setZoomFactor(params.scaleFactor)
      return null
    case 'window.show':
      windows.get(params.windowId)?.show()
      return null
    case 'window.hide':
      windows.get(params.windowId)?.hide()
      return null
    case 'window.focus':
      windows.get(params.windowId)?.focus()
      return null
    case 'window.nativeHandle': {
      // Tauri's host-side code (menus, window effects, Win32 tweaks) needs the
      // native handle; the Buffer holds the HWND / NSView / X11 window id.
      const window = findWindow(params.windowId)
      const buffer = window.getNativeWindowHandle()
      const handle =
        buffer.length >= 8 ? Number(buffer.readBigUInt64LE(0)) : buffer.readUInt32LE(0)
      return { handle }
    }
    case 'window.close':
      windows.get(params.windowId)?.close()
      return null
    case 'window.destroy':
      closeAllowed.add(params.windowId)
      windows.get(params.windowId)?.destroy()
      return null
    case 'window.visibleOnAllWorkspaces':
      findWindow(params.windowId).setVisibleOnAllWorkspaces(params.value, {
        visibleOnFullScreen: true,
      })
      return null
    case 'window.icon': {
      const image = imageFromPayload(params.icon)
      if (image) findWindow(params.windowId).setIcon(image)
      return null
    }
    case 'window.enabled':
      findWindow(params.windowId).setEnabled(params.enabled)
      return null
    case 'window.enabledQuery':
      return findWindow(params.windowId).isEnabled()
    case 'window.simpleFullscreen': {
      const window = findWindow(params.windowId)
      if (typeof window.setSimpleFullScreen === 'function') {
        window.setSimpleFullScreen(params.enable)
      }
      return null
    }
    case 'window.progressBar': {
      const window = findWindow(params.windowId)
      window.setProgressBar(params.state === 'none' ? -1 : Number(params.progress) || 0, {
        mode: params.state === 'paused' ? 'paused' : params.state === 'error' ? 'error' : 'normal',
      })
      return null
    }
    case 'window.shadow':
      findWindow(params.windowId).setHasShadow(params.value)
      return null
    case 'window.decorations':
      // The frame itself is toggled from Rust through the Win32 styles; this only
      // keeps readGeometry honest about it.
      findWindow(params.windowId).__tauriDecorations = Boolean(params.value)
      return null
    case 'app.badgeLabel':
      // Only macOS has a dock badge label; Windows has the count above.
      if (process.platform === 'darwin' && app.dock) app.dock.setBadge(params.label ?? '')
      return null
    case 'window.badgeCount':
      app.setBadgeCount(params.count || 0)
      return null
    case 'window.overlayIcon': {
      const image = imageFromPayload(params.icon)
      findWindow(params.windowId).setOverlayIcon(image, params.description || '')
      return null
    }
    case 'window.backgroundColor':
      findWindow(params.windowId).setBackgroundColor(params.color)
      return null
    case 'window.theme': {
      nativeTheme.themeSource = params.theme === 'dark' ? 'dark' : params.theme === 'light' ? 'light' : 'system'
      return null
    }
    case 'window.ignoreCursorEvents':
      findWindow(params.windowId).setIgnoreMouseEvents(params.ignore, { forward: true })
      return null
    case 'window.themeQuery':
      return { theme: nativeTheme.shouldUseDarkColors ? 'dark' : 'light' }
    case 'app.focus':
      app.focus({ steal: params.steal === true })
      return null
    case 'app.show':

      // macOS shows/hides the whole application; elsewhere these are no-ops.
      if (process.platform === 'darwin') app.show()
      return null
    case 'app.hide':
      if (process.platform === 'darwin') app.hide()
      return null
    case 'app.activationPolicy':
      if (process.platform === 'darwin') app.setActivationPolicy(params.policy)
      return null
    case 'app.dockVisibility':
      if (process.platform === 'darwin') {
        app.dock[params.visible ? 'show' : 'hide']()
      }
      return null
    case 'app.theme': {
      nativeTheme.themeSource =
        params.theme === 'dark' ? 'dark' : params.theme === 'light' ? 'light' : 'system'
      return null
    }
    case 'window.displays':
      return {
        displays: screen.getAllDisplays().map(readDisplay),
        primary: readDisplay(screen.getPrimaryDisplay()),
      }
    case 'window.cursorPosition': {
      const point = screen.getCursorScreenPoint()
      const scale = screen.getDisplayNearestPoint(point).scaleFactor
      return { x: Math.round(point.x * scale), y: Math.round(point.y * scale) }
    }
    case 'window.currentDisplay':
      return readDisplay(screen.getDisplayMatching(findWindow(params.windowId).getBounds()))
    case 'window.displayForPoint':
      return readDisplay(screen.getDisplayNearestPoint({ x: params.x, y: params.y }))
    case 'window.trafficLight': {
      const window = findWindow(params.windowId)
      if (typeof window.setWindowButtonPosition === 'function') {
        window.setWindowButtonPosition({ x: params.x, y: params.y })
      }
      return null
    }
    case 'webview.bounds':
      return readWebviewBounds(params.webviewId)
    case 'webview.setBounds': {
      const window = findWebview(params.webviewId)
      const child = childWebviews.get(params.webviewId)
      if (!child) return null
      const bounds = child.view.getBounds()
      if (params.position) {
        bounds.x = toDip(window, params.position, 'x')
        bounds.y = toDip(window, params.position, 'y')
      }
      if (params.size) {
        bounds.width = toDip(window, params.size, 'width')
        bounds.height = toDip(window, params.size, 'height')
      }
      setChildBounds(window, child, bounds)
      return null
    }
    case 'webview.visibility': {
      const window = findWebview(params.webviewId)
      const child = childWebviews.get(params.webviewId)
      if (child) child.view.setVisible(Boolean(params.visible))
      else if (params.visible) window.show()
      else window.hide()
      return null
    }
    case 'webview.backgroundColor': {
      const child = childWebviews.get(params.webviewId)
      findWebview(params.webviewId)
      if (child) child.view.setBackgroundColor(params.color ?? '#00000000')
      return null
    }
    case 'webview.focus': {
      findWebview(params.webviewId).focus()
      contentsFor(params.webviewId).focus()
      return null
    }
    case 'webview.print': {
      contentsFor(params.webviewId).print({ silent: false })
      return null
    }
    case 'webview.close': {
      if (childWebviews.has(params.webviewId)) closeChildWebview(params.webviewId)
      else findWebview(params.webviewId).close()
      return null
    }
    case 'webview.cookies':
      return contentsFor(params.webviewId).session.cookies.get(params.url ? { url: params.url } : {})
    case 'webview.setCookie': {
      const { url, name, value, domain, path: cookiePath, secure, httpOnly, sameSite, expirationDate } = params
      return contentsFor(params.webviewId)
        .session.cookies.set({
          url,
          name,
          value,
          ...(domain ? { domain } : {}),
          ...(cookiePath ? { path: cookiePath } : {}),
          ...(secure !== undefined ? { secure } : {}),
          ...(httpOnly !== undefined ? { httpOnly } : {}),
          ...(sameSite ? { sameSite } : {}),
          ...(expirationDate ? { expirationDate } : {}),
        })
        .then(() => null)
    }
    case 'webview.deleteCookie':
      return contentsFor(params.webviewId).session.cookies.remove(params.url, params.name).then(() => null)
    case 'webview.clearBrowsingData':
      return contentsFor(params.webviewId).session.clearStorageData().then(() => null)
    case 'protocol.register': {
      const { scheme } = params
      if (registeredSchemes.has(scheme)) return null
      protocol.handle(scheme, (request) => dispatchProtocol(scheme, request))
      registeredSchemes.add(scheme)
      installLocalhostProtocols()
      return null
    }
    case 'protocol.respond': {
      const resolve = protocolRequests.get(params.id)
      protocolRequests.delete(params.id)
      if (resolve) resolve(params)
      return null
    }
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

  Promise.resolve()
    .then(() => handle(message.method, message.params ?? {}))
    .then((result) => {
      send({ id: message.id, result: result ?? null })
    })
    .catch((error) => {
      log('error', message.method, serializeError(error))
      send({ id: message.id, error: serializeError(error) })
    })
}

ipcMain.handle('tauri-ipc-fetch', async (event, data) => {
  const frame = event.senderFrame
  const webviewId = webviewByContents.get(event.sender.id)
  if (!frame || frame !== event.sender.mainFrame || frame.origin === 'null' || webviewId === undefined) {
    throw new Error('unknown IPC sender')
  }
  const url = new URL(data.url)
  if (url.username || url.password || url.port || !(
    (url.protocol === 'ipc:' && url.hostname === 'localhost') ||
    (/^https?:$/.test(url.protocol) && url.hostname === 'ipc.localhost')
  ) || data.method !== 'POST' || !(data.body instanceof Uint8Array)) {
    throw new Error('invalid IPC request')
  }
  const headers = new Headers(data.headers)
  // The renderer can choose payload/headers, never its ACL origin or webview.
  headers.set('Origin', frame.origin)
  const request = new Request(url, { method: 'POST', headers, body: data.body })
  const response = await dispatchProtocol('ipc', request, `ipc://localhost${url.pathname}${url.search}`, webviewId)
  return {
    status: response.status, headers: [...response.headers],
    body: new Uint8Array(await response.arrayBuffer()),
  }
})

ipcMain.on('tauri-ipc', (event, data) => {
  // Only the frame that received Tauri's main-frame preload may invoke it.
  // Do not relabel an iframe's URL (including an opaque origin) as its parent.
  const frame = event.senderFrame
  if (!frame || frame !== event.sender.mainFrame || frame.origin === 'null' || typeof data !== 'string') return
  const webviewId = webviewByContents.get(event.sender.id)
  if (webviewId === undefined) {
    log('ipc message from unknown contents', event.sender.id)
    return
  }
  send({
    event: 'ipc.message',
    params: { webviewId, url: frame.url, body: data },
  })
})

app.whenReady().then(async () => {
  // Windows groups taskbar buttons by AppUserModelID; without the app's own id
  // the window inherits Electron's identity.
  if (appIdentifier) app.setAppUserModelId(appIdentifier)
  // Tauri apps draw their own chrome; Electron's default File/Edit/View menu is
  // noise here.
  Menu.setApplicationMenu(null)
  // Covers the window's own contents and every `<webview>` guest created later.
  app.on('web-contents-created', (_event, contents) => {
    installDevtoolsShortcut(contents)
    installContextMenu(contents)
  })
  installPolicyHandlers()
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
  socket.on('error', () => {
    app.quit()
  })
})

// The Rust side owns the lifetime: no window means keep running until the
// socket closes.
app.on('window-all-closed', () => {})
