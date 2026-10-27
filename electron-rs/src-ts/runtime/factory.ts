import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import { app, BaseWindow, nativeImage, session, WebContentsView } from 'electron'
import type { BrowserWindowConstructorOptions, Session, WebContents } from 'electron'
import type { Policies, PopupPlan } from './policy'
import type { Protocols } from './protocol'
import type { Transport } from '../socket/transport'
import { Webview, type WebviewInit, type Webviews } from './webview'
import { Window, normalizeConstraint, cssColor, type SizeConstraints, type ConstraintInput, type WindowConfig, type WindowEnvironment, type Windows } from './window'

export interface WindowFactory {
  createWindow(params: Record<string, any>): { windowId: number }
  createWebview(params: Record<string, any>): { webviewId: number }
}

export interface FactoryDependencies {
  readonly windows: Windows
  readonly webviews: Webviews
  readonly policies: Policies
  readonly protocols: Protocols
  readonly transport: Transport
  readonly environment: WindowEnvironment
  readonly popups: Map<number, PopupPlan>
}

type Params = Record<string, any>
type PopupContext = { request: number; opener: Webview }

export class Factory implements WindowFactory {
  private readonly customSessions = new Set<Session>()

  constructor(private readonly deps: FactoryDependencies) {}

  createWindow(params: Params): { windowId: number } {
    const { windowId } = params
    if (!Number.isSafeInteger(windowId) || this.deps.windows.peek(windowId)) throw new Error('invalid or duplicate window ID')
    const config: WindowConfig = params.window ?? params
    for (const key of ['width', 'height', 'x', 'y']) {
      const value = config[key]
      if (value != null && (typeof value !== 'number' || !Number.isFinite(value) || (['width', 'height'].includes(key) && value <= 0))) throw new Error('invalid window geometry')
    }
    const constraints: SizeConstraints = {}
    for (const key of ['minWidth', 'minHeight', 'maxWidth', 'maxHeight'] as const) {
      constraints[key] = normalizeConstraint((params.sizeConstraints && Object.hasOwn(params.sizeConstraints, key) ? params.sizeConstraints[key] : config[key]) as ConstraintInput)
    }
    const popup = params.newWindowRequest !== undefined
    const active = (this.deps.policies as Policies & { activePopup?: PopupContext }).activePopup
    if (popup && (!Number.isSafeInteger(params.newWindowRequest) || !active || active.request !== params.newWindowRequest || this.deps.popups.has(active.request))) throw new Error('invalid or consumed native popup opener')
    const webview = params.webview as WebviewInit | undefined
    if (popup && !webview) throw new Error('native popup requires a main webview')
    const targetSession = this.sessionFor(webview)
    if (popup && targetSession && targetSession !== active?.opener.contents.session) throw new Error('native popup target session must match its opener; incognito/dataDirectory cannot be changed')
    const options = this.browserWindowOptions(config, params, popup ? active!.opener.contents.session : targetSession ?? session.defaultSession)
    const window = new Window(this.deps.environment, new BaseWindow({ ...options, show: false }), windowId, config, constraints)
    this.deps.windows.add(window)
    this.wireWindow(window)
    try {
      window.initialize()
      if (webview) {
        if (!Number.isSafeInteger(webview.webviewId) || this.deps.webviews.peek(webview.webviewId)) throw new Error('invalid or duplicate webview ID')
        if (popup) {
          const request = params.newWindowRequest as number
          const plan: PopupPlan = {
            window, options,
            attach: (contents: WebContents) => {
              // Chrome created this guest with the opener; keep that real relationship and session.
              const page = new Webview(window, webview.webviewId, new WebContentsView({ webContents: contents, webPreferences: options.webPreferences }), { ...webview, autoResize: true })
              this.deps.webviews.add(page)
              this.wireWebview(page)
              this.deps.policies.attach(page)
              void page.attach({ main: true, navigate: false })
              if (!params.deferShow) window.reveal()
            },
          }
          this.deps.popups.set(request, plan)
        } else {
          const page = new Webview(window, webview.webviewId, new WebContentsView({ webPreferences: options.webPreferences }), { ...webview, autoResize: true })
          this.deps.webviews.add(page)
          this.wireWebview(page)
          this.deps.policies.attach(page)
          void page.attach({ main: true, navigate: true })
        }
      }
      if (!popup && !params.deferShow) window.reveal()
      return { windowId }
    } catch (error) {
      window.destroy()
      throw error
    }
  }

