// Controlled handoff for an already-settled durable Kernel batch. This module
// is not registered as a Legacy sidecar command. It executes no recovered tool,
// grants no permission and regenerates no missing result.
import { validateKernelControl } from './control-binding.mjs'
import { sanitizeProviderHistory } from './runtime-session.mjs'
import { RUNTIME_TOOL_CATALOG, validateWireValue } from '../../../packages/fox-engine-protocol/index.mjs'
import { finalizeKernelAnswer, completionPreview } from './kernel-completion.mjs'
import { createRoundProgress, toolParamBytesOf } from './kernel-model-progress.mjs'
import { modelToolResultContent, toolResultRef } from './tool-view.mjs'
import { steeringNoticeText, validateSteeringNotices } from './steering-notice.mjs'

const knownTools = new Set(RUNTIME_TOOL_CATALOG.map(tool => tool.name))
const deliveries = new WeakMap()
const proposalToolsBySession = new WeakMap()
const fail = message => { throw new Error(`Invalid Kernel batch resume: ${message}`) }
const record = value => value !== null && typeof value === 'object' && !Array.isArray(value)
const nonempty = value => typeof value === 'string' && value.trim().length > 0

function canonical(value, omitUndefined = false) {
  if (value === null || typeof value === 'string' || typeof value === 'boolean') return value
  if (typeof value === 'number' && Number.isFinite(value)) return value
  if (Array.isArray(value)) return value.map(item => canonical(item, omitUndefined))
  if (record(value)) return Object.fromEntries(Object.keys(value).sort()
    .filter(key => !omitUndefined || value[key] !== undefined).map(key => [key, canonical(value[key], omitUndefined)]))
  fail('non-JSON value')
}
const sameJson = (a, b) => JSON.stringify(canonical(a)) === JSON.stringify(canonical(b))

function preparePiReplayHistory(messages) {
  // Keep provider diagnostics stripped. Pi 0.84's context estimator requires
  // usage on assistant messages even when compaction is disabled. Zero usage
  // asks it to estimate from content; this is disposable SDK metadata, not
  // authoritative billing and must never be written back to durable history.
  return sanitizeProviderHistory(structuredClone(messages)).map(message => message.role === 'assistant'
    ? { ...message, usage: {
        input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0,
        cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
      } }
    : message)
}

export function prepareKernelModelResponse(assistantMessage, prepared) {
  // Pi uses undefined for absent optional metadata. Omit those object fields
  // as JSON transport does, but never repair non-JSON proposed arguments.
  for (const block of assistantMessage?.content ?? []) {
    if (block?.type === 'toolCall') canonical(block.arguments)
  }
  assistantMessage = canonical(assistantMessage, true)
  assistantMessage = finalizeKernelAnswer(assistantMessage, prepared.requireCompletion)
  const response = { schemaVersion: 1, runId: prepared.runId, turnId: prepared.turnId,
    ...(prepared.initial ? {} : { batchId: prepared.batchId }), checkpointSeq: prepared.checkpointSeq, assistantMessage }
  canonical(response)
  if (Buffer.byteLength(JSON.stringify(response), 'utf8') > 1_048_576) fail('model response exceeds protocol size limit')
  const errors = validateWireValue(prepared.initial ? 'KernelInitialModelResponse' : 'KernelModelResponse', response)
  if (errors.length) fail(errors.join('; '))
  if (!nonempty(response.runId) || !nonempty(response.turnId) || !prepared.initial && !nonempty(response.batchId) || !Number.isSafeInteger(response.checkpointSeq) || response.checkpointSeq < 1
      || assistantMessage?.role !== 'assistant' || !Array.isArray(assistantMessage.content)) fail('invalid model response')
  if (assistantMessage.stopReason === 'toolUse') {
    const calls = assistantMessage.content.filter(block => block?.type === 'toolCall')
    const ids = new Set()
    if (!calls.length || calls.length > 64) fail('invalid next tool batch size')
    for (const call of calls) {
      if (!nonempty(call.id) || ids.has(call.id) || !knownTools.has(call.name) || !record(call.arguments)) fail('invalid next tool proposal')
      ids.add(call.id)
    }
  } else if (assistantMessage.stopReason !== 'stop' || assistantMessage.content.some(block =>
    !(block?.type === 'text' && typeof block.text === 'string' || block?.type === 'thinking' && typeof block.thinking === 'string'))) {
    fail('model continuation has no completed response')
  }
  return structuredClone(response)
}

