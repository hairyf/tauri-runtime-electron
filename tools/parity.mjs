// Compares this crate against `tauri-runtime-cef`, file by file.
//
// Three tiers, because not every difference is a parity failure:
//
//   1. FILES      every reference file must have a counterpart here.
//   2. API        every item the reference makes public must exist here under the
//                 rename map. This is the contract: an application written against
//                 the CEF runtime must compile against this one.
//   3. TRAITS     every member of every `tauri-runtime` trait the reference
//                 implements must be implemented here too, with the same gating.
//
// CEF's own interface impls (`impl DisplayHandler::on_title_change`) have no
// Electron counterpart by construction — Electron pushes events over a socket
// instead of calling back into C callbacks — so they are reported separately and
// never count as failures.
//
// Usage: node tools/parity.mjs [--json] [--all]
//   --json  machine-readable result
//   --all   also list the informational CEF-interface and internal-helper deltas
import { existsSync, readFileSync, readdirSync, statSync } from 'node:fs'
import { join, relative } from 'node:path'

const ROOT = new URL('..', import.meta.url).pathname.replace(/^\/([A-Za-z]:)/, '$1')
const REFERENCE = join(ROOT, 'source/tauri/crates/tauri-runtime-cef/src')
const TARGET = join(ROOT, 'src')

/** CEF-specific names and their Electron counterparts. Everything else is identical. */
const RENAMES = [
  ['CefWebviewAttributes', 'ElectronWebviewAttributes'],
  ['CefWebviewDispatcher', 'ElectronWebviewDispatcher'],
  ['CefWindowDispatcher', 'ElectronWindowDispatcher'],
  ['CefRuntimeHandle', 'ElectronRuntimeHandle'],
  ['CefRuntime', 'ElectronRuntime'],
  ['CefInitScript', 'ElectronInitScript'],
  ['AsCefWebviewAttributes', 'AsElectronWebviewAttributes'],
  ['AsCefWebviewDispatcher', 'AsElectronWebviewDispatcher'],
  ['AsCefWindowOpener', 'AsElectronWindowOpener'],
  ['WebviewWindowBuilderCefExt', 'WebviewWindowBuilderElectronExt'],
  ['WebviewBuilderCefExt', 'WebviewBuilderElectronExt'],
  ['WebviewCefExt', 'WebviewElectronExt'],
  ['WinitCefApp', 'TauriElectronApp'],
  ['TauriCefApp', 'TauriElectronApp'],
  ['TauriCefHelperApp', 'TauriElectronHelperApp'],
  ['run_cef_helper_process', 'run_electron_helper_process'],
  ['cef_entry_point', 'electron_entry_point'],
  ['cef_api_version', 'electron_api_version'],
  ['cef_impl', 'electron_impl'],
  ['TauriCef', 'TauriElectron'],
  // Method names that carry the engine's name, not just the types that do.
  ['as_cef_webview_attributes_mut', 'as_electron_webview_attributes_mut'],
  ['as_cef_webview_attributes', 'as_electron_webview_attributes'],
  ['as_cef_webview_dispatcher', 'as_electron_webview_dispatcher'],
  ['as_cef_window_opener', 'as_electron_window_opener'],
  ['with_cef_webview', 'with_electron_webview'],
  ['from_cef', 'from_electron'],
  ['Cef', 'Electron'],
]

/** Applies the rename map longest-first, so `CefWebviewAttributes` wins over `Cef`. */
function renameName(name) {
  for (const [from, to] of RENAMES) {
    if (name === from) return to
    if (name.startsWith(from)) return to + name.slice(from.length)
  }
  return name
}

/**
 * Renames one surface item, keeping its `kind ` prefix and any `module::` or
 * `Trait::` qualifier. The crate re-exports the bridge crate where the reference
 * re-exports `cef`, so those two `use` lines have their own answers.
 */
