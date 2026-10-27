import { AsyncLocalStorage, createHook } from 'node:async_hooks'
import { Console } from 'node:console'
import { createRequire } from 'node:module'
import path from 'node:path'
import { Worker } from 'node:worker_threads'

export interface ExecutionTransport {
  send(event: string, params: object): void
  onClose(handler: () => void): void
}

export interface ExecutionParams {
  taskId: string
  mode: 'node' | 'electron'
  script: string
  args: string[]
  timeoutMs: number
  maxOutputBytes: number
  stream: boolean
}

export interface ExecutionResult {
  code: number
  stdout: string
  stderr: string
  result: unknown
}

type Stream = 'stdout' | 'stderr'
type Timer = { kind: 'Timeout' | 'Immediate', handle: NodeJS.Timeout | NodeJS.Immediate }
type Chunk = { sequence: number, stream: Stream, buffer: Buffer, start: number, end: number }
type Output = { buffers: Buffer[], length: number, pending?: Chunk }
const chunkBytes = 32 * 1024
const contexts = new AsyncLocalStorage<Execution>()
const promises = new WeakMap<Promise<unknown>, Execution>()
const timers = new Map<number, Execution>()
let mainUsers = 0
let restoreMain: (() => void) | undefined

function message(error: unknown): string {
  return error instanceof Error ? error.stack ?? error.message : String(error)
}

function exitCode(value: unknown): number {
  const code = value == null ? 0 : typeof value === 'string' && /^-?\d+$/.test(value) ? Number(value) : value
  if (typeof code !== 'number' || !Number.isInteger(code) || code < -2147483648 || code > 2147483647) throw new TypeError('invalid process exit code')
  return code
}

/** One shared hook, never a per-task global patch. Async dependencies inherit the route. */
function acquireMain(): () => void {
  if (mainUsers++ === 0) {
    const originals = { stdout: process.stdout.write, stderr: process.stderr.write, exit: process.exit }
    const writes = {} as Record<Stream, typeof process.stdout.write>
    for (const stream of ['stdout', 'stderr'] as const) {
      writes[stream] = function (this: NodeJS.WriteStream, chunk: string | Uint8Array, ...args: any[]): boolean {
        const task = contexts.getStore()
        if (!task) return originals[stream].call(this, chunk, ...args)
        if (typeof chunk !== 'string' && !(chunk instanceof Uint8Array)) throw new TypeError('stdout/stderr writes require a string or Uint8Array')
        const data = typeof chunk === 'string' ? Buffer.from(chunk, typeof args[0] === 'string' ? args[0] as BufferEncoding : undefined) : Buffer.from(chunk)
        if (!task.ending && !task.finished) task.output(stream, data)
        const callback = typeof args[0] === 'function' ? args[0] : args[1]
        if (typeof callback === 'function') process.nextTick(callback)
        return true
      } as typeof process.stdout.write
      process[stream].write = writes[stream]
    }
    const trappedExit = ((code?: number | string) => {
      const task = contexts.getStore()
      if (!task) return originals.exit(code)
      return task.exit(code)
    }) as typeof process.exit
    process.exit = trappedExit
    const descriptor = Object.getOwnPropertyDescriptor(process, 'exitCode')!
    let outsideCode = process.exitCode
    const getCode = () => contexts.getStore()?.code ?? (descriptor.get ? descriptor.get.call(process) : outsideCode)
    if (descriptor.configurable) Object.defineProperty(process, 'exitCode', {
      configurable: true,
      enumerable: descriptor.enumerable,
      get: getCode,
      set(value: unknown) {
        const task = contexts.getStore()
        if (task) task.code = exitCode(value)
        else if (descriptor.set) descriptor.set.call(process, value)
        else outsideCode = value as typeof process.exitCode
      },
    })
    const hook = createHook({
      init(id, kind, _trigger, resource) {
        const task = contexts.getStore()
        if (!task) return
        if (kind === 'PROMISE') promises.set(resource as Promise<unknown>, task)
        if (kind !== 'Timeout' && kind !== 'Immediate') return
        const timer = { kind, handle: resource } as Timer
        if (task.finished || task.ending) process.nextTick(() => clearTimer(timer))
        else { task.timers.set(id, timer); timers.set(id, task) }
      },
      destroy(id) {
        timers.get(id)?.timers.delete(id)
        timers.delete(id)
      },
    }).enable()
    const uncaught = (error: Error): void => {
      const task = contexts.getStore()
      if (task) task.scriptError(error)
      else if (process.listenerCount('uncaughtException') === 1) throw error
    }
    const rejected = (error: unknown, promise: Promise<unknown>): void => {
      const task = promises.get(promise) ?? contexts.getStore()
      if (task) task.scriptError(error)
      else if (process.listenerCount('unhandledRejection') === 1) throw error
    }
    process.on('uncaughtException', uncaught)
    process.on('unhandledRejection', rejected)
    restoreMain = () => {
      hook.disable()
      process.removeListener('uncaughtException', uncaught)
      process.removeListener('unhandledRejection', rejected)
      for (const stream of ['stdout', 'stderr'] as const) if (process[stream].write === writes[stream]) process[stream].write = originals[stream]
      if (process.exit === trappedExit) process.exit = originals.exit
      if (descriptor.configurable && Object.getOwnPropertyDescriptor(process, 'exitCode')?.get === getCode) {
        Object.defineProperty(process, 'exitCode', descriptor.get || descriptor.set ? descriptor : { ...descriptor, value: outsideCode })
      }
    }
  }
  let released = false
  return () => {
    if (released) return
    released = true
    if (--mainUsers === 0) { restoreMain?.(); restoreMain = undefined }
  }
}

