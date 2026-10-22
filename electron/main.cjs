// Electron side of the probe: speaks newline-delimited JSON with the Rust
// runtime over a loopback TCP socket and turns RPC calls into BrowserWindows.
'use strict'

const {
  app,
  BrowserWindow,
  ipcMain,
  Menu,
  nativeImage,
  nativeTheme,
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
const { pathToFileURL } = require('node:url')

const logFile = process.env.TAURI_RUNTIME_ELECTRON_LOG ?? path.join(os.tmpdir(), 'tauri-runtime-electron.log')
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

// Verification hook: `TAURI_RUNTIME_ELECTRON_DEBUG_PORT=9222` exposes the
// DevTools protocol so a test driver can read the DOM and take real screenshots
// of every window and guest. Switches must be appended before `ready`.
if (process.env.TAURI_RUNTIME_ELECTRON_DEBUG_PORT) {
  app.commandLine.appendSwitch(
    'remote-debugging-port',
    process.env.TAURI_RUNTIME_ELECTRON_DEBUG_PORT,
  )
}

// Chromium names its profile after the executable, and every app here runs the
// same cached `electron.exe`, so without this every Tauri app on this runtime
// would share one profile: cookies, local storage and the cache lock included
// (two apps at once corrupt it and crash the renderer).
if (appIdentifier) {
  // `setPath` requires the directory to exist.
  const profile = path.join(app.getPath('appData'), appIdentifier)
  fs.mkdirSync(profile, { recursive: true })
  app.setPath('userData', profile)
  // Taskbar grouping and notification identity (Windows only).
  if (process.platform === 'win32') app.setAppUserModelId(appIdentifier)
}

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

/** id -> resolve(Response); a `protocol.request` awaiting `protocol.respond`. */
const protocolRequests = new Map()
let nextProtocolId = 1
const registeredSchemes = new Set()

async function dispatchProtocol(scheme, request, uri = request.url) {
  const id = nextProtocolId++
  const body = Buffer.from(await request.arrayBuffer()).toString('base64')
  const headers = {}
  for (const [name, value] of request.headers) headers[name] = value
  if (!headers.origin) {
    // Tauri's protocols validate the request origin; a synthetic one keeps
    // `tauri://localhost` and `ipc://localhost` requests looking native.
    try {
      const url = new URL(uri)
      headers.origin = `${url.protocol.slice(0, -1)}://${url.host}`
    } catch {}
  }
  const response = new Promise((resolve) => protocolRequests.set(id, resolve))
  send({
    event: 'protocol.request',
    params: { id, scheme, uri, method: request.method, headers, body },
  })
  const reply = await response
  const data = Buffer.from(reply.body ?? '', 'base64')
  log('protocol', scheme, request.method, request.url, reply.status, JSON.stringify({
    range: headers.range, contentRange: reply.headers?.['content-range'],
    contentLength: reply.headers?.['content-length'], bytes: data.length,
  }))
  if (reply.status >= 400) {
    log('protocol-error', request.url, data.toString().slice(0, 300))
  }
  return new Response(data, {
    status: reply.status ?? 200,
    headers: reply.headers ?? {},
  })
}

function dispatchHttpProtocol(request) {
  const url = new URL(request.url)
  const scheme = url.hostname.endsWith('.localhost') ? url.hostname.slice(0, -'.localhost'.length) : ''
  if (url.username || url.password || url.port || !registeredSchemes.has(scheme)) {
    return session.defaultSession.fetch(request, { bypassCustomProtocolHandlers: true })
  }
  // Keep commands on the authenticated IPC bridge, including their HTTP aliases.
  if (scheme === 'ipc') return new Response('Use native IPC', { status: 403 })
  // Chromium reads Content-Range's total only for HTTP-family media URLs.
  return dispatchProtocol(scheme, request, `${scheme}://localhost${url.pathname}${url.search}`)
}

/** @type {import('node:net').Socket | undefined} */
let socket
const windows = new Map()
// Windows Tauri accepted to close; everything else holds the close.
const closeAllowed = new Set()
/** BrowserWindow -> whether the user may close it (`closable`). */
const closableState = new WeakMap()
/** BrowserWindow -> whether it still has a native frame. */
const decoratedState = new WeakMap()
/** BrowserWindow -> whether the window manager may maximize it (Linux has no
 * Electron API for this, so the value is tracked here). */
const maximizableState = new WeakMap()
/** BrowserWindow -> same, for minimize. */
const minimizableState = new WeakMap()
/** BrowserWindow -> `{ css, key }` of the injected page-wide `cursor` rule. */
const cursorCssKey = new WeakMap()
// Tauri constrains the content area; Electron's public min/max APIs include the frame.
const sizeConstraints = new WeakMap()
/** webviewId -> BrowserWindow */
const webviews = new Map()
/** webContents id -> webviewId (for routing `tauri-ipc` messages) */
const webviewByContents = new Map()
/** windowId -> webviewId of the contents that hosts the page */
const mainWebview = new Map()
/** webviewId -> webContents id of a `<webview>` guest */
const guestContentsIds = new Map()
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

/**
 * Window config keys Electron cannot express, with the value that means
 * "nothing to do". Only a key whose value differs from that default is worth
 * warning about.
 */
const UNSUPPORTED_WINDOW_OPTIONS = [
  ['windowClassname', undefined], // Electron creates its own Win32 window class
  ['noRedirectionBitmap', false], // creation-time DWM style Electron does not expose
  ['additionalBrowserArgs', undefined], // would need a process relaunch
  ['browserExtensionsEnabled', false], // session-wide launch flags
  ['dataStoreIdentifier', undefined], // Electron partitions are string-keyed
  ['limitNavigationsToAppBoundDomains', false], // WKWebView only
  ['activityName', undefined],
  ['createdByActivityName', undefined],
  ['requestedBySceneIdentifier', undefined], // mobile only
  ['proxyUrl', undefined], // Electron proxies are per session, not per window
  ['dataDirectory', undefined], // Electron's userData path is app-wide
  ['generalAutofillEnabled', true],
  ['disableInputAccessoryView', false], // iOS / Mac Catalyst only
  ['allowLinkPreview', true], // iOS only
]

/** `[r, g, b, a]` from `tauri_utils::config::Color` as a CSS color. */
function cssColor(color) {
  if (typeof color === 'string') return color
  if (!Array.isArray(color) || color.length < 3) return undefined
  const [r, g, b, a = 255] = color
  return `rgba(${r}, ${g}, ${b}, ${(a / 255).toFixed(3)})`
}

/** Tauri's title bar styles are macOS concepts; map the closest Electron one. */
function titleBarStyleOf(config) {
  if (config.hiddenTitle) return 'hidden'
  switch (config.titleBarStyle) {
    case 'transparent':
      return 'hidden'
    case 'overlay':
      return 'hiddenInset'
    default:
      return 'default'
  }
}

/** Translates tauri window effects to Electron's `backgroundMaterial`. */
function windowMaterial(config) {
  const effects = config.windowEffects?.effects
  if (!Array.isArray(effects)) return undefined
  const has = (...names) => names.some((name) => effects.includes(name))
  if (has('mica', 'micaDark', 'micaLight')) return 'mica'
  if (has('tabbed', 'tabbedDark', 'tabbedLight')) return 'tabbed'
  if (has('acrylic', 'blur')) return 'acrylic'
  return undefined
}

/** Keeps the window inside its display's work area (`prevent_overflow`). */
function clampToWorkArea(window, preventOverflow) {
  const margin = typeof preventOverflow === 'object' ? preventOverflow : {}
  const bounds = window.getBounds()
  const area = screen.getDisplayMatching(bounds).workArea
  const width = Math.max(1, Math.min(bounds.width, area.width - (margin.width ?? 0)))
  const height = Math.max(1, Math.min(bounds.height, area.height - (margin.height ?? 0)))
  const x = Math.min(Math.max(bounds.x, area.x), area.x + area.width - width)
  const y = Math.min(Math.max(bounds.y, area.y), area.y + area.height - height)
  window.setBounds({ x, y, width, height })
}

function browserWindowOptions(config, params, iconImage, iconPath) {
  const { windowId } = params
  const options = {}
  const set = (key, value) => {
    if (value !== undefined) options[key] = value
  }
  set('title', config.title ?? 'Electron Runtime')
  set('width', Math.max(1, Math.round(config.width ?? 800)))
  set('height', Math.max(1, Math.round(config.height ?? 600)))
  set('x', typeof config.x === 'number' ? Math.round(config.x) : undefined)
  set('y', typeof config.y === 'number' ? Math.round(config.y) : undefined)
  set('center', config.center === true)
  set('resizable', config.resizable !== false)
  // Tauri's `inner_size` is the webview area, which is what `useContentSize`
  // makes of a BrowserWindow's width/height.
  set('useContentSize', true)
  set('maximizable', config.maximizable !== false)
  set('minimizable', config.minimizable !== false)
  set('fullscreen', config.fullscreen === true)
  set('focusable', config.focusable !== false)
  set('transparent', config.transparent === true)
  set('show', config.visible !== false)
  set('frame', config.decorations !== false)
  set('alwaysOnTop', config.alwaysOnTop === true)
  set('skipTaskbar', config.skipTaskbar === true)
  set('autoHideMenuBar', true)
  set('backgroundColor', cssColor(config.backgroundColor))
  set('hasShadow', config.shadow !== false)
  set('titleBarStyle', titleBarStyleOf(config))
  set('acceptFirstMouse', config.acceptFirstMouse === true)
  set('tabbingIdentifier', config.tabbingIdentifier)
  if (process.platform === 'win32') set('backgroundMaterial', windowMaterial(config))
  set('icon', iconImage ?? iconPath)

  const webPreferences = {
    contextIsolation: true,
    sandbox: true,
    nodeIntegration: false,
    // Tauri pages embed guests with the Chromium `<webview>` element; Electron
    // disables that tag by default since v5.
    webviewTag: true,
    preload: params.webview
      ? writeInitPreload(params.webview.webviewId, params.webview.initScripts)
      : path.join(__dirname, 'preload.cjs'),
    userAgent: config.userAgent,
    javascript: config.javascriptDisabled !== true,
    devTools: config.devtools !== false,
    backgroundThrottling: config.backgroundThrottling !== 'disabled',
    // Without `persist:` the partition is in-memory, which is what `incognito`
    // means here.
    partition: config.incognito ? `tauri-incognito-${windowId}` : undefined,
  }
  options.webPreferences = webPreferences
  return options
}

/** Config Electron can only apply once the window exists. */
function applyWindowConfig(window, config, params) {
  const constraints = {}
  for (const key of ['minWidth', 'minHeight', 'maxWidth', 'maxHeight']) {
    if (typeof config[key] === 'number') constraints[key] = Math.max(0, Math.round(config[key]))
  }
  sizeConstraints.set(window, constraints)
  applySizeConstraints(window)
  // Tauri's size and constraints both describe the content area.
  if (typeof config.width === 'number' && typeof config.height === 'number') {
    setWindowSize(window, { width: config.width, height: config.height })
  }
  if (config.maximized && config.fullscreen !== true) window.maximize()
  if (config.focus && config.focusable !== false && config.visible !== false) window.focus()
  if (config.visibleOnAllWorkspaces) {
    window.setVisibleOnAllWorkspaces(true, { visibleOnFullScreen: true })
  }
  if (config.contentProtected) window.setContentProtection(true)
  if (config.preventOverflow) clampToWorkArea(window, config.preventOverflow)
  if (config.trafficLightPosition && process.platform === 'darwin') {
    window.setWindowButtonPosition(config.trafficLightPosition)
  }
  if (config.dragDropEnabled === false) {
    // Electron has no window-level drop switch; block the page's drop events.
    window.webContents.on('did-finish-load', () => {
      window.webContents.mainFrame.executeJavaScript(
        "for (const type of ['dragenter', 'dragover', 'drop']) document.addEventListener(type, (event) => { event.preventDefault(); event.stopPropagation() }, true)",
      )
    })
  }
  if (config.zoomHotkeysEnabled === false) {
    // Chromium zooms on Ctrl/⌘ + wheel and the zoom accelerators; Tauri's flag
    // says this window must not, so cancel both at the source.
    const disableZoom = () => {
      // Returns a promise, so a missing renderer surfaces as a rejection rather
      // than a throw; the wheel is still pinned by the key handler below.
      window.webContents
        .setVisualZoomLevelLimits(1, 1)
        .catch((error) => log('window.zoom error', params.label ?? params.windowId, serializeError(error)))
    }
    if (window.webContents.getURL() && !window.webContents.isLoading()) {
      disableZoom()
    } else {
      window.webContents.once('dom-ready', disableZoom)
    }
    window.webContents.on('before-input-event', (event, input) => {
      if (!(input.control || input.meta)) return
      if (input.key === '+' || input.key === '-' || input.key === '=' || input.key === '0') {
        event.preventDefault()
      }
    })
  }
  if (config.scrollBarStyle === 'fluentOverlay') {
    // Electron has no per-window scrollbar switch; draw the overlay look instead.
    window.webContents.on('did-finish-load', () => {
      window.webContents
        .insertCSS(
          '::-webkit-scrollbar { width: 8px; height: 8px } ::-webkit-scrollbar-track { background: transparent } ::-webkit-scrollbar-thumb { background: rgba(128, 128, 128, 0.45); border-radius: 4px }',
        )
        .catch((error) => log('window.scrollbar error', serializeError(error)))
    })
  }
  const unsupported = UNSUPPORTED_WINDOW_OPTIONS.filter(
    ([key, defaultValue]) => config[key] !== undefined && config[key] !== defaultValue,
  )
  if (unsupported.length) {
    log(
      'window.unsupported',
      params.label ?? params.windowId,
      unsupported.map(([key]) => key).join(','),
    )
  }
}

/** Applies the remembered `cursor` rule; `null` removes it. */
async function applyWindowCursorCss(window) {
  const state = cursorCssKey.get(window)
  if (!state) return
  if (state.key) {
    const key = state.key
    state.key = undefined
    await window.webContents.removeInsertedCSS(key).catch(() => {})
  }
  if (!state.css) return
  try {
    state.key = await window.webContents.insertCSS(state.css, { cssOrigin: 'user' })
  } catch {
    // The window has no render frame yet (nothing navigated). `dom-ready`
    // applies the rule as soon as the first page exists.
  }
}

/** Replaces the page-wide `cursor` rule; `null` removes it. */
async function setWindowCursorCss(window, css) {
  cursorCssKey.set(window, { css, key: cursorCssKey.get(window)?.key })
  await applyWindowCursorCss(window)
}

// Electron's nativeTheme is app-wide. CDP changes Chromium's real media queries
// per WebContents, including CSS and matchMedia; no page APIs are replaced.
const windowThemeState = new WeakMap()
const themeDebuggers = new WeakSet()

function windowThemeOverride(theme) {
  if (theme === 'dark' || theme === 'light') return theme
  if (theme == null || theme === 'system') return null
  throw new Error(`invalid window theme ${theme}`)
}

async function readWindowTheme(window) {
  const contents = window.webContents
  // Before the first document commits there is no renderer preference to query.
  // Do not wait on it: Rust may still be servicing creation/menu callbacks.
  const dark = contents.getURL()
    ? await contents.mainFrame.executeJavaScript("matchMedia('(prefers-color-scheme: dark)').matches")
    : nativeTheme.shouldUseDarkColors
  return { theme: dark ? 'dark' : 'light' }
}

async function notifyWindowTheme(window, windowId) {
  if (window.isDestroyed()) return
  const { theme } = await readWindowTheme(window)
  if (!window.isDestroyed()) send({
    event: 'window.themeChanged',
    params: { windowId, theme, dark: theme === 'dark' && !nativeTheme.shouldUseHighContrastColors },
  })
}

async function emulateContentsTheme(contents, theme) {
  if (contents.isDestroyed()) return
  const debugger_ = contents.debugger
  if (!themeDebuggers.has(contents)) {
    if (theme === null) return
    // Never send emulation commands through somebody else's debugger session.
    if (debugger_.isAttached()) throw new Error('window theme cannot use an existing debugger session')
    debugger_.attach()
    themeDebuggers.add(contents)
  }
  await debugger_.sendCommand('Emulation.setEmulatedMedia', {
    features: theme === null ? [] : [{ name: 'prefers-color-scheme', value: theme }],
  })
  if (theme === null && themeDebuggers.has(contents)) {
    themeDebuggers.delete(contents)
    debugger_.detach()
  }
}

function setWindowTheme(window, theme) {
  theme = windowThemeOverride(theme)
  const state = windowThemeState.get(window)
  // Serialize calls so a slow earlier request cannot overwrite a later one.
  const pending = state.pending.then(async () => {
    const previous = state.theme
    state.theme = theme
    try {
      await Promise.all([...state.contents].map((contents) => emulateContentsTheme(contents, theme)))
      await notifyWindowTheme(window, state.windowId)
    } catch (error) {
      state.theme = previous
      await Promise.all([...state.contents].map((contents) =>
        emulateContentsTheme(contents, previous).catch((rollbackError) => log('window.theme rollback failed', serializeError(rollbackError)))))
      throw error
    }
    return null
  })
  state.pending = pending.catch(() => {})
  return pending
}

function watchThemeContents(window, contents) {
  const state = windowThemeState.get(window)
  state.contents.add(contents)
  contents.once('destroyed', () => state.contents.delete(contents))
  contents.debugger.on('detach', () => {
    if (!themeDebuggers.delete(contents)) return
    // Do not fight DevTools or an external debugger by immediately reattaching.
    log('window.theme debugger detached', state.windowId)
    notifyWindowTheme(window, state.windowId).catch((error) => log('window.theme', serializeError(error)))
  })
  contents.on('devtools-closed', () => {
    if (state.theme !== null && !contents.debugger.isAttached()) {
      setWindowTheme(window, state.theme).catch((error) => log('window.theme', serializeError(error)))
    }
  })
  // CDP retains emulation across navigation and renderer swaps. A new guest is
  // a separate target, so apply its owner's override before its app navigation.
  if (state.theme !== null) {
    emulateContentsTheme(contents, state.theme).catch((error) => log('window.theme guest', serializeError(error)))
  }
}

function installWindowTheme(window, windowId, theme) {
  windowThemeState.set(window, { windowId, theme: null, contents: new Set(), pending: Promise.resolve() })
  watchThemeContents(window, window.webContents)
  window.webContents.on('did-attach-webview', (_event, guest) => watchThemeContents(window, guest))
  const notify = () => notifyWindowTheme(window, windowId).catch((error) => log('window.theme', serializeError(error)))
  window.webContents.on('dom-ready', notify)
  nativeTheme.on('updated', notify)
  window.once('closed', () => nativeTheme.off('updated', notify))
  // A debugger command can wait for first navigation. Creation must return so
  // Rust can service its protocol requests; never await this in createWindow.
  setWindowTheme(window, theme).catch((error) => log('window.theme config', serializeError(error)))
}

async function createWindow(params) {
  const { windowId } = params
  const config = params.window ?? {}
  // Windows canonicalized paths arrive with a verbatim `\\?\` prefix that
  // Electron's image loader cannot open.
  const iconPath =
    typeof params.icon === 'string' ? params.icon.replace(/^\\\\\?\\/, '') : undefined
  // `WindowBuilder::icon` sends raw BGRA; Electron takes that directly.
  const iconImage = params.windowIcon
    ? nativeImage.createFromBitmap(Buffer.from(params.windowIcon.bgra, 'base64'), {
        width: params.windowIcon.width,
        height: params.windowIcon.height,
        scaleFactor: 1,
      })
    : undefined
  log('window.create', JSON.stringify({ windowId, label: params.label }))
  const window = new BrowserWindow(browserWindowOptions(config, params, iconImage, iconPath))
  trackDocument(window.webContents)
  windows.set(windowId, window)
  closableState.set(window, config.closable !== false)
  decoratedState.set(window, config.decorations !== false)
  maximizableState.set(window, config.maximizable !== false)
  minimizableState.set(window, config.minimizable !== false)
  // The page webview of this window; later webviews on it become guests.
  if (params.webview) mainWebview.set(windowId, params.webview.webviewId)
  // Read this now: by the time `closed` fires the window and its webContents are
  // destroyed, and `window.webContents.id` throws "Object has been destroyed".
  const contentsId = window.webContents.id
  if (process.platform === 'win32' && process.env.TAURI_RUNTIME_ELECTRON_LOG) {
    for (const [message, phase] of [[0x0231, 'enter'], [0x0232, 'exit']]) {
      window.hookWindowMessage(message, () => log('native-move-loop', phase, JSON.stringify({ windowId, ...readGeometry(window) })))
    }
  }

  applyWindowConfig(window, config, params)
  installWindowTheme(window, windowId, config.theme)

  // Electron destroys the window on `close`; Tauri decides instead (an app may
  // `prevent_close` and keep the window around, e.g. hiding to the tray), so
  // hold the close and let the runtime answer with `window.destroy`.
  window.on('close', (event) => {
    if (closeAllowed.has(windowId)) return
    // `closable: false` keeps the window alive; Tauri's own close goes through
    // `window.destroy`, which sets `closeAllowed` first.
    if (!closableState.get(window)) {
      event.preventDefault()
      return
    }
    event.preventDefault()
    send({ event: 'window.closeRequested', params: { windowId } })
  })

  // A cursor rule needs a render frame; a window created before its first page
  // navigated applies the remembered one here.
  window.webContents.on('dom-ready', () => applyWindowCursorCss(window))

  window.on('closed', () => {
    closeAllowed.delete(windowId)
    windows.delete(windowId)
    mainWebview.delete(windowId)
    webviewByContents.delete(contentsId)
    for (const [webviewId, owner] of webviews) {
      if (owner === window) webviews.delete(webviewId)
    }
    send({ event: 'window.closed', params: { windowId } })
  })
  const emit = (event, extra = {}) => {
    if (window.isDestroyed()) return
    const geometry = readGeometry(window)
    send({ event: 'window.event', params: { windowId, event, ...geometry, ...extra } })
  }
  for (const event of ['resize', 'move', 'focus', 'blur', 'maximize', 'unmaximize', 'enter-full-screen', 'leave-full-screen']) {
    window.on(event, () => emit(event))
  }
  window.webContents.on('did-finish-load', () => {
    log('loaded', window.webContents.getURL())
  })

  const webview = params.webview
  if (webview) return loadWebview(window, webview)
  // A `Window` without a webview still needs a document: its children are
  // `<webview>` guests appended to this page, and executeJavaScript never
  // settles on a webContents that has never navigated.
  window.loadURL('about:blank').catch((error) => log('about:blank failed', serializeError(error)))
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
  const file = path.join(os.tmpdir(), `tauri-runtime-electron-preload-${process.pid}-${webviewId}.cjs`)
  const source = `'use strict'
const { contextBridge, ipcRenderer, webFrame } = require('electron')
contextBridge.exposeInMainWorld('ipc', {
  postMessage(data) {
    ipcRenderer.send('tauri-ipc', String(data))
  },
  fetch(request) {
    return ipcRenderer.invoke('tauri-protocol', request)
  },
})
for (const source of ${JSON.stringify(scripts)}) {
  webFrame.executeJavaScript(source).catch(error => console.error('init script failed', error))
}
`
  fs.writeFileSync(file, source)
  return file
}

// Only the bundled app's root needs an explicit entry file. External URLs and
// isolation schemes have their own routing (isolation serves its document at /).
function webviewUrl(url) {
  const parsed = new URL(url)
  if (parsed.protocol === 'tauri:' && parsed.pathname === '/') {
    parsed.pathname = '/index.html'
    return parsed.href
  }
  return url
}

function loadWebview(window, webview) {
  const { webviewId } = webview
  const url = webviewUrl(webview.url)
  webviews.set(webviewId, window)
  webviewByContents.set(window.webContents.id, webviewId)
  recordPolicy(webviewId, webview.policy)
  wireWebviewEvents(window.webContents, webviewId)
  // Do not await: the page may need a `protocol.request` round trip to the Rust
  // runtime, which only starts pumping events after this call returns.
  log('loadURL', url)
  window.loadURL(url).catch((error) => log('loadURL failed', url, serializeError(error)))
  return { windowId: webviewId }
}

// DOM readiness is independent of outstanding images, streams and subframes.
const readyDocuments = new WeakSet()
function trackDocument(contents) {
  contents.on('did-start-navigation', (details) => {
    if (details.isMainFrame && !details.isSameDocument) readyDocuments.delete(contents)
  })
  contents.on('dom-ready', () => readyDocuments.add(contents))
}

function pageReady(window) {
  const contents = window.webContents
  if (contents.isDestroyed()) return Promise.reject(new Error('window page was destroyed'))
  if (readyDocuments.has(contents)) return Promise.resolve()
  return new Promise((resolve, reject) => {
    const finish = (error) => {
      clearTimeout(timer)
      contents.off('dom-ready', onReady)
      contents.off('destroyed', onDestroyed)
      contents.off('did-fail-load', onFailure)
      if (error) reject(error)
      else resolve()
    }
    const onReady = () => finish()
    const onDestroyed = () => finish(new Error('window page was destroyed'))
    const onFailure = (_event, code, description, _url, isMainFrame) => {
      if (isMainFrame) finish(new Error(`window page failed to load: ${code} ${description}`))
    }
    const timer = setTimeout(() => finish(new Error('window page did not become ready')), 10000)
    contents.once('dom-ready', onReady)
    contents.once('destroyed', onDestroyed)
    contents.on('did-fail-load', onFailure)
  })
}

// Runs in the host renderer. Percentages keep all four coordinates proportional
// when auto resize is enabled; fixed views retain CSS pixel bounds.
function placeGuest(element, bounds, autoResize) {
  bounds ??= element.getBoundingClientRect()
  autoResize ??= element.dataset.tauriAutoResize === 'true'
  const length = (value, total) => autoResize ? `${100 * value / total}%` : `${value}px`
  Object.assign(element.style, {
    position: 'absolute', right: 'auto', bottom: 'auto',
    left: length(bounds.x, innerWidth), top: length(bounds.y, innerHeight),
    width: length(bounds.width, innerWidth), height: length(bounds.height, innerHeight),
  })
  element.dataset.tauriAutoResize = String(autoResize)
}

function guestBounds(window, bounds) {
  if (!bounds) {
    const [width, height] = window.getContentSize()
    return { x: 0, y: 0, width, height }
  }
  return {
    x: toDip(window, bounds.position, 'x'), y: toDip(window, bounds.position, 'y'),
    width: toDip(window, bounds.size, 'width'), height: toDip(window, bounds.size, 'height'),
  }
}

async function attachWebviewElement(window, webview) {
  const { webviewId, url, initScripts, bounds, autoResize } = webview
  webviews.set(webviewId, window)
  recordPolicy(webviewId, webview.policy)
  const elementId = `tauri-webview-${webviewId}`
  // Creating the element is all this call may wait for: the guest asks the Rust
  // runtime for its page (`tauri://` goes through the protocol round trip), and
  // the runtime is blocked in this very call until it answers. Waiting for the
  // guest to load here would deadlock; the load reports itself later through the
  // usual webview events.
  const code = `(() => {
    const element = document.createElement('webview')
    element.id = ${JSON.stringify(elementId)}
    element.setAttribute('webpreferences', 'contextIsolation=yes,sandbox=yes')
    element.setAttribute('preload', ${JSON.stringify(pathToFileURL(writeInitPreload(webviewId, initScripts)).href)})
    element.dataset.tauriSource = ${JSON.stringify(webviewUrl(url))}
    element.src = 'about:blank'
    ;(${placeGuest.toString()})(element, ${JSON.stringify(guestBounds(window, bounds))}, ${Boolean(autoResize)})
    document.body.appendChild(element)
    return true
  })()`
  await pageReady(window)
  let cleanup
  const attached = new Promise((resolve, reject) => {
    cleanup = () => {
      clearTimeout(timer)
      window.webContents.off('did-attach-webview', onAttach)
      window.webContents.off('destroyed', onDestroyed)
    }
    const onAttach = (_event, guest) => {
      cleanup()
      // Register before guest navigation can send its first IPC message.
      webviewByContents.set(guest.id, webviewId)
      guestContentsIds.set(webviewId, guest.id)
      wireWebviewEvents(guest, webviewId)
      // Navigation needs the Rust loop, so acknowledge attachment immediately.
      guest.loadURL(webviewUrl(url)).catch((error) => log('guest load failed', webviewId, serializeError(error)))
      resolve(guest)
    }
    const onDestroyed = () => { cleanup(); reject(new Error('window page was destroyed')) }
    const timer = setTimeout(() => { cleanup(); reject(new Error('guest did not attach to the window page')) }, 10000)
    window.webContents.on('did-attach-webview', onAttach)
    window.webContents.once('destroyed', onDestroyed)
  })
  try {
    // WebContents.executeJavaScript waits for all loading resources; the frame
    // API can run as soon as this document's DOM is ready.
    const [guest] = await Promise.all([attached, window.webContents.mainFrame.executeJavaScript(code, true)])
    log('webview.attach', webviewId, 'attached', guest.id)
    return { webviewId }
  } finally {
    cleanup()
  }
}

/** The webContents that belongs to `webviewId`: the window page or a guest. */
function contentsFor(webviewId) {
  const guestId = guestContentsIds.get(webviewId)
  if (guestId !== undefined) {
    const guest = webContents.fromId(guestId)
    if (guest && !guest.isDestroyed()) return guest
  }
  return findWebview(webviewId).webContents
}

function readWebviewBounds(webviewId) {
  const window = findWebview(webviewId)
  const scaleFactor = scaleOf(window)
  const guestId = guestContentsIds.get(webviewId)
  if (guestId === undefined) {
    // the main webview fills the window content area, so its origin is (0, 0)
    const rect = window.getContentBounds()
    return {
      scaleFactor,
      bounds: {
        x: 0,
        y: 0,
        width: Math.round(rect.width * scaleFactor),
        height: Math.round(rect.height * scaleFactor),
      },
      contentBounds: {
        x: Math.round(rect.x * scaleFactor),
        y: Math.round(rect.y * scaleFactor),
        width: Math.round(rect.width * scaleFactor),
        height: Math.round(rect.height * scaleFactor),
      },
      visible: window.isVisible(),
    }
  }
  const elementId = `tauri-webview-${webviewId}`
  return pageReady(window).then(() => window.webContents.mainFrame
    .executeJavaScript(
      `(() => {
        const element = document.getElementById(${JSON.stringify(elementId)})
        if (!element) return null
        const rect = element.getBoundingClientRect()
        return { x: rect.x, y: rect.y, width: rect.width, height: rect.height }
      })()`,
      true
    ))
    .then((rect) => {
      const origin = window.getContentBounds()
      const toPhysical = (value) => Math.round(value * scaleFactor)
      return {
        scaleFactor,
        bounds: rect
          ? {
              x: toPhysical(rect.x),
              y: toPhysical(rect.y),
              width: toPhysical(rect.width),
              height: toPhysical(rect.height),
            }
          : { x: 0, y: 0, width: 0, height: 0 },
        contentBounds: { x: 0, y: 0, width: toPhysical(origin.width), height: toPhysical(origin.height) },
        visible: Boolean(rect),
      }
    })
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
    fullscreen:
      window.isFullScreen() ||
      (process.platform === 'darwin' && window.isSimpleFullScreen()),
    resizable: window.isResizable(),
    maximizable: maximizableState.get(window) ?? true,
    minimizable: minimizableState.get(window) ?? true,
    closable: closableState.get(window) ?? true,
    decorated: decoratedState.get(window) ?? true,
    alwaysOnTop: window.isAlwaysOnTop(),
    title: window.getTitle(),
  }
}

