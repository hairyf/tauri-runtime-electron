// Reports how much of `tauri-runtime` this crate actually implements, and how it
// compares with `tauri-runtime-wry`.
//
//   node tests/checks/coverage.mjs          human summary
//   node tests/checks/coverage.mjs --md     markdown to stdout
//   node tests/checks/coverage.mjs --write  rewrite docs/coverage.md
//   node tests/checks/coverage.mjs --check  exit 1 when docs/coverage.md is stale
//
// Both the traits and the wry reference are read from the checked-out submodule.
// This scan does not verify that its revision matches the gitlink or Cargo.lock.
// `git submodule update --init` populates a missing submodule.
import assert from 'node:assert/strict'
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { dirname } from 'node:path'
import { fileURLToPath } from 'node:url'

const repo = (path) => fileURLToPath(new URL(`../../${path}`, import.meta.url))
const TAURI = 'source/tauri/crates'
const RUNTIME_LIB = `${TAURI}/tauri-runtime/src/lib.rs`
const WRY_LIB = `${TAURI}/tauri-runtime-wry/src/lib.rs`
const DOC = 'docs/coverage.md'

const targets = [
  { trait: 'WindowDispatch', file: 'src/window.rs' },
  { trait: 'WebviewDispatch', file: 'src/webview.rs' },
  { trait: 'Runtime', file: 'src/runtime.rs' },
  { trait: 'RuntimeHandle', file: 'src/window_handle.rs' },
]

const read = (path) => {
  try {
    return readFileSync(repo(path), 'utf8').replace(/\r\n/g, '\n')
  } catch {
    console.error(`missing ${path}; run: git submodule update --init`)
    process.exit(2)
  }
}

/** Trait member names, in declaration order. Defaults live after `{}` bodies. */
function members(src, trait) {
  const lines = src.split('\n')
  const start = lines.findIndex((line) => line.includes(`pub trait ${trait}<T: UserEvent>`))
  if (start < 0) throw new Error(`trait ${trait} not found`)
  const names = []
  for (let i = start + 1; i < lines.length; i += 1) {
    if (/^}/.test(lines[i])) break
    const match = lines[i].match(/^ {2}(?:unsafe )?fn ([a-z_0-9]+)/)
    if (match) names.push(match[1])
  }
  return names
}

