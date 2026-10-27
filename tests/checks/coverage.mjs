// Callable desktop API inventory. Only exact-commit real CI assertions count.
//   node tests/checks/coverage.mjs --write|--check  deterministic documentation
//   node tests/checks/coverage.mjs --md --evidence DIR --commit SHA --run ID/ATTEMPT
//   node tests/checks/coverage.mjs --evidence DIR --commit SHA --run ID/ATTEMPT --require-platforms --min-percent 80
import assert from 'node:assert/strict'
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { PLATFORMS, aggregateEvidence, cfgApplies, readArtifacts, traitMembers } from './runtime-evidence.mjs'
import { requireSuites } from './runtime-suites.mjs'

const repo = (path) => fileURLToPath(new URL(`../../${path}`, import.meta.url))
const TAURI = 'source/tauri/crates'
const RUNTIME_LIB = `${TAURI}/tauri-runtime/src/lib.rs`
const WRY_LIB = `${TAURI}/tauri-runtime-wry/src/lib.rs`
const DOC = 'docs/coverage.md'

const targets = [
  { trait: 'WindowDispatch', file: 'src/window.rs' },
  { trait: 'WebviewDispatch', file: 'src/webview.rs' },
  { trait: 'Runtime', file: 'src/runtime.rs' },
  { trait: 'RuntimeHandle', file: 'src/window_handle.rs' },
  { trait: 'EventLoopProxy', file: 'src/runtime.rs' },
  { trait: 'WindowBuilder', file: 'src/window_builder.rs', declaration: `${TAURI}/tauri-runtime/src/window.rs` },
]

const read = (path) => {
  try {
    return readFileSync(repo(path), 'utf8').replace(/\r\n/g, '\n')
  } catch {
    console.error(`missing ${path}; run: git submodule update --init`)
    process.exit(2)
  }
}

/** Source links only: never infer verification from an implementation body. */
export function definitions(src, trait) {
  const lines = src.split('\n')
  const start = lines.findIndex(line => new RegExp(`^impl.*\\b${trait}(?:<T>)? for `).test(line))
  assert.ok(start >= 0, `impl ${trait} not found`)
  const result = new Map()
  for (let i = start + 1; i < lines.length && !/^}/.test(lines[i]); i += 1) {
    const match = lines[i].match(/^ {2}(?:unsafe )?fn ([a-z_0-9]+)/)
    if (match && !result.has(match[1])) result.set(match[1], { line: i + 1 })
  }
  return result
}