/** Tauri sizes and positions are physical pixels; Electron's are DIPs. */
function toDip(window, params, key) {
  const value = params[key]
  if (typeof value !== 'number') return undefined
  return params.physical ? Math.round(value / scaleOf(window)) : Math.round(value)
}

function applySizeConstraints(window) {
  const constraints = sizeConstraints.get(window)
  if (!constraints) return
  const bounds = window.getBounds()
  const content = window.getContentBounds()
  const frameWidth = Math.max(0, bounds.width - content.width)
  const frameHeight = Math.max(0, bounds.height - content.height)
  const outer = (value, frame) => value > 0 ? value + frame : 0
  window.setMinimumSize(outer(constraints.minWidth, frameWidth), outer(constraints.minHeight, frameHeight))
  window.setMaximumSize(outer(constraints.maxWidth, frameWidth), outer(constraints.maxHeight, frameHeight))
}

function setSizeConstraint(window, kind, params) {
  const constraints = sizeConstraints.get(window) ?? {}
  constraints[`${kind}Width`] = Math.max(0, toDip(window, params, 'width') ?? 0)
  constraints[`${kind}Height`] = Math.max(0, toDip(window, params, 'height') ?? 0)
  sizeConstraints.set(window, constraints)
  applySizeConstraints(window)
  const [width, height] = window.getContentSize()
  setWindowSize(window, { width, height })
}

