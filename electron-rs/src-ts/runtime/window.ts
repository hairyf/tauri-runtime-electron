import { EventEmitter } from 'node:events'
import os from 'node:os'
import { Menu, nativeImage } from 'electron'
import type { App, BaseWindow, BrowserWindow, NativeImage, NativeTheme, Rectangle, WebContents } from 'electron'
import { displayForBounds, findDisplay, type DisplayInfo, type DisplaySource } from './display'
import { readContentsTheme } from './native/theme'
import type { AppKit } from './native/appkit'

export type Theme = 'dark' | 'light' | null
export interface SizeConstraint { value: number; physical: boolean }
export type SizeConstraints = Partial<Record<'minWidth' | 'minHeight' | 'maxWidth' | 'maxHeight', SizeConstraint | null>>
export type ConstraintInput = number | { value?: number; physical?: boolean } | null | undefined
export interface CursorState { visible: boolean; cursor?: string }
export interface WindowGeometry {
  scaleFactor: number; bounds: Rectangle; contentBounds: Rectangle; contentConstraints: Record<string, number>
  maximized: boolean; minimized: boolean; focused: boolean; visible: boolean; fullscreen: boolean
  resizable: boolean; alwaysOnTop: boolean; decorated: boolean; title: string
  maximizable: boolean; minimizable: boolean; closable: boolean
}
export interface ThemeChange { theme: 'dark' | 'light'; dark: boolean }
export interface WindowConfig extends Record<string, unknown> {
  title?: string; width?: number; height?: number; x?: number; y?: number; transparent?: boolean
  decorations?: boolean; visible?: boolean; focus?: boolean; focusable?: boolean; center?: boolean
  maximized?: boolean; fullscreen?: boolean; resizable?: boolean; maximizable?: boolean
  minimizable?: boolean; closable?: boolean; alwaysOnTop?: boolean; skipTaskbar?: boolean
  shadow?: boolean; backgroundColor?: string | number[]; titleBarStyle?: string; titleBarHidden?: boolean
  hiddenTitle?: boolean; trafficLightPosition?: { x: number; y: number }
  visibleOnAllWorkspaces?: boolean; contentProtected?: boolean
  preventOverflow?: boolean | { width?: number; height?: number }; theme?: string | null
}
export interface WindowMenuItem {
  type?: 'separator'; label?: string; enabled?: boolean; accelerator?: string
  checked?: boolean; commandId?: number; submenu?: WindowMenuItem[]
}
export interface SizeParams { width?: number; height?: number; physical?: boolean }
export interface PositionParams { x?: number; y?: number; physical?: boolean }
export interface FullscreenParams { fullscreen: boolean; x?: number; y?: number }
export interface IconPayload { width?: number; height?: number; rgba?: number[] }
export interface WindowEnvironment extends DisplaySource {
  readonly nativeTheme: NativeTheme
  readonly app: App
  appKit(): AppKit
  contents(window: Window): WebContents[]
  applyCursor(contents: WebContents, cursor: Readonly<CursorState>): Promise<void>
  applyTheme(contents: WebContents, theme: Theme): Promise<void>
}
export interface WindowEvents {
  closeRequested: []; menuCommand: [commandId: number | undefined]; themeChanged: [payload: ThemeChange]
}
type NativeWindow = BaseWindow & {
  setSimpleFullScreen?(fullscreen: boolean): void
  isSimpleFullScreen?(): boolean
  setBackgroundMaterial?(material: 'none' | 'acrylic' | 'mica' | 'tabbed'): void
}
type FullscreenRestore = { bounds: Rectangle; maximized: boolean }

export class Window extends EventEmitter<WindowEvents> {
  private closable: boolean
  private maximizable: boolean
  private decorations: boolean
  private background: string
  private materialBackground?: string
  private cursor: CursorState = { visible: true }
  private fullscreenBounds?: FullscreenRestore
  private theme: Theme = null
  private themeQueue: Promise<unknown> = Promise.resolve()
  private appliedConstraints?: string
  private allowClose = false
  private unsubscribeTheme?: () => void
  private readonly onMetricsChanged = () => this.applyConstraints()

