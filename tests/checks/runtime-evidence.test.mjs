// Node-only protocol checks. Synthetic fixtures below are never written as CI evidence.
import assert from 'node:assert/strict'
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { spawnSync } from 'node:child_process'
import { fileURLToPath } from 'node:url'
import { aggregateEvidence, cfgApplies, collectEvidence, MARKER, PLATFORMS, readArtifacts, traitMembers, validateArtifact } from './runtime-evidence.mjs'
import { SUITES, requireSuites, suitesFor } from './runtime-suites.mjs'

// Protocol checks also run in the no-submodule regressions job.
const inventory = [
  { id: 'Runtime::new', na: {}, scope: 'runtime' },
  { id: 'Runtime::new_any_thread', na: { macos: 'upstream cfg' }, scope: 'runtime' },
  { id: 'ElectronWebviewDispatcher::on_new_window', na: {}, scope: 'extension' },
]
const commit = 'a'.repeat(40)
const runId = '123/1'
const metadata = platform => ({ commit, platform, run: { id: runId, url: 'https://ci.example/example/repo/actions/runs/123/attempts/1' },
  testRun: { id: 'runtime-suite', status: 'pass', executor: 'real-electron' } })
const result = (member, overrides = {}) => ({ member, test: 'smoke::title', assertion: 'actual title equals requested title', kind: 'behavior', status: 'pass', ...overrides })
const artifact = (platform = 'windows', results = [result('Runtime::new')]) => validateArtifact({ schemaVersion: 1, ...metadata(platform), results })
const aggregate = artifacts => aggregateEvidence(inventory, artifacts, { commit, runId })
const cell = (artifacts, member = 'Runtime::new', platform = 'windows') => aggregate(artifacts).cells.get(member)[platform].status