function rename(item) {
  if (item === 'use cef') return 'use electron_rs'
  // The reference re-exports the attribute macro from `tauri-macros`; this crate
  // defines it itself, because `tauri-macros` has no Electron entry point.
  if (item === 'use tauri_macros') return 'use electron_entry_point'
  if (item === 'tauri_macros::cef_entry_point') return 'macros::electron_entry_point'

  // `kind rest` — the declaration kind is not renamed, everything after it is.
  const space = item.indexOf(' ')
  if (space > 0) {
    const kind = item.slice(0, space)
    const rest = item.slice(space + 1)
    if (/^(fn|struct|enum|trait|type|const|static|mod|use)$/.test(kind)) {
      return `${kind} ${rename(rest)}`
    }
  }

  // `module::Item` or `Trait::member` — rename both halves.
  const separator = item.indexOf('::')
  if (separator >= 0) {
    return `${renameName(item.slice(0, separator))}::${rename(item.slice(separator + 2))}`
  }

  return renameName(item)
}

function renamePath(path) {
  for (const [from, to] of RENAMES) {
    if (path === from || path.startsWith(`${from}/`)) return to + path.slice(from.length)
  }
  return path
}

/** Files this crate needs and the reference does not, each with its reason. */
const ALLOWED_EXTRA_FILES = new Map([
  [
    'unsupported.rs',
    'the reference can do everything CEF exposes; this runtime needs one place to record what Electron cannot',
  ],
  [
    'command_bridge.rs',
    'the running Electron client, for the work no tauri-runtime call can express',
  ],
])

/** CEF interface traits. An `impl` of one of these has no Electron counterpart. */
const CEF_INTERFACES = new Set([
  'ApplicationHandler',
  'BrowserProcessHandler',
  'Client',
  'CommandHandler',
  'ContextMenuHandler',
  'DisplayHandler',
  'DownloadHandler',
  'DragHandler',
  'EventHandler',
  'FrameHandler',
  'KeyboardHandler',
  'LifeSpanHandler',
  'LoadHandler',
  'PermissionHandler',
  'RenderProcessHandler',
  'RenderHandler',
  'RequestContextHandler',
  'RequestHandler',
  'ResourceRequestHandler',
  'SchemeHandlerFactory',
  'V8Handler',
  'WrapClient',
  'WrapCommandHandler',
  'WrapContextMenuHandler',
  'WrapDisplayHandler',
  'WrapDownloadHandler',
  'WrapDragHandler',
  'WrapFrameHandler',
  'WrapKeyboardHandler',
  'WrapLifeSpanHandler',
  'WrapLoadHandler',
  'WrapPermissionHandler',
  'WrapRenderHandler',
  'WrapResourceRequestHandler',
])

/** The `tauri-runtime` traits the reference implements, and this crate must too. */
const RUNTIME_TRAITS = new Set([
  'WindowDispatch',
  'WebviewDispatch',
  'Runtime',
  'RuntimeHandle',
  'RuntimeInitAttrs',
  'WindowBuilder',
  'WindowBuilderBase',
  'EventLoopProxy',
])

function walk(dir) {
  return readdirSync(dir).flatMap((entry) => {
    const path = join(dir, entry)
    return statSync(path).isDirectory() ? walk(path) : path.endsWith('.rs') ? [path] : []
  })
}

const key = (root, path) => relative(root, path).replaceAll('\\', '/')

/**
 * Reads one file into the shapes the comparison needs:
 * `api` (items visible outside the crate), `internal` (`pub(crate)`/`pub(super)`,
 * visible only inside it), `traits` (members of the runtime traits this file
 * implements) and `cef` (members of CEF interface impls).
 */
