import type { event as tauriEvent, window as tauriWindow } from '@tauri-apps/api'

export type CheckStatus = 'pass' | 'fail' | 'unknown' | 'na' | 'skip'
export type CheckResult = { id: string, status: CheckStatus, detail: string }
export type Check = { ids: string[], run: () => Promise<string | void> }

type BooleanGetter = 'isResizable' | 'isDecorated' | 'isEnabled' | 'isVisible' | 'isMaximized' | 'isMinimized' | 'isFullscreen' | 'isFocused' | 'isAlwaysOnTop'
type BooleanSetter = 'setResizable' | 'setDecorations' | 'setEnabled' | 'setAlwaysOnTop' | 'setFullscreen'
type QueryMethod = 'title' | 'scaleFactor' | 'innerSize' | 'outerSize' | 'innerPosition' | 'outerPosition' | BooleanGetter

const sleep = (ms: number) => new Promise<void>(resolve => setTimeout(resolve, ms))
const assert = (condition: unknown, detail: string) => { if (!condition) throw new Error(detail) }

export async function eventually<T>(read: () => Promise<T>, expected: (actual: T) => boolean): Promise<T> {
  const deadline = Date.now() + 4000
  let actual: T
  do {
    actual = await read()
    if (expected(actual)) return actual
    await sleep(80)
  } while (Date.now() < deadline)
  throw new Error(`读回结果不符合预期：${JSON.stringify(actual)}`)
}

export async function runChecks(checks: Check[], report: (result: CheckResult) => void): Promise<void> {
  for (const { ids, run } of checks) {
    try {
      const detail = await run()
      for (const id of ids) report({ id, status: 'pass', detail: detail ?? '实际调用并验证读回结果' })
    } catch (error) {
      for (const id of ids) report({ id, status: 'fail', detail: String(error) })
    }
  }
}