function assertCompleteHistory(history) {
  const pending = new Map()
  for (const message of history) {
    if (!record(message) || !['user', 'assistant', 'toolResult'].includes(message.role)) fail('unsupported history message')
    if (message.role === 'toolResult') {
      if (!nonempty(message.toolCallId) || !nonempty(message.toolName) || pending.get(message.toolCallId) !== message.toolName) fail('orphan, duplicate or mismatched historical result')
      if (typeof message.isError !== 'boolean') fail('historical result has no error classification')
      resultContent(message)
      pending.delete(message.toolCallId)
      continue
    }
    if (pending.size) fail('incomplete historical tool batch')
    if (typeof message.content !== 'string' && !Array.isArray(message.content)) fail('invalid history content')
    if (message.role === 'assistant' && Array.isArray(message.content)) {
      for (const block of message.content.filter(item => item?.type === 'toolCall')) {
        if (!nonempty(block.id) || !nonempty(block.name) || pending.has(block.id)) fail('invalid historical tool identity')
        pending.set(block.id, block.name)
      }
    }
  }
  if (pending.size) fail('incomplete historical tool batch')
}

function resultContent(result) {
  if (!record(result) || !Array.isArray(result.content)) fail('missing durable tool result content')
  for (const block of result.content) {
    if (block?.type === 'text' && typeof block.text === 'string') continue
    if (block?.type === 'image' && typeof block.data === 'string' && typeof block.mimeType === 'string') continue
    fail('unsupported durable result content')
  }
  return result.content
}

