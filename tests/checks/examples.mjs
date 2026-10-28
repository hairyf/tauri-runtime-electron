#!/usr/bin/env node
// Runs the dashboard or transparent fixture and retains unmodified keyframes.
// A successful assertion is not a visual verdict.
//   pnpm check                       # coverage dashboard
//   pnpm check coverage transparent
import assert from 'node:assert/strict'
import { spawn, spawnSync } from 'node:child_process'
import { closeSync, existsSync, mkdirSync, openSync, readdirSync, realpathSync, rmSync, writeFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { createServer } from 'node:net'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import transparentChecks from './transparent.mjs'

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..')
const artifacts = path.join(root, 'target', 'example-checks')
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms))
const labelExpression = 'window.__TAURI_INTERNALS__?.metadata?.currentWindow?.label'

async function until(read, accept = Boolean, timeout = 15_000) {
  const deadline = Date.now() + timeout
  let value
  let error
  do {
    try {
      value = await read()
      if (accept(value)) return value
      error = undefined
    } catch (reason) {
      if (reason.fatal) throw reason
      error = reason
    }
    await sleep(100)
  } while (Date.now() < deadline)
  throw new Error(`condition timed out: ${error?.message ?? JSON.stringify(value)}`)
}

/** The Chromium shipped by this runtime is the only browser this test needs. */
class Session {
  next = 0
  pending = new Map()
  events = []

  static async connect(target) {
    const socket = new WebSocket(target.webSocketDebuggerUrl)
    await new Promise((resolve, reject) => {
      const timer = setTimeout(() => { socket.close(); reject(new Error('CDP connect timed out')) }, 5000)
      socket.onopen = () => { clearTimeout(timer); resolve() }
      socket.onerror = () => { clearTimeout(timer); reject(new Error(`cannot attach to ${target.url}`)) }
    })
    const session = new Session()
    session.socket = socket
    session.target = target
    socket.onmessage = ({ data }) => {
      const message = JSON.parse(data)
      if (!message.id) { session.events.push(message); return }
      const pending = session.pending.get(message.id)
      if (!pending) return
      session.pending.delete(message.id)
      clearTimeout(pending.timer)
      if (message.error) pending.reject(new Error(message.error.message))
      else pending.resolve(message.result)
    }
    socket.onclose = () => {
      for (const { timer, reject } of session.pending.values()) {
        clearTimeout(timer)
        reject(new Error('CDP target closed'))
      }
      session.pending.clear()
    }
    await session.call('Runtime.enable')
    await session.call('Page.enable')
    return session
  }

  call(method, params = {}, timeout = 20_000, sessionId) {
    return new Promise((resolve, reject) => {
      const id = ++this.next
      const timer = setTimeout(() => {
        this.pending.delete(id)
        reject(new Error(`${method} timed out`))
      }, timeout)
      this.pending.set(id, { resolve, reject, timer })
      try { this.socket.send(JSON.stringify({ id, method, params, ...(sessionId ? { sessionId } : {}) })) }
      catch (error) { clearTimeout(timer); this.pending.delete(id); reject(error) }
    })
  }

  async evaluate(expression) {
    const { result, exceptionDetails } = await this.call('Runtime.evaluate', {
      expression, awaitPromise: true, returnByValue: true,
    })
    if (exceptionDetails) throw new Error(exceptionDetails.exception?.description ?? result?.description ?? JSON.stringify(exceptionDetails))
    return result?.value
  }

  close() { this.socket.close() }
}

async function click(page, selector) {
  await page.call('Page.bringToFront')
  const point = await page.evaluate(`(() => {
    const element = document.querySelector(${JSON.stringify(selector)})
    if (!element || element.disabled) throw new Error('not actionable: ' + ${JSON.stringify(selector)})
    element.scrollIntoView({ block: 'center' })
    const rect = element.getBoundingClientRect()
    if (!rect.width || !rect.height) throw new Error('not visible')
    return { x: rect.x + rect.width / 2, y: rect.y + rect.height / 2 }
  })()`)
  await page.call('Input.dispatchMouseEvent', { type: 'mousePressed', button: 'left', clickCount: 1, ...point })
  await page.call('Input.dispatchMouseEvent', { type: 'mouseReleased', button: 'left', clickCount: 1, ...point })
}

function nativeBlocked(message) {
  throw Object.assign(new Error(`BLOCKED: ${message}`), { blocked: true })
}

