import type { App, NativeTheme } from 'electron'
import type { CommandTable, Params } from '../socket/table'
import { cursorPosition, displayAtPoint, displays } from './display'
import type { DisplaySource } from './display'
import type { WindowFactory } from './factory'
import type { Policies } from './policy'
import type { Protocols } from './protocol'
import type { CookieInit, Webview, Webviews } from './webview'
import type { Window, Windows } from './window'

/**
 * 命令表：协议字符串 → 领域对象调用。
 *
 * 这里只有 adapter，没有逻辑：
 * - 解析 id → 领域对象（找不到就抛错，语义由 get/peek 决定）
 * - 调一个方法，返回它的结果
 * 任何 if/平台判断/状态写入都应该在 Window / Webview / Policies 里，
 * 否则 state 式耦合会从 commands 长回来。
 *
 * 入参形态由线协议决定，只能宽松处理（Params），类型定义在 rpc/table.ts。
 */
export interface CommandContext {
  readonly windows: Windows
  readonly webviews: Webviews
  readonly policies: Policies
  readonly protocols: Protocols
  readonly display: DisplaySource
  readonly app: App
  readonly nativeTheme: NativeTheme
  /** 窗口与页面的创建涉及策略装配，由组合根提供。 */
  readonly factory: WindowFactory
  /** 回复阻塞中的策略请求（webview.newWindowResponse）。 */
  readonly respond: (id: number, response: unknown) => void
}

export function createCommands(ctx: CommandContext): CommandTable {
  return {
    ...windowCommands(ctx),
    ...webviewCommands(ctx),
    ...protocolCommands(ctx),
    ...appCommands(ctx),
  }
}

function windowCommands(ctx: CommandContext): CommandTable {
  const window = (params: Params): Window => ctx.windows.get(params.windowId)
  /** show/hide/focus 等命令允许窗口已消失，与 Rust 的尽力而为语义一致。 */
  const maybe = (params: Params): Window | undefined => ctx.windows.peek(params.windowId)

  return {
    'window.create': (p) => ctx.factory.createWindow(p),
    'window.ready': (p) => { window(p).reveal(); return null },
    'window.setTitle': (p) => { maybe(p)?.setTitle(p.title); return null },
    'window.geometry': (p) => window(p).geometry(),
    'window.size': (p) => { window(p).setSize(p); return null },
    'window.position': (p) => { window(p).setPosition(p); return null },
    'window.minSize': (p) => { window(p).setMinSize(p); return null },
    'window.maxSize': (p) => { window(p).setMaxSize(p); return null },
    'window.sizeConstraints': (p) => { window(p).setConstraints(p.constraints ?? p); return null },
    'window.resizable': (p) => { window(p).setResizable(p.resizable); return null },
    'window.maximizable': (p) => { window(p).setMaximizable(p.maximizable !== false); return null },
    'window.minimizable': (p) => { window(p).setMinimizable(p.minimizable !== false); return null },
    'window.closable': (p) => { window(p).setClosable(p.closable !== false); return null },
    'window.maximize': (p) => { window(p).maximize(); return null },
    'window.unmaximize': (p) => { window(p).unmaximize(); return null },
    'window.minimize': (p) => { window(p).minimize(); return null },
    'window.unminimize': (p) => window(p).unminimize(),
    'window.fullscreen': (p) => window(p).setFullscreen({ fullscreen: Boolean(p.fullscreen), x: p.x, y: p.y }),
    'window.alwaysOnTop': (p) => { window(p).setAlwaysOnTop(p.alwaysOnTop); return null },
    'window.cursor': (p) => window(p).setCursor({ cursor: p.cursor }),
    'window.setCursorIcon': (p) => window(p).setCursor({ cursor: p.cursor }),
    'window.setCursorVisible': (p) => window(p).setCursor({ visible: p.visible }),
    'window.setParentWindow': (p) => {
      const child = window(p)
      child.setParent(p.parentId == null ? null : ctx.windows.get(p.parentId))
      return null
    },
    'window.setMenu': (p) => { window(p).setMenu(p.menu); return null },
    'window.skipTaskbar': (p) => { window(p).setSkipTaskbar(p.skip); return null },
    'window.focusable': (p) => { window(p).setFocusable(p.focusable); return null },
    'window.contentProtected': (p) => { window(p).setContentProtection(p.protected); return null },
    'window.center': (p) => { window(p).center(); return null },
    'window.requestAttention': (p) => { window(p).requestAttention(Boolean(p.flash)); return null },
    'window.show': (p) => { maybe(p)?.show(); return null },
    'window.hide': (p) => { maybe(p)?.hide(); return null },
    'window.focus': (p) => { maybe(p)?.focus(); return null },
    'window.nativeHandle': (p) => window(p).nativeHandle(),
    'window.close': (p) => { maybe(p)?.close(); return null },
    'window.destroy': (p) => { maybe(p)?.destroy(); return null },
    'window.visibleOnAllWorkspaces': (p) => { window(p).setVisibleOnAllWorkspaces(p.value); return null },
    'window.icon': (p) => { window(p).setIcon(p.icon); return null },
    'window.enabled': (p) => { window(p).setEnabled(p.enabled); return null },
    'window.enabledQuery': (p) => window(p).isEnabled(),
    'window.simpleFullscreen': (p) => { window(p).setSimpleFullscreen(p.fullscreen ?? p.enable); return null },
    'window.progressBar': (p) => { window(p).setProgressBar(p.state, Number(p.progress) || 0); return null },
    'window.shadow': (p) => { window(p).setShadow(p.value); return null },
    'window.decorations': (p) => { window(p).setDecorations((p.value ?? p.decorations) !== false); return null },
    'window.badgeCount': (p) => { ctx.app.setBadgeCount(Number(p.count) || 0); return null },
    'window.overlayIcon': (p) => { window(p).setOverlayIcon(p.icon, p.description || ''); return null },
    'window.backgroundColor': (p) => { window(p).setBackgroundColor(p.color); return null },
    'window.backgroundMaterial': (p) => { window(p).setBackgroundMaterial(p.material); return null },
    'window.theme': (p) => window(p).setTheme(p.theme),
    'window.ignoreCursorEvents': (p) => { window(p).ignoreCursorEvents(p.ignore); return null },
    'window.themeQuery': async (p) => ({ theme: (await window(p).readTheme()).theme }),
    'window.displays': () => displays(ctx.display),
    'window.cursorPosition': () => cursorPosition(ctx.display),
    'window.currentDisplay': (p) => window(p).currentDisplay(),
    'window.displayForPoint': (p) => displayAtPoint(ctx.display, p.x, p.y),
    'window.titleBarStyle': (p) => { window(p).setTitleBarStyle(p.style); return null },
    'window.trafficLight': (p) => { window(p).setTrafficLight(p); return null },
  }
}

