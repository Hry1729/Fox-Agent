import test from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import {
  CACHE_MODEL,
  COMPLETENESS,
  DEFAULT_ATTEMPT_ID,
  REQUEST_IDENTITY_SCOPE,
  USAGE_SCHEMA_VERSION,
  aggregateUsage,
  assertNoUnsupportedConfigFields,
  buildConfigLayers,
  cacheModelForApi,
  computeCost,
  createRequestLedger,
  createUsageRecord,
  normalizeUsage,
  requestUnitKey,
  resolvePrice,
  summarizeTimings,
  summarizeTokens,
  validatePriceTable,
} from '../src/usage-accounting.mjs'

const PRICE_TABLE = {
  priceVersion: 'test-fixture-2026-09-19',
  unit: 'per_million_tokens',
  defaultCurrency: 'USD',
  verified: true,
  models: {
    'openai/gpt-test': {
      currency: 'USD',
      effectiveAt: '2026-01-01T00:00:00Z',
      verifiedAt: '2026-01-02T00:00:00Z',
      source: 'test-fixture',
      rates: { input: 1, output: 2, cacheRead: 0.5, cacheWrite: 0.25 },
    },
    'deepseek/deepseek-test': {
      currency: 'USD',
      effectiveAt: '2026-01-01T00:00:00Z',
      verifiedAt: '2026-01-02T00:00:00Z',
      source: 'test-fixture',
      rates: { input: 1, output: null, cacheRead: null, cacheWrite: null },
    },
    'anthropic/claude-test': {
      currency: 'CNY',
      effectiveAt: '2026-01-01T00:00:00Z',
      verifiedAt: null,
      source: 'test-fixture',
      rates: { input: 7, output: 21, cacheRead: 0.7, cacheWrite: 8.75 },
    },
  },
}

function recordFrom(overrides = {}) {
  return createUsageRecord({
    eventId: overrides.eventId ?? 'evt-1',
    requestId: overrides.requestId ?? 'req-1',
    runId: overrides.runId ?? 'run-1',
    provider: overrides.provider ?? 'openai',
    modelId: overrides.modelId ?? 'gpt-test',
    attemptId: overrides.attemptId ?? 'attempt-1',
    stage: overrides.stage ?? 'agent',
    outcome: overrides.outcome ?? 'success',
    api: overrides.api ?? 'openai-completions',
    rawUsage: overrides.rawUsage,
    priceTable: 'priceTable' in overrides ? overrides.priceTable : PRICE_TABLE,
    at: overrides.at ?? '2026-06-01T00:00:00Z',
    timings: overrides.timings ?? {},
    config: overrides.config ?? null,
    cacheModel: overrides.cacheModel ?? null,
  })
}

test('inclusive cache providers report billable input without double counting cached tokens', () => {
  const record = recordFrom({
    rawUsage: { prompt_tokens: 1000, completion_tokens: 200, prompt_tokens_details: { cached_tokens: 400 } },
  })
  assert.equal(record.usage.cacheModel, CACHE_MODEL.INCLUSIVE)
  assert.equal(record.usage.input, 600)
  assert.equal(record.usage.cacheRead, 400)
  assert.equal(record.usage.cacheWrite, 0)
  assert.equal(record.usage.completeness, COMPLETENESS.COMPLETE)
  // The prompt total is preserved exactly once across input + cacheRead.
  assert.equal(record.usage.input + record.usage.cacheRead, 1000)
  // 600*1 + 200*2 + 400*0.5, per million tokens.
  assert.equal(record.cost.knownCost, 0.0012)
  assert.equal(record.cost.costComplete, true)
})

test('inclusive cache providers keep the split unknown when cached tokens are unreported', () => {
  const record = recordFrom({ rawUsage: { prompt_tokens: 1000, completion_tokens: 200 } })
  assert.equal(record.usage.input, 1000)
  assert.equal(record.usage.cacheRead, null)
  assert.equal(record.usage.completeness, COMPLETENESS.PARTIAL)
  assert.ok(record.usage.missingFields.includes('cacheRead'))
  assert.ok(record.usage.notes.some((note) => /cache split unknown/.test(note)))
  // Billing the whole prompt as input over-charges, so the partial cost is not a lower bound.
  assert.equal(record.usage.overApproximated, true)
  assert.equal(record.cost.costComplete, false)
  assert.equal(record.cost.bound, 'indeterminate')
})