// N/A is an upstream contract restriction, never an implementation's missing path.
const platformLimits = {
  'WindowDispatch::is_maximizable': { linux: '官方契约：Linux 不支持最大化按钮查询' },
  'WindowDispatch::is_minimizable': { linux: '官方契约：Linux 不支持最小化按钮查询' },
  'WindowDispatch::set_maximizable': { linux: '官方契约：Linux 不支持设置最大化按钮' },
  'WindowDispatch::set_minimizable': { linux: '官方契约：Linux 不支持设置最小化按钮' },
  'WindowDispatch::set_badge_count': { windows: '官方契约：Windows 不支持数字角标' },
  'WindowDispatch::set_badge_label': { windows: '官方契约：仅 macOS', linux: '官方契约：仅 macOS' },
  'WindowDispatch::set_overlay_icon': { macos: '官方契约：仅 Windows', linux: '官方契约：仅 Windows' },
  'WindowDispatch::set_title_bar_style': { windows: '官方契约：仅 macOS', linux: '官方契约：仅 macOS' },
  'WindowDispatch::set_traffic_light_position': { windows: '官方契约：仅 macOS', linux: '官方契约：仅 macOS' },
  'WindowBuilder::maximizable': { linux: '官方契约：Linux 不支持设置最大化按钮' },
  'WindowBuilder::minimizable': { linux: '官方契约：Linux 不支持设置最小化按钮' },
  'WindowBuilder::shadow': { linux: '官方契约：Linux 不支持窗口阴影' },
  'WindowBuilder::window_classname': { macos: '官方契约：仅 Windows', linux: '官方契约：仅 Windows' },
  'WindowBuilder::no_redirection_bitmap': { macos: '官方契约：仅 Windows', linux: '官方契约：仅 Windows' },
  ...Object.fromEntries(['Runtime', 'RuntimeHandle'].map(trait => [`${trait}::set_device_event_filter`, {
    macos: '官方契约：macOS 不支持设备事件过滤', linux: '官方契约：Linux 不支持设备事件过滤',
  }])),
}
const referenceSource = read(WRY_LIB)
const excluded = []
const rows = targets.map(target => {
  const mine = definitions(read(target.file), target.trait)
  const wry = definitions(referenceSource, target.trait)
  const declaration = target.declaration ?? RUNTIME_LIB
  return {
    ...target,
    members: traitMembers(read(declaration), target.trait).flatMap(member => {
      const id = `${target.trait}::${member.name}`
      const na = Object.fromEntries(PLATFORMS.flatMap(platform => {
        const applicable = member.declarations.some(item => item.cfg.every(cfg => cfgApplies(cfg, platform)))
        const reason = applicable ? platformLimits[id]?.[platform] : '上游 cfg 不在此平台提供'
        return reason ? [[platform, reason]] : []
      }))
      if (PLATFORMS.every(platform => na[platform])) {
        excluded.push(id)
        return []
      }
      return [{ ...member, id, na, scope: 'runtime',
        line: mine.get(member.name)?.line ?? null,
        declaration, declarationLine: member.declarations[0].line,
        wryLine: wry.get(member.name)?.line ?? null,
      }]
    }),
  }
})
const extensions = [
  ['ElectronWebviewDispatcher::on_new_window', '注册 Chromium 新窗口的 Allow/Create/Deny 回调', 'src/new_window.rs', '  pub fn on_new_window<F>'],
  ['tauri_runtime_electron::with_opener', '在原生弹窗请求上下文中构建目标窗口', 'src/new_window.rs', 'pub fn with_opener<R>'],
  ['tauri_runtime_electron::on_new_window', '在普通 Tauri webview 上注册 Electron 新窗口回调', 'src/new_window.rs', 'pub fn on_new_window<F>'],
  ['NewWindowFeatures::opener', '取得当前 Chromium 弹窗请求的一次性 opener', 'src/new_window.rs', '  pub fn opener(&self)'],
  ['NewWindowResponse::create', '将普通 Tauri 新窗口选作原生弹窗目标', 'src/new_window.rs', '  pub fn create(window:'],
].map(([id, purpose, file, anchor]) => ({ id, purpose, file, anchor, na: {}, scope: 'extension' }))
export const inventory = [...rows.flatMap(row => row.members), ...extensions]

