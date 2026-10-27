import { EventEmitter } from 'node:events'
import { MessageChannel, Worker, isMainThread, receiveMessageOnPort, workerData } from 'node:worker_threads'
import type { MessagePort } from 'node:worker_threads'
import net from 'node:net'
import { noop } from '../utils'

/**
 * 与 Rust 的线协议通道：单文件、双角色。
 *
 * 本文件被当成两个端点执行：
 * - 主线程：IPC 类（MessageChannel + Atomics + 同步捞取）
 * - Worker：由 `new Worker(__filename)` 重新执行本文件，独占 socket
 *
 * 之所以不拆：Worker 靠 `__filename` 自举，拆开就失去"同一个文件既可当入口
 * 又可被 import"这个前提。构建时必须让它保持独立产物（assets/ipc.cjs），
 * 不能内联进 main 的 bundle，否则 `__filename` 会指向 main.cjs。
 *
 * 存在的理由只有一个：主线程执行同步策略回调时会被 Atomics.wait 冻结，
 * 事件循环停转，socket 的 'data' 永远不会触发；读 socket 必须发生在另一个线程。
 */

/** 主线程 → Worker 的指令。 */
interface IPCChannelMessage {
  close?: boolean
  send?: unknown
}

/** Worker 启动参数：channel 与 signal 由主线程创建并转移过来。 */
interface IPCWorkerData {
  port: number
  channel: MessagePort
  signal: SharedArrayBuffer
}

/** Rust → Electron 的请求：`method` + 顶层 `id`（与 Electron → Rust 的 `event` 相反）。 */
export interface IPCRequestPayload {
  id: number
  method: string
  params: Record<string, unknown>
  /** 通知类：不需要回包。 */
  notification?: boolean
  /** 异步类：可以在阻塞回调之外完成。 */
  asynchronous?: boolean
}

/** Worker → 主线程的消息。 */
export interface IPCIncomingMessage {
  closed?: boolean
  connected?: boolean
  request?: IPCRequestPayload
}

export interface IPCEvents {
  connect: []
  close: []
  request: [req: IPCRequestPayload, synchronous: boolean]
}

export class IPC extends EventEmitter<IPCEvents> {
  private readonly worker: Worker
  private readonly channel: MessagePort
  private readonly state: Int32Array
  private readonly replies = new Map<number, unknown>()
  private readonly inbox: IPCIncomingMessage[] = []
  private readonly deferred: IPCIncomingMessage[] = []
  private scheduled = false
  private nextRequest = 0
  private _closed = false

  constructor(port: number) {
    super()
    const { port1, port2 } = new MessageChannel()
    this.channel = port1
    const signal = new SharedArrayBuffer(8)
    this.state = new Int32Array(signal)

    this.worker = new Worker(__filename, {
      workerData: { port, channel: port2, signal } satisfies IPCWorkerData,
      transferList: [port2],
    })

    // 未冻结时由事件回调日常派发；冻结期间由 request() 里的
    // receiveMessageOnPort 同步捞取，两条路径共用 dispatch。
    port1.on('message', (message: IPCIncomingMessage) => {
      this.inbox.push(message)
      this.schedule()
    })
    this.worker.on('error', () => this.close())
    this.worker.on('exit', () => this.close())
  }

  get closed(): boolean {
    return this._closed || Atomics.load(this.state, 1) !== 0
  }

  /** 单向消息。 */
  send(message: unknown): void {
    if (!this.closed) this.channel.postMessage({ send: message })
  }

  /** 响应阻塞中的策略请求（id 由 Rust 原样回显，不是 RPC id）。 */
  respond(id: number, response: unknown): void {
    if (!this.replies.has(id)) throw new Error('expired native policy request')
    this.replies.set(id, response)
  }

