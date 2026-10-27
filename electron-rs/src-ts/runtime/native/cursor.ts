import type { WebContents } from 'electron'
import type { CursorState } from '../window'

const states = new WeakMap<WebContents, { key?: string; pending: Promise<void> }>()

export function cursorCss(state: Readonly<CursorState> | undefined): string | undefined {
  return state?.visible === false ? 'none' : state?.cursor
}

export function applyCursor(contents: WebContents, state: Readonly<CursorState> | undefined): Promise<void> {
  const current = states.get(contents) ?? { pending: Promise.resolve() }
  states.set(contents, current)
  const value = cursorCss(state)
  const pending = current.pending.then(async () => {
    if (contents.isDestroyed()) return
    if (current.key) {
      const key = current.key
      current.key = undefined
      await contents.removeInsertedCSS(key).catch(() => {})
    }
    if (value) current.key = await contents.insertCSS(`* { cursor: ${value} !important }`, { cssOrigin: 'user' })
  })
  // A page may not have committed yet; its owner replays on dom-ready.
  current.pending = pending.catch(() => {})
  return pending
}

export function clearCursor(contents: WebContents): Promise<void> {
  return applyCursor(contents, undefined)
}
