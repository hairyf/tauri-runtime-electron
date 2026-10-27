import { EventEmitter } from 'node:events'
import type { DownloadItem, WebContents } from 'electron'

/**
 * 下载领域（骨架）。
 *
 * 原来这一块散在 main.cjs 的 will-download + 三个函数 + commands.cjs 的
 * webview.downloadResponse 里，靠 state.downloadRequests 一张表通信。
 *
 * 关键约束（决定了为什么要有 staging）：
 * - Electron 只允许在 will-download 这个同步回调里 setSavePath；
 * - 真正落盘路径由 Rust 稍后决定，且不能覆盖已有文件；
 * - 决策与完成可能以任意顺序到达，必须两边都齐了才能收尾。
 * 于是先写到临时目录，决策到手后用 COPYFILE_EXCL 发布。
 */

export type DownloadState = 'completed' | 'interrupted' | 'cancelled'

export interface DownloadInit {
  id: number
  webviewId: number
  item: DownloadItem
  contents: WebContents
  /** 决策未到之前的落盘位置（临时目录里的 provisional）。 */
  directory: string
  provisional: string
  /** 默认目标：Rust 没指定 destination 时用它。 */
  suggestedPath: string
}

export interface DownloadDecision {
  allow: boolean
  destination?: string
}

export interface DownloadFinished {
  webviewId: number
  url: string
  path: string | null
  success: boolean
}

export interface DownloadEvents {
  finished: [payload: DownloadFinished]
}

/** 单次下载：只负责状态与落盘，不发事件。 */
export class Download {
  private allow?: boolean
  private state?: DownloadState
  private finalPath: string
  private published = false

  constructor(private readonly init: DownloadInit) {
    this.finalPath = init.suggestedPath
  }

  get id(): number {
    return this.init.id
  }

  get webviewId(): number {
    return this.init.webviewId
  }

  get url(): string {
    return this.init.item.getURL()
  }

  get item(): DownloadItem {
    return this.init.item
  }

  get contents(): WebContents {
    return this.init.contents
  }

  get allowed(): boolean | undefined {
    return this.allow
  }

  get finishedState(): DownloadState | undefined {
    return this.state
  }

  get settled(): boolean {
    return this.allow !== undefined && this.state !== undefined
  }

  /** 仅第一次决策生效；后续重复应答忽略。 */
  decide(decision: DownloadDecision): void {
    // TODO: Implement decide
  }

  /** 来自 DownloadItem 的 done 事件。 */
  mark(state: DownloadState): void {
    // TODO: Implement mark
  }

  /**
   * 用 COPYFILE_EXCL 把临时文件挪到最终路径：跨设备可复制，且绝不覆盖已有数据。
   * 失败时保留临时文件，便于用户手动取回。
   */
  publish(): boolean {
    // TODO: Implement publish
    throw new Error('not implemented')
  }

  clean(): void {
    // TODO: Implement clean
  }
}

/**
 * request id → Download。收尾条件满足时抛 finished 事件，宿主转成线协议。
 */
export class Downloads extends EventEmitter<DownloadEvents> {
  private readonly pending = new Map<number, Download>()

  begin(init: DownloadInit): Download {
    // TODO: Implement begin
    throw new Error('not implemented')
  }

  get(id: number): Download | undefined {
    // TODO: Implement get
    throw new Error('not implemented')
  }

  /** webview.downloadResponse。 */
  decide(id: number, decision: DownloadDecision): void {
    // TODO: Implement decide
  }

  /** DownloadItem 'done' → completed / interrupted。 */
  mark(id: number, state: DownloadState): void {
    // TODO: Implement mark
  }

  dispose(): void {
    // TODO: Implement dispose
  }
}
