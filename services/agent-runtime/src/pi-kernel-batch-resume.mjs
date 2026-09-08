// Controlled handoff for an already-settled durable Kernel batch. This module
// is not registered as a Legacy sidecar command. It executes no recovered tool,
// grants no permission and regenerates no missing result.
import { validateKernelControl } from './control-binding.mjs'
import { sanitizeProviderHistory } from './runtime-session.mjs'
import { RUNTIME_TOOL_CATALOG, validateWireValue } from '../../../packages/fox-engine-protocol/index.mjs'

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

// Advertise schemas without installing a Resource executor. The public event
// boundary below stops the disposable engine before argument preparation or
// any tool callback. Even a broken boundary cannot execute a local resource.
export function installKernelProposalTools(session, definitions) {
  if (session?.isIdle !== true || !Array.isArray(session.agent?.state?.tools)
      || session.agent.state.tools.length || typeof session.agent.subscribe !== 'function'
      || typeof session.agent.abort !== 'function') fail('proposal tools require an empty idle public Pi adapter')
  if (!Array.isArray(definitions) || definitions.length > 64) fail('invalid proposal tool definitions')
  const names = new Set()
  const tools = definitions.map(definition => {
    if (!knownTools.has(definition?.name) || names.has(definition.name) || !nonempty(definition.description)
        || !record(definition.parameters) || definition.parameters.type !== 'object') fail('invalid proposal tool schema')
    names.add(definition.name)
    const parameters = canonical(definition.parameters)
    if (Buffer.byteLength(JSON.stringify(parameters), 'utf8') > 131_072) fail('proposal schema is too large')
    return Object.freeze({ name: definition.name, label: definition.name, description: definition.description,
      parameters, executionMode: 'parallel', execute: async () => { fail('proposal-only engine has no resource executor') } })
  })
  proposalToolsBySession.set(session, tools)
  session.agent.state.tools = tools
}

export function prepareKernelModelResponse(assistantMessage, prepared) {
  // Pi uses undefined for absent optional metadata. Omit those object fields
  // as JSON transport does, but never repair non-JSON proposed arguments.
  for (const block of assistantMessage?.content ?? []) {
    if (block?.type === 'toolCall') canonical(block.arguments)
  }
  assistantMessage = canonical(assistantMessage, true)
  const response = { schemaVersion: 1, runId: prepared.runId, turnId: prepared.turnId,
    batchId: prepared.batchId, checkpointSeq: prepared.checkpointSeq, assistantMessage }
  canonical(response)
  if (Buffer.byteLength(JSON.stringify(response), 'utf8') > 1_048_576) fail('model response exceeds protocol size limit')
  const errors = validateWireValue('KernelModelResponse', response)
  if (errors.length) fail(errors.join('; '))
  if (!nonempty(response.runId) || !nonempty(response.turnId) || !nonempty(response.batchId) || !Number.isSafeInteger(response.checkpointSeq) || response.checkpointSeq < 1
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
  validateKernelControl(request, identity.executionProfileId)
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
      content: resultContent(item.result), details: {}, isError,
      timestamp: Number.isSafeInteger(assistant.timestamp) ? assistant.timestamp : 0,
    }
  })
  // All results were checked before the normal provider projection; no synthetic
  // recovery failures may be inserted to fill a missing result here.
  const messages = sanitizeProviderHistory(structuredClone([...frame.history, assistant, ...results]))
  return { messages, runId: request.runId, turnId: frame.turnId,
    idempotencyKey: frame.idempotencyKey, batchId: frame.batchId, checkpointSeq: frame.checkpointSeq }
}

export async function resumePiKernelBatch(session, request, identity, signal) {
  const prepared = prepareKernelBatchResume(request, identity)
  if (!session?.agent?.state || typeof session.agent.continue !== 'function' || typeof session.abort !== 'function') fail('missing public Pi session adapter')
  if (!signal || typeof signal.addEventListener !== 'function') fail('missing Host cancellation signal')
  if (signal.aborted) fail('Host cancelled the resume')
  if (session.isIdle !== true || session.state?.isStreaming || session.agent.signal) fail('session is already running')
  const installed = proposalToolsBySession.get(session) ?? []
  if (!Array.isArray(session.agent.state.tools) || session.agent.state.tools.length !== installed.length
      || session.agent.state.tools.some((tool, index) => tool !== installed[index])) fail('replacement session must not retain executable local tools')
  const settings = session.settingsManager
  if (settings?.getRetryEnabled?.() !== false || settings?.getRetrySettings?.().maxRetries !== 0
      || settings?.getProviderRetrySettings?.().maxRetries !== 0 || settings?.getCompactionEnabled?.() !== false) {
    fail('replacement session retains autonomous retry or compaction policy')
  }
  const key = JSON.stringify([request.runId, prepared.idempotencyKey])
  const claimed = deliveries.get(session) ?? new Set()
  if (claimed.has(key)) fail('batch delivery already claimed; reconcile instead of replaying')
  if (session.agent.state.messages?.length || session.agent.hasQueuedMessages?.()) fail('resume requires an empty replacement session')
  claimed.add(key)
  deliveries.set(session, claimed)
  let abortError
  let aborting
  let timedOut = false
  // Keep the cancellation request observed, but still await the engine settling:
  // a timeout must never make a live model request look safe to replay.
  const abort = () => {
    aborting ??= Promise.resolve().then(() => session.abort()).catch(error => { abortError = error })
  }
  signal.addEventListener('abort', abort, { once: true })
  const timer = setTimeout(() => { timedOut = true; abort() }, request.payload.controlBinding.budgets.modelRequestMs)
  let proposed
  const stopMarker = `fox-kernel-proposal-boundary:${request.runId}:${prepared.idempotencyKey}`
  let unsubscribe
  try {
  unsubscribe = typeof session.agent.subscribe === 'function' ? session.agent.subscribe(event => {
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
    if (timedOut) fail('frozen model request deadline exceeded')
    if (signal.aborted) fail('Host cancelled the resume')
    const final = session.agent.state.messages.at(-1)
    if (proposed) {
      if (final?.stopReason !== 'aborted' || final.errorMessage !== stopMarker) fail('proposal boundary did not stop the engine')
      return { idempotencyKey: prepared.idempotencyKey, checkpointSeq: prepared.checkpointSeq, response: proposed }
    }
    if (final?.role !== 'assistant' || final.stopReason !== 'stop') {
      fail(`model continuation has no completed response (${final?.stopReason ?? final?.role ?? 'missing'})`)
    }
    const response = prepareKernelModelResponse(final, prepared)
    return { idempotencyKey: prepared.idempotencyKey, checkpointSeq: prepared.checkpointSeq, response }
  } finally {
    clearTimeout(timer)
    unsubscribe?.()
    signal.removeEventListener('abort', abort)
  }
}
