import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import type { BrowserWindowConstructorOptions, Session, WebContents } from 'electron'
import type { DownloadDecision, Downloads } from './downloads'
import type { Webview, Webviews } from './webview'
import type { Window, Windows } from './window'

export type PermissionKind = 'geolocation' | 'notifications' | 'clipboard-read' | 'display-capture' | 'midi' | 'sensors' | 'camera' | 'microphone'
export interface PolicyBridge { send(event: string, params: Record<string, unknown>): void; request<T>(event: string, params: Record<string, unknown>, timeout?: number): T }
export interface PopupDetails { request: number; webviewId: number; url: string; frameName: string; features: string; disposition: string; position: { x: number; y: number } | null; size: { width: number; height: number } | null }
export interface NewWindowResponse { action: 'allow' | 'deny'; windowId?: number }
export interface PopupPlan { window: Window; options: BrowserWindowConstructorOptions; attach(contents: WebContents): void }
export interface PolicyHost { readonly transport: PolicyBridge; readonly downloads: Downloads; readonly webviews: Webviews; readonly windows: Windows; readonly popups: Map<number, PopupPlan> }
interface NavigationEntry { finish(allow: boolean, retired?: boolean): void; timer: NodeJS.Timeout; onDestroyed(): void; nativeId: number; contents: WebContents }

const popupGeometry = (features: string): Pick<PopupDetails, 'position' | 'size'> => {
  const values = Object.fromEntries(String(features ?? '').split(',').map((entry) => entry.trim().split('=')))
  const number = (...keys: string[]) => { const raw = keys.map((key) => values[key]).find((value) => value !== undefined); const n = Number.parseInt(raw, 10); return Number.isFinite(n) ? n : undefined }
  const x = number('left', 'x'); const y = number('top', 'y'); const width = number('width', 'innerWidth'); const height = number('height', 'innerHeight')
  return { position: x === undefined && y === undefined ? null : { x: x ?? 0, y: y ?? 0 }, size: width === undefined && height === undefined ? null : { width: width ?? 800, height: height ?? 600 } }
}

