import type { BaseWindow, BrowserWindowConstructorOptions, DownloadItem, Protocol, Rectangle, Session, WebContents, WebContentsView } from 'electron'

export interface AppKit {
  configureStyle(handle: Buffer, decorated: boolean, style: string, hiddenTitle: boolean): void
  isDecorated(handle: Buffer): boolean
  setMaximizable(handle: Buffer, enabled: boolean): void
  isMaximizable(handle: Buffer): boolean
  setTrafficLightPosition(handle: Buffer, x: number, y: number): void
}

export interface IpcFetchRequest {
  url: string
  method: string
  headers: [string, string][]
  body: Uint8Array<ArrayBuffer>
}

export interface IpcFetchResponse {
  status: number
  headers: [string, string][]
  body: Uint8Array<ArrayBuffer>
}

declare global {
  interface Window {
    ipc: {
      postMessage(data: string): void
      fetch(request: IpcFetchRequest): Promise<IpcFetchResponse>
    }
  }
}

// Runtime-owned fields are kept separate from Electron's native state.
declare module 'electron' {
  interface BaseWindow {
    __tauriBackgroundColor: string
    __tauriDecorations: boolean
    __tauriSizeConstraints?: string
    __tauriMaterialBackground?: string
  }
}

export interface ProtocolReply {
  status?: number
  headers?: Record<string, string>
  body?: string
}

export interface DownloadRequest {
  item: DownloadItem
  contents: WebContents
  webviewId: number
  url: string
  finalPath: string
  directory?: string
  provisional?: string
  allow?: boolean
  state?: 'completed' | 'cancelled' | 'interrupted'
  timer?: ReturnType<typeof setTimeout>
  onDestroyed?: () => void
}

export interface NavigationRequest {
  finish: (allow: boolean, retired?: boolean) => void
  timer: ReturnType<typeof setTimeout>
  onDestroyed: () => void
}

export interface ChildWebview {
  view: WebContentsView
  contentsId: number
  main: boolean
  autoResize: boolean
  reference?: Rectangle & { parent: Rectangle }
}

export interface PendingPopup {
  window: BaseWindow
  params: { windowId: number }
  options: BrowserWindowConstructorOptions
  attach: (contents: WebContents) => void
}

export interface HandlerPolicy {
  download?: boolean
  permission?: boolean
  navigation?: boolean
}

export type Theme = 'dark' | 'light' | null
export type SizeConstraints = Partial<Record<'minWidth' | 'minHeight' | 'maxWidth' | 'maxHeight', { value: number; physical: boolean } | null>>

export interface RuntimeState {
  openingDevtools: WeakMap<WebContents, Promise<null>>
  localhostProtocolSessions: WeakSet<Protocol>
  policySessions: WeakSet<Session>
  navigationRequests: Map<number, NavigationRequest>
  nextNavigationRequest: number
  protocolRequests: Map<number, (reply: ProtocolReply) => void>
  nextProtocolId: number
  registeredSchemes: Set<string>
  socket: ReturnType<typeof import('./ipc.cjs')> | undefined
  windows: Map<number, BaseWindow>
  closeAllowed: Set<number>
  closableState: WeakMap<BaseWindow, boolean>
  maximizableState: WeakMap<BaseWindow, boolean>
  minimizableState: WeakMap<BaseWindow, boolean>
  sizeConstraints: WeakMap<BaseWindow, SizeConstraints>
  windowCursorState: WeakMap<BaseWindow, { visible: boolean; cursor?: string }>
  cursorCssKey: WeakMap<WebContents, { pending: Promise<void>; key?: string }>
  windowConfigs: WeakMap<BaseWindow, { transparent?: boolean; decorations?: boolean; titleBarStyle?: string; hiddenTitle?: boolean; titleBarHidden?: boolean; trafficLightPosition?: { x: number; y: number } }>
  fullscreenBounds: WeakMap<BaseWindow, { bounds: Rectangle; maximized: boolean }>
  webviews: Map<number, BaseWindow>
  webviewByContents: Map<number, number>
  childWebviews: Map<number, ChildWebview>
  mainViews: WeakMap<BaseWindow, ChildWebview>
  pendingPopups: Map<number, PendingPopup>
  activePopupRequest: number | undefined
  activePopupContents: WebContents | undefined
  nextPopupRequest: number
  handlerPolicy: Map<number, HandlerPolicy>
  downloadRequests: Map<number, DownloadRequest>
  permissionRequests: Map<number, (allow: boolean) => void>
  nextDownloadRequest: number
  nextPermissionRequest: number
  customSessions: Set<Session>
  nextIncognitoSession: number
  windowThemeState: WeakMap<BaseWindow, { windowId: number; theme: Theme; pending: Promise<null | void> }>
  themeDebuggers: WeakSet<WebContents>
  synchronousMethods: Set<string>
}