  /**
   * 同步阻塞请求：为原生策略回调争取 Rust 的决策。
   *
   * 阻塞期间事件循环停转，只能靠 receiveMessageOnPort 同步取消息；
   * 但只处理"安全"的请求（见 dispatch），嵌套的窗口创建才能完成。
   */
  request<T = unknown>(event: string, params: Record<string, unknown>, timeout = 15_000): T {
    if (this.closed) throw new Error('host IPC closed')
    if (!Number.isFinite(timeout) || timeout < 0) throw new Error('invalid native policy timeout')

    const id = ++this.nextRequest
    const deadline = performance.now() + timeout
    this.replies.set(id, undefined)

    try {
      this.channel.postMessage({ send: { event, params: { ...params, id } } })
      for (;;) {
        // 先读版本号再检查消息：期间若有通知，要么消息已进 inbox，
        // 要么 Atomics.wait 因版本变化立即返回，不会错过唤醒。
        const version = Atomics.load(this.state, 0)
        const response = this.replies.get(id)
        if (response !== undefined) return response as T
        if (this.closed) throw new Error('host IPC closed during native policy request')

        const remaining = deadline - performance.now()
        if (remaining <= 0) throw new Error('native policy request timed out')

        const incoming = this.nextMessage()
        if (incoming) this.dispatch(incoming, true)
        else Atomics.wait(this.state, 0, version, remaining)
      }
    } finally {
      this.replies.delete(id)
    }
  }

  close = (): void => {
    if (this._closed) return
    this._closed = true
    Atomics.store(this.state, 1, 1)
    Atomics.add(this.state, 0, 1)
    Atomics.notify(this.state, 0)
    this.channel.close()
    this.worker.terminate().catch(noop)
    this.emit('close')
  }

  private dispatch(message: IPCIncomingMessage, synchronous = false): void {
    if (this._closed) return
    if (message.closed) {
      this.close()
    } else if (message.connected) {
      this.emit('connect')
    } else if (message.request) {
      // 通知类/异步类不能在原生回调里重入 Chromium，推到 setImmediate 之后；
      // 其余请求（可以立即返回值的）允许在同步栈上执行。
      if (synchronous && (message.request.notification === true || message.request.asynchronous === true)) {
        this.deferred.push(message)
        this.schedule()
      } else {
        this.emit('request', message.request, synchronous)
      }
    }
  }

  /** 同步、非阻塞地取一条消息；冻结期间这是唯一入口。 */
  private nextMessage(): IPCIncomingMessage | undefined {
    return this.inbox.shift() ?? (receiveMessageOnPort(this.channel)?.message as IPCIncomingMessage | undefined)
  }

  private schedule(): void {
    if (this.scheduled) return
    this.scheduled = true
    setImmediate(() => {
      this.scheduled = false
      let incoming: IPCIncomingMessage | undefined
      while ((incoming = this.deferred.shift() ?? this.nextMessage())) this.dispatch(incoming)
    })
  }
}

/** Worker 线程侧：独占 socket，按行分帧。 */
function thread(): void {
  const { port, channel, signal } = workerData as IPCWorkerData
  const state = new Int32Array(signal)
  let buffer = ''

  process.once('exit', () => {
    Atomics.store(state, 1, 1)
    Atomics.add(state, 0, 1)
    Atomics.notify(state, 0)
  })

  const socket = net.connect(port, '127.0.0.1')
  socket.setEncoding('utf8')
  socket.on('connect', () => notify({ connected: true }))
  socket.on('data', (chunk: string) => receive(chunk))
  socket.on('error', () => socket.destroy())
  socket.once('close', close)
  channel.on('message', (message: IPCChannelMessage) => {
    if (message.close) socket.destroy()
    else if (!socket.destroyed) socket.write(`${JSON.stringify(message.send)}\n`)
  })

  function close(): void {
    Atomics.store(state, 1, 1)
    notify({ closed: true })
    channel.close()
  }

  /** 先投递消息再递增信号，避免主线程被唤醒时还看不到消息。 */
  function notify(message: IPCIncomingMessage): void {
    channel.postMessage(message)
    Atomics.add(state, 0, 1)
    Atomics.notify(state, 0)
  }

  /** 一行一条 JSON；解析失败说明协议被破坏，直接断开。 */
  function receive(chunk: string): void {
    buffer += chunk
    let index: number
    while ((index = buffer.indexOf('\n')) !== -1) {
      const line = buffer.slice(0, index)
      buffer = buffer.slice(index + 1)
      try {
        notify({ request: JSON.parse(line) as IPCRequestPayload })
      } catch {
        socket.destroy(new Error('invalid host IPC message'))
      }
    }
  }
}

if (!isMainThread) thread()
