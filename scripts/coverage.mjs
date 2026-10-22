// Generates docs/coverage.md: which `tauri-runtime` trait members this runtime
// actually implements.
//
// Statuses:
//   implemented - the body does real work
//   ignored     - returns success and drops the request (the dangerous kind)
//   logged      - returns success but reports itself through `unsupported`
//   refused     - returns an error / todo!
//   missing     - required by the trait, not implemented at all
import { execFileSync } from 'node:child_process'
import { readFileSync, readdirSync, writeFileSync, mkdirSync } from 'node:fs'
import { dirname, join, relative } from 'node:path'

const ROOT = new URL('..', import.meta.url).pathname.replace(/^\/([A-Za-z]:)/, '$1')

// --- locate the tauri-runtime sources this build actually uses
function tauriRuntimeSources() {
  const metadata = JSON.parse(
    execFileSync('cargo', ['metadata', '--format-version', '1'], {
      cwd: ROOT,
      maxBuffer: 256 * 1024 * 1024,
    }),
  )
  const pkg = metadata.packages.find((p) => p.name === 'tauri-runtime')
  if (!pkg) throw new Error('tauri-runtime is not in the dependency graph')
  return { dir: join(dirname(pkg.manifest_path), 'src'), version: pkg.version }
}

// --- minimal Rust source scanner
const rustFiles = (dir) =>
  readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const path = join(dir, entry.name)
    if (entry.isDirectory()) return rustFiles(path)
    return entry.name.endsWith('.rs') ? [path] : []
  })

/** Blocks starting at every `headerRegex` match; body is brace-balanced. */
function blocks(text, headerRegex) {
  const out = []
  const re = new RegExp(headerRegex, 'g')
  let match
  while ((match = re.exec(text))) {
    const open = text.indexOf('{', match.index)
    if (open < 0) continue
    let depth = 0
    let close = open
    for (; close < text.length; close++) {
      if (text[close] === '{') depth++
      else if (text[close] === '}' && --depth === 0) break
    }
    out.push({
      header: match[0],
      body: text.slice(open + 1, close),
      line: text.slice(0, match.index).split('\n').length,
    })
  }
  return out
}

const FN = 'fn\\s+([A-Za-z_]\\w*)\\s*[(<]'
const fnName = (block) => new RegExp(FN).exec(block.header)[1]

/** Methods of a trait body, with whether the trait supplies a default body. */
function traitMembers(body) {
  const members = []
  const re = new RegExp(FN, 'g')
  let match
  while ((match = re.exec(body))) {
    // first `;` or `{` after the signature decides declaration vs default
    const rest = body.slice(re.lastIndex)
    const semi = rest.indexOf(';')
    const brace = rest.indexOf('{')
    const hasDefault = brace >= 0 && (semi < 0 || brace < semi)
    members.push({ name: match[1], hasDefault })
  }
  return members
}

