// Disposable provider observations. Only the fixed summary returned by snapshot
// can leave this process; SSE frames and argument bytes are never diagnostics.
const reasons = new Set(['stop', 'length', 'tool_calls', 'function_call', 'content_filter'])
const stops = new Set(['stop', 'length', 'toolUse', 'error', 'aborted'])
const categories = new Set(['none', 'length', 'invalid_tool_arguments', 'truncated_tool_proposal',
  'protocol_invalid', 'stream_interrupted', 'worker_exception', 'cancelled', 'incomplete_response', 'argument_validation_unavailable'])
const stages = new Set(['provider_stream', 'response_projection', 'tool_proposal', 'worker_result'])
const cap = 1_048_576
const count = value => Number.isSafeInteger(value) && value >= 0 && value <= 67_108_864 ? value : null
const boundedId = value => typeof value === 'string' && /^[a-zA-Z0-9:_-]{1,512}$/.test(value) ? value : null
const tagged = new WeakMap()

export function responseAdmissionError(category, stage = 'response_projection') {
  const error = new Error('Kernel response could not be admitted')
  tagged.set(error, { category: categories.has(category) ? category : 'worker_exception',
    stage: stages.has(stage) ? stage : 'worker_result' })
  return error
}
export const responseAdmissionTag = error => tagged.get(error) ?? null

