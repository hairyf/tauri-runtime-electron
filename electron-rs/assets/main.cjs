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
  webContents,
} = require('electron')
const net = require('node:net')
const fs = require('node:fs')
const os = require('node:os')
const path = require('node:path')

// Electron keys its profile by app name, and that name is "Electron" for every
// app built on this runtime: without this, two running apps would share one
// profile, cache and lock.
const appName = process.env.ELECTRON_RS_APP_NAME
if (appName) app.setName(appName)
const owner = process.env.TAURI_RUNTIME_ELECTRON_OWNER
if (owner) {
  const directory = path.join(app.getPath('appData'), owner)
  fs.mkdirSync(directory, { recursive: true })
  app.setPath('userData', directory)
}

const logFile = path.join(os.tmpdir(), 'tauri-runtime-electron.log')
function log(...args) {
  fs.appendFileSync(logFile, `${new Date().toISOString()} ${args.join(' ')}\n`)
}

function trace(...args) {
  if (process.env.TAURI_RUNTIME_ELECTRON_TRACE === '1') console.error('[electron trace]', new Date().toISOString(), ...args)
}

const openingDevtools = new WeakMap()

function openDevtools(contents) {
  const pending = openingDevtools.get(contents)
  if (pending) return pending
  // Visibility can become true before the frontend finishes loading. Electron
  // exposes devToolsWebContents only after its devtools-opened lifecycle step.
  if (contents.devToolsWebContents) return null
  const contentsId = contents.id
  let finish
  const opening = new Promise((resolve, reject) => {
    finish = (error) => {
      clearTimeout(timer)
      contents.removeListener('devtools-opened', opened)
      contents.removeListener('devtools-closed', closed)
      contents.removeListener('destroyed', destroyed)
      openingDevtools.delete(contents)
      trace('devtools.open.finished', contentsId, error ? serializeError(error) : 'ready')
      if (error) reject(error)
      else resolve(null)
    }
    const opened = () => finish()
    const closed = () => finish(new Error('DevTools closed before opening completed'))
    const destroyed = () => finish(new Error('webview destroyed while opening DevTools'))
    const timer = setTimeout(() => finish(new Error('DevTools opening timed out')), 10_000)
    contents.once('devtools-opened', opened)
    contents.once('devtools-closed', closed)
    contents.once('destroyed', destroyed)
  })
  openingDevtools.set(contents, opening)
  try {
    trace('devtools.open.enter', contents.id)
    contents.openDevTools({ mode: 'detach' })
    trace('devtools.open.return', contents.id)
  } catch (error) { finish(error) }
  return opening
}

/** F12 / Ctrl+Shift+I toggle the DevTools of the focused renderer. */
function installDevtoolsShortcut(contents) {
  contents.on('before-input-event', (event, input) => {
    if (input.type !== 'keyDown') return
    const f12 = input.key === 'F12'
    const inspect = input.control && input.shift && input.key.toLowerCase() === 'i'
    if (!f12 && !inspect) return
    event.preventDefault()
    if (contents.isDevToolsOpened() && !openingDevtools.has(contents)) contents.closeDevTools()
    else Promise.resolve(openDevtools(contents)).catch((error) => log('devtools open failed', serializeError(error)))
    setTimeout(() => { if (!contents.isDestroyed()) log('devtools', contents.id, contents.isDevToolsOpened()) }, 300)
  })
}

