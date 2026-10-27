// Sequential GUI suites. Each command gets its own raw log and exact-exit artifact.
import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { appendFileSync, closeSync, mkdirSync, openSync, readFileSync, writeFileSync } from 'node:fs'
import { join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { ciMetadata, collectEvidence, PLATFORMS } from './runtime-evidence.mjs'

export const SUITES = [
  { id: 'runtime-tests', command: 'cargo', args: ['test', '--locked', '--', '--nocapture', '--test-threads=1'] },
  ...['new_window_smoke', 'tauri_new_window_smoke', 'runtime_behavior_smoke', 'webview_behavior_smoke', 'desktop_behavior_smoke', 'system_behavior_smoke', 'macos_behavior_smoke', 'windows_behavior_smoke', 'linux_behavior_smoke', 'rendering_behavior_smoke'].map(id => ({
    id, command: 'cargo', args: ['run', '--locked', ...(id === 'tauri_new_window_smoke' ? ['--features', 'tauri'] : []), '--bin', id],
    ...(id === 'macos_behavior_smoke' ? { platforms: ['macos'] } : {}),
    ...(id === 'windows_behavior_smoke' ? { platforms: ['windows'] } : {}),
    ...(id === 'linux_behavior_smoke' ? { platforms: ['linux'] } : {}),
  })),
]

export function suitesFor(platform, suites = SUITES) {
  assert.ok(PLATFORMS.includes(platform), `unrecognized suite platform: ${platform}`)
  return suites.filter(suite => !suite.platforms || suite.platforms.includes(platform))
}

export function requireSuites(artifacts) {
  for (const artifact of artifacts) {
    assert.ok(SUITES.some(suite => suite.id === artifact.testRun.id), `unrecognized CI suite: ${artifact.testRun.id}`)
    assert.ok(suitesFor(artifact.platform).some(suite => suite.id === artifact.testRun.id), `suite not applicable: ${artifact.platform}/${artifact.testRun.id}`)
  }
  for (const platform of PLATFORMS) for (const suite of suitesFor(platform)) {
    const matches = artifacts.filter(artifact => artifact.platform === platform && artifact.testRun.id === suite.id)
    assert.equal(matches.length, 1, `missing or duplicate suite: ${platform}/${suite.id}`)
    assert.equal(matches[0].testRun.status, 'pass', `suite did not pass: ${platform}/${suite.id}`)
  }
}

export function runSuites(directory, env = process.env, suites = SUITES) {
  // Validate CI identity before starting expensive processes or claiming evidence.
  const metadata = collectEvidence('', ciMetadata(env, 'runtime-tests', 'unknown'))
  suites = suitesFor(metadata.platform, suites)
  mkdirSync(directory, { recursive: true })
  for (const suite of suites) {
    assert.match(suite.id, /^[a-z][a-z0-9_-]+$/, 'unsafe suite id')
    // A cached or interrupted attempt cannot leave a valid pass for an unrun suite.
    writeFileSync(join(directory, `${suite.id}.json`), '{}\n')
  }
  let failed = false
  for (const suite of suites) {
    const log = join(directory, `${suite.id}.log`)
    console.log(`::group::${suite.id}`)
    const fd = openSync(log, 'w')
    let result
    try {
      // No pipes: Electron descendants retaining stdout cannot hang collection.
      result = spawnSync(suite.command, suite.args, { env, stdio: ['ignore', fd, fd] })
    } finally {
      closeSync(fd)
    }
    const status = result.status === 0 && !result.error && !result.signal ? 'pass' : 'fail'
    const output = readFileSync(log, 'utf8')
    // Runner diagnostics are not child assertion output (arguments may quote a marker).
    appendFileSync(log, `\n[runner] ${suite.command} ${suite.args.join(' ')}\n[runner] exit=${result.status} signal=${result.signal ?? 'none'} error=${result.error?.message ?? 'none'}\n`)
    process.stdout.write(output)
    if (result.error) console.error(`${suite.id}: ${result.error.message}`)
    console.log(`${suite.id}: ${status} (exit ${result.status}, signal ${result.signal ?? 'none'})`)
    try {
      const artifact = collectEvidence(output, ciMetadata(env, suite.id, status))
      writeFileSync(join(directory, `${suite.id}.json`), `${JSON.stringify(artifact, null, 2)}\n`)
    } catch (error) {
      failed = true // Keep invalid sentinel; aggregation must reject malformed evidence.
      console.error(`${suite.id}: ${error.message}`)
    }
    console.log('::endgroup::')
    failed ||= status !== 'pass'
  }
  return failed ? 1 : 0
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  assert.equal(process.argv.length, 3, 'expected runtime-suites.mjs OUTPUT_DIRECTORY')
  process.exitCode = runSuites(process.argv[2])
}
