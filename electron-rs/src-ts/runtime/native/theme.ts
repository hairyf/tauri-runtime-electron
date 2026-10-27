import type { WebContents } from 'electron'
import type { Theme } from '../window'

const debuggers = new WeakSet<WebContents>()

export async function applyTheme(contents: WebContents, theme: Theme): Promise<void> {
  if (contents.isDestroyed()) return
  const debugger_ = contents.debugger
  if (!debuggers.has(contents)) {
    if (theme === null) return
    if (debugger_.isAttached()) throw new Error('window theme cannot use an existing debugger session')
    debugger_.attach()
    debuggers.add(contents)
    debugger_.once('detach', () => debuggers.delete(contents))
  }
  await debugger_.sendCommand('Emulation.setEmulatedMedia', {
    features: theme === null ? [] : [{ name: 'prefers-color-scheme', value: theme }],
  })
  if (theme === null && debuggers.delete(contents)) debugger_.detach()
}

export async function readContentsTheme(contents: WebContents | undefined, fallbackDark: boolean): Promise<boolean> {
  return contents && !contents.isDestroyed() && contents.getURL()
    ? Boolean(await contents.mainFrame.executeJavaScript("matchMedia('(prefers-color-scheme: dark)').matches"))
    : fallbackDark
}

export function holdsDebugger(contents: WebContents): boolean {
  return debuggers.has(contents)
}
