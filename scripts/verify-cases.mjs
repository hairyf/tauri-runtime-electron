import assert from 'node:assert/strict'
import fs from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { verifyApi } from './verify-api.mjs'
import { observer, verifyDrag, verifyIsolation, verifyMultiwebview } from './verify-embedded.mjs'

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const read = (relative) => fs.readFileSync(path.join(root, relative), 'utf8')
const json = (value) => JSON.stringify(value)

async function text(t, target, selector) {
  return t.evaluate(target, `document.querySelector(${json(selector)})?.textContent ?? ''`)
}

async function setValue(t, target, selector, value) {
  await t.evaluate(
    target,
    `(() => {
      const element = document.querySelector(${json(selector)})
      if (!element) throw new Error(${json(`missing ${selector}`)})
      element.focus()
      element.value = ${json(value)}
      element.dispatchEvent(new Event('input', { bubbles: true }))
      element.dispatchEvent(new Event('change', { bubbles: true }))
    })()`,
  )
}

async function click(t, target, selector) {
  await t.click(target, selector)
}

async function clickButton(t, target, label) {
  await t.button(target, label)
}

async function labelOf(t, target) {
  return t.evaluate(target, 'window.__TAURI_INTERNALS__?.metadata?.currentWindow?.label ?? null')
}

async function targetByLabel(t, label, timeoutMs = 20000) {
  return t.target(
    async (candidate) => candidate.type === 'page' && (await labelOf(t, candidate)) === label,
    timeoutMs,
  )
}

async function invoke(t, target, name, args = {}) {
  return t.evaluate(
    target,
    `(async () => {
      try {
        return { ok: true, value: await window.__TAURI__.core.invoke(${json(name)}, ${json(args)}) }
      } catch (error) {
        return { ok: false, error: String(error) }
      }
    })()`,
  )
}

async function sendMessage(t, source, label, message) {
  await setValue(t, source, '#send-label', label)
  await setValue(t, source, '#send-message', message)
  await click(t, source, '#send-message-form button')
}

const missing = (command, key) =>
  `Err(invalid args \`${key}\` for command \`${command}\`: command ${command} missing required key ${key})`

const commandNames = [
  'borrow_cmd',
  'raw_request',
  'window_label',
  'simple_command',
  'stateful_command',
  'async_simple_command',
  'async_simple_command_snake',
  'future_simple_command',
  'async_stateful_command',
  'simple_command_with_result',
  'future_simple_command_snake',
  'future_simple_command_with_return_snake',
  'future_simple_command_with_result_snake',
  'force_async_snake',
  'force_async_with_result_snake',
  'simple_command_with_result_snake',
  'stateful_command_with_result_snake',
  'stateful_command_with_result',
  'async_simple_command_with_result',
  'future_simple_command_with_return',
  'future_simple_command_with_result',
  'async_stateful_command_with_result',
  'command_arguments_wild',
  'command_arguments_struct',
  'command_arguments_tuple_struct',
  'renamed_command',
  'renamed_command_new',
  'renamed_command_in_mod',
  'renamed_command_in_mod_new',
]

function commandExpected(name, supplied) {
  if (name === 'renamed_command') return 'Err(Command renamed_command not found)'
  if (name === 'renamed_command_in_mod') return 'Err(Command renamed_command_in_mod not found)'
  if (name === 'raw_request') return `Ok(${read('source/tauri/examples/commands/README.md').replace(/\r\n?/g, '\n')})`
  if (name === 'window_label' || name === 'command_arguments_wild') return 'Ok(null)'
  if (name === 'renamed_command_new' || name === 'renamed_command_in_mod_new') return 'Ok(null)'
  if (name === 'stateful_command' || name === 'async_stateful_command') return 'Ok(null)'
  if (name === 'command_arguments_struct' || name === 'command_arguments_tuple_struct') {
    return supplied
      ? 'Ok(null)'
      : missing(name, name === 'command_arguments_struct' ? 'person' : 'inlinePerson')
  }
  if (name === 'stateful_command_with_result' || name === 'stateful_command_with_result_snake') {
    return supplied ? 'Ok(value)' : 'Err(FooError)'
  }
  if (name === 'async_stateful_command_with_result') return supplied ? 'Ok(value)' : 'Ok()'
  if (
    name === 'simple_command' ||
    name === 'async_simple_command' ||
    name === 'async_simple_command_snake' ||
    name === 'future_simple_command' ||
    name === 'future_simple_command_snake'
  ) {
    return supplied ? 'Ok(null)' : missing(name, name.endsWith('snake') ? 'the_argument' : 'theArgument')
  }
  const key = name.endsWith('snake') ? 'the_argument' : 'theArgument'
  return supplied ? 'Ok(value)' : missing(name, key)
}

