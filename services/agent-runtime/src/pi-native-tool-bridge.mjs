// Host owns admission and settlement while Pi drives the loop.
import { createHash } from 'node:crypto'
import { validateWireValue } from '../../../packages/fox-engine-protocol/index.mjs'
import { decodeHostToolResponse, hostToolResultStorage } from './host-tools.mjs'
import { normalizeFoxToolResultForPi } from './tool-adapter.mjs'
import { capModelToolResultContent } from './tool-view.mjs'

export const TOOL_BATCH_REGISTER = 'tool_batch.register'
export const TOOL_CALL_EXECUTE = 'tool_call.execute'
export const TOOL_CALL_CLOSE = 'tool_call.close'
export const TOOL_CALL_CANCEL = 'tool_call.cancel'

function canonical(value) {
  if (Array.isArray(value)) return `[${value.map((item) => canonical(item) ?? 'null').join(',')}]`
  if (value && typeof value === 'object') {
    return `{${Object.keys(value).filter((key) => value[key] !== undefined).sort()
      .map((key) => `${JSON.stringify(key)}:${canonical(value[key])}`).join(',')}}`
  }
  return JSON.stringify(value)
}
export const nativeToolInputDigest = (value) => 'sha256:' + createHash('sha256').update(canonical(value)).digest('hex')
export const NATIVE_RESPONSE_DIGEST = 'foxNativeHostResponseDigest'
export const NATIVE_NONEXECUTION = 'foxNativeNonExecution'
const cleanupSignal = () => AbortSignal.timeout(5000)

export function nativeHostToolResult(tool, payload, { runId, toolCallId }) {
  let result
  try {
    const raw = decodeHostToolResponse(tool, { payload })
    result = normalizeFoxToolResultForPi(tool, raw, {
      runId, toolCallId, storage: hostToolResultStorage(toolCallId, raw),
    })
  } catch (error) {
    // TB-2: a failure's diagnostics obey the same ceiling as a success.
    const capped = capModelToolResultContent([{ type: 'text', text: error.message }], {
      resultRef: typeof payload?.resultRef === 'string' ? payload.resultRef : null,
      storage: payload?.toolResultStorage ?? null,
    })
    result = { content: capped.content, isError: true,
      details: { ...(error.code ? { errorCode: error.code } : {}), ...(capped.capped ? { outputTruncated: true } : {}) } }
  }
  const details = result.details && typeof result.details === 'object' && !Array.isArray(result.details)
    ? result.details : { foxNativeOriginalDetails: result.details ?? null }
  return { ...result, isError: result.isError === true || payload?.isError === true,
    details: { ...details, [NATIVE_RESPONSE_DIGEST]: nativeToolInputDigest(payload) } }
}

function toolCallsOf(assistantMessage) {
  const content = assistantMessage?.content
  if (!Array.isArray(content)) return []
  return content.filter((block) => block?.type === 'toolCall')
}

// The envelope `type` names the request; the wire schema is named after its
// payload DTO. Keeping the two apart is what lets both sides validate against
// the generated contract instead of a hand-copied shape.
const REQUEST_SCHEMAS = {
  [TOOL_BATCH_REGISTER]: 'ToolBatchRegisterRequest',
  [TOOL_CALL_EXECUTE]: 'ToolCallExecuteRequest',
  [TOOL_CALL_CLOSE]: 'ToolCallCloseRequest',
  [TOOL_CALL_CANCEL]: 'ToolCallCancelRequest',
}

function assertRequestPayload(request, payload) {
  const schema = REQUEST_SCHEMAS[request]
  if (!schema) throw new Error(`unfrozen native batch request: ${request}`)
  const errors = validateWireValue(schema, payload)
  if (errors.length) throw new Error(`invalid ${request} payload: ${errors.join('; ')}`)
}

export const NATIVE_TOOL_CALL_ID_REUSED = 'runtime.native_tool_call_id_reused'
export const NATIVE_BATCH_REFUSED = 'host.native_batch_refused'

function codedError(code, message) {
  const error = new Error(`[${code}] ${message}`)
  error.code = code
  return error
}

