import fs from 'node:fs'
import path from 'node:path'
import { app, ipcMain, Menu, nativeTheme, screen, session } from 'electron'
import { IPC } from './socket/ipc'
import { Transport } from './socket/transport'
import { createCommands } from './runtime/commands'
import { Downloads } from './runtime/downloads'
import { Environment } from './runtime/environment'
import { Executions } from './runtime/execution'
import { Factory } from './runtime/factory'
import { Policies } from './runtime/policy'
import type { PopupPlan } from './runtime/policy'
import { Protocols } from './runtime/protocol'
import { Webviews } from './runtime/webview'
import { Windows } from './runtime/window'
import { arg, noop } from './utils'
import type { WebContents } from 'electron'

/**
 * 组合根：只描述「谁依赖谁」和「按什么顺序建」，不含任何行为。
 *
 * 三条数据流，全部经过 transport：
 *   Rust --命令--> transport --查表--> commands --调用--> Window / Webview / Policies
 *   Rust --事件--> Policies --transport.request()--> transport（阻塞等 Rust 决策）
 *   领域对象 --事件--> factory 订阅后 --> transport.send --> Rust
 *
 * 想改行为去对应文件；这里能改的只有依赖图本身。
 */

const appName = process.env.ELECTRON_RS_APP_NAME
if (appName) app.setName(appName)
const owner = process.env.TAURI_RUNTIME_ELECTRON_OWNER
if (owner) {
  const directory = path.join(app.getPath('appData'), owner)
  fs.mkdirSync(directory, { recursive: true })
  app.setPath('userData', directory)
}

// ── 进程输入 ───────────────────────────────────────────────────────────────
// argv + 继承的 stdin（一次性凭据）。全仓只有这里读 process。
const port = Number(arg('--port'))
if (!port) {
  console.error('missing --port')
  process.exit(1)
}

// 阻塞读 fd 0：Rust spawn 后立刻写入，之后不再写。
const credential = fs.readFileSync(0, 'utf8').trim()
if (!/^[a-f0-9]{64}$/.test(credential)) throw new Error('missing Electron IPC credential')

// Tauri 应用标识：任务栏、通知、单实例都靠它认人。
const identifier = arg('--tauri-identifier') ?? process.env.ELECTRON_RS_IDENTIFIER

// 自定义 scheme（tauri://、插件协议）：逗号分隔，空项丢掉。
const schemes = (arg('--tauri-schemes') ?? '')
  .split(',')
  .map((scheme) => scheme.trim())
  .filter(Boolean)

// 必须在 app ready 之前注册，否则首次导航会被当成外部 URL。
Protocols.registerPrivileged(schemes)

// ── 运行期注册表 ───────────────────────────────────────────────────────────
// 生命周期与进程一致。取代 main.cjs 里散在 state 上的多张 Map / WeakMap。
const windows = new Windows()
const webviews = new Webviews()
const downloads = new Downloads()
/** 阻塞期间由 Rust 嵌套创建、等待 attach 的 popup：factory 写入，policy 取出。 */
const popups = new Map<number, PopupPlan>()

// ── 基础设施 ───────────────────────────────────────────────────────────────
// IPC 只是字节通道：Worker（同文件自举）+ socket，与业务无关。
const ipc = new IPC(port)

// transport 才负责「请求 → 查表 → 回包」，是唯一能被替身替换的接缝。
const transport = new Transport(ipc)
const executions = new Executions(transport)

// Window 的原生接缝：appkit / 光标 / 主题。只有这里知道 native 适配器。
const environment = new Environment(webviews)

// ── 领域对象 ───────────────────────────────────────────────────────────────
// 策略：唯一允许阻塞等 Rust 决策的地方，桥就是 transport。
const policies = new Policies({ transport, downloads, webviews, windows, popups })

// 协议：注册 tauri:// 与插件协议；自定义 session 需要同时挂协议和策略。
const protocols = new Protocols({ transport, policies })

// 工厂：建窗口 / 页面，需要上面全部对象，因此放在最后。
const factory = new Factory({
  windows,
  webviews,
  policies,
  protocols,
  transport,
  environment,
  popups,
})

// ── 命令表 ─────────────────────────────────────────────────────────────────
// 命令只是 adapter：解析 id → 取领域对象 → 调一个方法。
const commands = createCommands({
  factory,
  executions,
  windows,
  webviews,
  policies,
  protocols,
  app,
  nativeTheme,
  display: { screen, platform: process.platform },
  respond: (id, response) => transport.respond(id, response),
})

// ── 组装 ───────────────────────────────────────────────────────────────────
// commands 依赖 transport（respond），transport 又需要命令表，所以表建好后再注入。
transport.use(commands)

export { windows, webviews, downloads, popups, transport, environment, policies, protocols, factory, commands, executions }

// IPC identity is derived from Electron's sender, never supplied by the renderer.
ipcMain.handle('tauri-ipc-fetch', async (event, data) => {
  const frame = event.senderFrame
  const webview = webviews.forContents(event.sender.id)
  if (!frame || frame !== event.sender.mainFrame || frame.origin === 'null' || !webview) throw new Error('unknown IPC sender')
  if (!data || typeof data.url !== 'string' || data.method !== 'POST' || !(data.body instanceof Uint8Array)) throw new Error('invalid IPC request')
  const url = new URL(data.url)
  if (url.username || url.password || url.port || !((url.protocol === 'ipc:' && url.hostname === 'localhost') || (/^https?:$/.test(url.protocol) && url.hostname === 'ipc.localhost'))) throw new Error('invalid IPC request')
  const headers = new Headers(data.headers)
  headers.set('Origin', frame.origin)
  const response = await protocols.fetchIpc(new Request(url, { method: 'POST', headers, body: data.body }), webview.id)
  return { status: response.status, headers: [...response.headers], body: new Uint8Array(await response.arrayBuffer()) }
})

ipcMain.on('tauri-ipc', (event, data) => {
  const frame = event.senderFrame
  if (!frame || frame !== event.sender.mainFrame || frame.origin === 'null' || typeof data !== 'string') return
  const webview = webviews.forContents(event.sender.id)
  if (!webview) return
  transport.send('ipc.message', { webviewId: webview.id, url: frame.url, body: data })
})

// 下载终态没有对应的命令回包，只能当事件发出去。
downloads.on('finished', (payload) => transport.send('download.finished', payload))

// ── 生命周期 ───────────────────────────────────────────────────────────────
// 每一条都对应一个所有权判断，所以集中在这里。
const ready = app.whenReady().then(() => {
  if (identifier) app.setAppUserModelId(identifier)
  // Tauri 应用自绘窗口装饰；Electron 默认菜单只会造成干扰。
  Menu.setApplicationMenu(null)
  policies.install(session.defaultSession)
})

// 握手后 Rust 即可调用原生 API，必须先完成 Electron 初始化。
transport.onConnect(async () => {
  await ready
  transport.send('ready', { electron: process.versions.electron, token: credential })
})

// 连接断开即退出：窗口寿命归 Rust，socket 断了就没有继续存在的理由。
transport.onClose(() => app.quit())
app.on('will-quit', () => { void executions.close() })

// 没有窗口时继续运行，直到 socket 断开。
app.on('window-all-closed', noop)

// 退出同样要问 Rust：只有连接已断才真正退出，否则视为请求。
app.on('before-quit', (event) => {
  if (transport.closed) return
  event.preventDefault()
  transport.send('app.exitRequested', {})
})

// macOS dock 激活：转给 Rust，由它决定是否建窗。
app.on('activate', (_event, hasVisibleWindows) => {
  transport.send('app.reopen', { hasVisibleWindows })
})