const squish = (text) =>
  text
    .replace(/\/\/.*$/gm, '')
    .replace(/\/\*[\s\S]*?\*\//g, '')
    .replace(/\s+/g, '')

function classify(body) {
  const code = squish(body)
  if (/^$|^self$|^Ok\(\(\)\)$|^Ok\(None\)$/.test(code)) return 'ignored'
  // A body that both does work and reports is only unsupported on some
  // targets (the `#[cfg(not(windows))]` branch).
  const platformGated = /#\[cfg\((?:not\()?windows/.test(body)
  if (code.includes('.unsupported("') || code.includes('unsupported::report(')) {
    return platformGated ? 'partial' : 'logged'
  }
  if (/^(Err\(|todo!|unimplemented!)/.test(code)) return 'refused'
  return 'implemented'
}

// --- collect the intended surface
const runtime = tauriRuntimeSources()
const surface = new Map() // trait -> [{name, hasDefault}]
for (const file of rustFiles(runtime.dir)) {
  const text = readFileSync(file, 'utf8')
  for (const block of blocks(text, 'pub trait\\s+[A-Za-z_]\\w*')) {
    const name = /pub trait\s+([A-Za-z_]\w*)/.exec(block.header)[1]
    const members = traitMembers(block.body)
    if (members.length) surface.set(name, members)
  }
}

// --- collect the implementation
const impls = new Map() // trait -> Map(name -> {status, where})
for (const file of rustFiles(join(ROOT, 'src'))) {
  const text = readFileSync(file, 'utf8')
  for (const block of blocks(text, 'impl(?:<[^>]*>)?\\s+[A-Za-z_][\\w<>,: ]*\\s+for\\s+[A-Za-z_]\\w*')) {
    const head = /impl(?:<[^>]*>)?\s+([A-Za-z_]\w*)(?:<[^>]*>)?\s+for\s+([A-Za-z_]\w*)/.exec(
      block.header,
    )
    if (!head || !surface.has(head[1])) continue
    const methods = impls.get(head[1]) || new Map()
    for (const method of blocks(block.body, FN)) {
      methods.set(fnName(method), {
        status: classify(method.body),
        where: `${relative(ROOT, file).replaceAll('\\', '/')}:${block.line + method.line - 1}`,
      })
    }
    impls.set(head[1], methods)
  }
}

// --- report
const ORDER = [
  'Runtime',
  'RuntimeHandle',
  'EventLoopProxy',
  'RuntimeInitAttrs',
  'WindowDispatch',
  'WebviewDispatch',
  'WindowBuilder',
]
const traits = [...surface.keys()].sort((a, b) => {
  const ai = ORDER.indexOf(a)
  const bi = ORDER.indexOf(b)
  return (ai < 0 ? 99 : ai) - (bi < 0 ? 99 : bi) || a.localeCompare(b)
})

const STATUS_LABEL = {
  implemented: '✅ implemented',
  logged: '⚠️ accepted then ignored',
  partial: '⚠️ only some platforms',
  ignored: '⚠️ silently ignored',
  refused: '⛔ refused',
  missing: '❌ missing',
}

let totals = {
  implemented: 0,
  logged: 0,
  partial: 0,
  ignored: 0,
  refused: 0,
  missing: 0,
  inherited: 0,
}
const sections = []

for (const trait of traits) {
  const methods = impls.get(trait) || new Map()
  const rows = []
  for (const member of surface.get(trait)) {
    const hit = methods.get(member.name)
    let status = hit ? hit.status : member.hasDefault ? 'inherited' : 'missing'
    if (status !== 'inherited') totals[status]++
    rows.push({ name: member.name, status, where: hit?.where })
  }
  const count = (status) => rows.filter((row) => row.status === status).length
  const header = `| ${trait} | ${rows.length} | ${count('implemented')} | ${
    count('logged') + count('ignored') + count('partial')
  } | ${count('refused')} | ${count('missing')} |`
  sections.push({ trait, rows, header })
}

const gaps = (rows) => rows.filter((row) => row.status !== 'implemented')

const markdown = [
  '# tauri-runtime coverage',
  '',
  `Generated by \`node scripts/coverage.mjs\` against \`tauri-runtime ${runtime.version}\`.`,
  'Do not edit by hand — re-run the script.',
  '',
  '| status | meaning |',
  '| --- | --- |',
  '| ✅ implemented | the call does real work |',
  '| ⚠️ accepted then ignored | returns success, drops the request, logs once (`unsupported`) |',
  '| ⚠️ only some platforms | works on Windows; reports and skips elsewhere |',
  '| ⚠️ silently ignored | returns a success value and drops the request — **the dangerous kind** |',
  '| ⛔ refused | returns an error / `todo!` |',
  '| ❌ missing | required by the trait, not implemented at all |',
  '',
  '## Summary',
  '',
  '| trait | members | ✅ | ⚠️ | ⛔ | ❌ |',
  '| --- | --- | --- | --- | --- | --- |',
  ...sections.map((section) => section.header),
  `| **total** | **${
    totals.implemented +
    totals.logged +
    totals.partial +
    totals.ignored +
    totals.refused +
    totals.missing
  }** | **${totals.implemented}** | **${totals.logged + totals.partial + totals.ignored}** | **${
    totals.refused
  }** | **${totals.missing}** |`,
  '',
  `${totals.inherited} further members are omitted because the trait supplies a default body.`,
  '',
]

for (const section of sections) {
  const sectionGaps = gaps(section.rows)
  markdown.push(`## ${section.trait}`, '')
  if (!sectionGaps.length) {
    markdown.push('Every member is implemented.', '')
    continue
  }
  markdown.push(
    `${section.rows.length - sectionGaps.length} of ${section.rows.length} implemented.`,
    '',
    '| member | status | where |',
    '| --- | --- | --- |',
    ...sectionGaps.map(
      (row) => `| \`${row.name}\` | ${STATUS_LABEL[row.status]} | ${row.where ? `\`${row.where}\`` : '—'} |`,
    ),
    '',
  )
}

const out = join(ROOT, 'docs', 'coverage.md')
mkdirSync(dirname(out), { recursive: true })
writeFileSync(out, `${markdown.join('\n')}\n`)

const silent = totals.ignored
console.log(`tauri-runtime ${runtime.version}: ${Object.entries(totals).map(([k, v]) => `${k}=${v}`).join(' ')}`)
console.log(`wrote ${relative(ROOT, out)}`)
if (silent) {
  console.error(`\n${silent} member(s) are still silently ignored — they report nothing to the caller.`)
  process.exitCode = 1
}
