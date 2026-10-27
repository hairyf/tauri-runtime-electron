import type { BaseWindowConstructorOptions } from 'electron'

/**
 * appkit.node 的原生适配（骨架）。
 *
 * 之所以单独一个文件：这几件事 Electron 的公开 API 要么没有、要么语义不同——
 * - 装饰/标题栏样式：Electron 在构造时就缓存 has_frame，改不回来
 * - 可最大化：Electron 的 zoom 同时覆盖全屏，而 Tauri 把两者分开
 * - 红绿灯位置：只有 macOS 的 NSWindow 有
 * 所以渲染侧的 Window 只声明需要什么（WindowEnvironment.appKit），实现留在这里。
 */

/** appkit.node 暴露的能力。仅 macOS 存在；其它平台不应调用。 */
export interface AppKit {
  configureStyle(handle: Buffer, decorated: boolean, style: string, hiddenTitle: boolean): void
  isDecorated(handle: Buffer): boolean
  setMaximizable(handle: Buffer, enabled: boolean): void
  isMaximizable(handle: Buffer): boolean
  setTrafficLightPosition(handle: Buffer, x: number, y: number): void
}

let loaded: AppKit | undefined

/**
 * 惰性加载并缓存。位置固定在打包产物同目录（assets/appkit.node），
 * 不能用 __dirname 之外的路径，否则 asar 里会取不到。
 */
export function appkit(): AppKit {
  // TODO: Implement require(path.join(__dirname, 'appkit.node')) 并缓存
  throw new Error('not implemented')
}

export interface TitleBarConfig {
  titleBarStyle?: string
  titleBarHidden?: boolean
  hiddenTitle?: boolean
}

/**
 * Tauri 的 titleBarStyle → Electron 的构造参数。
 *
 * macOS 一律返回 'default'：Electron 在构造时就固定了 has_frame，
 * 之后再让 AppKit 套 Tauri 的样式；若这里返回 hidden，Electron 会接管
 * 无边框按钮代理，和 AppKit 的结果互相打架。
 */
export function titleBarStyle(
  platform: NodeJS.Platform,
  config: TitleBarConfig,
): BaseWindowConstructorOptions['titleBarStyle'] {
  // TODO: Implement titleBarStyle
  throw new Error('not implemented')
}