test('exclusive cache providers keep cache read and write outside input', () => {
  const record = recordFrom({
    provider: 'anthropic',
    modelId: 'claude-test',
    api: 'anthropic-messages',
    rawUsage: {
      input_tokens: 600,
      output_tokens: 200,
      cache_read_input_tokens: 400,
      cache_creation_input_tokens: 100,
    },
  })
  assert.equal(record.usage.cacheModel, CACHE_MODEL.EXCLUSIVE)
  assert.equal(record.usage.input, 600)
  assert.equal(record.usage.cacheRead, 400)
  assert.equal(record.usage.cacheWrite, 100)
  assert.equal(record.currency, 'CNY')
  // (600*7 + 200*21 + 400*0.7 + 100*8.75) / 1e6
  assert.equal(record.cost.knownCost, 0.009555)
  assert.equal(record.cost.costComplete, true)
})

test('a provider without prompt caching reports a known zero rather than a missing value', () => {
  const record = recordFrom({ rawUsage: { prompt_tokens: 10, completion_tokens: 5 }, cacheModel: CACHE_MODEL.NONE })
  assert.equal(record.usage.cacheRead, 0)
  assert.equal(record.usage.cacheWrite, 0)
  assert.equal(record.usage.completeness, COMPLETENESS.COMPLETE)
  assert.notEqual(record.usage.cacheRead, null)
})

test('missing usage stays unavailable and can never be priced as free', () => {
  const record = recordFrom({ rawUsage: null })
  assert.equal(record.usage.completeness, COMPLETENESS.UNAVAILABLE)
  assert.equal(record.usage.input, null)
  assert.equal(record.usage.output, null)
  assert.equal(record.cost.knownCost, null)
  assert.equal(record.cost.costComplete, false)
})

test('a genuinely zero-token request prices as zero while an unpriced one stays null', () => {
  const zero = recordFrom({
    rawUsage: { prompt_tokens: 0, completion_tokens: 0, prompt_tokens_details: { cached_tokens: 0 } },
  })
  assert.equal(zero.cost.knownCost, 0)
  assert.equal(zero.cost.costComplete, true)

  const unpriced = recordFrom({
    provider: 'openai',
    modelId: 'unknown-model',
    rawUsage: { prompt_tokens: 0, completion_tokens: 0, prompt_tokens_details: { cached_tokens: 0 } },
  })
  assert.equal(unpriced.cost.knownCost, null)
  assert.equal(unpriced.cost.costComplete, false)
  assert.equal(unpriced.cost.missing[0].reason, 'no_price_entry')
})

test('an unknown price yields tokens only, with the missing reason kept', () => {
  const record = recordFrom({
    modelId: 'never-priced',
    rawUsage: { prompt_tokens: 100, completion_tokens: 50, prompt_tokens_details: { cached_tokens: 0 } },
  })
  assert.equal(record.cost.knownCost, null)
  assert.equal(record.priceVersion, PRICE_TABLE.priceVersion)
  assert.notEqual(record.usage.input, null)
  assert.equal(record.cost.missing[0].reason, 'no_price_entry')
})

test('a partially priced request reports a labelled lower bound instead of a full cost', () => {
  const record = recordFrom({
    provider: 'deepseek',
    modelId: 'deepseek-test',
    rawUsage: { prompt_tokens: 1000, completion_tokens: 100, prompt_tokens_details: { cached_tokens: 0 } },
  })
  assert.equal(record.cost.knownCost, 0.001)
  assert.equal(record.cost.costComplete, false)
  assert.equal(record.cost.bound, 'lower')
  assert.deepEqual(
    record.cost.missing.map((entry) => entry.reason),
    ['unknown_rate'],
  )
})

