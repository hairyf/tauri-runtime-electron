import { useEffect, useRef, useState } from 'react'
import { createRoot } from 'react-dom/client'
import * as tauri from '@tauri-apps/api'
import { createChecks, runChecks } from './probes'
import type { CheckResult } from './probes'
import inventory from './inventory.json'
import './style.css'

type Platform = 'windows' | 'macos' | 'linux'
type Status = 'pass' | 'fail' | 'unknown' | 'na' | 'skip'
type Cell = { status: Status; reason?: string; assertion?: string }
type Member = {
  id: string
  purpose: string
  na?: Partial<Record<Platform, string>>
  archived?: Partial<Record<Platform, Cell>>
}
type Result = Omit<CheckResult, 'status'> & { status: Status }
type Coverage = {
  done: boolean
  results: Result[]
  error: string | null
  platform: Platform
  archivedCommit: string
}

declare global {
  interface Window { __coverage: Coverage }
}

const platforms: Platform[] = ['windows', 'macos', 'linux']
const platform: Platform = /Win/.test(navigator.platform) ? 'windows' : /Mac/.test(navigator.platform) ? 'macos' : 'linux'
const members: Member[] = [
  ...inventory.members as Member[],
  { id: 'app::ipc', purpose: 'Tauri IPC 往返' },
  { id: 'app::event', purpose: 'Tauri 事件 payload 往返' },
]
const initial: Coverage = { done: false, results: [], error: null, platform, archivedCommit: inventory.commit }
window.__coverage = initial

const labels: Record<Status, string> = { pass: '通过', fail: '失败', unknown: '未验证', na: '不适用', skip: '跳过' }
const colors: Record<Status, string> = {
  pass: 'bg-emerald-100 text-emerald-800 dark:bg-emerald-950 dark:text-emerald-300',
  fail: 'bg-red-100 text-red-800 dark:bg-red-950 dark:text-red-300',
  unknown: 'bg-slate-100 text-slate-600 dark:bg-slate-800 dark:text-slate-300',
  na: 'bg-amber-100 text-amber-900 dark:bg-amber-950 dark:text-amber-300',
  skip: 'bg-amber-100 text-amber-900 dark:bg-amber-950 dark:text-amber-300',
}
const control = 'rounded-md border border-slate-300 bg-white px-3 py-2 text-sm dark:border-slate-600 dark:bg-slate-900'
const button = `${control} cursor-pointer disabled:cursor-wait disabled:opacity-50`
const detailStyle = 'mt-1 block max-w-md text-xs leading-relaxed break-words text-slate-600 dark:text-slate-400'

function StatusCell({ status, detail }: { status: Status; detail?: string }) {
  return <td className="border-t border-slate-200 px-4 py-3 align-top dark:border-slate-700">
    <span className={`inline-block rounded px-2 py-0.5 text-xs font-semibold whitespace-nowrap ${colors[status]}`}>{labels[status]}</span>
    {detail && <span className={detailStyle}>{detail}</span>}
  </td>
}

function exportResults(state: Coverage) {
  const blob = new Blob([JSON.stringify({ ...state, recordedAt: new Date().toISOString() }, null, 2)], { type: 'application/json' })
  const url = URL.createObjectURL(blob)
  const link = document.createElement('a')
  link.href = url
  link.download = 'local-api-acceptance.json'
  link.click()
  setTimeout(() => URL.revokeObjectURL(url), 1000)
}