/** Every `fn` defined in the file, with its cfg gate, line and source body. */
function definitions(src) {
  const lines = src.split('\n')
  const result = new Map()
  for (let i = 0; i < lines.length; i += 1) {
    const match = lines[i].match(/^ {2}(?:unsafe )?fn ([a-z_0-9]+)/)
    if (!match) continue
    const name = match[1]
    if (result.has(name)) continue
    // A platform gate directly above the fn decides whether it exists at all.
    let cfg = null
    for (let j = i - 1; j >= 0 && /^\s*(#\[|$|\/\/)/.test(lines[j]); j -= 1) {
      const gate = lines[j].match(/^\s*#\[cfg\((.*)\)\]$/)
      if (gate) cfg = gate[1]
    }
    let depth = 0
    let started = false
    const body = []
    for (let j = i; j < lines.length; j += 1) {
      for (const ch of lines[j]) {
        if (ch === '{') {
          depth += 1
          started = true
        } else if (ch === '}') depth -= 1
      }
      body.push(lines[j])
      if (started && depth === 0) break
    }
    // `statements` drops attributes, so look for the Windows branch first.
    const raw = body.join('\n')
    result.set(name, {
      body: statements(raw),
      windows: /#\[cfg\(\s*windows\s*\)\]/.test(raw),
      cfg,
      line: i + 1,
    })
  }
  return result
}

/** Body without the signature, comments, attributes and braces. */
function statements(text) {
  return text
    .split('\n')
    .filter((line) => !/^\s*(\/\/|#\[|\/\/\/|\/\*)/.test(line))
    .join('\n')
    .replace(/^[^{]*\{/, '')
    .replace(/\}\s*$/, '')
}

const LITERAL = /^\s*(return\s+)?Ok\((true|false|None|\(\))\)\s*;?\s*$/

/** wired | error | stub | noop | warned | panic. */
function classify({ body }) {
  const code = body.trim()
  if (/\b(todo|unimplemented)!/.test(code)) return 'panic'
  if (/unsupported\(/.test(code)) return 'warned'
  if (/^(?:return\s+)?Err\(/.test(code)) return 'error'
  if (LITERAL.test(code)) return 'stub'
  if (!/\bself\b/.test(code) && !/\b(Self::|json!|ipc|context|dispatch)/.test(code)) return 'noop'
  return 'wired'
}

assert.equal(classify({ body: 'Err(crate::runtime::create_webview_err("unavailable".into()))' }), 'error')
assert.equal(classify({ body: 'self.dispatch("window.show", json!({}))' }), 'wired')
assert.equal(classify({ body: 'Ok(true)' }), 'stub')

const ours = Object.fromEntries(targets.map((t) => [t.file, definitions(read(t.file))]))
const wry = definitions(read(WRY_LIB))
const lib = read(RUNTIME_LIB)

const rows = targets.map((target) => ({
  ...target,
  members: members(lib, target.trait).map((name) => {
    const mine = ours[target.file].get(name)
    const reference = wry.get(name)
    return {
      name,
      kind: mine ? classify(mine) : 'missing',
      cfg: mine?.cfg ?? null,
      line: mine?.line ?? null,
      body: mine?.body ?? '',
      windows: mine?.windows ?? false,
      wry: reference ? classify(reference) : 'missing',
      wryCfg: reference?.cfg ?? null,
    }
  }),
}))

const KINDS = ['wired', 'error', 'stub', 'noop', 'warned', 'panic', 'missing']
const ICON = { wired: 'wired', error: 'error', stub: 'stub', noop: 'no-op', warned: 'warned', panic: 'panic', missing: 'missing' }
const counts = (members) =>
  Object.fromEntries(KINDS.map((kind) => [kind, members.filter((m) => m.kind === kind).length]))

const summaryTable = [
  '| trait | members | wired | error | stub | no-op | warned | panic | missing |',
  '| --- | --- | --- | --- | --- | --- | --- | --- | --- |',
  ...rows.map((r) => {
    const c = counts(r.members)
    return `| \`${r.trait}\` | ${r.members.length} | ${c.wired} | ${c.error} | ${c.stub} | ${c.noop} | ${c.warned} | ${c.panic} | ${c.missing} |`
  }),
]

const gaps = rows.flatMap((r) =>
  r.members
    .filter((m) => m.kind !== 'wired')
    .map(
      (m) =>
        `| \`${r.trait}::${m.name}\` | \`${ICON[m.kind]}\`${windowsPath(m)} | ${m.line ? `[${r.file}:${m.line}](${r.file}#L${m.line})` : '—'} | ${m.wry === 'wired' ? 'implemented' : `\`${m.wry}\``} |`,
    ),
)

/** A `warned` member with a `#[cfg(windows)]` branch still works on Windows. */
function windowsPath(member) {
  return member.windows ? ' — Windows path wired' : ''
}

const detail = rows.map((r) => [
  `### ${r.trait}`,
  '',
  `\`${r.file}\` — ${r.members.length} members.`,
  '',
  '| member | electron | tauri-runtime-wry | note |',
  '| --- | --- | --- | --- |',
  ...r.members.map((m) => {
    const notes = []
    if (m.cfg) notes.push(`\`#[cfg(${m.cfg})]\``)
    if (m.kind === 'error') notes.push('returns an explicit unsupported error')
    if (r.trait === 'WindowDispatch' && m.name === 'window_handle') notes.push('macOS returns HandleError::Unavailable; Win32/X11 IDs are not local native objects')
    if (m.kind === 'stub') notes.push('returns a literal')
    if (m.kind === 'noop') notes.push('never reaches Electron')
    if (m.kind === 'warned') notes.push(`logs once when the platform refuses${windowsPath(m)}`)
    if (m.kind === 'panic') notes.push('panics if called')
    if (m.wryCfg) notes.push(`wry is \`#[cfg(${m.wryCfg})]\``)
    return `| \`${m.name}\` | \`${ICON[m.kind]}\` | \`${m.wry}\` | ${notes.join('; ')} |`
  }),
  '',
].join('\n'))

const markdown = `# Runtime coverage

Generated by \`node tests/checks/coverage.mjs --write\` — do not edit by hand.
Traits come from \`${RUNTIME_LIB}\` and the reference column from \`${WRY_LIB}\`, both from the
checked-out \`source/tauri\` tree (Tauri 2.12). This scan does not verify that its
revision matches the parent gitlink or the registry packages selected by \`Cargo.lock\`.

This source scan lists implementation paths, not verified capability coverage.
\`cargo test\` includes a real-window smoke test through \`src/bin/smoke.rs\`; CI is
configured for Windows, macOS and Linux. Platform skips are printed. Configuration
is not evidence that a particular CI run passed. The tests do not verify every
method, native effect or official example. The separate \`pnpm check\` runner records
official-example assertions and screenshots; those screenshots still require visual
review. The official generic runtime traits have no native menu attachment or
window-effects delegation hooks. Tauri's \`muda\` and \`window-vibrancy\` paths are not
delegated to Electron. Native Tauri menus, callbacks/accelerators and popup attachment
remain unsupported. This crate's own window API delegates supported native effects
through its Tauri plugin; that extension is outside the trait-method counts below.
Upstream sources and copied framework files remain unchanged; no framework patch or
ACL bypass is required. Official generic \`setEffects\`/\`clearEffects\` calls are not
redirected to this extension or the inherent
\`ElectronWindowDispatcher::set_window_effects\` helper. CI runs the shared package
commands: \`pnpm run test:bridge:regressions\`, \`pnpm run test:checks\`,
\`pnpm run test:bridge:prepare\`, \`pnpm run test:bridge\`, \`pnpm run test:runtime\`,
\`pnpm run test:examples\` and \`pnpm run test:coverage\`, plus
\`cargo check --locked --all-targets --all-features\`. These configured checks are not
remote pass results. Bridge-owned protocol, layout, download and policy regressions
stay in \`electron-rs/tests\`; root \`tests/checks\` owns adapter/window API and example
checks. Rust assertions over Node VM observations and the example effects queue check
use native doubles. The separately invoked real bridge tests in
\`electron-rs/tests/bridge.rs\` exercise ElectronBuilder, loaded-page preload/protocol
IPC, events, selected native window/webview RPCs and shutdown. They are ignored by
default and require preparation plus a desktop; Linux sandbox permissions are set
after staging. None of these checks establishes every RPC, native effect or
three-platform desktop behavior.

| kind | meaning |
| --- | --- |
| wired | the body reaches Electron over IPC or a native ID; platform branches may still be unavailable |
| error | the body immediately returns an explicit unsupported error; callbacks are not invoked |
| stub | the body is a literal: \`Ok(())\`, \`Ok(true)\`, \`Ok(None)\` — it silently does nothing |
| no-op | the body never touches Electron or \`self\` — it silently does nothing |
| warned | the body calls \`runtime::unsupported\`: it does what the platform allows and logs once when it does not |
| panic | \`todo!()\`/\`unimplemented!()\` — loud, but only on the platform that compiles it |
| missing | no override, so the trait default applies |

## Support matrix

| target | CI configured | window-smoke configuration | notes |
| --- | --- | --- | --- |
| Windows | yes | desktop smoke and transparent-window check | library window effects support Acrylic/Mica/Tabbed on build 22621+; Win32 decoration/drag/cursor paths exist; WebView2 interop and Tauri native menus remain unsupported |
| macOS | yes | desktop smoke | \`window_handle\` returns \`HandleError::Unavailable\`; WKWebView data-store enumeration and removal error without callbacks; cursor warp via CoreGraphics |
| Linux | yes | desktop smoke under xvfb | missing local GTK windows/widgets return errors; \`display_handle\` reports \`Xlib(None)\`, so wgpu/GL interop cannot work; cursor warp logs a gap |
| Android / iOS | no | none | not supported desktop targets; mobile branches remain incomplete |

This table records intended CI jobs, not successful remote run results. Win32/X11
IDs do not grant local WebView2, GTK or AppKit/WKWebView access; native objects and
process-local pointers cannot be fabricated across the process boundary.
\`with_webview\` errors without invoking its callback on every platform; no local native
webview object is available. Configured \`new_window_handler\` fails creation rather
than fabricating its required native opener. Default page popups are denied without
automatic external launch. Navigation pauses/resumes the original request, preserving
POST data and redirects. Subframe veto cancels only that request, not parent/sibling
loads; Chromium may commit an iframe error document instead of retaining its previous
one. Windows screen coordinates use native DIP/physical conversions for mixed-DPI
origins. Initialization scripts retain frame scope through sandboxed
subframe preload, while IPC remains main-frame-only. \`data_directory\` uses resolved
\`Session.fromPath\` directories shared by path; incognito uses unique in-memory sessions.
Exit veto/code handling distinguishes \`run_return\` from process-exiting \`run\`; macOS
reopen and supported process-termination events respect trait platform gates.

On macOS, Overlay maps to Electron's hidden title bar; logical traffic-light positions
and tabbing identifiers are forwarded. Transparent title bars and hidden titles fail
creation explicitly because no matching public Electron API preserves their semantics.
Runtime title-bar-style changes remain unsupported.

## Library window API (outside trait counts)

With the optional \`tauri\` feature, \`tauri_runtime_electron::window::Window\` wraps
an official \`tauri::Window<ElectronRuntime>\`, converts from official Window and
WebviewWindow handles and retains official methods through \`Deref\`. The module
re-exports official \`Color\`, \`Effect\`, \`EffectState\`, \`WindowEffectsConfig\` and
\`EffectsBuilder\`. Its \`set_effects<E: Into<Option<WindowEffectsConfig>>>\` and
\`clear_effects\` methods, or the free functions accepting an official Window, route
through the selected dispatcher's \`run_on_main_thread\` task with a window serial
and weak Electron connection scoped to that task, without a global current runtime
or an upstream downcast.

The TypeScript entry \`tauri-runtime-electron/api/window\` extends official Window,
exports \`Effect\`/\`EffectState\` and returns library handles from its factories.
The package is private and unpublished; consume a checkout with the official v2
\`@tauri-apps/api\` peer. Its \`setEffects\`/\`clearEffects\` invoke the async
\`plugin:electron-window|set_effects\` command. Applications register \`window::init()\`,
declare the \`electron-window\` inlined plugin and \`set_effects\` command through
\`tauri_build::Attributes::new().plugin(...)\` and \`try_build\`, and explicitly grant
\`electron-window:allow-set-effects\` in their capabilities. Both set and clear use
normal Tauri IPC/ACL. [The README](../README.md#2-window-api-and-native-effects) has
minimal Rust/TS and build/ACL examples.

Only material selection through \`effects\` is supported. On Windows 11 22H2
(build 22621)+, the first supported Acrylic, Mica or Tabbed entry maps to the real
\`window.backgroundMaterial\` Electron RPC. A list with no supported effect or an
unsupported platform returns an error. Official \`state\`, \`radius\` and
\`interactive\` fields apply only on macOS, while \`color\` has no effect on
Windows 11; this backend ignores them. Clearing restores the configured background
color, including alpha, across repeated enable/clear calls.

The transparent demo uses this library API, with generated \`window-api.js\` assets,
the \`tauri\` feature, plugin registration, inlined command metadata and an explicit
capability. Its page probes support, serializes effects changes and clears them on
reset/reload; unsupported errors are shown and keep the blur control disabled.
Zero-alpha window/page backgrounds, panel opacity and window controls are checked
separately from desktop compositing review. CSS \`backdrop-filter\` only blurs page
content and does not establish desktop blur. OS settings can limit the visible effect.

Downloads stage privately until completion and the first host decision, then publish
exclusively without overwriting destinations; failed publication retains the staged
file, while denial, timeout and destruction clean it up.

## Native code

| file | why it cannot be Electron IPC |
| --- | --- |
| \`src/platform/cursor.rs\` | Electron has no cursor API; \`SetCursorPos\` on Windows and \`CGWarpMouseCursorPosition\` on macOS |
| \`src/platform/dpi.rs\` | a host that is not DPI aware has its coordinates scaled, which puts \`SetCursorPos\` near the corner |
| \`src/window.rs\` \`imp\` | Windows frame/z-order changes and real non-client move/resize use Win32, including asynchronous \`PostMessageW\` so modal drags do not block the IPC thread |

## Summary

${summaryTable.join('\n')}

## Gaps

Every member that is not \`wired\`, with the file to open and what wry does instead.

| member | electron | location | wry |
| --- | --- | --- | --- |
${gaps.join('\n')}

## Every member

${detail.join('\n')}
`

if (process.argv.includes('--write')) {
  mkdirSync(dirname(repo(DOC)), { recursive: true })
  writeFileSync(repo(DOC), markdown)
  console.log(`wrote ${DOC}`)
} else if (process.argv.includes('--md')) process.stdout.write(markdown)
else if (process.argv.includes('--check')) {
  let current = ''
  try {
    current = readFileSync(repo(DOC), 'utf8').replace(/\r\n/g, '\n')
  } catch {
    console.error(`${DOC} is missing; run: node tests/checks/coverage.mjs --write`)
    process.exit(1)
  }
  if (current !== markdown) {
    console.error(`${DOC} is stale; run: node tests/checks/coverage.mjs --write`)
    process.exit(1)
  }
} else {
  for (const row of rows) {
    const c = counts(row.members)
    console.log(`\n== ${row.file} — ${row.trait} (${row.members.length} members)`)
    for (const kind of KINDS.filter((k) => k !== 'wired')) {
      const list = row.members.filter((m) => m.kind === kind).map((m) => `${m.name}${m.line ? `:${m.line}` : ''}`)
      console.log(`  ${kind.padEnd(7)} ${c[kind] === 0 ? '-' : `${c[kind]}: ${list.join(', ')}`}`)
    }
  }
}