export function prepareKernelBatchResume(request, identity) {
  if (!identity || !nonempty(identity.executionProfileId)) fail('missing adapter identity')
  validateKernelControl(request, identity.executionProfileId, identity.engineId)
  for (const key of ['runId', 'conversationId', 'runtimeSessionId']) {
    if (!nonempty(identity[key]) || request[key] !== identity[key]) fail(`mismatched ${key}`)
  }
  // Bound before cloning; an invalid frame is not repaired into plausible input.
  const encoded = JSON.stringify(request)
  if (!encoded || Buffer.byteLength(encoded, 'utf8') > 1_048_576) fail('frame exceeds the protocol size limit')
  canonical(request)
  const frame = request.payload?.batchResume
  const schemaErrors = validateWireValue('KernelBatchResumeFrame', frame)
  if (schemaErrors.length) fail(schemaErrors.join('; '))
  if (!record(frame) || frame.schemaVersion !== 1 || !nonempty(frame.turnId) || !nonempty(frame.batchId)
      || frame.idempotencyKey !== `tool-batch-delivery:${frame.batchId}`
      || !Number.isSafeInteger(frame.checkpointSeq) || frame.checkpointSeq < 1) fail('invalid durable batch identity')
  if (!Array.isArray(frame.history)) fail('missing history')
  assertCompleteHistory(frame.history)
  const assistant = frame.assistantMessage
  if (!record(assistant) || assistant.role !== 'assistant' || assistant.stopReason !== 'toolUse'
      || !Array.isArray(assistant.content)) fail('missing original assistant tool proposal')
  const calls = assistant.content.filter(block => block?.type === 'toolCall')
  if (calls.length < 1 || calls.length > 64 || !Array.isArray(frame.tools) || frame.tools.length !== calls.length) fail('incomplete result barrier')
  const byId = new Map()
  for (const item of frame.tools) {
    if (!record(item) || !nonempty(item.toolCallId) || byId.has(item.toolCallId)) fail('duplicate or invalid result identity')
    byId.set(item.toolCallId, item)
  }
  const seen = new Set()
  const results = calls.map((call, sourceOrder) => {
    if (!nonempty(call.id) || seen.has(call.id) || !knownTools.has(call.name) || !record(call.arguments)) fail('invalid proposed tool')
    seen.add(call.id)
    const item = byId.get(call.id)
    if (!item || item.tool !== call.name || item.sourceOrder !== sourceOrder || !record(item.canonicalInput)
        || !sameJson(item.canonicalInput, call.arguments)) fail('approved tool identity or parameters changed')
    if (!['completed', 'failed'].includes(item.state)) fail('unsettled or cancelled tool cannot resume a batch')
    const isError = item.state === 'failed'
    if (Object.hasOwn(item.result ?? {}, 'isError') && item.result.isError !== isError) fail('result contradicts durable terminal state')
    return {
      role: 'toolResult', toolCallId: call.id, toolName: call.name,
      // Model-view bounding only; the durable item.result stays complete and
      // errors/receipts pass through unchanged. The bound view carries a stable
      // reference to the persisted result (see toolResultRef).
      content: modelToolResultContent(call.name, {
        isError,
        content: resultContent(item.result),
        details: item.result?.details,
        resultRef: toolResultRef(request.runId, call.id),
      }),
      details: {}, isError,
      timestamp: Number.isSafeInteger(assistant.timestamp) ? assistant.timestamp : 0,
    }
  })
  // Mid-run additions bound to this (possibly retried) dispatch: ordinary
  // user text appended AFTER the settled results, matching the live-session
  // directive ordering; they never carry tools, grants or authority.
  const steering = validateSteeringNotices(frame.steering, fail)
  const steeringMessages = steering.map(notice => ({
    role: 'user',
    content: [{ type: 'text', text: steeringNoticeText(notice.content) }],
    timestamp: 0,
  }))
  // All results were checked before the normal provider projection; no synthetic
  // recovery failures may be inserted to fill a missing result here.
  const messages = preparePiReplayHistory([...frame.history, assistant, ...results, ...steeringMessages])
  return { messages, runId: request.runId, turnId: frame.turnId,
    idempotencyKey: frame.idempotencyKey, batchId: frame.batchId, checkpointSeq: frame.checkpointSeq }
}

export function prepareKernelInitialModel(request, identity) {
  validateKernelControl(request, identity?.executionProfileId, identity?.engineId)
  if (!identity || ['runId', 'conversationId', 'runtimeSessionId'].some(key => !nonempty(identity[key]) || request[key] !== identity[key])) fail('initial identity mismatch')
  canonical(request)
  if (Buffer.byteLength(JSON.stringify(request), 'utf8') > 1_048_576) fail('initial frame is too large')
  const frame = request.payload?.initialModel
  const continuationKey = frame?.continuationKey ?? undefined
  if (continuationKey !== undefined && (typeof continuationKey !== 'string'
      || !/^continuation:[1-9][0-9]*$/.test(continuationKey)
      || !Number.isSafeInteger(Number(continuationKey.slice('continuation:'.length))))) fail('invalid continuation identity')
  const expectedKey = continuationKey === undefined ? 'initial-model-delivery' : `continuation-delivery:${continuationKey}`
  if (validateWireValue('KernelInitialModelFrame', frame).length || frame.schemaVersion !== 1
      || frame.idempotencyKey !== expectedKey || !Number.isSafeInteger(frame.checkpointSeq) || frame.checkpointSeq < 1) fail('invalid initial frame')
  const input = frame.input
  if (input.schemaVersion !== 1 || input.runId !== request.runId || !nonempty(input.turnId) || !nonempty(input.promptConfigHash)
      || !input.messages.length || input.messages.at(-1)?.role !== 'user') fail('invalid initial input')
  assertCompleteHistory(input.messages)
  for (const message of input.messages) {
    const calls = Array.isArray(message.content) && message.content.some(block => block?.type === 'toolCall')
    if (message.role === 'assistant' && Object.hasOwn(message, 'stopReason') && message.stopReason !== (calls ? 'toolUse' : 'stop')) fail('unfinished initial history')
    for (const block of Array.isArray(message.content) ? message.content : []) {
      if (block?.type === 'text' && typeof block.text === 'string'
          || message.role !== 'assistant' && block?.type === 'image' && typeof block.data === 'string' && typeof block.mimeType === 'string'
          || message.role === 'assistant' && block?.type === 'thinking' && typeof block.thinking === 'string'
          || message.role === 'assistant' && block?.type === 'toolCall' && record(block.arguments)) continue
      fail('unsupported initial content')
    }
  }
  return { messages: preparePiReplayHistory(input.messages), runId: input.runId, turnId: input.turnId,
    idempotencyKey: frame.idempotencyKey, checkpointSeq: frame.checkpointSeq, initial: true }
}

