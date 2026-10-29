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

function observedTokens(message, fields) {
  const values = fields.map(field => message?.usage?.[field]).filter(value => Number.isSafeInteger(value) && value > 0)
  return values.length ? values.reduce((total, value) => total + value, 0) : null
}

function withSignal(operation, signal) {
  if (!signal) return operation
  if (signal.aborted) return Promise.reject(signal.reason)
  return new Promise((resolve, reject) => {
    const abort = () => reject(signal.reason)
    signal.addEventListener('abort', abort, { once: true })
    Promise.resolve(operation).then(resolve, reject).finally(() => signal.removeEventListener('abort', abort))
  })
}

function managedStream(stream, onEvent, finish, signal) {
  const events = []
  const listeners = new Set()
  let ended = false, failure, consumer = null
  const wake = () => { for (const listener of listeners) listener(); listeners.clear() }
  const completion = (async () => {
    const iterator = stream[Symbol.asyncIterator]()
    try {
      while (true) {
        const next = await withSignal(iterator.next(), signal)
        if (next.done) break
        const event = next.value
        onEvent(event)
        if (event.type === 'done') await finish(event.message)
        else if (event.type === 'error') await finish(event.error, true)
        if (consumer !== 'result') events.push(event)
        wake()
      }
      const result = await withSignal(stream.result(), signal)
      await finish(result)
      return result
    } catch (error) {
      failure = error
      await finish(null, error)
      throw error
    } finally {
      if (signal?.aborted) void Promise.resolve(iterator.return?.()).catch(() => {})
      ended = true
      wake()
    }
  })()
  completion.catch(() => {})
  return new Proxy(stream, { get(target, property) {
    if (property === 'result') return () => {
      if (consumer === null) { consumer = 'result'; events.length = 0 }
      return completion
    }
    if (property === Symbol.asyncIterator) return async function* () {
      consumer = 'iterator'
      let index = 0
      while (true) {
        while (index < events.length) yield events[index++]
        if (index) { events.splice(0, index); index = 0 }
        if (ended) {
          if (failure) throw failure
          return
        }
        await new Promise(resolve => listeners.add(resolve))
      }
    }
    const value = Reflect.get(target, property)
    return typeof value === 'function' ? value.bind(target) : value
  } })
}

