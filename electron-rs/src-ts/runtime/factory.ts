import type { Session } from 'electron'
import type { Policies, PopupPlan } from './policy'
import type { Protocols } from './protocol'
import type { Transport } from '../socket/transport'
import type { Webview, Webviews } from './webview'
import type { Window, WindowEnvironment, Windows } from './window'

/**
 * 命令表看到的创建入口。定义在使用它的抽象旁边（与 Factory 同文件），
 * commands 只依赖这个窄接口，不依赖 Factory 的任何实现细节。
 */
export interface WindowFactory {
  createWindow(params: Record<string, any>): { windowId: number }
  createWebview(params: Record<string, any>): { webviewId: number }
}

/**
 * 窗口 / 页面工厂（骨架）。
 *
 * 这是 main.cjs 里唯一无法靠拆文件消掉的部分：创建过程本身就要跨层——
 * 建原生窗口、装配 webview、装策略、注册进注册表、接事件转发。
 * 所以它单独存在，并显式接收所有依赖，而不是去摸全局 state。
 *
 * 两条必须保留的约束：
 * 1. popup：Rust 在阻塞的 setWindowOpenHandler 里嵌套下发 window.create，
 *    这里要把结果放进 popups，等策略侧 attach。
 * 2. 主视图：visible:false 时先建不显示，等 window.ready 再 reveal。
 */

export interface FactoryDependencies {
  readonly windows: Windows
  readonly webviews: Webviews
  readonly policies: Policies
  readonly protocols: Protocols
  readonly transport: Transport
  readonly environment: WindowEnvironment
  /** request id → 阻塞期间建好的 popup，由策略侧消费。 */
  readonly popups: Map<number, PopupPlan>
}

type Params = Record<string, any>

export class Factory implements WindowFactory {
  constructor(private readonly deps: FactoryDependencies) {}

  createWindow(params: Params): { windowId: number } {
    // TODO: Implement createWindow（含 newWindowRequest 分支：建完要写进 deps.popups）
    throw new Error('not implemented')
  }

  createWebview(params: Params): { webviewId: number } {
    // TODO: Implement createWebview
    throw new Error('not implemented')
  }

  /** 新窗口的 session 由 Rust 指定（incognito / partition / dataDirectory）。 */
  private sessionFor(params: Params): Session {
    // TODO: Implement sessionFor
    throw new Error('not implemented')
  }

  /**
   * 把 Window 的领域事件映射成线协议。事件名只在这里出现一次：
   * closeRequested → window.closeRequested、menuCommand → menu.command、
   * themeChanged → window.themeChanged。
   */
  private wireWindow(window: Window): void {
    // TODO: Implement wireWindow
  }

  /** 页面事件：navigation / load / title / crash / close。 */
  private wireWebview(webview: Webview): void {
    // TODO: Implement wireWebview
  }
}