const DESCRIPTION = {
  send_event: '将自定义用户事件送入运行时事件循环',
  with_config: '按窗口配置初始化构建器',
  min_inner_size: '设置窗口创建时的最小内容区尺寸',
  max_inner_size: '设置窗口创建时的最大内容区尺寸',
  inner_size_constraints: '设置窗口创建时各轴的尺寸约束',
  prevent_overflow: '创建时将窗口限制在显示器工作区内',
  prevent_overflow_with_margin: '创建时保留指定边距并防止窗口越出工作区',
  resizable: '设置窗口创建时是否允许调整大小',
  maximizable: '设置窗口创建时原生最大化按钮是否可用',
  minimizable: '设置窗口创建时原生最小化按钮是否可用',
  closable: '设置窗口创建时原生关闭按钮是否可用',
  fullscreen: '设置窗口创建时是否全屏',
  focused: '设置窗口创建时是否获取焦点',
  focusable: '设置窗口创建时是否允许获取焦点',
  maximized: '设置窗口创建时是否最大化',
  visible: '设置窗口创建时是否立即可见',
  transparent: '设置窗口创建时是否支持透明内容',
  decorations: '设置窗口创建时是否显示系统标题栏和边框',
  always_on_bottom: '设置窗口创建时是否始终置底',
  always_on_top: '设置窗口创建时是否始终置顶',
  visible_on_all_workspaces: '设置窗口创建时是否出现在所有虚拟桌面',
  content_protected: '设置窗口创建时是否阻止外部截图',
  icon: '设置窗口创建时的图标',
  skip_taskbar: '设置窗口创建时是否隐藏任务栏图标',
  background_color: '设置窗口创建时的背景色',
  shadow: '设置窗口创建时是否启用阴影',
  owner: '设置 Windows 新窗口的所有者窗口',
  parent: '设置新窗口关联的原生父窗口',
  transient_for: '设置 Linux 新窗口的临时父 GTK 窗口',
  drag_and_drop: '启用或禁用 Windows 文件拖放支持',
  title_bar_style: '设置 macOS 新窗口的标题栏样式',
  traffic_light_position: '设置 macOS 新窗口红黄绿按钮的位置',
  hidden_title: '设置 macOS 新窗口是否隐藏标题文字',
  tabbing_identifier: '设置 macOS 新窗口的标签分组标识',
  has_icon: '查询构建器是否已设置窗口图标',
  get_theme: '查询构建器保存的窗口主题',
  window_classname: '设置 Windows 新窗口的原生类名',
  no_redirection_bitmap: '设置 Windows 新窗口的无重定向位图样式',
  created_by_activity_name: '设置 Android 新窗口的创建来源 Activity',
  requested_by_scene_identifier: '设置 iOS 新窗口的来源场景标识',
  run_on_main_thread: '将任务交给运行时主线程执行',
  on_window_event: '注册窗口移动、缩放、关闭等事件回调',
  scale_factor: '获取逻辑像素与物理像素的缩放比例',
  inner_position: '获取内容区左上角的桌面坐标',
  outer_position: '获取含边框窗口左上角的桌面坐标',
  inner_size: '获取内容区尺寸，不含标题栏和边框',
  outer_size: '获取整个窗口尺寸，包含标题栏和边框',
  is_fullscreen: '查询窗口是否处于全屏状态',
  is_minimized: '查询窗口是否已最小化',
  is_maximized: '查询窗口是否已最大化',
  is_focused: '查询窗口是否拥有输入焦点',
  is_decorated: '查询是否启用系统标题栏和边框',
  is_resizable: '查询是否允许用户调整窗口大小',
  is_maximizable: '查询原生最大化按钮是否可用',
  is_minimizable: '查询原生最小化按钮是否可用',
  is_closable: '查询原生关闭按钮是否可用',
  is_visible: '查询窗口当前是否可见',
  is_enabled: '查询窗口是否允许用户交互',
  is_always_on_top: '查询窗口是否置顶',
  title: '获取窗口标题',
  current_monitor: '获取窗口当前所在显示器',
  primary_monitor: '获取系统主显示器',
  monitor_from_point: '查找包含指定桌面坐标的显示器',
  available_monitors: '列出系统所有可用显示器',
  gtk_window: '获取 Linux/BSD 的本地 GTK 窗口对象',
  default_vbox: '获取 GTK 窗口默认的垂直布局容器',
  activity_name: '获取 Android 窗口关联的 Activity 名称',
  scene_identifier: '获取 iOS 窗口关联的 UIScene 标识',
  window_handle: '获取用于原生互操作的窗口句柄',
  theme: '获取当前窗口的明暗主题',
  center: '将窗口居中',
  request_user_attention: '请求或取消窗口闪烁等用户提醒',
  create_window: '创建原生窗口，可附带初始 webview',
  create_webview: '在指定窗口中创建 webview',
  set_resizable: '设置是否允许用户调整窗口大小',
  set_enabled: '启用或禁用窗口交互',
  set_maximizable: '启用或禁用原生最大化按钮',
  set_minimizable: '启用或禁用原生最小化按钮',
  set_closable: '启用或禁用原生关闭按钮',
  set_title: '修改窗口标题',
  maximize: '将窗口最大化',
  unmaximize: '将窗口从最大化恢复',
  minimize: '将窗口最小化',
  unminimize: '将窗口从最小化恢复',
  show: '显示窗口',
  hide: '隐藏窗口',
  close: '请求正常关闭，可由关闭事件否决',
  destroy: '直接销毁窗口，不走关闭请求确认',
  set_decorations: '显示或隐藏系统标题栏和边框',
  set_shadow: '启用或禁用窗口阴影',
  set_always_on_bottom: '设置窗口是否始终位于其他窗口下方',
  set_always_on_top: '设置窗口是否始终位于其他窗口上方',
  set_visible_on_all_workspaces: '设置窗口是否显示在所有虚拟桌面',
  set_background_color: '设置窗口或 webview 的背景色',
  set_content_protected: '请求阻止其他应用截取窗口内容',
  set_size: '调整窗口内容区或 webview 的尺寸',
  set_min_size: '设置或清除窗口内容区最小尺寸',
  set_max_size: '设置或清除窗口内容区最大尺寸',
  set_size_constraints: '设置各轴的最小和最大尺寸约束',
  set_position: '移动窗口或窗口内的 webview',
  set_fullscreen: '进入或退出全屏',
  set_fullscreen_on_monitor: '在指定桌面坐标所在的显示器全屏',
  set_simple_fullscreen: '切换 macOS 不创建独立空间的简单全屏',
  set_focus: '将窗口置前并获取输入焦点',
  set_focusable: '设置窗口是否可以获取输入焦点',
  set_icon: '修改窗口图标',
  set_skip_taskbar: '设置是否隐藏任务栏中的窗口图标',
  set_cursor_grab: '将鼠标指针限制在窗口范围内',
  set_cursor_visible: '显示或隐藏窗口中的鼠标指针',
  set_cursor_icon: '修改窗口中的鼠标指针样式',
  set_cursor_position: '以窗口坐标设置鼠标指针位置',
  set_ignore_cursor_events: '设置鼠标事件是否穿透窗口',
  start_dragging: '启动由系统管理的窗口拖动',
  start_resize_dragging: '从指定边缘或角启动窗口缩放拖动',
  set_badge_count: '设置或清除应用图标上的数字角标',
  set_badge_label: '设置或清除 macOS Dock 图标角标文字',
  set_overlay_icon: '设置或清除 Windows 任务栏叠加图标',
  set_progress_bar: '设置任务栏或 Dock 的进度与状态',
  set_title_bar_style: '修改 macOS 标题栏样式',
  set_traffic_light_position: '移动 macOS 标题栏红黄绿按钮',
  set_theme: '设置明暗主题，或跟随系统',
  on_webview_event: '注册 webview 事件回调',
  with_webview: '在回调中访问平台原生 webview 对象',
  open_devtools: '打开 webview 开发者工具',
  close_devtools: '关闭 webview 开发者工具',
  is_devtools_open: '查询开发者工具是否已打开',
  url: '获取 webview 当前页面 URL',
  bounds: '获取 webview 在窗口内的位置和尺寸',
  position: '获取 webview 相对窗口内容区的位置',
  size: '获取 webview 内容区的物理像素尺寸',
  navigate: '让 webview 导航到指定 URL',
  reload: '重新加载 webview 当前页面',
  print: '打开当前网页的打印对话框',
  set_bounds: '同时设置 webview 的位置和尺寸',
  eval_script: '执行 JavaScript，不接收返回值',
  eval_script_with_callback: '执行 JavaScript，并回调 JSON 序列化结果',
  reparent: '将 webview 移到另一个窗口',
  cookies_for_url: '读取适用于指定 URL 的 Cookie',
  cookies: '读取当前会话的全部 Cookie',
  set_cookie: '向当前会话写入 Cookie',
  delete_cookie: '从当前会话删除指定 Cookie',
  set_auto_resize: '设置 webview 是否随父窗口自动调整布局',
  set_zoom: '设置网页缩放倍数',
  clear_all_browsing_data: '清除 webview 会话的浏览数据',
  new: '在主线程创建并初始化运行时',
  new_any_thread: '在允许的平台上从任意线程创建运行时',
  create_proxy: '创建向事件循环发送用户事件的代理',
  handle: '获取可跨线程调用的运行时句柄',
  cursor_position: '获取鼠标指针相对桌面的物理坐标',
  set_activation_policy: '设置 macOS 应用激活策略',
  set_activate_ignoring_other_apps: '设置 macOS 启动时是否抢占应用焦点',
  set_dock_visibility: '显示或隐藏 macOS Dock 应用图标',
  set_device_event_filter: '设置未聚焦窗口的底层设备事件过滤策略',
  run_iteration: '处理一轮事件，然后将控制权交回调用方',
  run_return: '运行事件循环，退出时返回退出码',
  run: '运行事件循环，结束时退出进程',
  request_exit: '携带退出码请求结束事件循环',
  display_handle: '获取窗口系统的原生显示连接句柄',
  find_class: '在 Android 项目作用域查找 Java 类',
  run_on_android_context: '在 Android JNI、Activity 和 webview 上执行回调',
  fetch_data_store_identifiers: '枚举 Apple webview 数据存储的标识',
  remove_data_store: '按标识删除 Apple webview 数据存储',
}
function description(trait, name) {
  if (trait === 'WindowBuilder') {
    const purpose = { new: '初始化窗口属性构建器', center: '设置新窗口居中', position: '设置新窗口的逻辑像素位置',
      inner_size: '设置新窗口的逻辑像素内容区尺寸', title: '设置新窗口标题', theme: '设置新窗口主题或跟随系统' }[name]
    if (purpose) return purpose
  }
  if (['Runtime', 'RuntimeHandle'].includes(trait) && ['show', 'hide'].includes(name)) {
    return name === 'show' ? '显示 macOS 应用，不自动聚焦' : '隐藏整个 macOS 应用'
  }
  if (trait === 'WebviewDispatch') {
    if (name === 'close') return '关闭 webview 原生视图，保留所属宿主窗口'
    if (name === 'set_focus') return '将所属窗口置前并聚焦 webview'
    if (name === 'show') return '显示 webview 内容'
    if (name === 'hide') return '隐藏 webview 内容'
  }
  assert.ok(DESCRIPTION[name], `${trait}::${name}: missing Chinese description`)
  return DESCRIPTION[name]
}

