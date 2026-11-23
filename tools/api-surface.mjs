// Extracts the public item surface of a Rust crate's `src` tree.
// Usage: node tools/api-surface.mjs <src-dir> [--json]
import { readFileSync, readdirSync, statSync } from 'node:fs'
import { join, relative } from 'node:path'

const root = process.argv[2]
if (!root) {
  console.error('usage: node tools/api-surface.mjs <src-dir> [--json]')
  process.exit(2)
}

function walk(dir) {
  return readdirSync(dir).flatMap((entry) => {
    const path = join(dir, entry)
    return statSync(path).isDirectory() ? walk(path) : entry.endsWith('.rs') ? [path] : []
  })
}

const ITEM = /^\s*pub (?:async )?(?:unsafe )?(fn|struct|enum|trait|type|const|static|mod|use) ([A-Za-z_][\w]*)/gm

const files = {}
for (const file of walk(root).sort()) {
  const text = readFileSync(file, 'utf8')
  const items = new Set()
  let match
  ITEM.lastIndex = 0
  while ((match = ITEM.exec(text))) items.add(`${match[1]} ${match[2]}`)
  const key = relative(root, file).replaceAll('\\', '/')
  files[key] = [...items].sort()
}

if (process.argv.includes('--json')) {
  console.log(JSON.stringify(files, null, 2))
} else {
  for (const [file, items] of Object.entries(files)) {
    console.log(`### ${file} (${items.length})`)
    console.log(items.join(', '))
  }
}
