import { net, protocol } from 'electron'
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
  private disposed = false
  private readonly sessions = new Set<Session>()

  constructor(private readonly host: ProtocolHost) {}

  /** 必须在 app.whenReady() 之前调用；schemes 来自 --tauri-schemes。 */
  static registerPrivileged(schemes: string[]): void {
    protocol.registerSchemesAsPrivileged(schemes.filter((scheme, i, all) => /^[a-z][a-z0-9+.-]*$/.test(scheme) && all.indexOf(scheme) === i).map((scheme) => ({ scheme, privileges: { standard: true, secure: true, supportFetchAPI: true, corsEnabled: true, stream: true } })))
  }

  /** protocol.register：幂等，且要把已存在的自定义 session 一起补齐。 */
  register(scheme: string): void {
    if (this.disposed || this.registered.has(scheme) || !/^[a-z][a-z0-9+.-]*$/.test(scheme)) return
    this.registered.add(scheme)
    for (const session of this.sessions) session.protocol.handle(scheme, (request) => this.dispatch(scheme, request))
    protocol.handle(scheme, (request) => this.dispatch(scheme, request))
    this.installLocalhost(protocol, net)
  }

  /** Authenticated IPC fetch: unlike session protocol handlers, this has a trusted webview identity. */
  fetchIpc(request: Request, webviewId: number): Promise<Response> {
    return this.dispatch('ipc', request, `ipc://localhost${new URL(request.url).pathname}${new URL(request.url).search}`, webviewId)
  }

  /** protocol.respond */
  answer(id: number, response: ProtocolResponse): void {
    const resolve = this.pending.get(id)
    if (!resolve) return
    this.pending.delete(id)
    resolve(response)
  }

  /** 给一个新建的自定义 session 挂上策略、所有已注册协议与 localhost 转发。 */
  prepare(session: Session): void {
    if (this.prepared.has(session)) return
    this.prepared.add(session)
    this.sessions.add(session)
    this.host.policies.install(session)
    for (const scheme of this.registered) session.protocol.handle(scheme, (request) => this.dispatch(scheme, request))
    this.installLocalhost(session.protocol, session)
  }

  dispose(): void {
    this.disposed = true
    for (const resolve of this.pending.values()) resolve({ status: 499 })
    this.pending.clear()
    this.registered.clear()
  }

  /**
   * 发起一次到 Rust 的请求并等待响应。
   * webviewId 为 undefined 时拒绝 `ipc://`：没带身份的调用无法判定归属。
   */
  private async dispatch(scheme: string, request: Request, uri = request.url, webviewId?: number): Promise<Response> {
    if (this.disposed || (scheme === 'ipc' && webviewId === undefined)) return Response.error()
    const id = this.nextRequest++
    const body = Buffer.from(await request.arrayBuffer()).toString('base64')
    const headers: Record<string, string> = {}
    request.headers.forEach((value, key) => { headers[key] = value })
    const initiator = (request as Request & { initiatorOrigin?: string }).initiatorOrigin
    if (typeof initiator === 'string') headers.origin = initiator
    const reply = new Promise<ProtocolResponse>((resolve) => this.pending.set(id, resolve))
    this.host.transport.send('protocol.request', { id, scheme, webviewId, uri, method: request.method, headers, body })
    const result = await reply
    const status = result.status ?? 200
    const responseHeaders = new Headers(result.headers ?? {})
    if (status === 204 || status === 205) { responseHeaders.delete('content-length'); responseHeaders.delete('transfer-encoding') }
    return new Response(isEmptyBody(request.method, status) ? null : Buffer.from(result.body ?? '', 'base64'), { status, headers: responseHeaders })
  }

  /**
   * `<scheme>.localhost` → `<scheme>://localhost`。
   * 命中规则必须严格：有端口、非 .localhost、scheme 未注册时一律交回默认网络栈。
   */
  private installLocalhost(target: Protocol, network: typeof net | Session): void {
    if (this.localhost.has(target)) return
    this.localhost.add(target)
    const fetch = (request: Request) => network.fetch(request, { bypassCustomProtocolHandlers: true })
    for (const transport of ['http', 'https'] as const) target.handle(transport, (request) => {
      const url = new URL(request.url)
      const match = /^([a-z][a-z0-9+.-]*)\.localhost$/.exec(url.hostname)
      if (url.port || !match || !this.registered.has(match[1])) return fetch(request)
      return this.dispatch(match[1], request, `${match[1]}://localhost${url.pathname}${url.search}`)
    })
  }
}

/** HTTP 语义：HEAD 与 204/205/304 不返回 body。 */
export function isEmptyBody(method: string, status: number): boolean {
  return method.toUpperCase() === 'HEAD' || [204, 205, 304].includes(status)
}