assert.equal(cell([]), 'unknown')
assert.equal(cell([artifact()]), 'pass')
for (const status of ['fail', 'skip', 'unknown']) assert.equal(cell([artifact('windows', [result('Runtime::new', { status })])]), status)
for (const kind of ['static', 'wiring', 'negative']) assert.equal(cell([artifact('windows', [result('Runtime::new', { kind })])]), 'unknown')
for (const status of ['fail', 'skip', 'unknown']) {
  const failedSuite = artifact()
  failedSuite.testRun.status = status
  assert.equal(cell([failedSuite]), 'unknown')
}
const conflict = artifact('windows', [result('Runtime::new'), result('Runtime::new', { assertion: 'second assertion', status: 'fail' })])
assert.equal(cell([conflict]), 'fail')
assert.equal(aggregate([artifact()]).runtime.all.passed, 0, 'one platform is not full desktop acceptance')
assert.equal(aggregate(PLATFORMS.map(platform => artifact(platform))).runtime.all.passed, 1)
assert.equal(aggregate([artifact('windows', [result('Runtime::new_any_thread')]), artifact('linux', [result('Runtime::new_any_thread')])]).runtime.all.passed, 1, 'upstream macOS N/A is excluded, not a fake pass')
assert.equal(aggregate(PLATFORMS.map(platform => artifact(platform, [result('ElectronWebviewDispatcher::on_new_window')]))).runtime.all.passed, 0)
assert.equal(aggregate(PLATFORMS.map(platform => artifact(platform, [result('ElectronWebviewDispatcher::on_new_window')]))).extensions.all.passed, 1)
assert.deepEqual(SUITES.map(suite => suite.id), ['runtime-tests', 'new_window_smoke', 'tauri_new_window_smoke', 'runtime_behavior_smoke', 'webview_behavior_smoke', 'desktop_behavior_smoke', 'system_behavior_smoke', 'macos_behavior_smoke', 'windows_behavior_smoke', 'linux_behavior_smoke', 'rendering_behavior_smoke'])
const commonSuites = ['runtime-tests', 'new_window_smoke', 'tauri_new_window_smoke', 'runtime_behavior_smoke', 'webview_behavior_smoke', 'desktop_behavior_smoke', 'system_behavior_smoke']
assert.deepEqual(suitesFor('windows').map(suite => suite.id), [...commonSuites, 'windows_behavior_smoke', 'rendering_behavior_smoke'])
assert.deepEqual(suitesFor('linux').map(suite => suite.id), [...commonSuites, 'linux_behavior_smoke', 'rendering_behavior_smoke'])
assert.deepEqual(suitesFor('macos').map(suite => suite.id), [...commonSuites, 'macos_behavior_smoke', 'rendering_behavior_smoke'])
assert.ok(SUITES.every(suite => suite.command === 'cargo' && suite.args.includes('--locked')))
assert.ok(SUITES.find(suite => suite.id === 'tauri_new_window_smoke').args.includes('tauri'))
const completeSuites = PLATFORMS.flatMap(platform => suitesFor(platform).map(suite => {
  const evidence = artifact(platform, [])
  evidence.testRun.id = suite.id
  return evidence
}))
assert.equal(completeSuites.length, 27, 'each desktop platform requires nine suites')
requireSuites(completeSuites)
assert.equal(aggregate(completeSuites).runtime.all.passed, 0, 'suite success never invents member behavior')
const unrecognized = artifact('windows', [])
unrecognized.testRun.id = 'macos_behavior_smoke'
assert.throws(() => requireSuites([...completeSuites, unrecognized]), /suite not applicable/)
unrecognized.testRun.id = 'not-a-suite'
assert.throws(() => requireSuites([...completeSuites, unrecognized]), /unrecognized CI suite/)
assert.throws(() => requireSuites(completeSuites.filter(report => report.testRun.id !== 'macos_behavior_smoke')), /missing or duplicate suite: macos\/macos_behavior_smoke/)
assert.throws(() => requireSuites(completeSuites.slice(1)), /missing or duplicate suite/)
assert.throws(() => requireSuites([...completeSuites, completeSuites[0]]), /missing or duplicate suite/)
for (const status of ['fail', 'skip', 'unknown']) {
  const incomplete = structuredClone(completeSuites)
  incomplete[0].testRun.status = status
  assert.throws(() => requireSuites(incomplete), /suite did not pass/)
}
assert.throws(() => aggregate([artifact(), artifact()]), /duplicate test run/)
assert.throws(() => aggregate([artifact('macos', [result('Runtime::new_any_thread')])]), /N\/A platform/)
assert.throws(() => aggregate([artifact('windows', [result('Runtime::not_a_member')])]), /unknown evidence member/)
assert.throws(() => aggregateEvidence(inventory, [artifact()]), /requires --commit/)
const stale = artifact()
stale.commit = 'b'.repeat(40)
assert.throws(() => aggregate([stale]), /mixed-commit/)
const wrongRun = artifact()
wrongRun.run.id = '123/2'
wrongRun.run.url = 'https://ci.example/example/repo/actions/runs/123/attempts/2'
assert.throws(() => aggregate([wrongRun]), /mixed CI runs/)
const wrongUrl = artifact('linux')
wrongUrl.run.url = 'https://other-ci.example/example/repo/actions/runs/123/attempts/1'
assert.throws(() => aggregate([artifact(), wrongUrl]), /mixed CI run URLs/)
for (const mutate of [
  a => { a.schemaVersion = 2 }, a => { a.commit = 'short' }, a => { a.platform = 'win32' },
  a => { a.testRun.executor = 'mock' }, a => { a.testRun.status = 'success' },
  a => { a.run.url = 'file:///fake' }, a => { a.run.url = 'https://user:secret@ci.example/example/repo/actions/runs/123/attempts/1' },
  a => { a.run.id = '123/2' }, a => { a.run.url = 'https://ci.example/example/repo/actions/runs/123/attempts/1#fake' },
  a => { a.run.url = 'https://ci.example/example/repo/actions/runs/999/attempts/1' },
  a => { a.results[0].assertion = '' }, a => { a.results[0].member = 'new' },
  a => { a.results[0].status = 'success' }, a => { a.results[0].kind = 'source' },
  a => { a.results[0].extra = 'typo' }, a => { a.results.push(a.results[0]) },
]) {
  const invalid = artifact()
  mutate(invalid)
  assert.throws(() => validateArtifact(invalid))
}
assert.deepEqual(collectEvidence(`unrelated ok\r\ntest name ... ${MARKER}${JSON.stringify(result('Runtime::new'))}\r\n`, metadata('windows')).results, [result('Runtime::new')])
assert.equal(collectEvidence('ok title\nok close\n', metadata('windows')).results.length, 0, 'never infer APIs from old ok lines')
assert.throws(() => collectEvidence(`${MARKER}{bad json`, metadata('windows')))