  createWebview(params: Params): { webviewId: number } {
    const { webviewId } = params
    if (!Number.isSafeInteger(webviewId) || this.deps.webviews.peek(webviewId)) throw new Error('invalid or duplicate webview ID')
    const window = this.deps.windows.get(params.windowId)
    const init = { ...params, webviewId } as WebviewInit
    const config = { ...window.configuration, ...Object.fromEntries(Object.entries(params).filter(([, value]) => value != null)) } as WindowConfig
    const targetSession = this.sessionFor(init) ?? this.deps.webviews.main(window)?.contents.session ?? session.defaultSession
    const view = new WebContentsView({ webPreferences: this.webPreferences(init, config, targetSession) })
    const webview = new Webview(window, webviewId, view, init)
    this.deps.webviews.add(webview)
    try {
      this.wireWebview(webview)
      this.deps.policies.attach(webview)
      void webview.attach({ navigate: true })
      return { webviewId }
    } catch (error) {
      webview.close()
      throw error
    }
  }

  private sessionFor(params?: Params | null): Session | undefined {
    if (!params) return undefined
    let target: Session | undefined
    if (params.incognito) target = session.fromPartition(`tauri-private-${++Factory.nextPrivateSession}`)
    else if (params.dataDirectory) target = session.fromPath(path.resolve(params.dataDirectory))
    if (target && !this.customSessions.has(target)) {
      this.customSessions.add(target)
      this.deps.protocols.prepare(target)
    }
    return target
  }

  private static nextPrivateSession = 0

  private browserWindowOptions(config: WindowConfig, params: Params, targetSession: Session): BrowserWindowConstructorOptions {
    let icon: BrowserWindowConstructorOptions['icon']
    if (params.windowIcon) {
      const { width, height, bgra } = params.windowIcon
      const bitmap = Buffer.from(bgra, 'base64')
      if (!Number.isSafeInteger(width) || !Number.isSafeInteger(height) || width <= 0 || height <= 0 || bitmap.length !== width * height * 4) throw new Error('invalid window icon')
      icon = nativeImage.createFromBitmap(bitmap, { width, height, scaleFactor: 1 })
    } else if (typeof params.icon === 'string') icon = params.icon.replace(/^\\\\\?\\/, '')
    const style = String(config.titleBarStyle ?? 'visible').toLowerCase()
    if (!['visible', 'transparent', 'overlay'].includes(style)) throw new Error('invalid title bar style')
    const webview = params.webview ?? {}
    const pageConfig = { ...config, ...Object.fromEntries(Object.entries(webview).filter(([, value]) => value != null)) } as WindowConfig
    const color = cssColor(config.backgroundColor)
    const effects = (config.windowEffects as { effects?: string[] } | undefined)?.effects ?? []
    const material = process.platform !== 'win32' ? undefined
      : effects.some(effect => ['mica', 'micaDark', 'micaLight'].includes(effect)) ? 'mica'
        : effects.some(effect => ['tabbed', 'tabbedDark', 'tabbedLight'].includes(effect)) ? 'tabbed'
          : effects.some(effect => ['acrylic', 'blur'].includes(effect)) ? 'acrylic' : undefined
    return {
      title: config.title ?? 'Electron Runtime', useContentSize: true,
      width: Math.max(1, Math.round(config.width ?? 800)), height: Math.max(1, Math.round(config.height ?? 600)),
      x: config.x == null ? undefined : Math.round(config.x), y: config.y == null ? undefined : Math.round(config.y),
      center: config.center ?? false, resizable: config.resizable ?? true,
      frame: config.decorations ?? true, transparent: config.transparent ?? false, show: false,
      fullscreen: config.fullscreen ?? false, fullscreenable: true,
      maximizable: config.maximizable ?? true, minimizable: config.minimizable ?? true,
      closable: config.closable ?? true, focusable: process.platform === 'linux' ? true : config.focusable ?? true,
      alwaysOnTop: config.alwaysOnTop ?? false, skipTaskbar: config.skipTaskbar ?? false,
      hasShadow: config.shadow ?? true, autoHideMenuBar: true,
      titleBarStyle: process.platform === 'darwin' ? 'default' : config.hiddenTitle || config.titleBarHidden || style === 'transparent' ? 'hidden' : style === 'overlay' ? 'hiddenInset' : 'default',
      acceptFirstMouse: pageConfig.acceptFirstMouse === true,
      ...(config.titleBarOverlay ? { titleBarOverlay: true } : {}),
      ...(config.trafficLightPosition ? { trafficLightPosition: config.trafficLightPosition } : {}),
      ...(typeof config.tabbingIdentifier === 'string' ? { tabbingIdentifier: config.tabbingIdentifier } : {}),
      ...(material ? { backgroundMaterial: material } : {}),
      ...(color ? { backgroundColor: color } : {}), ...(icon ? { icon } : {}),
      webPreferences: this.webPreferences(webview, pageConfig, targetSession),
    }
  }

