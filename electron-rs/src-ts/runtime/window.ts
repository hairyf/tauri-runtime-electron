import { EventEmitter } from 'node:events'
import type {
  App, BaseWindow, NativeImage, NativeTheme, Rectangle, WebContents,
} from 'electron'
import type { DisplayInfo, DisplaySource } from './display'
import type { AppKit } from './native/appkit'

/**
 * 窗口领域对象（骨架）。
 *
 * 这里是 internal.cjs 里 9 个 per-window WeakMap 的替代品：配置、可关闭性、可最大化性、
 * 尺寸约束、光标、全屏前的 bounds、主题，全部是 Window 的字段。
 *
 * 两条硬约束：
 * 1. 不 import ipc / host —— 对外通知通过 EventEmitter 抛出，宿主负责映射成线协议事件名。
 * 2. 不做跨域编排 —— 创建窗口（涉及 popup 策略与 webview 装配）不在这里，
 *    页面上的光标/主题下发通过 env 转交给 webview 领域，Window 只负责“要什么”。
 */

export type Theme = 'dark' | 'light' | null

export interface SizeConstraint {
  value: number
  physical: boolean
}

export type SizeConstraints = Partial<
  Record<'minWidth' | 'minHeight' | 'maxWidth' | 'maxHeight', SizeConstraint | null>
>

/** Rust 侧传来的约束值：裸数字或 { value, physical }。 */
export type ConstraintInput = number | { value?: number; physical?: boolean } | null | undefined

export interface CursorState {
  visible: boolean
  cursor?: string
}

export interface WindowGeometry {
  scaleFactor: number
  bounds: Rectangle
  contentBounds: Rectangle
  contentConstraints: Record<string, number>
  maximized: boolean
  minimized: boolean
  focused: boolean
  visible: boolean
  fullscreen: boolean
  resizable: boolean
  alwaysOnTop: boolean
  decorated: boolean
  title: string
  maximizable: boolean
  minimizable: boolean
  closable: boolean
}

export interface ThemeChange {
  theme: 'dark' | 'light'
  dark: boolean
}

/** Tauri 侧窗口配置。字段宽松，因为 Rust 只传它关心的那部分。 */
export interface WindowConfig extends Record<string, unknown> {
  title?: string
  width?: number
  height?: number
  x?: number
  y?: number
  transparent?: boolean
  decorations?: boolean
  visible?: boolean
  focus?: boolean
  focusable?: boolean
  center?: boolean
  maximized?: boolean
  fullscreen?: boolean
  resizable?: boolean
  maximizable?: boolean
  minimizable?: boolean
  closable?: boolean
  alwaysOnTop?: boolean
  skipTaskbar?: boolean
  shadow?: boolean
  backgroundColor?: string | number[]
  titleBarStyle?: string
  titleBarHidden?: boolean
  hiddenTitle?: boolean
  trafficLightPosition?: { x: number; y: number }
  visibleOnAllWorkspaces?: boolean
  contentProtected?: boolean
  preventOverflow?: boolean | { width?: number; height?: number }
}

export interface WindowMenuItem {
  type?: 'separator'
  label?: string
  enabled?: boolean
  accelerator?: string
  checked?: boolean
  commandId?: number
  submenu?: WindowMenuItem[]
}

export interface SizeParams {
  width?: number
  height?: number
  physical?: boolean
}

export interface PositionParams {
  x?: number
  y?: number
  physical?: boolean
}

export interface FullscreenParams {
  fullscreen: boolean
  x?: number
  y?: number
}

export interface IconPayload {
  width?: number
  height?: number
  rgba?: number[]
}

/**
 * Window 需要外部世界提供的全部能力。每个成员都是一个真实的接缝，
 * 取代了原来 `state` 这个 37 字段的全局袋子。
 *
 * 继承 DisplaySource：显示器的坐标换算全部在 display.ts，Window 只借用。
 */
