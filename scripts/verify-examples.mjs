// Real-application acceptance driver. No browser mock and no skipped-as-passed checks.
// node scripts/verify-examples.mjs [names...] [--no-install]
import assert from 'node:assert/strict'
import { spawn, spawnSync } from 'node:child_process'
import fs from 'node:fs'
import path from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { createInterface } from 'node:readline'
import { exampleNames } from './example.mjs'
import { cases } from './verify-cases.mjs'

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const args = process.argv.slice(2)
const names = args.filter((arg) => !arg.startsWith('--'))
const selected = names.length ? names : exampleNames()
for (const name of selected) assert(exampleNames().includes(name), `unknown example ${name}`)
const evidence = path.join(root, '.cache', 'acceptance')
fs.mkdirSync(evidence, { recursive: true })
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms))

async function connect(url) {
  const socket = new WebSocket(url)
  const pending = new Map()
  let next = 1
  await new Promise((resolve, reject) => {
    const timer = setTimeout(() => { socket.close(); reject(new Error('CDP connection timed out')) }, 5000)
    socket.onopen = () => { clearTimeout(timer); resolve() }
    socket.onerror = () => { clearTimeout(timer); reject(new Error(`CDP connection failed: ${url}`)) }
  })
  socket.onmessage = ({ data }) => {
    const message = JSON.parse(data)
    const entry = pending.get(message.id)
    if (!entry) return
    pending.delete(message.id)
    clearTimeout(entry.timer)
    if (message.error) entry.reject(new Error(message.error.message))
    else entry.resolve(message.result)
  }
  socket.onclose = () => {
    for (const entry of pending.values()) {
      clearTimeout(entry.timer)
      entry.reject(new Error('CDP target disconnected'))
    }
    pending.clear()
  }
  return {
    send(method, params = {}, timeout = 20000) {
      const id = next++
      return new Promise((resolve, reject) => {
        const timer = setTimeout(() => { pending.delete(id); reject(new Error(`${method} timed out`)) }, timeout)
        pending.set(id, { resolve, reject, timer })
        socket.send(JSON.stringify({ id, method, params }))
      })
    },
    close() { socket.close() },
  }
}

async function command(target, method, params = {}, timeout = 20000) {
  const session = await connect(target.webSocketDebuggerUrl)
  try { return await session.send(method, params, timeout) }
  finally { session.close() }
}

async function evaluate(target, expression, timeout = 20000) {
  const result = await command(target, 'Runtime.evaluate', {
    expression, awaitPromise: true, returnByValue: true, userGesture: true,
  }, timeout)
  if (result.exceptionDetails) throw new Error(result.exceptionDetails.exception?.description ?? result.exceptionDetails.text)
  return result.result?.value
}

function stop(child) {
  if (!child.pid || child.exitCode !== null) return
  if (process.platform === 'win32') {
    spawnSync('taskkill', ['/F', '/T', '/PID', String(child.pid)], { stdio: 'ignore' })
  } else {
    try { process.kill(-child.pid, 'SIGTERM') } catch {}
  }
}

async function watchSplash() {
  const child = spawn('pwsh', [
    '-NoProfile', '-File', path.join(root, 'scripts', 'verify-native.ps1'),
    '-RootPid', String(process.pid), '-Action', 'watch-splash',
  ], { cwd: root, stdio: ['ignore', 'pipe', 'pipe'] })
  let ready, rejectReady, resolveState, rejectState
  const started = new Promise((resolve, reject) => { ready = resolve; rejectReady = reject })
  const state = new Promise((resolve, reject) => { resolveState = resolve; rejectState = reject })
  state.catch(() => {})
  const lines = createInterface({ input: child.stdout })
  let errors = ''
  child.stderr.on('data', (data) => { errors += data })
  lines.on('line', (line) => {
    if (line.trim() === 'READY') ready()
    else {
      try { resolveState(JSON.parse(line.replace(/^\uFEFF/, ''))) }
      catch (error) { rejectState(error) }
    }
  })
  child.on('error', (error) => { rejectReady(error); rejectState(error) })
  child.on('exit', (code) => {
    const error = new Error(errors || `splash watcher exited (${code}) before recording the initial windows`)
    rejectReady(error); rejectState(error)
  })
  const timer = setTimeout(() => { rejectReady(new Error('splash watcher did not become ready')); stop(child) }, 30000)
  try { await started }
  catch (error) { stop(child); throw error }
  finally { clearTimeout(timer) }
  return { state, stop: () => { stop(child); lines.close() } }
}

