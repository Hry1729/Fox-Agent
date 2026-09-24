// Live Kernel engine loop for one authoritative Run. The single-use worker
// process drives the public Pi agent loop; every engine round output is sent
// to the Host as a durable `kernel.round_output` request and only Host-settled
// results flow back as tool results. The engine never executes a local
// resource: an installed executor without a settled Host directive throws, and
// replacement sessions are refused when they retain executable tools.
import { RUNTIME_TOOL_CATALOG, validateWireValue } from '../../../packages/fox-engine-protocol/index.mjs'
import { finalizeKernelAnswer, completionPreview, KernelIncompleteResponseError } from './kernel-completion.mjs'
import { createRoundProgress, toolParamBytesOf } from './kernel-model-progress.mjs'
import { effectiveBoundableTool, modelToolResultContent, toolResultRef } from './tool-view.mjs'
import { steeringNoticeText, validateSteeringNotices } from './steering-notice.mjs'
import { hostJobNoticeMessage, validateHostJobNotices } from './host-job-notice.mjs'
import { describeKernelError, diagnosticLine } from './pi-kernel-diagnostics.mjs'

export { steeringNoticeText }

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
function settledToolResult(item, runId = null) {
  if (!record(item) || !nonempty(item.toolCallId) || !nonempty(item.tool)
    || !knownTools.has(item.tool) || !['completed', 'failed'].includes(item.state)
    || !record(item.result) || !Array.isArray(item.result.content)) {
    fail('invalid settled tool result directive')
  }
  const isError = item.state === 'failed'
  // Model-view bounding only: the durable item.result stays complete. Errors
  // and receipt-bearing results always pass through unchanged. The reference is
  // what makes an omitted byte recoverable through `read_tool_result`, so it is
  // attached on every settled result, not only on the batch path.
  // The Host dispatches every built-in Office operation through the
  // `call_mcp_tool` wrapper, so boundability is judged on the operation that
  // actually ran, not the wrapper name.
  const content = modelToolResultContent(effectiveBoundableTool(item.tool, item.canonicalInput), {
    isError,
    content: item.result.content,
    details: item.result.details,
    storage: item.storage,
    resultRef: toolResultRef(runId, item.toolCallId),
  })
  return {
    content,
    details: {},
    isError,
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

// Exact names of Host-native tools the live Host scheduler may fan out as
// independent read-only calls (see parallel_read_only_dispatch in
// runtime_host/kernel_coordinator/live.rs). The flag below only unblocks
// engine-side parallel invocation: the Host stays the sole authority on
// independence, read-only-ness and conflict safety, it owns per-call leases
// and source-order settlement, and writers/unknowns remain serial barriers.
// Generic MCP tools are NEVER assumed read-only, so no mcp__* name appears
// here; proposal-only schemas stay sequential as well.
const HOST_PARALLEL_READ_ONLY_TOOLS = Object.freeze(new Set(['read', 'ls', 'find', 'grep', 'skill_load']))

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
    parameters: PERMISSIVE_TOOL_PARAMETERS,
    executionMode: HOST_PARALLEL_READ_ONLY_TOOLS.has(name) ? 'parallel' : 'sequential',
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
  return validateKernelToolCatalog(definitions, knownTools)
}

// Pure validation over a supplied registry. Production always binds this to the
// protocol catalog above; registry size is independent of per-batch execution.
export function validateKernelToolCatalog(definitions, registeredNames) {
  if (!Array.isArray(definitions)) fail('invalid Host tool definitions')
  const names = new Set()
  const strictParameters = new Map()
  for (const definition of definitions) {
    if (!registeredNames.has(definition?.name) || names.has(definition.name) || !nonempty(definition.description)
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

  // Mid-run user requests handed over in round directives. They are ordinary
  // user text: they carry no tools, grants or authority. They are spliced into
  // the transcript only when the NEXT provider request starts — i.e. after the
  // settled tool results (batch) or the Host review prompt (continuation) — so
  // the durable ordering the Host records matches the engine transcript.
  let pendingSteering = []
  let pendingHostNotices = []
  // Ids already spliced into THIS session: a directive replay (or the same row
  // riding a retried dispatch) must never duplicate the user message locally.
  // Durable exactly-once application stays Host-side; this is transcript-side.
  const seenSteeringIds = new Set()
  const seenHostJobIds = new Set(prepared.hostJobNoticeIds ?? [])
  const steeringMessage = notice => ({
    role: 'user',
    content: [{ type: 'text', text: steeringNoticeText(notice.content) }],
    timestamp: notice.receivedAt ?? Date.now(),
  })
  const queueSteering = notices => {
    for (const notice of validateSteeringNotices(notices, fail)) {
      if (seenSteeringIds.has(notice.messageId)) continue
      seenSteeringIds.add(notice.messageId)
      pendingSteering.push(steeringMessage(notice))
    }
  }
  const queueHostJobNotices = notices => {
    for (const notice of validateHostJobNotices(notices)) {
      if (seenHostJobIds.has(notice.jobId)) continue
      seenHostJobIds.add(notice.jobId)
      pendingHostNotices.push(hostJobNoticeMessage(notice))
    }
  }
  const projectedStream = session.agent.streamFunction
  session.agent.streamFunction = (model, context, options) => {
    if (pendingSteering.length || pendingHostNotices.length) {
      const additions = [...pendingSteering.splice(0), ...pendingHostNotices.splice(0)]
      const transcript = session.agent.state.messages
      if (!Array.isArray(transcript)) fail('engine transcript unavailable for steering')
      for (const message of additions) {
        transcript.push(message)
        if (Array.isArray(context?.messages) && context.messages !== transcript) context.messages.push(message)
      }
    }
    return projectedStream.call(session.agent, model, context, options)
  }

  let roundCursor = prepared.checkpointSeq
  // Preview-attribution cursor for the current round. Tool/initial rounds use
  // the durable checkpoint cursor; a continuation round uses the Host-provided
  // previewSeq so its streamed message never reuses the previous round's id.
  let previewCursor = prepared.checkpointSeq
  let pendingBatch = null
  let fatal = null
  let timedOut = false
  let timeoutKind = null
  let abortError = null
  let previewRevision = 0
  let lastPreviewAt = 0
  let lastPreviewText = ''
  let lastPreviewReasoning = ''
  let lastPreviewToolBytes = 0
  let lastProgressPreviewAt = 0
  // First-response / idle / whole-round bounds from the frozen Host binding.
  // Text, thinking, AND tool-parameter bytes all count as progress, so long
  // Office JSON generated as tool arguments never looks like a stalled stream.
  // The Host controller enforces the same semantics from its own anchors; this
  // worker-side enforcement stops the provider call promptly and reports which
  // bound fired with byte-level telemetry.
  const roundBudgets = request.payload?.controlBinding?.budgets
  let roundProgress = createRoundProgress({ budgets: roundBudgets })
  let roundTimer = null
  const armRoundTimer = () => {
    clearRoundTimer()
    // Each continuation round is a fresh provider request with fresh bounds.
    roundProgress = createRoundProgress({ budgets: roundBudgets })
    roundTimer = setInterval(() => {
      if (signal.aborted || timedOut) return
      const breach = roundProgress.check()
      if (breach === 'ok') return
      timeoutKind = breach
      timedOut = true
      session.agent.abort()
    }, 250)
    roundTimer.unref?.()
  }
  const clearRoundTimer = () => { if (roundTimer) { clearInterval(roundTimer); roundTimer = null } }
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
    // Progress is tracked independently of display previews: the idle bound
    // must see tool-parameter bytes even when the Host disabled streaming.
    if (!signal.aborted && !timedOut && ['message_update', 'message_end'].includes(event.type)
        && event.message?.role === 'assistant' && Array.isArray(event.message.content)) {
      const text = completionPreview(event.message.content.filter(block => block?.type === 'text' && typeof block.text === 'string')
        .map(block => block.text).join(''), requireCompletion)
      const reasoning = event.message.content.filter(block => block?.type === 'thinking' && typeof block.thinking === 'string')
        .map(block => block.thinking).join('\n\n')
      const toolBytes = toolParamBytesOf(event.message.content)
      roundProgress.note({ text, reasoning, toolParamBytes: toolBytes })
      if (preview) {
        const now = performance.now()
        const toolGrowth = toolBytes - lastPreviewToolBytes
        // Tool-parameter-only progress never appears in text/reasoning, but it
        // must still reach the Host: it drives the controller's idle bound and
        // lets operators tell slow generation apart from a stalled stream.
        // Throttled harder than text previews (1s / 16KiB) to avoid preview spam
        // during long argument generation.
        const toolPreviewDue = toolGrowth !== 0 && (toolBytes === 0 || toolGrowth >= 16384 || now - lastProgressPreviewAt >= 1000)
        if (((text !== lastPreviewText || reasoning !== lastPreviewReasoning) || toolPreviewDue)
            && Buffer.byteLength(text, 'utf8') <= 262_144 && Buffer.byteLength(reasoning, 'utf8') <= 262_144
            && (previewRevision === 0 || now - lastPreviewAt >= 100 || event.type === 'message_end')) {
          lastPreviewText = text
          lastPreviewReasoning = reasoning
          lastPreviewToolBytes = toolBytes
          lastPreviewAt = now
          if (toolPreviewDue) lastProgressPreviewAt = now
          preview({ schemaVersion: 1, runId: request.runId, conversationId: request.conversationId, turnId: prepared.turnId,
            checkpointSeq: previewCursor, revision: ++previewRevision, text, ...(reasoning ? { reasoning } : {}),
            progressBytes: roundProgress.totalBytes() })
        }
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
      for (const item of directive.tools) settled.set(item.toolCallId, settledToolResult(item, request.runId))
      pendingBatch = { batchId: directive.batchId, settled }
      hooks?.onSettled?.(settled)
      // Appended to the transcript when the next model request starts, after
      // the settled tool results — never into the frozen proposal round.
      queueSteering(directive.steering)
      queueHostJobNotices(directive.hostJobNotices)
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
      if (timedOut) {
        // Categorized timeout evidence with byte-level telemetry: the Host
        // admits a turn-budget retry only when the failed round dispatched no
        // new tool, so no execution can be replayed. Telemetry carries sizes
        // only, never content or credentials. The evidence object must match
        // the KernelModelFailure protocol shape exactly (deny_unknown_fields);
        // the bound kind travels in the message string for logs only.
        const kind = timeoutKind && timeoutKind !== 'ok' ? timeoutKind : roundProgress.check()
        const timeout = new Error(`Kernel model round exceeded its frozen time budget (${kind})`)
        timeout.evidence = {
          schemaVersion: 1, runId: request.runId, turnId: prepared.turnId,
          checkpointSeq: roundCursor, category: 'model_timeout',
          httpStatus: null, retryAfterMs: null,
          telemetry: roundProgress.telemetry(),
        }
        throw timeout
      }
      if (signal.aborted) fail('Host cancelled the run')
      final = session.agent.state.messages.at(-1)
      if (final?.role !== 'assistant' || final.stopReason !== 'stop') {
        // Diagnosing a real-chain failure needs the *shape* of the failed round:
        // role, stop reason, block kinds and whether a provider request was even
        // attempted. Only a fixed field list is published, and the one field
        // whose text the provider writes (the fetch error) goes through the
        // capped, redacted shape in pi-kernel-diagnostics.mjs — never a raw
        // message, stack or argument. Diagnosis switch only.
        if (process.env.FOX_KERNEL_WORKER_DEBUG) {
          const structure = {
            role: final?.role ?? null,
            stopReason: final?.stopReason ?? null,
            blockKinds: (final?.content ?? []).map(block => block?.type ?? null),
            blocks: (final?.content ?? []).length,
            fetchCalls: hooks?.fetchCalls?.() ?? null,
            hasRejection: Boolean(hooks?.lastRejection?.()),
            fetchError: describeKernelError(hooks?.lastFetchError?.()),
            roundCursor,
          }
          console.error(diagnosticLine('kernel-worker unclassified round end', structure))
        }
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
        if (Array.isArray(directive.steering) && directive.steering.length) {
          fail('Host carried steering into a final directive')
        }
        if (Array.isArray(directive.hostJobNotices) && directive.hostJobNotices.length) {
          fail('Host carried a job notice into a final directive')
        }
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
        roundCursor = previewCursor
        session.agent.state.messages = [...session.agent.state.messages,
          { role: 'user', content: [{ type: 'text', text: directive.prompt }], timestamp: 0 }]
        // Spliced after the review prompt at the next provider request.
        queueSteering(directive.steering)
        queueHostJobNotices(directive.hostJobNotices)
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
    // The evidence object must always satisfy the frozen protocol: a null
    // category is rejected by the Host ("invalid type: null, expected a string"),
    // which turns a real transport failure into an unreadable one and hides the
    // cause. A round that ended without a successful stop is a transport failure
    // by definition, so that is the category of last resort.
    const resolved = typeof category === 'string' && category ? category : 'model_transport_failure'
    const error = new Error('Kernel model round failed; the Host owns retry admission')
    error.evidence = {
      schemaVersion: 1, runId: request.runId, turnId: prepared.turnId,
      checkpointSeq: roundCursor, category: resolved,
      httpStatus: resolved === 'provider_unavailable' ? rejection?.httpStatus ?? null : null,
      retryAfterMs: resolved === 'provider_unavailable' ? rejection?.retryAfterMs ?? null : null,
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
