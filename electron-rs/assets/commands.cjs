'use strict'

/** Strategy for one cohesive RPC command family. */
class CommandStrategy {
  constructor(name, methods, execute) {
    this.name = name
    this.methods = new Set(methods)
    this.execute = execute
  }

  canHandle(method) {
    return this.methods.has(method)
  }
}

class CommandRouter {
  constructor(strategies) {
    this.strategies = strategies
  }

  execute(method, params) {
    const strategy = this.strategies.find((candidate) => candidate.canHandle(method))
    if (!strategy) throw new Error(`unknown method ${method}`)
    return strategy.execute(method, params)
  }
}

/** @param {import('./main.cjs').CommandContext} context */
function createCommandRouter(context) {
  const {
    state,
    app,
    nativeTheme,
    protocol,
    screen,
    os,
    log,
    trace,
    openDevtools,
    dispatchProtocol,
    cleanDownloadStage,
    finishDownload,
    installLocalhostProtocols,
    serializeError,
    setWindowBackgroundColor,
    configureWindowStyle,
    applyMaximizable,
    setTrafficLights,
    applyWindowCursorCss,
    setWindowCursor,
    readWindowTheme,
    emulateContentsTheme,
    setWindowTheme,
    createWindow,
    showWindow,
    createChildWebview,
    rememberChildBounds,
    setChildBounds,
    closeChildWebview,
    contentsFor,
    readWebviewBounds,
    findWebview,
    findWindow,
    scaleOf,
    displayForPoint,
    centerWindowOnDisplay,
    readDisplay,
    imageFromPayload,
    readGeometry,
    toDip,
    applySizeConstraints,
    updateSizeConstraints,
    setSizeConstraint,
    setWindowSize,
    setWindowPosition,
    setWindowFullscreen,
    setWindowMenu,
  } = context

  function executeWindow(method, params) {
    switch (method) {
      case 'window.create':
        return createWindow(params)
      case 'window.ready': {
        const window = findWindow(params.windowId)
        showWindow(window, state.windowConfigs.get(window) ?? {})
        return null
      }
      case 'window.setTitle': {
        const window = state.windows.get(params.windowId)
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
      case 'window.resizable': {
        const window = findWindow(params.windowId)
        window.setResizable(params.resizable)
        applyMaximizable(window)
        return null
      }
      case 'window.maximizable': {
        const window = findWindow(params.windowId)
        state.maximizableState.set(window, params.maximizable !== false)
        if (process.platform !== 'linux') window.setMaximizable(params.maximizable !== false)
        applyMaximizable(window)
        return null
      }
      case 'window.minimizable': {
        const window = findWindow(params.windowId)
        state.minimizableState.set(window, params.minimizable !== false)
        if (process.platform !== 'linux') window.setMinimizable(params.minimizable !== false)
        return null
      }
      case 'window.closable': {
        const window = findWindow(params.windowId)
        state.closableState.set(window, params.closable !== false)
        if (process.platform !== 'linux') window.setClosable(params.closable !== false)
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
      case 'window.fullscreen':
        return setWindowFullscreen(findWindow(params.windowId), params)
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
      case 'window.center': {
        const window = findWindow(params.windowId)
        if (process.platform === 'darwin') centerWindowOnDisplay(window)
        else window.center()
        return null
      }
      case 'window.requestAttention':
        findWindow(params.windowId).flashFrame(Boolean(params.flash))
        return null
      case 'window.show':
        state.windows.get(params.windowId)?.show()
        return null
      case 'window.hide':
        state.windows.get(params.windowId)?.hide()
        return null
      case 'window.focus': {
        // macOS `app.show()` restores windows but intentionally does not activate
        // the app; a Tauri focus request must make the Electron app frontmost.
        if (process.platform === 'darwin') app.focus({ steal: true })
        state.windows.get(params.windowId)?.focus()
        return null
      }
      case 'window.nativeHandle': {
        // Tauri's host-side code (menus, window effects, Win32 tweaks) needs the
        // native handle; the Buffer holds the HWND / NSView / X11 window id.
        const window = findWindow(params.windowId)
        const buffer = window.getNativeWindowHandle()
        const handle =
          buffer.length >= 8 ? Number(buffer.readBigUInt64LE(0)) : buffer.readUInt32LE(0)
        return { handle, processId: process.pid }
      }
      case 'window.close':
        state.windows.get(params.windowId)?.close()
        return null
      case 'window.destroy':
        state.closeAllowed.add(params.windowId)
        state.windows.get(params.windowId)?.destroy()
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
        const decorations = (params.value ?? params.decorations) !== false
        if (process.platform === 'darwin') configureWindowStyle(window, { decorations })
        window.__tauriDecorations = decorations
        applySizeConstraints(window)
        setImmediate(() => applySizeConstraints(window))
        return null
      }
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
        const background = setWindowBackgroundColor(window, params.color)
        if (window.__tauriMaterialBackground !== undefined) window.__tauriMaterialBackground = background
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
          window.__tauriMaterialBackground = window.__tauriBackgroundColor
        }
        const background = window.__tauriMaterialBackground ?? window.__tauriBackgroundColor
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
      case 'window.displayForPoint': {
        // Tao uses physical points on Windows and logical points on Unix desktops.
        const display = displayForPoint(params.x, params.y, process.platform === 'win32')
        return display ? readDisplay(display) : null
      }
      case 'window.titleBarStyle':
        configureWindowStyle(findWindow(params.windowId), { titleBarStyle: params.style })
        return null
      case 'window.trafficLight': {
        const window = findWindow(params.windowId)
        setTrafficLights(window, { x: toDip(window, params, 'x'), y: toDip(window, params, 'y') })
        return null
      }
      default: throw new Error(`unknown window method ${method}`)
    }
  }

  function executeWebview(method, params) {
    switch (method) {
      case 'webview.create': {
        const window = state.windows.get(params.windowId)
        if (!window) throw new Error(`unknown window ${params.windowId}`)
        return createChildWebview(window, params)
      }
      case 'webview.url':
        return contentsFor(params.webviewId).getURL()
      case 'webview.eval': {
        const contents = contentsFor(params.webviewId)
        // The runtime posts eval without waiting, so a failure is only visible here.
        trace('eval.start', contents.id, params.script.slice(0, 180))
        return contents.executeJavaScript(params.script, true).then((result) => {
          trace('eval.done', contents.id)
          return result
        }).catch((error) => {
          log('eval failed', contents.id, String(error), params.script.slice(0, 200))
          throw error
        })
      }
      case 'webview.newWindowResponse':
        state.socket.respond(params.id, params.response)
        return null
      case 'webview.navigationResponse': {
        state.navigationRequests.get(params.id)?.finish(params.allow === true)
        return null
      }
      case 'webview.downloadResponse': {
        const entry = state.downloadRequests.get(params.id)
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
        const callback = state.permissionRequests.get(params.id)
        state.permissionRequests.delete(params.id)
        // `default` keeps Electron's own behaviour, which is to deny.
        if (callback) callback(params.response === 'allow')
        return null
      }
      case 'webview.navigate':
        // Navigation can request a custom protocol from the Rust loop awaiting us.
        contentsFor(params.webviewId).loadURL(params.url).catch((error) => log('navigate failed', params.url, serializeError(error)))
        return null
      case 'webview.reload':
        contentsFor(params.webviewId).reload()
        return null
      case 'webview.setAutoResize': {
        const window = findWebview(params.webviewId)
        const child = state.childWebviews.get(params.webviewId)
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
      case 'webview.reparent': {
        const source = findWebview(params.webviewId)
        const destination = findWindow(params.windowId)
        if (source === destination) return null
        const child = state.childWebviews.get(params.webviewId)
        const previous = child.view.getBounds()
        const ratio = scaleOf(source) / scaleOf(destination)
        const bounds = Object.fromEntries(Object.entries(previous).map(([key, value]) => [key, Math.round(value * ratio)]))
        source.contentView.removeChildView(child.view)
        try {
          destination.contentView.addChildView(child.view)
          setChildBounds(destination, child, bounds)
        } catch (error) {
          destination.contentView.removeChildView(child.view)
          source.contentView.addChildView(child.view)
          setChildBounds(source, child, previous)
          throw error
        }
        if (state.mainViews.get(source) === child) state.mainViews.delete(source)
        child.main = !state.mainViews.has(destination)
        if (child.main) state.mainViews.set(destination, child)
        state.webviews.set(params.webviewId, destination)
        applyWindowCursorCss(destination, child.view.webContents).catch((error) => log('reparent.cursor', serializeError(error)))
        emulateContentsTheme(child.view.webContents, state.windowThemeState.get(destination).theme)
          .catch((error) => log('reparent.theme', serializeError(error)))
        return null
      }
      case 'webview.bounds':
        return readWebviewBounds(params.webviewId)
      case 'webview.setBounds': {
        const window = findWebview(params.webviewId)
        const child = state.childWebviews.get(params.webviewId)
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
        const child = state.childWebviews.get(params.webviewId)
        if (child) child.view.setVisible(Boolean(params.visible))
        else if (params.visible) window.show()
        else window.hide()
        return null
      }
      case 'webview.backgroundColor': {
        const child = state.childWebviews.get(params.webviewId)
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
        findWebview(params.webviewId)
        closeChildWebview(params.webviewId)
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
      default: throw new Error(`unknown webview method ${method}`)
    }
  }

  function executeProtocol(method, params) {
    switch (method) {
      case 'protocol.register': {
        const { scheme } = params
        if (state.registeredSchemes.has(scheme)) return null
        protocol.handle(scheme, (request) => dispatchProtocol(scheme, request))
        for (const target of state.customSessions) {
          target.protocol.handle(scheme, (request) => dispatchProtocol(scheme, request))
        }
        state.registeredSchemes.add(scheme)
        installLocalhostProtocols()
        return null
      }
      case 'protocol.respond': {
        const resolve = state.protocolRequests.get(params.id)
        state.protocolRequests.delete(params.id)
        if (resolve) resolve(params)
        return null
      }
      default: throw new Error(`unknown protocol method ${method}`)
    }
  }

  function executeApp(method, params) {
    switch (method) {
      case 'app.badgeLabel':
        // Only macOS has a dock badge label; Windows has the count above.
        if (process.platform === 'darwin' && app.dock) app.dock.setBadge(params.label ?? '')
        return null
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
      default: throw new Error(`unknown app method ${method}`)
    }
  }

  return new CommandRouter([
    new CommandStrategy('window', ["window.create","window.ready","window.setTitle","window.geometry","window.size","window.position","window.minSize","window.maxSize","window.sizeConstraints","window.resizable","window.maximizable","window.minimizable","window.closable","window.maximize","window.unmaximize","window.minimize","window.unminimize","window.fullscreen","window.alwaysOnTop","window.cursor","window.setCursorIcon","window.setCursorVisible","window.setParentWindow","window.setMenu","window.skipTaskbar","window.focusable","window.contentProtected","window.center","window.requestAttention","window.show","window.hide","window.focus","window.nativeHandle","window.close","window.destroy","window.visibleOnAllWorkspaces","window.icon","window.enabled","window.enabledQuery","window.simpleFullscreen","window.progressBar","window.shadow","window.decorations","window.badgeCount","window.overlayIcon","window.backgroundColor","window.backgroundMaterial","window.theme","window.ignoreCursorEvents","window.themeQuery","window.displays","window.cursorPosition","window.currentDisplay","window.displayForPoint","window.trafficLight","window.titleBarStyle"], executeWindow),
    new CommandStrategy('webview', ["webview.create","webview.url","webview.eval","webview.newWindowResponse","webview.navigationResponse","webview.downloadResponse","webview.permissionResponse","webview.navigate","webview.reload","webview.setAutoResize","webview.goBack","webview.canGoBack","webview.goForward","webview.canGoForward","webview.openDevtools","webview.closeDevtools","webview.isDevtoolsOpen","webview.setZoom","webview.reparent","webview.bounds","webview.setBounds","webview.visibility","webview.backgroundColor","webview.focus","webview.print","webview.close","webview.cookies","webview.setCookie","webview.deleteCookie","webview.clearBrowsingData"], executeWebview),
    new CommandStrategy('protocol', ["protocol.register","protocol.respond"], executeProtocol),
    new CommandStrategy('app', ["app.badgeLabel","app.focus","app.show","app.hide","app.activationPolicy","app.dockVisibility","app.theme"], executeApp),
  ])
}

module.exports = { createCommandRouter }

