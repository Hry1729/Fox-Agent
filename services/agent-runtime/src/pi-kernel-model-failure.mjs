// Observe the public per-request fetch hook. Never infer retry permission from
// arbitrary error strings, and never copy provider bodies/headers into the wire.
export function observeKernelModelTransport(session, fetchImpl = globalThis.fetch) {
  let calls = 0
  let rejection
  const stream = session.agent.streamFunction.bind(session.agent)
  session.agent.streamFunction = (model, context, options) => stream(model, context, {
    ...options, maxRetries: 0,
    fetch: async (...args) => {
      calls += 1
      const response = await fetchImpl(...args)
      if ([429, 500, 502, 503, 504, 529].includes(response.status)) {
        const raw = response.headers.get('retry-after')
        const rawMs = response.headers.get('retry-after-ms')
        let delay
        if (rawMs !== null && /^\d+(?:\.\d+)?$/.test(rawMs)) delay = Math.ceil(Number(rawMs))
        else if (raw !== null && /^\d+(?:\.\d+)?$/.test(raw)) delay = Math.ceil(Number(raw) * 1000)
        else if (raw !== null && Number.isFinite(Date.parse(raw))) delay = Math.max(0, Date.parse(raw) - Date.now())
        // An unrepresentable server delay must not become an immediate retry.
        if ((raw !== null || rawMs !== null) && (!Number.isSafeInteger(delay) || delay > 86_400_000)) {
          rejection = null
        } else rejection = { category: 'provider_unavailable', httpStatus: response.status, retryAfterMs: delay ?? null }
      } else rejection = null
      return response
    },
  })
  return request => {
    const final = session.agent.state.messages.at(-1)
    const frame = request.payload?.initialModel ?? request.payload?.batchResume
    if (!frame) return null
    const noOutput = final?.role === 'assistant' && !(final.content ?? []).some(block =>
      block?.type === 'toolCall' || block?.text?.length || block?.thinking?.length)
    const evidence = calls === 1 && rejection && noOutput && final.stopReason === 'error'
      ? rejection
      : final?.role === 'assistant' && final.stopReason === 'length'
        && !(final.content ?? []).some(block => block?.type === 'toolCall')
        ? { category: 'incomplete_response', httpStatus: null, retryAfterMs: null } : null
    if (!evidence) return null
    return { schemaVersion: 1, runId: request.runId, turnId: frame.input?.turnId ?? frame.turnId,
      checkpointSeq: frame.checkpointSeq, ...evidence }
  }
}
