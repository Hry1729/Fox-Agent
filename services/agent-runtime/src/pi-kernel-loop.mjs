// Live Kernel engine loop for one authoritative Run. The single-use worker
// process drives the public Pi agent loop; every engine round output is sent
// to the Host as a durable `kernel.round_output` request and only Host-settled
// results flow back as tool results. The engine never executes a local
// resource: an installed executor without a settled Host directive throws, and
// replacement sessions are refused when they retain executable tools.
import { RUNTIME_TOOL_CATALOG, validateWireValue } from '../../../packages/fox-engine-protocol/index.mjs'
import { finalizeKernelAnswer, completionPreview, KernelIncompleteResponseError } from './kernel-completion.mjs'

const knownTools = new Set(RUNTIME_TOOL_CATALOG.map(tool => tool.name))
const fail = message => { throw new Error(`Invalid Kernel engine loop: ${message}`) }
const record = value => value !== null && typeof value === 'object' && !Array.isArray(value)
const nonempty = value => typeof value === 'string' && value.trim().length > 0

export const KERNEL_ROUND_FATAL = 'fox-kernel-round-fatal'

function canonical(value, omitUndefined = false) {
  if (value === null || typeof value === 'string' || typeof value === 'boolean') return value
  if (typeof value === 'number' && Number.isFinite(value)) return value
  if (Array.isArray(value)) return value.map(item => canonical(item, omitUndefined))
  if (record(value)) return Object.fromEntries(Object.keys(value).sort()
    .filter(key => !omitUndefined || value[key] !== undefined).map(key => [key, canonical(value[key], omitUndefined)]))
  fail('non-JSON value')
}

/** Canonicalize and validate one engine round output before it leaves the worker. */
export function prepareRoundOutput(assistantMessage, { turnId, requireCompletion = false } = {}) {
  for (const block of assistantMessage?.content ?? []) {
    if (block?.type === 'toolCall') canonical(block.arguments)
  }
  let message = canonical(assistantMessage, true)
  message = finalizeKernelAnswer(message, requireCompletion)
  const frame = { schemaVersion: 1, turnId, assistantMessage: message }
  canonical(frame)
  if (Buffer.byteLength(JSON.stringify(frame), 'utf8') > 1_048_576) fail('round output exceeds the protocol size limit')
  const errors = validateWireValue('KernelRoundOutputFrame', frame)
  if (errors.length) fail(errors.join('; '))
  return frame
}

/** Durable settled result projected into the Pi tool-result shape. */
function settledToolResult(item) {
  if (!record(item) || !nonempty(item.toolCallId) || !nonempty(item.tool)
    || !knownTools.has(item.tool) || !['completed', 'failed'].includes(item.state)
    || !record(item.result) || !Array.isArray(item.result.content)) {
    fail('invalid settled tool result directive')
  }
  return {
    content: item.result.content,
    details: {},
    isError: item.state === 'failed',
  }
}

/**
 * Install schema-faithful tools whose executors only return Host-settled
 * durable results. Sequential execution keeps each proposal's settlement
 * ordered and every result inside the same durable batch.
 *
 * The engine-side tools carry a deliberately permissive argument schema so the
 * public loop never decides proposal validity itself: a malformed call crosses
 * the Host boundary as a proposal, and the model receives the Host's durable,
 * authoritative failure result. The strict Host schema is still published to
 * the model via a stream-function projection (see installHostSchemaProjection)
 * so guided generation and provider-side tool typing stay exact.
 */
const PERMISSIVE_TOOL_PARAMETERS = Object.freeze({ type: 'object' })

export function installKernelHostTools(session, definitions, { settledFor } = {}) {
  if (session?.isIdle !== true || !Array.isArray(session.agent?.state?.tools)
      || session.agent.state.tools.length || typeof session.agent.subscribe !== 'function'
      || typeof session.agent.abort !== 'function' || typeof settledFor !== 'function') {
    fail('Host tools require an empty idle public Pi adapter and a settlement source')
  }
  if (session.agent.state.tools.length) fail('Host tools require an empty idle tool set')
  const strict = kernelToolDefinitions(definitions)
  const strictParameters = new Map([...strict].map(([name, value]) => [name, value.parameters]))
  const tools = [...strict.entries()].map(([name, { description }]) => Object.freeze({
    name, label: name, description,
    parameters: PERMISSIVE_TOOL_PARAMETERS, executionMode: 'sequential',
    execute: async toolCallId => {
      const settled = settledFor(toolCallId)
      if (!settled) fail(`tool ${name} has no settled Host result; the round output was not committed`)
      return settled
    },
  }))
  session.agent.state.tools = tools
  installedToolsBySession.set(session, tools)
  installHostSchemaProjection(session, strictParameters)
}