  constructor(private readonly env: WindowEnvironment, readonly native: NativeWindow, readonly id: number,
    private config: WindowConfig, private constraints: SizeConstraints) {
    super()
    this.closable = config.closable !== false
    this.maximizable = config.maximizable !== false
    this.decorations = config.decorations !== false
    this.background = cssColor(config.backgroundColor) ?? (config.transparent ? '#00000000' : '#ffffff')
  }

  initialize(): void {
    this.native.once('closed', () => this.dispose())
    if (this.env.platform === 'darwin') this.configureStyle()
    if (this.env.platform === 'linux') this.setBackgroundColor(this.config.backgroundColor)
    if (this.env.platform === 'linux' && this.config.focusable === false) this.native.setFocusable(false)
    this.native.on('resize', this.onMetricsChanged)
    this.native.on('move', this.onMetricsChanged)
    this.native.on('maximize', this.onMetricsChanged)
    this.native.on('unmaximize', this.onMetricsChanged)
    this.native.on('enter-full-screen', this.onMetricsChanged)
    this.native.on('leave-full-screen', this.onMetricsChanged)
    this.native.on('leave-full-screen', () => this.restoreFullscreen())
    this.env.screen.on('display-metrics-changed', this.onMetricsChanged)
    this.applyConstraints()
    this.setSize({ width: this.config.width ?? 800, height: this.config.height ?? 600 })
    if (this.config.maximized && !this.config.fullscreen) this.native.maximize()
    if (this.env.platform === 'darwin' && this.config.center && !this.config.maximized && !this.config.fullscreen) this.center()
    if (this.config.visibleOnAllWorkspaces) this.setVisibleOnAllWorkspaces(true)
    if (this.config.contentProtected) this.setContentProtection(true)
    if (this.config.preventOverflow) {
      const bounds = this.native.getBounds(), area = this.env.screen.getDisplayMatching(bounds).workArea
      const margin = typeof this.config.preventOverflow === 'object' ? this.config.preventOverflow : {}
      const width = Math.max(1, Math.min(bounds.width, area.width - (margin.width ?? 0)))
      const height = Math.max(1, Math.min(bounds.height, area.height - (margin.height ?? 0)))
      this.native.setBounds({ x: Math.min(Math.max(bounds.x, area.x), area.x + area.width - width), y: Math.min(Math.max(bounds.y, area.y), area.y + area.height - height), width, height })
    }
    this.installThemeSync()
  }

  get configuration(): Readonly<WindowConfig> { return this.config }
  get destroyed(): boolean { return this.native.isDestroyed() }
  get themeOverride(): Theme { return this.theme }

  requestClose(): boolean {
    if (this.allowClose || this.destroyed) return false
    if (this.closable) this.emit('closeRequested')
    return true
  }
  destroy(): void { if (!this.destroyed) { this.allowClose = true; this.native.destroy() } }
  reveal(): void {
    if (this.destroyed || this.config.visible === false) return
    if (this.config.focus === false || this.config.focusable === false) this.native.showInactive()
    else this.native.show()
  }
  dispose(): void {
    this.unsubscribeTheme?.(); this.unsubscribeTheme = undefined
    this.env.screen.removeListener('display-metrics-changed', this.onMetricsChanged)
    const emitter = this.native as unknown as NodeJS.EventEmitter
    for (const event of ['resize', 'move', 'maximize', 'unmaximize', 'enter-full-screen', 'leave-full-screen'])
      emitter.removeListener(event, this.onMetricsChanged)
  }

