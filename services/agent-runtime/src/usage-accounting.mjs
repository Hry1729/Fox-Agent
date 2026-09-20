/**
 * USAGE-v1 — the single production accounting vocabulary for model usage, price and cost.
 *
 * Owner: agent D. Consumers: agent E (experiment ledger) and agent F (cross-module verification).
 *
 * This module is intentionally IO-free and dependency-free so that it can be unit-tested in
 * isolation and reused by any caller. It does NOT read the database, the worker, or events.
 * Wiring this vocabulary into the production request path is a separate, not-yet-granted
 * batch; until that batch lands, every result produced here is *module evidence* only.
 *
 * Accounting unit: one actual model request. A retry is a new attempt and therefore a new
 * accounting unit; a re-delivered event for the same attempt is a duplicate and is dropped.
 *
 * Honesty rules encoded below (CONTRACTS USAGE-v1):
 *   - A missing price is null. A known zero is 0. These are different values and never merged.
 *   - Cache tokens are not mechanically added to input. Whether the provider's input token
 *     count already contains cache reads/writes is provider semantics, captured per record.
 *   - An incomplete total is reported with an explicit direction (`bound`: lower /
 *     indeterminate), never as a silent understatement and never with the wrong direction. The
 *     direction has to be *provable*: only skipped components could add cost, and the moment a
 *     priced component may itself over-count the direction is no longer knowable.
 */

export const USAGE_SCHEMA_VERSION = 'usage-v1'

/** Token accounting completeness. Never collapse `partial` into `complete`. */
export const COMPLETENESS = Object.freeze({
  COMPLETE: 'complete',
  PARTIAL: 'partial',
  UNAVAILABLE: 'unavailable',
})

/**
 * How a provider's reported input token count relates to cache tokens.
 * This is an API-shape fact, not a model-name guess; callers may override it explicitly.
 */
export const CACHE_MODEL = Object.freeze({
  /** input_tokens excludes cache tokens; billable input = input + cacheRead + cacheWrite */
  EXCLUSIVE: 'exclusive',
  /** prompt_tokens already includes cached tokens; billable input = prompt_tokens - cached */
  INCLUSIVE: 'inclusive',
  /** the provider has no prompt-cache concept; cacheRead/cacheWrite are a known 0 */
  NONE: 'none',
  /** unknown: cache tokens stay null, and completeness is downgraded to partial */
  UNKNOWN: 'unknown',
})

export const STAGES = Object.freeze(['agent', 'planner', 'reviewer', 'compaction', 'subtask', 'other'])

export const OUTCOMES = Object.freeze(['success', 'failure', 'cancelled', 'timeout', 'unknown'])

/**
 * Allowed `cost.bound` values (CONTRACTS USAGE-v1). A direction is only ever named when it is
 * provable from the components that were actually consulted:
 *   LOWER         a proven floor — every skipped component could only have added cost
 *   INDETERMINATE a priced component may itself over-count while other components are missing,
 *                 so neither direction is provable
 *   UPPER         reserved by the contract for a provable ceiling. No branch of this module
 *                 produces it today: an over-counting `input` always leaves `cacheRead` unknown
 *                 as well, which is indeterminate rather than upper. It is listed so consumers
 *                 do not treat "I have never seen upper" as "upper is impossible".
 *   null          nothing priced, or the cost is complete and needs no bound
 */
export const COST_BOUND = Object.freeze({
  LOWER: 'lower',
  UPPER: 'upper',
  INDETERMINATE: 'indeterminate',
})

export const PRICE_UNIT = 'per_million_tokens'

/** Timing fields, each with a precise definition so totals cannot be silently reinterpreted. */
export const TIMING_FIELDS = Object.freeze({
  queuedMs: 'time between the request being accepted and the transport starting',
  ttftMs: 'time from transport start to the first streamed token',
  modelRequestMs: 'full provider request duration, excluding approval wait',
  toolMs: 'tool execution time attributed to this request',
  approvalWaitMs: 'human approval wait; never charged to the execution budget',
  wallClockMs: 'observed span of this request; NOT the sum of the other fields',
})

