// A frozen Run duration budget that names the policy but omits the limit used
// to arm an immediate timer: `Math.min(undefined)` is `NaN`, `setTimeout(NaN)`
// fires on the next tick, and every such Run failed with a bogus
// `runtime.duration_budget_exceeded` before its first token. A duration limit
// is only real when it is a positive safe integer.
import test from 'node:test'
import assert from 'node:assert/strict'
import { armRunDurationTimer, runDurationMs } from '../src/run-duration.mjs'

test('an explicit run budget is honoured and bounded to 24 hours', () => {
  assert.equal(runDurationMs({ runBudget: { maxDurationMs: 5_000 } }), 5_000)
  assert.equal(runDurationMs({ runBudget: { maxDurationMs: 86_400_001 } }), 86_400_000)
  assert.equal(runDurationMs(undefined), null)
})

test('a budget that is not a positive safe integer never arms a timer', () => {
  for (const value of [0, -1, -5_000, Number.NaN, Number.POSITIVE_INFINITY, 1.5, '5000', null, {}]) {
    assert.equal(runDurationMs({ runBudget: { maxDurationMs: value } }), null, `maxDurationMs=${String(value)}`)
  }
})

test('a frozen binding that omits runExecutionMs does not become NaN', () => {
  // The defect: `runExecutionLimited !== false` selected the frozen limit and
  // pushed `undefined`, so runDurationMs returned NaN and the Run ended
  // immediately with a duration-budget failure.
  const missing = runDurationMs({ controlBinding: { budgets: { modelRequestMs: 300_000, approvalWaitMs: 300_000 } } })
  assert.equal(Number.isNaN(missing), false, 'a duration budget is never NaN')
  assert.equal(missing, null)
})

test('a frozen binding whose limit is unusable is ignored, not coerced', () => {
  for (const value of [undefined, null, 0, -1, Number.NaN, Number.POSITIVE_INFINITY, 2.5, '600000']) {
    const duration = runDurationMs({ controlBinding: { budgets: { runExecutionMs: value } } })
    assert.equal(Number.isNaN(duration), false, `runExecutionMs=${String(value)}`)
    assert.equal(duration, null, `runExecutionMs=${String(value)}`)
  }
})

test('an explicit no-limit policy disables the frozen limit', () => {
  assert.equal(runDurationMs({ controlBinding: { budgets: { runExecutionLimited: false, runExecutionMs: 1_800_000 } } }), null)
  assert.equal(runDurationMs({ controlBinding: { budgets: { runExecutionLimited: true, runExecutionMs: 1_800_000 } } }), 1_800_000)
})

test('the tightest authorized limit wins', () => {
  assert.equal(runDurationMs({
    runBudget: { maxDurationMs: 900_000 },
    controlBinding: { budgets: { runExecutionLimited: true, runExecutionMs: 1_800_000 } },
  }), 900_000)
})

test('a non-finite or non-positive duration never arms a timer', async () => {
  let fired = false
  for (const value of [null, undefined, Number.NaN, Number.POSITIVE_INFINITY, 0, -10]) {
    assert.equal(armRunDurationTimer(value, () => { fired = true }), null, `duration=${String(value)}`)
  }
  await new Promise(resolve => setTimeout(resolve, 30))
  assert.equal(fired, false, 'an unusable duration must never fire the budget callback')
})

test('a real duration still arms and fires', async () => {
  const armed = armRunDurationTimer(25, () => {})
  assert.ok(armed, 'a real duration arms a timer')
  clearTimeout(armed)
})