function webviewCommands(ctx: CommandContext): CommandTable {
  const webview = (params: Params): Webview => ctx.webviews.get(params.webviewId)

  return {
    'webview.create': (p) => ctx.factory.createWebview(p),
    'webview.url': (p) => webview(p).contents.getURL(),
    'webview.eval': (p) => webview(p).evaluate(p.script),
    // 策略应答：id 是策略请求自己的编号，不是这条 RPC 的编号。
    'webview.newWindowResponse': (p) => { ctx.respond(p.id, p.response); return null },
    'webview.navigationResponse': (p) => { ctx.policies.answerNavigation(p.id, p.allow === true); return null },
    'webview.downloadResponse': (p) => {
      ctx.policies.answerDownload(p.id, { allow: p.allow === true, destination: p.destination })
      return null
    },
    'webview.permissionResponse': (p) => { ctx.policies.answerPermission(p.id, p.response); return null },
    'webview.navigate': (p) => { webview(p).navigate(p.url); return null },
    'webview.reload': (p) => { webview(p).reload(); return null },
    'webview.setAutoResize': (p) => { webview(p).setAutoResize(Boolean(p.autoResize)); return null },
    'webview.goBack': (p) => { webview(p).goBack(); return null },
    'webview.canGoBack': (p) => webview(p).canGoBack(),
    'webview.goForward': (p) => { webview(p).goForward(); return null },
    'webview.canGoForward': (p) => webview(p).canGoForward(),
    'webview.openDevtools': (p) => webview(p).openDevtools(),
    'webview.closeDevtools': (p) => { webview(p).closeDevtools(); return null },
    'webview.isDevtoolsOpen': (p) => webview(p).isDevtoolsOpen(),
    'webview.setZoom': (p) => { webview(p).setZoom(p.scaleFactor); return null },
    'webview.reparent': (p) => { webview(p).reparent(ctx.windows.get(p.windowId)); return null },
    'webview.bounds': (p) => webview(p).snapshot(),
    'webview.setBounds': (p) => { webview(p).setBounds(p); return null },
    'webview.visibility': (p) => { webview(p).setVisible(Boolean(p.visible)); return null },
    'webview.backgroundColor': (p) => { webview(p).setBackgroundColor(p.color); return null },
    'webview.focus': (p) => { webview(p).focus(); return null },
    'webview.print': (p) => webview(p).print(),
    'webview.close': (p) => { webview(p).close(); return null },
    'webview.cookies': (p) => webview(p).cookies({ url: p.url }),
    'webview.setCookie': (p) => webview(p).setCookie(p as CookieInit),
    'webview.deleteCookie': (p) => webview(p).deleteCookie({ url: p.url, name: p.name }),
    'webview.clearBrowsingData': (p) => webview(p).clearBrowsingData(),
  }
}

function protocolCommands(ctx: CommandContext): CommandTable {
  return {
    'protocol.register': (p) => { ctx.protocols.register(p.scheme); return null },
    'protocol.respond': (p) => { ctx.protocols.answer(p.id, p); return null },
  }
}

function appCommands(ctx: CommandContext): CommandTable {
  const mac = (): boolean => ctx.display.platform === 'darwin'

  return {
    'app.badgeLabel': (p) => { if (mac()) ctx.app.dock?.setBadge(p.label ?? ''); return null },
    'app.focus': (p) => { ctx.app.focus({ steal: p.steal === true }); return null },
    'app.show': () => { if (mac()) ctx.app.show(); return null },
    'app.hide': () => { if (mac()) ctx.app.hide(); return null },
    'app.activationPolicy': (p) => { if (mac()) ctx.app.setActivationPolicy(p.policy); return null },
    'app.dockVisibility': (p) => {
      if (mac()) {
        if (p.visible) ctx.app.dock?.show()
        else ctx.app.dock?.hide()
      }
      return null
    },
    'app.theme': (p) => {
      ctx.nativeTheme.themeSource = p.theme === 'dark' ? 'dark' : p.theme === 'light' ? 'light' : 'system'
      return null
    },
  }
}