export function createChecks(tauri: typeof import('@tauri-apps/api')): Check[] {
  const { window: api, webviewWindow, dpi, event, app } = tauri
  let window: tauriWindow.Window
  const check = (ids: string, run: Check['run']): Check => ({ ids: ids.split(' '), run })
  const query = <Method extends QueryMethod>(name: string, method: Method, valid: (value: Awaited<ReturnType<tauriWindow.Window[Method]>>) => boolean) => check(`WindowDispatch::${name}`, async () => {
    const actual = await window[method]() as Awaited<ReturnType<tauriWindow.Window[Method]>>
    assert(valid(actual), `${method}: ${JSON.stringify(actual)}`)
    return JSON.stringify(actual)
  })
  const toggle = (name: string, setter: BooleanSetter, getter: BooleanGetter) => check(`WindowDispatch::${name}`, async () => {
    const original = await window[getter]()
    try {
      await window[setter](!original)
      await eventually(() => window[getter](), value => value === !original)
    } finally {
      await window[setter](original)
      await eventually(() => window[getter](), value => value === original)
    }
  })
  const booleanGetters: BooleanGetter[] = ['isResizable', 'isDecorated', 'isEnabled', 'isVisible', 'isMaximized', 'isMinimized', 'isFullscreen', 'isFocused', 'isAlwaysOnTop']
  const toggles: [string, BooleanSetter, BooleanGetter][] = [
    ['set_resizable', 'setResizable', 'isResizable'],
    ['set_decorations', 'setDecorations', 'isDecorated'],
    ['set_enabled', 'setEnabled', 'isEnabled'],
    ['set_always_on_top', 'setAlwaysOnTop', 'isAlwaysOnTop'],
    ['set_fullscreen', 'setFullscreen', 'isFullscreen'],
  ]
  return [
    check('WindowBuilder::with_config', async () => {
      const probe = new webviewWindow.WebviewWindow('probe', {
        url: 'index.html?probe=1', title: 'API probe', width: 520, height: 360,
        resizable: true, decorations: true, visible: true, focus: false,
      })
      await new Promise<void>((resolve, reject) => {
        const timeout = setTimeout(() => reject(new Error('创建测试窗口超时')), 15000)
        const finish = <T>(callback: (value: T) => void) => (value: T) => { clearTimeout(timeout); callback(value) }
        probe.once('tauri://created', finish(() => resolve())).catch(finish(reject))
        probe.once('tauri://error', finish((error: tauriEvent.Event<unknown>) => reject(new Error(JSON.stringify(error.payload))))).catch(finish(reject))
      })
      const existingWindow: tauriWindow.WindowOptions & { skip: boolean } = { skip: true }
      window = new api.Window('probe', existingWindow)
      assert(await window.title() === 'API probe', '创建标题不匹配')
      assert(await window.isResizable() && await window.isDecorated() && await window.isVisible(), '创建属性不匹配')
      const size = await window.innerSize(), scale = await window.scaleFactor()
      assert(Math.abs(size.width / scale - 520) < 3 && Math.abs(size.height / scale - 360) < 3, '创建尺寸不匹配')
      return '创建独立测试窗口；标题、内容尺寸、可见性、边框与 resizable 均已读回'
    }),
    query('title', 'title', value => value === 'API probe'),
    query('scale_factor', 'scaleFactor', value => Number.isFinite(value) && value > 0),
    query('inner_size', 'innerSize', value => value.width > 0 && value.height > 0),
    query('outer_size', 'outerSize', value => value.width > 0 && value.height > 0),
    query('inner_position', 'innerPosition', value => Number.isFinite(value.x) && Number.isFinite(value.y)),
    query('outer_position', 'outerPosition', value => Number.isFinite(value.x) && Number.isFinite(value.y)),
    ...booleanGetters.map(method => query(method.replace(/[A-Z]/g, c => `_${c.toLowerCase()}`), method, value => typeof value === 'boolean')),
    check('WindowDispatch::set_title', async () => {
      try {
        await window.setTitle('title readback')
        await eventually(() => window.title(), value => value === 'title readback')
      } finally { await window.setTitle('API probe') }
    }),
    ...toggles.map(args => toggle(...args)),
    check('WindowDispatch::set_size', async () => {
      const original = await window.innerSize()
      try {
        await window.setSize(new dpi.LogicalSize(580, 400))
        const scale = await window.scaleFactor()
        await eventually(() => window.innerSize(), size => Math.abs(size.width / scale - 580) < 3 && Math.abs(size.height / scale - 400) < 3)
      } finally { await window.setSize(original) }
    }),
    check('WindowDispatch::set_position', async () => {
      const original = await window.outerPosition()
      try {
        const position = new dpi.PhysicalPosition(original.x + 30, original.y + 30)
        await window.setPosition(position)
        await eventually(() => window.outerPosition(), actual => Math.abs(actual.x - position.x) < 3 && Math.abs(actual.y - position.y) < 3)
      } finally { await window.setPosition(original) }
    }),
    check('WindowDispatch::hide WindowDispatch::show', async () => {
      try {
        await window.hide()
        await eventually(() => window.isVisible(), value => value === false)
      } finally {
        await window.show()
        await eventually(() => window.isVisible(), value => value === true)
      }
    }),
    check('WindowDispatch::maximize WindowDispatch::unmaximize', async () => {
      try {
        await window.maximize()
        await eventually(() => window.isMaximized(), value => value === true)
      } finally {
        await window.unmaximize()
        await eventually(() => window.isMaximized(), value => value === false)
      }
    }),
    check('WindowDispatch::minimize WindowDispatch::unminimize', async () => {
      try {
        await window.minimize()
        await eventually(() => window.isMinimized(), value => value === true)
      } finally {
        await window.unminimize()
        await eventually(() => window.isMinimized(), value => value === false)
      }
    }),
    check('WindowDispatch::on_window_event', async () => {
      let observed = false
      const unlisten = await window.onResized(() => { observed = true })
      const original = await window.innerSize()
      try {
        await window.setSize(new dpi.PhysicalSize(original.width + 20, original.height + 20))
        await eventually(async () => observed, Boolean)
      } finally { unlisten(); await window.setSize(original) }
      return '实际调整测试窗口尺寸，收到原生 resize 事件'
    }),
    // These are useful app checks, but not substitutes for runtime trait evidence.
    check('app::ipc', async () => {
      assert((await app.getName()).length > 0, 'IPC 未返回应用名称')
      return '通过官方 Tauri IPC 读取应用名称'
    }),
    check('app::event', async () => {
      let received: unknown
      const unlisten = await event.listen('acceptance-event', message => { received = message.payload })
      try {
        await event.emit('acceptance-event', 'roundtrip')
        await eventually(async () => received, value => value === 'roundtrip')
      } finally { unlisten() }
      return 'Rust 事件桥往返 payload 已读回'
    }),
    check('WindowDispatch::close', async () => {
      await window.close()
      await eventually(() => api.getAllWindows(), windows => !windows.some(item => item.label === 'probe'))
      return '测试窗口已关闭，并从窗口列表移除'
    }),
  ]
}