async function expectCommand(t, target, name) {
  await t.evaluate(
    target,
    `(() => {
      for (const selector of ['#response', '#response-optional']) {
        const element = document.querySelector(selector)
        if (element) element.textContent = ''
      }
    })()`,
  )
  // One click deliberately issues the two source-defined invokes: provided args
  // into #response and an empty object into #response-optional.
  await clickButton(t, target, `Run ${name}`)
  await t.wait(
    target,
    `({ response: document.querySelector('#response')?.innerText ?? '', optional: document.querySelector('#response-optional')?.innerText ?? '' })`,
    (value) => value.response !== '' && value.optional !== '',
    20000,
  )
  // The source writes innerText: its newlines are <br> nodes, absent from textContent.
  const result = await t.evaluate(target, `({ response: document.querySelector('#response').innerText, optional: document.querySelector('#response-optional').innerText })`)
  t.check(`${name} with args response`, result.response, commandExpected(name, true))
  t.check(`${name} without args response`, result.optional, commandExpected(name, false))
  if (name === 'raw_request') {
    const raw = await t.evaluate(target, `(async () => {
      const response = await window.__TAURI__.core.invoke('raw_request', { theArgument: 'value' })
      if (!(response instanceof ArrayBuffer)) throw new Error('raw_request must return an ArrayBuffer')
      return [...new Uint8Array(response)]
    })()`)
    t.check('raw_request exact binary bytes', raw, [...fs.readFileSync(path.join(root, 'source/tauri/examples/commands/README.md'))])
  }
}

// Probe through a real media request: the official CSP permits media-src, while
// connect-src correctly refuses page fetches to this cross-origin resource.
async function mediaRange(t, target, source, connect = observer) {
  const url = new URL(source)
  url.searchParams.set('__verify_range', String(Date.now()))
  let resolve, reject, timer, cdp
  const received = new Promise((ok, fail) => { resolve = ok; reject = fail })
  // Attach the rejection handler before launching a request that may fail early.
  received.catch(() => {})
  cdp = await connect(target, ({ method, params }) => {
    if (method !== 'Fetch.requestPaused') return
    const request = { requestId: params.requestId }
    const respond = async () => {
      if (params.responseErrorReason) throw new Error(`media range request failed: ${params.responseErrorReason}`)
      if (params.responseStatusCode !== undefined) {
        const body = await cdp.send('Fetch.getResponseBody', request)
        const headers = Object.fromEntries(params.responseHeaders.map(({ name, value }) => [name.toLowerCase(), value]))
        await cdp.send('Fetch.continueRequest', request)
        resolve({ status: params.responseStatusCode, contentRange: headers['content-range'],
          contentLength: headers['content-length'], bytes: [...Buffer.from(body.body, body.base64Encoded ? 'base64' : 'utf8')] })
      } else {
        const headers = Object.entries(params.request.headers).filter(([name]) => name.toLowerCase() !== 'range')
          .map(([name, value]) => ({ name, value }))
        await cdp.send('Fetch.continueRequest', { ...request, headers: [...headers, { name: 'Range', value: 'bytes=0-15' }] })
      }
    }
    respond().catch(reject)
  })
  try {
    await cdp.send('Fetch.enable', { patterns: ['Request', 'Response'].map(requestStage => ({ urlPattern: url.href, resourceType: 'Media', requestStage })) })
    timer = setTimeout(() => reject(new Error('media range response timed out')), 20000)
    await t.evaluate(target, `(() => {
      const video = document.createElement('video')
      video.id = '__verify_range_media'; video.preload = 'auto'; video.muted = true
      video.src = ${json(url.href)}; document.body.append(video); video.load()
    })()`)
    return await received
  } finally {
    clearTimeout(timer)
    try { await cdp.send('Fetch.disable') } finally { cdp.close() }
    await t.evaluate(target, `document.querySelector('#__verify_range_media')?.remove()`)
  }
}