async function run(name, index) {
  const dir = path.join(evidence, name)
  fs.mkdirSync(dir, { recursive: true })
  // Retain other examples' evidence, never mix this case with a previous run.
  const result = { name, platform: process.platform, started: new Date().toISOString(), checks: [], notes: [], screenshots: [] }
  const fileFixtures = ['hello world.txt', '文件.txt'].map((file) => path.join(dir, file))
  if (name === 'file-associations') for (const [i, file] of fileFixtures.entries()) fs.writeFileSync(file, `fixture ${i}\n`)
  const port = 9400 + index
  const logFd = fs.openSync(path.join(dir, 'app.log'), 'w')
  const splashWatch = name === 'splashscreen' ? await watchSplash() : null
  const child = spawn(process.execPath, [
    path.join(root, 'scripts', 'example.mjs'), name,
    ...(args.includes('--no-install') ? ['--no-install'] : []),
    ...(name === 'file-associations' ? ['--', ...fileFixtures.map((file) => pathToFileURL(file).href)] : []),
  ], {
    cwd: root,
    stdio: ['ignore', logFd, logFd],
    env: { ...process.env, TAURI_RUNTIME_ELECTRON_DEBUG_PORT: String(port) },
    detached: process.platform !== 'win32',
  })
  fs.closeSync(logFd)
  const output = () => fs.readFileSync(path.join(dir, 'app.log'), 'utf8')
  const exited = new Promise((resolve, reject) => {
    child.once('exit', (code, signal) => resolve(signal ?? code))
    child.once('error', reject)
  })
  const targets = async () => {
    const response = await fetch(`http://127.0.0.1:${port}/json/list`, { signal: AbortSignal.timeout(2000) })
    assert(response.ok, 'CDP target list must answer')
    return response.json()
  }
  const t = {
    name, root, dir, fileFixtures, command, evaluate, output, targets, childPid: child.pid,
    splashBefore: () => splashWatch.state,
    native(action, params = {}) {
      assert.equal(process.platform, 'win32', 'native desktop checks require Windows')
      const nativeResult = spawnSync('pwsh', [
        '-NoProfile', '-File', path.join(root, 'scripts', 'verify-native.ps1'),
        '-RootPid', String(child.pid), '-Action', action,
        ...Object.entries(params).flatMap(([key, value]) => [`-${key}`, String(value)]),
      ], { cwd: root, encoding: 'utf8', timeout: 20000 })
      if (nativeResult.error || nativeResult.status !== 0) {
        throw new Error(nativeResult.error?.message ?? nativeResult.stderr ?? `native ${action} failed`)
      }
      const value = JSON.parse(nativeResult.stdout.trim().replace(/^\uFEFF/, ''))
      if (action === 'capture') {
        const file = path.resolve(params.OutputPath)
        result.screenshots.push({ label: path.basename(file, '.png'), file: path.relative(root, file).replaceAll('\\', '/'), inspected: false, kind: 'desktop' })
      }
      return value
    },
    check(label, actual, expected) {
      const item = { label, actual, expected, ok: true }
      try { assert.deepStrictEqual(actual, expected, label) }
      catch (error) { item.ok = false; throw error }
      finally { result.checks.push(item); console.log(`${item.ok ? 'ok' : 'FAIL'} ${name}: ${label}`) }
    },
    note(label, reason) { result.notes.push({ label, reason }); console.log(`UNVERIFIED ${name}: ${label} — ${reason}`) },
    async wait(target, expression, predicate, timeout = 20000) {
      const deadline = Date.now() + timeout
      let last
      while (Date.now() < deadline) {
        last = await evaluate(target, expression, Math.min(timeout, 20000))
        if (predicate(last)) return last
        await delay(100)
      }
      throw new Error(`wait failed: ${expression.slice(0, 180)}; last=${JSON.stringify(last)}`)
    },
    async target(predicate, timeout = 20000) {
      const deadline = Date.now() + timeout
      let lastError
      while (Date.now() < deadline) {
        try {
          for (const candidate of await targets()) if (await predicate(candidate)) return candidate
        } catch (error) { lastError = error }
        if (child.exitCode !== null) throw new Error(`app exited (${child.exitCode})\n${output().slice(-1600)}`)
        await delay(100)
      }
      throw new Error(`target not found${lastError ? `: ${lastError.message}` : ''}`)
    },
    async click(target, selector, expression) {
      const point = await evaluate(target, `(() => {
        const element = ${expression ?? `document.querySelector(${JSON.stringify(selector)})`}
        if (!element) throw new Error('missing clickable element: ' + ${JSON.stringify(selector)})
        if (element.disabled) throw new Error('element is disabled')
        element.scrollIntoView({ block: 'center', inline: 'center' })
        const rect = element.getBoundingClientRect()
        if (rect.width <= 0 || rect.height <= 0) throw new Error('element is not visible')
        return { x: rect.x + rect.width / 2, y: rect.y + rect.height / 2 }
      })()`)
      await command(target, 'Input.dispatchMouseEvent', { type: 'mouseMoved', ...point })
      await command(target, 'Input.dispatchMouseEvent', { type: 'mousePressed', button: 'left', clickCount: 1, ...point })
      await command(target, 'Input.dispatchMouseEvent', { type: 'mouseReleased', button: 'left', clickCount: 1, ...point })
    },
    async button(target, label) {
      return t.click(target, `button ${label}`, `[...document.querySelectorAll('button')].find(e => e.textContent.trim() === ${JSON.stringify(label)})`)
    },
    async capture(target, label) {
      assert(/^[a-z0-9_-]+$/i.test(label), 'screenshot label must be a basename')
      const file = path.join(dir, `${label}.png`)
      const data = await command(target, 'Page.captureScreenshot', { format: 'png', captureBeyondViewport: false })
      fs.writeFileSync(file, Buffer.from(data.data, 'base64'))
      const state = await evaluate(target, `({ url: location.href, title: document.title, text: document.body?.innerText, width: innerWidth, height: innerHeight })`)
      fs.writeFileSync(path.join(dir, `${label}.json`), JSON.stringify(state, null, 2))
      result.screenshots.push({ label, file: path.relative(root, file).replaceAll('\\', '/'), inspected: false })
      console.log(`SHOT ${name}: ${label}`)
    },
    async closed(timeout = 20000) {
      let timer
      try {
        return await Promise.race([exited, new Promise((_, reject) => { timer = setTimeout(() => reject(new Error('app did not exit after window closed')), timeout) })])
      } finally { clearTimeout(timer) }
    },
  }
  try {
    assert(cases[name], `missing acceptance case ${name}`)
    // A clean checkout can spend several minutes building all Rust dependencies.
    t.main = await t.target((target) => target.type === 'page' && !target.url.startsWith('devtools://'), 600000)
    // Splash has a real 2s transition; observe it before generic CDP work.
    if (name !== 'splashscreen') {
      await t.wait(t.main, `document.readyState`, (state) => state === 'complete' || state === 'interactive')
      await t.capture(t.main, `${name}-initial`)
    }
    await cases[name](t)
    assert(result.checks.length > 0, 'a case without assertions cannot pass')
    result.status = result.notes.length ? 'assertions-passed-with-unverified-scope' : 'assertions-passed-awaiting-visual-review'
  } catch (error) {
    result.status = 'failed'
    result.error = String(error.stack ?? error)
    console.error(`FAIL ${name}: ${error.message}`)
    try { for (const [i, target] of (await targets()).entries()) await t.capture(target, `${name}-failure-${i}`) } catch {}
  } finally {
    stop(child)
    splashWatch?.stop()
    await Promise.race([exited.catch(() => {}), delay(2000)])
    result.finished = new Date().toISOString()
    fs.writeFileSync(path.join(dir, 'result.json'), JSON.stringify(result, null, 2) + '\n')
  }
  return result
}

const results = []
for (const [index, name] of selected.entries()) results.push(await run(name, index))
fs.writeFileSync(path.join(evidence, 'summary.json'), JSON.stringify(results, null, 2) + '\n')
console.log(results.map(({ name, status }) => `${name}: ${status}`).join('\n'))
process.exitCode = results.some((item) => item.status === 'failed') ? 1 : 0