function finiteNonNegative(value) {
  if (value === null || value === undefined || value === '') return null
  const number = Number(value)
  return Number.isFinite(number) && number >= 0 ? number : null
}

function nonNegativeIntegerOrNull(value) {
  const number = finiteNonNegative(value)
  return number === null ? null : Math.round(number)
}

function tokenOrNull(value) {
  return nonNegativeIntegerOrNull(value)
}

/** Round away binary floating point noise without pretending to more precision than we have. */
function roundCost(value) {
  return Math.round(value * 1e12) / 1e12
}

/**
 * Protocol-level cache semantics. Third-party OpenAI-compatible endpoints vary, so callers
 * must pass an explicit override when they have verified the endpoint's behaviour.
 */
export function cacheModelForApi(api) {
  if (api === 'anthropic-messages') return CACHE_MODEL.EXCLUSIVE
  if (api === 'openai-responses' || api === 'openai-completions') return CACHE_MODEL.INCLUSIVE
  if (api === 'faux') return CACHE_MODEL.NONE
  return CACHE_MODEL.UNKNOWN
}

function pickRawTokens(rawUsage) {
  return {
    rawInput: tokenOrNull(rawUsage.prompt_tokens ?? rawUsage.input_tokens),
    output: tokenOrNull(rawUsage.output_tokens ?? rawUsage.completion_tokens),
    cached: tokenOrNull(
      rawUsage.cache_read_input_tokens ??
        rawUsage.cached_tokens ??
        rawUsage.prompt_tokens_details?.cached_tokens,
    ),
    cacheWrite: tokenOrNull(
      rawUsage.cache_creation_input_tokens ??
        rawUsage.cache_write_tokens ??
        rawUsage.prompt_tokens_details?.cache_creation_tokens,
    ),
  }
}

/**
 * Normalize raw provider usage into billable input plus separately-tracked cache tokens.
 *
 * Returns nulls (not zeros) for anything the provider did not report. `completeness` is
 * `unavailable` when there is no usage at all, `partial` when a needed field is missing or the
 * cache relationship is unknown, and `complete` only when every component is accounted for.
 */
export function normalizeUsage({ rawUsage = null, api = null, cacheModel = null } = {}) {
  const resolvedCacheModel = cacheModel || (api ? cacheModelForApi(api) : CACHE_MODEL.UNKNOWN)
  const missingFields = []

  if (rawUsage === null || typeof rawUsage !== 'object') {
    return {
      raw: null,
      input: null,
      output: null,
      cacheRead: null,
      cacheWrite: null,
      cacheModel: resolvedCacheModel,
      completeness: COMPLETENESS.UNAVAILABLE,
      missingFields: ['usage'],
      overApproximated: false,
      notes: ['provider returned no usage object'],
    }
  }

  const { rawInput, output, cached, cacheWrite } = pickRawTokens(rawUsage)
  const notes = []
  let input = null
  let cacheRead = null
  let cacheWriteOut = null
  // True when `input` contains tokens that the provider prices elsewhere (an over-charge),
  // so a partial cost derived from it may not be a lower bound.
  let overApproximated = false

  if (resolvedCacheModel === CACHE_MODEL.INCLUSIVE) {
    cacheRead = cached ?? null
    // These APIs bill no separate cache-write token type, so a known 0 is accurate rather than
    // a guess. The cache-read split, however, does change billing and stays unknown when absent.
    cacheWriteOut = 0
    if (rawInput === null) {
      missingFields.push('input')
    } else if (cached === null) {
      input = rawInput
      overApproximated = true
      missingFields.push('cacheRead')
      notes.push('cache split unknown; the prompt total is reported as input and may contain cache tokens')
    } else {
      input = rawInput - cached
      if (input < 0) {
        notes.push('reported cached tokens exceeded reported input tokens; clamped billable input to 0')
        input = 0
      }
    }
  } else if (resolvedCacheModel === CACHE_MODEL.EXCLUSIVE) {
    input = rawInput
    cacheRead = cached
    cacheWriteOut = cacheWrite
    if (rawInput === null) missingFields.push('input')
    if (cached === null) missingFields.push('cacheRead')
    if (cacheWrite === null) missingFields.push('cacheWrite')
  } else if (resolvedCacheModel === CACHE_MODEL.NONE) {
    input = rawInput
    cacheRead = 0
    cacheWriteOut = 0
    if (rawInput === null) missingFields.push('input')
  } else {
    // Unknown cache relationship: rawInput cannot be asserted to be billable-non-cached.
    input = rawInput
    cacheRead = null
    cacheWriteOut = null
    if (rawInput === null) missingFields.push('input')
    // Two opposite errors are possible at once here and neither can be ruled out from the API
    // shape: if the prompt total already contains cache tokens, pricing the whole count as input
    // over-charges; if the endpoint bills a cache-write token type we never saw, we under-charge.
    // An unknown cache model therefore never yields a provable direction.
    if (rawInput !== null) overApproximated = true
    missingFields.push('cacheRead', 'cacheWrite')
    notes.push('cache model unknown; input may contain cache tokens and is not asserted billable')
  }

  if (output === null) missingFields.push('output')

  const completeness =
    missingFields.length === 0
      ? COMPLETENESS.COMPLETE
      : input === null && output === null
        ? COMPLETENESS.UNAVAILABLE
        : COMPLETENESS.PARTIAL

  return {
    raw: rawUsage,
    input,
    output,
    cacheRead,
    cacheWrite: cacheWriteOut,
    cacheModel: resolvedCacheModel,
    completeness,
    missingFields,
    overApproximated,
    notes,
  }
}

