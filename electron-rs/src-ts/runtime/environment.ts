import { app, nativeTheme, screen } from 'electron'
import type { WebContents } from 'electron'
import { appkit } from './native/appkit'
import { applyCursor } from './native/cursor'
import { applyTheme } from './native/theme'
import type { Webviews } from './webview'
import type { Window, WindowEnvironment } from './window'

/**
 * Window 的原生接缝。
 *
 * 全仓只有这里同时知道三样东西：Electron 的全局单例、native 适配器、
 * 页面注册表。Window 自己只看到 WindowEnvironment 这个接口，
 * 所以 window.ts 里不会出现 `app` / `screen` 之类的全局引用。
 *
 * 做成类而不是字面量，是因为 contents() 需要持有页面注册表；
 * 用类字段表达"常量"、用方法表达"每次现查"，边界一眼能看出来。
 */
export class Environment implements WindowEnvironment {
  readonly screen = screen
  readonly platform = process.platform
  readonly nativeTheme = nativeTheme
  readonly app = app
  readonly appKit = appkit
  /** 原生下发原语：与 this 无关，直接引用。 */
  readonly applyCursor = applyCursor
  readonly applyTheme = applyTheme

  constructor(private readonly webviews: Webviews) {}

  /** 页面所有权是活的：同一个窗口的页面会随 reparent / 关闭变化，不能缓存。 */
  contents(window: Window): WebContents[] {
    return this.webviews.of(window).map((webview) => webview.contents)
  }
}