/**
 * Single-round runner for the durable per-round transport (Host compaction and
 * replacement-session initial/batch deliveries). The Run tool loop lives in
 * pi-kernel-loop.mjs; this runner never executes tools, never seeds results
 * and never continues after the one model round. A proposal stopReason is a
 * normal output here: the Host approves and executes it in the next durable
 * delivery.
 */
export async function runPiKernelModel(session, request, prepared, signal, preview, { allowProposals = false } = {}) {
  if (!session?.agent?.state || typeof session.agent.continue !== 'function' || typeof session.abort !== 'function') fail('missing public Pi session adapter')
  if (!signal || typeof signal.addEventListener !== 'function') fail('missing Host cancellation signal')
  if (signal.aborted) fail('Host cancelled the resume')
  if (session.isIdle !== true || session.state?.isStreaming || session.agent.signal) fail('session is already running')
  // Tool-free requests (Host compaction) cannot carry proposal schemas. A
  // single-round delivery advertises strict schemas with proposal-only tools;
  // their boundary (below) stops the engine before any execution.
  if (!allowProposals && session.agent.state.tools?.length) fail('single-round requests cannot retain executable tools')
  const settings = session.settingsManager
  if (settings?.getRetryEnabled?.() !== false || settings?.getRetrySettings?.().maxRetries !== 0
      || settings?.getProviderRetrySettings?.().maxRetries !== 0 || settings?.getCompactionEnabled?.() !== false) {
    fail('replacement session retains autonomous retry or compaction policy')
  }
  const deliveries = deliveriesBySession.get(session) ?? new Set()
  const key = JSON.stringify([request.runId, prepared.idempotencyKey])
  if (deliveries.has(key)) fail('batch delivery already claimed; reconcile instead of replaying')
  if (session.agent.state.messages?.length || session.agent.hasQueuedMessages?.()) fail('resume requires an empty replacement session')
  deliveries.add(key)
  deliveriesBySession.set(session, deliveries)
  let abortError
  let aborting
  let timedOut = false
  let timeoutKind = null
  const abort = () => { aborting ??= Promise.resolve().then(() => session.abort()).catch(error => { abortError = error }) }
  signal.addEventListener('abort', abort, { once: true })
  // Same first-response / idle / whole-round contract as the live loop: tool
  // parameter bytes count as progress, and the breach kind plus byte-level
  // telemetry travel with the timeout failure for Host retry admission.
  const roundProgress = createRoundProgress({ budgets: request.payload.controlBinding.budgets })
  const timer = setInterval(() => {
    if (signal.aborted || timedOut) return
    const breach = roundProgress.check()
    if (breach === 'ok') return
    timeoutKind = breach
    timedOut = true
    abort()
  }, 250)
  timer.unref?.()
  let proposed
  const stopMarker = `fox-kernel-proposal-boundary:${request.runId}:${prepared.idempotencyKey}`
  let unsubscribe
  let previewRevision = 0
  let lastPreviewAt = 0
  let lastPreviewText = ''
  let lastPreviewReasoning = ''
  let lastPreviewToolBytes = 0
  let lastProgressPreviewAt = 0
  try {
    unsubscribe = typeof session.agent.subscribe === 'function' ? session.agent.subscribe(event => {
      if (signal.aborted || timedOut || !['message_update', 'message_end'].includes(event.type)
          || event.message?.role !== 'assistant' || !Array.isArray(event.message.content)) {
        if (!allowProposals) return
      } else {
        const text = completionPreview(event.message.content.filter(block => block?.type === 'text' && typeof block.text === 'string').map(block => block.text).join(''), prepared.requireCompletion)
        const reasoning = event.message.content.filter(block => block?.type === 'thinking' && typeof block.thinking === 'string').map(block => block.thinking).join('\n\n')
        const toolBytes = toolParamBytesOf(event.message.content)
        roundProgress.note({ text, reasoning, toolParamBytes: toolBytes })
        if (preview) {
          const now = performance.now()
          const toolGrowth = toolBytes - lastPreviewToolBytes
          const toolPreviewDue = toolGrowth !== 0 && (toolBytes === 0 || toolGrowth >= 16384 || now - lastProgressPreviewAt >= 1000)
          if (((text !== lastPreviewText || reasoning !== lastPreviewReasoning) || toolPreviewDue) && Buffer.byteLength(text, 'utf8') <= 262_144 && Buffer.byteLength(reasoning, 'utf8') <= 262_144
              && (previewRevision === 0 || now - lastPreviewAt >= 100 || event.type === 'message_end')) {
            lastPreviewText = text; lastPreviewReasoning = reasoning; lastPreviewToolBytes = toolBytes; lastPreviewAt = now
            if (toolPreviewDue) lastProgressPreviewAt = now
            preview({ schemaVersion: 1, runId: request.runId, conversationId: request.conversationId, turnId: prepared.turnId,
              checkpointSeq: prepared.checkpointSeq, revision: ++previewRevision, text, ...(reasoning ? { reasoning } : {}),
              progressBytes: roundProgress.totalBytes() })
          }
        }
      }
      if (!allowProposals) return
      if (event.type !== 'message_end' || event.message?.role !== 'assistant'
          || event.message.stopReason !== 'toolUse' && !event.message.content?.some?.(block => block?.type === 'toolCall')) return
      proposed = prepareKernelModelResponse(event.message, prepared)
      // Throwing from this awaited public event prevents even tool preparation.
      // Do not await session.abort() here: it waits on the same event listener.
      session.agent.abort()
      throw new Error(stopMarker)
    }) : undefined
    // Pi 0.84's documented public state setter copies the message array.
    session.agent.state.messages = prepared.messages
    await session.agent.continue()
    if (aborting) await aborting
    if (abortError) fail(`engine cancellation failed: ${abortError.message ?? String(abortError)}`)
    if (timedOut) {
      const kind = timeoutKind && timeoutKind !== 'ok' ? timeoutKind : roundProgress.check()
      const timeout = new Error(`Kernel batch resume exceeded its frozen time budget (${kind})`)
      timeout.evidence = {
        schemaVersion: 1, runId: request.runId, turnId: prepared.turnId,
        checkpointSeq: prepared.checkpointSeq, category: 'model_timeout',
        httpStatus: null, retryAfterMs: null,
        telemetry: roundProgress.telemetry(),
      }
      throw timeout
    }
    if (signal.aborted) fail('Host cancelled the resume')
    const final = session.agent.state.messages.at(-1)
    if (proposed) {
      if (final?.stopReason !== 'aborted' || final.errorMessage !== stopMarker) fail('proposal boundary did not stop the engine')
      return { idempotencyKey: prepared.idempotencyKey, checkpointSeq: prepared.checkpointSeq, response: proposed }
    }
    if (final?.role !== 'assistant' || final.stopReason !== 'stop') {
      fail(`model continuation has no completed response (${final?.stopReason ?? final?.role ?? 'missing'})`)
    }
    return { idempotencyKey: prepared.idempotencyKey, checkpointSeq: prepared.checkpointSeq, response: prepareKernelModelResponse(final, prepared) }
  } finally {
    clearInterval(timer)
    signal.removeEventListener('abort', abort)
    unsubscribe?.()
  }
}

const deliveriesBySession = new WeakMap()
