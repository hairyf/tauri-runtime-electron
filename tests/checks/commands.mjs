// Run after the parent builds assets. The supervisor catches leaked workers and accidental host exits.
import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { createRequire } from 'node:module'
import { fileURLToPath } from 'node:url'
import { setTimeout as sleep } from 'node:timers/promises'

const self = fileURLToPath(import.meta.url)
const success = 'commands: PASS'
const hostOut = ['host-before', 'host-during', 'host-after']
const hostErr = ['host-error-before', 'host-error-during', 'host-error-after']

function deadline(promise, label, ms = 4_000) {
  let timer
  return Promise.race([
    promise,
    new Promise((_, reject) => { timer = setTimeout(() => reject(Object.assign(new Error(`${label} exceeded ${ms}ms`), { code: 'CHECK_DEADLINE' })), ms) }),
  ]).finally(() => clearTimeout(timer))
}

async function until(predicate, label) {
  const end = performance.now() + 3_000
  while (!predicate()) {
    assert.ok(performance.now() < end, `${label} did not arrive`)
    await sleep(5)
  }
}

function nodeProcess(args, timeoutMs = 5_000) {
  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, args, { stdio: ['ignore', 'pipe', 'pipe'], windowsHide: true })
    const stdout = [], stderr = []
    let expired = false, size = 0
    const timer = setTimeout(() => { expired = true; child.kill('SIGKILL') }, timeoutMs)
    for (const [stream, chunks] of [[child.stdout, stdout], [child.stderr, stderr]]) {
      stream.on('data', chunk => {
        chunks.push(chunk)
        size += chunk.length
        if (size > 2 * 1024 * 1024) { expired = true; child.kill('SIGKILL') }
      })
    }
    child.once('error', error => { clearTimeout(timer); reject(error) })
    child.once('close', (code, signal) => {
      clearTimeout(timer)
      if (expired) reject(new Error(`Node check exceeded its ${timeoutMs}ms deadline or output limit`))
      else resolve({ code, signal, stdout: Buffer.concat(stdout), stderr: Buffer.concat(stderr) })
    })
  })
}

async function reference(script, args = []) {
  // A real Node process is the output oracle, never a mock of the executor.
  const result = await nodeProcess(['-e', `
    const args = ${JSON.stringify(args)};
    (async function(args) { ${script}\n })(args).catch(error => {
      console.error(error && error.stack || String(error)); process.exitCode = 1;
    });
  `])
  // CommandOutput.code is Rust i32, matching electron.node().status.code()
  // (std::process::ExitStatus), while Node exposes Windows exit DWORDs unsigned.
  return { ...result, code: process.platform === 'win32' ? (result.code | 0) : result.code }
}

function bytes(value, label) {
  assert.equal(typeof value, 'string', `${label} must be base64`)
  const decoded = Buffer.from(value, 'base64')
  assert.equal(decoded.toString('base64'), value, `${label} must be canonical base64`)
  return decoded
}

// Error locations differ between an async-function body and Node's -e wrapper.
const withoutFrames = buffer => buffer.toString('utf8').replace(/^\s+at .*\r?\n?/gm, '')

