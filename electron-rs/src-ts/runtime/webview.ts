import { EventEmitter } from 'node:events'
import type { Cookie, Rectangle, WebContents, WebContentsView } from 'electron'
import { cssColor, type Window } from './window'

export interface WebviewPolicy { navigation?: boolean; download?: boolean; permission?: boolean }
export interface WebviewBoundsInput { position?: { x: number; y: number; physical?: boolean }; size?: { width: number; height: number; physical?: boolean }; physical?: boolean }
export interface WebviewInit { webviewId: number; url: string; autoResize?: boolean; bounds?: WebviewBoundsInput; policy?: WebviewPolicy; [key: string]: unknown }
export interface WebviewSnapshot { scaleFactor: number; bounds: Rectangle; contentBounds: Rectangle; visible: boolean }
export interface CookieFilter { url?: string }
export interface CookieInit { url: string; name: string; value: string; domain?: string; path?: string; secure?: boolean; httpOnly?: boolean; sameSite?: 'unspecified' | 'no_restriction' | 'lax' | 'strict'; expirationDate?: number }
export interface WebviewEvents {
  navigationCommitted: [url: string]; loadStarted: [url: string]; loadFinished: [url: string]
  titleChanged: [title: string]; processGone: [details: { reason: string; exitCode: number }]; closed: []; reparented: [previous: Window]
}

type NativeView = WebContentsView & { getVisible?(): boolean; setVisible?(visible: boolean): void }
type NativeContents = WebContents & { navigationHistory?: { canGoBack(): boolean; goBack(): void; canGoForward(): boolean; goForward(): void } }

