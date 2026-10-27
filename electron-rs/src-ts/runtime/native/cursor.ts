import type { WebContents } from 'electron'
import type { CursorState } from '../window'

/**
 * 光标下发（骨架）。
 *
 * 取代 state.cursorCssKey 与 applyWindowCursorCss 里的那条 Promise 链：
 * 每个页面的「已插入的 CSS key + 串行队列」是适配器自己的状态，不是窗口的状态。
 *
 * 三点必须保留：
 * - insertCSS / removeInsertedCSS 是异步的，同一页面必须串行，否则旧 key 泄漏；
 * - 页面未提交时插入会失败，dom-ready 需要重放（由 Window.paintCursor 触发）；
 * - visible:false 优先于自定义图标。
 */

/** 计算 CSS 值：不可见时是 'none'，否则是指定光标。 */
export function cursorCss(state: Readonly<CursorState> | undefined): string | undefined {
  // TODO: Implement cursorCss
  throw new Error('not implemented')
}

/**
 * 用一个页面级的串行队列替换已插入的样式。
 * 内部状态：WeakMap<WebContents, { key?: string; pending: Promise<void> }>。
 */
export function applyCursor(contents: WebContents, state: Readonly<CursorState> | undefined): Promise<void> {
  // TODO: Implement applyCursor
  throw new Error('not implemented')
}

/** 页面销毁或换所有者时清掉，避免 key 泄漏。 */
export function clearCursor(contents: WebContents): Promise<void> {
  // TODO: Implement clearCursor
  throw new Error('not implemented')
}