export const cases = {
  api: verifyApi,
  drag: verifyDrag,
  isolation: verifyIsolation,
  multiwebview: verifyMultiwebview,
  async helloworld(t) {
    const target = t.main
    t.check('helloworld initial message', await text(t, target, '#message'), '')
    await setValue(t, target, '#name', 'Ferris')
    await click(t, target, '#form button')
    await t.wait(target, `document.querySelector('#message')?.textContent ?? ''`, (value) => value.length > 0)
    t.check('helloworld greeting', await text(t, target, '#message'), 'Hello Ferris, You have been greeted from Rust!')
    await t.capture(target, 'helloworld-greeting')
  },

  async commands(t) {
    const target = t.main
    await t.wait(target, 'document.querySelectorAll("#container button").length', (value) => value === 29)
    t.check('commands button count', await t.evaluate(target, 'document.querySelectorAll("#container button").length'), 29)
    await t.capture(target, 'commands-initial')

    for (const name of commandNames) {
      await expectCommand(t, target, name)
      if (['borrow_cmd', 'raw_request', 'stateful_command_with_result', 'renamed_command', 'renamed_command_new'].includes(name)) {
        await t.evaluate(target, 'scrollTo(0, 0)')
        await t.capture(target, `commands-${name}`)
      }
    }

    const extras = [
      ['borrow_cmd_async', { theArgument: 'value' }, 'value'],
      ['force_async', { theArgument: 'value' }, 'value'],
      ['force_async_with_result', { theArgument: 'value' }, 'value'],
      ['cmd', { argument: 'value' }, null],
      ['invoke', { argument: 'value' }, null],
      ['message', { argument: 'value' }, null],
      ['resolver', { argument: 'value' }, null],
    ]
    for (const [name, args, expected] of extras) {
      const result = await invoke(t, target, name, args)
      t.check(`${name} extra handler succeeds`, result, { ok: true, value: expected })
    }
    await t.capture(target, 'commands-final')
  },

  async state(t) {
    const target = t.main
    await t.wait(target, `document.querySelector('#counter')?.textContent`, (value) => value !== '')
    t.check('state initial count', await text(t, target, '#counter'), '0')
    await clickButton(t, target, 'Increment')
    await clickButton(t, target, 'Increment')
    await t.wait(target, `document.querySelector('#counter')?.textContent`, (value) => value === '2')
    t.check('state increments', await text(t, target, '#counter'), '2')
    await t.capture(target, 'state-two')

    await t.command(target, 'Page.reload', { ignoreCache: false })
    await t.wait(target, `document.querySelector('#counter')?.textContent`, (value) => value === '2')
    t.check('state survives frontend reload', await text(t, target, '#counter'), '2')
    await clickButton(t, target, 'Decrement')
    await clickButton(t, target, 'Decrement')
    await clickButton(t, target, 'Decrement')
    await t.wait(target, `document.querySelector('#counter')?.textContent`, (value) => value === '-1')
    t.check('state decrements below zero', await text(t, target, '#counter'), '-1')
    await clickButton(t, target, 'Reset')
    await t.wait(target, `document.querySelector('#counter')?.textContent`, (value) => value === '0')
    t.check('state reset', await text(t, target, '#counter'), '0')
    await t.capture(target, 'state-reset')
  },

  async multiwindow(t) {
    const main = await targetByLabel(t, 'Main')
    const secondary = await targetByLabel(t, 'Secondary')
    const third = await targetByLabel(t, 'Third')
    t.check('multiwindow initial labels', (await Promise.all([main, secondary, third].map((target) => labelOf(t, target)))).sort(), ['Main', 'Secondary', 'Third'])
    t.check('multiwindow initial page count', (await t.targets()).filter((target) => target.type === 'page').length, 3)
    const nativeTitles = () => t.native('state').filter(window => window.visible && window.clientWidth > 0 && window.clientHeight > 0).map(window => window.title).sort()
    const initialTitles = ['Tauri - Main', 'Tauri - Secondary', 'Tauri - Third']
    if (process.platform === 'win32') t.check('multiwindow native titles match configuration and Rust builder', nativeTitles(), initialTitles)
    else t.note('multiwindow native titles', 'Native title observation requires the Windows helper on this platform.')
    await t.capture(main, 'multiwindow-main-initial')
    await t.capture(secondary, 'multiwindow-secondary-initial')
    await t.capture(third, 'multiwindow-third-initial')

    const mainBefore = await text(t, main, '#messages-view')
    await sendMessage(t, main, 'Secondary', 'from Main')
    await t.wait(secondary, `document.querySelector('#messages-view')?.textContent ?? ''`, (value) => value.includes('from Main'))
    t.check('multiwindow targeted Secondary receives', (await text(t, secondary, '#messages-view')).includes('from Main'), true)
    t.check('multiwindow Main is not receiver', await text(t, main, '#messages-view'), mainBefore)

    await sendMessage(t, secondary, 'Main', 'from Secondary')
    await t.wait(main, `document.querySelector('#messages-view')?.textContent ?? ''`, (value) => value.includes('from Secondary'))
    t.check('multiwindow Main receives', (await text(t, main, '#messages-view')).includes('from Secondary'), true)
    t.check('multiwindow Third is not receiver', (await text(t, third, '#messages-view')).includes('from Secondary'), false)
    await t.capture(secondary, 'multiwindow-secondary-received')
    await t.capture(main, 'multiwindow-main-received')

    await setValue(t, main, '#new-label', 'Fourth')
    await setValue(t, main, '#new-title', 'QA Fourth')
    await click(t, main, '#new-window-form button')
    const fourth = await targetByLabel(t, 'Fourth')
    await t.wait(fourth, `document.querySelector('#messages-view')?.textContent ?? ''`, (value) => typeof value === 'string')
    await sendMessage(t, main, 'Fourth', 'from Main to Fourth')
    await t.wait(fourth, `document.querySelector('#messages-view')?.textContent ?? ''`, (value) => value.includes('from Main to Fourth'))
    t.check('multiwindow Fourth receives', (await text(t, fourth, '#messages-view')).includes('from Main to Fourth'), true)
    if (process.platform === 'win32') {
      t.check('multiwindow Fourth applies form title without changing sibling titles', nativeTitles(), [...initialTitles, 'QA Fourth'].sort())
      t.native('capture', { Title: 'QA Fourth', OutputPath: path.join(t.dir, 'multiwindow-fourth-native-title.png') })
    }
    await t.capture(fourth, 'multiwindow-fourth-received')

    await t.command(fourth, 'Page.close', {})
    await t.target(async (candidate) => {
      if (candidate.id !== main.id) return false
      const remaining = (await t.targets()).filter((target) => target.type === 'page')
      return remaining.length === 3 && !remaining.some((target) => target.id === fourth.id)
    }, 15000)
    t.check('multiwindow Fourth target closes', (await t.targets()).some((target) => target.id === fourth.id), false)
    if (process.platform === 'win32') t.check('multiwindow closing Fourth preserves native sibling windows', nativeTitles(), initialTitles)
  },

  async resources(t) {
    const target = t.main
    const expected = read('source/tauri/examples/resources/src-tauri/assets/index.js')
    await t.wait(target, `document.querySelector('#content')?.textContent ?? ''`, (value) => value.includes("console.log('hello world')"))
    const actualPath = await text(t, target, '#path')
    const actualContent = await text(t, target, '#content')
    t.check('resources path resolves to asset', actualPath.split('\\').join('/').endsWith('/assets/index.js'), true)
    t.check('resources exact asset content', actualContent, expected)
    t.check('resources content is nonempty', actualContent.length > 0, true)
    await t.capture(target, 'resources-read')
    t.note('resources packaged install', 'Development verification only; packaged resource staging/installation requires the build runner.')
  },

  async ['run-return'](t) {
    const target = t.main
    await t.wait(target, `document.body.innerText.trim()`, (value) => value === 'Welcome to Tauri!')
    await t.capture(target, 'run-return-before-close')
    await t.command(target, 'Page.close', {})
    const code = await t.closed()
    t.check('run-return exit code', code, 0)
    t.check('run-return resumes after event loop', t.output().includes('I run after exit'), true)
  },

  async splashscreen(t) {
    // The driver must enter this case before its generic initial screenshot:
    // the source's main document closes the splash 2000 ms after DOMContentLoaded.
    const splash = await t.target((target) => target.type === 'page' && target.url.includes('splashscreen.html'), 20000)
    await t.capture(splash, 'splashscreen-early')
    // The driver starts this read-only watcher before launch; cold PowerShell
    // compilation here would consume the source's two-second display interval.
    const before = await t.splashBefore()
    fs.writeFileSync(path.join(t.dir, 'splashscreen-native-before.json'), JSON.stringify(before, null, 2) + '\n')
    const splashWindow = before.find((window) => window.visible && window.clientWidth > 0 && window.clientHeight > 0)
    const mainWindow = before.find((window) => !window.visible && window.clientWidth > 0 && window.clientHeight > 0)
    t.check('splashscreen initially has one visible native window', before.filter((window) => window.visible).length, 1)
    t.check('splashscreen main native window starts hidden', Boolean(mainWindow), true)
    assert(splashWindow, 'splashscreen must be visible before its source timer fires')
    const main = await targetByLabel(t, 'main')
    await t.wait(main, `document.querySelector('h1')?.textContent ?? ''`, (value) => value === 'This is the main window!', 10000)
    t.check('splashscreen main content', await text(t, main, 'h1'), 'This is the main window!')
    await t.target(async (candidate) => candidate.id === main.id && !(await t.targets()).some((item) => item.id === splash.id), 10000)
    t.check('splashscreen source timer destroys splash target', (await t.targets()).some((target) => target.id === splash.id), false)
    const after = await t.native('state')
    fs.writeFileSync(path.join(t.dir, 'splashscreen-native-after.json'), JSON.stringify(after, null, 2) + '\n')
    const visible = after.filter((window) => window.visible && window.clientWidth > 0 && window.clientHeight > 0)
    t.check('splashscreen transition leaves one visible native main window', visible.length, 1)
    t.check('splashscreen source timer destroys original native splash', after.some((window) => window.handle === splashWindow.handle), false)
    t.check('splashscreen source timer shows original main native window', after.find((window) => window.handle === mainWindow.handle)?.visible, true)
    t.main = main
    await t.capture(main, 'splashscreen-main')
    await t.native('capture', { OutputPath: path.join(t.dir, 'splashscreen-main-native.png') })
  },

  async ['file-associations'](t) {
    const target = t.main
    const fixtures = t.fileFixtures
    assert(Array.isArray(fixtures) && fixtures.length === 2, 'file association fixtures must contain exactly two paths')
    await t.wait(target, 'Array.isArray(window.openedFiles)', (value) => value === true)
    // The source accepts file: URLs and converts them back to native paths;
    // drive-letter argv is parsed as an unsupported URL scheme upstream.
    t.check('file associations file URL argv resolves to native fixture paths', await t.evaluate(target, 'window.openedFiles'), fixtures)
    t.check('file associations rendered paths', await text(t, target, '#files'), fixtures.join('\n'))
    await t.capture(target, 'file-associations-fixtures')
    t.note('file associations OS registration', 'Direct executable-argument verification only; packaged/native Open-with registration is untested.')
  },

  async streaming(t) {
    const target = t.main
    const video = await t.wait(
      target,
      `(async () => {
        const element = document.querySelector('#video_source')
        const state = () => ({
          url: location.href, src: element?.getAttribute('src'),
          sources: [...(element?.querySelectorAll('source') ?? [])].map(source => source.src),
          currentSrc: element?.currentSrc, duration: element?.duration,
          readyState: element?.readyState, networkState: element?.networkState,
          decodedFrames: element?.getVideoPlaybackQuality?.().totalVideoFrames ?? element?.webkitDecodedFrameCount,
          canPlayType: element?.canPlayType('video/mp4; codecs="avc1.42E01E, mp4a.40.2"'),
          error: element?.error && { code: element.error.code, message: element.error.message },
          resources: performance.getEntriesByType('resource').filter(entry => entry.initiatorType === 'video' || entry.name.startsWith('stream:')).map(entry => ({ url: entry.name, status: entry.responseStatus, duration: entry.duration, bytes: entry.transferSize })),
        })
        if (element?.error) throw new Error('streaming media failed: ' + JSON.stringify(state()))
        if (!element || !element.currentSrc || !Number.isFinite(element.duration) || element.duration <= 0 || element.readyState < 2) return state()
        element.muted = true
        let frames = 0
        const frame = () => { frames += 1; if (frames < 3 && element.requestVideoFrameCallback) element.requestVideoFrameCallback(frame) }
        if (element.requestVideoFrameCallback) element.requestVideoFrameCallback(frame)
        const before = element.currentTime
        try { await element.play() }
        catch (error) { throw new Error('streaming play failed: ' + error.message + '; ' + JSON.stringify(state())) }
        const deadline = performance.now() + 10000
        while (performance.now() < deadline && (element.currentTime <= before || frames < 1)) {
          if (element.error) throw new Error('streaming media failed: ' + JSON.stringify(state()))
          await new Promise((resolve) => setTimeout(resolve, 50))
        }
        if (element.error) throw new Error('streaming media failed: ' + JSON.stringify(state()))
        return { ...state(), src: element.currentSrc, before, after: element.currentTime, frames }
      })()`,
      (value) => value && value.duration > 0 && value.after > value.before && value.frames > 0,
      90000,
    )
    t.check('streaming duration is positive', video.duration > 0, true)
    t.check('streaming playback currentTime increases', video.after > video.before, true)
    t.check('streaming has decoded video frames', video.frames > 0, true)

    const seek = await t.evaluate(
      target,
      `(async () => {
        const element = document.querySelector('#video_source')
        const targetTime = Math.min(30, element.duration / 2)
        element.currentTime = targetTime
        await new Promise((resolve, reject) => {
          const timer = setTimeout(() => reject(new Error('seek timed out')), 10000)
          element.addEventListener('seeked', () => { clearTimeout(timer); resolve() }, { once: true })
        })
        await new Promise((resolve, reject) => {
          const timer = setTimeout(() => reject(new Error('seeked frame was not rendered')), 10000)
          element.requestVideoFrameCallback(() => { clearTimeout(timer); element.pause(); resolve() })
        })
        await new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))
        return { requested: targetTime, actual: element.currentTime }
      })()`,
      30000,
    )
    t.check('streaming seek reaches requested time', Math.abs(seek.actual - seek.requested) < 0.25, true)

    const videoPath = path.join(t.root, '.cache', 'examples', t.name, 'streaming_example_test_video.mp4')
    assert(fs.existsSync(videoPath), `streaming source file is missing: ${videoPath}`)
    const bytes = fs.readFileSync(videoPath)
    await t.capture(target, 'streaming-seeked')
    const range = await mediaRange(t, target, video.src)
    fs.writeFileSync(path.join(t.dir, 'streaming-range.json'), JSON.stringify(range, null, 2))
    t.check('streaming range status', range.status, 206)
    t.check('streaming range content-length', range.contentLength, '16')
    t.check('streaming range content-range', range.contentRange, `bytes 0-15/${bytes.length}`)
    t.check('streaming range bytes', range.bytes, [...bytes.subarray(0, 16)])
    await t.capture(target, 'streaming-playing')
  },
}