export class Policies {
  private readonly sessions = new WeakSet<Session>(); private readonly navigations = new Map<number, NavigationEntry>(); private readonly nativeNavigations = new WeakMap<Session, Map<number, NavigationEntry>>(); private readonly permissions = new Map<number, (allow: boolean) => void>(); private nextNavigationRequest = 0; private nextDownloadRequest = 0; private nextPermissionRequest = 0; private nextPopupRequest = 0; private disposed = false
  private popup?: { request: number; opener: Webview }
  constructor(private readonly host: PolicyHost) {}
  get activePopup(): { request: number; opener: Webview } | undefined { return this.popup }
  install(session: Session): void {
    if (this.disposed || this.sessions.has(session)) return; this.sessions.add(session)
    const nativeNavigations = new Map<number, NavigationEntry>()
    this.nativeNavigations.set(session, nativeNavigations)
    const retire = (details: { id: number }) => nativeNavigations.get(details.id)?.finish(false, true)
    session.webRequest.onCompleted(retire); session.webRequest.onErrorOccurred(retire)
    session.webRequest.onBeforeRequest((details, callback) => {
      const contentsId = details.webContents?.id ?? details.webContentsId
      const webview = contentsId === undefined ? undefined : this.host.webviews.forContents(contentsId)
      const contents = webview?.contents
      if (!contents || !['mainFrame', 'subFrame'].includes(details.resourceType) || !webview?.webviewPolicy?.navigation) { callback({}); return }
      const id = ++this.nextNavigationRequest
      const entry: NavigationEntry = { nativeId: details.id, contents, timer: undefined as unknown as NodeJS.Timeout, onDestroyed: () => finish(false) , finish: (allow, retired = false) => {
        if (!this.navigations.delete(id)) return; nativeNavigations.delete(details.id); clearTimeout(entry.timer); contents.removeListener('destroyed', entry.onDestroyed); if (retired || this.disposed) return
        try { if (!allow && details.resourceType === 'mainFrame' && !contents.isDestroyed()) contents.stop() } finally { callback({ cancel: !allow }) }
      } }
      const finish = (allow: boolean) => entry.finish(allow)
      entry.onDestroyed = () => finish(false); entry.timer = setTimeout(entry.onDestroyed, 15_000); entry.timer.unref(); this.navigations.set(id, entry); nativeNavigations.set(details.id, entry); contents.once('destroyed', entry.onDestroyed)
      this.host.transport.send('webview.navigationRequested', { id, webviewId: webview.id, url: details.url })
    })
    session.on('will-download', (event, item, contents) => {
      const webview = contents ? this.host.webviews.forContents(contents.id) : undefined
      if (!webview?.webviewPolicy?.download) return
      const id = ++this.nextDownloadRequest
      const suggestedPath = item.getSavePath() || path.join(os.tmpdir(), item.getFilename())
      let directory: string
      try { directory = fs.mkdtempSync(path.join(os.tmpdir(), 'electron-download-')); const provisional = path.join(directory, 'download'); item.setSavePath(provisional); const download = this.host.downloads.begin({ id, webviewId: webview.id, item, contents, directory, provisional, suggestedPath }); this.host.transport.send('download.requested', { id, webviewId: webview.id, url: item.getURL(), destination: suggestedPath }); void download } catch { event.preventDefault(); this.host.transport.send('download.finished', { webviewId: webview.id, url: item.getURL(), path: null, success: false }) }
    })
    session.setPermissionRequestHandler((contents, permission, callback, details) => {
      const webview = this.host.webviews.forContents(contents.id); const kind = permissionKind(permission, details as { mediaTypes?: string[] })
      if (!webview?.webviewPolicy?.permission || !kind) { callback(false); return }
      const id = ++this.nextPermissionRequest; this.permissions.set(id, callback); this.host.transport.send('permission.requested', { id, webviewId: webview.id, kind })
    })
  }
  attach(webview: Webview): void {
    const contents = webview.contents
    contents.setWindowOpenHandler((details) => {
      const request = this.nextPopupRequest++
      const previous = this.popup; this.popup = { request, opener: webview }
      let allowed = false
      try {
        const response = this.host.transport.request<NewWindowResponse>('webview.newWindow', { request, webviewId: webview.id, url: details.url, frameName: details.frameName, features: details.features ?? '', disposition: details.disposition, ...popupGeometry(details.features ?? '') })
        if (response.action !== 'allow' || !Number.isSafeInteger(response.windowId)) return { action: 'deny' as const }
        const selected = this.host.popups.get(request)
        if (!selected || selected.window.id !== response.windowId || selected.window.destroyed) throw new Error('missing native popup destination')
        allowed = true
        return { action: 'allow' as const, outlivesOpener: true, overrideBrowserWindowOptions: selected.options, createWindow: (options: BrowserWindowConstructorOptions) => { const guest = (options as BrowserWindowConstructorOptions & { webContents: WebContents }).webContents; this.host.popups.delete(request); selected.attach(guest); return guest } }
      } catch { return { action: 'deny' as const } } finally {
        this.popup = previous
        const abandoned = this.host.popups.get(request)
        if (abandoned && !allowed) { this.host.popups.delete(request); abandoned.window.destroy() }
      }
    })
  }
  answerNavigation(id: number, allow: boolean): void { if (typeof allow !== 'boolean') throw new Error('invalid navigation decision'); this.navigations.get(id)?.finish(allow) }
  answerDownload(id: number, decision: DownloadDecision): void { this.host.downloads.decide(id, decision) }
  answerPermission(id: number, response: 'allow' | 'deny' | 'default'): void { const callback = this.permissions.get(id); if (!callback) return; this.permissions.delete(id); callback(response === 'allow') }
  dispose(): void { this.disposed = true; for (const entry of this.navigations.values()) entry.finish(false); this.permissions.forEach((callback) => callback(false)); this.permissions.clear(); this.host.downloads.dispose() }
}

export function permissionKind(permission: string, details?: { mediaTypes?: string[] }): PermissionKind | undefined {
  if (permission === 'media') return details?.mediaTypes?.includes('video') ? 'camera' : 'microphone'
  return ({ geolocation: 'geolocation', notifications: 'notifications', 'clipboard-read': 'clipboard-read', 'display-capture': 'display-capture', midi: 'midi', midiSysex: 'midi', sensors: 'sensors' } as Record<string, PermissionKind>)[permission]
}
