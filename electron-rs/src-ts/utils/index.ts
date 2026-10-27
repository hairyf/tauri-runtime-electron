/**
 * 与业务无关的通用工具。
 *
 * 约束：这里只放「跨模块使用、且不依赖任何领域类型」的东西。
 * 只被单个域使用的纯函数（窗口尺寸换算、CSS 颜色等）留在那个域自己的文件里，
 * 否则 utils 迟早变成杂物间。
 */

/** 读 `--name value` 或 `--name=value` 两种写法。 */
export function arg(name: string, argv: readonly string[] = process.argv): string | undefined {
  const index = argv.indexOf(name)
  if (index !== -1) return argv[index + 1]
  const prefixed = argv.find((value) => value.startsWith(`${name}=`))
  return prefixed?.slice(name.length + 1)
}

/** 空实现。写出来比空箭头函数更明确：这里确实什么都不做。 */
export const noop = (): void => {}

/** 命令表允许返回 Promise；同步策略回调期间不能容忍。 */
export const isThenable = (value: unknown): value is PromiseLike<unknown> =>
  typeof (value as PromiseLike<unknown> | null)?.then === 'function'

/**
 * 已绑定作用域的日志出口。需要日志的模块各自 createLogger，不经过组合根传递。
 *
 * 直接写 console：Electron 的 stdout/stderr 由 Rust 继承（ipc.rs 只 pipe 了 stdin），
 * 所以不需要另开文件或 IPC 通道。代价是打包成 GUI 后若无人收集 stderr，日志会丢。
 */
export interface Logger {
  error(...args: unknown[]): void
  warn(...args: unknown[]): void
}

export function createLogger(scope: string): Logger {
  const prefix = `[${scope}]`
  return {
    error: (...args) => console.error(prefix, ...args),
    warn: (...args) => console.warn(prefix, ...args),
  }
}
