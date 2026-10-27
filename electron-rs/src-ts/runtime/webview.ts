import { EventEmitter } from 'node:events'
import type { Cookie, Rectangle, WebContents, WebContentsView } from 'electron'
import type { Window } from './window'

/**
 * 页面领域（骨架）。
 *
 * 取代 internal.cjs 里四张平行表：webviews、webviewByContents、childWebviews、mainViews。
 * 一个 Webview 同时持有「归属哪个 Window / 哪个 WebContents / 是不是主视图」，
 * 于是 `findWebview` + `contentsFor` + `state.childWebviews.get(...)` 三连消失了。
 */

/** Rust 侧为该 webview 注册的 out-of-band 策略（原来是 state.handlerPolicy）。 */
export interface WebviewPolicy {
  navigation?: boolean
  download?: boolean
  permission?: boolean
}

/** 物理像素坐标，`physical` 为 true 时按 Rust 的物理像素解释。 */
export interface WebviewBoundsInput {
  position?: { x: number; y: number }
  size?: { width: number; height: number }
  physical?: boolean
}

export interface WebviewInit {
  webviewId: number
  url: string
  autoResize?: boolean
  bounds?: WebviewBoundsInput
  policy?: WebviewPolicy
  [key: string]: unknown
}

/** 位置始终相对父窗口，不是屏幕坐标。 */
export interface WebviewSnapshot {
  scaleFactor: number
  bounds: Rectangle
  contentBounds: Rectangle
  visible: boolean
}

export interface CookieFilter {
  url?: string
}

export interface CookieInit {
  url: string
  name: string
  value: string
  domain?: string
  path?: string
  secure?: boolean
  httpOnly?: boolean
  sameSite?: 'unspecified' | 'no_restriction' | 'lax' | 'strict'
  expirationDate?: number
}

/**
 * 领域事件。宿主映射成线协议：
 * navigationCommitted → webview.navigationCommitted、closed → webview.closed …
 */
export interface WebviewEvents {
  navigationCommitted: [url: string]
  loadStarted: [url: string]
  loadFinished: [url: string]
  titleChanged: [title: string]
  processGone: [details: { reason: string; exitCode: number }]
  closed: []
}

export class Webview extends EventEmitter<WebviewEvents> {
  /** 主视图随父窗口缩放；子 webview 由 Tauri 自己布局。 */
  private main = false
  private autoResize: boolean
  private policy?: WebviewPolicy
  /** 记录上一次的父子比例基准，父窗口 resize 时按比例重算。 */
  private reference?: { bounds: Rectangle; parent: Rectangle }

  constructor(
    private readonly owner: Window,
    readonly id: number,
    readonly view: WebContentsView,
    private config: WebviewInit,
  ) {
    super()
    this.autoResize = Boolean(config.autoResize)
    this.policy = config.policy
  }

  get window(): Window {
    return this.owner
  }

  get contents(): WebContents {
    // TODO: Implement contents
    throw new Error('not implemented')
  }

  get isMain(): boolean {
    return this.main
  }

  get webviewPolicy(): WebviewPolicy | undefined {
    return this.policy
  }

  // ─── 生命周期 ────────────────────────────────────────────────────────────

  /**
   * 挂到父窗口并装载。navigate 为 false 时只建视图不导航
   * （本地协议需要当前这条 Rust 调用先返回）。
   */
  attach(options: { main?: boolean; navigate?: boolean }): Promise<void> {
    // TODO: Implement attach
    throw new Error('not implemented')
  }

  /** 从父窗口摘掉但不销毁，用于 reparent 的中间态。 */
  detach(): void {
    // TODO: Implement detach
  }

  /** 显式关闭：先把归属摘干净，再销毁 WebContents。 */
  close(): void {
    // TODO: Implement close
  }

  // ─── 导航 ────────────────────────────────────────────────────────────────

  navigate(url: string): void {
    // TODO: Implement navigate
  }

  reload(): void {
    // TODO: Implement reload
  }

