import { readFileSync } from 'node:fs'
import vm from 'node:vm'

// Keep production CJS modules in the fixture's VM (platform, timers and globals).
export function runtimeModules(context, fallback) {
  const cache = new Map()
  const require = (name) => {
    if (name !== './internal.cjs' && name !== './commands.cjs') return fallback(name)
    if (!cache.has(name)) {
      const module = { exports: {} }
      cache.set(name, module)
      const file = new URL(`../../assets/${name.slice(2)}`, import.meta.url)
      const execute = vm.runInContext(`(function(module, exports, require) {\n${readFileSync(file, 'utf8')}\n})`, context, { filename: file.pathname })
      execute(module, module.exports, require)
    }
    return cache.get(name).exports
  }
  return require
}