/**
 * Structural validation for a versioned price table. Returns a list of problems; an empty list
 * means the table is usable. This does not verify that the rates are *correct* — that requires
 * `verifiedAt` and `source`, which are human review artefacts.
 */
export function validatePriceTable(table) {
  const problems = []
  if (!table || typeof table !== 'object') return ['price table is missing']
  if (typeof table.priceVersion !== 'string' || table.priceVersion.trim() === '') {
    problems.push('priceVersion is required')
  }
  if (table.unit !== PRICE_UNIT) problems.push(`unit must be ${PRICE_UNIT}`)
  if (!table.models || typeof table.models !== 'object') {
    problems.push('models map is required')
    return problems
  }
  for (const [key, entries] of Object.entries(table.models)) {
    for (const entry of Array.isArray(entries) ? entries : [entries]) {
      const where = `models.${key}`
      if (!entry || typeof entry !== 'object') {
        problems.push(`${where} must be an object`)
        continue
      }
      if (typeof entry.currency !== 'string' || entry.currency.trim() === '') {
        problems.push(`${where}.currency is required`)
      }
      if (typeof entry.effectiveAt !== 'string' || Number.isNaN(Date.parse(entry.effectiveAt))) {
        problems.push(`${where}.effectiveAt must be an ISO 8601 instant`)
      }
      if (entry.verifiedAt !== null && entry.verifiedAt !== undefined && Number.isNaN(Date.parse(entry.verifiedAt))) {
        problems.push(`${where}.verifiedAt must be null or an ISO 8601 instant`)
      }
      if (!entry.rates || typeof entry.rates !== 'object') {
        problems.push(`${where}.rates is required`)
        continue
      }
      for (const component of ['input', 'output', 'cacheRead', 'cacheWrite']) {
        const rate = entry.rates[component]
        if (rate === null || rate === undefined) continue
        if (!(Number.isFinite(rate) && rate >= 0)) {
          problems.push(`${where}.rates.${component} must be null or a non-negative number`)
        }
      }
    }
  }
  return problems
}

/**
 * Resolve the rate entry in force for a request. An entry younger than the request time is not
 * eligible, so a historical request can never be priced with a newer card.
 */