  private webPreferences(webview: Params, config: WindowConfig, target = this.sessionFor(webview)): BrowserWindowConstructorOptions['webPreferences'] {
    return {
      contextIsolation: true, sandbox: true, nodeIntegration: false, webviewTag: false,
      nodeIntegrationInSubFrames: true, session: target,
      preload: this.writeInitPreload(webview.webviewId, webview.initScripts),
      javascript: config.javascriptDisabled !== true, devTools: config.devtools !== false,
      backgroundThrottling: config.backgroundThrottling !== 'disabled',
    }
  }

  private writeInitPreload(webviewId?: number, initScripts?: unknown[]): string {
    const scripts = (initScripts ?? []).map((item) => typeof item === 'string' ? { script: item, mainFrameOnly: true } : item)
    if (!scripts.length) return path.join(__dirname, '..', 'preload.cjs')
    const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'tauri-runtime-electron-preload-'))
    const file = path.join(directory, `${webviewId ?? 'guest'}.cjs`)
    app.once('will-quit', () => fs.rmSync(directory, { recursive: true, force: true }))
    const source = `${fs.readFileSync(path.join(__dirname, '..', 'preload.cjs'), 'utf8')}\nfor (const entry of ${JSON.stringify(scripts)}) {\n  if (entry.mainFrameOnly && !process.isMainFrame) continue\n  try { require('electron').webFrame.executeJavaScript(entry.script) } catch (error) { console.error('init script failed', error) }\n}\n`
    fs.writeFileSync(file, source)
    return file
  }

  private wireWindow(window: Window): void {
    const id = window.id
    window.on('closeRequested', () => this.deps.transport.send('window.closeRequested', { windowId: id }))
    window.on('menuCommand', (commandId) => this.deps.transport.send('menu.command', { windowId: id, commandId }))
    window.on('themeChanged', (payload) => this.deps.transport.send('window.themeChanged', { windowId: id, ...payload }))
    const emit = (event: string) => { if (!window.destroyed) this.deps.transport.send('window.event', { windowId: id, event, ...window.geometry() }) }
    for (const [nativeEvent, event] of [['resize', 'resize'], ['move', 'move'], ['focus', 'focus'], ['blur', 'blur'], ['maximize', 'maximize'], ['unmaximize', 'unmaximize'], ['enter-full-screen', 'enter-full-screen'], ['leave-full-screen', 'leave-full-screen']] as const) (window.native as any).on(nativeEvent, () => emit(event))
    window.native.on('close', (event) => { if (!this.deps.transport.closed && window.requestClose()) event.preventDefault() })
    window.native.once('closed', () => {
      for (const [request, plan] of this.deps.popups) if (plan.window === window) this.deps.popups.delete(request)
      for (const page of this.deps.webviews.of(window)) page.close()
      this.deps.transport.send('window.closed', { windowId: id })
    })
  }

  private wireWebview(webview: Webview): void {
    const { id } = webview
    webview.on('navigationCommitted', (url) => this.deps.transport.send('webview.navigationCommitted', { webviewId: id, url }))
    webview.on('loadStarted', (url) => this.deps.transport.send('webview.loadStarted', { webviewId: id, url }))
    webview.on('loadFinished', (url) => this.deps.transport.send('webview.loadFinished', { webviewId: id, url }))
    webview.on('titleChanged', (title) => this.deps.transport.send('webview.titleChanged', { webviewId: id, title }))
    webview.on('processGone', (details) => this.deps.transport.send('webview.processGone', { webviewId: id, ...details }))
    webview.on('closed', () => {
      this.deps.webviews.delete(id)
      this.deps.transport.send('webview.closed', { webviewId: id })
    })
  }
}