export function createResponseDiagnosticObserver(api, fetchImpl = globalThis.fetch) {
  let state
  const reset = () => { state = { providerFinishReason: 'unknown', adapterStopReason: 'unknown',
    category: 'none', stage: 'worker_result', outputTokens: null, maxOutputTokens: null,
    usageRequestId: null, tools: new Set(), byIndex: new Map(), byId: new Map(), argumentBytes: 0,
    sawDone: false, observationComplete: true, supported: false } }
  reset()
  const note = message => {
    state.adapterStopReason = stops.has(message?.stopReason) ? message.stopReason : 'unknown'
  }
  const noteUsage = record => {
    state.usageRequestId = boundedId(record?.requestId)
    const output = count(record?.usage?.output)
    if (output !== null) state.outputTokens = output
    const sent = record?.config?.sent
    state.maxOutputTokens = count(sent?.max_tokens ?? sent?.max_completion_tokens ?? sent?.max_output_tokens)
  }
  function data(raw) {
    if (state.sawDone) return
    if (raw === '[DONE]') { state.sawDone = true; return }
    let frame
    try { frame = JSON.parse(raw) } catch { state.category = 'protocol_invalid'; state.stage = 'provider_stream'; return }
    // The observer knows this dialect only. Other adapters keep unknown facts.
    const choice = Array.isArray(frame?.choices) ? frame.choices[0] : null
    if (choice?.finish_reason != null) {
      state.providerFinishReason = reasons.has(choice.finish_reason) ? choice.finish_reason : 'unknown'
    }
    if (count(frame?.usage?.completion_tokens) !== null) state.outputTokens = frame.usage.completion_tokens
    if (!Array.isArray(choice?.delta?.tool_calls) && choice?.delta?.tool_calls != null) return
    for (const tool of choice?.delta?.tool_calls ?? []) {
      const index = typeof tool?.index === 'number' ? tool.index : undefined
      const id = typeof tool?.id === 'string' && tool.id.length <= 512 ? tool.id : null
      // Match the installed SDK: stream index first, then tool id. Some
      // compatible providers omit index and use id on every argument delta.
      let entry = index !== undefined ? state.byIndex.get(index) : null
      entry ??= id ? state.byId.get(id) : null
      if (!entry) {
        if (state.tools.size >= 64) { state.observationComplete = false; continue }
        entry = { raw: '', observed: false, index: undefined, id: null }
        state.tools.add(entry)
      }
      if (index !== undefined && entry.index === undefined) {
        if (state.byIndex.size >= 64) state.observationComplete = false
        else { entry.index = index; state.byIndex.set(index, entry) }
      }
      if (id && !entry.id) {
        if (state.byId.size >= 64) state.observationComplete = false
        else { entry.id = id; state.byId.set(id, entry) }
      }
      const fragment = tool?.function?.arguments
      if (typeof fragment === 'string') {
        state.argumentBytes += Buffer.byteLength(fragment, 'utf8')
        entry.observed = true
        if (state.argumentBytes > cap) { state.observationComplete = false; entry.raw = '' }
        else entry.raw += fragment
      }
    }
  }
  const assertProposal = message => {
    const calls = message?.content?.filter?.(block => block?.type === 'toolCall') ?? []
    if (!calls.length) return
    if (message.stopReason !== 'toolUse') throw responseAdmissionError('truncated_tool_proposal', 'tool_proposal')
    if (state.category !== 'none') throw responseAdmissionError(state.category, 'provider_stream')
    if (state.supported && !state.observationComplete) throw responseAdmissionError('argument_validation_unavailable', 'tool_proposal')
    if (state.supported && state.tools.size !== calls.length) throw responseAdmissionError('argument_validation_unavailable', 'tool_proposal')
    for (const entry of state.tools.values()) {
      if (!entry.observed) throw responseAdmissionError('invalid_tool_arguments', 'tool_proposal')
      let parsed
      try { parsed = JSON.parse(entry.raw) } catch { throw responseAdmissionError('invalid_tool_arguments', 'tool_proposal') }
      if (parsed === null || typeof parsed !== 'object' || Array.isArray(parsed)) {
        throw responseAdmissionError('invalid_tool_arguments', 'tool_proposal')
      }
    }
  }
  const fetch = async (...args) => {
    reset()
    try {
      const body = JSON.parse(args[1]?.body ?? 'null')
      state.maxOutputTokens = count(body?.max_tokens ?? body?.max_completion_tokens ?? body?.max_output_tokens)
    } catch {}
    const response = await fetchImpl(...args)
    // The installed SDK consumes SSE for this streaming request regardless of
    // a compatible provider's Content-Type. Follow that same consumption path.
    if (api !== 'openai-completions' || !response.ok || !response.body) return response
    state.supported = true
    const decoder = new TextDecoder()
    let pending = '', pendingBytes = 0, dataLines = [], eventBytes = 0, discarding = false, afterCR = false, lineOverflow = false
    const line = () => {
      if (pending === '' && !lineOverflow) {
        if (!discarding && dataLines.length) data(dataLines.join('\n'))
        dataLines = []; eventBytes = 0; discarding = false
      } else if (!discarding && (pending === 'data' || pending.startsWith('data:'))) {
        const value = pending === 'data' ? '' : pending.slice(5).replace(/^ /, '')
        eventBytes += Buffer.byteLength(value, 'utf8') + 1
        if (eventBytes > cap || dataLines.length >= 65_536) { discarding = true; state.observationComplete = false; dataLines = [] }
        else dataLines.push(value)
      }
      pending = ''; pendingBytes = 0; lineOverflow = false
    }
    const stream = new TransformStream({
      transform(chunk, controller) {
        // Observe the same pull-controlled stream; no tee or detached reader.
        if (state.sawDone) { controller.enqueue(chunk); return }
        const text = decoder.decode(chunk, { stream: true })
        for (const character of text) {
          if (state.sawDone) break
          if (character === '\n' && afterCR) { afterCR = false; continue }
          afterCR = character === '\r'
          if (character === '\r' || character === '\n') { line(); continue }
          if (!discarding) { pending += character; pendingBytes += Buffer.byteLength(character, 'utf8') }
          if (pendingBytes > cap) { pending = ''; lineOverflow = true; discarding = true; state.observationComplete = false; dataLines = [] }
        }
        controller.enqueue(chunk)
      },
      flush() {
        // SSE dispatches only at an empty line; EOF alone is not completion.
        pending = ''; dataLines = []
      },
    })
    return new Response(response.body.pipeThrough(stream), {
      status: response.status, statusText: response.statusText, headers: response.headers,
    })
  }
  const snapshot = (error, { turnId, checkpointSeq, cancelled = false } = {}) => {
    const tag = responseAdmissionTag(error)
    let category = tag?.category ?? (cancelled ? 'cancelled' : state.category !== 'none' ? state.category
      : state.providerFinishReason === 'length' ? 'length'
        : state.adapterStopReason === 'error' || state.adapterStopReason === 'aborted' ? 'stream_interrupted'
          : 'worker_exception')
    return { schemaVersion: 1, turnId: boundedId(turnId), checkpointSeq,
      source: 'worker_observation', stage: tag?.stage ?? state.stage, category,
      providerFinishReason: state.providerFinishReason, adapterStopReason: state.adapterStopReason,
      maxOutputTokens: state.maxOutputTokens, outputTokens: state.outputTokens,
      usageRequestId: state.usageRequestId, resultComplete: false }
  }
  return { fetch, note, noteUsage, assertProposal, snapshot,
    admissionFailure: () => state.category !== 'none' ? responseAdmissionError(state.category, state.stage) : null }
}