test('a rate card younger than the request is never used to price it', () => {
  const future = { ...PRICE_TABLE, models: { 'openai/gpt-test': { ...PRICE_TABLE.models['openai/gpt-test'], effectiveAt: '2027-01-01T00:00:00Z' } } }
  const resolved = resolvePrice({ provider: 'openai', modelId: 'gpt-test', at: '2026-06-01T00:00:00Z' }, future)
  assert.equal(resolved.matched, false)
  assert.equal(resolved.reason, 'price_not_effective_yet')
})

test('retries are new accounting units while re-delivered events are dropped', () => {
  const ledger = createRequestLedger()
  const first = recordFrom({ eventId: 'evt-a', requestId: 'req-x', attemptId: 'attempt-1', outcome: 'failure' })
  const retry = recordFrom({ eventId: 'evt-b', requestId: 'req-x', attemptId: 'attempt-2', outcome: 'success' })
  const duplicateEvent = recordFrom({ eventId: 'evt-b', requestId: 'req-x', attemptId: 'attempt-2' })
  const duplicateUnit = recordFrom({ eventId: 'evt-c', requestId: 'req-x', attemptId: 'attempt-2' })

  assert.equal(ledger.record(first).accepted, true)
  assert.equal(ledger.record(retry).accepted, true)
  assert.equal(ledger.record(duplicateEvent).reason, 'duplicate_event')
  assert.equal(ledger.record(duplicateUnit).reason, 'duplicate_request_attempt')
  assert.equal(ledger.size(), 2)
})

test('failed, retried, planned, reviewed, compacted and subtask requests all reach the ledger', () => {
  const records = [
    recordFrom({ eventId: 'e1', requestId: 'r1', attemptId: 'a1', stage: 'agent' }),
    recordFrom({ eventId: 'e2', requestId: 'r2', attemptId: 'a1', stage: 'planner', outcome: 'failure' }),
    recordFrom({ eventId: 'e3', requestId: 'r2', attemptId: 'a2', stage: 'planner' }),
    recordFrom({ eventId: 'e4', requestId: 'r3', attemptId: 'a1', stage: 'reviewer' }),
    recordFrom({ eventId: 'e5', requestId: 'r4', attemptId: 'a1', stage: 'compaction' }),
    recordFrom({ eventId: 'e6', requestId: 'r5', attemptId: 'a1', stage: 'subtask', parentRequestId: 'r1' }),
  ]
  const aggregate = aggregateUsage(records)
  assert.equal(aggregate.requestCount, 6)
  assert.equal(aggregate.byCurrency.USD.requestCount, 6)
  assert.equal(aggregate.byCurrency.USD.failureCount, 1)
  assert.deepEqual(aggregate.byCurrency.USD.stageCounts, {
    agent: 1,
    planner: 2,
    reviewer: 1,
    compaction: 1,
    subtask: 1,
  })
})

test('parallel stage durations are not presented as the wall clock', () => {
  const records = [
    recordFrom({ eventId: 'p1', requestId: 'r1', timings: { modelRequestMs: 1000, ttftMs: 120 } }),
    recordFrom({ eventId: 'p2', requestId: 'r2', timings: { modelRequestMs: 1000, ttftMs: 150 } }),
  ]
  const timings = summarizeTimings(records, { startedAtMs: 0, endedAtMs: 1200 })
  assert.equal(timings.modelRequestMsTotal, 2000)
  assert.equal(timings.wallClockMs, 1200)
  assert.notEqual(timings.wallClockMs, timings.modelRequestMsTotal)
  assert.equal(timings.sumOfStageDurationsIsNotWallClock, true)
  assert.equal(timings.ttftMsTotal, 270)
})

test('missing timing marks leave the wall clock unknown instead of estimated', () => {
  const timings = summarizeTimings([recordFrom({ eventId: 't1', timings: { modelRequestMs: 500 } })])
  assert.equal(timings.wallClockMs, null)
  assert.equal(timings.recordsMissingTimingMarks.ttftMs, 1)
  assert.equal(timings.approvalWaitMs, 0)
})