export interface WindowEnvironment extends DisplaySource {
  readonly nativeTheme: NativeTheme
  readonly app: App
  appKit(): AppKit
  /** 当前归属该窗口的页面（含子 webview）。所有权是活的，必须每次现查。 */
  contents(window: Window): WebContents[]
  applyCursor(contents: WebContents, cursor: Readonly<CursorState>): Promise<void>
  applyTheme(contents: WebContents, theme: Theme): Promise<void>
}

/**
 * 本窗口对外抛出的领域事件。宿主负责映射成线协议：
 * closeRequested → window.closeRequested、menuCommand → menu.command、
 * themeChanged → window.themeChanged。
 */
export interface WindowEvents {
  closeRequested: []
  menuCommand: [commandId: number | undefined]
  themeChanged: [payload: ThemeChange]
}

/** BaseWindow 上签名不稳定或平台相关的几个方法。 */
type NativeWindow = BaseWindow & {
  setSimpleFullScreen?(fullscreen: boolean): void
  setBackgroundMaterial?(material: 'none' | 'acrylic' | 'mica' | 'tabbed'): void
}

export class Window extends EventEmitter<WindowEvents> {
  // TODO: 由创建流程按 config 初始化 closable / maximizable / decorations / background
  private closable = true
  private maximizable = true
  private decorations = true
  private background = '#ffffff'
  private materialBackground?: string
  private cursor: CursorState = { visible: true }
  private fullscreenBounds?: { bounds: Rectangle; maximized: boolean }
  private theme: Theme = null
  private themeQueue: Promise<unknown> = Promise.resolve()
  /** 上次真正下发给原生的约束，避免 setMinimumSize 触发 resize 再回到这里递归。 */
  private appliedConstraints?: string
  /** 只有 Rust 明确要 destroy 时才放行原生 close。 */
  private allowClose = false
  private unsubscribeTheme?: () => void

  constructor(
    private readonly env: WindowEnvironment,
    readonly native: NativeWindow,
    readonly id: number,
    private config: WindowConfig,
    private constraints: SizeConstraints,
  ) {
    super()
  }

  get configuration(): Readonly<WindowConfig> {
    // TODO: Implement configuration
    throw new Error('not implemented')
  }

  get destroyed(): boolean {
    // TODO: Implement destroyed
    throw new Error('not implemented')
  }

  get themeOverride(): Theme {
    // TODO: Implement themeOverride
    throw new Error('not implemented')
  }

  // ─── 生命周期 ────────────────────────────────────────────────────────────

  /**
   * 原生 'close' 事件里调用。返回 true 表示调用方必须 preventDefault。
   * 窗口生命周期由 Rust 决定：关闭请求先上报，真正销毁走 destroy()。
   */
  requestClose(): boolean {
    // TODO: Implement requestClose
    throw new Error('not implemented')
  }

  /** 只有 host 决策后才会调用；放行一次原生关闭并销毁。 */
  destroy(): void {
    // TODO: Implement destroy
  }

  /** 创建完成后首次显示。visible:false 的窗口保持隐藏，等 window.ready。 */
  reveal(): void {
    // TODO: Implement reveal
  }

  dispose(): void {
    // TODO: Implement dispose
  }

  // ─── 几何 ────────────────────────────────────────────────────────────────

  get scale(): number {
    // TODO: Implement scale
    throw new Error('not implemented')
  }

  get contentConstraints(): Record<string, number> {
    // TODO: Implement contentConstraints
    throw new Error('not implemented')
  }

  /** Tauri 用物理像素，Electron 用 DIP。 */
  physical(rect: Rectangle, scale?: number): Rectangle {
    // TODO: Implement physical
    throw new Error('not implemented')
  }

  toDip(params: { physical?: boolean }, key: 'x' | 'y' | 'width' | 'height'): number | undefined {
    // TODO: Implement toDip
    throw new Error('not implemented')
  }

