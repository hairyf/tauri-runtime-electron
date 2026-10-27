'use strict'

/**
 * Mutable Electron runtime state. Keeping the registries together makes their
 * ownership and cleanup visible to every command strategy.
 */
/** @type {import('./runtime.d.ts', { with: { 'resolution-mode': 'import' } }).RuntimeState} */
const state = {
  openingDevtools: new WeakMap(),
  localhostProtocolSessions: new WeakSet(),
  policySessions: new WeakSet(),
  navigationRequests: new Map(),
  nextNavigationRequest: 0,
  protocolRequests: new Map(),
  nextProtocolId: 1,
  registeredSchemes: new Set(),
  socket: undefined,
  windows: new Map(),
  closeAllowed: new Set(),
  closableState: new WeakMap(),
  maximizableState: new WeakMap(),
  minimizableState: new WeakMap(),
  sizeConstraints: new WeakMap(),
  windowCursorState: new WeakMap(),
  cursorCssKey: new WeakMap(),
  windowConfigs: new WeakMap(),
  fullscreenBounds: new WeakMap(),
  webviews: new Map(),
  webviewByContents: new Map(),
  childWebviews: new Map(),
  mainViews: new WeakMap(),
  pendingPopups: new Map(),
  activePopupRequest: undefined,
  activePopupContents: undefined,
  nextPopupRequest: 0,
  handlerPolicy: new Map(),
  downloadRequests: new Map(),
  permissionRequests: new Map(),
  nextDownloadRequest: 0,
  nextPermissionRequest: 0,
  customSessions: new Set(),
  nextIncognitoSession: 0,
  windowThemeState: new WeakMap(),
  themeDebuggers: new WeakSet(),
  synchronousMethods: new Set([
    'window.create', 'protocol.register', 'protocol.respond', 'webview.url',
    'webview.newWindowResponse', 'webview.navigationResponse', 'webview.downloadResponse', 'webview.permissionResponse',
    'window.nativeHandle', 'window.setMenu', 'window.setTitle', 'window.geometry', 'window.icon',
    'window.size', 'window.position', 'window.minSize', 'window.maxSize', 'window.sizeConstraints',
    'window.resizable', 'window.maximizable', 'window.minimizable', 'window.closable',
    'window.maximize', 'window.unmaximize', 'window.minimize', 'window.simpleFullscreen',
    'window.alwaysOnTop', 'window.setParentWindow', 'window.skipTaskbar', 'window.focusable',
    'window.contentProtected', 'window.center', 'window.requestAttention',
    'window.show', 'window.hide', 'window.focus', 'window.close', 'window.destroy',
    'window.visibleOnAllWorkspaces', 'window.enabled', 'window.enabledQuery',
    'window.progressBar', 'window.shadow', 'window.decorations', 'window.badgeCount',
    'window.overlayIcon', 'window.backgroundColor', 'window.backgroundMaterial', 'window.ignoreCursorEvents',
    'window.displays', 'window.cursorPosition', 'window.currentDisplay', 'window.displayForPoint', 'window.trafficLight',
  ]),
}

module.exports = { state }