  get scale(): number { return this.env.screen.getDisplayMatching(this.native.getBounds()).scaleFactor || 1 }
  get contentConstraints(): Record<string, number> {
    return Object.fromEntries(Object.entries(this.constraints).map(([key, axis]) => [key, axis ? Math.round(axis.value / (axis.physical ? this.scale : 1)) : 0]))
  }
  physical(rect: Rectangle, scale = this.scale): Rectangle {
    if (this.env.platform === 'win32') return this.env.screen.dipToScreenRect(this.native as BrowserWindow, rect)
    return { x: Math.round(rect.x * scale), y: Math.round(rect.y * scale), width: Math.round(rect.width * scale), height: Math.round(rect.height * scale) }
  }
  toDip(params: { physical?: boolean }, key: 'x' | 'y' | 'width' | 'height'): number | undefined {
    const value = (params as Record<string, unknown>)[key]
    if (typeof value !== 'number') return undefined
    if (params.physical && this.env.platform === 'win32' && (key === 'x' || key === 'y')) {
      const point = this.env.screen.screenToDipPoint({
        x: key === 'x' ? value : Number((params as Record<string, unknown>).x) || 0,
        y: key === 'y' ? value : Number((params as Record<string, unknown>).y) || 0,
      })
      return Math.round(point[key])
    }
    return Math.round(value / (params.physical ? this.scale : 1))
  }
  geometry(): WindowGeometry {
    const w = this.native
    return {
      scaleFactor: this.scale, bounds: this.physical(w.getBounds()), contentBounds: this.physical(w.getContentBounds()),
      contentConstraints: this.contentConstraints, maximized: w.isMaximized(), minimized: w.isMinimized(), focused: w.isFocused(), visible: w.isVisible(),
      fullscreen: w.isFullScreen() || (this.env.platform === 'darwin' && Boolean(w.isSimpleFullScreen?.())),
      resizable: w.isResizable(), alwaysOnTop: w.isAlwaysOnTop(),
      decorated: this.env.platform === 'darwin' ? this.env.appKit().isDecorated(w.getNativeWindowHandle()) : this.decorations,
      title: w.getTitle(), maximizable: this.env.platform === 'darwin' ? this.env.appKit().isMaximizable(w.getNativeWindowHandle()) : w.isMaximizable(),
      minimizable: w.isMinimizable(), closable: this.env.platform === 'linux' ? this.closable : w.isClosable(),
    }
  }
  setSize(params: SizeParams): void {
    let width = this.toDip(params, 'width'), height = this.toDip(params, 'height')
    if (width === undefined || height === undefined) return
    if (!Number.isFinite(width) || !Number.isFinite(height)) throw new Error('invalid window size')
    const c = this.contentConstraints
    const clamp = (value: number, min = 0, max = 0) => Math.max(1, min, max > 0 ? Math.min(value, max) : value)
    this.native.setContentSize(clamp(width, c.minWidth, c.maxWidth), clamp(height, c.minHeight, c.maxHeight))
  }
  setPosition(params: PositionParams): void {
    if (typeof params.x !== 'number' || typeof params.y !== 'number') return
    if (params.physical && this.env.platform === 'win32') {
      const point = this.env.screen.screenToDipPoint({ x: params.x, y: params.y }); this.native.setPosition(point.x, point.y); return
    }
    const x = this.toDip(params, 'x'), y = this.toDip(params, 'y')
    if (x !== undefined && y !== undefined) this.native.setPosition(x, y)
  }
  setMinSize(params: SizeParams): void { this.setConstraints({ minWidth: sizeLimit(params.width, params.physical), minHeight: sizeLimit(params.height, params.physical) }) }
  setMaxSize(params: SizeParams): void { this.setConstraints({ maxWidth: sizeLimit(params.width, params.physical), maxHeight: sizeLimit(params.height, params.physical) }) }
  setConstraints(axes: SizeConstraints): void {
    const changes: SizeConstraints = {}
    for (const key of ['minWidth', 'minHeight', 'maxWidth', 'maxHeight'] as const) {
      if (Object.hasOwn(axes, key)) changes[key] = normalizeConstraint(axes[key])
    }
    this.constraints = { ...this.constraints, ...changes }
    this.appliedConstraints = undefined
    this.applyConstraints()
    const { width, height } = this.native.getContentBounds(); this.setSize({ width, height })
  }
  applyConstraints(): void {
    if (this.destroyed) return
    const c = this.contentConstraints, bounds = this.native.getBounds(), content = this.native.getContentBounds()
    const frameWidth = Math.max(0, bounds.width - content.width), frameHeight = Math.max(0, bounds.height - content.height)
    const outer = (value: number | undefined, frame: number) => value && value > 0 ? value + frame : 0
    const min = [outer(c.minWidth, frameWidth), outer(c.minHeight, frameHeight)] as const
    const max = [outer(c.maxWidth, frameWidth), outer(c.maxHeight, frameHeight)] as const
    const key = JSON.stringify([min, max]); if (key === this.appliedConstraints) return
    this.appliedConstraints = key
    this.native.setMinimumSize(...min); this.native.setMaximumSize(...max)
  }
  center(): void {
    if (this.env.platform === 'linux') { this.native.center?.(); return }
    const bounds = this.native.getBounds(), area = this.env.screen.getDisplayMatching(bounds).workArea
    this.native.setPosition(Math.round(area.x + (area.width - bounds.width) / 2), Math.round(area.y + (area.height - bounds.height) / 2))
  }
  currentDisplay(): DisplayInfo { return displayForBounds(this.env, this.native.getBounds()) }

