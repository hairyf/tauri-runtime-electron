import { net } from 'electron'
import type { Protocol, Session } from 'electron'
import type { Transport } from '../socket/transport'
import type { Policies } from './policy'

/**
 * 自定义协议领域（骨架）。
 *
 * Tauri 的 `tauri://localhost`、插件协议、`<scheme>.localhost` 都走这里。
 * 两个容易踩的点被单独固定下来：
 * 1. scheme 必须在 app ready **之前**注册为 privileged，否则 fetch/CORS 全部失效；
 * 2. Electron 的 protocol 回调是 session 级、无发送者身份，所以 `ipc://` 必须由
 *    preload 的窄桥带上 webviewId，不能靠猜。
 */

/** Rust 侧处理一次请求所需的全部信息，body 用 base64（可能非 UTF-8）。 */
export interface ProtocolRequest {
  id: number
  scheme: string
  webviewId?: number
  uri: string
  method: string
  headers: Record<string, string>
  body: string
}

export interface ProtocolResponse {
  status?: number
  headers?: Record<string, string>
  /** base64；HEAD / 204 / 304 时忽略。 */
  body?: string
}

export interface ProtocolHost {
  readonly transport: Transport
  /** 自定义 session 既要挂协议，也要挂策略，所以这里直接持有策略。 */
  readonly policies: Policies
}

export class Protocols {
  private readonly registered = new Set<string>()
  private readonly prepared = new WeakSet<Session>()
  private readonly localhost = new WeakSet<Protocol>()
  private readonly pending = new Map<number, (response: ProtocolResponse) => void>()
  private nextRequest = 1

  constructor(private readonly host: ProtocolHost) {}

  /** 必须在 app.whenReady() 之前调用；schemes 来自 --tauri-schemes。 */
  static registerPrivileged(schemes: string[]): void {
    // TODO: Implement registerPrivileged
  }

  /** protocol.register：幂等，且要把已存在的自定义 session 一起补齐。 */
  register(scheme: string): void {
    // TODO: Implement register
  }

  /** protocol.respond */
  answer(id: number, response: ProtocolResponse): void {
    // TODO: Implement answer
  }

  /** 给一个新建的自定义 session 挂上策略、所有已注册协议与 localhost 转发。 */
  prepare(session: Session): void {
    // TODO: Implement prepare（含 this.host.policies.install(session)）
  }

  dispose(): void {
    // TODO: Implement dispose
  }

  /**
   * 发起一次到 Rust 的请求并等待响应。
   * webviewId 为 undefined 时拒绝 `ipc://`：没带身份的调用无法判定归属。
   */
  private dispatch(scheme: string, request: Request, uri?: string, webviewId?: number): Promise<Response> {
    // TODO: Implement dispatch
    throw new Error('not implemented')
  }

  /**
   * `<scheme>.localhost` → `<scheme>://localhost`。
   * 命中规则必须严格：有端口、非 .localhost、scheme 未注册时一律交回默认网络栈。
   */
  private installLocalhost(target: Protocol, network: typeof net): void {
    // TODO: Implement installLocalhost
  }
}

/** HTTP 语义：HEAD 与 204/205/304 不返回 body。 */
export function isEmptyBody(method: string, status: number): boolean {
  // TODO: Implement isEmptyBody
  throw new Error('not implemented')
}
