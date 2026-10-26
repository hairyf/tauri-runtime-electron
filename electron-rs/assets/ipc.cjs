'use strict'

// The worker owns the socket so a native synchronous policy callback can still
// receive Rust's answer, including nested window-creation RPCs on the UI thread.
const { Worker, MessageChannel, receiveMessageOnPort, isMainThread, workerData } = require('node:worker_threads')

if (!isMainThread) {
  const { port, channel, signal } = workerData
  const state = new Int32Array(signal)
  process.once('exit', () => {
    Atomics.store(state, 1, 1)
    Atomics.add(state, 0, 1)
    Atomics.notify(state, 0)
  })
  const socket = require('node:net').connect(port, '127.0.0.1')
  const notify = (message) => {
    channel.postMessage(message)
    Atomics.add(state, 0, 1)
    Atomics.notify(state, 0)
  }
  socket.setEncoding('utf8')
  socket.on('connect', () => notify({ connected: true }))
  let buffer = ''
  socket.on('data', (chunk) => {
    buffer += chunk
    let index
    while ((index = buffer.indexOf('\n')) !== -1) {
      const line = buffer.slice(0, index)
      buffer = buffer.slice(index + 1)
      try { notify({ request: JSON.parse(line) }) }
      catch { socket.destroy(new Error('invalid host IPC message')) }
    }
  })
  socket.on('error', () => socket.destroy())
  socket.once('close', () => {
    Atomics.store(state, 1, 1)
    notify({ closed: true })
    channel.close()
  })
  channel.on('message', (message) => {
    if (message.close) socket.destroy()
    else if (!socket.destroyed) socket.write(`${JSON.stringify(message.send)}\n`)
  })
} else {
  module.exports = function connect(port, onRequest, onConnected, onClosed) {
    const { port1, port2 } = new MessageChannel()
    const signal = new SharedArrayBuffer(8)
    const state = new Int32Array(signal)
    const replies = new Map()
    const inbox = []
    const deferred = []
    let scheduled = false
    let nextRequest = 0
    let closed = false
    const worker = new Worker(__filename, { workerData: { port, channel: port2, signal }, transferList: [port2] })
    const close = () => {
      if (closed) return
      closed = true
      Atomics.store(state, 1, 1)
      Atomics.add(state, 0, 1)
      Atomics.notify(state, 0)
      port1.close()
      worker.terminate().catch(() => {})
      onClosed()
    }
    const dispatch = (message, synchronous = false) => {
      if (closed) return
      if (message.closed) close()
      else if (message.connected) onConnected()
      else if (message.request) {
        // Nonblocking work runs only after the outer native policy callback
        // releases Chromium, including async RPCs that still expect a reply.
        if (synchronous && (message.request.notification === true || message.request.asynchronous === true)) {
          deferred.push(message)
          schedule()
        } else onRequest(message.request, synchronous)
      }
    }
    const nextMessage = () => inbox.shift() ?? receiveMessageOnPort(port1)?.message
    const schedule = () => {
      if (scheduled) return
      scheduled = true
      setImmediate(() => {
        scheduled = false
        let incoming
        while ((incoming = deferred.shift() ?? nextMessage())) dispatch(incoming)
      })
    }
    // Keep already-delivered messages visible to a reentrant synchronous request;
    // per-message setImmediate callbacks would strand them while Atomics.wait runs.
    port1.on('message', (message) => {
      inbox.push(message)
      schedule()
    })
    worker.on('error', close)
    worker.on('exit', close)
    return {
      get closed() { return closed || Atomics.load(state, 1) !== 0 },
      send(message) {
        if (!closed && !Atomics.load(state, 1)) port1.postMessage({ send: message })
      },
      respond(id, response) {
        if (!replies.has(id)) throw new Error('expired native policy request')
        replies.set(id, response)
      },
      request(event, params, timeout = 15_000) {
        if (closed || Atomics.load(state, 1)) throw new Error('host IPC closed')
        if (!Number.isFinite(timeout) || timeout < 0) throw new Error('invalid native policy timeout')
        const id = ++nextRequest
        const deadline = performance.now() + timeout
        replies.set(id, undefined)
        try {
          port1.postMessage({ send: { event, params: { ...params, id } } })
          for (;;) {
            const version = Atomics.load(state, 0)
            const response = replies.get(id)
            if (response !== undefined) return response
            if (closed || Atomics.load(state, 1)) throw new Error('host IPC closed during native policy request')
            const remaining = deadline - performance.now()
            if (remaining <= 0) throw new Error('native policy request timed out')
            const incoming = nextMessage()
            if (incoming) dispatch(incoming, true)
            else Atomics.wait(state, 0, version, remaining)
          }
        } finally {
          replies.delete(id)
        }
      },
      close,
    }
  }
}