function setWindowSize(window, params) {
  let width = toDip(window, params, 'width')
  let height = toDip(window, params, 'height')
  if (width === undefined || height === undefined) return
  const constraints = sizeConstraints.get(window) ?? {}
  const clamp = (value, min, max) => Math.max(1, min || 0, max > 0 ? Math.min(value, max) : value)
  width = clamp(width, constraints.minWidth, constraints.maxWidth)
  height = clamp(height, constraints.minHeight, constraints.maxHeight)
  window.setContentSize(width, height)
}

function setWindowPosition(window, params) {
  const x = toDip(window, params, 'x')
  const y = toDip(window, params, 'y')
  if (x === undefined || y === undefined) return
  window.setPosition(x, y)
}

/** Mirror muda's Win32 menu; the Rust owner remains authoritative for clicks. */
function setWindowMenu(windowId, menu) {
  const window = findWindow(windowId)
  const template = (items) => items.map((item) => {
    if (item.type === 'separator') return { type: 'separator' }
    const entry = {
      label: item.label,
      enabled: item.enabled,
      ...(item.accelerator ? { accelerator: item.accelerator } : {}),
    }
    if (item.submenu) entry.submenu = template(item.submenu)
    else {
      // Unchecked Win32 check items are indistinguishable from ordinary items.
      // Rust toggles them and the next snapshot supplies the visible checkmark.
      if (item.checked) Object.assign(entry, { type: 'checkbox', checked: true })
      entry.click = () => {
        if (!window.isDestroyed()) send({ event: 'menu.command', params: { windowId, commandId: item.commandId } })
      }
    }
    return entry
  })
  window.setMenu(menu === null ? null : Menu.buildFromTemplate(template(menu)))
  if (menu !== null) {
    window.setAutoHideMenuBar(false)
    window.setMenuBarVisibility(true)
  }
  return null
}

