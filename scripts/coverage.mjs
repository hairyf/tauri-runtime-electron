// Reports which `tauri-runtime` trait members still lack a real implementation.
// Compares the trait declarations against our dispatch modules; a body whose
// only statement is `Ok(())`/`Ok(None)` counts as a stub.
import { readFileSync } from 'node:fs'

const LIB = String.raw`C:\Users\wwu71\.cargo\registry\src\index.crates.io-1949cf8c6b5b557f\tauri-runtime-3.0.0-alpha.4\src\lib.rs`

const lines = readFileSync(LIB, 'utf8').split('\n').map((line) => line.replace(/\r$/, ''))

function members(marker) {
  const start = lines.findIndex((line) => line.includes(marker))
  const names = []
  for (let i = start + 1; i < lines.length; i += 1) {
    if (lines[i] === '}') break
    const match = lines[i].match(/^ {2}(?:unsafe )?fn ([a-z_0-9]+)/)
    if (match) names.push(match[1])
  }
  return names
}

function check(path, names) {
  const text = readFileSync(path, 'utf8')
  const missing = []
  const stubs = []
  for (const name of names) {
    const found = new RegExp(`fn ${name}\\b[^{]*\\{`).exec(text)
    if (!found) {
      missing.push(name)
      continue
    }
    const rest = text.slice(found.index + found[0].length)
    const end = rest.indexOf('\n  }\n')
    const body = (end >= 0 ? rest.slice(0, end) : rest.slice(0, 300))
      .split('\n')
      .map((line) => line.trim())
      .filter((line) => line && !line.startsWith('//'))
      .join(' ')
    if (/^Ok\((\(\)|None)\);?$/.test(body)) stubs.push(name)
  }
  return { missing, stubs }
}

const targets = [
  ['src/window.rs', members('pub trait WindowDispatch<T: UserEvent>')],
  ['src/webview.rs', members('pub trait WebviewDispatch<T: UserEvent>')],
  ['src/runtime.rs', members('pub trait Runtime<T: UserEvent>')],
  ['src/window_handle.rs', members('pub trait RuntimeHandle<T: UserEvent>')],
]

for (const [path, names] of targets) {
  const { missing, stubs } = check(path, names)
  console.log(`\n== ${path} (${names.length} members)`)
  console.log('missing:', missing.join(', ') || '-')
  console.log('stub  :', stubs.join(', ') || '-')
}