/**
 * Validate the Host tool definitions.
 */
export function kernelToolDefinitions(definitions) {
  if (!Array.isArray(definitions) || definitions.length > 64) fail('invalid Host tool definitions')
  const names = new Set()
  const strictParameters = new Map()
  for (const definition of definitions) {
    if (!knownTools.has(definition?.name) || names.has(definition.name) || !nonempty(definition.description)
        || !record(definition.parameters) || definition.parameters.type !== 'object') {
      fail('invalid Host tool schema')
    }
    names.add(definition.name)
    const parameters = canonical(definition.parameters)
    if (Buffer.byteLength(JSON.stringify(parameters), 'utf8') > 131_072) fail('tool schema is too large')
    strictParameters.set(definition.name, { parameters, description: definition.description })
  }
  return strictParameters
}

/**
 * Publish strict Host schemas with no executor for a single-round delivery:
 * the model sees the exact schema and a produced proposal is returned as the
 * round output for the Host to approve, never executed in this process.
 */
export function installKernelProposalSchemas(session, definitions, { requireEmpty = false } = {}) {
  if (session?.isIdle !== true || !Array.isArray(session.agent?.state?.tools)
      || typeof session.agent.subscribe !== 'function' || typeof session.agent.abort !== 'function') {
    fail('Host tool schemas require an idle public Pi adapter')
  }
  if (requireEmpty && session.agent.state.tools.length) fail('session already advertises tools')
  const strict = kernelToolDefinitions(definitions)
  const tools = [...strict.entries()].map(([name, { parameters, description }]) => Object.freeze({
    name, label: name, description, parameters, executionMode: 'sequential',
    // The proposal boundary aborts the run before preparation; an executor
    // must never run in a single-round delivery process.
    execute: async () => fail('proposal-only engine has no resource executor'),
  }))
  session.agent.state.tools = tools
  return { strictParameters: new Map([...strict].map(([name, value]) => [name, value.parameters])) }
}

const installedToolsBySession = new WeakMap()

/**
 * Publish the strict Host schema at the provider boundary while the live loop
 * keeps permissive engine-side tools. Every provider request for this session
 * sees the exact Host schema; loop-internal validation keeps permissive
 * parameters.
 */
function installHostSchemaProjection(session, strictParameters) {
  const innerStream = session.agent.streamFunction
  session.agent.streamFunction = (model, context, options) => {
    const tools = context?.tools
    if (!Array.isArray(tools) || !tools.length || ![...strictParameters.keys()].some(name => tools.some(tool => tool?.name === name))) {
      return innerStream(model, context, options)
    }
    const projectedTools = tools.map(tool => strictParameters.has(tool?.name)
      ? { ...tool, parameters: strictParameters.get(tool.name) }
      : tool)
    return innerStream(model, { ...context, tools: projectedTools }, options)
  }
}

/**
 * Drive the public Pi loop for the whole Run. Seeding history and settled
 * results come from the Host; the loop ends only on a Host-confirmed final
 * answer, a Host decision, a model failure, a timeout or cancellation.
 */