export function resolvePrice({ provider, modelId, at = null } = {}, table = null) {
  const priceVersion = table?.priceVersion ?? null
  if (!table || typeof table.models !== 'object') {
    return { matched: false, priceVersion, currency: null, rates: null, reason: 'no_price_table', effectiveAt: null }
  }
  const candidates = []
  for (const key of [`${provider}/${modelId}`, String(modelId)]) {
    const entries = table.models[key]
    if (!entries) continue
    for (const entry of Array.isArray(entries) ? entries : [entries]) candidates.push(entry)
  }
  if (candidates.length === 0) {
    return { matched: false, priceVersion, currency: null, rates: null, reason: 'no_price_entry', effectiveAt: null }
  }
  const atMs = at === null ? Number.POSITIVE_INFINITY : Date.parse(at)
  const eligible = candidates
    .filter((entry) => Date.parse(entry.effectiveAt) <= atMs)
    .sort((left, right) => Date.parse(left.effectiveAt) - Date.parse(right.effectiveAt))
  if (eligible.length === 0) {
    return { matched: false, priceVersion, currency: null, rates: null, reason: 'price_not_effective_yet', effectiveAt: null }
  }
  const entry = eligible[eligible.length - 1]
  const rates = {
    input: finiteNonNegative(entry.rates?.input),
    output: finiteNonNegative(entry.rates?.output),
    cacheRead: finiteNonNegative(entry.rates?.cacheRead),
    cacheWrite: finiteNonNegative(entry.rates?.cacheWrite),
  }
  const currency = typeof entry.currency === 'string' && entry.currency.trim() !== '' ? entry.currency : null
  return {
    matched: true,
    priceVersion,
    currency,
    rates,
    reason: 'matched',
    effectiveAt: entry.effectiveAt,
    verifiedAt: entry.verifiedAt ?? null,
  }
}

/**
 * Cost for a single request. `knownCost` is null when nothing can be priced. When only part of
 * the request can be priced the result carries an explicit, *provable* `bound`:
 *   'lower'         every skipped component could only have added cost, so the sum is a floor
 *   'indeterminate' at least one priced component may itself over-count (an unreported cache
 *                   split billed as input) while other components are missing, so neither
 *                   direction can be proven
 *   null            nothing was priced, or the cost is complete and needs no bound
 * A partial sum is never presented as a complete cost, and an unprovable direction is never
 * named a lower bound.
 */
export function computeCost(normalizedUsage, price) {
  if (!price?.matched) {
    return {
      knownCost: null,
      costComplete: false,
      bound: null,
      currency: price?.currency ?? null,
      priceVersion: price?.priceVersion ?? null,
      missing: [{ component: 'all', reason: price?.reason ?? 'no_price' }],
    }
  }
  const missing = []
  let total = 0
  let priced = 0
  for (const component of ['input', 'output', 'cacheRead', 'cacheWrite']) {
    const tokens = normalizedUsage?.[component] ?? null
    if (tokens === null) {
      missing.push({ component, reason: 'unknown_tokens' })
      continue
    }
    if (tokens === 0) continue
    const rate = price.rates?.[component]
    if (rate === null || rate === undefined) {
      missing.push({ component, reason: 'unknown_rate' })
      continue
    }
    total += (tokens * rate) / 1e6
    priced += 1
  }
  const costComplete = missing.length === 0
  const overApproximated = normalizedUsage?.overApproximated === true
  let bound = null
  if (!costComplete) {
    if (priced === 0) bound = null
    else if (overApproximated) bound = COST_BOUND.INDETERMINATE
    else bound = COST_BOUND.LOWER
  }
  return {
    knownCost: priced === 0 && !costComplete ? null : roundCost(total),
    costComplete,
    bound,
    currency: price.currency,
    priceVersion: price.priceVersion,
    missing,
  }
}

/**
 * Three configuration layers, kept apart on purpose:
 *   raw      — what the user/UI supplied
 *   resolved — what capability resolution produced (see model-profile.resolveModelProfile)
 *   sent     — the parameters actually serialized onto the wire
 * A field present in `sent` that is not in `allowedSendFields` is reported, never dropped
 * quietly: silently discarding a parameter and then reporting it as applied is a false claim.
 */