  geometry(): WindowGeometry {
    // TODO: Implement geometry
    throw new Error('not implemented')
  }

  setSize(params: SizeParams): void {
    // TODO: Implement setSize
  }

  setPosition(params: PositionParams): void {
    // TODO: Implement setPosition
  }

  setMinSize(params: SizeParams): void {
    // TODO: Implement setMinSize
  }

  setMaxSize(params: SizeParams): void {
    // TODO: Implement setMaxSize
  }

  setConstraints(axes: SizeConstraints): void {
    // TODO: Implement setConstraints
  }

  applyConstraints(): void {
    // TODO: Implement applyConstraints
  }

  center(): void {
    // TODO: Implement center
  }

  currentDisplay(): DisplayInfo {
    // TODO: Implement currentDisplay
    throw new Error('not implemented')
  }

  // ─── 窗口状态 ────────────────────────────────────────────────────────────

  setTitle(title: string): void {
    // TODO: Implement setTitle
  }

  setResizable(resizable: boolean): void {
    // TODO: Implement setResizable
  }

  setMaximizable(maximizable: boolean): void {
    // TODO: Implement setMaximizable
  }

  setMinimizable(minimizable: boolean): void {
    // TODO: Implement setMinimizable
  }

  setClosable(closable: boolean): void {
    // TODO: Implement setClosable
  }

  maximize(): void {
    // TODO: Implement maximize
  }

  unmaximize(): void {
    // TODO: Implement unmaximize
  }

  minimize(): void {
    // TODO: Implement minimize
  }

  unminimize(): Promise<null> {
    // TODO: Implement unminimize
    throw new Error('not implemented')
  }

  show(): void {
    // TODO: Implement show
  }

  hide(): void {
    // TODO: Implement hide
  }

  focus(): void {
    // TODO: Implement focus
  }

  close(): void {
    // TODO: Implement close
  }

  setAlwaysOnTop(value: boolean): void {
    // TODO: Implement setAlwaysOnTop
  }

  setSkipTaskbar(skip: boolean): void {
    // TODO: Implement setSkipTaskbar
  }

  setFocusable(focusable: boolean): void {
    // TODO: Implement setFocusable
  }

  setContentProtection(value: boolean): void {
    // TODO: Implement setContentProtection
  }

  setVisibleOnAllWorkspaces(value: boolean): void {
    // TODO: Implement setVisibleOnAllWorkspaces
  }

  setEnabled(enabled: boolean): void {
    // TODO: Implement setEnabled
  }

  isEnabled(): boolean {
    // TODO: Implement isEnabled
    throw new Error('not implemented')
  }

  requestAttention(flash: boolean): void {
    // TODO: Implement requestAttention
  }

  setSimpleFullscreen(fullscreen: boolean): void {
    // TODO: Implement setSimpleFullscreen
  }

  setProgressBar(state: string, progress: number): void {
    // TODO: Implement setProgressBar
  }

  setShadow(value: boolean): void {
    // TODO: Implement setShadow
  }

  ignoreCursorEvents(ignore: boolean): void {
    // TODO: Implement ignoreCursorEvents
  }

  nativeHandle(): { handle: number; processId: number } {
    // TODO: Implement nativeHandle
    throw new Error('not implemented')
  }

  setParent(parent: Window | null): void {
    // TODO: Implement setParent
  }

  setIcon(payload: IconPayload | undefined): void {
    // TODO: Implement setIcon
  }

  setOverlayIcon(payload: IconPayload | undefined, description: string): void {
    // TODO: Implement setOverlayIcon
  }

  // ─── 外观 ────────────────────────────────────────────────────────────────

  setDecorations(decorations: boolean): void {
    // TODO: Implement setDecorations
  }

  setTitleBarStyle(style: string): void {
    // TODO: Implement setTitleBarStyle
  }