  goBack(): void {
    // TODO: Implement goBack
  }

  canGoBack(): boolean {
    // TODO: Implement canGoBack
    throw new Error('not implemented')
  }

  goForward(): void {
    // TODO: Implement goForward
  }

  canGoForward(): boolean {
    // TODO: Implement canGoForward
    throw new Error('not implemented')
  }

  evaluate(script: string): Promise<unknown> {
    // TODO: Implement evaluate
    throw new Error('not implemented')
  }

  // ─── 布局 ────────────────────────────────────────────────────────────────

  snapshot(): WebviewSnapshot {
    // TODO: Implement snapshot
    throw new Error('not implemented')
  }

  setBounds(input: WebviewBoundsInput): void {
    // TODO: Implement setBounds
  }

  setAutoResize(autoResize: boolean): void {
    // TODO: Implement setAutoResize
  }

  /** 父窗口内容区变化时按比例重算（Tauri 的 position 与 size 都相对父窗口缩放）。 */
  resizeWithParent(parent: Rectangle): void {
    // TODO: Implement resizeWithParent
  }

  setVisible(visible: boolean): void {
    // TODO: Implement setVisible
  }

  setBackgroundColor(color: string | undefined): void {
    // TODO: Implement setBackgroundColor
  }

  focus(): void {
    // TODO: Implement focus
  }

  print(): Promise<void> {
    // TODO: Implement print
    throw new Error('not implemented')
  }

  // ─── 调试与缩放 ──────────────────────────────────────────────────────────

  openDevtools(): void {
    // TODO: Implement openDevtools
  }

  closeDevtools(): void {
    // TODO: Implement closeDevtools
  }

  isDevtoolsOpen(): boolean {
    // TODO: Implement isDevtoolsOpen
    throw new Error('not implemented')
  }

  setZoom(scaleFactor: number): void {
    // TODO: Implement setZoom
  }

  // ─── 存储 ────────────────────────────────────────────────────────────────

  cookies(filter: CookieFilter): Promise<Cookie[]> {
    // TODO: Implement cookies
    throw new Error('not implemented')
  }

  setCookie(cookie: CookieInit): Promise<null> {
    // TODO: Implement setCookie
    throw new Error('not implemented')
  }

  deleteCookie(filter: { url: string; name: string }): Promise<null> {
    // TODO: Implement deleteCookie
    throw new Error('not implemented')
  }

  clearBrowsingData(): Promise<null> {
    // TODO: Implement clearBrowsingData
    throw new Error('not implemented')
  }

  // ─── 归属与策略 ──────────────────────────────────────────────────────────

  /** 换父窗口并保持视觉位置（跨 DPI 时按比例换算）。 */
  reparent(destination: Window): void {
    // TODO: Implement reparent
  }

  setPolicy(policy: WebviewPolicy | undefined): void {
    // TODO: Implement setPolicy
  }
}

/**
 * webviewId → Webview，同时按 contentsId 反查。
 * 反查是给策略回调用的：Electron 只给 WebContents，不给 Tauri id。
 */
export class Webviews {
  private readonly byId = new Map<number, Webview>()
  private readonly byContents = new Map<number, Webview>()

  add(webview: Webview): void {
    // TODO: Implement add
  }

  get(id: number): Webview {
    // TODO: Implement get
    throw new Error('not implemented')
  }

  peek(id: number): Webview | undefined {
    // TODO: Implement peek
    throw new Error('not implemented')
  }

  forContents(contentsId: number): Webview | undefined {
    // TODO: Implement forContents
    throw new Error('not implemented')
  }

  of(window: Window): Webview[] {
    // TODO: Implement of
    throw new Error('not implemented')
  }

  main(window: Window): Webview | undefined {
    // TODO: Implement main
    throw new Error('not implemented')
  }

  all(): Webview[] {
    // TODO: Implement all
    throw new Error('not implemented')
  }

  delete(id: number): void {
    // TODO: Implement delete
  }

  clear(): void {
    // TODO: Implement clear
  }
}
