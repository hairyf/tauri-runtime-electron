import type { BaseWindow, BrowserWindow, Display, Rectangle, Screen } from 'electron'

export interface DisplayInfo {
  name: string
  position: Rectangle
  size: { width: number; height: number }
  workArea: { position: Rectangle; size: { width: number; height: number } }
  scaleFactor: number
}
export interface CursorPoint { x: number; y: number }
export interface DisplaySource { readonly screen: Screen; readonly platform: NodeJS.Platform }
export interface DisplayList { displays: DisplayInfo[]; primary: DisplayInfo }

export function physicalScreenRect(source: DisplaySource, window: BaseWindow | null, rect: Rectangle, scaleFactor: number): Rectangle {
  if (source.platform === 'win32') return source.screen.dipToScreenRect(window as BrowserWindow | null, rect)
  return { x: Math.round(rect.x * scaleFactor), y: Math.round(rect.y * scaleFactor), width: Math.round(rect.width * scaleFactor), height: Math.round(rect.height * scaleFactor) }
}

export function readDisplay(source: DisplaySource, display: Display): DisplayInfo {
  const bounds = physicalScreenRect(source, null, display.bounds, display.scaleFactor)
  const workArea = physicalScreenRect(source, null, display.workArea, display.scaleFactor)
  return {
    name: display.label, position: bounds, size: { width: bounds.width, height: bounds.height },
    workArea: { position: workArea, size: { width: workArea.width, height: workArea.height } },
    scaleFactor: display.scaleFactor,
  }
}

export function displays(source: DisplaySource): DisplayList {
  return { displays: source.screen.getAllDisplays().map((display) => readDisplay(source, display)), primary: readDisplay(source, source.screen.getPrimaryDisplay()) }
}

export function cursorPosition(source: DisplaySource): CursorPoint {
  const point = source.screen.getCursorScreenPoint()
  if (source.platform === 'win32') return source.screen.dipToScreenPoint(point)
  const scale = source.screen.getDisplayNearestPoint(point).scaleFactor
  return { x: Math.round(point.x * scale), y: Math.round(point.y * scale) }
}

export function findDisplay(source: DisplaySource, x: number, y: number, physical: boolean): Display | undefined {
  if (!Number.isFinite(x) || !Number.isFinite(y)) throw new Error('invalid monitor point')
  return source.screen.getAllDisplays().find((display) => {
    const rect = physical ? physicalScreenRect(source, null, display.bounds, display.scaleFactor) : display.bounds
    return x >= rect.x && y >= rect.y && x < rect.x + rect.width && y < rect.y + rect.height
  })
}

export function displayAtPoint(source: DisplaySource, x: number, y: number): DisplayInfo | null {
  const display = findDisplay(source, x, y, source.platform === 'win32')
  return display ? readDisplay(source, display) : null
}

export function displayForBounds(source: DisplaySource, bounds: Rectangle): DisplayInfo {
  return readDisplay(source, source.screen.getDisplayMatching(bounds))
}

export function centeredPosition(source: DisplaySource, bounds: Rectangle): CursorPoint {
  const workArea = source.screen.getDisplayMatching(bounds).workArea
  return { x: Math.round(workArea.x + (workArea.width - bounds.width) / 2), y: Math.round(workArea.y + (workArea.height - bounds.height) / 2) }
}