/** Bind OS observations to this runner's Electron descendant, never another repo window. */
async function native(test, action, options = {}) {
  const helper = path.join(root, 'tests/checks/example-native.ps1')
  if (process.platform !== 'win32' || !existsSync(helper)) nativeBlocked('Windows native window observation is unavailable')
  const literal = (value) => `'${String(value).replaceAll("'", "''")}'`
  const args = Object.entries({ Action: action, RepositoryPath: root, ...options })
    .map(([key, value]) => typeof value === 'boolean' ? (value ? `-${key}` : '') : `-${key} ${literal(value)}`).join(' ')
  const lookup = test.nativePid ? `$ownerId = ${test.nativePid}` : `
    [Console]::Error.WriteLine('[native-phase] owner lookup start')
    $rows = @(Get-CimInstance Win32_Process)
    $parents = @{}
    foreach ($row in $rows) { $parents[[int] $row.ProcessId] = [int] $row.ParentProcessId }
    $owners = @(foreach ($row in $rows) {
      if ([string] $row.CommandLine -notmatch '(?:^|\\s)--remote-debugging-port=\\d+(?:\\s|$)' -or [string] $row.CommandLine -match '(?:^|\\s)--type=') { continue }
      $ancestor = [int] $row.ProcessId
      $seen = @{}
      while ($ancestor -and -not $seen.ContainsKey($ancestor)) {
        if ($ancestor -eq ${test.child.pid}) { $row; break }
        $seen[$ancestor] = $true
        $ancestor = [int] $parents[$ancestor]
      }
    })
    [Console]::Error.WriteLine('[native-phase] owner lookup done')
    if ($owners.Count -eq 0) { '{"ok":true,"windows":[]}'; exit 0 }
    if ($owners.Count -ne 1) { throw 'Ambiguous Electron descendants of this runner' }
    $ownerId = [int] $owners[0].ProcessId
  `
  const source = `$ErrorActionPreference = 'Stop'; try { ${lookup}; & ${literal(helper)} -ProcessId $ownerId ${args} } catch { @{ ok = $false; error = $_.Exception.Message } | ConvertTo-Json -Compress; exit 1 }`
  const { value, exitCode, stderr, raw } = await new Promise((resolve, reject) => {
    const child = spawn('pwsh', ['-NoProfile', '-NonInteractive', '-EncodedCommand', Buffer.from(source, 'utf16le').toString('base64')], { cwd: root, windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'] })
    let stdout = ''
    let stderr = ''
    const failure = (message) => {
      const phase = [...stderr.matchAll(/^\[native-phase\] (.+)$/gm)].at(-1)?.[1] ?? 'pwsh startup'
      return Object.assign(new Error(`${action}: ${message}; last phase: ${phase}; stderr: ${stderr.trim() || '(empty)'}; stdout: ${stdout.trim() || '(empty)'}`), { stderr, raw: stdout, phase })
    }
    const timer = setTimeout(() => { child.kill(); reject(failure('native helper timed out after 60000ms')) }, 60_000)
    child.stdout.on('data', (data) => { stdout += data })
    child.stderr.on('data', (data) => { stderr += data })
    child.on('error', (error) => { clearTimeout(timer); reject(failure(error.message)) })
    child.on('close', (code) => {
      clearTimeout(timer)
      try {
        const value = JSON.parse(stdout.trim().replace(/^\uFEFF/, ''))
        resolve({ value, exitCode: code, stderr, raw: stdout })
      } catch (error) { reject(failure(error.message)) }
    })
  }).catch((error) => {
    test.nativeObservations ??= []
    test.nativeObservations.push({ observedAt: new Date().toISOString(), action, ok: false, error: error.message, phase: error.phase, stderr: error.stderr, raw: error.raw })
    writeFileSync(path.join(test.directory, 'native-observations.json'), `${JSON.stringify(test.nativeObservations, null, 2)}\n`)
    nativeBlocked(error.message)
  })
  if (value.windows?.length) {
    const owners = new Set(value.windows.map((window) => window.pid))
    assert.equal(owners.size, 1, 'native windows must share the runner-owned Electron PID')
    test.nativePid = value.windows[0].pid
  }
  test.nativeObservations ??= []
  test.nativeObservations.push({ observedAt: new Date().toISOString(), action, ...value, exitCode, stderr, ...(!value.ok ? { raw } : {}) })
  writeFileSync(path.join(test.directory, 'native-observations.json'), `${JSON.stringify(test.nativeObservations, null, 2)}\n`)
  if (value.assertion && value.assertion.expected !== 'Any') assert.equal(value.assertion.passed, true, `${action}: ${JSON.stringify(value)}`)
  if (exitCode !== 0 || !value.ok) nativeBlocked(`${action}: ${value.error || stderr.trim() || `native helper exited ${exitCode}`}`)
  return value
}

function nativeWindow(snapshot, title) {
  const matches = snapshot.windows.filter((window) => window.title === title && window.className === 'Chrome_WidgetWin_1')
  assert.equal(matches.length, 1, `expected one runner-owned native window titled ${JSON.stringify(title)}`)
  return matches[0]
}

async function nativeWindows(test, titles, timeout = 15_000) {
  const deadline = Date.now() + timeout
  do {
    const snapshot = await native(test, 'List')
    if (titles.every((title) => snapshot.windows.some((window) => window.title === title && window.className === 'Chrome_WidgetWin_1'))) return snapshot
    await sleep(100)
  } while (Date.now() < deadline)
  nativeBlocked(`could not observe the original native windows: ${titles.join(', ')}`)
}

const checks = {
  transparent: (test) => transparentChecks(test, { assert, click, until, native, nativeWindow, nativeWindows }),
  coverage: async (test) => {
    const page = await test.page('main')
    const coverage = await until(() => page.evaluate('window.__coverage'), (value) => value?.done === true, 120_000)
    test.coverage = coverage
    // Retain results and a screenshot even when a dashboard assertion failed.
    await test.shot(page, '01-coverage-complete')
    assert.ok(!coverage.error, `dashboard error: ${coverage.error}`)
    assert.ok(Array.isArray(coverage.results) && coverage.results.length, 'dashboard must publish nonempty results')
    for (const result of coverage.results) {
      assert.equal(typeof result.id, 'string', 'dashboard result requires an id')
      assert.ok(['pass', 'fail', 'skip', 'na'].includes(result.status), `unknown dashboard status: ${result.status}`)
    }
    const failures = coverage.results.filter((result) => result.status === 'fail')
    assert.equal(failures.length, 0, `dashboard failures: ${failures.map((result) => `${result.id}: ${result.detail}`).join('; ')}`)
    for (const result of coverage.results) {
      if (result.status === 'pass') test.pass(`${result.id}: ${result.detail}`)
      else test.limitations.push(`${result.id}: ${result.detail}`)
    }
  },
}

async function freePort() {
  const server = createServer()
  await new Promise((resolve, reject) => { server.once('error', reject); server.listen(0, '127.0.0.1', resolve) })
  const port = server.address().port
  await new Promise((resolve) => server.close(resolve))
  return port
}

async function check(name) {
  const cwd = path.join(root, name === 'transparent' ? 'tests/assets/transparent' : 'example')
  const [command, args] = name === 'transparent'
    ? ['cargo', ['run', '--locked', '--manifest-path', path.join(cwd, 'Cargo.toml')]]
    : [process.execPath, [createRequire(path.join(cwd, 'package.json')).resolve('@tauri-apps/cli/tauri.js'), 'dev', '--no-watch']]
  const directory = path.join(artifacts, name)
  mkdirSync(directory, { recursive: true })
  assert.equal(path.dirname(path.resolve(directory)), path.resolve(root, 'target/example-checks'), 'evidence cleanup must stay inside target/example-checks')
  assert.equal(realpathSync(directory), path.resolve(directory), 'evidence cleanup must not follow a redirected directory')
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    if (entry.isFile() && (entry.name.endsWith('.png') || ['native-observations.json', 'result.json', 'run.log'].includes(entry.name))) {
      rmSync(path.join(directory, entry.name))
    }
  }
  const log = path.join(directory, 'run.log')
  const output = openSync(log, 'w')
  const port = await freePort()
  const child = spawn(command, args, {
    cwd, stdio: ['ignore', output, output], detached: process.platform !== 'win32',
    env: { ...process.env, INIT_CWD: cwd, CARGO_TARGET_DIR: path.join(root, 'target'), TAURI_RUNTIME_ELECTRON_ARGS: `${process.env.TAURI_RUNTIME_ELECTRON_ARGS ?? ''} --remote-debugging-address=127.0.0.1 --remote-debugging-port=${port}` },
  })
  const sessions = new Map()
  const report = { name, platform: process.platform, started: new Date().toISOString(), checks: [], frames: [], assertions: 'failed', visualReview: 'pending' }
  child.on('error', (error) => { report.error = error.message })
  const targets = async () => {
    const response = await fetch(`http://127.0.0.1:${port}/json/list`, { signal: AbortSignal.timeout(1500) })
    assert.ok(response.ok, 'CDP endpoint failed')
    return (await response.json()).filter((target) => ['page', 'webview'].includes(target.type) && !target.url.startsWith('devtools:'))
  }
  const pages = async () => (await targets()).filter((target) => /^(tauri:|https?:|file:|data:)/.test(target.url))
  async function session(target) {
    if (!sessions.has(target.id)) sessions.set(target.id, await Session.connect(target))
    return sessions.get(target.id)
  }
  const test = {
    child, directory, limitations: [], coverage: [],
    pass(message) { report.checks.push(message); console.log(`  ok   ${message}`) },
    async page(label) {
      return until(async () => {
        if (report.error || child.exitCode !== null) throw Object.assign(new Error(report.error ?? `runner exited ${child.exitCode}; see ${log}`), { fatal: true })
        for (const target of await pages()) {
          const page = await session(target)
          if (await page.evaluate('document.readyState === "complete" && Boolean(window.__TAURI_INTERNALS__?.invoke)') && (!label || await page.evaluate(labelExpression) === label)) return page
        }
      }, Boolean, 900_000)
    },
    async nativeShot(window, key) {
      const file = path.join(directory, `${key}.png`)
      if (existsSync(file)) rmSync(file)
      const capture = await native(test, 'Capture', { WindowHandle: window.hwnd, OutputPath: file, Activate: true })
      report.frames.push({ file: path.relative(root, file).replaceAll('\\', '/'), title: capture.window.title, capture: capture.capture, window: capture.window })
      console.log(`  frame ${path.relative(root, file)} (native)`)
    },
    async shot(page, key, foreground = true) {
      if (foreground) await page.call('Page.bringToFront')
      const { data } = await page.call('Page.captureScreenshot', { format: 'png', captureBeyondViewport: false })
      const file = path.join(directory, `${key}.png`)
      writeFileSync(file, Buffer.from(data, 'base64'))
      const state = await page.evaluate(`({ label: ${labelExpression}, url: location.href, text: document.body.innerText.slice(0, 8000), size: [innerWidth, innerHeight] })`)
      report.frames.push({ file: path.relative(root, file).replaceAll('\\', '/'), ...state })
      console.log(`  frame ${path.relative(root, file)}`)
    },
  }
  try {
    await checks[name](test)
    assert.ok(report.frames.length, 'no visual evidence was captured')
    report.assertions = 'passed'
  } catch (error) {
    report.error = error.message
    if (error.blocked) report.assertions = 'blocked'
    report.rendererErrors = [...sessions.values()].flatMap((page) => page.events
      .filter((event) => event.method === 'Runtime.exceptionThrown' || (event.method === 'Runtime.consoleAPICalled' && event.params.type === 'error'))
      .map((event) => ({ target: page.target.url, ...event })))
    if (name === 'coverage') {
      // A timeout/closed target must not overwrite the original failure.
      report.coverageDiagnostics = await Promise.all([...sessions.values()].map(async (page) => ({
        target: page.target.url,
        coverage: await page.evaluate('window.__coverage').catch((error) => ({ diagnosticError: error.message })),
      })))
    }
    for (const page of sessions.values()) {
      try { await test.shot(page, `failure-${report.frames.length + 1}`, false) } catch {}
    }
    console.error(`  FAIL ${report.error}`)
  } finally {
    // Normal window closure first, then stop only this runner's process tree.
    for (const page of sessions.values()) {
      try { await page.evaluate('window.__TAURI__?.window?.getCurrentWindow().close()') } catch {}
    }
    const deadline = Date.now() + 8000
    while (child.exitCode === null && Date.now() < deadline) await sleep(100)
    if (child.exitCode === null) {
      if (process.platform === 'win32') spawnSync('taskkill', ['/PID', String(child.pid), '/T', '/F'], { stdio: 'ignore' })
      else { try { process.kill(-child.pid, 'SIGKILL') } catch {} }
      await until(() => child.exitCode, (code) => code !== null, 5000).catch(() => {})
    }
    for (const page of sessions.values()) page.close()
    closeSync(output)
    report.finished = new Date().toISOString()
    report.limitations = test.limitations
    report.coverage = test.coverage
    writeFileSync(path.join(directory, 'result.json'), `${JSON.stringify(report, null, 2)}\n`)
  }
  return report
}

const available = Object.keys(checks)
const requested = process.argv.slice(2)
assert.ok(requested.every((name) => available.includes(name)), `unknown example; choose ${available.join(', ')}`)
const results = []
for (const name of requested.length ? requested : ['coverage']) {
  console.log(`[check] ${name}`)
  results.push(await check(name))
}
mkdirSync(artifacts, { recursive: true })
writeFileSync(path.join(artifacts, 'results.json'), `${JSON.stringify(results, null, 2)}\n`)
for (const result of results) console.log(`${result.assertions === 'passed' ? 'ASSERTIONS' : 'FAIL'} ${result.name}: ${result.error ?? 'passed; screenshots await visual review'}`)
process.exitCode = results.some((result) => result.assertions !== 'passed') ? 1 : 0
