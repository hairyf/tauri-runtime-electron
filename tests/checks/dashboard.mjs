// Node-only checks; never written as native/CI member evidence.
import assert from 'node:assert/strict'
import { createChecks, eventually, runChecks } from '../../example/probes.ts'

const observed = []
await runChecks([
  { ids: ['one', 'two'], run: async () => 'readback verified' },
  { ids: ['broken'], run: async () => { throw new Error('readback mismatch') } },
  { ids: ['last'], run: async () => 'continued after failure' },
  { ids: ['default'], run: async () => {} },
], result => observed.push(result))
assert.deepEqual(observed.map(item => [item.id, item.status]), [['one', 'pass'], ['two', 'pass'], ['broken', 'fail'], ['last', 'pass'], ['default', 'pass']])
assert.match(observed[2].detail, /readback mismatch/)
assert.equal(observed[3].detail, 'continued after failure')
assert.equal(observed[4].detail, '实际调用并验证读回结果')
assert.equal(await eventually(async () => 42, value => value === 42), 42)
let attempts = 0
assert.equal(await eventually(async () => ++attempts, value => value === 2), 2)
const checks = createChecks({ window: {}, webviewWindow: {}, dpi: {}, event: {}, app: {} })
assert.equal(new Set(checks.flatMap(check => check.ids)).size, checks.flatMap(check => check.ids).length, 'duplicate live evidence')
assert.ok(checks.some(check => check.ids.includes('WindowDispatch::set_size')))
assert.ok(checks.some(check => check.ids.includes('WindowDispatch::close')))
console.log('dashboard result propagation, readback polling, and unique API bindings passed')