function read(file) {
  const lines = readFileSync(file, 'utf8').split('\n')
  const api = new Set()
  const internal = new Set()
  const traits = new Set()
  const cef = new Set()
  const helpers = new Set()

  // `pub(crate)` and `pub(super)` are crate-internal: an application cannot name
  // them, so they are not part of the API contract.
  const DECL =
    /^\s*(pub(?:\([^)]*\))?)\s+(?:async\s+)?(?:unsafe\s+)?(fn|struct|enum|trait|type|const|static|mod|use)\s+([A-Za-z_]\w*)/
  for (const line of lines) {
    const match = DECL.exec(line)
    if (!match) continue
    const [, visibility, kind, name] = match
    const bucket = visibility === 'pub' ? api : internal
    bucket.add(`${kind} ${name}`)
  }

  /** Walks a brace-balanced block starting at `start`, yielding its `fn` members. */
  const members = (start) => {
    const found = []
    let depth = 0
    let started = false
    for (let cursor = start; cursor < lines.length; cursor++) {
      for (const char of lines[cursor]) {
        if (char === '{') {
          depth++
          started = true
        } else if (char === '}') depth--
      }
      const member = /^\s*(pub(?:\([^)]*\))?\s+)?(?:async\s+)?(?:unsafe\s+)?(?:const\s+)?fn\s+([A-Za-z_]\w*)/.exec(
        lines[cursor],
      )
      if (started && member && cursor !== start) found.push([member[2], member[1] === 'pub '])
      if (started && depth === 0) break
    }
    return found
  }

  // `pub trait Name { … }` — the trait's own members.
  const TRAIT = /^\s*pub\s+trait\s+([A-Za-z_]\w*)/
  for (let index = 0; index < lines.length; index++) {
    const header = TRAIT.exec(lines[index])
    if (!header) continue
    for (const [member] of members(index)) api.add(`trait ${header[1]}::${member}`)
  }

  // `impl [<…>] [Trait for] Type { … }`
  const IMPL = /^\s*impl(?:<[^>]*>)?\s+([A-Za-z_][\w:<>, ]*?)(?:\s+for\s+([A-Za-z_][\w:<>, ]*?))?\s*(?:where|\{)/
  for (let index = 0; index < lines.length; index++) {
    const header = IMPL.exec(lines[index])
    if (!header) continue
    const trait = header[2] ? header[1].trim().replace(/<.*$/, '') : null
    for (const [member] of members(index)) {
      if (trait && RUNTIME_TRAITS.has(trait)) traits.add(`${trait}::${member}`)
      else if (trait && CEF_INTERFACES.has(trait)) cef.add(`${trait}::${member}`)
      else helpers.add(member)
    }
  }

  return { api, internal, traits, cef, helpers }
}

/**
 * The items an application can actually name, read from `lib.rs`.
 *
 * `pub use module::Item` exports one item; `pub use module::*` exports every `pub`
 * item of that module's file. Everything else in `src/` is crate-internal, so a
 * difference there is an implementation detail rather than an API gap.
 */
function exportedSurface(srcDir) {
  const lib = readFileSync(join(srcDir, 'lib.rs'), 'utf8')
  const exported = new Set()
  const globs = new Set()
  const USE = /^\s*pub\s+use\s+([A-Za-z_]\w*)::(\{[^}]*\}|\*|[A-Za-z_]\w*)\s*;/gm
  let match
  while ((match = USE.exec(lib))) {
    const [, module, rest] = match
    if (rest === '*') {
      globs.add(module)
      continue
    }
    for (const name of rest.replace(/[{}]/g, '').split(',')) {
      const item = name.trim().split(/\s+as\s+/).pop()
      if (item) exported.add(`${module}::${item}`)
    }
  }
  for (const module of globs) {
    const file = join(srcDir, `${module}.rs`)
    if (!existsSync(file)) continue
    for (const item of read(file).api) exported.add(`${module}::${item}`)
  }
  return { exported, globs }
}

function compareFiles() {
  const reference = new Map()
  for (const path of walk(REFERENCE)) reference.set(renamePath(key(REFERENCE, path)), path)
  const target = new Map()
  for (const path of walk(TARGET)) target.set(key(TARGET, path), path)
  return {
    reference,
    target,
    missing: [...reference.keys()].filter((path) => !target.has(path)).sort(),
    extra: [...target.keys()].filter((path) => !reference.has(path)).sort(),
  }
}

const files = compareFiles()
const report = {
  files: { missing: files.missing, extra: files.extra },
  api: [],
  traits: [],
  informational: [],
}