function clearTimer(timer: Timer): void {
  if (timer.kind === 'Immediate') clearImmediate(timer.handle as NodeJS.Immediate)
  else clearTimeout(timer.handle as NodeJS.Timeout)
}

class Execution {
  readonly timers = new Map<number, Timer>()
  readonly controller = new AbortController()
  readonly done: Promise<ExecutionResult>
  code = 0
  ending = false
  finished = false
  private readonly stdout: Output = { buffers: [], length: 0 }
  private readonly stderr: Output = { buffers: [], length: 0 }
  private readonly pending: Chunk[] = []
  private readonly exitSignal = new Error('command process.exit')
  private resolve!: (value: ExecutionResult) => void
  private reject!: (error: Error) => void
  private deadline?: NodeJS.Timeout
  private scheduled?: NodeJS.Immediate
  private finalTurn?: NodeJS.Immediate
  private worker?: Worker
  private cleanupWorker?: () => void
  private stopping = false
  private failure?: Error
  private value: unknown = null
  private sequence = 0

  constructor(readonly params: ExecutionParams, private readonly transport: ExecutionTransport) {
    this.done = new Promise((resolve, reject) => { this.resolve = resolve; this.reject = reject })
  }

  start(): void {
    this.deadline = setTimeout(() => this.stop(124), this.params.timeoutMs)
    try {
      if (this.params.mode === 'node') this.startWorker()
      else this.startMain()
    } catch (error) { this.fail(error) }
  }

  output(stream: Stream, data: Buffer): void {
    if (this.finished || this.failure || data.length === 0) return
    if (this.stdout.length + this.stderr.length + data.length > this.params.maxOutputBytes) {
      this.fail(new Error(`command output exceeds maxOutputBytes (${this.params.maxOutputBytes})`))
      return
    }
    const output = this[stream]
    for (let offset = 0; offset < data.length;) {
      let buffer = output.buffers[output.buffers.length - 1]
      let start = output.length % chunkBytes
      if (!buffer || start === 0) {
        buffer = Buffer.allocUnsafe(chunkBytes)
        output.buffers.push(buffer)
        start = 0
      }
      const length = Math.min(data.length - offset, chunkBytes - start)
      data.copy(buffer, start, offset, offset + length)
      output.length += length
      offset += length
      if (!this.params.stream) continue
      if (output.pending?.buffer === buffer && output.pending.end === start) output.pending.end += length
      else {
        output.pending = { sequence: this.sequence++, stream, buffer, start, end: start + length }
        this.pending.push(output.pending)
      }
    }
    if (this.params.stream && !this.scheduled) {
      this.scheduled = contexts.exit(() => setImmediate(() => {
        this.scheduled = undefined
        try { this.flush() } catch (error) { this.fail(error) }
      }))
    }
  }