export class Webview extends EventEmitter<WebviewEvents> {
  private main = false
  private autoResize: boolean
  private policy?: WebviewPolicy
  private reference?: { bounds: Rectangle; parent: Rectangle }
  private attached = false
  private closed = false
  private nativePopup = false
  private openingDevtools?: Promise<void>
  private readonly nativeContents: WebContents
  readonly contentsId: number
  private readonly onResize = () => { if (!this.owner.destroyed) this.resizeWithParent(this.owner.native.getContentBounds()) }
  constructor(private owner: Window, readonly id: number, readonly view: WebContentsView, private config: WebviewInit) {
    super(); this.nativeContents = view.webContents; this.contentsId = this.nativeContents.id; this.autoResize = Boolean(config.autoResize); this.policy = config.policy; this.bindEvents()
  }
  get window(): Window { return this.owner }
  get contents(): WebContents { return this.nativeContents }
  get isMain(): boolean { return this.main }
  setMain(main: boolean): void { this.main = main }
  get webviewPolicy(): WebviewPolicy | undefined { return this.policy }
  private bindEvents(): void {
    const c = this.contents
    c.on('did-navigate', (_e, url) => this.emit('navigationCommitted', url))
    c.on('did-navigate-in-page', (_e, url, isMain) => { if (isMain) this.emit('navigationCommitted', url) })
    c.on('dom-ready', () => { if (this.attached) this.syncOwnerState() })
    c.on('did-start-loading', () => this.emit('loadStarted', c.getURL()))
    c.on('did-finish-load', () => this.emit('loadFinished', c.getURL()))
    c.on('page-title-updated', (_e, title) => this.emit('titleChanged', title))
    c.on('render-process-gone', (_e, details) => this.emit('processGone', { reason: details.reason, exitCode: details.exitCode }))
    c.once('destroyed', () => {
      const closePopup = this.attached && this.nativePopup && this.main
      this.finishClose()
      if (closePopup && !this.owner.destroyed) this.owner.destroy()
    })
  }
  attach(options: { main?: boolean; navigate?: boolean }): Promise<void> {
    if (this.attached) this.detach()
    this.main = Boolean(options.main)
    this.nativePopup = this.main && options.navigate === false
    const config = { ...this.owner.configuration, ...Object.fromEntries(Object.entries(this.config).filter(([, value]) => value != null)) }
    this.view.setBackgroundColor(cssColor(config.backgroundColor) ?? (config.transparent ? '#00000000' : '#ffffff'))
    if (config.userAgent != null) this.contents.setUserAgent(String(config.userAgent))
    this.owner.native.contentView.addChildView(this.view)
    this.attached = true
    this.owner.native.on('resize', this.onResize)
    const parent = this.owner.native.getContentBounds()
    const input = this.config.bounds ?? {}
    const position = input.position ?? { x: 0, y: 0 }
    const size = input.size ?? { width: parent.width, height: parent.height }
    this.setNativeBounds({
      x: this.toDip(position.x, input.position?.physical ?? input.physical),
      y: this.toDip(position.y, input.position?.physical ?? input.physical),
      width: this.toDip(size.width, input.size ? input.size.physical ?? input.physical : false),
      height: this.toDip(size.height, input.size ? input.size.physical ?? input.physical : false),
    })
    this.syncOwnerState()
    if (options.navigate !== false && this.config.url) this.navigate(this.config.url)
    return Promise.resolve()
  }
  private syncOwnerState(): void { this.owner.syncContents(this.contents) }
  detach(): void {
    if (!this.attached) return
    this.owner.native.removeListener('resize', this.onResize)
    if (!this.owner.destroyed) this.owner.native.contentView.removeChildView(this.view)
    this.attached = false
  }
  private finishClose(): void {
    if (this.closed) return
    this.closed = true
    this.detach()
    this.main = false
    this.emit('closed')
  }
  close(): void { if (this.closed) return; this.finishClose(); if (!this.contents.isDestroyed()) this.contents.close() }
  navigate(url: string): void { void this.contents.loadURL(url).catch((error) => console.error('navigate failed', url, error)) }
  reload(): void { this.contents.reload() }
  goBack(): void { const h = (this.contents as NativeContents).navigationHistory; if (h?.canGoBack()) h.goBack() }
  canGoBack(): boolean { return Boolean((this.contents as NativeContents).navigationHistory?.canGoBack()) }
  goForward(): void { const h = (this.contents as NativeContents).navigationHistory; if (h?.canGoForward()) h.goForward() }
  canGoForward(): boolean { return Boolean((this.contents as NativeContents).navigationHistory?.canGoForward()) }
  evaluate(script: string): Promise<unknown> { return this.contents.executeJavaScript(script, true) }
  snapshot(): WebviewSnapshot { const scaleFactor = this.owner.scale; const contentBounds = this.owner.native.getContentBounds(); const c = this.view as NativeView; return { scaleFactor, bounds: this.physical(this.view.getBounds(), scaleFactor), contentBounds: this.physical(contentBounds, scaleFactor), visible: !this.owner.destroyed && this.owner.native.isVisible() && (c.getVisible?.() ?? true) } }
  private physical(rect: Rectangle, scale: number): Rectangle { return { x: Math.round(rect.x * scale), y: Math.round(rect.y * scale), width: Math.round(rect.width * scale), height: Math.round(rect.height * scale) } }
  private setNativeBounds(bounds: Rectangle): void { if (!Object.values(bounds).every(Number.isFinite) || bounds.width < 0 || bounds.height < 0) throw new Error('invalid child webview bounds'); this.view.setBounds(bounds); this.reference = { bounds: { ...bounds }, parent: this.owner.native.getContentBounds() } }
  private toDip(value: number, physical?: boolean): number { return Math.round(value / (physical ? this.owner.scale : 1)) }
  setBounds(input: WebviewBoundsInput): void {
    const bounds = { ...this.view.getBounds() }
    if (input.position) {
      bounds.x = this.toDip(input.position.x, input.position.physical ?? input.physical)
      bounds.y = this.toDip(input.position.y, input.position.physical ?? input.physical)
    }
    if (input.size) {
      bounds.width = this.toDip(input.size.width, input.size.physical ?? input.physical)
      bounds.height = this.toDip(input.size.height, input.size.physical ?? input.physical)
    }
    this.setNativeBounds(bounds)
  }
  setAutoResize(autoResize: boolean): void { this.autoResize = autoResize; this.reference = { bounds: this.view.getBounds(), parent: this.owner.native.getContentBounds() } }
  resizeWithParent(parent: Rectangle): void {
    if (!this.attached || !this.autoResize || !this.reference || parent.width <= 0 || parent.height <= 0) return
    const r = this.reference
    if (r.parent.width <= 0 || r.parent.height <= 0) { this.reference = { bounds: this.view.getBounds(), parent }; return }
    const x = Math.round(r.bounds.x * parent.width / r.parent.width), y = Math.round(r.bounds.y * parent.height / r.parent.height)
    this.view.setBounds({ x, y, width: Math.round((r.bounds.x + r.bounds.width) * parent.width / r.parent.width) - x, height: Math.round((r.bounds.y + r.bounds.height) * parent.height / r.parent.height) - y })
  }
  setVisible(visible: boolean): void { (this.view as NativeView).setVisible?.(visible) }
  setBackgroundColor(color: string | undefined): void { this.view.setBackgroundColor(color ?? '#00000000') }
  focus(): void { this.owner.focus(); this.contents.focus() }
  print(): Promise<void> { this.contents.print({ silent: false }); return Promise.resolve() }
  openDevtools(): Promise<void> {
    if (this.openingDevtools) return this.openingDevtools
    const contents = this.contents
    if (contents.devToolsWebContents) return Promise.resolve()
    let finish: (error?: unknown) => void = () => {}
    const opening = new Promise<void>((resolve, reject) => {
      let settled = false
      finish = (error) => {
        if (settled) return
        settled = true
        clearTimeout(timer)
        contents.removeListener('devtools-opened', opened)
        contents.removeListener('devtools-closed', closed)
        contents.removeListener('destroyed', destroyed)
        this.openingDevtools = undefined
        if (error) reject(error)
        else resolve()
      }
      const opened = () => finish()
      const closed = () => finish(new Error('DevTools closed before opening completed'))
      const destroyed = () => finish(new Error('webview destroyed while opening DevTools'))
      const timer = setTimeout(() => finish(new Error('DevTools opening timed out')), 10_000)
      contents.once('devtools-opened', opened)
      contents.once('devtools-closed', closed)
      contents.once('destroyed', destroyed)
    })
    this.openingDevtools = opening
    try { contents.openDevTools({ mode: 'detach' }) } catch (error) { finish(error) }
    return opening
  }
  closeDevtools(): void { this.contents.closeDevTools() }
  isDevtoolsOpen(): boolean { return this.contents.isDevToolsOpened() }
  setZoom(scaleFactor: number): void { if (!Number.isFinite(scaleFactor) || scaleFactor <= 0) throw new Error('invalid zoom factor'); this.contents.setZoomFactor(scaleFactor) }
  cookies(filter: CookieFilter): Promise<Cookie[]> { return this.contents.session.cookies.get(filter.url ? { url: filter.url } : {}) }
  setCookie(cookie: CookieInit): Promise<null> { const { url, name, value, ...rest } = cookie; return this.contents.session.cookies.set({ url, name, value, ...rest }).then(() => null) }
  deleteCookie(filter: { url: string; name: string }): Promise<null> { return this.contents.session.cookies.remove(filter.url, filter.name).then(() => null) }
  clearBrowsingData(): Promise<null> { return this.contents.session.clearStorageData().then(() => null) }
  reparent(destination: Window): void {
    if (destination === this.owner) return
    const previousOwner = this.owner, previous = this.view.getBounds(), reference = this.reference
    const ratio = previousOwner.scale / destination.scale
    const bounds = Object.fromEntries(Object.entries(previous).map(([key, value]) => [key, Math.round(value * ratio)])) as unknown as Rectangle
    previousOwner.native.contentView.removeChildView(this.view)
    try {
      destination.native.contentView.addChildView(this.view)
      this.owner = destination
      this.setNativeBounds(bounds)
    } catch (error) {
      this.owner = previousOwner
      destination.native.contentView.removeChildView(this.view)
      previousOwner.native.contentView.addChildView(this.view)
      this.view.setBounds(previous)
      this.reference = reference
      throw error
    }
    if (this.attached) {
      previousOwner.native.removeListener('resize', this.onResize)
      destination.native.on('resize', this.onResize)
    }
    this.syncOwnerState()
    this.emit('reparented', previousOwner)
  }
  setPolicy(policy: WebviewPolicy | undefined): void { this.policy = policy }
}