function Dashboard() {
  const [state, setState] = useState(initial)
  const [search, setSearch] = useState('')
  const [group, setGroup] = useState('')
  const [status, setStatus] = useState('')
  const running = useRef(false)

  async function run() {
    if (running.current) return
    running.current = true
    let current: Coverage = { ...initial, results: [] }
    const publish = () => { window.__coverage = current; setState(current) }
    const append = (result: Result) => { current = { ...current, results: [...current.results, result] }; publish() }
    publish()
    try {
      await runChecks(createChecks(tauri), result => {
        const reason = members.find(member => member.id === result.id)?.na?.[platform]
        append(reason ? { ...result, status: 'na', detail: reason } : result)
      })
    } catch (error) {
      current = { ...current, error: String(error) }
      append({ id: 'app::runner', status: 'fail', detail: String(error) })
    } finally {
      try {
        const probe = await tauri.window.Window.getByLabel('probe')
        if (probe) await probe.destroy()
      } catch (error) {
        append({ id: 'app::cleanup', status: 'fail', detail: String(error) })
      }
      current = { ...current, done: true }
      running.current = false
      publish()
    }
  }

  useEffect(() => { void run() }, [])

  const results = new Map(state.results.map(result => [result.id, result]))
  const extra = state.results.filter(result => !members.some(member => member.id === result.id))
  const rows: Member[] = [...members, ...extra.map(result => ({ id: result.id, purpose: '验收过程错误' }))]
  const visible = rows.filter(row => (!search || `${row.id} ${row.purpose}`.toLowerCase().includes(search.trim().toLowerCase()))
    && (!group || row.id.startsWith(`${group}::`)) && (!status || localResult(row).status === status))
  const passed = state.results.filter(result => result.status === 'pass').length
  const failed = state.results.filter(result => result.status === 'fail').length
  const untouched = members.filter(member => !results.has(member.id)).length

  function localResult(row: Member): Result {
    return results.get(row.id) ?? {
      id: row.id,
      status: row.na?.[platform] ? 'na' : 'unknown',
      detail: row.na?.[platform] ?? '本次未执行；请运行对应 Rust suite 或专项人工验收',
    }
  }

  return <main className="mx-auto max-w-[1440px] p-4 md:p-8">
    <header className="max-w-4xl">
      <p className="text-xs font-bold tracking-widest text-indigo-700 dark:text-indigo-300">TAURI × ELECTRON · DESKTOP ACCEPTANCE</p>
      <h1 className="mt-2 mb-3 text-3xl font-bold">API 验收面板</h1>
      <p>逐项列出桌面运行时 API。打开后自动使用独立窗口实测；失败会显示具体原因，未覆盖的成员保持“未验证”。</p>
    </header>
    <section className="my-6 flex flex-wrap items-center gap-3" aria-label="验收控制">
      <button id="run" className={`${button} border-indigo-700 bg-indigo-700! text-white dark:border-indigo-500`} disabled={!state.done} onClick={() => void run()}>重新运行实测</button>
      <button id="export" className={button} onClick={() => exportResults(state)}>导出本次结果</button>
      <p id="summary" role="status" aria-live="polite" className="text-sm">{state.error ? `启动失败：${state.error}` : `${state.done ? '本次完成' : '实测进行中'} · ${passed} 通过 / ${failed} 失败 / ${untouched} 未执行（含不适用）`}</p>
    </section>
    <aside className="border-l-4 border-indigo-500 bg-indigo-50 px-5 py-4 text-sm leading-relaxed dark:bg-slate-800">
      <strong>本次实测 ≠ 存档 CI。</strong> 本次列只记录实际调用及断言；setter 仅返回成功不算通过。存档列仅证明标注提交，不能证明当前工作区。
      <p id="archive" className="mt-2"><a className="text-indigo-700 underline dark:text-indigo-300" href={inventory.runUrl} target="_blank" rel="noreferrer">存档 CI 提交 {inventory.commit}（非本次工作区结果）</a></p>
      <p className="mt-2">原生互操作、平台专有、创建属性和生命周期等完整验收仍由 Rust runtime suites 执行：<code>pnpm test:runtime</code>。UI 不将这些未运行项目冒充通过；视觉效果仍需人工观察。</p>
    </aside>
    <section className="my-6 flex flex-wrap items-center gap-4 text-sm" aria-label="筛选 API">
      <label className="flex items-center gap-2">搜索 <input id="search" className={`${control} w-60`} type="search" placeholder="API 名称或用途" value={search} onChange={event => setSearch(event.target.value)} /></label>
      <label className="flex items-center gap-2">分组 <select id="group" className={control} value={group} onChange={event => setGroup(event.target.value)}><option value="">全部分组</option>{[...new Set(rows.map(row => row.id.split('::')[0]))].map(name => <option key={name}>{name}</option>)}</select></label>
      <label className="flex items-center gap-2">本次状态 <select id="status" className={control} value={status} onChange={event => setStatus(event.target.value)}><option value="">全部</option>{Object.entries(labels).map(([value, label]) => <option key={value} value={value}>{label}</option>)}</select></label>
      <span className="text-slate-500 dark:text-slate-400">显示 {visible.length} / {rows.length} 项</span>
    </section>
    <div className="overflow-auto rounded-lg border border-slate-200 bg-white dark:border-slate-700 dark:bg-slate-900">
      <table className="w-full border-collapse text-left text-sm">
        <caption className="p-4 text-left text-xs text-slate-600 dark:text-slate-400">API 逐项验收；额外应用级检查单列，不计入 trait 覆盖率</caption>
        <thead className="bg-slate-100 text-xs whitespace-nowrap dark:bg-slate-800"><tr>{['成员 / 用途', '本次实测', '存档 Windows', '存档 macOS', '存档 Linux'].map(title => <th key={title} scope="col" className="px-4 py-3">{title}</th>)}</tr></thead>
        <tbody id="members">{visible.map(row => {
          const local = localResult(row)
          return <tr key={row.id}>
            <td className="w-2/5 border-t border-slate-200 px-4 py-3 align-top dark:border-slate-700"><code className="break-all">{row.id}</code><span className={detailStyle}>{row.purpose}</span></td>
            <StatusCell status={local.status} detail={local.detail} />
            {platforms.map(os => {
              const cell = row.archived?.[os]
              return <StatusCell key={os} status={cell?.status ?? 'unknown'} detail={cell?.reason || cell?.assertion} />
            })}
          </tr>
        })}</tbody>
      </table>
      {!visible.length && <p className="p-6 text-center text-slate-500">没有匹配的 API，请调整筛选条件。</p>}
    </div>
  </main>
}

const root = document.getElementById('root')
if (!root) throw new Error('缺少 React 挂载节点')
createRoot(root).render(new URLSearchParams(location.search).has('probe')
  ? <main className="p-6">独立 API 测试窗口：大小、标题、显示与状态会自动变化，结束后自动关闭。</main>
  : <Dashboard />)
