const tauriWindow = globalThis.__TAURI__?.window ?? await import('@tauri-apps/api/window')
const { invoke } = globalThis.__TAURI__?.core ?? await import('@tauri-apps/api/core')

export const { Effect, EffectState } = tauriWindow

// ponytail: Tauri v2's handle constructor uses internal `skip`; follow upstream if it gains a public equivalent.
const existingWindow = (label) => new Window(label, { skip: true })

/** Official Window APIs, with native effects provided by window::init(). */
export class Window extends tauriWindow.Window {
  static async getByLabel(label) {
    return (await getAllWindows()).find((window) => window.label === label) ?? null
  }

  static getCurrent() {
    return getCurrentWindow()
  }

  static async getAll() {
    return getAllWindows()
  }

  static async getFocusedWindow() {
    for (const window of await getAllWindows()) {
      if (await window.isFocused()) return window
    }
    return null
  }

  /** Requires electron-window:allow-set-effects; unsupported effects reject. */
  async setEffects(effects) {
    return invoke('plugin:electron-window|set_effects', { label: this.label, value: effects })
  }

  /** Clears native effects and restores the window's transparent background. */
  async clearEffects() {
    return invoke('plugin:electron-window|set_effects', { label: this.label, value: null })
  }
}

export function getCurrentWindow() {
  return existingWindow(tauriWindow.getCurrentWindow().label)
}

export async function getAllWindows() {
  return (await tauriWindow.getAllWindows()).map((window) => existingWindow(window.label))
}