export function buildConfigLayers({ raw = null, resolved = null, sent = null, allowedSendFields = null } = {}) {
  const rejected = []
  if (sent && allowedSendFields) {
    const allowed = new Set(allowedSendFields)
    for (const key of Object.keys(sent)) {
      if (!allowed.has(key)) rejected.push({ field: key, reason: 'unsupported_field' })
    }
  }
  return { raw, resolved, sent, rejected }
}

export function assertNoUnsupportedConfigFields(layers) {
  if (layers?.rejected?.length) {
    const fields = layers.rejected.map((entry) => entry.field).join(', ')
    const error = new Error(`usage.unsupported_config_field: ${fields}`)
    error.code = 'usage.unsupported_config_field'
    error.fields = layers.rejected.map((entry) => entry.field)
    throw error
  }
}

/**
 * Build one immutable accounting record. Required identity: the request it bills. Everything
 * that would let a reader audit or aggregate the record is optional but recorded as null so
 * that "absent" is visible rather than inferred.
 */
export function createUsageRecord({
  eventId,
  requestId,
  runId,
  provider,
  modelId,
  taskId = null,
  attemptId = null,
  parentRequestId = null,
  stage = 'agent',
  outcome = 'unknown',
  api = null,
  cacheModel = null,
  rawUsage = null,
  priceTable = null,
  at = null,
  timings = {},
  config = null,
  recordedAt = null,
} = {}) {
  for (const [field, value] of Object.entries({ eventId, requestId, runId, provider, modelId })) {
    if (typeof value !== 'string' || value.trim() === '') {
      const error = new Error(`usage.missing_identity: ${field}`)
      error.code = 'usage.missing_identity'
      throw error
    }
  }
  if (!STAGES.includes(stage)) {
    const error = new Error(`usage.unknown_stage: ${stage}`)
    error.code = 'usage.unknown_stage'
    throw error
  }
  if (!OUTCOMES.includes(outcome)) {
    const error = new Error(`usage.unknown_outcome: ${outcome}`)
    error.code = 'usage.unknown_outcome'
    throw error
  }

  const usage = normalizeUsage({ rawUsage, api, cacheModel })
  const price = resolvePrice({ provider, modelId, at }, priceTable)
  const cost = computeCost(usage, price)
  const normalizedTimings = {}
  for (const field of Object.keys(TIMING_FIELDS)) {
    normalizedTimings[field] = finiteNonNegative(timings?.[field])
  }

  return Object.freeze({
    schemaVersion: USAGE_SCHEMA_VERSION,
    eventId,
    requestId,
    runId,
    taskId,
    attemptId,
    parentRequestId,
    stage,
    outcome,
    provider,
    modelId,
    api: api ?? null,
    usage,
    cost,
    timings: normalizedTimings,
    config,
    priceVersion: price.priceVersion,
    currency: price.currency,
    effectiveAt: price.effectiveAt ?? null,
    recordedAt,
  })
}

/**
 * Identity scope of the accounting unit, stated explicitly because "the request ID is unique"
 * is only true within a stated boundary: a `requestId` is unique *within its `runId`*. The
 * ledger therefore keys on the whole `(runId, requestId, attemptId)` tuple.
 */
export const REQUEST_IDENTITY_SCOPE =
  'requestId is unique within its runId; the accounting unit is the (runId, requestId, attemptId) tuple'

/** Key component used when the caller did not distinguish attempts. A label, not a claim. */
export const DEFAULT_ATTEMPT_ID = 'attempt-0'

/**
 * Unambiguous unit key. A delimiter-joined string such as `requestId::attemptId` collides:
 * ('a::b', 'c') and ('a', 'b::c') would both render as `a::b::c` and two real requests would be
 * billed as one. A JSON tuple keeps every component separable and non-forgeable.
 */