test('currencies are never summed together', () => {
  const records = [
    recordFrom({ eventId: 'c1', requestId: 'r1', rawUsage: { prompt_tokens: 1000, completion_tokens: 0, prompt_tokens_details: { cached_tokens: 0 } } }),
    recordFrom({
      eventId: 'c2',
      requestId: 'r2',
      provider: 'anthropic',
      modelId: 'claude-test',
      api: 'anthropic-messages',
      rawUsage: { input_tokens: 1000, output_tokens: 0, cache_read_input_tokens: 0, cache_creation_input_tokens: 0 },
    }),
  ]
  const aggregate = aggregateUsage(records)
  assert.equal(aggregate.mixedCurrencies, true)
  assert.deepEqual(Object.keys(aggregate.byCurrency).sort(), ['CNY', 'USD'])
  assert.equal(aggregate.byCurrency.USD.cost.knownCost, 0.001)
  assert.equal(aggregate.byCurrency.CNY.cost.knownCost, 0.007)

  const usdOnly = aggregateUsage(records, { currency: 'USD' })
  assert.equal(usdOnly.requestCount, 1)
  assert.deepEqual(Object.keys(usdOnly.byCurrency), ['USD'])
})

test('an experiment with no requests reports a null cost rather than zero', () => {
  const aggregate = aggregateUsage([])
  assert.equal(aggregate.requestCount, 0)
  assert.equal(aggregate.byCurrency.USD, undefined)
  assert.equal(aggregate.timings.wallClockMs, null)
})

test('configuration layers keep raw, resolved and sent apart and surface unsupported fields', () => {
  const layers = buildConfigLayers({
    raw: { modelId: 'gpt-test', contextWindow: 128000, thinkingLevel: 'high' },
    resolved: { modelId: 'gpt-test', contextWindow: 128000, thinkingLevel: 'medium' },
    sent: { model: 'gpt-test', max_completion_tokens: 8192, reasoning_effort: 'medium', temperature: 0.2 },
    allowedSendFields: ['model', 'max_completion_tokens', 'reasoning_effort'],
  })
  assert.deepEqual(layers.rejected, [{ field: 'temperature', reason: 'unsupported_field' }])
  assert.deepEqual(layers.raw, { modelId: 'gpt-test', contextWindow: 128000, thinkingLevel: 'high' })
  assert.deepEqual(layers.resolved, { modelId: 'gpt-test', contextWindow: 128000, thinkingLevel: 'medium' })
  assert.throws(() => assertNoUnsupportedConfigFields(layers), (error) => {
    assert.equal(error.code, 'usage.unsupported_config_field')
    assert.deepEqual(error.fields, ['temperature'])
    return true
  })
  assert.doesNotThrow(() => assertNoUnsupportedConfigFields(buildConfigLayers({ sent: { model: 'x' }, allowedSendFields: ['model'] })))
})

test('records reject unknown identity, stage and outcome values', () => {
  assert.throws(() => recordFrom({ requestId: '' }), (error) => error.code === 'usage.missing_identity')
  assert.throws(() => recordFrom({ stage: 'guesswork' }), (error) => error.code === 'usage.unknown_stage')
  assert.throws(() => recordFrom({ outcome: 'probably-fine' }), (error) => error.code === 'usage.unknown_outcome')
})

test('records are frozen and identify the schema version E and F may rely on', () => {
  const record = recordFrom({ rawUsage: { prompt_tokens: 1, completion_tokens: 1, prompt_tokens_details: { cached_tokens: 0 } } })
  assert.equal(record.schemaVersion, USAGE_SCHEMA_VERSION)
  assert.equal(Object.isFrozen(record), true)
})

test('protocol cache semantics come from the API shape, not from the model name', () => {
  assert.equal(cacheModelForApi('anthropic-messages'), CACHE_MODEL.EXCLUSIVE)
  assert.equal(cacheModelForApi('openai-completions'), CACHE_MODEL.INCLUSIVE)
  assert.equal(cacheModelForApi('openai-responses'), CACHE_MODEL.INCLUSIVE)
  assert.equal(cacheModelForApi('faux'), CACHE_MODEL.NONE)
  assert.equal(cacheModelForApi('mystery-protocol'), CACHE_MODEL.UNKNOWN)
  const unknown = normalizeUsage({ rawUsage: { prompt_tokens: 10, completion_tokens: 1 }, api: 'mystery-protocol' })
  assert.equal(unknown.cacheRead, null)
  assert.equal(unknown.completeness, COMPLETENESS.PARTIAL)
})