  setTrafficLight(position: { x?: number; y?: number; physical?: boolean }): void {
    // TODO: Implement setTrafficLight
  }

  setBackgroundColor(color: string | number[] | undefined): void {
    // TODO: Implement setBackgroundColor
  }

  setBackgroundMaterial(material: string): void {
    // TODO: Implement setBackgroundMaterial
  }

  // ─── 光标 ────────────────────────────────────────────────────────────────

  setCursor(changes: Partial<CursorState>): Promise<null> {
    // TODO: Implement setCursor
    throw new Error('not implemented')
  }

  /** 页面 dom-ready 后调用：新文档没有继承上一份 CSS。 */
  paintCursor(): Promise<null> {
    // TODO: Implement paintCursor
    throw new Error('not implemented')
  }

  // ─── 主题 ────────────────────────────────────────────────────────────────

  /** 串行化：主题下发是异步的，但必须按请求顺序落地。 */
  setTheme(theme: string | null | undefined): Promise<null> {
    // TODO: Implement setTheme
    throw new Error('not implemented')
  }

  readTheme(): Promise<ThemeChange> {
    // TODO: Implement readTheme
    throw new Error('not implemented')
  }

  /** 新页面加入时补齐当前覆盖，避免它先按系统主题渲染一帧。 */
  syncTheme(contents: WebContents): void {
    // TODO: Implement syncTheme
  }

  /** 系统主题变化时重算本窗口的汇报值。 */
  installThemeSync(): void {
    // TODO: Implement installThemeSync
  }

  // ─── 全屏 ────────────────────────────────────────────────────────────────

  setFullscreen(params: FullscreenParams): Promise<null> {
    // TODO: Implement setFullscreen
    throw new Error('not implemented')
  }

  // ─── 菜单 ────────────────────────────────────────────────────────────────

  setMenu(menu: WindowMenuItem[] | null | { items?: WindowMenuItem[] }): void {
    // TODO: Implement setMenu
  }
}

/**
 * Rust 侧的 id → Window 索引，取代 state.windows / state.closeAllowed。
 * 两种语义分开：get() 找不到就抛错，peek() 用于允许窗口已消失的命令。
 */
export class Windows {
  private readonly byId = new Map<number, Window>()

  add(window: Window): void {
    // TODO: Implement add
  }

  get(id: number): Window {
    // TODO: Implement get
    throw new Error('not implemented')
  }

  peek(id: number): Window | undefined {
    // TODO: Implement peek
    throw new Error('not implemented')
  }

  all(): Window[] {
    // TODO: Implement all
    throw new Error('not implemented')
  }

  clear(): void {
    // TODO: Implement clear
  }
}

// ─── 纯函数 ────────────────────────────────────────────────────────────────

/** 单个约束轴：undefined 表示该轴没被约束。 */
export function sizeLimit(value: number | undefined, physical: boolean | undefined): SizeConstraint | null {
  // TODO: Implement sizeLimit
  throw new Error('not implemented')
}

export function normalizeConstraint(input: ConstraintInput): SizeConstraint | null {
  // TODO: Implement normalizeConstraint
  throw new Error('not implemented')
}

export function toSizeConstraints(input: Record<string, ConstraintInput>): SizeConstraints {
  // TODO: Implement toSizeConstraints
  throw new Error('not implemented')
}

export function normalizeTheme(theme: string | null | undefined): Theme {
  // TODO: Implement normalizeTheme
  throw new Error('not implemented')
}

/** 接受 CSS 字符串或 [r, g, b, a?]。 */
export function cssColor(color: unknown): string | undefined {
  // TODO: Implement cssColor
  throw new Error('not implemented')
}

/** Tauri 交给的是原生 RGBA；Electron 的 bitmap 是 BGRA，交换红蓝通道。 */
export function imageFromPayload(payload: IconPayload | undefined): NativeImage | null {
  // TODO: Implement imageFromPayload
  throw new Error('not implemented')
}
