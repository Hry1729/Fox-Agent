import { incompleteResponseReason, modelFailureDiagnostic } from './kernel-completion.mjs'
// Observe the public per-request fetch hook. Never infer retry permission from
// arbitrary error strings, and never copy provider bodies/headers into the wire.
// `completionRequired` is the frozen prompt's own contract, passed in by the worker
// that read it: without it this exit could not tell "answered but never closed the
// answer" from "answered", and the Host would only ever see one unnamed branch.
export function observeKernelModelTransport(session, fetchImpl = globalThis.fetch, { completionRequired = false } = {}) {
  let calls = 0
  let rejection
  let thrown = null
  const stream = session.agent.streamFunction.bind(session.agent)
  session.agent.streamFunction = (model, context, options) => stream(model, context, {
    ...options, maxRetries: 0,
    fetch: async (...args) => {
      calls += 1
      let response
      try {
        response = await fetchImpl(...args)
      } catch (error) {
        // A transport error that never produced a response (bad URL, refused
        // connection, aborted socket) has no HTTP status, so it cannot be
        // classified from the response alone. Keep the message for diagnosis;
        // it is reported only through the opt-in debug hook.
        thrown = error
        throw error
      }
      thrown = null
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
  const noOutput = final => final?.role === 'assistant' && !(final.content ?? []).some(block =>
    block?.type === 'toolCall' || block?.text?.length || block?.thinking?.length)
  return {
    /** Last provider transport rejection observed on this session. */
    lastRejection: () => rejection,
    fetchCallCount: () => calls,
    /** Message of the last fetch that threw before any response, if any. */
    lastFetchError: () => thrown,
    /** Settled failure evidence for a single-round request frame. */
    failureFor: request => {
      const final = session.agent.state.messages.at(-1)
      const frame = request.payload?.initialModel ?? request.payload?.batchResume
      if (!frame) return null
      const evidence = calls >= 1 && rejection && noOutput(final) && final.stopReason === 'error'
        ? rejection
        : final?.role === 'assistant' && final.stopReason === 'length'
          && !(final.content ?? []).some(block => block?.type === 'toolCall')
          ? { category: 'incomplete_response', httpStatus: null, retryAfterMs: null }
          // A round that ended without a successful stop is a transport failure
          // even when the provider never produced a classified rejection (for
          // example the request never left the worker). `null` here would be
          // rejected by the Host as malformed evidence, turning a real failure
          // into an unreadable one; the category itself never grants a retry —
          // the Host still owns retry admission from durable tool state.
          : final?.role === 'assistant' && final.stopReason !== 'stop'
            ? { category: 'model_transport_failure', httpStatus: null, retryAfterMs: null }
            : null
      if (!evidence) return null
      // Name the branch behind the category and carry the round facts the worker
      // could actually see; anything it could not see stays absent.
      const settled = final?.role === 'assistant' ? final : null
      const diagnostic = evidence.category === 'incomplete_response'
        ? modelFailureDiagnostic({ message: settled,
            reason: incompleteResponseReason(settled, completionRequired),
            completionRequired,
            dispatchId: typeof request?.id === 'string' ? request.id : undefined })
        : modelFailureDiagnostic({ message: settled, completionRequired })
      return { schemaVersion: 1, runId: request.runId, turnId: frame.input?.turnId ?? frame.turnId,
        checkpointSeq: frame.checkpointSeq, ...evidence, ...(diagnostic ? { diagnostic } : {}) }
    },
  }
}