test('price table validation separates structural problems from rate correctness', () => {
  assert.deepEqual(validatePriceTable(PRICE_TABLE), [])
  const broken = {
    priceVersion: '',
    unit: 'per_token',
    models: { 'x/y': { currency: '', effectiveAt: 'not-a-date', rates: { input: -1 } } },
  }
  const problems = validatePriceTable(broken)
  assert.ok(problems.some((problem) => /priceVersion/.test(problem)))
  assert.ok(problems.some((problem) => /unit/.test(problem)))
  assert.ok(problems.some((problem) => /currency/.test(problem)))
  assert.ok(problems.some((problem) => /effectiveAt/.test(problem)))
  assert.ok(problems.some((problem) => /rates\.input/.test(problem)))
})

test('the shipped price table is structurally valid and honest about being unverified', () => {
  const shipped = JSON.parse(readFileSync(new URL('../src/model-prices.json', import.meta.url), 'utf8'))
  assert.deepEqual(validatePriceTable(shipped), [])
  assert.equal(shipped.verified, false)
  assert.deepEqual(shipped.models, {})
  const aggregate = aggregateUsage([
    recordFrom({ priceTable: shipped, rawUsage: { prompt_tokens: 1000, completion_tokens: 100, prompt_tokens_details: { cached_tokens: 0 } } }),
  ])
  // Without a matched price entry there is no currency to attribute the request to.
  assert.deepEqual(Object.keys(aggregate.byCurrency), ['unknown'])
  assert.equal(aggregate.byCurrency.unknown.cost.knownCost, null)
  assert.equal(aggregate.byCurrency.unknown.cost.costComplete, false)
})

// ---------------------------------------------------------------------------
// O-REVIEW-01 / O-D-01 regressions, promoted to long-term tests. Each case
// below failed against 336e66f and is kept so it cannot silently return.
// ---------------------------------------------------------------------------

test('O-D-01(1): an unknown cache model never claims a lower bound it cannot prove', () => {
  const record = recordFrom({ api: 'mystery-protocol', rawUsage: { prompt_tokens: 1000, completion_tokens: 0 } })
  assert.equal(record.usage.cacheModel, CACHE_MODEL.UNKNOWN)
  assert.equal(record.usage.input, 1000)
  assert.equal(record.usage.cacheRead, null)
  assert.equal(record.usage.overApproximated, true)
  // The tokens are still reported, but the direction is undecidable: if this endpoint's prompt
  // total already contains cached tokens, 1000 input tokens over-charges.
  assert.equal(record.cost.knownCost, 0.001)
  assert.notEqual(record.cost.bound, 'lower')
  assert.equal(record.cost.bound, 'indeterminate')
  assert.equal(record.cost.costComplete, false)
  assert.deepEqual(
    record.cost.missing.map((entry) => entry.component).sort(),
    ['cacheRead', 'cacheWrite'],
  )
})

test('O-D-01(1): a floor is still reported where the floor is actually provable', () => {
  // Exclusive API that reported no cache fields at all: the price table has rates for every
  // component, so each skipped component can only ever add cost.
  const record = recordFrom({
    provider: 'anthropic',
    modelId: 'claude-test',
    api: 'anthropic-messages',
    rawUsage: { input_tokens: 1000, output_tokens: 100 },
  })
  assert.equal(record.usage.overApproximated, false)
  assert.equal(record.cost.bound, 'lower')
  assert.equal(record.cost.costComplete, false)
  // (1000*7 + 100*21) / 1e6
  assert.equal(record.cost.knownCost, 0.0091)

  // Nothing priced at all: no partial sum exists, so no bound is claimed either.
  const unpriced = recordFrom({
    provider: 'deepseek',
    modelId: 'deepseek-test',
    api: 'mystery-protocol',
    rawUsage: { completion_tokens: 100 },
  })
  assert.equal(unpriced.usage.input, null)
  assert.equal(unpriced.cost.knownCost, null)
  assert.equal(unpriced.cost.bound, null)
})

