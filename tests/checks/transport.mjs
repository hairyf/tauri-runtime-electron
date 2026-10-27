// Real TCP peer in its own worker: the caller blocks in Atomics.wait during policy RPCs.
import assert from 'node:assert/strict'
import { createRequire } from 'node:module'
import net from 'node:net'
import { Worker, isMainThread, parentPort } from 'node:worker_threads'

if (!isMainThread) {
  const server = net.createServer((socket) => {
    socket.setEncoding('utf8')
    const send = (id, method, params) => socket.write(JSON.stringify({ id, method, params }) + '\n')
    let buffer = '', queued, cached = false, rpc = 200
    const finishQueued = () => {
      if (queued && cached) send(++rpc, 'respond', { id: queued.id, response: { action: 'allow' } })
    }
    socket.on('data', (chunk) => {
      buffer += chunk
      let end
      while ((end = buffer.indexOf('\n')) !== -1) {
        const message = JSON.parse(buffer.slice(0, end))
        buffer = buffer.slice(end + 1)
        const { event, params = {} } = message
        if (event === 'ready') {
          parentPort.postMessage({ ready: params })
          // Split a UTF-8 character across reads, then coalesce multiple messages later.
          const data = Buffer.from(JSON.stringify({ id: 100, method: 'async', params: { value: '窗口' } }) + '\n')
          const split = data.indexOf(Buffer.from('窗口')) + 1
          socket.write(data.subarray(0, split))
          setTimeout(() => socket.write(data.subarray(split)), 5)
        } else if (message.id === 100) {
          parentPort.postMessage({ async: message })
        } else if (event === 'start-queued') {
          socket.write(JSON.stringify({ id: 101, method: 'trigger' }) + '\n' + JSON.stringify({ id: 102, method: 'cached' }) + '\n')
        } else if (event === 'queued') {
          // Must survive two nested policy waits, in order, without execution
          // on Chromium's blocked stack or preventing the synchronous reply.
          socket.write(JSON.stringify({ id: 110, method: 'deferred', notification: true, params: { value: 1 } }) + '\n'
            + JSON.stringify({ id: 111, method: 'deferred', asynchronous: true, params: { value: 2 } }) + '\n')
          queued = params
          finishQueued()
        } else if (message.id === 102) {
          assert.deepEqual(message.result, { nested: true })
          cached = true
          finishQueued()
        } else if (message.id === 101) {
          parentPort.postMessage({ queued: message })
        } else if (event === 'nested') {
          send(++rpc, 'respond', { id: params.id, response: { nested: true } })
        } else if (event === 'timeout') {
          setTimeout(() => send(++rpc, 'respond', { id: params.id, response: { stale: true } }), 60)
        } else if (event === 'after-timeout') {
          setTimeout(() => send(++rpc, 'respond', { id: params.id, response: { fresh: true } }), 100)
        } else if (event === 'busy') {
          for (let i = 0; i < 100; i++) send(++rpc, 'noise', {})
        } else if (event === 'disconnect') {
          socket.destroy()
        } else if (event === 'malformed') {
          socket.write('not-json\n')
        } else if (message.error) {
          parentPort.postMessage({ error: message.error })
        }
      }
    })
    socket.on('error', () => {})
  })
  server.listen(0, '127.0.0.1', () => parentPort.postMessage({ port: server.address().port }))
} else {
  const { IPC } = createRequire(import.meta.url)('../../electron-rs/assets/socket/ipc.cjs')
  const peer = new Worker(new URL(import.meta.url))
  const received = []
  peer.on('message', (message) => received.push(message))
  const until = async (predicate) => {
    const deadline = performance.now() + 3_000
    while (!predicate()) {
      assert.ok(performance.now() < deadline, 'TCP test peer did not finish')
      await new Promise((resolve) => setTimeout(resolve, 5))
    }
  }
  let transport, closes = 0, stale = 0, noise = 0
  const deferred = []
  const pause = new Int32Array(new SharedArrayBuffer(4))
  try {
    await until(() => received.some((message) => message.port))
    const { port } = received.find((message) => message.port)
    const onRequest = (message, synchronous) => {
      try {
        if (message.method === 'noise') {
          if (synchronous) Atomics.wait(pause, 0, 0, 10)
          noise++
          return
        }
        if (message.method === 'deferred') {
          assert.equal(synchronous, false, 'nonblocking work must leave every native callback before execution')
          deferred.push(message.params.value)
          return
        }
        if (message.method === 'async') {
          assert.equal(synchronous, false)
          setTimeout(() => transport.send({ id: message.id, result: message.params }), 5)
          return
        }
        let result
        if (message.method === 'trigger') {
          assert.equal(synchronous, false)
          result = transport.request('queued', {}, 1_000)
          assert.deepEqual(deferred, [], 'notifications must not run before the outer native callback returns')
        } else if (message.method === 'cached') {
          assert.equal(synchronous, true, 'queued RPC must be serviced inside the policy callback')
          result = transport.request('nested', {}, 1_000)
        } else if (message.method === 'respond') {
          assert.equal(synchronous, true)
          transport.respond(message.params.id, message.params.response)
        } else {
          assert.fail(`unexpected RPC ${message.method}`)
        }
        transport.send({ id: message.id, result: result ?? null })
      } catch (error) {
        if (message.params?.response?.stale) stale++
        transport.send({ id: message.id, error: error.message })
      }
    }
    transport = new IPC(port)
    transport.on('request', onRequest)
    transport.on('connect', () => transport.send({ event: 'ready', params: { token: 'handshake' } }))
    transport.on('close', () => { closes++ })
    await until(() => received.some((message) => message.async))
    assert.deepEqual(received.find((message) => message.ready).ready, { token: 'handshake' })
    assert.deepEqual(received.find((message) => message.async).async.result, { value: '窗口' })
    transport.send({ event: 'start-queued' })
    await until(() => received.some((message) => message.queued))
    assert.deepEqual(received.find((message) => message.queued).queued.result, { action: 'allow' })
    await until(() => deferred.length === 2)
    assert.deepEqual(deferred, [1, 2], 'deferred messages retain wire order')

    const before = performance.now()
    assert.throws(() => transport.request('timeout', {}, 20), /timed out/)
    assert.ok(performance.now() - before < 1_000, 'timeout must bound the synchronous wait')
    assert.deepEqual(transport.request('after-timeout', {}, 1_000), { fresh: true })
    assert.equal(stale, 1, 'late reply must not complete a different request')
    await until(() => received.some((message) => message.error === 'expired native policy request'))
    assert.throws(() => transport.request('invalid', {}, Infinity), /invalid native policy timeout/)
    const busy = performance.now()
    assert.throws(() => transport.request('busy', {}, 30), /timed out/)
    assert.ok(performance.now() - busy < 300, 'queued RPC traffic must not starve the policy deadline')
    await until(() => noise === 100)

    const disconnected = performance.now()
    assert.throws(() => transport.request('disconnect', {}, 3_000), /host IPC closed/)
    assert.ok(performance.now() - disconnected < 1_000, 'disconnect must wake Atomics.wait')
    await until(() => closes === 1)
    transport.close()
    assert.equal(closes, 1)
    assert.throws(() => transport.request('closed', {}, 100), /host IPC closed/)

    // JSON serialization crashes the socket worker, not the blocked caller.
    transport = new IPC(port)
    transport.on('request', onRequest)
    transport.on('close', () => { closes++ })
    transport.send({ value: 1n })
    const crashed = performance.now()
    assert.throws(() => transport.request('worker-crash', {}, 3_000), /host IPC closed/)
    assert.ok(performance.now() - crashed < 1_000, 'worker exception must wake Atomics.wait')
    await until(() => closes === 2)

    transport = new IPC(port)
    transport.on('request', onRequest)
    transport.on('close', () => { closes++ })
    assert.throws(() => transport.request('malformed', {}, 3_000), /host IPC closed/)
    await until(() => closes === 3)
    console.log('IPC transport: handshake, async, queued/reentrant nested RPC, timeout/stale reply, disconnect, worker exception, invalid JSON passed')
  } finally {
    transport?.close()
    await peer.terminate()
  }
}