export function requestUnitKey(record) {
  return JSON.stringify([
    String(record?.runId ?? ''),
    String(record?.requestId ?? ''),
    String(record?.attemptId ?? DEFAULT_ATTEMPT_ID),
  ])
}

/**
 * Deduplication ledger over the tuple above, which satisfies both halves of the requirement:
 *   - a stream reconnect that re-delivers the final event for the same attempt is dropped;
 *   - a retry carries a new attemptId, so it is a distinct unit and is never deduplicated away.
 */
export function createRequestLedger() {
  const units = new Map()
  const eventIds = new Set()
  return {
    record(record) {
      const unitKey = requestUnitKey(record)
      if (eventIds.has(record.eventId)) return { accepted: false, reason: 'duplicate_event' }
      if (units.has(unitKey)) return { accepted: false, reason: 'duplicate_request_attempt' }
      eventIds.add(record.eventId)
      units.set(unitKey, record)
      return { accepted: true, unitKey }
    },
    records() {
      return [...units.values()]
    },
    size() {
      return units.size
    },
  }
}

/** The four token components of a usage record. Order is stable for reporting. */
export const TOKEN_COMPONENTS = Object.freeze(['input', 'output', 'cacheRead', 'cacheWrite'])

/**
 * Per-component token totals that never turn "unknown" into 0.
 *
 * Each component carries `value` (the total, or null when any contributor is unknown),
 * `knownSum` (what we do know), `knownCount`, `missingCount` and `complete`. A consumer can thus
 * print a partial floor *and* say how much is missing instead of guessing; no component is ever
 * silently reported as 0 because its contributors were unknown.
 */
export function summarizeTokens(records, components = TOKEN_COMPONENTS) {
  const rows = Array.isArray(records) ? records : []
  const summary = {}
  for (const component of components) {
    let knownSum = 0
    let knownCount = 0
    let missingCount = 0
    for (const record of rows) {
      const tokens = record?.usage?.[component] ?? null
      if (typeof tokens !== 'number' || !Number.isFinite(tokens)) {
        missingCount += 1
      } else {
        knownSum += tokens
        knownCount += 1
      }
    }
    const complete = rows.length > 0 && missingCount === 0
    summary[component] = Object.freeze({
      value: complete ? knownSum : null,
      knownSum,
      knownCount,
      missingCount,
      complete,
      reason: rows.length === 0 ? 'no_records' : complete ? 'complete' : 'unknown_contributors',
    })
  }
  return summary
}

function projectTokens(summary) {
  const projection = {}
  for (const [component, entry] of Object.entries(summary)) projection[component] = entry.value
  return projection
}

function tokensAreComplete(summary) {
  return TOKEN_COMPONENTS.every((component) => summary[component]?.complete === true)
}

/**
 * Aggregate stage durations and the observed wall clock. Stage durations are summed only as an
 * explicitly labelled diagnostic: for work that ran in parallel the sum exceeds the real
 * elapsed time, so it is never presented as the total wall clock. When start/end marks are
 * missing the wall clock is null, not an estimate.
 */
export function summarizeTimings(records, { startedAtMs = null, endedAtMs = null } = {}) {
  const sums = {}
  const gaps = {}
  for (const field of Object.keys(TIMING_FIELDS)) {
    let total = 0
    let missing = 0
    for (const record of records) {
      const value = record.timings?.[field] ?? null
      if (value === null) missing += 1
      else total += value
    }
    sums[field] = total
    gaps[field] = missing
  }
  const wallClockMs =
    Number.isFinite(startedAtMs) && Number.isFinite(endedAtMs) && endedAtMs >= startedAtMs
      ? endedAtMs - startedAtMs
      : null
  return {
    wallClockMs,
    sumOfStageDurationsMs: sums.modelRequestMs + sums.toolMs + sums.approvalWaitMs,
    sumOfStageDurationsIsNotWallClock: true,
    approvalWaitMs: sums.approvalWaitMs,
    ttftMsTotal: sums.ttftMs,
    modelRequestMsTotal: sums.modelRequestMs,
    toolMsTotal: sums.toolMs,
    recordsMissingTimingMarks: gaps,
    definitions: TIMING_FIELDS,
  }
}