  private flush(): void {
    this.stdout.pending = this.stderr.pending = undefined
    while (this.pending.length) {
      const chunks = this.pending.splice(0, 32).map(({ sequence, stream, buffer, start, end }) => ({ sequence, stream, data: buffer.subarray(start, end).toString('base64') }))
      contexts.exit(() => this.transport.send('command.output', { taskId: this.params.taskId, chunks }))
    }
  }

  private startWorker(): void {
    const worker = this.worker = new Worker(path.join(__dirname, 'command-worker.cjs'), {
      stdout: true,
      stderr: true,
      workerData: { script: this.params.script, args: this.params.args },
    })
    let started = false, exited = false, ended = 0
    const drain = (): void => { if (exited && ended === 2) this.finish() }
    const onMessage = (event: { kind: string, json?: string, message?: string }): void => {
      if (this.finished || this.stopping) return
      if (event.kind === 'started') started = true
      else if (event.kind === 'failure') this.fail(new Error(event.message ?? 'command worker failed'))
      else if (event.kind === 'result') {
        try { this.value = JSON.parse(event.json!) } catch (error) { this.fail(new Error(`invalid command worker result: ${message(error)}`)) }
      }
    }
    const onError = (error: Error): void => {
      if (!started) this.fail(new Error(`command worker failed to start: ${message(error)}`))
      else if (!this.stopping) { this.code = 1; this.value = null; this.output('stderr', Buffer.from(`${message(error)}\n`)) }
    }
    const onExit = (code: number): void => { if (!this.stopping) this.code = code; exited = true; drain() }
    worker.on('message', onMessage)
    worker.on('error', onError)
    worker.on('exit', onExit)
    const cleanups: (() => void)[] = []
    for (const stream of ['stdout', 'stderr'] as const) {
      const pipe = worker[stream]!
      let closed = false
      const data = (chunk: Buffer): void => this.output(stream, chunk)
      const end = (): void => { if (!closed) { closed = true; ended++; drain() } }
      const error = (error: Error): void => { this.fail(new Error(`command ${stream} pipe failed: ${message(error)}`)); end() }
      pipe.on('data', data)
      pipe.on('end', end)
      pipe.on('close', end)
      pipe.on('error', error)
      cleanups.push(() => { pipe.removeListener('data', data); pipe.removeListener('end', end); pipe.removeListener('close', end); pipe.removeListener('error', error) })
    }
    this.cleanupWorker = () => {
      worker.removeListener('message', onMessage)
      worker.removeListener('error', onError)
      worker.removeListener('exit', onExit)
      for (const cleanup of cleanups) cleanup()
    }
  }

  private startMain(): void {
    const requireScript = createRequire(path.join(process.cwd(), '__electron_rs_command__.cjs'))
    const facade = Object.create(process, {
      exit: { value: (code?: number | string) => this.exit(code) },
      exitCode: { get: () => this.code, set: (code: unknown) => { this.code = exitCode(code) } },
    }) as NodeJS.Process
    const scriptRequire = Object.assign((id: string) => id === 'process' || id === 'node:process' ? facade : requireScript(id), requireScript)
    const AsyncFunction = Object.getPrototypeOf(async function () {}).constructor
    contexts.run(this, () => {
      void Promise.resolve().then(() => {
        if (this.finished) return
        return new AsyncFunction('args', 'signal', 'require', 'process', 'console', this.params.script)(
          this.params.args, this.controller.signal, scriptRequire, facade, new Console({ stdout: process.stdout, stderr: process.stderr }),
        )
      }).then((result) => {
        if (this.finished) return
        try {
          const json = JSON.stringify(result === undefined ? null : result)
          if (json === undefined) throw new Error('result has no JSON representation')
          this.value = JSON.parse(json)
        } catch (error) { this.fail(new Error(`command result is not JSON serializable: ${message(error)}`)); return }
        this.ending = true
        this.clearTimers()
        clearTimeout(this.deadline)
        // Observe this turn's unhandled rejections before replying; detached work is not a task lifetime.
        this.finalTurn = contexts.exit(() => setImmediate(() => this.finish()))
      }, (error) => this.scriptError(error)).catch((error) => this.fail(error))
    })
  }

  exit(value?: number | string): never {
    this.stop(value === undefined ? this.code : exitCode(value))
    throw this.exitSignal
  }