  setTitle(title: string): void { this.native.setTitle(title) }
  setResizable(value: boolean): void { this.native.setResizable(value); this.applyMaximizable() }
  setMaximizable(value: boolean): void {
    this.maximizable = value
    if (this.env.platform !== 'linux') this.native.setMaximizable(value)
    this.applyMaximizable()
  }
  setMinimizable(value: boolean): void { if (this.env.platform !== 'linux') this.native.setMinimizable(value) }
  setClosable(value: boolean): void { this.closable = value; if (this.env.platform !== 'linux') this.native.setClosable(value) }
  maximize(): void { this.native.maximize() }
  unmaximize(): void { this.native.unmaximize() }
  minimize(): void { this.native.minimize() }
  unminimize(): Promise<null> {
    if (this.env.platform !== 'darwin' || !this.native.isMinimized()) { this.native.restore(); return Promise.resolve(null) }
    return new Promise((resolve, reject) => {
      let settled = false
      const finish = (error?: Error) => { if (settled) return; settled = true; clearTimeout(timer); this.native.removeListener('restore', restored); this.native.removeListener('closed', closed); error ? reject(error) : resolve(null) }
      const restored = () => finish(), closed = () => finish(new Error('window closed during restore'))
      const timer = setTimeout(() => finish(new Error('window restore timed out')), 10_000)
      this.native.once('restore', restored); this.native.once('closed', closed)
      try { this.native.restore() } catch (error) { finish(error as Error) }
    })
  }
  show(): void { this.native.show() }
  hide(): void { this.native.hide() }
  focus(): void { if (this.env.platform === 'darwin') this.env.app.focus({ steal: true }); this.native.focus() }
  close(): void { this.native.close() }
  setAlwaysOnTop(value: boolean): void { this.native.setAlwaysOnTop(value) }
  setSkipTaskbar(value: boolean): void { this.native.setSkipTaskbar(value) }
  setFocusable(value: boolean): void { this.native.setFocusable(value) }
  setContentProtection(value: boolean): void { this.native.setContentProtection(value) }
  setVisibleOnAllWorkspaces(value: boolean): void { this.native.setVisibleOnAllWorkspaces(value, { visibleOnFullScreen: true }) }
  setEnabled(value: boolean): void { this.native.setEnabled(value) }
  isEnabled(): boolean { return this.native.isEnabled() }
  requestAttention(flash: boolean): void { this.native.flashFrame(flash) }
  setSimpleFullscreen(value: boolean): void { this.native.setSimpleFullScreen?.(value) }
  setProgressBar(state: string, progress: number): void {
    this.native.setProgressBar(state === 'none' ? -1 : Number(progress) || 0, { mode: state === 'paused' ? 'paused' : state === 'error' ? 'error' : 'normal' })
  }
  setShadow(value: boolean): void { this.native.setHasShadow(value) }
  ignoreCursorEvents(ignore: boolean): void { this.native.setIgnoreMouseEvents(ignore, { forward: true }) }
  nativeHandle(): { handle: number; processId: number } {
    const buffer = this.native.getNativeWindowHandle()
    return { handle: buffer.length >= 8 ? Number(buffer.readBigUInt64LE(0)) : buffer.readUInt32LE(0), processId: process.pid }
  }
  setParent(parent: Window | null): void { if (parent === this) throw new Error('window cannot be its own parent'); this.native.setParentWindow(parent?.native ?? null) }
  setIcon(payload: IconPayload | undefined): void { const image = imageFromPayload(payload); if (image) this.native.setIcon(image) }
  setOverlayIcon(payload: IconPayload | undefined, description: string): void { this.native.setOverlayIcon(imageFromPayload(payload), description) }