/**
 * Cross-record totals. Currencies are never mixed: without an explicit `currency` the result is
 * grouped per currency and the combined total stays null.
 */
export function aggregateUsage(records, { currency = null, startedAtMs = null, endedAtMs = null } = {}) {
  const all = Array.isArray(records) ? records : []
  const codes = currency === null ? [...new Set(all.map((record) => record.currency ?? 'unknown'))] : [currency]
  const selected = currency === null ? all : all.filter((record) => (record.currency ?? 'unknown') === currency)
  const mixedCurrencies = currency === null && codes.length > 1

  const byCurrency = {}
  for (const code of codes) {
    const bucket = selected.filter((record) => (record.currency ?? 'unknown') === code)
    if (bucket.length === 0) continue
    const tokenSummary = summarizeTokens(bucket)
    const priced = bucket.filter((record) => record.cost?.knownCost !== null && record.cost?.knownCost !== undefined)
    const knownCost = priced.length > 0 ? roundCost(priced.reduce((sum, r) => sum + r.cost.knownCost, 0)) : null
    const costComplete = bucket.length > 0 && bucket.every((record) => record.cost?.costComplete === true)
    const bounds = [...new Set(bucket.map((record) => record.cost?.bound).filter((value) => value !== null && value !== undefined))]
    let bound = null
    if (!costComplete && bounds.length > 0) bound = bounds.length === 1 ? bounds[0] : COST_BOUND.INDETERMINATE
    byCurrency[code] = {
      requestCount: bucket.length,
      attemptCount: new Set(bucket.map(requestUnitKey)).size,
      failureCount: bucket.filter((r) => r.outcome !== 'success').length,
      stageCounts: bucket.reduce((counts, r) => ({ ...counts, [r.stage]: (counts[r.stage] ?? 0) + 1 }), {}),
      tokens: projectTokens(tokenSummary),
      tokensComplete: tokensAreComplete(tokenSummary),
      tokenCompleteness: tokenSummary,
      cost: {
        knownCost,
        costComplete,
        bound,
        priceVersions: [...new Set(bucket.map((r) => r.priceVersion).filter((v) => v !== null))],
        reason: knownCost === null ? 'no_priced_requests' : costComplete ? 'complete' : 'partial',
      },
      timings: summarizeTimings(bucket, { startedAtMs, endedAtMs }),
    }
  }

  const totalTokens = summarizeTokens(selected)
  return {
    schemaVersion: USAGE_SCHEMA_VERSION,
    currency,
    mixedCurrencies,
    byCurrency,
    requestCount: selected.length,
    tokens: projectTokens(totalTokens),
    tokensComplete: tokensAreComplete(totalTokens),
    tokenCompleteness: totalTokens,
    timings: summarizeTimings(selected, { startedAtMs, endedAtMs }),
  }
}

/** Documentation artefact: the record shape E and F may rely on. Field names are frozen here. */
export const USAGE_RECORD_FIELDS = Object.freeze({
  identity: ['schemaVersion', 'eventId', 'requestId', 'runId', 'taskId', 'attemptId', 'parentRequestId', 'stage'],
  identityScope: REQUEST_IDENTITY_SCOPE,
  routing: ['provider', 'modelId', 'api'],
  usage: ['usage.raw', 'usage.input', 'usage.output', 'usage.cacheRead', 'usage.cacheWrite', 'usage.cacheModel', 'usage.completeness', 'usage.missingFields'],
  cost: ['priceVersion', 'currency', 'effectiveAt', 'cost.knownCost', 'cost.costComplete', 'cost.bound', 'cost.missing'],
  aggregate: ['tokens', 'tokensComplete', 'tokenCompleteness.<component>.{value,knownSum,knownCount,missingCount,complete}'],
  timing: Object.keys(TIMING_FIELDS),
  config: ['config.raw', 'config.resolved', 'config.sent', 'config.rejected'],
  outcome: ['outcome'],
})
