import type { WebContents } from 'electron'
import type { Theme } from '../window'

/**
 * 主题覆盖（骨架）。
 *
 * 窗口级主题不能改 app.nativeTheme（那是全进程的），只能用 Chromium 的媒体仿真：
 * `Emulation.setEmulatedMedia` 覆盖 prefers-color-scheme。这条路只有 debugger 协议提供，
 * 于是带来两个必须处理的边界：
 * - devtools 也占用 debugger：已被别处 attach 时要报错，不能抢；
 * - devtools 关闭会 detach 我们的会话，主题需要重放。
 *
 * 取代 state.themeDebuggers + emulateContentsTheme。
 */

/**
 * theme 为 null 表示撤掉覆盖：发空的 features 后 detach，把控制权还给系统。
 * 同时要维护「本模块是否持有该页面的 debugger」，否则会误 detach devtools 的会话。
 */
export function applyTheme(contents: WebContents, theme: Theme): Promise<void> {
  // TODO: Implement applyTheme
  throw new Error('not implemented')
}

/**
 * 页面自身解析出的深浅色。
 * 未加载的窗口没有文档，这时回退到 fallbackDark（通常是 nativeTheme.shouldUseDarkColors）。
 */
export function readContentsTheme(contents: WebContents | undefined, fallbackDark: boolean): Promise<boolean> {
  // TODO: Implement readContentsTheme
  throw new Error('not implemented')
}

/** 当前页面是否仍由本模块持有 debugger（devtools 抢占/关闭后需要重放）。 */
export function holdsDebugger(contents: WebContents): boolean {
  // TODO: Implement holdsDebugger
  throw new Error('not implemented')
}
