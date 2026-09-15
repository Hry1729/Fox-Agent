// Progress tracker contract for Kernel model rounds: first-response, idle
// (text/thinking/tool-parameter bytes), and whole-round bounds.
import test from 'node:test'
import assert from 'node:assert/strict'
import { createRoundProgress, toolParamBytesOf } from '../src/kernel-model-progress.mjs'

test('silent round trips the first-response bound, not the whole-round bound', () => {
  let at = 0
  const progress = createRoundProgress({
    budgets: { modelRequestMs: 10_000, modelFirstResponseMs: 2_000, modelIdleMs: 3_000 },
    now: () => at,
  })
  at = 1_999
  assert.equal(progress.check(), 'ok')
  at = 2_000
  assert.equal(progress.check(), 'first_response')
  const telemetry = progress.telemetry()
  assert.equal(telemetry.firstResponseMs, null)
  assert.equal(telemetry.textBytes, 0)
})

test('tool-parameter bytes count as progress and reset idleness', () => {
  let at = 0
  const progress = createRoundProgress({
    budgets: { modelRequestMs: 10_000, modelFirstResponseMs: 2_000, modelIdleMs: 3_000 },
    now: () => at,
  })
  at = 500
  assert.equal(progress.note({ text: '', reasoning: '', toolParamBytes: 40_000 }), true)
  assert.equal(progress.telemetry().firstResponseMs, 500)
  at = 500 + 2_999
  assert.equal(progress.check(), 'ok')
  at = 500 + 3_000
  assert.equal(progress.check(), 'idle')
})

test('steady progress still meets the whole-round backstop', () => {
  let at = 0
  const progress = createRoundProgress({
    budgets: { modelRequestMs: 10_000, modelFirstResponseMs: 2_000, modelIdleMs: 3_000 },
    now: () => at,
  })
  for (let step = 1; step <= 9; step += 1) {
    at = step * 1_000
    progress.note({ text: `chunk-${step}-` + 'x'.repeat(step * 100), reasoning: '', toolParamBytes: 0 })
    assert.equal(progress.check(), 'ok')
  }
  at = 10_000
  progress.note({ text: 'chunk-10-' + 'x'.repeat(1000), reasoning: '', toolParamBytes: 0 })
  assert.equal(progress.check(), 'total')
})

test('missing new budgets fall back without crashing old Hosts', () => {
  const progress = createRoundProgress({ budgets: { modelRequestMs: 120_000 } })
  assert.deepEqual(progress.bounds, { total: 120_000, first: 60_000, idle: 120_000 })
  const empty = createRoundProgress({})
  assert.deepEqual(empty.bounds, { total: 120_000, first: 60_000, idle: 120_000 })
})

test('sub-round bounds clamp to the whole-round bound', () => {
  const progress = createRoundProgress({
    budgets: { modelRequestMs: 5_000, modelFirstResponseMs: 60_000, modelIdleMs: 120_000 },
  })
  assert.deepEqual(progress.bounds, { total: 5_000, first: 5_000, idle: 5_000 })
})

test('toolParamBytesOf counts serialized arguments only', () => {
  assert.equal(toolParamBytesOf([]), 0)
  assert.equal(toolParamBytesOf(null), 0)
  const content = [
    { type: 'text', text: 'hello' },
    { type: 'toolCall', id: 'a', name: 'office_edit', arguments: { operations: [1, 2, 3] } },
  ]
  assert.equal(toolParamBytesOf(content), Buffer.byteLength(JSON.stringify({ operations: [1, 2, 3] }), 'utf8'))
})
