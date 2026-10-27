import type { IPC, IPCRequestPayload } from './ipc'
import { isThenable } from '../utils'
import type { CommandTable } from './table'

const synchronousMethods = new Set([
  'window.create', 'protocol.register', 'protocol.respond', 'webview.url',
  'webview.newWindowResponse', 'webview.navigationResponse', 'webview.downloadResponse', 'webview.permissionResponse',
  'window.nativeHandle', 'window.setMenu', 'window.setTitle', 'window.geometry', 'window.icon',
  'window.size', 'window.position', 'window.minSize', 'window.maxSize', 'window.sizeConstraints',
  'window.resizable', 'window.maximizable', 'window.minimizable', 'window.closable',
  'window.maximize', 'window.unmaximize', 'window.minimize', 'window.simpleFullscreen',
  'window.alwaysOnTop', 'window.setParentWindow', 'window.skipTaskbar', 'window.focusable',
  'window.contentProtected', 'window.center', 'window.requestAttention', 'window.show', 'window.hide',
  'window.focus', 'window.close', 'window.destroy', 'window.visibleOnAllWorkspaces', 'window.enabled',
  'window.enabledQuery', 'window.progressBar', 'window.shadow', 'window.decorations', 'window.badgeCount',
  'window.overlayIcon', 'window.backgroundColor', 'window.backgroundMaterial', 'window.ignoreCursorEvents',
  'window.displays', 'window.cursorPosition', 'window.currentDisplay', 'window.displayForPoint', 'window.trafficLight',
])

/**
 * 线协议边界。
 *
 * 只做三件事，全部与业务无关：
 * - 请求 → 命令表 → 回包（`{id, result}` / `{id, error}`）
 * - 单向事件发给 Rust
 * - 为策略提供同步 request / respond 通道
 *
 * 之所以能从 runtime 抽出来：它只依赖 IPC 与命令表类型，不认识 electron。
 * 这正是 internal.cjs 里 synchronousMethods 白名单彻底消失的地方 ——
 * 能否阻塞不再靠名单，而靠"同步栈上出现了 Promise"这一事实。
 */
export class Transport {
  /** 由组合根在命令构建完成后注入，避免 transport ↔ commands 的初始化顺序耦合。 */
  private table?: CommandTable

  constructor(private readonly ipc: IPC) {
    // 同步/异步两种请求共用同一入口，由 ipc 在派发时标明。
    this.ipc.on('request', (request, synchronous) => this.receive(request, synchronous))
  }

  use(table: CommandTable): void {
    this.table = table
  }

  /** socket 已连上 Rust（握手前）。 */
  onConnect(handler: () => void): void {
    this.ipc.on('connect', handler)
  }

  /** 连接断开：Worker 退出或 Rust 主动关闭都会走到这里。 */
  onClose(handler: () => void): void {
    this.ipc.on('close', handler)
  }

  get closed(): boolean {
    return this.ipc.closed
  }

  send(event: string, params: object): void {
    this.ipc.send({ event, params })
  }

  /** 回复阻塞中的策略请求（webview.newWindowResponse 携带的 id 不是 RPC id）。 */
  respond(id: number, response: unknown): void {
    this.ipc.respond(id, response)
  }

  /** 同步阻塞，只允许 popup 使用。 */
  request<T>(event: string, params: Record<string, unknown>, timeout?: number): T {
    return this.ipc.request<T>(event, params, timeout)
  }

  private receive(request: IPCRequestPayload, synchronous: boolean): void {
    const { id, method, notification } = request
    const params = request.params ?? {}
    if (!Number.isSafeInteger(id)) return

    const reply = (payload: Record<string, unknown>): void => {
      if (notification !== true) this.ipc.send(payload)
    }
    const fail = (error: unknown): void => {
      const message = error instanceof Error ? error.message : String(error)
      console.error(method, message)
      reply({ id, error: message })
    }

    try {
      if (synchronous && !synchronousMethods.has(method)) throw new Error(`${method} requires the native policy callback to return first`)
      const command = this.table?.[method]
      if (!command) throw new Error(`unknown method ${method}`)
      const result = command(params)
      if (isThenable(result)) {
        // 原生策略回调期间 Chromium 停在同步栈上，renderer/CDP 的 Promise 不可能完成。
        // 命令已经执行（副作用已生效），这里只保证不把主线程挂死。
        if (synchronous) throw new Error(`${method} requires the native policy callback to return first`)
        result.then((value) => reply({ id, result: value ?? null }), fail)
        return
      }
      reply({ id, result: result ?? null })
    } catch (error) {
      fail(error)
    }
  }
}