  setDecorations(value: boolean): void { this.decorations = value; if (this.env.platform === 'darwin') this.configureStyle(); this.appliedConstraints = undefined; this.applyConstraints(); setImmediate(() => this.applyConstraints()) }
  setTitleBarStyle(style: string): void { this.config = { ...this.config, titleBarStyle: style }; if (this.env.platform === 'darwin') this.configureStyle() }
  setTrafficLight(position: { x?: number; y?: number; physical?: boolean }): void {
    if (this.env.platform !== 'darwin') return
    const x = this.toDip(position, 'x'), y = this.toDip(position, 'y')
    if (x !== undefined && y !== undefined) {
      this.env.appKit().setTrafficLightPosition(this.native.getNativeWindowHandle(), x, y)
      this.config = { ...this.config, trafficLightPosition: { x, y } }
    }
  }
  setBackgroundColor(color: string | number[] | undefined): void {
    const transparent = this.config.transparent === true
    const value = cssColor(color) ?? (transparent ? '#00000000' : '#ffffff')
    this.native.setBackgroundColor(value)
    this.background = value
    if (this.materialBackground !== undefined) this.materialBackground = value
    if (this.env.platform === 'linux') this.native.contentView.setBackgroundColor(transparent ? value : this.native.getBackgroundColor())
  }
  setBackgroundMaterial(material: string): void {
    if (!['none', 'acrylic', 'mica', 'tabbed'].includes(material)) throw new Error('invalid background material')
    const release = os.release()
    if (this.env.platform !== 'win32' || Number(release.split('.')[2]) < 22621) throw new Error('Electron background materials require Windows 11 22H2 or later')
    if (material !== 'none' && this.materialBackground === undefined) this.materialBackground = this.background
    this.native.setBackgroundMaterial?.(material as 'none' | 'acrylic' | 'mica' | 'tabbed')
    if (material === 'none') { this.native.setBackgroundColor(this.materialBackground ?? this.background); this.materialBackground = undefined }
  }

  setCursor(changes: Partial<CursorState>): Promise<null> {
    if (changes.cursor !== undefined && !/^[a-z][a-z-]*$/.test(changes.cursor)) throw new Error('invalid cursor icon')
    if (changes.visible !== undefined && typeof changes.visible !== 'boolean') throw new Error('invalid cursor visibility')
    this.cursor = { ...this.cursor, ...changes }
    return Promise.all(this.env.contents(this).map(contents => this.env.applyCursor(contents, this.cursor))).then(() => null)
  }
  paintCursor(): Promise<null> { return this.setCursor({}) }
  setTheme(value: string | null | undefined): Promise<null> {
    const theme = normalizeTheme(value)
    const pending = this.themeQueue.then(async () => {
      if (this.destroyed) return null
      const previous = this.theme; this.theme = theme
      try { await Promise.all(this.env.contents(this).map(contents => this.env.applyTheme(contents, theme))); await this.notifyTheme() }
      catch (error) {
        this.theme = previous
        await Promise.all(this.env.contents(this).map(contents => this.env.applyTheme(contents, previous).catch(() => {})))
        throw error
      }
      return null
    })
    this.themeQueue = pending.catch(() => {})
    return pending
  }
  async readTheme(): Promise<ThemeChange> {
    const contents = this.env.contents(this).find(item => !item.isDestroyed() && item.getURL())
    const fallback = this.theme === null ? this.env.nativeTheme.shouldUseDarkColors : this.theme === 'dark'
    const dark = contents ? await readContentsTheme(contents, fallback) : fallback
    return { theme: dark ? 'dark' : 'light', dark: dark && !this.env.nativeTheme.shouldUseHighContrastColors }
  }
  syncContents(contents: WebContents): void {
    void this.env.applyCursor(contents, this.cursor).catch(() => {})
    void this.env.applyTheme(contents, this.theme).catch(() => {})
  }
  installThemeSync(): void {
    const notify = () => { void this.notifyTheme().catch(() => {}) }
    this.env.nativeTheme.on('updated', notify)
    this.unsubscribeTheme = () => this.env.nativeTheme.off('updated', notify)
    void this.setTheme(this.config.theme).catch(() => {})
  }
  setFullscreen(params: FullscreenParams): Promise<null> { return this.fullscreen(params) }

