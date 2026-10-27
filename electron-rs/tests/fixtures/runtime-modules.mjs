import { readFileSync } from 'node:fs'
import path from 'node:path'
import { builtinModules, createRequire } from 'node:module'
import vm from 'node:vm'
import { EventEmitter } from 'node:events'
import { fileURLToPath } from 'node:url'

const assets = fileURLToPath(new URL('../../assets/', import.meta.url))
const builtin = createRequire(import.meta.url)

// Load only generated CJS files below assets; native modules remain fixture mocks.
export function runtimeModules(context, fallback, parentFilename = path.join(assets, 'main.cjs'), IPC) {
  const cache = new Map()
  const withinAssets = (filename) => {
    const relative = path.relative(assets, filename)
    return relative !== '' && relative !== '..' && !relative.startsWith(`..${path.sep}`) && !path.isAbsolute(relative)
  }
  const load = (filename) => {
    const resolved = path.resolve(filename)
    if (!withinAssets(resolved) || path.extname(resolved) !== '.cjs') throw new Error(`runtime module outside assets: ${filename}`)
    if (resolved === path.join(assets, 'socket', 'ipc.cjs') && IPC) return { IPC }
    if (cache.has(resolved)) return cache.get(resolved).exports
    const module = { exports: {} }
    cache.set(resolved, module)
    const localRequire = (specifier) => {
      if (specifier.startsWith('node:') || builtinModules.includes(specifier)) return fallback(specifier)
      if (!specifier.startsWith('.')) {
        if (path.isAbsolute(specifier) && withinAssets(path.resolve(specifier)) && path.extname(specifier) === '.node') return fallback(specifier)
        return fallback(specifier)
      }
      const child = path.resolve(path.dirname(resolved), specifier)
      return load(path.extname(child) ? child : `${child}.cjs`)
    }
    const source = readFileSync(resolved, 'utf8')
    const execute = vm.runInContext(`(function(module, exports, require, __filename, __dirname) {\n${source}\n})`, context, { filename: resolved })
    execute(module, module.exports, localRequire, resolved, path.dirname(resolved))
    return module.exports
  }
  const resolve = (specifier, parent = parentFilename) => {
    if (!specifier.startsWith('.')) return fallback(specifier)
    const filename = path.resolve(path.dirname(parent), specifier)
    return load(path.extname(filename) ? filename : `${filename}.cjs`)
  }
  resolve.load = load
  resolve.cache = cache
  return resolve
}

export function createRuntime(context, electron, options = {}) {
  const ipc = options.ipc ?? Object.assign(new EventEmitter(), { closed: false, send: options.send ?? ((message) => context.send?.(message)), respond: options.respond ?? (() => {}), request: options.request ?? (() => ({ action: 'deny' })), close() { if (this.closed) return; this.closed = true; this.emit('close') } })
  const nativeRequire = (name) => name === 'electron' ? electron : options.require?.(name) ?? builtin(name)
  const load = runtimeModules(context, (name) => {
    if (name !== 'node:fs') return nativeRequire(name)
    const fs = nativeRequire(name)
    return { ...fs, readFileSync: (file, ...args) => file === 0 ? '0123456789abcdef'.repeat(4) + '\n' : fs.readFileSync(file, ...args) }
  }, options.filename, class { constructor() { return ipc } })
  context.process ??= {}
  context.process.argv ??= ['node', 'main.cjs', '--port', '1']
  context.process.env ??= {}
  const { windows, webviews, downloads, popups, transport, environment, policies, protocols, factory, commands } = load.load(path.join(assets, 'main.cjs'))
  // Cases change the injected OS to exercise platform branches, never the host OS.
  Object.defineProperty(environment, 'platform', { get: () => context.process.platform })
  const handle = (method, params = {}) => {
    const command = commands[method]
    if (!command) throw new Error(`unknown method ${method}`)
    return command(params)
  }
  const state = {
    windows: { get: (id) => windows.get(id).native, has: (id) => Boolean(windows.peek(id)), get size() { return windows.all().length } },
    webviews: { get: (id) => webviews.peek(id), has: (id) => Boolean(webviews.peek(id)), get size() { return webviews.all().length } },
    childWebviews: { get: (id) => webviews.peek(id), has: (id) => Boolean(webviews.peek(id)), get size() { return webviews.all().filter((page) => !page.isMain).length }, values: () => webviews.all().filter((page) => !page.isMain).map((page) => ({ view: page.view })) },
    mainViews: { get: (native) => { const page = webviews.all().find((item) => item.window.native === native && item.isMain); return page ? { view: page.view } : undefined }, has: (native) => webviews.all().some((page) => page.window.native === native && page.isMain) },
    webviewByContents: { get: (id) => webviews.forContents(id)?.id, has: (id) => Boolean(webviews.forContents(id)) },
    handlerPolicy: { get size() { return webviews.all().filter((page) => page.webviewPolicy).length } },
    openingDevtools: { has: (contents) => Boolean(webviews.all().find((page) => page.contents === contents)?.openingDevtools) },
    get socket() { return ipc },
    set socket(value) { ipc.closed = !value || value.closed === true },
    get navigationRequests() { return policies.navigations },
    get permissionRequests() { return policies.permissions },
    sessionFor: (params) => factory.sessionFor(params),
    installPolicyHandlers: (target) => policies.install(target),
  }
  return { load, ipc, windows, webviews, downloads, popups, transport, environment, policies, protocols, factory, commands, state, handle }
}