export class Webviews {
  private readonly byId = new Map<number, Webview>(); private readonly byContents = new Map<number, Webview>()
  add(webview: Webview): void {
    if (this.byId.has(webview.id)) throw new Error(`duplicate webview ${webview.id}`)
    this.byId.set(webview.id, webview)
    this.byContents.set(webview.contentsId, webview)
    webview.on('reparented', () => webview.setMain(!this.of(webview.window).some((other) => other !== webview && other.isMain)))
  }
  get(id: number): Webview { const result = this.byId.get(id); if (!result || result.window.destroyed || result.contents.isDestroyed()) throw new Error(`unknown webview ${id}`); return result }
  peek(id: number): Webview | undefined { return this.byId.get(id) }
  forContents(contentsId: number): Webview | undefined { return this.byContents.get(contentsId) }
  of(window: Window): Webview[] { return [...this.byId.values()].filter((webview) => webview.window === window) }
  main(window: Window): Webview | undefined { return this.of(window).find((webview) => webview.isMain) }
  all(): Webview[] { return [...this.byId.values()] }
  delete(id: number): void { const webview = this.byId.get(id); if (!webview) return; this.byId.delete(id); this.byContents.delete(webview.contentsId) }
  clear(): void { this.byId.clear(); this.byContents.clear() }
}
