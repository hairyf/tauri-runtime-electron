import type { BaseWindow, Rectangle, Screen } from 'electron'

/**
 * 显示器领域（骨架）。
 *
 * 从 window.ts 里拆出来的原因：window.displays / window.cursorPosition /
 * window.displayForPoint 三个命令与具体窗口无关，之前却住在 executeWindow 里，
 * 于是任何窗口逻辑改动都要碰它们。
 *
 * 坐标约定的唯一来源也在这里：
 * - Tauri 的显示器与坐标全部是物理像素
 * - Electron 的 display / bounds / cursor 全部是 DIP
 * 所以每个函数要么显式接收是否 physical，要么显式做换算。
 */

/** Tauri 侧的显示器信息，全部是物理像素。 */
export interface DisplayInfo {
  name: string
  position: { x: number; y: number }
  size: { width: number; height: number }
  workArea: {
    position: { x: number; y: number }
    size: { width: number; height: number }
  }
  scaleFactor: number
}

export interface CursorPoint {
  x: number
  y: number
}

/** 无状态世界：只有 Electron 的 screen 和平台判断，没有其它依赖。 */
export interface DisplaySource {
  readonly screen: Screen
  readonly platform: NodeJS.Platform
}

/** window.displays 的返回结构。 */
export interface DisplayList {
  displays: DisplayInfo[]
  primary: DisplayInfo
}

export function displays(source: DisplaySource): DisplayList {
  // TODO: Implement displays
  throw new Error('not implemented')
}

/** 物理像素的全局光标位置。 */
export function cursorPosition(source: DisplaySource): CursorPoint {
  // TODO: Implement cursorPosition
  throw new Error('not implemented')
}

/** 覆盖 Window 所在显示器的信息（window.currentDisplay）。 */
export function displayForBounds(source: DisplaySource, bounds: Rectangle): DisplayInfo {
  // TODO: Implement displayForBounds
  throw new Error('not implemented')
}

/** window.displayForPoint：点在所有显示器之外时返回 null，而不是就近吸附。 */
export function displayAtPoint(source: DisplaySource, x: number, y: number): DisplayInfo | null {
  // TODO: Implement displayAtPoint
  throw new Error('not implemented')
}

/** 命中测试：Tauri 在窗口外返回 undefined，Electron 的 nearest 会吸附。 */
export function findDisplay(
  source: DisplaySource,
  x: number,
  y: number,
  physical: boolean,
): Electron.Display | undefined {
  // TODO: Implement findDisplay
  throw new Error('not implemented')
}

/** DIP → 物理。Windows 上必须走 screen.dipToScreenRect，不能用 scaleFactor 乘。 */
export function physicalScreenRect(
  source: DisplaySource,
  window: BaseWindow | null,
  rect: Rectangle,
  scaleFactor: number,
): Rectangle {
  // TODO: Implement physicalScreenRect
  throw new Error('not implemented')
}

export function readDisplay(source: DisplaySource, display: Electron.Display): DisplayInfo {
  // TODO: Implement readDisplay
  throw new Error('not implemented')
}

/** 窗口所在显示器的工作区中心，供 Window.center() 使用。 */
export function centeredPosition(source: DisplaySource, bounds: Rectangle): CursorPoint {
  // TODO: Implement centeredPosition
  throw new Error('not implemented')
}
