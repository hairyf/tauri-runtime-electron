// Prints a structural outline of Rust files: top-level items, impl blocks and
// their members, with line numbers. Used to compare two crates side by side.
// Usage: node tools/outline.mjs <file-or-dir> [--depth=N]
import { readFileSync, readdirSync, statSync } from 'node:fs'
import { join, relative } from 'node:path'

const target = process.argv[2]
const depthArg = process.argv.find((a) => a.startsWith('--depth='))
const depth = depthArg ? Number(depthArg.slice(8)) : 2

function walk(path) {
  return statSync(path).isDirectory()
    ? readdirSync(path).flatMap((e) => walk(join(path, e)))
    : path.endsWith('.rs')
      ? [path]
      : []
}

/** Strips comments and strings so brace counting is reliable. */
function strip(text) {
  let out = ''
  let i = 0
  while (i < text.length) {
    const c = text[i]
    if (c === '/' && text[i + 1] === '/') {
      while (i < text.length && text[i] !== '\n') i++
    } else if (c === '/' && text[i + 1] === '*') {
      i += 2
      let nest = 1
      while (i < text.length && nest) {
        if (text[i] === '/' && text[i + 1] === '*') { nest++; i += 2 }
        else if (text[i] === '*' && text[i + 1] === '/') { nest--; i += 2 }
        else i++
      }
    } else if (c === '"') {
      i++
      while (i < text.length) {
        if (text[i] === '\\') i += 2
        else if (text[i] === '"') { i++; break }
        else i++
      }
      out += '""'
    } else if (c === "'" && /[A-Za-z_]/.test(text[i + 1] ?? '') && text[i + 2] === "'") {
      out += "''"
      i += 3
    } else {
      out += c
      i++
    }
  }
  return out
}

const ITEM =
  /^(?:pub(?:\([^)]*\))?\s+)?(?:(?:async|unsafe|const|extern(?:\s+"[^"]*")?)\s+)*(fn|struct|enum|trait|type|const|static|mod|impl|macro_rules!)\b(.*)$/

for (const file of walk(target).sort()) {
  const raw = readFileSync(file, 'utf8')
  const text = strip(raw)
  const lines = raw.split('\n')
  const clean = text.split('\n')
  console.log(`\n=== ${relative(process.cwd(), file).replaceAll('\\', '/')} (${lines.length} lines) ===`)
  let depthCount = 0
  let implHeader = null
  for (let n = 0; n < clean.length; n++) {
    const line = clean[n]
    const trimmed = line.trim()
    const atTop = depthCount === 0
    const m = atTop && ITEM.exec(trimmed)
    if (m) {
      const kind = m[1]
      const rest = m[2].replace(/\s*\{.*$/, '').replace(/;\s*$/, '').trim()
      if (kind === 'impl') {
        implHeader = rest
        console.log(`  ${n + 1}: impl ${rest}`)
      } else {
        console.log(`${n + 1}: ${kind} ${rest}`)
      }
    } else if (depthCount === 1 && implHeader && /^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?(?:unsafe\s+)?fn\s+[A-Za-z_]/.test(line)) {
      const name = /fn\s+([A-Za-z_]\w*)/.exec(line)[1]
      console.log(`    ${n + 1}: ${name}`)
    }
    for (const ch of line) {
      if (ch === '{') depthCount++
      else if (ch === '}') { depthCount--; if (depthCount === 0) implHeader = null }
    }
  }
}