test('O-D-01(2): an aggregate of unknown token counts preserves the unknown instead of summing to zero', () => {
  const summary = aggregateUsage([recordFrom({ rawUsage: null })])
  assert.equal(summary.tokens.input, null)
  assert.notEqual(summary.tokens.input, 0)
  assert.equal(summary.tokens.output, null)
  assert.equal(summary.tokensComplete, false)
  assert.equal(summary.tokenCompleteness.input.value, null)
  assert.equal(summary.tokenCompleteness.input.knownSum, 0)
  assert.equal(summary.tokenCompleteness.input.knownCount, 0)
  assert.equal(summary.tokenCompleteness.input.missingCount, 1)
  assert.equal(summary.tokenCompleteness.input.complete, false)
  assert.equal(summary.tokenCompleteness.input.reason, 'unknown_contributors')
  assert.equal(summary.byCurrency.USD.tokens.input, null)
  assert.equal(summary.byCurrency.USD.tokensComplete, false)
})

test('O-D-01(2): a partly known component reports a floor beside its missing count', () => {
  const summary = aggregateUsage([
    recordFrom({ eventId: 'k1', requestId: 'r1', rawUsage: { prompt_tokens: 100, completion_tokens: 10, prompt_tokens_details: { cached_tokens: 0 } } }),
    recordFrom({ eventId: 'k2', requestId: 'r2', rawUsage: null }),
  ])
  assert.equal(summary.tokens.input, null)
  assert.equal(summary.tokenCompleteness.input.knownSum, 100)
  assert.equal(summary.tokenCompleteness.input.knownCount, 1)
  assert.equal(summary.tokenCompleteness.input.missingCount, 1)
  assert.equal(summary.tokensComplete, false)

  // With no unknown contributor the same component is a real total, so the distinction is
  // between "0 because nothing happened" and "null because something is unknown".
  const complete = aggregateUsage([
    recordFrom({ eventId: 'k3', requestId: 'r3', rawUsage: { prompt_tokens: 100, completion_tokens: 10, prompt_tokens_details: { cached_tokens: 0 } } }),
    recordFrom({ eventId: 'k4', requestId: 'r4', rawUsage: { prompt_tokens: 200, completion_tokens: 20, prompt_tokens_details: { cached_tokens: 0 } } }),
  ])
  assert.equal(complete.tokens.input, 300)
  assert.equal(complete.tokens.output, 30)
  assert.equal(complete.tokens.cacheRead, 0)
  assert.equal(complete.tokensComplete, true)

  // An empty set has no total either: null with an explicit reason, not a zero.
  assert.equal(summarizeTokens([]).input.value, null)
  assert.equal(summarizeTokens([]).input.reason, 'no_records')
  assert.equal(summarizeTokens([], []).input, undefined)
})

test('O-D-01(3): request/attempt identity is a tuple, not a delimited string', () => {
  const ledger = createRequestLedger()
  assert.equal(ledger.record(recordFrom({ eventId: 'e1', requestId: 'a::b', attemptId: 'c' })).accepted, true)
  assert.equal(ledger.record(recordFrom({ eventId: 'e2', requestId: 'a', attemptId: 'b::c' })).accepted, true)
  assert.equal(ledger.size(), 2)
  assert.notEqual(
    requestUnitKey({ runId: 'run-1', requestId: 'a::b', attemptId: 'c' }),
    requestUnitKey({ runId: 'run-1', requestId: 'a', attemptId: 'b::c' }),
  )
  // Re-delivery of the identical tuple is still a duplicate.
  assert.equal(ledger.record(recordFrom({ eventId: 'e3', requestId: 'a', attemptId: 'b::c' })).reason, 'duplicate_request_attempt')
})

test('O-D-01(3): the identity boundary is the run, and it is declared rather than assumed', () => {
  const ledger = createRequestLedger()
  assert.equal(ledger.record(recordFrom({ eventId: 'b1', requestId: 'req-1', attemptId: 'a1', runId: 'run-1' })).accepted, true)
  assert.equal(ledger.record(recordFrom({ eventId: 'b2', requestId: 'req-1', attemptId: 'a1', runId: 'run-2' })).accepted, true)
  assert.equal(ledger.size(), 2)
  assert.match(REQUEST_IDENTITY_SCOPE, /unique within its runId/)
  // A record that does not distinguish attempts is one per-request unit, consistently.
  assert.equal(
    requestUnitKey({ runId: 'run-1', requestId: 'req-1' }),
    requestUnitKey({ runId: 'run-1', requestId: 'req-1', attemptId: DEFAULT_ATTEMPT_ID }),
  )
})