export async function runPiKernelLoop(session, request, prepared, hooks) {
  const { signal, preview, requestHost, requireCompletion = false } = hooks ?? {}
  if (!session?.agent?.state || typeof session.agent.continue !== 'function' || typeof session.abort !== 'function') {
    fail('missing public Pi session adapter')
  }
  if (!signal || typeof signal.addEventListener !== 'function' || typeof requestHost !== 'function') {
    fail('missing Host cancellation signal or request channel')
  }
  if (signal.aborted) fail('Host cancelled the run')
  if (session.isIdle !== true || session.state?.isStreaming || session.agent.signal) fail('session is already running')
  const installed = installedToolsBySession.get(session)
  if (!Array.isArray(installed) || !Array.isArray(session.agent.state.tools)
      || session.agent.state.tools.length !== installed.length
      || session.agent.state.tools.some((tool, index) => tool !== installed[index])) {
    fail('replacement session must not retain executable local tools')
  }
  const settings = session.settingsManager
  if (settings?.getRetryEnabled?.() !== false || settings?.getRetrySettings?.().maxRetries !== 0
      || settings?.getProviderRetrySettings?.().maxRetries !== 0 || settings?.getCompactionEnabled?.() !== false) {
    fail('replacement session retains autonomous retry or compaction policy')
  }
  if (!nonempty(prepared?.turnId) || !Array.isArray(prepared.messages)) fail('invalid prepared round input')
  const deliveries = deliveriesBySession.get(session) ?? new Set()
  const claimKey = JSON.stringify([request.runId, prepared.idempotencyKey ?? 'initial'])
  if (deliveries.has(claimKey)) fail('delivery already claimed; reconcile instead of replaying')
  if (session.agent.state.messages?.length || session.agent.hasQueuedMessages?.()) {
    fail('the loop requires an empty replacement session')
  }
  deliveries.add(claimKey)
  deliveriesBySession.set(session, deliveries)

  let roundCursor = prepared.checkpointSeq
  // Preview-attribution cursor for the current round. Tool/initial rounds use
  // the durable checkpoint cursor; a continuation round uses the Host-provided
  // previewSeq so its streamed message never reuses the previous round's id.
  let previewCursor = prepared.checkpointSeq
  let pendingBatch = null
  let fatal = null
  let timedOut = false
  let abortError = null
  let previewRevision = 0
  let lastPreviewAt = 0
  let lastPreviewText = ''
  let lastPreviewReasoning = ''
  let roundTimer = null
  const armRoundTimer = () => {
    clearRoundTimer()
    const budget = Number(request.payload?.controlBinding?.budgets?.modelRequestMs)
    if (!Number.isFinite(budget) || budget <= 0) return
    roundTimer = setTimeout(() => { timedOut = true; session.agent.abort() }, budget)
  }
  const clearRoundTimer = () => { if (roundTimer) { clearTimeout(roundTimer); roundTimer = null } }
  // Each model round owns an independent display message identity (cursor),
  // while the revision cursor stays run-monotonic: the Host orders previews by
  // a single increasing revision and de-duplicates the visible row by message
  // id, so resetting the revision here would be rejected as an out-of-order
  // preview on later rounds.
  const resetPreviewRound = cursor => {
    previewCursor = Number.isFinite(cursor) && cursor > 0 ? cursor : previewCursor
    lastPreviewAt = 0
    lastPreviewText = ''
    lastPreviewReasoning = ''
  }
  const abort = () => { try { session.agent.abort() } catch (error) { abortError ??= error } }
  signal.addEventListener('abort', abort, { once: true })

  const unsubscribe = session.agent.subscribe(async event => {
    if (preview && !signal.aborted && !timedOut && ['message_update', 'message_end'].includes(event.type)
        && event.message?.role === 'assistant' && Array.isArray(event.message.content)) {
      const text = completionPreview(event.message.content.filter(block => block?.type === 'text' && typeof block.text === 'string')
        .map(block => block.text).join(''), requireCompletion)
      const reasoning = event.message.content.filter(block => block?.type === 'thinking' && typeof block.thinking === 'string')
        .map(block => block.thinking).join('\n\n')
      const now = performance.now()
      if ((text !== lastPreviewText || reasoning !== lastPreviewReasoning)
          && Buffer.byteLength(text, 'utf8') <= 262_144 && Buffer.byteLength(reasoning, 'utf8') <= 262_144
          && (previewRevision === 0 || now - lastPreviewAt >= 100 || event.type === 'message_end')) {
        lastPreviewText = text
        lastPreviewReasoning = reasoning
        lastPreviewAt = now
        preview({ schemaVersion: 1, runId: request.runId, conversationId: request.conversationId, turnId: prepared.turnId,
          checkpointSeq: previewCursor, revision: ++previewRevision, text, ...(reasoning ? { reasoning } : {}) })
      }
    }
    if (event.type !== 'message_end' || event.message?.role !== 'assistant') return
    // The streamed round finished; the per-request deadline stops here so the
    // Host round-trip and any approval/execution wait are never charged to it.
    clearRoundTimer()
    if (signal.aborted || timedOut) return
    const message = event.message
    const hasToolCalls = Array.isArray(message.content) && message.content.some(block => block?.type === 'toolCall')
    if (!hasToolCalls) return
    // Commit the proposal durably before the engine can execute anything.
    // Blocking inside this awaited listener stops the engine at the boundary.
    try {
      if (message.stopReason !== 'toolUse') fail('engine produced a truncated tool proposal')
      const output = prepareRoundOutput(message, { turnId: prepared.turnId, requireCompletion })
      pendingBatch = null
      hooks?.onSettled?.(null)
      const directive = await requestHost('kernel.round_output', output)
      if (directive?.schemaVersion !== 1 || directive?.kind !== 'batch' || !nonempty(directive.batchId)
          || !Array.isArray(directive.tools) || !directive.tools.length) {
        fail('Host returned a non-batch directive for a tool proposal')
      }
      roundCursor = directive.checkpointSeq
      resetPreviewRound(directive.checkpointSeq)
      const settled = new Map()
      for (const item of directive.tools) settled.set(item.toolCallId, settledToolResult(item))
      pendingBatch = { batchId: directive.batchId, settled }
      hooks?.onSettled?.(settled)
    } catch (error) {
      pendingBatch = null
      fatal ??= error
      session.agent.abort()
      throw error
    }
  })

  try {
    session.agent.state.messages = prepared.messages
    let final
    while (true) {
      // Arm the per-round deadline before the provider request is issued, not
      // on the first streamed assistant event: the time spent sending the
      // request and waiting for response headers is part of the model round
      // and must count against the frozen per-request budget. It is disarmed
      // when the committed round output leaves for the Host (tool proposal
      // listener) or once the stop answer settles below, so Host/approval waits
      // are never charged to this timer.
      armRoundTimer()
      await session.agent.continue()
      if (pendingBatch === 'awaiting') fail('batch settlement did not finish before the engine settled')
      if (abortError) fail(`engine cancellation failed: ${abortError.message ?? String(abortError)}`)
      if (fatal) throw fatal
      if (timedOut) fail('frozen model round deadline exceeded')
      if (signal.aborted) fail('Host cancelled the run')
      final = session.agent.state.messages.at(-1)
      if (final?.role !== 'assistant' || final.stopReason !== 'stop') {
        throw modelFailureError(transportFailureCategory(final), hooks?.lastRejection?.())
      }
      let output
      try {
        output = prepareRoundOutput(final, { turnId: prepared.turnId, requireCompletion })
      } catch (error) {
        if (error instanceof KernelIncompleteResponseError) throw modelFailureError('incomplete_response')
        throw error
      }
      const directive = await requestHost('kernel.round_output', output)
      if (directive?.schemaVersion !== 1) fail('Host directive is not a valid round directive')
      if (directive.kind === 'final') {
        return {
          idempotencyKey: prepared.idempotencyKey ?? 'initial-model-delivery',
          checkpointSeq: roundCursor,
          response: {
            schemaVersion: 1, runId: request.runId, turnId: prepared.turnId,
            ...(prepared.batchId ? { batchId: prepared.batchId } : {}),
            // Project the Host-validated frame message, not the raw transcript
            // entry: for a frozen completion contract this is the answer after
            // its internal marker was stripped.
            checkpointSeq: roundCursor, assistantMessage: output.assistantMessage,
          },
        }
      }
      if (directive.kind === 'continuation' && nonempty(directive.prompt)) {
        // Review round owns an independent preview identity so its streamed
        // answer is attributed to the Host-provided cursor, never the prior
        // tool round's message id.
        resetPreviewRound(directive.previewSeq)
        session.agent.state.messages = [...session.agent.state.messages,
          { role: 'user', content: [{ type: 'text', text: directive.prompt }], timestamp: Date.now() }]
        continue
      }
      fail('unexpected Host directive for a completed round')
    }
  } finally {
    clearRoundTimer()
    unsubscribe?.()
    signal.removeEventListener('abort', abort)
  }

  function modelFailureError(category, rejection = null) {
    const error = new Error('Kernel model round failed; the Host owns retry admission')
    error.evidence = {
      schemaVersion: 1, runId: request.runId, turnId: prepared.turnId,
      checkpointSeq: roundCursor, category,
      httpStatus: category === 'provider_unavailable' ? rejection?.httpStatus ?? null : null,
      retryAfterMs: category === 'provider_unavailable' ? rejection?.retryAfterMs ?? null : null,
    }
    return error
  }

  function transportFailureCategory(final) {
    if (final?.role === 'assistant' && final.stopReason === 'length'
        && !(final.content ?? []).some(block => block?.type === 'toolCall')) return 'incomplete_response'
    const rejection = hooks?.lastRejection?.()
    if (final?.role === 'assistant' && final.stopReason === 'error'
        && !(final.content ?? []).some(block => block?.type === 'toolCall' || block?.text?.length || block?.thinking?.length)
        && rejection) return 'provider_unavailable'
    return null
  }
}

const deliveriesBySession = new WeakMap()