// TB-3: a Host refusal is not a malformed success. Keep its reason and code so
// they reach run.failed instead of a schema-validation message.
function responsePayload(request, response, schema) {
  const payload = response?.payload ?? {}
  if (response?.type === 'native_batch.refused' || payload?.isError === true) {
    const reason = typeof payload.error === 'string' && payload.error
      ? payload.error
      : typeof payload.message === 'string' && payload.message ? payload.message : 'no reason given'
    const code = payload.errorDetails?.code || payload.code || NATIVE_BATCH_REFUSED
    const error = codedError(code, `${request} refused by Host: ${reason}`)
    error.hostReason = reason
    throw error
  }
  const errors = validateWireValue(schema, payload)
  if (errors.length) throw new Error(`invalid ${request} response: ${errors.join('; ')}`)
  return payload
}

/**
 * Build the bridge for one Run and one context generation.
 *
 * `requestHost(type, payload, signal)` is the existing sidecar->Host transport.
 * The bridge never decides whether a call may run; it only reports positions and
 * waits for the Host's answer.
 */
export function createNativeToolBridge({ requestHost, runId, contextGeneration, signal: runSignal, callIdPrefix = '' }) {
  if (typeof requestHost !== 'function') throw new Error('a native tool bridge needs requestHost')
  if (!runId || !runId.trim()) throw new Error('a native tool bridge needs a runId')
  if (!Number.isSafeInteger(contextGeneration) || contextGeneration < 1) {
    throw new Error('a native tool bridge needs a positive contextGeneration')
  }
  if (typeof callIdPrefix !== 'string') throw new Error('a native tool call prefix must be a string')
  const hostCallId = callId => `${callIdPrefix}${callId}`

  // Call ids identify a batch even when the SDK copies its assistant message.
  const batches = new Map()
  const callBatches = new Map()
  const prepared = new Set()
  const executed = new Set()
  const closed = new Set()
  const closing = new Map()
  const pendingEnds = new Map()
  const pending = new Set()
  const failures = new Set()
  const catalogNames = new Set()
  const activeSignal = signal => runSignal
    ? (signal ? AbortSignal.any([runSignal, signal]) : runSignal) : signal
  const registrationSignal = signal => {
    const active = activeSignal(signal)
    return active ? AbortSignal.any([active, cleanupSignal()]) : cleanupSignal()
  }

  function track(operation) {
    pending.add(operation)
    operation.then(
      () => pending.delete(operation),
      (error) => { pending.delete(operation); failures.add(error) },
    )
    return operation
  }

  // Event subscribers need not await cleanup, but callers must still observe failures.
  async function drain() {
    while (pending.size) await Promise.allSettled([...pending])
    if (failures.size) {
      const errors = [...failures]
      failures.clear()
      throw errors.length === 1 ? errors[0] : new AggregateError(errors, 'native tool cleanup failed')
    }
  }

  // TB-1: the stream layer rewrites empty or repeated ids before Pi sees them,
  // so a reused id here is a contract breach. Fail fast instead of replaying an
  // earlier batch's result or mixing two batches in one Host ledger entry.
  function batchFor(assistantMessage, { allowEnded = false } = {}) {
    const calls = toolCallsOf(assistantMessage)
    if (!calls.length) return null
    const ids = calls.map((call) => call.id)
    if (ids.some((id) => typeof id !== 'string' || !id)) {
      throw codedError(NATIVE_TOOL_CALL_ID_REUSED, 'a native tool call has no id')
    }
    if (new Set(ids).size !== ids.length) {
      throw codedError(NATIVE_TOOL_CALL_ID_REUSED, `a native tool batch repeats a call id: ${ids.join(', ')}`)
    }
    const key = [...ids].sort().join('|')
    let batch = batches.get(key)
    if (batch) {
      if (batch.ended && !allowEnded) {
        throw codedError(NATIVE_TOOL_CALL_ID_REUSED, `native tool call ids were already used by an earlier turn: ${key}`)
      }
      return batch
    }
    for (const id of ids) {
      if (callBatches.has(id)) {
        throw codedError(NATIVE_TOOL_CALL_ID_REUSED, `native tool call id was already used by another batch: ${id}`)
      }
    }
    batch = {
      key,
      calls,
      batchId: null,
      registration: null,
      ended: false,
    }
    batches.set(key, batch)
    for (const call of calls) callBatches.set(call.id, batch)
    return batch
  }

  function register(batch, signal) {
    if (!batch.registration) {
      const payload = {
        runId,
        contextGeneration,
        calls: batch.calls.map((call, index) => ({
          callId: hostCallId(call.id),
          tool: call.name,
          sourceOrder: index,
          // The model's own arguments, before SDK conversion: this is what a
          // replay must match. Approval binds to the converted input instead.
          inputDigest: nativeToolInputDigest(call.arguments ?? {}),
        })),
      }
      assertRequestPayload(TOOL_BATCH_REGISTER, payload)
      batch.registration = Promise.resolve().then(() => requestHost(TOOL_BATCH_REGISTER, payload, signal))
        .then((response) => {
          const body = responsePayload(TOOL_BATCH_REGISTER, response, 'ToolBatchRegisterResponse')
          batch.batchId = body.batchId
          for (const call of batch.calls) {
            const end = pendingEnds.get(call.id)
            if (end) close(batch, call.id, reasonForUnexecuted(batch, call.id, end.event, end.signal))
          }
          return body
        }).catch((error) => {
          batch.registration = null
          throw error
        })
    }
    return batch.registration
  }

  function close(batch, callId, reason) {
    if (closed.has(callId) || executed.has(callId)) return Promise.resolve(null)
    if (closing.has(callId)) return closing.get(callId)
    const operation = (async () => {
      await batch.registration
      const payload = { runId, batchId: batch.batchId, callId: hostCallId(callId), reason, isError: true }
      assertRequestPayload(TOOL_CALL_CLOSE, payload)
      const response = await requestHost(TOOL_CALL_CLOSE, payload, cleanupSignal())
      const body = responsePayload(TOOL_CALL_CLOSE, response, 'ToolCallCloseResponse')
      if (!body.closed && !['completed', 'not_started', 'cancelled'].includes(body.state)) {
        throw new Error(`tool_call.close was not confirmed for ${callId}: ${body.state}`)
      }
      closed.add(callId)
      pendingEnds.delete(callId)
      return body
    })().finally(() => closing.delete(callId))
    closing.set(callId, operation)
    return track(operation)
  }

  async function cancel(batch, callId) {
    const payload = { runId, batchId: batch.batchId, callId: hostCallId(callId) }
    assertRequestPayload(TOOL_CALL_CANCEL, payload)
    const response = await requestHost(TOOL_CALL_CANCEL, payload, cleanupSignal())
    return responsePayload(TOOL_CALL_CANCEL, response, 'ToolCallCancelResponse')
  }

  /**
   * The SDK enters this after arguments are validated, so a call that never
   * arrives here either named an unknown tool or failed validation.
   */
  async function beforeToolCall(context, signal) {
    const batch = batchFor(context.assistantMessage)
    if (!batch) return undefined
    const callId = context.toolCall?.id
    if (callId) prepared.add(callId)
    // Registration only. Never await a settlement here.
    await register(batch, activeSignal(signal))
    return undefined
  }

  async function waitForEventRegistration(event, signal) {
    if (!['tool_execution_start', 'tool_execution_update', 'tool_execution_end'].includes(event?.type)) return
    const batch = callBatches.get(event.toolCallId)
    if (!batch) throw new Error(`native tool event has no assistant batch: ${event.toolCallId}`)
    await register(batch, registrationSignal(signal))
  }

  /**
   * Mark every managed tool parallel and route its execution through the Host.
   * The Host applies the read-only-prefix/write barrier before it claims the
   * call, so concurrent submission does not mean concurrent side effects.
   */
  function wrapTools(tools) {
    for (const tool of tools) catalogNames.add(tool.name)
    return tools.map((tool) => ({
      ...tool,
      executionMode: 'parallel',
      execute: async (toolCallId, args, signal) => {
        signal = activeSignal(signal)
        const batch = callBatches.get(toolCallId)
        if (!batch) {
          throw new Error(`tool_call.execute without a registered batch: ${toolCallId}`)
        }
        if (executed.has(toolCallId) || closed.has(toolCallId)) {
          throw codedError(NATIVE_TOOL_CALL_ID_REUSED, `native tool call ${toolCallId} was already ${executed.has(toolCallId) ? 'executed' : 'closed'}`)
        }
        await batch.registration
        if (signal?.aborted) {
          await cancel(batch, toolCallId)
          throw new Error(`tool call ${toolCallId} was aborted before execution`)
        }
        executed.add(toolCallId)
        const payload = { runId, batchId: batch.batchId, callId: hostCallId(toolCallId), tool: tool.name, input: args ?? {} }
        assertRequestPayload(TOOL_CALL_EXECUTE, payload)
        let response
        try {
          response = await requestHost(TOOL_CALL_EXECUTE, payload, signal)
        } catch (error) {
          await cancel(batch, toolCallId)
          throw error
        }
        if (signal?.aborted) {
          await cancel(batch, toolCallId)
          throw new Error(`tool call ${toolCallId} was aborted during execution`)
        }
        // The Host answer carries the same payload the Legacy execute path
        // decodes, so result storage and failure diagnostics stay identical.
        return nativeHostToolResult(tool.name, response.payload ?? {}, { runId, toolCallId: hostCallId(toolCallId) })
      },
    }))
  }

  /**
   * Feed every Agent event here. `tool_execution_end` is how the SDK reports a
   * position that never reached execute, and `turn_end` is the last chance to
   * close a position that produced no event at all.
   */
  function onEvent(event, signal) {
    if (event?.type === 'message_end' && event.message?.role === 'assistant') {
      const batch = batchFor(event.message)
      if (batch) return track(register(batch, cleanupSignal()))
    }
    if (event?.type === 'tool_execution_end') {
      const callId = event.toolCallId
      if (executed.has(callId) || closed.has(callId)) return Promise.resolve(null)
      if (!pendingEnds.has(callId)) pendingEnds.set(callId, { event, signal })
      const batch = callBatches.get(callId)
      if (!batch) return Promise.resolve(null)
      const end = pendingEnds.get(callId)
      const reason = reasonForUnexecuted(batch, callId, end.event, end.signal)
      const call = batch.calls.find(call => call.id === callId)
      if (call && event.result) {
        event.result.details = { ...event.result.details, [NATIVE_NONEXECUTION]: {
          schemaVersion: 1, reason, tool: call.name, inputDigest: nativeToolInputDigest(call.arguments ?? {}),
        } }
      }
      if (!batch.registration) return Promise.resolve(null)
      return close(batch, callId, reason)
    }
    if (event?.type === 'turn_end') {
      if (event.message?.role === 'assistant') {
        const ending = batchFor(event.message, { allowEnded: true })
        if (ending) ending.ended = true
      }
      return track((async () => {
        const closes = []
        for (const batch of batches.values()) {
          await register(batch, cleanupSignal())
          for (const call of batch.calls) {
            if (executed.has(call.id) || closed.has(call.id)) continue
            const end = pendingEnds.get(call.id)
            const reason = end ? reasonForUnexecuted(batch, call.id, end.event, end.signal) : 'not_started'
            closes.push(close(batch, call.id, reason))
          }
        }
        return Promise.all(closes)
      })())
    }
    return Promise.resolve(null)
  }

  // beforeToolCall runs only after validation, so what the call did reach tells
  // us why it produced no result. Unknown names never reach it either, and are
  // distinguished from a validation failure by the catalog.
  function reasonForUnexecuted(batch, callId, event, signal) {
    if (signal?.aborted) return 'aborted'
    if (prepared.has(callId)) return 'intercepted'
    const call = batch.calls.find((entry) => entry.id === callId)
    const name = call?.name ?? (typeof event?.toolName === 'string' ? event.toolName : null)
    if (name && !catalogNames.has(name)) return 'unknown_tool'
    return 'validation_failed'
  }

  return {
    beforeToolCall,
    waitForEventRegistration,
    wrapTools,
    onEvent,
    drain,
    hostCallId,
    get batchCount() { return batches.size },
    get pendingPositions() {
      let count = 0
      for (const batch of batches.values()) {
        for (const call of batch.calls) if (!executed.has(call.id) && !closed.has(call.id)) count += 1
      }
      return count
    },
  }
}