  setMenu(menu: WindowMenuItem[] | null | { items?: WindowMenuItem[] }): void {
    const items = menu === null ? null : Array.isArray(menu) ? menu : menu.items ?? []
    const template = (entries: WindowMenuItem[]): import('electron').MenuItemConstructorOptions[] => entries.map(item => {
      if (item.type === 'separator') return { type: 'separator' }
      const entry: import('electron').MenuItemConstructorOptions = { label: item.label, enabled: item.enabled !== false, ...(item.accelerator ? { accelerator: item.accelerator } : {}) }
      if (item.submenu) entry.submenu = template(item.submenu)
      else { if (item.checked) Object.assign(entry, { type: 'checkbox', checked: true }); entry.click = () => { if (!this.destroyed) this.emit('menuCommand', item.commandId) } }
      return entry
    })
    this.native.setMenu(items === null ? null : Menu.buildFromTemplate(template(items)))
    if (items !== null) { this.native.setAutoHideMenuBar(false); this.native.setMenuBarVisibility(true) }
    this.appliedConstraints = undefined; this.applyConstraints(); setImmediate(() => this.applyConstraints())
  }

  private applyMaximizable(): void { if (this.env.platform === 'darwin') this.env.appKit().setMaximizable(this.native.getNativeWindowHandle(), this.maximizable) }
  private configureStyle(): void {
    const style = String(this.config.titleBarStyle ?? 'visible').toLowerCase()
    if (!['visible', 'transparent', 'overlay'].includes(style)) throw new Error('invalid title bar style')
    this.env.appKit().configureStyle(this.native.getNativeWindowHandle(), this.decorations, style, this.config.hiddenTitle === true || this.config.titleBarHidden === true)
    this.applyMaximizable()
    if (this.config.trafficLightPosition) this.setTrafficLight(this.config.trafficLightPosition)
  }
  private async notifyTheme(): Promise<void> { const theme = await this.readTheme(); if (!this.destroyed) this.emit('themeChanged', theme) }
  private async transitionFullscreen(fullscreen: boolean): Promise<void> {
    if (this.env.platform !== 'darwin') { this.native.setFullScreen(fullscreen); return }
    if (this.native.isFullScreen() === fullscreen) return
    await new Promise<void>((resolve, reject) => {
      let settled = false
      const event = fullscreen ? 'enter-full-screen' : 'leave-full-screen'
      const emitter = this.native as unknown as NodeJS.EventEmitter
      const finish = (error?: Error) => {
        if (settled) return
        settled = true; clearTimeout(timer); emitter.removeListener(event, completed); emitter.removeListener('closed', closed)
        error ? reject(error) : resolve()
      }
      const completed = () => finish(), closed = () => finish(new Error('window closed during fullscreen transition'))
      const timer = setTimeout(() => finish(new Error('native fullscreen transition timed out')), 10_000)
      emitter.once(event, completed); emitter.once('closed', closed)
      try { this.native.setFullScreen(fullscreen) } catch (error) { finish(error as Error) }
    })
  }
  private async fullscreen(params: FullscreenParams): Promise<null> {
    if (typeof params.fullscreen !== 'boolean') throw new Error('invalid fullscreen state')
    if (params.x !== undefined || params.y !== undefined) {
      const display = findDisplay(this.env, params.x ?? 0, params.y ?? 0, true)
      if (!display) return null
      const normal = this.fullscreenBounds ?? { bounds: this.native.getNormalBounds(), maximized: this.native.isMaximized() }
      if (this.native.isFullScreen()) await this.transitionFullscreen(false)
      this.fullscreenBounds = normal
      if (this.native.isMaximized()) this.native.unmaximize()
      this.native.setBounds({ x: display.bounds.x, y: display.bounds.y, width: Math.min(normal.bounds.width, display.bounds.width), height: Math.min(normal.bounds.height, display.bounds.height) })
    } else if (params.fullscreen && !this.fullscreenBounds) this.fullscreenBounds = { bounds: this.native.getNormalBounds(), maximized: this.native.isMaximized() }
    try { await this.transitionFullscreen(params.fullscreen) }
    catch (error) { if (!this.destroyed && !this.native.isFullScreen()) this.restoreFullscreen(); throw error }
    if (!params.fullscreen) this.restoreFullscreen()
    return null
  }
  private restoreFullscreen(): void {
    const normal = this.fullscreenBounds
    if (!normal || this.destroyed) return
    this.fullscreenBounds = undefined; this.native.setBounds(normal.bounds)
    if (normal.maximized) this.native.maximize()
  }
}

