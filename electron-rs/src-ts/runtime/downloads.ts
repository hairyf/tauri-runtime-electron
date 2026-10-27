import { EventEmitter } from 'node:events'
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import type { DownloadItem, WebContents } from 'electron'

export type DownloadState = 'completed' | 'interrupted' | 'cancelled'
export interface DownloadInit { id: number; webviewId: number; item: DownloadItem; contents: WebContents; directory: string; provisional: string; suggestedPath: string }
export interface DownloadDecision { allow: boolean; destination?: string }
export interface DownloadFinished { webviewId: number; url: string; path: string | null; success: boolean }
export interface DownloadEvents { finished: [payload: DownloadFinished] }

export class Download {
  private allow?: boolean
  private state?: DownloadState
  private finalPath: string
  private published = false
  constructor(private readonly init: DownloadInit) { this.finalPath = init.suggestedPath }
  get id(): number { return this.init.id }
  get webviewId(): number { return this.init.webviewId }
  get url(): string { return this.init.item.getURL() }
  get item(): DownloadItem { return this.init.item }
  get contents(): WebContents { return this.init.contents }
  get allowed(): boolean | undefined { return this.allow }
  get finishedState(): DownloadState | undefined { return this.state }
  get settled(): boolean { return this.allow !== undefined && this.state !== undefined }
  get path(): string { return this.finalPath }
  decide(decision: DownloadDecision): void {
    if (this.allow !== undefined) return
    if (typeof decision.allow !== 'boolean') throw new Error('invalid download decision')
    const destination = decision.destination ?? this.finalPath
    this.allow = decision.allow && typeof destination === 'string' && destination.length > 0 && !destination.includes('\0')
    if (this.allow) this.finalPath = destination
    if (!this.allow && this.state === undefined) {
      try { this.init.item.cancel() } catch { /* done event will settle it */ }
      this.state = 'cancelled'
    }
  }
  mark(state: DownloadState): void { if (this.state === undefined) this.state = state }
  publish(): boolean {
    if (this.published || !this.allow || this.state !== 'completed') return this.published
    try { fs.copyFileSync(this.init.provisional, this.finalPath, fs.constants.COPYFILE_EXCL); this.published = true; return true } catch (error) {
      try { fs.appendFileSync(path.join(os.tmpdir(), 'tauri-runtime-electron.log'), `download.publish failed; staged data retained ${this.init.provisional} ${String(error)}\n`) } catch { /* best effort */ }
      return false
    }
  }
  clean(): void { try { fs.rmSync(this.init.directory, { recursive: true, force: true }) } catch { /* best effort */ } }
}

export class Downloads extends EventEmitter<DownloadEvents> {
  private readonly pending = new Map<number, Download>()
  private readonly timers = new Map<number, NodeJS.Timeout>()
  begin(init: DownloadInit): Download {
    if (this.pending.has(init.id)) throw new Error(`duplicate download ${init.id}`)
    const download = new Download(init); this.pending.set(init.id, download)
    const onDestroyed = () => { if (!download.settled) { download.decide({ allow: false }); download.mark('cancelled') }; this.finish(init.id) }
    init.contents.once('destroyed', onDestroyed)
    const timer = setTimeout(onDestroyed, 30_000); timer.unref(); this.timers.set(init.id, timer)
    init.item.once('done', (_event, state) => { if (!this.pending.has(init.id)) download.clean(); else { download.mark(state as DownloadState); this.finish(init.id) } })
    return download
  }
  get(id: number): Download | undefined { return this.pending.get(id) }
  decide(id: number, decision: DownloadDecision): void {
    const download = this.pending.get(id); if (!download) return
    download.decide(decision)
    const timer = this.timers.get(id); if (timer) { clearTimeout(timer); this.timers.delete(id) }
    this.finish(id)
  }
  mark(id: number, state: DownloadState): void { const download = this.pending.get(id); if (!download) return; download.mark(state); this.finish(id) }
  private finish(id: number): void {
    const download = this.pending.get(id); if (!download || !download.settled) return
    this.pending.delete(id); const timer = this.timers.get(id); if (timer) clearTimeout(timer); this.timers.delete(id)
    const success = download.publish(); if (success || !download.allowed || download.finishedState !== 'completed') download.clean()
    this.emit('finished', { webviewId: download.webviewId, url: download.url, path: success ? download.path : null, success })
  }
  dispose(): void { for (const id of [...this.pending.keys()]) { const download = this.pending.get(id); if (!download) continue; download.decide({ allow: false }); download.mark('cancelled'); this.finish(id) }; for (const timer of this.timers.values()) clearTimeout(timer); this.timers.clear() }
}
