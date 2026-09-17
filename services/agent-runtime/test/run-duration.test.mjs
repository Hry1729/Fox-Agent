import test from 'node:test'
import assert from 'node:assert/strict'
import { runDurationMs, armRunDurationTimer } from '../src/run-duration.mjs'

test('ordinary and continuous runs survive a day without a whole-task timeout', context => {
  context.mock.timers.enable({ apis: ['setTimeout'] })
  let timedOut = 0
  for (const payload of [{}, { controlBinding: { budgets: { runExecutionMs: 1_800_000, runExecutionLimited: false } } }]) {
    assert.equal(armRunDurationTimer(runDurationMs(payload), () => timedOut++), null)
  }
  context.mock.timers.tick(86_400_000)
  assert.equal(timedOut, 0)
})

test('explicit child limits and old frozen limits still fire exactly once', context => {
  context.mock.timers.enable({ apis: ['setTimeout'] })
  let timedOut = 0
  const frozen = { runExecutionMs: 100 }
  assert.equal(runDurationMs({ controlBinding: { budgets: frozen } }), 100)
  const payload = { controlBinding: { budgets: { ...frozen, runExecutionLimited: false } }, runBudget: { maxDurationMs: 50 } }
  armRunDurationTimer(runDurationMs(payload), () => timedOut++)
  context.mock.timers.tick(49)
  assert.equal(timedOut, 0)
  context.mock.timers.tick(1)
  assert.equal(timedOut, 1)
  context.mock.timers.tick(1000)
  assert.equal(timedOut, 1)
  assert.equal(runDurationMs({ controlBinding: { budgets: frozen }, runBudget: { maxDurationMs: 150 } }), 100)
})