  scriptError(error: unknown): void {
    if (this.finished || error === this.exitSignal) return
    this.code = 1
    this.value = null
    try { this.output('stderr', Buffer.from(`${message(error)}\n`)) } catch (error) { this.fail(error); return }
    this.stop(1)
  }

  fail(error: unknown): void {
    if (this.finished) return
    this.failure ??= error instanceof Error ? error : new Error(message(error))
    this.stop(this.code)
  }

  stop(code: number): void {
    if (this.finished || this.stopping) return
    this.stopping = true
    this.code = code
    this.ending = true
    clearTimeout(this.deadline)
    this.clearTimers()
    contexts.run(this, () => this.controller.abort())
    this.code = code
    if (code === 130 || code === 124) this.value = null
    if (this.worker) {
      void this.worker.terminate().catch((error) => { this.failure ??= new Error(`command worker termination failed: ${message(error)}`); this.finish() })
    } else this.finish()
  }

  private clearTimers(): void {
    for (const [id, timer] of this.timers) { clearTimer(timer); timers.delete(id) }
    this.timers.clear()
  }

  private finish(): void {
    if (this.finished) return
    this.finished = this.ending = true
    clearTimeout(this.deadline)
    clearImmediate(this.scheduled)
    clearImmediate(this.finalTurn)
    this.clearTimers()
    this.cleanupWorker?.()
    try {
      this.flush()
      if (this.failure) throw this.failure
      const code = process.platform === 'win32' ? (this.code | 0) : (this.code & 255)
      this.resolve({ code, stdout: Buffer.concat(this.stdout.buffers, this.stdout.length).toString('base64'), stderr: Buffer.concat(this.stderr.buffers, this.stderr.length).toString('base64'), result: this.value })
    } catch (error) { this.reject(error instanceof Error ? error : new Error(message(error))) }
  }
}

/** Authenticated Rust transport only; this service is not a JavaScript security sandbox. */
export class Executions {
  private readonly jobs = new Map<string, Execution>()
  private closed = false
  private releaseMain?: () => void
  private closing?: Promise<void>

  constructor(private readonly transport: ExecutionTransport) {
    transport.onClose(() => { void this.close() })
  }

  async execute(input: unknown): Promise<ExecutionResult> {
    if (this.closed) throw new Error('command executions are closed')
    const p = input as Partial<ExecutionParams> | null
    if (!p || typeof p !== 'object' || typeof p.taskId !== 'string' || !p.taskId.trim() || (p.mode !== 'node' && p.mode !== 'electron') || typeof p.script !== 'string' || !Array.isArray(p.args) || !p.args.every((arg) => typeof arg === 'string') || !Number.isInteger(p.timeoutMs) || p.timeoutMs! <= 0 || p.timeoutMs! > 2147483647 || (p.stream !== undefined && typeof p.stream !== 'boolean')) throw new TypeError('invalid command.execute parameters')
    const maxOutputBytes = p.maxOutputBytes === undefined ? 16 * 1024 * 1024 : p.maxOutputBytes
    if (!Number.isInteger(maxOutputBytes) || maxOutputBytes < 1 || maxOutputBytes > 256 * 1024 * 1024) throw new TypeError('invalid command maxOutputBytes (expected 1..268435456)')
    if (this.jobs.has(p.taskId)) throw new Error(`command task already exists: ${p.taskId}`)
    if (p.mode === 'electron') this.releaseMain ??= acquireMain()
    const task = new Execution({ ...p, args: [...p.args], maxOutputBytes, stream: p.stream ?? false } as ExecutionParams, this.transport)
    this.jobs.set(p.taskId, task)
    contexts.exit(() => task.start())
    try { return await task.done } finally { this.jobs.delete(task.params.taskId) }
  }

  cancel(input: unknown): null {
    const p = input as { taskId?: unknown } | null
    if (!p || typeof p.taskId !== 'string' || !p.taskId.trim()) throw new TypeError('invalid command.cancel taskId')
    this.jobs.get(p.taskId)?.stop(130)
    return null
  }

  close(): Promise<void> {
    if (this.closing) return this.closing
    this.closed = true
    const jobs = [...this.jobs.values()]
    for (const task of jobs) task.stop(130)
    this.releaseMain?.()
    this.releaseMain = undefined
    this.closing = Promise.allSettled(jobs.map((task) => task.done)).then(() => {})
    return this.closing
  }
}