test('O-D-01(1)+(3): a retry adds a unit while a re-delivered event cannot double-charge', () => {
  const ledger = createRequestLedger()
  const usage = { prompt_tokens: 1000, completion_tokens: 100, prompt_tokens_details: { cached_tokens: 0 } }
  ledger.record(recordFrom({ eventId: 'x1', requestId: 'rq', attemptId: 'a1', outcome: 'failure', rawUsage: usage }))
  ledger.record(recordFrom({ eventId: 'x2', requestId: 'rq', attemptId: 'a2', outcome: 'success', rawUsage: usage }))
  ledger.record(recordFrom({ eventId: 'x2', requestId: 'rq', attemptId: 'a2', outcome: 'success', rawUsage: usage }))
  const summary = aggregateUsage(ledger.records())
  assert.equal(summary.requestCount, 2)
  assert.equal(summary.byCurrency.USD.requestCount, 2)
  assert.equal(summary.byCurrency.USD.attemptCount, 2)
  assert.equal(summary.byCurrency.USD.failureCount, 1)
  assert.equal(summary.byCurrency.USD.tokens.input, 2000)
  assert.equal(summary.byCurrency.USD.tokensComplete, true)
  // (1000*1 + 100*2) / 1e6 per request, twice.
  assert.equal(summary.byCurrency.USD.cost.knownCost, 0.0024)
  assert.equal(summary.byCurrency.USD.cost.costComplete, true)
})

test('O-D-01(1): an aggregate of mixed cost directions never reports the friendlier one', () => {
  const summary = aggregateUsage([
    // Provable floor: an exclusive API whose cache components were never reported.
    recordFrom({
      eventId: 'm1', requestId: 'm1', provider: 'anthropic', modelId: 'claude-test',
      api: 'anthropic-messages', rawUsage: { input_tokens: 1000, output_tokens: 0 },
    }),
    // Undecidable: an unknown cache model billing the whole prompt as input.
    recordFrom({ eventId: 'm2', requestId: 'm2', api: 'mystery-protocol', rawUsage: { prompt_tokens: 1000, completion_tokens: 0 } }),
  ])
  assert.equal(summary.byCurrency.CNY.cost.bound, 'lower')
  assert.equal(summary.byCurrency.USD.cost.bound, 'indeterminate')
  assert.equal(summary.byCurrency.CNY.cost.costComplete, false)
  assert.equal(summary.byCurrency.USD.cost.costComplete, false)
})

test('O-D-01(1): each cache API shape is mapped explicitly and an unmapped one stays unknown', () => {
  const shapes = [
    { api: 'faux', cacheModel: CACHE_MODEL.NONE },
    { api: 'anthropic-messages', cacheModel: CACHE_MODEL.EXCLUSIVE },
    { api: 'openai-completions', cacheModel: CACHE_MODEL.INCLUSIVE },
    { api: 'openai-responses', cacheModel: CACHE_MODEL.INCLUSIVE },
    { api: undefined, cacheModel: CACHE_MODEL.UNKNOWN },
    { api: 'openai-chat-legacy', cacheModel: CACHE_MODEL.UNKNOWN },
  ]
  for (const shape of shapes) {
    assert.equal(cacheModelForApi(shape.api), shape.cacheModel, String(shape.api))
  }
  // No api shape at all, and no explicit override: cache stays unknown, never billed as 0.
  const bare = normalizeUsage({ rawUsage: { prompt_tokens: 10, completion_tokens: 1 } })
  assert.equal(bare.cacheModel, CACHE_MODEL.UNKNOWN)
  assert.equal(bare.cacheRead, null)
  assert.notEqual(bare.cacheRead, 0)
  assert.equal(bare.completeness, COMPLETENESS.PARTIAL)
})
