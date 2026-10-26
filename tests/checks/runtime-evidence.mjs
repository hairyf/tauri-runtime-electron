// CI protocol: a real assertion emits one JSON marker per member it proves.
// [runtime-evidence] {"member":"WindowDispatch::set_title","test":"smoke::title_checks","assertion":"title() == smoke-1 after set_title","kind":"behavior","status":"pass"}
// Collect only after the entire runtime command finishes (including its exit status):
// node tests/checks/runtime-evidence.mjs collect --log run.log --output result.json --status pass
import assert from 'node:assert/strict'
import { mkdirSync, readFileSync, readdirSync, writeFileSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

export const PLATFORMS = ['windows', 'macos', 'linux']
export const MARKER = '[runtime-evidence] '
const statuses = ['pass', 'fail', 'skip', 'unknown']
const text = (value, field) => assert.ok(typeof value === 'string' && value.trim(), `${field}: expected nonempty string`)
const fields = (value, required, name) => {
  assert.ok(value && typeof value === 'object' && !Array.isArray(value), `${name}: expected object`)
  assert.deepEqual(Object.keys(value).sort(), [...required].sort(), `${name}: unexpected or missing fields`)
}

export function validateArtifact(artifact) {
  fields(artifact, ['schemaVersion', 'commit', 'platform', 'run', 'testRun', 'results'], 'artifact')
  assert.equal(artifact.schemaVersion, 1, 'unsupported evidence schema')
  assert.match(artifact.commit, /^[a-f0-9]{40}$/, 'commit: expected full SHA')
  assert.ok(PLATFORMS.includes(artifact.platform), 'platform: expected windows/macos/linux')
  fields(artifact.run, ['id', 'url'], 'run')
  assert.match(artifact.run.id, /^[1-9]\d*\/[1-9]\d*$/, 'run.id: expected CI run/attempt')
  const [id, attempt] = artifact.run.id.split('/')
  const url = new URL(artifact.run.url)
  assert.equal(url.protocol, 'https:', 'run.url: expected HTTPS CI run URL')
  assert.equal(artifact.run.url, url.href, 'run.url: expected canonical URL')
  assert.ok(url.pathname.endsWith(`/actions/runs/${id}/attempts/${attempt}`), 'run.url: must identify the exact run/attempt')
  assert.ok(!/[<>"\s]/.test(artifact.run.url) && !url.hash && !url.search, 'run.url: expected safe CI run path')
  assert.ok(!url.username && !url.password, 'run.url: credentials are not allowed')
  fields(artifact.testRun, ['id', 'status', 'executor'], 'testRun')
  text(artifact.testRun.id, 'testRun.id')
  assert.ok(statuses.includes(artifact.testRun.status), 'testRun.status: invalid status')
  assert.equal(artifact.testRun.executor, 'real-electron', 'only real Electron runs are evidence')
  assert.ok(Array.isArray(artifact.results), 'results: expected array')
  const seen = new Set()
  for (const result of artifact.results) {
    fields(result, ['member', 'test', 'assertion', 'kind', 'status'], 'result')
    assert.match(result.member, /^[A-Za-z_][\w]*::[a-z_][\w]*$/, 'member: expected qualified API name')
    text(result.test, 'result.test')
    text(result.assertion, 'result.assertion')
    assert.ok(['behavior', 'static', 'wiring', 'negative'].includes(result.kind), 'result.kind: invalid evidence kind')
    assert.ok(statuses.includes(result.status), 'result.status: invalid status')
    const key = JSON.stringify([result.member, result.test, result.assertion])
    assert.ok(!seen.has(key), `duplicate assertion: ${key}`)
    seen.add(key)
  }
  return artifact
}

export function collectEvidence(log, metadata) {
  const results = []
  for (const line of log.replaceAll('\r\n', '\n').split('\n')) {
    // Cargo --nocapture may print its test-name prefix on the marker's line.
    const offset = line.indexOf(MARKER)
    if (offset >= 0) results.push(JSON.parse(line.slice(offset + MARKER.length)))
  }
  return validateArtifact({ schemaVersion: 1, ...metadata, results })
}

export function readArtifacts(directory) {
  return readdirSync(directory, { withFileTypes: true }).flatMap(entry => {
    const path = join(directory, entry.name)
    if (entry.isDirectory()) return readArtifacts(path)
    return entry.isFile() && entry.name.endsWith('.json')
      ? [validateArtifact(JSON.parse(readFileSync(path, 'utf8')))] : []
  })
}

export function aggregateEvidence(inventory, artifacts, { commit, runId } = {}) {
  const members = new Map(inventory.map(member => [member.id, member]))
  assert.equal(members.size, inventory.length, 'duplicate inventory member')
  const cells = new Map(inventory.map(member => [member.id,
    Object.fromEntries(PLATFORMS.map(platform => [platform, {
      status: member.na[platform] ? 'na' : 'unknown', reason: member.na[platform] ?? '', evidence: [],
    }]))]))
  const runs = new Set()
  let runUrl
  if (artifacts.length) {
    assert.match(commit ?? '', /^[a-f0-9]{40}$/, 'evidence requires --commit with full SHA')
    text(runId, 'evidence requires --run for one exact CI attempt')
  }
  for (const artifact of artifacts) {
    validateArtifact(artifact)
    assert.equal(artifact.commit, commit, 'stale or mixed-commit evidence')
    assert.equal(artifact.run.id, runId, 'mixed CI runs/attempts')
    runUrl ??= artifact.run.url
    assert.equal(artifact.run.url, runUrl, 'mixed CI run URLs')
    const key = `${artifact.platform}/${artifact.testRun.id}`
    assert.ok(!runs.has(key), `duplicate test run: ${key}`)
    runs.add(key)
    for (const result of artifact.results) {
      assert.ok(members.has(result.member), `unknown evidence member: ${result.member}`)
      const cell = cells.get(result.member)[artifact.platform]
      assert.notEqual(cell.status, 'na', `${result.member}: evidence for N/A platform ${artifact.platform}`)
      if (result.kind === 'behavior') cell.evidence.push({ ...result, testRun: artifact.testRun, run: artifact.run })
    }
  }
  for (const platforms of cells.values()) for (const cell of Object.values(platforms)) {
    if (cell.status === 'na' || !cell.evidence.length) continue
    // Never pick a lucky pass out of conflicting assertions or a failed suite.
    cell.status = cell.evidence.some(result => result.status === 'fail') ? 'fail'
      : cell.evidence.some(result => result.testRun.status !== 'pass' || result.status === 'unknown') ? 'unknown'
      : cell.evidence.some(result => result.status === 'skip') ? 'skip' : 'pass'
  }
  const summary = (scope) => {
    const selected = inventory.filter(member => member.scope === scope)
    const count = (platform) => {
      const eligible = selected.filter(member => !member.na[platform])
      return { passed: eligible.filter(member => cells.get(member.id)[platform].status === 'pass').length, total: eligible.length }
    }
    return {
      ...Object.fromEntries(PLATFORMS.map(platform => [platform, count(platform)])),
      all: {
        passed: selected.filter(member => PLATFORMS.every(platform => ['pass', 'na'].includes(cells.get(member.id)[platform].status))).length,
        total: selected.length,
      },
    }
  }
  return { cells, runtime: summary('runtime'), extensions: summary('extension'),
    platforms: [...new Set(artifacts.map(artifact => artifact.platform))], commit, runId, runUrl }
}

// The inventory only needs upstream cfg syntax used by these six traits.
// ponytail: unknown cfg fails closed; switch to rustdoc JSON if upstream needs more syntax.
export function cfgApplies(expression, platform) {
  if (!expression) return true
  const tokens = expression.match(/"[^"]*"|[a-z_][a-z_0-9]*|[=(),]/g) ?? []
  assert.equal(tokens.join(''), expression.replace(/\s/g, ''), `unsupported cfg: ${expression}`)
  let index = 0
  const parse = () => {
    const key = tokens[index++]
    if (['any', 'all', 'not'].includes(key)) {
      assert.equal(tokens[index++], '(')
      const values = []
      while (tokens[index] !== ')') {
        values.push(parse())
        if (tokens[index] !== ')') assert.equal(tokens[index++], ',')
      }
      index += 1
      if (key === 'not') assert.equal(values.length, 1)
      return key === 'any' ? values.some(Boolean) : key === 'all' ? values.every(Boolean) : !values[0]
    }
    if (tokens[index] === '=') {
      index += 1
      const value = JSON.parse(tokens[index++])
      if (key === 'target_os') {
        assert.ok(['windows', 'macos', 'linux', 'dragonfly', 'freebsd', 'netbsd', 'openbsd', 'android', 'ios'].includes(value), `unreviewed target_os: ${value}`)
        return value === platform
      }
      assert.ok(key === 'feature' && value === 'devtools', `unsupported cfg: ${expression}`)
      return true // The documented denominator includes the devtools feature.
    }
    const values = { windows: platform === 'windows', unix: platform !== 'windows', desktop: true, mobile: false, debug_assertions: true }
    assert.ok(Object.hasOwn(values, key), `unsupported cfg: ${expression}`)
    return values[key]
  }
  const result = parse()
  assert.equal(index, tokens.length, `unconsumed cfg: ${expression}`)
  return result
}

export function traitMembers(source, trait) {
  const lines = source.replaceAll('\r\n', '\n').split('\n')
  const start = lines.findIndex(line => new RegExp(`^pub trait ${trait}(?:<|:|\\s)`).test(line))
  assert.ok(start >= 0, `trait ${trait} not found`)
  const result = new Map()
  let previous = start
  for (let i = start + 1; i < lines.length && !/^}/.test(lines[i]); i += 1) {
    const match = lines[i].match(/^ {2}(?:unsafe )?fn ([a-z_0-9]+)/)
    if (!match) continue // Associated types and marker traits are not callable members.
    const name = match[1]
    const cfg = [...lines.slice(previous + 1, i).join('\n').matchAll(/#\[cfg\(([\s\S]*?)\)\]/g)].map(match => match[1])
    const declaration = { line: i + 1, cfg }
    if (!result.has(name)) result.set(name, { name, declarations: [] })
    result.get(name).declarations.push(declaration)
    previous = i
  }
  return [...result.values()]
}

export function ciMetadata(env, suite, status) {
  for (const key of ['GITHUB_SHA', 'RUNNER_OS', 'GITHUB_RUN_ID', 'GITHUB_RUN_ATTEMPT', 'GITHUB_REPOSITORY', 'GITHUB_SERVER_URL']) text(env[key], key)
  return {
    commit: env.GITHUB_SHA,
    platform: { Windows: 'windows', macOS: 'macos', Linux: 'linux' }[env.RUNNER_OS],
    run: { id: `${env.GITHUB_RUN_ID}/${env.GITHUB_RUN_ATTEMPT}`, url: `${env.GITHUB_SERVER_URL}/${env.GITHUB_REPOSITORY}/actions/runs/${env.GITHUB_RUN_ID}/attempts/${env.GITHUB_RUN_ATTEMPT}` },
    testRun: { id: suite, executor: 'real-electron', status },
  }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  assert.equal(process.argv[2], 'collect', 'expected collect --log PATH --output PATH --status pass|fail')
  const args = process.argv.slice(3)
  assert.equal(args.length, 6, 'expected exactly --log PATH --output PATH --status pass|fail')
  const options = Object.fromEntries([0, 2, 4].map(index => [args[index], args[index + 1]]))
  assert.deepEqual(Object.keys(options).sort(), ['--log', '--output', '--status'])
  assert.ok(['pass', 'fail'].includes(options['--status']), 'collector needs the actual suite exit status')
  const artifact = collectEvidence(readFileSync(options['--log'], 'utf8'), ciMetadata(process.env, 'runtime-suite', options['--status']))
  const platform = artifact.platform
  mkdirSync(dirname(options['--output']), { recursive: true })
  writeFileSync(options['--output'], `${JSON.stringify(artifact, null, 2)}\n`)
  console.log(`collected ${artifact.results.length} explicit member assertions (${artifact.testRun.status}, ${platform})`)
}