export function observeModelUsage(session, context = {}) {
  const original = session.agent.streamFunction.bind(session.agent)
  session.agent.streamFunction = async (model, prompt, options = {}) => {
    let started = performance.now()
    const logicalRequestId = randomUUID()
    let requestId = logicalRequestId, attemptIndex = 0
    let sent = null, ttft = null, recorded = false, fetchCount = 0
    const stage = typeof context.stage === 'function' ? context.stage() : context.stage ?? 'agent'
    const cap = Number.isFinite(model.maxTokens) && model.maxTokens > 0 ? model.maxTokens : 8192
    let maxTokens = Number.isFinite(options.maxTokens) && options.maxTokens > 0 ? Math.min(options.maxTokens, cap) : cap
    const managed = typeof context.beforeAttempt === 'function' && typeof context.afterAttempt === 'function'
    const controller = managed ? new AbortController() : null
    const signal = managed ? AbortSignal.any([controller.signal, ...[options.signal, context.signal].filter(Boolean)]) : options.signal
    let admitted = false, completing = Promise.resolve(), timeout, firstResponse, idle, admission, timeoutError
    const clearTimers = () => { clearTimeout(timeout); clearTimeout(firstResponse); clearTimeout(idle) }
    const abortForTimeout = (code, message) => {
      const error = new Error(message)
      error.code = code
      timeoutError = error
      controller.abort(error)
      context.onFailure?.(error)
    }
    const progress = () => {
      if (!managed || !admitted || recorded) return
      clearTimeout(firstResponse)
      clearTimeout(idle)
      idle = setTimeout(() => abortForTimeout('runtime.model_idle_timeout', 'The model request exceeded its Host idle budget.'), admission.idleMs)
    }
    const begin = async () => {
      signal?.throwIfAborted()
      admission = await context.beforeAttempt({ requestId, attemptId: `attempt-${attemptIndex}`, stage, maxOutputTokens: maxTokens, signal })
      admitted = true
      signal?.throwIfAborted()
      for (const field of ['timeoutMs', 'firstResponseMs', 'idleMs', 'maxOutputTokens']) {
        if (!Number.isSafeInteger(admission?.[field]) || admission[field] <= 0) throw new Error(`Invalid Host model admission ${field}.`)
      }
      maxTokens = Math.min(maxTokens, admission.maxOutputTokens)
      started = performance.now()
      timeout = setTimeout(() => abortForTimeout('runtime.model_request_timeout', 'The model request exceeded its Host request budget.'), admission.timeoutMs)
      firstResponse = setTimeout(() => abortForTimeout('runtime.model_first_response_timeout', 'The model request exceeded its Host first-response budget.'), admission.firstResponseMs)
    }
    const config = buildConfigLayers({ raw: safeRaw(context.raw), resolved: {
      model: model.id, api: model.api, maxOutputTokens: model.maxTokens,
      contextWindow: model.contextWindow, reasoning: model.reasoning,
      thinkingLevel: options.reasoning ?? options.thinkingLevel ?? null,
    } })
    const finish = (message, error) => {
      if (recorded) return completing
      recorded = true
      clearTimers()
      const outcome = timeoutError ? 'failure' : signal?.aborted || message?.stopReason === 'aborted' ? 'cancelled'
        : error || message?.stopReason === 'error' ? 'failure' : message ? 'success' : 'unknown'
      const attemptId = `attempt-${managed ? attemptIndex : Math.max(0, fetchCount - 1)}`
      if (context.runId && typeof context.onRecord === 'function') {
        const record = createUsageRecord({ eventId: `usage:${requestId}:${attemptId}`, requestId, attemptId,
        runId: context.runId, taskId: context.taskId ?? null, parentRequestId: context.parentRequestId ?? null,
        provider: model.provider, modelId: model.id, api: model.api, stage, outcome,
        rawUsage: piUsage(message), cacheModel: CACHE_MODEL.EXCLUSIVE, priceTable: prices,
        at: new Date().toISOString(), recordedAt: new Date().toISOString(),
        config: { ...config, sent, evidenceLevel: sent ? 'request_captured' : 'unavailable' },
        timings: { modelRequestMs: performance.now() - started, ttftMs: ttft } })
        context.onRecord({ ...record, transportAttempts: fetchCount > 0 ? 1 : 0 })
      }
      if (managed && admitted) completing = Promise.resolve().then(() => context.afterAttempt({
        requestId, stage, inputTokens: observedTokens(message, ['input', 'cacheRead', 'cacheWrite']),
        outputTokens: observedTokens(message, ['output']),
        status: outcome === 'success' ? 'completed' : outcome === 'cancelled' ? 'cancelled' : 'failed',
      }))
      return completing
    }
    const fetchImpl = options.fetch ?? globalThis.fetch
    let stream
    try {
      if (managed) await begin()
      const operation = original(model, prompt, { ...options, maxTokens, ...(managed ? { signal } : {}),
        fetch: async (url, init) => {
          // Let the provider retain its configured retry policy. A subsequent
          // transport attempt cannot reuse the previous attempt's identity.
          if (fetchCount > 0) {
            await finish(null, true)
            if (managed) {
              attemptIndex += 1
              requestId = randomUUID()
              admitted = false
              recorded = false
              await begin()
            }
          }
          fetchCount += 1
          started = performance.now()
          recorded = false
          ttft = null
          sent = safeSentConfig(init?.body)
          try {
            if (managed && typeof init?.body === 'string') {
              const body = JSON.parse(init.body)
              let changed = false
              for (const field of ['max_tokens', 'max_completion_tokens', 'max_output_tokens']) {
                if (Number.isFinite(body[field]) && body[field] > maxTokens) { body[field] = maxTokens; changed = true }
              }
              if (changed) init = { ...init, body: JSON.stringify(body) }
              sent = safeSentConfig(init.body)
            }
            if (managed) init = { ...init, signal }
            const response = await fetchImpl(url, init)
            if (!response.ok) await finish(null, true)
            return response
          } catch (error) { await finish(null, error); throw error }
        } })
      stream = managed ? await withSignal(operation, signal) : await operation
    } catch (error) { await finish(null, error); throw error }
    if (managed) return managedStream(stream, event => {
      if (timeoutError) throw timeoutError
      if (/(?:text|thinking|toolcall|tool_call)_(?:start|delta|end)/.test(event.type)) progress()
      if (ttft === null && /(?:text|thinking)_delta/.test(event.type)) ttft = performance.now() - started
    }, finish, signal)
    return new Proxy(stream, { get(target, property) {
      if (property === Symbol.asyncIterator) return async function* () {
        try {
          for await (const event of target) {
            if (ttft === null && /(?:text|thinking)_delta/.test(event.type)) ttft = performance.now() - started
            if (event.type === 'done') await finish(event.message)
            else if (event.type === 'error') await finish(event.error, true)
            yield event
          }
        } catch (error) { await finish(null, error); throw error }
        finally { await finish(null) }
      }
      if (property === 'result') return async () => {
        try { const value = await target.result(); await finish(value); return value }
        catch (error) { await finish(null, error); throw error }
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
