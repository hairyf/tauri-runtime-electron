import type { BrowserWindowConstructorOptions, Session, WebContents } from 'electron'
import type { DownloadDecision, Downloads } from './downloads'
import type { Webview, Webviews } from './webview'
import type { Window, Windows } from './window'

/**
 * 策略桥（骨架）—— 唯一允许把决策权交给 Rust 的地方。
 *
 * 四种策略的 Electron API 形状完全不同，这也是它们过去被摊在 main.cjs 里的原因：
 *
 * | 策略       | 触发点                        | 能否阻塞 |
 * | navigation | webRequest.onBeforeRequest    | 否（异步回调） |
 * | download   | session 'will-download'       | 否，但 setSavePath 必须在同步阶段完成 |
 * | permission | setPermissionRequestHandler   | 否（callback 可延迟） |
 * | popup      | setWindowOpenHandler          | **是**（必须当场返回 action） |
 *
 * 只有 popup 走 `transport.request()`（PolicyBridge，刻意不等同于 transport.ts 的
 * 传输层）。它同时也解释了 ipc 为什么要 Worker 线程：
 * 阻塞期间 Rust 会嵌套下发 window.create，那条命令必须还能被执行。
 *
 * 因为没有第二条阻塞入口，internal.cjs 里的 synchronousMethods 白名单可以消失：
 * 能阻塞的只有下面 attach() 里那一次调用，是类型层面固定的。
 */

export type PermissionKind =
  | 'geolocation'
  | 'notifications'
  | 'clipboard-read'
  | 'display-capture'
  | 'midi'
  | 'sensors'
  | 'camera'
  | 'microphone'

/** 与 IPC 的接触面。刻意窄：只有 popup 允许 request()。 */
export interface PolicyBridge {
  send(event: string, params: Record<string, unknown>): void
  /** 同步阻塞：仅 webview.newWindow 使用。 */
  request<T>(event: string, params: Record<string, unknown>, timeout?: number): T
}

/** window.open 的入参，字段与 Electron 的 handler details 对齐。 */
export interface PopupDetails {
  request: number
  webviewId: number
  url: string
  frameName: string
  features: string
  disposition: string
  position: { x: number; y: number } | null
  size: { width: number; height: number } | null
}

export interface NewWindowResponse {
  action: 'allow' | 'deny'
  windowId?: number
}

/**
 * Rust 在阻塞期间嵌套建好的原生窗口（走 window.create + newWindowRequest）。
 * attach() 把 Chromium 刚创建的 guest WebContents 塞进这个既有窗口。
 */
export interface PopupPlan {
  window: Window
  options: BrowserWindowConstructorOptions
  attach(contents: WebContents): void
}

export interface PolicyHost {
  readonly transport: PolicyBridge
  readonly downloads: Downloads
  /** Electron 只给 WebContents；策略用 webviews.forContents 反查 Tauri 的 webviewId 与策略。 */
  readonly webviews: Webviews
  readonly windows: Windows
  /** 阻塞期间建好的 popup，按 request id 存；由 factory 写入、策略取出。 */
  readonly popups: Map<number, PopupPlan>
}

interface NavigationEntry {
  finish(allow: boolean, retired?: boolean): void
  timer: NodeJS.Timeout
  onDestroyed(): void
}

export class Policies {
  /** 同一个 session 可能被多个窗口共享，重复安装会叠加 handler。 */
  private readonly sessions = new WeakSet<Session>()
  private readonly navigations = new Map<number, NavigationEntry>()
  private readonly permissions = new Map<number, (allow: boolean) => void>()
  private nextNavigationRequest = 0
  private nextPermissionRequest = 0
  private disposed = false

  constructor(private readonly host: PolicyHost) {}

  /** 安装 session 级的三条策略（导航 / 下载 / 权限）。幂等。 */
  install(session: Session): void {
    // TODO: Implement install
  }

  /** 安装页面级的 popup 策略。每个 WebContents 都要装一次。 */
  attach(webview: Webview): void {
    // TODO: Implement attach
  }

  /** webview.navigationResponse */
  answerNavigation(id: number, allow: boolean): void {
    // TODO: Implement answerNavigation
  }

  /** webview.downloadResponse */
  answerDownload(id: number, decision: DownloadDecision): void {
    // TODO: Implement answerDownload
  }

  /** webview.permissionResponse；'default' 表示交回 Electron（其默认是拒绝）。 */
  answerPermission(id: number, response: 'allow' | 'deny' | 'default'): void {
    // TODO: Implement answerPermission
  }

  dispose(): void {
    // TODO: Implement dispose
  }
}

/** Electron 的权限名 → Tauri 能命名的种类。media 还要看 mediaTypes。 */
export function permissionKind(
  permission: string,
  details?: { mediaTypes?: string[] },
): PermissionKind | undefined {
  // TODO: Implement permissionKind
  throw new Error('not implemented')
}