assert.equal(cfgApplies('any(windows, target_os = "linux",)', 'windows'), true)
assert.equal(cfgApplies('any(windows, target_os = "linux")', 'macos'), false)
assert.equal(cfgApplies('all(desktop, not(mobile), unix)', 'linux'), true)
assert.equal(cfgApplies('any(debug_assertions, feature = "devtools")', 'linux'), true)
assert.throws(() => cfgApplies('feature = "unreviewed"', 'windows'), /unsupported cfg/)
assert.throws(() => cfgApplies('target_os = "unknown"', 'windows'), /unreviewed target_os/)
assert.throws(() => cfgApplies('not(desktop, mobile)', 'windows'))
assert.throws(() => cfgApplies('desktop || invented', 'windows'), /unsupported cfg/)
const members = traitMembers(`pub trait Example<T: UserEvent>: Debug {
  type Associated: Debug;
  #[cfg(windows)]
  fn parent(&self);
  #[cfg(target_os = "macos")]
  fn parent(&self);
  #[cfg(any(
    target_os = "linux",
    target_os = "freebsd"
  ))]
  #[cfg_attr(docsrs, doc(cfg(target_os = "android")))]
  fn linux(&self);
  fn common(&self);
}`, 'Example')
assert.deepEqual(members.map(member => member.name), ['parent', 'linux', 'common'])
assert.equal(members[0].declarations.length, 2, 'same-name platform variants count once')
assert.equal(cfgApplies(members[1].declarations[0].cfg[0], 'linux'), true)
assert.equal(members[1].declarations[0].cfg.length, 1, 'doc cfg_attr is not a compilation gate')
// Exercise the actual collector CLI too, keeping synthetic evidence outside the repo.
const temporary = mkdtempSync(join(tmpdir(), 'runtime-evidence-test-'))
try {
  const log = join(temporary, 'run.log')
  const output = join(temporary, 'result.json')
  writeFileSync(log, `${MARKER}${JSON.stringify(result('Runtime::new'))}\n`)
  const script = fileURLToPath(new URL('./runtime-evidence.mjs', import.meta.url))
  const args = [script, 'collect', '--log', log, '--output', output, '--status', 'pass']
  const env = { ...process.env, GITHUB_SHA: commit, RUNNER_OS: 'Windows', GITHUB_RUN_ID: '123',
    GITHUB_RUN_ATTEMPT: '1', GITHUB_REPOSITORY: 'example/repo', GITHUB_SERVER_URL: 'https://ci.example' }
  assert.equal(spawnSync(process.execPath, args, { env, stdio: 'ignore' }).status, 0)
  const collected = JSON.parse(readFileSync(output, 'utf8'))
  assert.equal(collected.run.url, 'https://ci.example/example/repo/actions/runs/123/attempts/1')
  assert.equal(readArtifacts(temporary).length, 1)
  assert.equal(cell([collected]), 'pass')
  args[args.length - 1] = 'fail'
  assert.equal(spawnSync(process.execPath, args, { env, stdio: 'ignore' }).status, 0)
  assert.equal(cell(readArtifacts(temporary)), 'unknown', 'a failed command cannot leave yesterday\'s pass')
  args[args.length - 1] = 'success'
  assert.notEqual(spawnSync(process.execPath, args, { env, stdio: 'ignore' }).status, 0)
  assert.throws(() => readArtifacts(join(temporary, 'missing')), /ENOENT/)
  writeFileSync(output, '{}')
  assert.throws(() => readArtifacts(temporary), /missing fields/)
  writeFileSync(output, '{broken json')
  assert.throws(() => readArtifacts(temporary), SyntaxError)

  // Real tiny Node children test the orchestrator, never Cargo/Electron/GUI.
  // Their synthetic markers stay in temp files and ignored child stdout.
  const state = join(temporary, 'order.txt')
  const runner = join(temporary, 'runner.mjs')
  writeFileSync(state, '')
  writeFileSync(runner, `import { runSuites } from ${JSON.stringify(new URL('./runtime-suites.mjs', import.meta.url).href)}; process.exitCode = runSuites(process.argv[2], process.env, JSON.parse(process.argv[3]));`)
  const childCode = `const fs = require('node:fs'); const [state, before, next, code] = process.argv.slice(1); if (fs.readFileSync(state, 'utf8') !== before) process.exit(99); fs.writeFileSync(state, next); console.log(${JSON.stringify(`${MARKER}${JSON.stringify(result('Runtime::new'))}`)}); process.exit(Number(code));`
  const suites = [0, 1, 2].map(index => ({ id: `synthetic-${index}`, command: process.execPath,
    args: ['-e', childCode, state, String(index ? index : ''), String(index + 1), index === 1 ? '7' : '0'] }))
  suites.push({ id: 'macos-only', platforms: ['macos'], command: process.execPath, args: ['-e', 'process.exit(77)'] })
  const suiteDirectory = join(temporary, 'suites')
  const run = spawnSync(process.execPath, [runner, suiteDirectory, JSON.stringify(suites)], { env, stdio: 'ignore' })
  assert.equal(run.status, 1, 'one failed suite makes the complete command fail')
  assert.equal(readFileSync(state, 'utf8'), '3', 'later suites still run sequentially after a failure')
  const reports = readArtifacts(suiteDirectory).sort((a, b) => a.testRun.id.localeCompare(b.testRun.id))
  assert.deepEqual(reports.map(report => report.testRun.status), ['pass', 'fail', 'pass'])
  assert.match(readFileSync(join(suiteDirectory, 'synthetic-1.log'), 'utf8'), /\[runner\] exit=7 signal=none error=none/)
  assert.equal(cell(reports), 'unknown', 'a failed suite marker cannot contribute a lucky pass')
  assert.equal(reports[2].results[0].member, 'Runtime::new')
  assert.equal(existsSync(join(suiteDirectory, 'macos-only.json')), false, 'non-Mac suites must not execute macOS-only commands or synthesize pass artifacts')
  const macDirectory = join(temporary, 'mac-only-suite')
  assert.equal(spawnSync(process.execPath, [runner, macDirectory, JSON.stringify([suites[3]])], { env: { ...env, RUNNER_OS: 'macOS' }, stdio: 'ignore' }).status, 1)
  const macReports = readArtifacts(macDirectory)
  assert.equal(macReports[0].testRun.status, 'fail', 'exit 77 on an applicable platform is not a passing suite')
  assert.equal(macReports[0].results.length, 0, 'no marker is invented for platform-only failures')
  suites[0].command = join(temporary, 'missing-command')
  suites.splice(1)
  assert.equal(spawnSync(process.execPath, [runner, suiteDirectory, JSON.stringify(suites)], { env, stdio: 'ignore' }).status, 1)
  assert.equal(readArtifacts(suiteDirectory).find(report => report.testRun.id === 'synthetic-0').testRun.status, 'fail', 'spawn failure replaces old passing evidence')
} finally {
  assert.equal(dirname(temporary), resolve(tmpdir()))
  rmSync(temporary, { recursive: true })
}

