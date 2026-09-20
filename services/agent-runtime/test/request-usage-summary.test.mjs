import test from 'node:test'
import assert from 'node:assert/strict'
import { createUsageRecord } from '../src/usage-accounting.mjs'
import { summarizeRequestUsage } from '../evals/summarize-request-usage.mjs'

test('E summary deduplicates persisted attempts without turning unknown cost into zero', () => {
  const base = { runId: 'run', requestId: 'request', provider: 'test', modelId: 'model', api: 'openai-completions', stage: 'agent', rawUsage: null }
  const failed = createUsageRecord({ ...base, eventId: 'e0', attemptId: 'attempt-0', outcome: 'failure' })
  const succeeded = createUsageRecord({ ...base, eventId: 'e1', attemptId: 'attempt-1', outcome: 'success', rawUsage: { prompt_tokens: 7, completion_tokens: 2 } })
  const result = summarizeRequestUsage([
    { event_json: JSON.stringify({ type: 'usage.request', record: failed }) },
    { type: 'kernel.usage_record', payload: failed },
    { type: 'usage.request', record: succeeded },
    { type: 'usage.updated', inputTokens: 999 },
  ])
  assert.equal(result.requestCount, 2)
  const bucket = Object.values(result.byCurrency)[0]
  assert.equal(bucket.attemptCount, 2)
  assert.equal(bucket.failureCount, 1)
  assert.equal(bucket.cost.knownCost, null)
  assert.equal(bucket.cost.costComplete, false)
})