const link = (file, line, label = file) => `[${label}](<../${file}${line ? `#L${line}` : ''}>)`
const at = (file, needle, label = file) => {
  const index = read(file).split('\n').findIndex(line => line.includes(needle))
  assert.ok(index >= 0, `${file}: missing source anchor ${needle}`)
  return link(file, index + 1, label)
}
const limitations = {
  create_window: '官方 new_window_handler 仍需进程内原生 opener；扩展回调另表',
  create_webview: '官方 new_window_handler 不能以 Electron 扩展通过替代',
  window_handle: 'Windows 为菜单代理 HWND；macOS 不可用；Linux 为 X11 ID',
  display_handle: 'Linux 为 Xlib(None)，不等于可用原生显示连接',
  set_always_on_bottom: '非 Windows 无操作；Windows 仅 true 单次置底',
  url: '查询 Electron 当前已提交 URL，页面自行导航后也需一致',
  gtk_window: '无本地 GTK 窗口；返回错误不算功能通过',
  default_vbox: '无本地 GTK 控件树；返回错误不算功能通过',
  with_webview: '报错且不调用回调，不算功能通过',
  set_device_event_filter: '本库仅警告；Windows 仍留在分母',
  set_cursor_position: 'Linux 仅警告，不得改为平台 N/A',
  reparent: '迁移原生视图及 Rust 所属关系；需保留页面状态、IPC 和父窗销毁语义',
  fetch_data_store_identifiers: '本库报错且不回调',
  remove_data_store: '本库报错且不回调',
  new_any_thread: '必须在非主线程实际创建，new 的成功不能代替',
}
const rate = ({ passed, total }) => `${passed}/${total}（${total ? (passed * 100 / total).toFixed(1) : '0.0'}%）`
const escape = text => String(text).replaceAll('|', '&#124;').replaceAll('\n', ' ')
function resultCell(cell) {
  if (cell.status === 'na') return `N/A：${escape(cell.reason)}`
  const labels = { pass: '✅ 通过', fail: '❌ 失败', skip: '跳过（不计）', unknown: '未验证' }
  if (!cell.evidence.length) return labels[cell.status]
  const evidence = cell.evidence.find(item => item.status === cell.status) ?? cell.evidence[0]
  const title = escape(`${evidence.testRun.id} / ${evidence.test}：${evidence.assertion}`)
  return `[${labels[cell.status]}](<${evidence.run.url}> "${title.replaceAll('"', '&quot;')}")`
}
export function renderCoverage(evidence = aggregateEvidence(inventory, [])) {
  const table = ['| 成员 | 一句用途 | Windows | macOS | Linux | 差异 / 条件 |', '| --- | --- | --- | --- | --- | --- |']
  const detail = rows.map(row => [
    `## ${row.trait}（${row.members.length}）`, '', ...table,
    ...row.members.map(member => {
      const cells = PLATFORMS.map(platform => resultCell(evidence.cells.get(member.id)[platform])).join(' | ')
      const sources = `${link(member.declaration, member.declarationLine, '契约')} / ${link(WRY_LIB, member.wryLine, 'wry')}`
      return `| ${link(row.file, member.line, `\`${member.name}\``)} | ${description(row.trait, member.name)} | ${cells} | ${limitations[member.name] ?? ''} ${sources} |`
    }), '',
  ].join('\n'))
  const total = evidence.runtime.all.total
  const ext = [
    '## Electron 扩展（独立分母，不补官方 API 的缺口）', '', ...table,
    ...extensions.map(member => `| ${at(member.file, member.anchor, `\`${member.id}\``)} | ${member.purpose} | ${PLATFORMS.map(platform => resultCell(evidence.cells.get(member.id)[platform])).join(' | ')} | Electron 专有契约 |`),
    '', `扩展完整三平台通过：${rate(evidence.extensions.all)}。`, '',
  ].join('\n')
  return `# 桌面运行时逐成员验收

由 \`node tests/checks/coverage.mjs --write\` 生成。用途说明官方契约；**源码连线、编译、负测和 setter 返回 Ok 均不算功能通过。**

## 口径与当前结果

- 分母：上游六个公开 trait 的桌面可调用成员，共 **${total}** 项；关联类型、空标记 trait、纯移动端 ${excluded.length} 项不计。Windows/macOS 同名重载按一个成员、各自平台验证。
- 平台 N/A 只依据上游 cfg 或明确官方平台限制；本库不支持、跳过、失败、无证据都不得移出分母。devtools 特性计入。
- 逐平台通过只接收同一提交、同次 CI attempt、真实 Electron suite 成功后的显式行为断言；所有适用平台均通过才算完整成员。扩展另计，不冒充官方 API。
- ${evidence.runUrl ? `本报告证据：[CI ${evidence.runId}](<${evidence.runUrl}>)，提交 \`${evidence.commit}\`。` : '未导入可追溯的逐成员运行证据，因此各平台保持未验证；旧 CI 总检查数不能反推逐成员通过。'}

| 范围 | 真实行为通过 / 分母 |
| --- | --- |
${PLATFORMS.map(platform => `| ${platform} | ${rate(evidence.runtime[platform])} |`).join('\n')}
| 全部适用平台 | ${rate(evidence.runtime.all)} |

80% 完整成员门槛至少 ${Math.ceil(total * 0.8)} 项，90% 至少 ${Math.ceil(total * 0.9)} 项；未知或跳过仍留在分母。完整 marker 协议与校验器见 ${link('tests/checks/runtime-evidence.mjs')}。

## 已知边界（不由单个成员通过自动消除）

- **原生弹窗**：官方 PendingWebview::new_window_handler 需要同进程原生 opener；Electron 回调 Allow/Create/Deny 是独立扩展，必须分别验收。
- **创建属性**：proxy_url、additional_browser_args、browser_extensions_enabled / extensions_path 尚需等价行为验证。
- **原生互操作**：with_webview、GTK、AppKit、Windows 菜单代理 HWND 的限制保留；错误返回的负测不证明原生功能可用。
- **测试范围**：菜单/托盘真实 OS 操作、安装态文件关联、截图视觉结论仍需专项证据，见 ${link('docs/acceptance.md')}；${link('scripts/example.mjs')} 不得删除官方 setup 回调绕过失败。

${detail.join('\n')}
${ext}`
}

export function main(args = process.argv.slice(2)) {
  const options = {}
  const flags = ['--write', '--check', '--md', '--require-platforms', '--require-suites']
  const values = ['--evidence', '--commit', '--run', '--min-percent']
  for (let i = 0; i < args.length; i += 1) {
    const key = args[i]
    assert.ok(!Object.hasOwn(options, key), `duplicate option ${key}`)
    assert.ok(flags.includes(key) || values.includes(key), `unknown option ${key}`)
    options[key] = flags.includes(key) ? true : args[++i]
    assert.ok(options[key] && !String(options[key]).startsWith('--'), `missing value for ${key}`)
  }
  assert.ok(flags.slice(0, 3).filter(flag => options[flag]).length <= 1, 'choose write, check or md')
  assert.ok(!options['--evidence'] || !(options['--write'] || options['--check']), 'CI evidence reports use --md; checked-in docs stay deterministic')
  const artifacts = options['--evidence'] ? readArtifacts(options['--evidence']) : []
  if (options['--evidence']) assert.ok(artifacts.length, 'evidence directory has no result artifacts')
  const evidence = aggregateEvidence(inventory, artifacts, { commit: options['--commit'], runId: options['--run'] })
  if (options['--require-suites']) requireSuites(artifacts)
  if (options['--require-platforms']) {
    assert.deepEqual([...evidence.platforms].sort(), [...PLATFORMS].sort(), 'missing Windows/macOS/Linux runtime artifacts')
  }
  const markdown = renderCoverage(evidence)
  if (options['--write']) {
    mkdirSync(dirname(repo(DOC)), { recursive: true })
    writeFileSync(repo(DOC), markdown)
    console.log(`wrote ${DOC}`)
  } else if (options['--md']) process.stdout.write(markdown)
  else if (options['--check']) {
    assert.equal(read(DOC), markdown, `${DOC} is stale; run: node tests/checks/coverage.mjs --write`)
  } else {
    console.log('真实行为逐成员验收（不计编译、静态、负测、跳过或未知）')
    for (const platform of PLATFORMS) console.log(`${platform}: ${rate(evidence.runtime[platform])}`)
    console.log(`全部适用平台: ${rate(evidence.runtime.all)}`)
    console.log(`Electron 扩展（独立）: ${rate(evidence.extensions.all)}`)
  }
  if (options['--min-percent']) {
    const minimum = Number(options['--min-percent'])
    assert.ok(Number.isFinite(minimum) && minimum >= 0 && minimum <= 100, 'min-percent must be 0..100')
    const { passed, total } = evidence.runtime.all
    assert.ok(passed * 100 >= total * minimum, `acceptance ${rate(evidence.runtime.all)} is below ${minimum}%`)
  }
  return evidence
}
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) main()
