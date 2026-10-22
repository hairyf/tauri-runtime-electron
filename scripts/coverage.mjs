// Reports how much of `tauri-runtime` this crate actually implements, and how it
// compares with `tauri-runtime-wry`.
//
//   node scripts/coverage.mjs          human summary
//   node scripts/coverage.mjs --md     markdown to stdout
//   node scripts/coverage.mjs --write  rewrite docs/coverage.md
//   node scripts/coverage.mjs --check  exit 1 when docs/coverage.md is stale
//
// Both the traits and the wry reference come from the pinned submodule, so the
// report always matches the version in Cargo.lock.
// `git submodule update --init` populates it.
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { dirname } from 'node:path'
import { fileURLToPath } from 'node:url'

const repo = (path) => fileURLToPath(new URL(`../${path}`, import.meta.url))
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

/** wired | stub | noop | warned | panic. */
function classify({ body }) {
  const code = body.trim()
  if (/\b(todo|unimplemented)!/.test(code)) return 'panic'
  if (/unsupported\(/.test(code)) return 'warned'
  if (LITERAL.test(code)) return 'stub'
  if (!/\bself\b/.test(code) && !/\b(Self::|json!|ipc|context|dispatch)/.test(code)) return 'noop'
  return 'wired'
}

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

const KINDS = ['wired', 'stub', 'noop', 'warned', 'panic', 'missing']
const ICON = { wired: 'wired', stub: 'stub', noop: 'no-op', warned: 'warned', panic: 'panic', missing: 'missing' }
const counts = (members) =>
  Object.fromEntries(KINDS.map((kind) => [kind, members.filter((m) => m.kind === kind).length]))

const summaryTable = [
  '| trait | members | wired | stub | no-op | warned | panic | missing |',
  '| --- | --- | --- | --- | --- | --- | --- | --- |',
  ...rows.map((r) => {
    const c = counts(r.members)
    return `| \`${r.trait}\` | ${r.members.length} | ${c.wired} | ${c.stub} | ${c.noop} | ${c.warned} | ${c.panic} | ${c.missing} |`
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

Generated by \`node scripts/coverage.mjs --write\` — do not edit by hand.
Traits come from \`${RUNTIME_LIB}\` and the reference column from \`${WRY_LIB}\`, both from the
pinned \`source/tauri\` submodule (Tauri 2.12), so they always match \`Cargo.lock\`.

This source scan lists implementation paths, not verified capability coverage.
\`cargo test\` opens a real window through \`src/bin/smoke.rs\` and checks its named
desktop subset on Windows, macOS and Linux in CI. Platform skips are printed.
It does not verify every method, native effect or official example. The separate
\`pnpm check\` runner records official-example assertions and screenshots; those
screenshots still require visual review. Native menus currently have an unbridged
cross-process command/accelerator path even when attaching a menu succeeds.

| kind | meaning |
| --- | --- |
| wired | the body reaches Electron over IPC or a cached native handle |
| stub | the body is a literal: \`Ok(())\`, \`Ok(true)\`, \`Ok(None)\` — it silently does nothing |
| no-op | the body never touches Electron or \`self\` — it silently does nothing |
| warned | the body calls \`runtime::unsupported\`: it does what the platform allows and logs once when it does not |
| panic | \`todo!()\`/\`unimplemented!()\` — loud, but only on the platform that compiles it |
| missing | no override, so the trait default applies |

## Support matrix

| target | compiles (CI) | window launched | notes |
| --- | --- | --- | --- |
| Windows | yes | yes | local runs and CI; the Win32 handle carries decorations, drag and the cursor |
| macOS | yes | in CI | \`fetch_data_store_identifiers\` panics; AppKit handle is handed back; cursor warp via CoreGraphics |
| Linux | yes | in CI (xvfb) | \`display_handle\` reports \`Xlib(None)\`, so wgpu/GL interop cannot work; the cursor cannot be warped without a GTK display, so \`set_cursor_position\` logs a gap |
| Android / iOS | no | no | not supported: the platform branches reference \`jni\`/\`objc\` without a dependency |

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
    console.error(`${DOC} is missing; run: node scripts/coverage.mjs --write`)
    process.exit(1)
  }
  if (current !== markdown) {
    console.error(`${DOC} is stale; run: node scripts/coverage.mjs --write`)
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
