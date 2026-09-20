// One accounting unit per actual HTTP attempt. This adapter observes the
// request boundary; it never persists prompts, authorization headers or URLs.
import { randomUUID } from 'node:crypto'
import prices from './model-prices.json' with { type: 'json' }
import { createUsageRecord, CACHE_MODEL, buildConfigLayers } from './usage-accounting.mjs'

const scalarFields = ['model', 'max_tokens', 'max_completion_tokens', 'max_output_tokens', 'temperature', 'top_p', 'reasoning_effort', 'stream']
export function safeSentConfig(body) {
  let parsed
  try { parsed = typeof body === 'string' ? JSON.parse(body) : null } catch { return null }
  if (!parsed || typeof parsed !== 'object') return null
  const out = {}
  for (const key of scalarFields) if (['string', 'number', 'boolean'].includes(typeof parsed[key])) out[key] = parsed[key]
  if (typeof parsed.reasoning?.effort === 'string') out.reasoning = { effort: parsed.reasoning.effort }
  if (typeof parsed.thinking?.type === 'string') out.thinking = { type: parsed.thinking.type,
    ...(Number.isSafeInteger(parsed.thinking.budget_tokens) ? { budget_tokens: parsed.thinking.budget_tokens } : {}) }
  return out
}
function safeRaw(raw = {}) {
  const safe = Object.fromEntries(['apiType', 'modelId', 'modelProfile', 'reasoning', 'thinkingLevel', 'contextWindow', 'maxOutputTokens', 'supportsImageInput']
    .filter(k => ['string', 'number', 'boolean'].includes(typeof raw[k])).map(k => [k, raw[k]]))
  if (raw.modelProfile && typeof raw.modelProfile === 'object') safe.modelProfile = Object.fromEntries(
    ['contextWindow', 'maxOutputTokens', 'reasoning', 'thinkingLevel', 'supportsImageInput', 'plannerMaxOutputTokens', 'plannerThinkingLevel']
      .filter(k => ['string', 'number', 'boolean'].includes(typeof raw.modelProfile[k])).map(k => [k, raw.modelProfile[k]]))
  return safe
}
function piUsage(message) {
  const u = message?.usage
  // Pi counters are already exclusive. Applying provider normalization again
  // would subtract cached input twice. All-zero synthetic error defaults do not
  // prove the provider reported usage.
  if (!u || !['input', 'output', 'cacheRead', 'cacheWrite'].some(k => Number(u[k]) > 0)) return null
  // The SDK also initializes absent individual counters to zero. Without a
  // raw-provider presence bit, preserve that uncertainty instead of asserting
  // an observed zero. Nonzero exclusive counters are unambiguous.
  const observed = value => Number.isSafeInteger(value) && value > 0 ? value : null
  return { input_tokens: observed(u.input), output_tokens: observed(u.output),
    cache_read_input_tokens: observed(u.cacheRead), cache_creation_input_tokens: observed(u.cacheWrite) }
}

export function observeModelUsage(session, context = {}) {
  const original = session.agent.streamFunction.bind(session.agent)
  session.agent.streamFunction = async (model, prompt, options = {}) => {
    let started = performance.now()
    const requestId = randomUUID()
    let sent = null, ttft = null, recorded = false, fetchCount = 0
    const stage = typeof context.stage === 'function' ? context.stage() : context.stage ?? 'agent'
    const cap = Number.isFinite(model.maxTokens) && model.maxTokens > 0 ? model.maxTokens : 8192
    const maxTokens = Number.isFinite(options.maxTokens) && options.maxTokens > 0 ? Math.min(options.maxTokens, cap) : cap
    const config = buildConfigLayers({ raw: safeRaw(context.raw), resolved: {
      model: model.id, api: model.api, maxOutputTokens: model.maxTokens,
      contextWindow: model.contextWindow, reasoning: model.reasoning,
      thinkingLevel: options.reasoning ?? options.thinkingLevel ?? null,
    } })
    const finish = (message, error) => {
      if (recorded) return
      recorded = true
      if (!context.runId || typeof context.onRecord !== 'function') return
      const outcome = options.signal?.aborted || message?.stopReason === 'aborted' ? 'cancelled'
        : error || message?.stopReason === 'error' ? 'failure' : message ? 'success' : 'unknown'
      const attemptId = `attempt-${Math.max(0, fetchCount - 1)}`
      const record = createUsageRecord({ eventId: `usage:${requestId}:${attemptId}`, requestId, attemptId,
        runId: context.runId, taskId: context.taskId ?? null, parentRequestId: context.parentRequestId ?? null,
        provider: model.provider, modelId: model.id, api: model.api, stage, outcome,
        rawUsage: piUsage(message), cacheModel: CACHE_MODEL.EXCLUSIVE, priceTable: prices,
        at: new Date().toISOString(), recordedAt: new Date().toISOString(),
        config: { ...config, sent, evidenceLevel: sent ? 'request_captured' : 'unavailable' },
        timings: { modelRequestMs: performance.now() - started, ttftMs: ttft } })
      context.onRecord({ ...record, transportAttempts: fetchCount > 0 ? 1 : 0 })
    }
    const fetchImpl = options.fetch ?? globalThis.fetch
    let stream
    try {
      stream = await original(model, prompt, { ...options, maxTokens,
        fetch: async (url, init) => {
          // Let the provider retain its configured retry policy. A subsequent
          // transport attempt cannot reuse the previous attempt's identity.
          if (fetchCount > 0) finish(null, true)
          fetchCount += 1
          started = performance.now()
          recorded = false
          ttft = null
          sent = safeSentConfig(init?.body)
          try {
            const response = await fetchImpl(url, init)
            if (!response.ok) finish(null, true)
            return response
          } catch (error) { finish(null, error); throw error }
        } })
    } catch (error) { finish(null, error); throw error }
    return new Proxy(stream, { get(target, property) {
      if (property === Symbol.asyncIterator) return async function* () {
        try {
          for await (const event of target) {
            if (ttft === null && /(?:text|thinking)_delta/.test(event.type)) ttft = performance.now() - started
            if (event.type === 'done') finish(event.message)
            else if (event.type === 'error') finish(event.error, true)
            yield event
          }
        } catch (error) { finish(null, error); throw error }
        finally { finish(null) }
      }
      if (property === 'result') return async () => {
        try { const value = await target.result(); finish(value); return value }
        catch (error) { finish(null, error); throw error }
      }
      const value = Reflect.get(target, property)
      return typeof value === 'function' ? value.bind(target) : value
    } })
  }
}

export function observeNativeUsage(operation, context) {
  return async args => {
    const started = performance.now(), requestId = randomUUID()
    let message, failure
    try { message = await operation(args); return message }
    catch (error) { failure = error; throw error }
    finally {
      const config = args.config.modelService
      context.onRecord(createUsageRecord({ eventId: `usage:${requestId}`, requestId, attemptId: 'attempt-0',
        runId: context.runId, provider: args.config.engineId ?? config.apiType, modelId: config.modelId,
        api: config.apiType, stage: context.stage(), outcome: args.signal?.aborted ? 'cancelled' : failure ? 'failure' : 'success',
        rawUsage: piUsage(message), cacheModel: CACHE_MODEL.EXCLUSIVE, priceTable: prices,
        config: { raw: safeRaw(config), resolved: null, sent: null, evidenceLevel: 'unavailable' },
        timings: { modelRequestMs: performance.now() - started }, recordedAt: new Date().toISOString(), at: new Date().toISOString() }))
    }
  }
}