/** Right-click menu; Electron ships none, the BrowserWindow only has a page. */
function installContextMenu(contents) {
  contents.on('context-menu', (_event, params) => {
    const items = []
    // Opening links belongs to the host's new-window policy, not a runtime
    // context-menu shortcut that bypasses it. Keep only ordinary editing here.
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
const appIdentifier = readArg('--tauri-identifier') || process.env.ELECTRON_RS_IDENTIFIER || undefined

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

const localhostProtocolSessions = new WeakSet()
const policySessions = new WeakSet()
const navigationRequests = new Map()
let nextNavigationRequest = 0

/** Keep WebView2's virtual HTTP(S) origin: isolation and CSP depend on it. */
function installLocalhostProtocols(target = protocol, network = electronNet) {
  if (localhostProtocolSessions.has(target)) return
  for (const transport of ['http', 'https']) {
    target.handle(transport, (request) => {
      const url = new URL(request.url)
      const match = /^([a-z][a-z0-9+.-]*)\.localhost$/.exec(url.hostname)
      // A dev server on a real port, or any unregistered host, stays ordinary HTTP.
      if (url.port || !match || !registeredSchemes.has(match[1])) {
        return network.fetch(request, { bypassCustomProtocolHandlers: true })
      }
      // Only the resolver sees the native scheme, just like Wry's WebView2 adapter.
      const uri = `${match[1]}://localhost${url.pathname}${url.search}`
      return dispatchProtocol(match[1], request, uri)
    })
  }
  localhostProtocolSessions.add(target)
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
const closableState = new WeakMap()
const maximizableState = new WeakMap()
const minimizableState = new WeakMap()
// Constraints describe content pixels; Electron's native limits include its frame.
const sizeConstraints = new WeakMap()
const windowCursorState = new WeakMap()
const cursorCssKey = new WeakMap()
const windowConfigs = new WeakMap()
/** webviewId -> BrowserWindow */
const webviews = new Map()
/** webContents id -> webviewId (for routing `tauri-ipc` messages) */
const webviewByContents = new Map()
/** webviewId -> native child view and its resize reference (main pages stay on BrowserWindow). */
const childWebviews = new Map()
/** webviewId -> the out-of-band callbacks the runtime registered */
const handlerPolicy = new Map()
/** request id -> staged download, joined with its first host decision and done state */
const downloadRequests = new Map()

function cleanDownloadStage(entry) {
  try {
    fs.rmSync(entry.directory, { recursive: true, force: true })
  } catch (error) {
    log('download.cleanup failed', entry.directory, serializeError(error))
  }
}

function finishDownload(id) {
  const entry = downloadRequests.get(id)
  if (!entry || entry.allow === undefined || entry.state === undefined) return
  downloadRequests.delete(id)
  clearTimeout(entry.timer)
  entry.contents.removeListener('destroyed', entry.onDestroyed)
  let success = false
  if (entry.allow && entry.state === 'completed') {
    try {
      // Exclusive copying works across devices and never replaces existing data.
      fs.copyFileSync(entry.provisional, entry.finalPath, fs.constants.COPYFILE_EXCL)
      success = true
    } catch (error) {
      // Keep the completed bytes recoverable if publishing fails.
      log('download.publish failed; staged data retained', entry.provisional, serializeError(error))
    }
  }
  if (success || !entry.allow || entry.state !== 'completed') cleanDownloadStage(entry)
  send({
    event: 'download.finished',
    params: { webviewId: entry.webviewId, url: entry.url, path: success ? entry.finalPath : null, success },
  })
}
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
  if (policy.download || policy.permission || policy.navigation) handlerPolicy.set(webviewId, policy)
}

/**
 * Downloads and permission prompts are decided by the Rust run loop, so they are
 * forwarded to it and answered later instead of blocking the main thread.
 */
function installPolicyHandlers(target = session.defaultSession) {
  if (policySessions.has(target)) return
  policySessions.add(target)
  const pendingNavigations = new Map()
  const retireNavigation = (details) => {
    const finish = pendingNavigations.get(details.id)
    if (finish) finish(false, true)
  }
  target.webRequest.onCompleted(retireNavigation)
  target.webRequest.onErrorOccurred(retireNavigation)
  // Pause the real request: preserve POST data, redirects and initiating frame.
  target.webRequest.onBeforeRequest((details, callback) => {
    // Requests raised without a page (protocol handlers, net.fetch) have no
    // navigation policy, and `webContents.fromId(undefined)` throws: an
    // uncaught main-process error opens a modal dialog that stalls every RPC.
    const contents = details.webContents
      ?? (details.webContentsId === undefined ? undefined : webContents.fromId(details.webContentsId))
    if (!['mainFrame', 'subFrame'].includes(details.resourceType) || !policyFor(contents)?.navigation) {
      callback({})
      return
    }
    const id = ++nextNavigationRequest
    const finish = (allow, retired = false) => {
      const entry = navigationRequests.get(id)
      if (!entry) return
      navigationRequests.delete(id)
      pendingNavigations.delete(details.id)
      clearTimeout(entry.timer)
      contents.removeListener('destroyed', entry.onDestroyed)
      // An ended request must not let a late decision stop a newer navigation.
      if (retired) return
      // Native cancel alone can commit ERR_BLOCKED_BY_CLIENT. Stop main-frame
      // navigation first to retain its document, never the whole page for an iframe.
      // ponytail: denied iframe may show an error page; retention needs a native async navigation throttle.
      try {
        if (!allow && details.resourceType === 'mainFrame' && !contents.isDestroyed()) contents.stop()
      } finally {
        callback({ cancel: !allow })
      }
    }
    const onDestroyed = () => finish(false)
    const timer = setTimeout(onDestroyed, 15_000)
    timer.unref()
    navigationRequests.set(id, { finish, timer, onDestroyed })
    pendingNavigations.set(details.id, finish)
    contents.once('destroyed', onDestroyed)
    send({ event: 'webview.navigationRequested', params: {
      id, webviewId: webviewIdFor(contents), url: details.url,
    } })
  })
  target.on('will-download', (event, item, contents) => {
    const webviewId = contents && webviewByContents.get(contents.id)
    const policy = webviewId === undefined ? undefined : handlerPolicy.get(webviewId)
    if (!policy || !policy.download) return
    const id = ++nextDownloadRequest
    const entry = {
      item, contents, webviewId, url: item.getURL(),
      finalPath: item.getSavePath() || path.join(app.getPath('downloads'), path.basename(item.getFilename())),
    }
    try {
      entry.directory = fs.mkdtempSync(path.join(os.tmpdir(), 'electron-download-'))
      entry.provisional = path.join(entry.directory, 'download')
      // Electron only permits setSavePath during this synchronous callback.
      item.setSavePath(entry.provisional)
    } catch (error) {
      event.preventDefault()
      if (entry.directory) cleanDownloadStage(entry)
      log('download.stage failed', serializeError(error))
      send({ event: 'download.finished', params: { webviewId, url: entry.url, path: null, success: false } })
      return
    }
    downloadRequests.set(id, entry)
    entry.onDestroyed = () => {
      entry.allow = false
      try { if (entry.state === undefined) item.cancel() }
      catch (error) { log('download.cancel failed', serializeError(error)) }
      entry.state ??= 'cancelled'
      finishDownload(id)
    }
    contents.once('destroyed', entry.onDestroyed)
    // A lost host reply must not leave private data or a live item indefinitely.
    entry.timer = setTimeout(entry.onDestroyed, 30_000)
    entry.timer.unref()
    item.once('done', (_done, state) => {
      entry.state = state
      // Cancellation may release its Windows file handle after destruction.
      if (!downloadRequests.has(id)) cleanDownloadStage(entry)
      else finishDownload(id)
    })
    send({
      event: 'download.requested',
      params: { id, webviewId, url: entry.url, destination: entry.finalPath },
    })
  })

  target.setPermissionRequestHandler((contents, permission, callback, details) => {
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

const customSessions = new Set()
let nextIncognitoSession = 0

function sessionFor(webview = {}) {
  if (!webview.incognito && !webview.dataDirectory) return undefined
  const target = webview.incognito
    ? session.fromPartition(`tauri-private-${++nextIncognitoSession}`)
    : session.fromPath(path.resolve(webview.dataDirectory))
  if (!customSessions.has(target)) {
    customSessions.add(target)
    installPolicyHandlers(target)
    for (const scheme of registeredSchemes) {
      target.protocol.handle(scheme, (request) => dispatchProtocol(scheme, request))
    }
    installLocalhostProtocols(target.protocol, target)
  }
  return target
}

function cssColor(color) {
  if (typeof color === 'string') return color
  if (!Array.isArray(color) || color.length < 3) return undefined
  const [red, green, blue, alpha = 255] = color
  return `rgba(${red}, ${green}, ${blue}, ${(alpha / 255).toFixed(3)})`
}

function titleBarStyleOf(config) {
  if (config.hiddenTitle || config.titleBarHidden || config.titleBarStyle === 'transparent') return 'hidden'
  return config.titleBarStyle === 'overlay' ? 'hiddenInset' : 'default'
}

function webviewConfig(config, webview = {}) {
  return { ...config, ...Object.fromEntries(Object.entries(webview).filter(([, value]) => value != null)) }
}

function webPreferences(config, webview = {}) {
  return {
    contextIsolation: true,
    sandbox: true,
    nodeIntegration: false,
    // Subframes receive the sandboxed script runner, never Node or Tauri IPC.
    nodeIntegrationInSubFrames: true,
    session: sessionFor(config),
    preload: writeInitPreload(webview.webviewId, webview.initScripts),
    javascript: config.javascriptDisabled !== true,
    devTools: config.devtools !== false,
    backgroundThrottling: config.backgroundThrottling !== 'disabled',
  }
}

function browserWindowOptions(config, params) {
  let icon
  if (params.windowIcon) {
    const { width, height, bgra } = params.windowIcon
    const bitmap = Buffer.from(bgra, 'base64')
    if (!Number.isSafeInteger(width) || !Number.isSafeInteger(height) || width <= 0 || height <= 0 || bitmap.length !== width * height * 4) {
      throw new Error('invalid window icon')
    }
    icon = nativeImage.createFromBitmap(bitmap, { width, height, scaleFactor: 1 })
  } else if (typeof params.icon === 'string') {
    // Electron's image loader does not accept Windows verbatim path prefixes.
    icon = params.icon.replace(/^\\\\\?\\/, '')
  }
  const options = {
    title: config.title ?? 'Electron Runtime',
    useContentSize: true,
    width: Math.max(1, Math.round(config.width ?? 800)),
    height: Math.max(1, Math.round(config.height ?? 600)),
    x: config.x == null ? undefined : Math.round(config.x),
    y: config.y == null ? undefined : Math.round(config.y),
    center: config.center ?? false,
    resizable: config.resizable ?? true,
    frame: config.decorations ?? true,
    transparent: config.transparent ?? false,
    show: config.visible ?? true,
    fullscreen: config.fullscreen ?? false,
    maximizable: config.maximizable ?? true,
    minimizable: config.minimizable ?? true,
    closable: config.closable ?? true,
    focusable: config.focusable ?? true,
    alwaysOnTop: config.alwaysOnTop ?? false,
    alwaysOnBottom: config.alwaysOnBottom ?? false,
    skipTaskbar: config.skipTaskbar ?? false,
    hasShadow: config.shadow ?? true,
    autoHideMenuBar: true,
    titleBarStyle: titleBarStyleOf(config),
    acceptFirstMouse: webviewConfig(config, params.webview).acceptFirstMouse === true,
    ...(cssColor(config.backgroundColor) ? { backgroundColor: cssColor(config.backgroundColor) } : {}),
    ...(config.titleBarOverlay ? { titleBarOverlay: true } : {}),
    ...(config.trafficLightPosition ? { trafficLightPosition: config.trafficLightPosition } : {}),
    ...(config.tabbingIdentifier ? { tabbingIdentifier: config.tabbingIdentifier } : {}),
    ...(icon ? { icon } : {}),
    webPreferences: { ...webPreferences(webviewConfig(config, params.webview), params.webview), webviewTag: true },
  }
  if (process.platform === 'win32') {
    const effects = config.windowEffects?.effects ?? []
    const material = effects.some((effect) => ['mica', 'micaDark', 'micaLight'].includes(effect)) ? 'mica'
      : effects.some((effect) => ['tabbed', 'tabbedDark', 'tabbedLight'].includes(effect)) ? 'tabbed'
        : effects.some((effect) => ['acrylic', 'blur'].includes(effect)) ? 'acrylic' : undefined
    if (material) options.backgroundMaterial = material
  }
  return options
}

function applyWebviewConfig(contents, config) {
  if (config.userAgent != null) contents.setUserAgent(config.userAgent)
  if (config.dragDropEnabled === false) {
    contents.on('did-finish-load', () => {
      contents.mainFrame.executeJavaScript(
        "for (const type of ['dragenter', 'dragover', 'drop']) document.addEventListener(type, (event) => { event.preventDefault(); event.stopPropagation() }, true)",
      ).catch((error) => log('webview.dragDrop', serializeError(error)))
    })
  }
  if (config.zoomHotkeysEnabled === false) {
    contents.on('dom-ready', () => {
      contents.setVisualZoomLevelLimits(1, 1).catch((error) => log('webview.zoom', serializeError(error)))
    })
    contents.on('before-input-event', (event, input) => {
      if ((input.control || input.meta) && ['+', '-', '=', '0'].includes(input.key)) event.preventDefault()
    })
  }
  if (config.scrollBarStyle === 'fluentOverlay') {
    contents.on('did-finish-load', () => {
      contents.insertCSS('::-webkit-scrollbar { width: 8px; height: 8px } ::-webkit-scrollbar-track { background: transparent } ::-webkit-scrollbar-thumb { background: rgba(128, 128, 128, 0.45); border-radius: 4px }')
        .catch((error) => log('webview.scrollbar', serializeError(error)))
    })
  }
}

function applyWindowConfig(window, config, params) {
  const constraints = {}
  for (const key of ['minWidth', 'minHeight', 'maxWidth', 'maxHeight']) {
    constraints[key] = constraintValue(params.sizeConstraints && Object.hasOwn(params.sizeConstraints, key)
      ? params.sizeConstraints[key] : config[key])
  }
  sizeConstraints.set(window, constraints)
  applySizeConstraints(window)
  setWindowSize(window, { width: config.width ?? 800, height: config.height ?? 600 })
  if (config.maximized && !config.fullscreen) window.maximize()
  if (config.focus && config.focusable !== false && config.visible !== false) window.focus()
  if (config.visibleOnAllWorkspaces) window.setVisibleOnAllWorkspaces(true, { visibleOnFullScreen: true })
  if (config.contentProtected) window.setContentProtection(true)
  if (process.platform === 'darwin' && config.trafficLightPosition) window.setWindowButtonPosition(config.trafficLightPosition)
  if (config.preventOverflow) {
    const bounds = window.getBounds()
    const area = screen.getDisplayMatching(bounds).workArea
    const margin = typeof config.preventOverflow === 'object' ? config.preventOverflow : {}
    const width = Math.max(1, Math.min(bounds.width, area.width - (margin.width ?? 0)))
    const height = Math.max(1, Math.min(bounds.height, area.height - (margin.height ?? 0)))
    window.setBounds({
      x: Math.min(Math.max(bounds.x, area.x), area.x + area.width - width),
      y: Math.min(Math.max(bounds.y, area.y), area.y + area.height - height), width, height,
    })
  }
}

function ownedContents(window) {
  const contents = new Set()
  for (const [id, owner] of webviews) {
    if (owner === window) contents.add(contentsFor(id))
  }
  return [...contents]
}

function applyWindowCursorCss(window, contents) {
  const state = cursorCssKey.get(contents) ?? { pending: Promise.resolve() }
  cursorCssKey.set(contents, state)
  const pending = state.pending.then(async () => {
    if (contents.isDestroyed()) return
    if (state.key) {
      const key = state.key
      state.key = undefined
      await contents.removeInsertedCSS(key).catch(() => {})
    }
    const cursor = windowCursorState.get(window)
    const value = cursor?.visible === false ? 'none' : cursor?.cursor
    if (value) state.key = await contents.insertCSS(`* { cursor: ${value} !important }`, { cssOrigin: 'user' })
  })
  // A page may not have committed yet; dom-ready replays the remembered state.
  state.pending = pending.catch(() => {})
  return pending
}

function watchWindowCursor(window, contents) {
  contents.on('dom-ready', () => {
    applyWindowCursorCss(window, contents).catch((error) => log('window.cursor', serializeError(error)))
  })
}

function setWindowCursor(window, changes) {
  if (changes.cursor !== undefined && (typeof changes.cursor !== 'string' || !/^[a-z][a-z-]*$/.test(changes.cursor))) {
    throw new Error('invalid cursor icon')
  }
  if (changes.visible !== undefined && typeof changes.visible !== 'boolean') throw new Error('invalid cursor visibility')
  windowCursorState.set(window, { visible: true, ...windowCursorState.get(window), ...changes })
  return Promise.all(ownedContents(window).map((contents) => applyWindowCursorCss(window, contents))).then(() => null)
}

// Chromium media emulation keeps per-window theme separate from app nativeTheme.
const windowThemeState = new WeakMap()
const themeDebuggers = new WeakSet()

function windowThemeOverride(theme) {
  if (theme === 'dark' || theme === 'light') return theme
  if (theme == null || theme === 'system') return null
  throw new Error(`invalid window theme ${theme}`)
}

async function readWindowTheme(window) {
  // An unloaded native host has no document. Query one of its owned pages instead.
  const state = windowThemeState.get(window)
  const contents = [...state.contents].find((item) => !item.isDestroyed() && item.getURL())
  const dark = contents
    ? await contents.mainFrame.executeJavaScript("matchMedia('(prefers-color-scheme: dark)').matches")
    : state.theme === null ? nativeTheme.shouldUseDarkColors : state.theme === 'dark'
  return { theme: dark ? 'dark' : 'light' }
}

async function notifyWindowTheme(window, windowId) {
  if (window.isDestroyed()) return
  const { theme } = await readWindowTheme(window)
  if (!window.isDestroyed()) send({ event: 'window.themeChanged', params: {
    windowId, theme, dark: theme === 'dark' && !nativeTheme.shouldUseHighContrastColors,
  } })
}

async function emulateContentsTheme(contents, theme) {
  if (contents.isDestroyed()) return
  const debugger_ = contents.debugger
  if (!themeDebuggers.has(contents)) {
    if (theme === null) return
    if (debugger_.isAttached()) throw new Error('window theme cannot use an existing debugger session')
    debugger_.attach()
    themeDebuggers.add(contents)
  }
  await debugger_.sendCommand('Emulation.setEmulatedMedia', {
    features: theme === null ? [] : [{ name: 'prefers-color-scheme', value: theme }],
  })
  if (theme === null && themeDebuggers.delete(contents)) debugger_.detach()
}

function setWindowTheme(window, theme) {
  theme = windowThemeOverride(theme)
  const state = windowThemeState.get(window)
  const pending = state.pending.then(async () => {
    if (window.isDestroyed()) return null
    const previous = state.theme
    state.theme = theme
    try {
      await Promise.all([...state.contents].map((contents) => emulateContentsTheme(contents, theme)))
      await notifyWindowTheme(window, state.windowId)
    } catch (error) {
      state.theme = previous
      await Promise.all([...state.contents].map((contents) => emulateContentsTheme(contents, previous)
        .catch((rollbackError) => log('window.theme rollback failed', serializeError(rollbackError)))))
      throw error
    }
    return null
  })
  state.pending = pending.catch(() => {})
  return pending
}

function watchThemeContents(window, contents) {
  const state = windowThemeState.get(window)
  if (state.contents.has(contents)) return
  state.contents.add(contents)
  contents.once('destroyed', () => state.contents.delete(contents))
  contents.debugger.on('detach', () => {
    if (!themeDebuggers.delete(contents)) return
    log('window.theme debugger detached', state.windowId)
    notifyWindowTheme(window, state.windowId).catch((error) => log('window.theme', serializeError(error)))
  })
  contents.on('devtools-closed', () => {
    if (state.theme !== null && !contents.debugger.isAttached()) {
      setWindowTheme(window, state.theme).catch((error) => log('window.theme', serializeError(error)))
    }
  })
  contents.on('dom-ready', () => {
    notifyWindowTheme(window, state.windowId).catch((error) => log('window.theme', serializeError(error)))
  })
  if (state.theme !== null) {
    emulateContentsTheme(contents, state.theme).catch((error) => log('window.theme child', serializeError(error)))
  }
}

function installWindowTheme(window, windowId, theme) {
  windowThemeState.set(window, { windowId, theme: null, contents: new Set(), pending: Promise.resolve() })
  watchThemeContents(window, window.webContents)
  window.webContents.on('did-attach-webview', (_event, guest) => watchThemeContents(window, guest))
  const notify = () => notifyWindowTheme(window, windowId).catch((error) => log('window.theme', serializeError(error)))
  nativeTheme.on('updated', notify)
  window.once('closed', () => nativeTheme.off('updated', notify))
  // Never await creation: navigation may need a Rust custom-protocol response.
  setWindowTheme(window, theme).catch((error) => log('window.theme config', serializeError(error)))
}

async function createWindow(params) {
  const { windowId } = params
  const config = params.window ?? params
  log('window.create', JSON.stringify({ windowId, label: params.label }))
  const window = new BrowserWindow(browserWindowOptions(config, params))
  windows.set(windowId, window)
  windowConfigs.set(window, config)
  closableState.set(window, config.closable !== false)
  maximizableState.set(window, config.maximizable !== false)
  minimizableState.set(window, config.minimizable !== false)
  // Electron has no decorations getter; Rust updates this after native style changes.
  window.__tauriDecorations = config.decorations ?? true
  applyWindowConfig(window, config, params)
  installWindowTheme(window, windowId, config.theme)
  // Read this now: by the time `closed` fires the window and its webContents are
  // destroyed, and `window.webContents.id` throws "Object has been destroyed".
  const contentsId = window.webContents.id

  // Electron destroys the window on `close`; Tauri decides instead (an app may
  // `prevent_close` and keep the window around, e.g. hiding to the tray), so
  // hold the close and let the runtime answer with `window.destroy`.
  window.on('close', (event) => {
    if (closeAllowed.has(windowId)) return
    event.preventDefault()
    if (!closableState.get(window)) return
    send({ event: 'window.closeRequested', params: { windowId } })
  })

  window.on('closed', () => {
    closeAllowed.delete(windowId)
    windows.delete(windowId)
    webviewByContents.delete(contentsId)
    for (const [webviewId, owner] of webviews) {
      if (owner !== window) continue
      if (childWebviews.has(webviewId)) closeChildWebview(webviewId)
      else {
        handlerPolicy.delete(webviewId)
        webviews.delete(webviewId)
      }
    }
    send({ event: 'window.closed', params: { windowId } })
  })
  const emit = (event, extra = {}) => {
    if (window.isDestroyed()) return
    const geometry = readGeometry(window)
    send({ event: 'window.event', params: { windowId, event, ...geometry, ...extra } })
  }
  window.on('resize', () => resizeChildWebviews(window))
  const reapplyConstraints = () => applySizeConstraints(window)
  for (const event of ['resize', 'move', 'maximize', 'unmaximize', 'enter-full-screen', 'leave-full-screen']) {
    window.on(event, reapplyConstraints)
  }
  screen.on('display-metrics-changed', reapplyConstraints)
  window.once('closed', () => screen.off('display-metrics-changed', reapplyConstraints))
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
  // Native opener callbacks remain rejected by Rust; keep its event diagnostic.
  contents.setWindowOpenHandler(({ url, features }) => {
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
  const scripts = (initScripts ?? []).map((entry) => typeof entry === 'string'
    ? { script: entry, mainFrameOnly: true } : entry)
  if (!scripts.length) return path.join(__dirname, 'preload.cjs')
  // Different apps reuse webview IDs; never share executable preload files.
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'tauri-runtime-electron-preload-'))
  const file = path.join(directory, `${webviewId}.cjs`)
  app.once('will-quit', () => fs.rmSync(directory, { recursive: true, force: true }))
  const source = `${fs.readFileSync(path.join(__dirname, 'preload.cjs'), 'utf8')}
for (const entry of ${JSON.stringify(scripts)}) {
  if (entry.mainFrameOnly && !process.isMainFrame) continue
  try {
    webFrame.executeJavaScript(entry.script)
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
  applyWebviewConfig(window.webContents, webviewConfig(windowConfigs.get(window) ?? {}, webview))
  watchWindowCursor(window, window.webContents)
  wireWebviewEvents(window.webContents, webviewId)
  // Do not await: the page may need a `protocol.request` round trip to the Rust
  // runtime, which only starts pumping events after this call returns.
  window.loadURL(url).catch((error) => log('loadURL failed', url, serializeError(error)))
  return { windowId: webviewId }
}

/** Native children do not depend on a host page, its CSP, or its load finishing. */
function createChildWebview(window, webview) {
  const { webviewId, url } = webview
  const config = webviewConfig(windowConfigs.get(window) ?? {}, webview)
  const view = new WebContentsView({ webPreferences: webPreferences(config, webview) })
  applyWebviewConfig(view.webContents, config)
  const child = { view, contentsId: view.webContents.id, autoResize: Boolean(webview.autoResize) }
  childWebviews.set(webviewId, child)
  webviews.set(webviewId, window)
  view.webContents.once('destroyed', () => closeChildWebview(webviewId))
  try {
    window.contentView.addChildView(view)
    webviewByContents.set(view.webContents.id, webviewId)
    recordPolicy(webviewId, webview.policy)
    watchWindowCursor(window, view.webContents)
    watchThemeContents(window, view.webContents)
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
  childWebviews.delete(webviewId)
  webviews.delete(webviewId)
  webviewByContents.delete(child.contentsId)
  handlerPolicy.delete(webviewId)
  if (window && !window.isDestroyed()) window.contentView.removeChildView(child.view)
  if (!child.view.webContents.isDestroyed()) child.view.webContents.close()
  send({ event: 'webview.closed', params: { webviewId } })
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

/** Global screen origins cannot be scaled like window-relative coordinates on mixed-DPI Windows. */
function physicalScreenRect(window, rect, scale) {
  if (process.platform === 'win32') return screen.dipToScreenRect(window, rect)
  return {
    x: Math.round(rect.x * scale), y: Math.round(rect.y * scale),
    width: Math.round(rect.width * scale), height: Math.round(rect.height * scale),
  }
}

/** Tauri monitors are physical pixels, Electron displays are DIPs. */
function readDisplay(display) {
  const toPhysical = (rect) => physicalScreenRect(null, rect, display.scaleFactor)
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
  const toPhysical = (rect) => physicalScreenRect(window, rect, scaleFactor)
  return {
    scaleFactor,
    bounds: toPhysical(window.getBounds()),
    contentBounds: toPhysical(window.getContentBounds()),
    // Native sizing must honor the same content limits (in DIPs) as window.size.
    contentConstraints: contentConstraints(window),
    maximized: window.isMaximized(),
    minimized: window.isMinimized(),
    focused: window.isFocused(),
    visible: window.isVisible(),
    fullscreen: window.isFullScreen() || (process.platform === 'darwin' && window.isSimpleFullScreen()),
    resizable: window.isResizable(),
    alwaysOnTop: window.isAlwaysOnTop(),
    decorated: window.__tauriDecorations ?? true,
    title: window.getTitle(),
    // Electron does not implement all three native flags on every platform.
    maximizable: maximizableState.get(window) ?? true,
    minimizable: minimizableState.get(window) ?? true,
    closable: closableState.get(window) ?? true,
  }
}

/** Tauri sizes and positions are physical pixels; Electron's are DIPs. */
function toDip(window, params, key) {
  const value = params[key]
  if (typeof value !== 'number') return undefined
  return params.physical ? Math.round(value / scaleOf(window)) : Math.round(value)
}

function constraintValue(axis) {
  if (axis == null) return null
  const value = typeof axis === 'number' ? axis : axis.value
  if (!Number.isFinite(value) || value < 0) throw new Error('invalid window size constraint')
  return { value, physical: typeof axis === 'object' && axis.physical === true }
}

function contentConstraints(window) {
  const scale = scaleOf(window)
  return Object.fromEntries(Object.entries(sizeConstraints.get(window) ?? {}).map(([key, axis]) => [
    key, axis ? Math.round(axis.value / (axis.physical ? scale : 1)) : 0,
  ]))
}

function applySizeConstraints(window) {
  if (window.isDestroyed() || !sizeConstraints.has(window)) return
  const constraints = contentConstraints(window)
  const bounds = window.getBounds()
  const content = window.getContentBounds()
  const frameWidth = Math.max(0, bounds.width - content.width)
  const frameHeight = Math.max(0, bounds.height - content.height)
  const outer = (value, frame) => value > 0 ? value + frame : 0
  const minimum = [outer(constraints.minWidth, frameWidth), outer(constraints.minHeight, frameHeight)]
  const maximum = [outer(constraints.maxWidth, frameWidth), outer(constraints.maxHeight, frameHeight)]
  // Native setters can raise resize events. Cache before calling to avoid recursion.
  const applied = JSON.stringify([minimum, maximum])
  if (window.__tauriSizeConstraints === applied) return
  window.__tauriSizeConstraints = applied
  window.setMinimumSize(...minimum)
  window.setMaximumSize(...maximum)
}

function updateSizeConstraints(window, axes) {
  const constraints = Object.fromEntries(Object.entries(axes).map(([key, axis]) => [key, constraintValue(axis)]))
  sizeConstraints.set(window, { ...sizeConstraints.get(window), ...constraints })
  applySizeConstraints(window)
  const [width, height] = window.getContentSize()
  setWindowSize(window, { width, height })
}

function setSizeConstraint(window, kind, params) {
  updateSizeConstraints(window, {
    [`${kind}Width`]: params.width == null ? null : { value: params.width, physical: params.physical },
    [`${kind}Height`]: params.height == null ? null : { value: params.height, physical: params.physical },
  })
}

function setWindowSize(window, params) {
  let width = toDip(window, params, 'width')
  let height = toDip(window, params, 'height')
  if (width === undefined || height === undefined) return
  if (!Number.isFinite(width) || !Number.isFinite(height)) throw new Error('invalid window size')
  const constraints = contentConstraints(window)
  const clamp = (value, min, max) => Math.max(1, min || 0, max > 0 ? Math.min(value, max) : value)
  width = clamp(width, constraints.minWidth, constraints.maxWidth)
  height = clamp(height, constraints.minHeight, constraints.maxHeight)
  window.setContentSize(width, height)
}

function setWindowPosition(window, params) {
  if (typeof params.x !== 'number' || typeof params.y !== 'number') return
  if (params.physical && process.platform === 'win32') {
    const point = screen.screenToDipPoint({ x: params.x, y: params.y })
    window.setPosition(point.x, point.y)
    return
  }
  const x = toDip(window, params, 'x')
  const y = toDip(window, params, 'y')
  if (x === undefined || y === undefined) return
  window.setPosition(x, y)
}

/** Mirror the host menu; its owner remains authoritative for commands. */
function setWindowMenu(windowId, menu) {
  const window = findWindow(windowId)
  if (menu && !Array.isArray(menu)) menu = menu.items
  const template = (items) => {
    if (!Array.isArray(items)) throw new Error('invalid window menu')
    return items.map((item) => {
      if (item.type === 'separator') return { type: 'separator' }
      const entry = { label: item.label, enabled: item.enabled !== false,
        ...(item.accelerator ? { accelerator: item.accelerator } : {}) }
      if (item.submenu) entry.submenu = template(item.submenu)
      else {
        // Unchecked native check items are indistinguishable from ordinary items.
        if (item.checked) Object.assign(entry, { type: 'checkbox', checked: true })
        entry.click = () => {
          if (!window.isDestroyed() && windows.get(windowId) === window) {
            send({ event: 'menu.command', params: { windowId, commandId: item.commandId } })
          }
        }
      }
      return entry
    })
  }
  window.setMenu(menu === null ? null : Menu.buildFromTemplate(template(menu)))
  if (menu !== null) {
    window.setAutoHideMenuBar(false)
    window.setMenuBarVisibility(true)
  }
  applySizeConstraints(window)
  setImmediate(() => applySizeConstraints(window))
  return null
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
      const contents = contentsFor(params.webviewId)
      // The runtime posts eval without waiting, so a failure is only visible here.
      return contents.executeJavaScript(params.script, true).catch((error) => {
        log('eval failed', contents.id, String(error), params.script.slice(0, 200))
        throw error
      })
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
    case 'window.minSize':
      setSizeConstraint(findWindow(params.windowId), 'min', params)
      return null
    case 'window.maxSize':
      setSizeConstraint(findWindow(params.windowId), 'max', params)
      return null
    case 'window.sizeConstraints': {
      const axes = params.constraints ?? params
      updateSizeConstraints(findWindow(params.windowId), Object.fromEntries(
        ['minWidth', 'minHeight', 'maxWidth', 'maxHeight'].map((key) => [key, axes[key]]),
      ))
      return null
    }
    case 'window.resizable':
      findWindow(params.windowId).setResizable(params.resizable)
      return null
    case 'window.maximizable': {
      const window = findWindow(params.windowId)
      maximizableState.set(window, params.maximizable !== false)
      if (process.platform !== 'linux') window.setMaximizable(params.maximizable !== false)
      return null
    }
    case 'window.minimizable': {
      const window = findWindow(params.windowId)
      minimizableState.set(window, params.minimizable !== false)
      if (process.platform !== 'linux') window.setMinimizable(params.minimizable !== false)
      return null
    }
    case 'window.closable': {
      const window = findWindow(params.windowId)
      closableState.set(window, params.closable !== false)
      if (process.platform === 'darwin') window.setClosable(params.closable !== false)
      return null
    }
    case 'window.maximize':
      findWindow(params.windowId).maximize()
      return null
    case 'window.unmaximize':
      findWindow(params.windowId).unmaximize()
      return null
    case 'window.minimize':
      findWindow(params.windowId).minimize()
      return null
    case 'window.unminimize': {
      const window = findWindow(params.windowId)
      if (process.platform !== 'darwin' || !window.isMinimized()) {
        window.restore()
        return null
      }
      // AppKit clears isMiniaturized before deminiaturization finishes. Wait
      // for the native completion so a subsequent hide is not undone by it.
      return new Promise((resolve, reject) => {
        const finish = (error) => {
          clearTimeout(timer)
          window.removeListener('restore', restored)
          window.removeListener('closed', closed)
          if (error) reject(error)
          else resolve(null)
        }
        const restored = () => finish()
        const closed = () => finish(new Error('window closed during restore'))
        const timer = setTimeout(() => finish(new Error('window restore timed out')), 10_000)
        window.once('restore', restored)
        window.once('closed', closed)
        try { window.restore() }
        catch (error) { finish(error) }
      })
    }
    case 'window.fullscreen': {
      const window = findWindow(params.windowId)
      window.setFullScreen(params.fullscreen)
      if (process.platform === 'darwin') {
        // AppKit can refuse Space switches on CI or desktops without a usable Space.
        setTimeout(() => {
          if (window.isDestroyed()) return
          if (params.fullscreen && !window.isFullScreen() && !window.isSimpleFullScreen()) window.setSimpleFullScreen(true)
          else if (!params.fullscreen && window.isSimpleFullScreen()) window.setSimpleFullScreen(false)
        }, 600)
      }
      return null
    }
    case 'window.alwaysOnTop':
      findWindow(params.windowId).setAlwaysOnTop(params.alwaysOnTop)
      return null
    case 'window.cursor':
    case 'window.setCursorIcon':
      return setWindowCursor(findWindow(params.windowId), { cursor: params.cursor })
    case 'window.setCursorVisible':
      return setWindowCursor(findWindow(params.windowId), { visible: params.visible })
    case 'window.setParentWindow': {
      const child = findWindow(params.windowId)
      const parent = params.parentId == null ? null : findWindow(params.parentId)
      if (parent === child) throw new Error('window cannot be its own parent')
      child.setParentWindow(parent)
      return null
    }
    case 'window.setMenu':
      return setWindowMenu(params.windowId, params.menu)
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
    case 'webview.navigationResponse': {
      navigationRequests.get(params.id)?.finish(params.allow === true)
      return null
    }
    case 'webview.downloadResponse': {
      const entry = downloadRequests.get(params.id)
      if (!entry || entry.allow !== undefined) return null
      clearTimeout(entry.timer)
      if (params.destination !== undefined) entry.finalPath = params.destination
      entry.allow = params.allow === true && typeof entry.finalPath === 'string' &&
        entry.finalPath.length > 0 && !entry.finalPath.includes('\0')
      if (!entry.allow && entry.state === undefined) {
        try { entry.item.cancel() }
        catch (error) {
          log('download.cancel failed', serializeError(error))
          entry.state = 'cancelled'
        }
      }
      finishDownload(params.id)
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
      // Navigation can request a custom protocol from the Rust loop awaiting us.
      contentsFor(params.webviewId).loadURL(params.url).catch((error) => log('navigate failed', params.url, serializeError(error)))
      return null
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
      return openDevtools(contentsFor(params.webviewId))
    case 'webview.closeDevtools': {
      const contents = contentsFor(params.webviewId)
      trace('devtools.close.enter', contents.id)
      contents.closeDevTools()
      trace('devtools.close.return', contents.id)
      return null
    }
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
        window.setSimpleFullScreen(params.fullscreen ?? params.enable)
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
    case 'window.decorations': {
      // The native frame is toggled by Rust; recalculate its content limits too.
      const window = findWindow(params.windowId)
      window.__tauriDecorations = (params.value ?? params.decorations) !== false
      applySizeConstraints(window)
      setImmediate(() => applySizeConstraints(window))
      return null
    }
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
    case 'window.backgroundColor': {
      const window = findWindow(params.windowId)
      window.setBackgroundColor(params.color)
      if (window.__tauriMaterialBackground !== undefined) window.__tauriMaterialBackground = params.color
      return null
    }
    case 'window.backgroundMaterial': {
      if (!['none', 'acrylic', 'mica', 'tabbed'].includes(params.material)) throw new Error('invalid background material')
      if (process.platform !== 'win32' || Number(os.release().split('.')[2]) < 22621) {
        throw new Error('Electron background materials require Windows 11 22H2 or later')
      }
      const window = findWindow(params.windowId)
      // Electron resets the backing to white on 'none'. Preserve the configured
      // color (including full transparency) across repeated enable/clear calls.
      if (params.material !== 'none' && window.__tauriMaterialBackground === undefined) {
        window.__tauriMaterialBackground = window.getBackgroundColor()
      }
      const background = window.__tauriMaterialBackground ?? window.getBackgroundColor()
      window.setBackgroundMaterial(params.material)
      if (params.material === 'none') {
        window.setBackgroundColor(background)
        delete window.__tauriMaterialBackground
      }
      return null
    }
    case 'window.theme':
      return setWindowTheme(findWindow(params.windowId), params.theme)
    case 'window.ignoreCursorEvents':
      findWindow(params.windowId).setIgnoreMouseEvents(params.ignore, { forward: true })
      return null
    case 'window.themeQuery':
      return readWindowTheme(findWindow(params.windowId))
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
      if (process.platform === 'win32') return screen.dipToScreenPoint(point)
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
      for (const target of customSessions) {
        target.protocol.handle(scheme, (request) => dispatchProtocol(scheme, request))
      }
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

async function onLine(line) {
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

  try {
    // Start native lifecycle calls in the scheduled task, not a Promise microtask.
    // Await only their result.
    trace('rpc.enter', message.id, message.method)
    const result = await handle(message.method, message.params ?? {})
    trace('rpc.return', message.id, message.method)
    send({ id: message.id, result: result ?? null })
  } catch (error) {
    trace('rpc.error', message.id, message.method, serializeError(error))
    log('error', message.method, serializeError(error))
    send({ id: message.id, error: serializeError(error) })
  }
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
    if (process.env.TAURI_RUNTIME_ELECTRON_TRACE === '1') {
      const id = contents.id
      trace('contents.created', id, contents.getType())
      for (const event of ['did-start-loading', 'did-stop-loading', 'dom-ready', 'did-finish-load', 'devtools-opened', 'devtools-closed', 'destroyed']) {
        contents.on(event, () => trace('contents.event', id, event))
      }
      contents.on('did-fail-load', (_event, code, description, url) => trace('contents.load.failed', id, code, description, url))
      contents.on('render-process-gone', (_event, details) => trace('contents.process.gone', id, JSON.stringify(details)))
    }
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
      const line = buffer.slice(0, index)
      // Drain input before entering native lifecycle code. Unlike a Promise
      // microtask, Electron's setImmediate also wakes its embedded Node loop.
      buffer = buffer.slice(index + 1)
      setImmediate(() => onLine(line))
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

// Native Quit (including the menu role) asks Rust first, just like close.
// A disconnected host must still be able to tear Electron down.
app.on('before-quit', (event) => {
  if (!socket || socket.destroyed) return
  event.preventDefault()
  send({ event: 'app.exitRequested', params: {} })
})

// The Rust side owns the lifetime: no window means keep running until the
// socket closes.
app.on('window-all-closed', () => {})
app.on('activate', (_event, hasVisibleWindows) => {
  send({ event: 'app.reopen', params: { hasVisibleWindows } })
})