async function checks() {
  const write = (stream, text) => deadline(new Promise((resolve, reject) => {
    process[stream].write(text + '\n', error => error ? reject(error) : resolve())
  }), `host ${stream} write`)
  const saved = { exitCode: process.exitCode }
  await write('stdout', hostOut[0])
  await write('stderr', hostErr[0])
  const { Executions } = createRequire(import.meta.url)('../../electron-rs/assets/runtime/execution.cjs')
  assert.equal(typeof Executions, 'function', 'generated execution.cjs must export Executions')
  let nextId = 0
  const events = [], completed = new Set(), closeHandlers = []
  const transport = {
    send(event, params) { events.push({ event, params: structuredClone(params), afterResult: completed.has(params.taskId) }) },
    onClose(handler) { closeHandlers.push(handler) },
  }
  const service = new Executions(transport)
  const chunksFor = taskId => events.filter(entry => entry.event === 'command.output' && entry.params.taskId === taskId)
    .flatMap(entry => entry.params.chunks)
  const start = (mode, script, overrides = {}) => {
    const taskId = `command-check-${++nextId}`
    const promise = deadline(Promise.resolve().then(() => service.execute({
      taskId, mode, script, args: [], timeoutMs: 2_000, maxOutputBytes: 256 * 1024, stream: false, ...overrides,
    })), taskId).then(result => {
      completed.add(taskId)
      assert.ok(result && typeof result === 'object', `${taskId}: result object`)
      assert.ok(Number.isInteger(result.code), `${taskId}: integer exit code`)
      bytes(result.stdout, `${taskId} stdout`)
      bytes(result.stderr, `${taskId} stderr`)
      assert.ok(Object.hasOwn(result, 'result'), `${taskId}: JSON result field`)
      assert.deepEqual(JSON.parse(JSON.stringify(result.result)), result.result, `${taskId}: JSON result`)
      return result
    })
    // Attach immediately: concurrent jobs may reject before their turn is awaited.
    promise.catch(() => {})
    return { taskId, promise }
  }
  const streamed = (taskId, result) => {
    const entries = events.filter(entry => entry.params.taskId === taskId)
    assert.ok(entries.length > 0, `${taskId}: output events`)
    assert.ok(entries.every(entry => entry.event === 'command.output' && !entry.afterResult), `${taskId}: flush events before result`)
    const chunks = chunksFor(taskId)
    assert.ok(chunks.length > 0, `${taskId}: nonempty chunks`)
    assert.deepEqual(chunks.map(chunk => chunk.sequence), chunks.map((_, index) => index), `${taskId}: contiguous per-task sequences from zero`)
    for (const chunk of chunks) {
      assert.ok(['stdout', 'stderr'].includes(chunk.stream), `${taskId}: valid stream`)
      bytes(chunk.data, `${taskId} chunk`)
    }
    for (const stream of ['stdout', 'stderr']) {
      assert.deepEqual(Buffer.concat(chunks.filter(chunk => chunk.stream === stream).map(chunk => bytes(chunk.data, 'chunk'))), bytes(result[stream], stream), `${taskId}: streamed ${stream} reconstructs final bytes`)
    }
  }
  try {
    // Shared async-function bodies exercise actual Node formatting and write overloads.
    const cases = [
      ['text', `console.log('hello'); console.error('error text'); return { ok: true };`],
      ['console formatting', `console.log('%s %d %j', 'value', 12, { a: 1 }); console.dir({ list: [1, 'two'], nested: { yes: true } }); console.warn('warn', null, undefined, { value: 3 }); return null;`],
      ['no newline', `process.stdout.write('left'); process.stdout.write('right', 'utf8'); process.stderr.write('err'); return null;`],
      ['raw bytes', `process.stdout.write(Buffer.from([0, 255, 128, 195, 40, 10])); process.stderr.write(new Uint8Array([254, 0, 129, 13])); process.stdout.write('ff00', 'hex'); return null;`],
    ]
    for (const [name, script] of cases) {
      const expected = await reference(script)
      for (const mode of ['node', 'electron']) {
        const { taskId, promise } = start(mode, script)
        const actual = await promise
        assert.equal(actual.code, expected.code, `${mode} ${name}: exit code`)
        assert.deepEqual(bytes(actual.stdout, 'stdout'), expected.stdout, `${mode} ${name}: direct Node stdout`)
        assert.deepEqual(bytes(actual.stderr, 'stderr'), expected.stderr, `${mode} ${name}: direct Node stderr`)
        assert.equal(chunksFor(taskId).length, 0, 'stream:false does not emit command.output')
      }
    }

    const args = ['', 'spaces and quotes " \'', '中文', '\u0000', '--flag']
    for (const mode of ['node', 'electron']) {
      const actual = await start(mode, `return { args, value: [1, true, null, { text: 'json' }] };`, { args }).promise
      assert.equal(actual.code, 0)
      assert.deepEqual(actual.result, { args, value: [1, true, null, { text: 'json' }] })
      const identity = await start(mode, `const { isMainThread, threadId } = require('node:worker_threads'); return { pid: process.pid, isMainThread, threadId };`).promise
      assert.equal(identity.result.pid, process.pid, `${mode}: no child process`)
      assert.equal(identity.result.isMainThread, mode === 'electron', `${mode}: required execution thread`)
      assert.equal(identity.result.threadId === 0, mode === 'electron')
    }

    for (const script of [
      `process.stdout.write('before exit'); process.exitCode = 7; return 'exit-code';`,
      `await new Promise(resolve => process.stdout.write('before worker exit', resolve)); process.exit(9);`,
      `setTimeout(() => { process.stdout.write('late timer'); process.stderr.write('late error'); }, 40); return 'body finished';`,
    ]) {
      const expected = await reference(script)
      const actual = await start('node', script).promise
      assert.equal(actual.code, expected.code, 'worker natural/explicit exit code matches Node')
      assert.deepEqual(bytes(actual.stdout, 'stdout'), expected.stdout, 'worker timers finish naturally')
      assert.deepEqual(bytes(actual.stderr, 'stderr'), expected.stderr)
      if (script.includes('body finished')) assert.equal(actual.result, 'body finished')
    }
    // Derive platform exit-status normalization from real Node, including boundary values.
    const exitMismatches = []
    for (const value of [-1, 255, 256, 257]) {
      for (const action of [`process.exitCode = ${value}`, `process.exit(${value})`]) {
        const script = `await new Promise(resolve => process.stdout.write('exit-boundary', resolve)); ${action}; return null;`
        const expected = await reference(script)
        for (const mode of ['node', 'electron']) {
          const actual = await start(mode, script).promise
          if (actual.code !== expected.code) exitMismatches.push({ mode, action, expected: expected.code, actual: actual.code })
          assert.deepEqual(bytes(actual.stdout, 'stdout'), expected.stdout, `${mode} ${action}: direct Node stdout`)
          assert.deepEqual(bytes(actual.stderr, 'stderr'), expected.stderr, `${mode} ${action}: direct Node stderr`)
          assert.equal(process.exitCode, saved.exitCode, `${mode} ${action}: host exitCode restored`)
        }
      }
    }
    assert.deepEqual(exitMismatches, [], 'exit status must match direct Node on this platform')

    const thrown = `process.stdout.write('before throw'); throw new Error('command-check thrown marker');`
    const expectedError = await reference(thrown)
    for (const mode of ['node', 'electron']) {
      const actual = await start(mode, thrown).promise
      assert.equal(actual.code, expectedError.code, `${mode}: thrown body is unsuccessful`)
      assert.deepEqual(bytes(actual.stdout, 'stdout'), expectedError.stdout)
      assert.equal(withoutFrames(bytes(actual.stderr, 'stderr')), withoutFrames(expectedError.stderr), `${mode}: direct Node error text excluding stack locations`)
    }

    for (const mode of ['node', 'electron']) {
      const job = start(mode, `process.stdout.write(Buffer.from([0, 255, 195])); await new Promise(resolve => setTimeout(resolve, 350)); process.stderr.write(Buffer.from([128, 10])); process.stdout.write('tail'); return { done: true };`, { stream: true })
      let finished = false
      job.promise.finally(() => { finished = true }).catch(() => {})
      await until(() => chunksFor(job.taskId).length > 0 || finished, `${mode} first live chunk`)
      assert.equal(finished, false, `${mode}: first chunk must arrive BEFORE completion`)
      const result = await job.promise
      assert.deepEqual(result.result, { done: true })
      streamed(job.taskId, result)
      const count = chunksFor(job.taskId).length
      await sleep(40)
      assert.equal(chunksFor(job.taskId).length, count, 'no output events after result')
    }

    // Four overlapping jobs exercise async-local routing in both directions.
    const concurrent = ['node', 'electron', 'node', 'electron'].map((mode, index) => {
      const token = `job-${index}-${mode}`
      return { token, ...start(mode, `for (let i = 0; i < 4; i++) { console.log(${JSON.stringify(token)} + ':' + i); await new Promise(resolve => setTimeout(resolve, 20)); process.stderr.write(${JSON.stringify(token)} + '-err:' + i + '\\n'); } return ${JSON.stringify(token)};`, { stream: true }) }
    })
    await until(() => concurrent.every(job => chunksFor(job.taskId).length), 'all concurrent jobs started')
    await write('stdout', hostOut[1])
    await write('stderr', hostErr[1])
    const concurrentResults = await deadline(Promise.all(concurrent.map(job => job.promise)), 'concurrent jobs')
    for (const [index, job] of concurrent.entries()) {
      const actual = concurrentResults[index]
      assert.equal(actual.code, 0)
      assert.equal(actual.result, job.token)
      assert.equal(bytes(actual.stdout, 'stdout').toString(), Array.from({ length: 4 }, (_, i) => `${job.token}:${i}\n`).join(''))
      assert.equal(bytes(actual.stderr, 'stderr').toString(), Array.from({ length: 4 }, (_, i) => `${job.token}-err:${i}\n`).join(''))
      streamed(job.taskId, actual)
    }

    // The host's exitCode must survive a main-thread job that changes its own.
    process.exitCode = 23
    const mainExit = await start('electron', `process.exitCode = 17; await new Promise(resolve => setTimeout(resolve, 10)); return null;`).promise
    assert.equal(mainExit.code, 17)
    assert.equal(process.exitCode, 23, 'main job exitCode must not overwrite host exitCode')
    process.exitCode = saved.exitCode

    const cancel = start('node', `process.stdout.write('cancel-ready'); await new Promise(resolve => setTimeout(resolve, 20)); while (true) {}`, { stream: true })
    await until(() => chunksFor(cancel.taskId).length > 0, 'cancel worker readiness')
    await sleep(60) // Let the Worker enter the synchronous loop before cancelling it.
    await deadline(Promise.resolve(service.cancel({ taskId: cancel.taskId })), 'cancel request')
    const cancelled = await cancel.promise
    assert.equal(cancelled.code, 130, 'cancel terminates CPU-bound Worker')
    streamed(cancel.taskId, cancelled)
    const timeout = await start('node', `while (true) {}`, { timeoutMs: 150 }).promise
    assert.equal(timeout.code, 124, 'timeout terminates CPU-bound Worker')
    assert.equal((await start('node', `return 'still usable';`).promise).result, 'still usable')

    for (const mode of ['node', 'electron']) {
      for (const stream of ['stdout', 'stderr']) {
        const overflow = start(mode, `process.${stream}.write(Buffer.alloc(4096, 120)); return 'must not silently truncate';`, { maxOutputBytes: 64 })
        let result, failure
        try { result = await overflow.promise } catch (error) { failure = error }
        if (failure) {
          assert.notEqual(failure.code, 'CHECK_DEADLINE', 'cap overflow must settle without reaching the test deadline')
          assert.match(String(failure), /output|limit|cap|overflow|exceed|64/i, `${mode}: explicit cap error`)
        } else {
          assert.notEqual(result.code, 0, `${mode}: cap overflow cannot report successful truncation`)
          assert.match(bytes(result.stderr, 'stderr').toString(), /output|limit|cap|overflow|exceed|64/i, `${mode}: explicit cap error`)
        }
      }
    }

    const closing = start('node', `process.stdout.write('close-ready'); setInterval(() => process.stdout.write('tick'), 25);`, { stream: true })
    await until(() => chunksFor(closing.taskId).length > 0, 'close worker readiness')
    await deadline(Promise.resolve(service.close()), 'service.close')
    assert.equal((await closing.promise).code, 130, 'close settles and terminates active Worker')
    const eventCount = events.length
    await sleep(80)
    assert.equal(events.length, eventCount, 'close leaves no continuing worker output')
    assert.equal(process.exitCode, saved.exitCode, 'host exitCode restored')
    await write('stdout', hostOut[2])
    await write('stderr', hostErr[2])

    // The registered transport close callback must perform the same cleanup.
    assert.ok(closeHandlers.length > 0, 'service registers transport shutdown cleanup')
    const disconnected = new Executions(transport)
    const pendingId = 'disconnect-check'
    const pending = deadline(disconnected.execute({ taskId: pendingId, mode: 'node', script: `process.stdout.write('disconnect-ready'); setInterval(() => {}, 25);`, args: [], timeoutMs: 2_000, maxOutputBytes: 1024, stream: true }), 'transport-close Worker')
    pending.catch(() => {})
    try {
      await until(() => chunksFor(pendingId).length > 0, 'transport-close worker readiness')
      for (const handler of closeHandlers) handler()
      assert.equal((await pending).code, 130, 'transport close settles active Worker')
    } finally { await deadline(Promise.resolve(disconnected.close()), 'disconnected.close') }
  } finally {
    await deadline(Promise.resolve(service.close()), 'final cleanup')
    process.exitCode = saved.exitCode
  }
  console.log(success)
}

if (process.argv[2] === '--run') {
  await checks()
} else {
  const actual = await nodeProcess([self, '--run'], 60_000)
  assert.equal(actual.signal, null, 'suite must exit naturally without leaked Worker handles')
  assert.equal(actual.code, 0, `command suite failed:\n${actual.stdout}\n${actual.stderr}`)
  assert.equal(actual.stdout.toString(), [...hostOut, success, ''].join('\n'), 'jobs must not leak output to host; premature host exit must fail')
  assert.equal(actual.stderr.toString(), [...hostErr, ''].join('\n'), 'ordinary host stderr stays outside job capture')
  console.log(`${success} (Node byte oracle, main/worker identity, async routing, live streaming, cancel/timeout, limits, shutdown)`)
}