function handle(method, params) {
  if (process.env.TAURI_RUNTIME_ELECTRON_DEBUG) log('ipc', method, JSON.stringify(params).slice(0, 200))
  switch (method) {
    case 'window.create':
      return createWindow(params)
    case 'window.setMenu':
      return setWindowMenu(params.windowId, params.menu)
    case 'webview.create': {
      const window = windows.get(params.windowId)
      if (!window) throw new Error(`unknown window ${params.windowId}`)
      // `window.create` already hosts the page when its payload carries a
      // webview, so a later `webview.create` is always an additional guest.
      // A plain `Window` hosts no page at all: its children are guests too.
      if (!mainWebview.has(params.windowId)) {
        mainWebview.set(params.windowId, params.webviewId)
      }
      return attachWebviewElement(window, params)
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
    case 'window.minSize':
      setSizeConstraint(findWindow(params.windowId), 'min', params)
      return null
    case 'window.maxSize':
      setSizeConstraint(findWindow(params.windowId), 'max', params)
      return null
    case 'window.resizable':
      findWindow(params.windowId).setResizable(params.resizable)
      return null
    case 'window.maximizable': {
      // Electron only implements this on macOS and Windows; on Linux the value
      // lives here so the getter is honest about what Tauri asked for.
      const window = findWindow(params.windowId)
      maximizableState.set(window, params.maximizable !== false)
      if (process.platform === 'linux') {
        log('window.unsupported', params.windowId, 'set_maximizable')
      } else {
        window.setMaximizable(params.maximizable !== false)
      }
      return null
    }
    case 'window.minimizable': {
      const window = findWindow(params.windowId)
      minimizableState.set(window, params.minimizable !== false)
      if (process.platform === 'linux') {
        log('window.unsupported', params.windowId, 'set_minimizable')
      } else {
        window.setMinimizable(params.minimizable !== false)
      }
      return null
    }
    case 'window.closable': {
      const window = findWindow(params.windowId)
      // Our close handler enforces this everywhere; macOS also greys the button.
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
    case 'window.unminimize':
      findWindow(params.windowId).restore()
      return null
    case 'window.fullscreen': {
      const window = findWindow(params.windowId)
      window.setFullScreen(params.fullscreen)
      if (process.platform === 'darwin') {
        // AppKit silently refuses the space switch when there is no usable
        // space (VMs, CI runners, some multi-display setups). Simple fullscreen
        // needs no space, so fall back to it instead of doing nothing.
        setTimeout(() => {
          if (params.fullscreen && !window.isFullScreen() && !window.isSimpleFullScreen()) {
            window.setSimpleFullScreen(true)
          } else if (!params.fullscreen && window.isSimpleFullScreen()) {
            window.setSimpleFullScreen(false)
          }
        }, 600)
      }
      return null
    }
    case 'window.setParentWindow': {
      const child = findWindow(params.windowId)
      const parent = findWindow(params.parentId)
      if (child && parent) child.setParentWindow(parent)
      return null
    }
    case 'window.alwaysOnTop':
      findWindow(params.windowId).setAlwaysOnTop(params.alwaysOnTop)
      return null
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
      log('webview.navigate', params.url)
      // Like creation, navigation may request a Rust-backed custom protocol.
      // Acknowledge dispatch, not completion: the caller must keep pumping it.
      contentsFor(params.webviewId).loadURL(webviewUrl(params.url))
        .catch((error) => log('navigate failed', params.url, serializeError(error)))
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
      if (!guestContentsIds.has(params.webviewId)) return null
      const window = findWebview(params.webviewId)
      const elementId = `tauri-webview-${params.webviewId}`
      return pageReady(window)
        .then(() => window.webContents.mainFrame.executeJavaScript(`(() => {
          const element = document.getElementById(${JSON.stringify(elementId)})
          if (!element) throw new Error('unknown webview element')
          ;(${placeGuest.toString()})(element, null, ${Boolean(params.autoResize)})
        })()`, true))
        .then(() => null)
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
    case 'window.theme':
      return setWindowTheme(findWindow(params.windowId), params.theme)
    case 'window.ignoreCursorEvents':
      findWindow(params.windowId).setIgnoreMouseEvents(params.ignore, { forward: true })
      return null
    case 'window.setCursorIcon':
      return setWindowCursorCss(
        findWindow(params.windowId),
        `* { cursor: ${params.cursor} !important }`,
      )
    case 'window.setCursorVisible':
      return setWindowCursorCss(
        findWindow(params.windowId),
        params.visible ? null : '* { cursor: none !important }',
      )
    // Windows toggles the frame natively; this keeps the reported state in sync.
    case 'window.decorations':
      decoratedState.set(findWindow(params.windowId), params.decorations !== false)
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
      if (!guestContentsIds.has(params.webviewId)) return null
      const elementId = `tauri-webview-${params.webviewId}`
      const bounds = guestBounds(window, params)
      return pageReady(window)
        .then(() => window.webContents.mainFrame.executeJavaScript(
          `(() => {
            const el = document.getElementById(${JSON.stringify(elementId)})
            if (!el) throw new Error('unknown webview element')
            ;(${placeGuest.toString()})(el, ${JSON.stringify(bounds)})
          })()`,
          true,
        ))
        .then(() => null)
    }
    case 'webview.visibility': {
      const window = findWebview(params.webviewId)
      if (!guestContentsIds.has(params.webviewId)) {
        if (params.visible) window.show()
        else window.hide()
        return null
      }
      const elementId = `tauri-webview-${params.webviewId}`
      const visible = JSON.stringify(Boolean(params.visible))
      return pageReady(window).then(() => window.webContents.mainFrame
        .executeJavaScript(
          `(() => {
            const el = document.getElementById(${JSON.stringify(elementId)})
            if (el) el.style.visibility = ${visible} ? 'visible' : 'hidden'
          })()`,
          true
        ))
        .then(() => null)
    }
    case 'webview.backgroundColor': {
      const window = findWebview(params.webviewId)
      if (!guestContentsIds.has(params.webviewId)) return null
      const elementId = `tauri-webview-${params.webviewId}`
      return pageReady(window).then(() => window.webContents.mainFrame
        .executeJavaScript(
          `(() => {
            const el = document.getElementById(${JSON.stringify(elementId)})
            if (el) el.style.backgroundColor = ${JSON.stringify(params.color ?? 'transparent')}
          })()`,
          true
        ))
        .then(() => null)
    }
    case 'webview.focus': {
      const window = findWebview(params.webviewId)
      if (!guestContentsIds.has(params.webviewId)) {
        window.focus()
        return null
      }
      const elementId = `tauri-webview-${params.webviewId}`
      return pageReady(window).then(() => window.webContents.mainFrame
        .executeJavaScript(
          `(() => { document.getElementById(${JSON.stringify(elementId)})?.focus() })()`,
          true
        ))
        .then(() => null)
    }
    case 'webview.print': {
      contentsFor(params.webviewId).print({ silent: false })
      return null
    }
    case 'webview.close': {
      const guestId = guestContentsIds.get(params.webviewId)
      if (guestId === undefined) {
        findWebview(params.webviewId).close()
        return null
      }
      const window = findWebview(params.webviewId)
      const elementId = `tauri-webview-${params.webviewId}`
      return pageReady(window).then(() => window.webContents.mainFrame
        .executeJavaScript(
          `(() => { document.getElementById(${JSON.stringify(elementId)})?.remove() })()`,
          true
        ))
        .then(() => {
          webviewByContents.delete(guestId)
          guestContentsIds.delete(params.webviewId)
          return null
        })
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
      if (!registeredSchemes.size) {
        for (const transport of ['http', 'https']) protocol.handle(transport, dispatchHttpProtocol)
      }
      registeredSchemes.add(scheme)
      // A protocol Request cannot identify its native caller. Commands must
      // use the authenticated tauri-ipc bridge, never the first scheme owner.
      protocol.handle(scheme, scheme === 'ipc'
        ? () => new Response('Use native IPC', { status: 403 })
        : (request) => dispatchProtocol(scheme, request))
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
      if (process.env.TAURI_RUNTIME_ELECTRON_DEBUG) log('ipc done', message.method)
      send({ id: message.id, result: result ?? null })
    })
    .catch((error) => {
      log('error', message.method, serializeError(error))
      send({ id: message.id, error: serializeError(error) })
    })
}

function nativeProtocolRequest(event, request) {
  const webviewId = webviewByContents.get(event.sender.id)
  const frame = event.senderFrame
  if (webviewId === undefined || !frame || frame !== event.sender.mainFrame) {
    throw new Error('IPC requires a mapped main frame')
  }
  const url = new URL(request?.url)
  if (!((url.protocol === 'ipc:' && url.host === 'localhost') ||
    (['http:', 'https:'].includes(url.protocol) && url.host === 'ipc.localhost')) || url.username || url.password || url.hash) {
    throw new Error('Invalid IPC URL')
  }
  if (!['POST', 'OPTIONS'].includes(request.method) || !Array.isArray(request.headers) || !(request.body instanceof Uint8Array)) {
    throw new Error('Invalid IPC request')
  }
  const headers = new Headers(request.headers)
  // Renderer-supplied identity, Origin and Referer are never authoritative.
  headers.set('origin', new URL(frame.url).origin === 'null'
    ? `${new URL(frame.url).protocol}//${new URL(frame.url).host}` : new URL(frame.url).origin)
  headers.delete('referer')
  return {
    scheme: 'ipc', webviewId, pageUrl: frame.url, uri: `ipc://localhost${url.pathname}${url.search}`,
    method: request.method, headers: Object.fromEntries(headers), body: Buffer.from(request.body).toString('base64'),
  }
}

ipcMain.handle('tauri-protocol', (event, request) => {
  const params = nativeProtocolRequest(event, request)
  const id = nextProtocolId++
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => { protocolRequests.delete(id); reject(new Error('Native IPC response timed out')) }, 60000)
    protocolRequests.set(id, (reply) => { clearTimeout(timer); resolve(reply) })
    send({ event: 'protocol.request', params: { id, ...params } })
  })
})

ipcMain.on('tauri-ipc', (event, data) => {
  const webviewId = webviewByContents.get(event.sender.id)
  const frame = event.senderFrame
  if (webviewId === undefined || !frame || frame !== event.sender.mainFrame || typeof data !== 'string') {
    log('ipc message from unknown contents or subframe', event.sender.id)
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
  socket.on('error', () => app.quit())
})

// The Rust side owns the lifetime: no window means keep running until the
// socket closes.
app.on('window-all-closed', () => {})