export class Windows {
  private readonly byId = new Map<number, Window>()
  add(window: Window): void {
    if (this.byId.has(window.id)) throw new Error(`duplicate window ${window.id}`)
    this.byId.set(window.id, window)
    window.native.once('closed', () => { if (this.byId.get(window.id) === window) this.byId.delete(window.id) })
  }
  get(id: number): Window { const window = this.byId.get(id); if (!window || window.destroyed) throw new Error(`unknown window ${id}`); return window }
  peek(id: number): Window | undefined { const window = this.byId.get(id); return window && !window.destroyed ? window : undefined }
  all(): Window[] { return [...this.byId.values()].filter(window => !window.destroyed) }
  clear(): void { this.byId.clear() }
}

export function sizeLimit(value: number | null | undefined, physical: boolean | undefined): SizeConstraint | null {
  return value == null ? null : normalizeConstraint({ value, physical })
}
export function normalizeConstraint(input: ConstraintInput): SizeConstraint | null {
  if (input == null) return null
  const value = typeof input === 'number' ? input : input.value
  if (typeof value !== 'number' || !Number.isFinite(value) || value < 0) throw new Error('invalid window size constraint')
  return { value, physical: typeof input === 'object' && input.physical === true }
}
export function toSizeConstraints(input: Record<string, ConstraintInput>): SizeConstraints {
  return Object.fromEntries(Object.entries(input).map(([key, value]) => [key, normalizeConstraint(value)]))
}
export function normalizeTheme(theme: string | null | undefined): Theme {
  if (theme === 'dark' || theme === 'light') return theme
  if (theme == null || theme === 'system') return null
  throw new Error(`invalid window theme ${theme}`)
}
export function cssColor(color: unknown): string | undefined {
  if (typeof color === 'string') return color
  if (!Array.isArray(color) || color.length < 3 || color.length > 4 || !color.every(value => typeof value === 'number' && Number.isFinite(value) && value >= 0 && value <= 255)) return undefined
  const [r, g, b, a = 255] = color as number[]
  return `rgba(${r}, ${g}, ${b}, ${(a / 255).toFixed(3)})`
}
export function imageFromPayload(payload: IconPayload | undefined): NativeImage | null {
  if (!payload) return null
  const { width, height, rgba } = payload
  if (!Number.isSafeInteger(width) || !Number.isSafeInteger(height) || !width || !height || !Array.isArray(rgba) || rgba.length !== width! * height! * 4 || !rgba.every(value => Number.isInteger(value) && value >= 0 && value <= 255)) return null
  const bitmap = Buffer.from(rgba)
  for (let index = 0; index < bitmap.length; index += 4) [bitmap[index], bitmap[index + 2]] = [bitmap[index + 2], bitmap[index]]
  return nativeImage.createFromBitmap(bitmap, { width: width!, height: height! })
}
