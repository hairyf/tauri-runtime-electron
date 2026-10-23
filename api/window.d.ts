import { Window as TauriWindow, type Effects } from '@tauri-apps/api/window'

export { Effect, EffectState } from '@tauri-apps/api/window'
export type { Effects } from '@tauri-apps/api/window'

/** Official Window APIs, with effects delegated to tauri_runtime_electron::window::init(). */
export declare class Window extends TauriWindow {
  static getByLabel(label: string): Promise<Window | null>
  static getCurrent(): Window
  static getAll(): Promise<Window[]>
  static getFocusedWindow(): Promise<Window | null>
  /** Requires electron-window:allow-set-effects. Windows 11 build 22621+ supports Acrylic, Mica and Tabbed. */
  setEffects(effects: Effects): Promise<void>
  /** Clears native effects and restores the transparent background; native errors reject. */
  clearEffects(): Promise<void>
}

export declare function getCurrentWindow(): Window
export declare function getAllWindows(): Promise<Window[]>