// Tier 2 is measured on the crate root, not per file: an application only sees
// what `lib.rs` exports, and a `pub` item in a private module is an implementation
// detail.
const referenceExports = exportedSurface(REFERENCE)
const targetExports = exportedSurface(TARGET)
const expectedExports = new Set([...referenceExports.exported].map(rename))
const missingExports = [...expectedExports].filter((item) => !targetExports.exported.has(item)).sort()
const extraExports = [...targetExports.exported].filter((item) => !expectedExports.has(item)).sort()
report.exports = { missing: missingExports, extra: extraExports }

for (const path of [...files.reference.keys()].sort()) {
  if (!files.target.has(path)) continue
  const reference = read(files.reference.get(path))
  const target = read(files.target.get(path))

  const expectedTraits = new Set([...reference.traits].map(rename))
  const missingTraits = [...expectedTraits].filter((item) => !target.traits.has(item)).sort()
  if (missingTraits.length) report.traits.push({ path, missing: missingTraits })

  const expectedApi = new Set([...reference.api].map(rename))
  const missingApi = [...expectedApi].filter((item) => !target.api.has(item)).sort()
  const extraApi = [...target.api].filter((item) => !expectedApi.has(item)).sort()
  const extraTraits = [...target.traits].filter((item) => !expectedTraits.has(item)).sort()
  if (missingApi.length || extraApi.length || extraTraits.length) {
    report.informational.push({ path, missingApi, extraApi, extraTraits })
  }
}

const json = process.argv.includes('--json')
const all = process.argv.includes('--all')

if (json) {
  console.log(JSON.stringify(report, null, 2))
} else {
  const line = (text) => console.log(text)
  line(`reference: ${files.reference.size} files`)
  line(`target:    ${files.target.size} files`)
  line('')

  line('1. FILES')
  if (files.missing.length) {
    line(`   MISSING (${files.missing.length}):`)
    for (const path of files.missing) line(`     - ${path}`)
  } else {
    line('   every reference file has a counterpart')
  }
  const unexplained = files.extra.filter((path) => !ALLOWED_EXTRA_FILES.has(path))
  if (unexplained.length) {
    line(`   UNEXPLAINED EXTRA (${unexplained.length}):`)
    for (const path of unexplained) line(`     + ${path}`)
  }
  for (const [path, reason] of ALLOWED_EXTRA_FILES) {
    if (files.extra.includes(path)) line(`   documented extra: ${path} — ${reason}`)
  }
  line('')

  line('2. EXPORTED API (what an application can name through the crate root)')
  if (!report.exports.missing.length) {
    line(`   every reference export exists here (${expectedExports.size} items)`)
  } else {
    line(`   ${report.exports.missing.length} missing of ${expectedExports.size}:`)
    for (const item of report.exports.missing) line(`     - ${item}`)
  }
  if (report.exports.extra.length) {
    line(`   ${report.exports.extra.length} extra:`)
    for (const item of report.exports.extra) line(`     + ${item}`)
  }
  line('')

  line('3. RUNTIME TRAIT MEMBERS')
  if (!report.traits.length) {
    line('   every reference trait member is implemented here')
  } else {
    const count = report.traits.reduce((total, entry) => total + entry.missing.length, 0)
    line(`   ${count} missing, across ${report.traits.length} files:`)
    for (const entry of report.traits) {
      line(`     ${entry.path}`)
      for (const item of entry.missing) line(`       - ${item}`)
    }
  }

  if (all) {
    line('')
    line('4. INFORMATIONAL (extra items this crate adds; CEF interface impls are excluded)')
    for (const entry of report.informational) {
      line(`   ${entry.path}`)
      for (const item of entry.extraApi) line(`     + ${item}`)
      for (const item of entry.extraTraits) line(`     + ${item}`)
    }
  }
}

const failed =
  files.missing.length > 0 ||
  files.extra.some((path) => !ALLOWED_EXTRA_FILES.has(path)) ||
  report.exports.missing.length > 0 ||
  report.traits.length > 0
if (failed) process.exitCode = 1