if (existsSync(new URL('../../source/tauri/crates/tauri-runtime/src/lib.rs', import.meta.url))) {
  const { definitions, inventory: desktop, renderCoverage, main } = await import('./coverage.mjs')
  assert.throws(() => main(['--require-platforms']), /missing Windows\/macOS\/Linux/)
  assert.throws(() => main(['--require-suites']), /missing or duplicate suite/)
  assert.throws(() => main(['--evidence', 'missing', '--write']), /stay deterministic/)
  assert.throws(() => main(['--evidence', '--commit']), /missing value/)
  assert.equal(definitions(`impl<T: UserEvent> RuntimeHandle<T> for Handle<T> {
  fn create_window(&self) { self.context.create() }
}
impl<T: UserEvent> Runtime<T> for App<T> {
  fn create_window(&self) { self.create() }
}`, 'Runtime').get('create_window').line, 5)
  assert.equal(desktop.filter(member => member.scope === 'runtime').length, 192)
  assert.equal(desktop.filter(member => member.scope === 'extension').length, 5)
  assert.ok(desktop.some(member => member.id === 'WindowBuilder::inner_size'))
  assert.ok(desktop.some(member => member.id === 'EventLoopProxy::send_event'))
  assert.ok(!desktop.some(member => member.id === 'WindowDispatch::activity_name'))
  assert.deepEqual(aggregateEvidence(desktop, []).runtime, {
    windows: { passed: 0, total: 169 }, macos: { passed: 0, total: 181 },
    linux: { passed: 0, total: 158 }, all: { passed: 0, total: 192 },
  })
  const markdown = renderCoverage()
  assert.match(markdown, /Windows \| macOS \| Linux/)
  assert.match(markdown, /未导入可追溯/)
  assert.doesNotMatch(markdown, /有路径|wired|✅ 通过|当前实现读取 Rust 缓存|仅警告并返回 Ok，不迁移|主页面关闭受窗口关闭策略影响/)
  assert.match(markdown, /关闭 webview 原生视图，保留所属宿主窗口/)
  assert.match(markdown, /查询 Electron 当前已提交 URL/)
  assert.match(markdown, /迁移原生视图及 Rust 所属关系/)
  const passed = aggregateEvidence(desktop, PLATFORMS.map(platform => artifact(platform)), { commit, runId })
  assert.match(renderCoverage(passed), /✅ 通过/)
  assert.match(renderCoverage(passed), /runtime-suite \/ smoke::title/)
}
// Static wiring assertions are regression checks, never runtime evidence.
const workflow = readFileSync(new URL('../../.github/workflows/ci.yml', import.meta.url), 'utf8')
assert.match(workflow, /sh -c 'openbox & xcompmgr & sleep 1; node tests\/checks\/runtime-suites\.mjs "\$1"' sh "\$output"/)
assert.match(workflow, /else\s+node tests\/checks\/runtime-suites\.mjs "\$output"\s+fi/)
assert.match(workflow, /name: runtime-evidence-\$\{\{ matrix\.os \}\}-\$\{\{ github\.run_attempt \}\}/)
assert.match(workflow, /pattern: runtime-evidence-\*-\$\{\{ github\.run_attempt \}\}/)
assert.match(workflow, /--require-platforms --require-suites --min-percent 80/)
assert.match(workflow, /apt-get install[^\n]*x11-utils[^\n]*xdotool[^\n]*xcompmgr/)
console.log('runtime evidence: schema, exact revision/run, failed suites, false positives, sequential runner, cfg/N/A and denominator checks passed')