// Offline verifier contract guard only; this does not verify any desktop app.
if (process.argv.includes('--self-check') && process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  let paused, rangeUrl, rangeHeaders, rangeClosed = false, rangeRemoved = false
  const range = await mediaRange({ async evaluate(_target, expression) {
    if (expression.includes("?.remove()")) { rangeRemoved = true; return }
    paused({ method: 'Fetch.requestPaused', params: { requestId: 'range', request: { headers: { Range: 'bytes=0-', Accept: '*/*' } } } })
  } }, {}, 'http://stream.localhost/video.mp4', async (_target, onEvent) => {
    paused = onEvent
    return { close() { rangeClosed = true }, async send(method, params) {
      if (method === 'Fetch.enable') { rangeUrl = params.patterns[0].urlPattern; assert.deepEqual(params.patterns.map(p => p.requestStage), ['Request', 'Response']) }
      if (method === 'Fetch.continueRequest' && params.headers) {
        rangeHeaders = params.headers
        paused({ method: 'Fetch.requestPaused', params: { requestId: 'range', responseStatusCode: 206, responseHeaders: [
          { name: 'Content-Range', value: 'bytes 0-15/123' }, { name: 'Content-Length', value: '16' },
        ] } })
      }
      if (method === 'Fetch.getResponseBody') return { body: Buffer.from(Array.from({ length: 16 }, (_, i) => i)).toString('base64'), base64Encoded: true }
    } }
  })
  assert.equal(new URL(rangeUrl).pathname, '/video.mp4')
  assert.deepEqual(rangeHeaders, [{ name: 'Accept', value: '*/*' }, { name: 'Range', value: 'bytes=0-15' }])
  assert.deepEqual(range, { status: 206, contentRange: 'bytes 0-15/123', contentLength: '16', bytes: Array.from({ length: 16 }, (_, i) => i) })
  assert(rangeClosed && rangeRemoved, 'range probe must release CDP and temporary media')
  const main = { id: 'main', type: 'page' }, splash = { id: 'splash', type: 'page', url: 'tauri://localhost/splashscreen.html' }
  const steps = []
  let closed = false
  const t = {
    dir: root,
    main: splash,
    check: (label, actual, expected) => assert.deepEqual(actual, expected, label),
    target: async (predicate) => { for (const target of [splash, main]) if (await predicate(target)) return target; throw new Error('missing test target') },
    targets: async () => { closed = true; return [main] },
    evaluate: async (target, expression) => expression.includes('currentWindow') ? target.id : 'This is the main window!',
    wait: async () => {},
    capture: async (_target, label) => { steps.push(label) },
    splashBefore: async () => { steps.push('before'); return [{ handle: 1, visible: true, clientWidth: 400, clientHeight: 200 }, { handle: 2, visible: false, clientWidth: 800, clientHeight: 600 }] },
    native: async (action) => { assert(closed, 'native helper must not delay the early keyframe'); return action === 'state' ? [{ handle: 2, visible: true, clientWidth: 800, clientHeight: 600 }] : undefined },
  }
  // Keep the contract guard read-only even though real cases retain JSON evidence.
  const originalWrite = fs.writeFileSync
  try {
    fs.writeFileSync = () => {}
    await cases.splashscreen(t)
    assert.equal(t.main, main, 'later menu checks must use the surviving main target')
    assert.deepEqual(steps, ['splashscreen-early', 'before', 'splashscreen-main'])
    t.native = async () => [{ handle: 1, visible: true, clientWidth: 400, clientHeight: 200 }]
    await assert.rejects(cases.splashscreen(t), /splashscreen/)
    const fixtures = ['D:\\fixture dir\\hello world.txt', 'D:\\fixture dir\\文件.txt']
    const files = { main, fileFixtures: fixtures, wait: async () => {}, check: t.check, capture: async () => {}, note: () => {},
      evaluate: async (_target, expression) => expression === 'window.openedFiles' ? fixtures : fixtures.join('\n') }
    await cases['file-associations'](files)
    files.evaluate = async (_target, expression) => expression === 'window.openedFiles' ? [] : ''
    await assert.rejects(cases['file-associations'](files), /native fixture paths/)
  } finally { fs.writeFileSync = originalWrite }
  console.log('Verifier contract self-check passed (no apps launched; no UI verification performed)')
}
