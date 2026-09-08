import { createInterface } from 'node:readline'
import {
  DefaultResourceLoader,
  PI_PACKAGE_VERSION,
  SessionManager,
  SettingsManager,
  createFoxAgentSession,
  createFoxModelRuntime,
  fauxAssistantMessage,
  registerFauxProvider,
} from './pi-adapter.mjs'
import { createEnvelope, PROTOCOL_NAME, PROTOCOL_VERSION, validateEnvelope } from './protocol.mjs'
import { createPiEventMapper, sanitizeAssistantHistory } from './pi-event-mapper.mjs'
import { createReadOnlyTools } from './read-only-tools.mjs'
import { createGraphReadonlyTools } from './graph-readonly-tools.mjs'
import { createHostTools, createKnowledgeTools, createMcpTools } from './host-tools.mjs'
import { createSessionState, loadSessionState, normalizeHistory, sanitizeProviderHistory, saveSessionState, transcriptFromSession } from './runtime-session.mjs'
import { RUNTIME_TOOL_CATALOG, assertRegisteredToolsMatchCatalog, createCapabilityManifest } from './runtime-contract.mjs'
import { createFoxPlanningExtension } from './fox-planning-extension.mjs'
import {
  APPROVAL_DEMO_NO_TOOL_MESSAGE,
  composeRuntimePrompt,
  isApprovalDemoFollowup,
  isApprovalDemoRequest,
  replaceLatestAssistantText,
} from './runtime-instructions.mjs'
import { stablePromptHash } from './prompt-composer.mjs'
import { plannerHandoff, runPlanner, shouldUsePlanner } from './planner-runtime.mjs'
import { diagnoseToolsForAgentContext, selectToolsForProjectContext } from './expert-package.mjs'
import { adaptFoxToolsToPi } from './tool-adapter.mjs'
import { runOfflineEvals } from './offline-evaluator.mjs'
import {
  modelProfilePrompt,
  modelProfileSnapshot,
  resolveModelProfile,
  transportProvider,
} from './model-profile.mjs'
import {
  applyExecutionProfileToTools,
  executionProfileCatalogTools,
  executionProfilePrompt,
  executionProfileSnapshot,
  resolveExecutionProfile,
} from './execution-profile.mjs'
import {
  CONTINUATION_PROPOSAL_EVENT_TYPE,
  CONTINUATION_PROPOSAL_TOOL_NAME,
  continuationDecisionContract,
  createContinuationDecisionTools,
} from './continuation-decision.mjs'

const sessions = new Map()
const activeRuns = new Map()
const pendingHostRequests = new Map()
const RUNTIME_HTTP_IDLE_TIMEOUT_MS = 90_000
const HOST_REQUEST_TIMEOUT_MS = 6 * 60_000
const HARD_RUN_BUDGET = Object.freeze({
  maxDurationMs: 30 * 60_000,
  maxIdenticalToolCalls: 4,
  maxWebSearchCalls: 6,
})
let modelService = null
let fauxProvider = null
let executionProfile = resolveExecutionProfile('legacy')

function configuredFauxResponses(config) {
  if (!Array.isArray(config.fauxResponses) || config.fauxResponses.length === 0) {
    return [fauxAssistantMessage('Fox Pi Runtime connected.')]
  }
  return config.fauxResponses.map((response) => {
    if (typeof response === 'string') return fauxAssistantMessage(response)
    return fauxAssistantMessage(response?.content ?? '', {
      stopReason: response?.stopReason ?? 'stop',
      errorMessage: response?.errorMessage,
    })
  })
}

function write(message) {
  process.stdout.write(`${JSON.stringify(message)}\n`)
}

function respond(request, type, payload = {}) {
  write(createEnvelope('response', type, {
    requestId: request.id,
    conversationId: request.conversationId ?? null,
    runtimeSessionId: request.runtimeSessionId ?? null,
    runId: request.runId ?? null,
    payload,
  }))
}

function runtimeEvent(request, seqRef, type, payload = {}) {
  write(createEnvelope('event', 'runtime_event', {
    conversationId: request.conversationId,
    runtimeSessionId: request.runtimeSessionId,
    runId: request.runId,
    seq: seqRef.value++,
    payload: { type, ...payload },
  }))
}

function requestHost(request, type, payload, signal) {
  const message = createEnvelope('request', type, {
    conversationId: request.conversationId,
    runtimeSessionId: request.runtimeSessionId,
    runId: request.runId,
    payload,
  })
  if (signal?.aborted) {
    const error = new Error('Host request was cancelled before dispatch.')
    error.code = 'runtime.host_request_cancelled'
    return Promise.reject(error)
  }
  return new Promise((resolve, reject) => {
    let settled = false
    const cleanup = () => {
      clearTimeout(timeout)
      signal?.removeEventListener('abort', abort)
      pendingHostRequests.delete(message.id)
    }
    const settle = (handler, value) => {
      if (settled) return
      settled = true
      cleanup()
      handler(value)
    }
    const abort = () => {
      const error = new Error('Host request was cancelled.')
      error.code = 'runtime.host_request_cancelled'
      settle(reject, error)
    }
    const timeout = setTimeout(() => {
      const error = new Error(`Host request ${type} timed out after ${HOST_REQUEST_TIMEOUT_MS}ms.`)
      error.code = 'runtime.host_request_timeout'
      settle(reject, error)
    }, HOST_REQUEST_TIMEOUT_MS)
    signal?.addEventListener('abort', abort, { once: true })
    pendingHostRequests.set(message.id, {
      runId: request.runId,
      resolve: (value) => settle(resolve, value),
      reject: (error) => settle(reject, error),
    })
    // Close the narrow race between the pre-dispatch check and listener setup.
    if (signal?.aborted) {
      abort()
      return
    }
    write(message)
  })
}

// Reject only the Host requests that belong to a single run. Cancellation must
// never tear down in-flight requests owned by other concurrent runs (for example
// the A5 child-runtime pool), so this is run-scoped rather than a global clear.
function cancelPendingHostRequests(runId) {
  for (const [id, pending] of pendingHostRequests.entries()) {
    if (runId === undefined || pending.runId === runId) {
      pendingHostRequests.delete(id)
      pending.reject(new Error('Host request was cancelled.'))
    }
  }
}

function createModel(config, profile) {
  if (config.apiType === 'faux') return fauxProvider.getModel(config.modelId) || fauxProvider.getModel()
  return {
    id: config.modelId,
    name: config.modelId,
    api: profile.api,
    provider: transportProvider(profile),
    baseUrl: config.baseUrl,
    reasoning: profile.reasoning,
    input: profile.supportsImageInput ? ['text', 'image'] : ['text'],
    cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
    contextWindow: profile.contextWindow,
    maxTokens: profile.maxOutputTokens,
    ...(Object.keys(profile.compat).length > 0 ? { compat: profile.compat } : {}),
  }
}

function currentSession(request) {
  const session = sessions.get(request.runtimeSessionId)
  if (!session) throw new Error('Runtime session is not open.')
  return session
}

function promptBudgetForRun(payload, modelProfile) {
  const configured = payload?.promptBudget && typeof payload.promptBudget === 'object'
    ? payload.promptBudget
    : {}
  const configuredRatio = Number(configured.charsPerToken)
  const charsPerToken = Number.isFinite(configuredRatio) && configuredRatio >= 1 && configuredRatio <= 16
    ? configuredRatio
    : 4
  const availableInputTokens = Math.max(
    512,
    modelProfile.contextWindow - modelProfile.maxOutputTokens - modelProfile.runtime.reserveTokens,
  )
  const modelMaxPromptChars = Math.floor(availableInputTokens * charsPerToken)
  const configuredChars = Number(configured.maxPromptChars)
  const configuredTokens = Number(configured.maxPromptTokens)
  const callerMaxPromptChars = Number.isFinite(configuredChars) && configuredChars > 0
    ? Math.floor(configuredChars)
    : Number.isFinite(configuredTokens) && configuredTokens > 0
      ? Math.floor(configuredTokens * charsPerToken)
      : modelMaxPromptChars
  return {
    maxPromptChars: Math.min(modelMaxPromptChars, callerMaxPromptChars),
    charsPerToken,
  }
}

function positiveBudgetValue(value, fallback, hardMaximum) {
  const parsed = Number(value)
  return Number.isFinite(parsed) && parsed > 0
    ? Math.min(Math.floor(parsed), hardMaximum)
    : fallback
}

function runBudgetForRequest(payload) {
  const configured = payload?.runBudget && typeof payload.runBudget === 'object'
    ? payload.runBudget
    : {}
  return {
    maxDurationMs: positiveBudgetValue(
      configured.maxDurationMs,
      HARD_RUN_BUDGET.maxDurationMs,
      HARD_RUN_BUDGET.maxDurationMs,
    ),
    maxTotalTokens: positiveBudgetValue(
      configured.maxTotalTokens,
      null,
      Number.MAX_SAFE_INTEGER,
    ),
    maxToolCalls: positiveBudgetValue(
      configured.maxToolCalls,
      null,
      Number.MAX_SAFE_INTEGER,
    ),
    maxIdenticalToolCalls: positiveBudgetValue(
      configured.maxIdenticalToolCalls,
      HARD_RUN_BUDGET.maxIdenticalToolCalls,
      HARD_RUN_BUDGET.maxIdenticalToolCalls,
    ),
  }
}

function boundedRetryInt(value, fallback, min, max) {
  const parsed = Number(value)
  return Number.isInteger(parsed) && parsed >= min && parsed <= max ? parsed : fallback
}

// Provider HTTP retry and whole-turn retry are separate policies. The sidecar
// defaults turn retry OFF (it is owned by the Fox Kernel/Host) so a rate-limited
// request is never retried by both Pi's provider layer and an outer turn retry,
// which would double cost and amplify 429s. The Host may supply an explicit,
// bounded retry policy per run; the model profile only supplies provider bounds.
function retryPolicyForRun(payload, modelProfile) {
  const host = payload?.retryPolicy && typeof payload.retryPolicy === 'object' ? payload.retryPolicy : {}
  const profileRuntime = modelProfile.runtime
  const providerMaxRetries = boundedRetryInt(
    host.providerMaxRetries,
    profileRuntime.providerMaxRetries,
    0,
    5,
  )
  const providerMaxRetryDelayMs = positiveBudgetValue(
    host.providerMaxRetryDelayMs,
    profileRuntime.providerMaxRetryDelayMs,
    60_000,
  )
  const turnMaxRetries = boundedRetryInt(
    host.turnMaxRetries,
    host.turnEnabled === true ? 1 : 0,
    0,
    5,
  )
  return {
    providerMaxRetries,
    providerMaxRetryDelayMs,
    turnEnabled: turnMaxRetries > 0,
    turnMaxRetries,
    turnBaseDelayMs: positiveBudgetValue(host.turnBaseDelayMs, 750, 30_000),
  }
}

function budgetedTools(tools, budget, failRun) {
  let totalCalls = 0
  let webSearchCalls = 0
  const identicalCalls = new Map()
  return tools.map((tool) => ({
    ...tool,
    execute: async (toolCallId, input, signal, ...rest) => {
      totalCalls += 1
      if (budget.maxToolCalls !== null && totalCalls > budget.maxToolCalls) {
        throw failRun(
          'runtime.tool_call_budget_exceeded',
          `Run exceeded its ${budget.maxToolCalls} tool-call budget.`,
        )
      }
      if (tool.name === 'web_search') {
        webSearchCalls += 1
        if (webSearchCalls > HARD_RUN_BUDGET.maxWebSearchCalls) {
          throw failRun(
            'runtime.web_search_budget_exceeded',
            `Run exceeded the hard limit of ${HARD_RUN_BUDGET.maxWebSearchCalls} web searches. Use the evidence already collected instead of retrying equivalent queries.`,
          )
        }
      }
      const fingerprint = `${tool.name}\0${JSON.stringify(input ?? {})}`
      const repeated = (identicalCalls.get(fingerprint) ?? 0) + 1
      identicalCalls.set(fingerprint, repeated)
      if (repeated > budget.maxIdenticalToolCalls) {
        throw failRun(
          'runtime.repeated_tool_call',
          `Tool ${tool.name} repeated the same input more than ${budget.maxIdenticalToolCalls} times.`,
        )
      }
      return tool.execute(toolCallId, input, signal, ...rest)
    },
  }))
}

async function executePrompt(request) {
  const activeExecutionProfile = executionProfile
  const activeExecutionProfileSnapshot = executionProfileSnapshot(activeExecutionProfile)
  const activeContinuationContract = continuationDecisionContract(activeExecutionProfile)
  const session = currentSession(request)
  const seq = { value: 1 }
  const fallbackMessages = normalizeHistory(request.payload?.messages)
  const currentText = String(request.payload?.text || '')
  const previousUserText = [...fallbackMessages]
    .reverse()
    .find((message) => message?.role === 'user' && message.content !== currentText)?.content
  const approvalDemo = isApprovalDemoRequest(currentText)
    || isApprovalDemoFollowup(currentText, previousUserText)
  const bufferedAssistantEvents = []
  let toolExecutionCount = 0
  const runBudget = runBudgetForRequest(request.payload)
  const runAbort = new AbortController()
  const runControl = {
    cancelled: false,
    agent: null,
    budgetError: null,
    abort: runAbort,
    mapper: null,
  }
  // Abort a Host request when either the run is cancelled or Pi's per-tool signal
  // fires. AbortSignal.any is available on the supported Node runtime.
  const runScopedSignal = (toolSignal) => {
    const signals = [runAbort.signal]
    if (toolSignal) signals.push(toolSignal)
    return signals.length === 1 ? signals[0] : AbortSignal.any(signals)
  }
  let mapper = null
  const failRun = (code, message) => {
    if (runControl.budgetError) return runControl.budgetError
    const error = new Error(message)
    error.code = code
    runControl.budgetError = error
    mapper?.fail(error)
    runControl.agent?.abort()
    return error
  }
  const emit = (type, payload = {}) => {
    if (runControl.cancelled
      && type !== 'usage.updated'
      && type !== 'run.completed'
      && type !== 'run.cancelled'
      && type !== 'run.interrupted'
      && type !== 'run.failed') return
    if (type === 'tool.started') toolExecutionCount += 1
    if (type === 'usage.updated' && runBudget.maxTotalTokens !== null && Number(payload.totalTokens) > runBudget.maxTotalTokens) {
      runtimeEvent(request, seq, type, payload)
      failRun(
        'runtime.token_budget_exceeded',
        `Run exceeded its ${runBudget.maxTotalTokens} total-token budget.`,
      )
      return
    }
    if (approvalDemo && (type === 'message.delta' || type === 'message.completed')) {
      bufferedAssistantEvents.push({ type, payload })
      return
    }
    runtimeEvent(request, seq, type, payload)
  }
  emit('run.started', { model: modelService.modelId, executionProfileId: activeExecutionProfile.id })
  const toolPreparers = new Map()
  mapper = createPiEventMapper(emit, { deferCompletion: true,
    prepareToolInput: (name, input) => toolPreparers.get(name)?.(input) ?? input,
  })
  runControl.mapper = mapper
  let agent = null
  let terminalSeen = false
  let checkpoint = Promise.resolve()
  let checkpointError = null
  let stopListening = () => {}
  const budgetTimer = setTimeout(() => {
    failRun(
      'runtime.duration_budget_exceeded',
      `Run exceeded its ${runBudget.maxDurationMs}ms duration budget.`,
    )
  }, runBudget.maxDurationMs)
  activeRuns.set(request.runId, runControl)
  try {
  const preflight = async (toolCallId, tool, input, signal, metadata = {}) => {
    const response = await requestHost(request, 'tool.preflight', {
      ...metadata,
      toolCallId,
      tool,
      input,
    }, runScopedSignal(signal))
    return response.payload ?? { decision: 'block', message: 'Fox returned no preflight decision.' }
  }
  const hostRequest = (type, payload, signal) => requestHost(request, type, payload, runScopedSignal(signal))
  const pendingUser = fallbackMessages.at(-1)
  if (pendingUser?.role === 'user' && pendingUser.content === request.payload?.text) fallbackMessages.pop()
  const transcript = sanitizeAssistantHistory(sanitizeProviderHistory(transcriptFromSession(session, fallbackMessages)))
  const modelProfile = resolveModelProfile(modelService)
  const retryPolicy = retryPolicyForRun(request.payload, modelProfile)
  const model = createModel(modelService, modelProfile)
  const modelRuntime = await createFoxModelRuntime({
    model,
    apiKey: modelService.apiKey,
    fauxRegistration: modelService.apiType === 'faux' ? fauxProvider : null,
  })
  const planningContext = {
    projectRoot: request.payload?.projectContext?.projectRoot,
    permissionMode: request.payload?.projectContext?.permissionMode,
    workSnapshot: request.payload?.workSnapshot,
    memoryContext: request.payload?.memoryContext,
    assistantPackage: request.payload?.assistantPackage,
    skillPrompt: request.payload?.skillPrompt,
    expertBinding: request.payload?.expertBinding,
    expertPackage: request.payload?.expertPackage,
    runContext: request.payload?.runContext,
    conversationId: request.conversationId,
    runtimeSessionId: request.runtimeSessionId,
    model: modelService.modelId,
    executionProfile: activeExecutionProfileSnapshot,
    continuationDecisionContract: activeContinuationContract,
  }
  const cwd = String(planningContext.projectRoot || process.cwd())
  const catalogTools = [
    ...createReadOnlyTools(preflight, { executeHost: hostRequest }),
    ...createGraphReadonlyTools({
      profile: activeExecutionProfile,
      cwd,
      model,
      modelRuntime,
      modelProfile,
      preflight,
      context: planningContext,
    }),
    ...createHostTools(hostRequest),
    ...createKnowledgeTools(hostRequest),
    ...createMcpTools(hostRequest),
  ]
  assertRegisteredToolsMatchCatalog(catalogTools)
  const continuationTools = createContinuationDecisionTools({
    runId: request.runId,
    executionProfile: activeExecutionProfile,
    getEventCursor: () => Math.max(0, seq.value - 1),
    onProposal: (proposal) => emit(CONTINUATION_PROPOSAL_EVENT_TYPE, { proposal }),
  })
  const toolDiagnostics = diagnoseToolsForAgentContext(
    catalogTools,
    request.payload?.assistantPackage,
    request.payload?.expertPackage,
  )
  const profileToolSelection = applyExecutionProfileToTools(
    [...toolDiagnostics.tools, ...continuationTools],
    activeExecutionProfile,
  )
  const projectToolSelection = selectToolsForProjectContext(
    profileToolSelection.tools, request.payload?.projectContext,
  )
  const effectiveToolNames = projectToolSelection.tools.map(({ name }) => name)
  const excludedTools = [
    ...toolDiagnostics.excludedTools.filter(({ name }) => name !== CONTINUATION_PROPOSAL_TOOL_NAME),
    ...profileToolSelection.excludedTools,
    ...projectToolSelection.excludedTools,
  ]
  const tools = budgetedTools(adaptFoxToolsToPi(projectToolSelection.tools), runBudget, failRun)
  for (const tool of tools) {
    if (tool.prepareArguments) toolPreparers.set(tool.name, tool.prepareArguments)
  }
  const requestSnapshot = {
    schemaVersion: 1,
    model: modelService.modelId,
    apiType: modelService.apiType,
    provider: model.provider,
    baseUrl: modelService.baseUrl,
    stablePromptHash: null,
    contextHash: null,
    toolCatalogHash: stablePromptHash(JSON.stringify(effectiveToolNames)),
    toolNames: effectiveToolNames,
    assistantDeclaredToolNames: toolDiagnostics.assistantDeclaredToolNames,
    expertDeclaredToolNames: toolDiagnostics.expertDeclaredToolNames,
    effectiveToolNames,
    excludedTools,
    messageCount: transcript.length,
    contextWindow: modelProfile.contextWindow,
    maxOutputTokens: modelProfile.maxOutputTokens,
    modelProfile: modelProfileSnapshot(modelProfile),
    assistantPackage: request.payload?.assistantPackage ?? null,
    expertBinding: request.payload?.expertBinding ?? null,
    expertPackage: request.payload?.expertPackage ?? null,
    runContext: request.payload?.runContext ?? null,
    memoryRecallStatus: request.payload?.memoryContext?.status ?? 'empty',
    memoryRecallCount: Array.isArray(request.payload?.memoryContext?.items)
      ? request.payload.memoryContext.items.length
      : 0,
    executionProfile: activeExecutionProfileSnapshot,
    continuationDecisionContract: activeContinuationContract,
    runBudget,
    retryPolicy: retryPolicyForRun(request.payload, modelProfile),
  }
  const workToolNames = new Set(RUNTIME_TOOL_CATALOG
    .filter((tool) => tool.category === 'work')
    .map((tool) => tool.name))
  const workTools = tools.filter((tool) => workToolNames.has(tool.name))
  const customTools = tools.filter((tool) => !workToolNames.has(tool.name))
  let plannerPlan = null
  if (modelProfile.planner.enabled && shouldUsePlanner(currentText, {
    approvalDemo,
    apiType: modelService.apiType,
    hasProject: Boolean(planningContext.projectRoot),
    force: modelService.plannerMode === 'always',
  })) {
    emit('planner.started', { model: modelService.modelId })
    emit('run.phase', { phase: 'planning' })
    try {
      const planned = await runPlanner({
        cwd,
        model,
        modelService,
        modelProfile,
        modelRuntime,
        context: planningContext,
        history: transcript,
        text: currentText,
        preflight,
        onAgent: (plannerAgent) => { runControl.agent = plannerAgent },
      })
      if (runControl.cancelled) throw new Error('Planner was cancelled.')
      plannerPlan = planned.plan
      emit('planner.completed', {
        durationMs: planned.durationMs,
        planHash: planned.planHash,
        stepCount: planned.plan.steps.length,
        needsGoal: planned.plan.needsGoal,
      })
    } catch (error) {
      if (runControl.cancelled) throw error
      emit('planner.failed', {
        code: 'planner.failed',
        message: error instanceof Error ? error.message : String(error),
        fallback: 'executor_only',
      })
    } finally {
      runControl.agent = null
    }
  }
  const promptComposition = composeRuntimePrompt({
    systemPrompt: request.payload?.systemPrompt,
    modelInstructions: [modelProfilePrompt(modelProfile), executionProfilePrompt(activeExecutionProfile)]
      .filter(Boolean)
      .join('\n\n'),
    approvalDemo,
    budget: promptBudgetForRun(request.payload, modelProfile),
    cache: {
      modelId: modelService.modelId,
      toolCatalogHash: requestSnapshot.toolCatalogHash,
      policy: modelProfile.supportsPromptCache ? 'read_write' : 'disabled',
    },
    context: planningContext,
    turn: {
      cwd,
      recovery: request.payload?.recoveryContext || null,
      plannerPlan: plannerPlan ? plannerHandoff(plannerPlan) : null,
    },
  })
  requestSnapshot.stablePromptHash = promptComposition.stablePromptHash
  requestSnapshot.contextHash = promptComposition.contextHash
  requestSnapshot.promptDefinitionId = promptComposition.diagnostics.promptRegistry.definitionId
  requestSnapshot.promptVersion = promptComposition.diagnostics.promptRegistry.version
  requestSnapshot.promptContentHash = promptComposition.diagnostics.promptRegistry.contentHash
  requestSnapshot.contextSchemaHash = promptComposition.diagnostics.contextSchemaHash
  requestSnapshot.promptCacheIdentity = promptComposition.diagnostics.cacheIdentity
  requestSnapshot.promptCacheDiagnostics = promptComposition.diagnostics.cache
  requestSnapshot.promptDiagnostics = promptComposition.diagnostics
  emit('run.request_snapshot', requestSnapshot)
  emit('run.phase', { phase: 'preparing' })
  const settingsManager = SettingsManager.inMemory({
    httpIdleTimeoutMs: RUNTIME_HTTP_IDLE_TIMEOUT_MS,
    compaction: {
      enabled: true,
      reserveTokens: modelProfile.runtime.reserveTokens,
      keepRecentTokens: modelProfile.runtime.keepRecentTokens,
    },
    retry: {
      // Turn retry is owned by the Fox Kernel/Host; the sidecar only performs it
      // when explicitly configured. Provider HTTP retry is independent and bounded.
      enabled: retryPolicy.turnEnabled,
      maxRetries: retryPolicy.turnMaxRetries,
      baseDelayMs: retryPolicy.turnBaseDelayMs,
      provider: {
        maxRetries: retryPolicy.providerMaxRetries,
        maxRetryDelayMs: retryPolicy.providerMaxRetryDelayMs,
      },
    },
    images: { blockImages: !modelProfile.supportsImageInput },
  })
  const resourceLoader = new DefaultResourceLoader({
    cwd,
    agentDir: process.cwd(),
    settingsManager,
    extensionFactories: [createFoxPlanningExtension({ workTools, context: planningContext })],
    noExtensions: true,
    noSkills: true,
    noPromptTemplates: true,
    noThemes: true,
    noContextFiles: true,
    systemPrompt: promptComposition.prompt,
  })
  await resourceLoader.reload()
  const created = await createFoxAgentSession({
    cwd,
    agentDir: process.cwd(),
    model,
    thinkingLevel: modelProfile.thinkingLevel,
    tools: effectiveToolNames,
    customTools,
    resourceLoader,
    sessionManager: SessionManager.inMemory(cwd),
    settingsManager,
    modelRuntime,
  })
  agent = created.session
  runControl.agent = agent
  agent.state.messages = transcript
  const saveCheckpoint = () => {
    checkpoint = checkpoint.then(async () => {
      session.messages = normalizeHistory(sanitizeAssistantHistory(agent.state.messages))
      if (session.sessionPath) await saveSessionState(session.sessionPath, session)
    }).catch((error) => {
      const wrapped = new Error(`Failed to save the runtime checkpoint: ${error instanceof Error ? error.message : String(error)}`)
      wrapped.code = 'runtime.checkpoint_failed'
      checkpointError = wrapped
    })
  }
  let shadowTurn = 0
  stopListening = agent.subscribe((event) => {
    if (event?.type === 'turn_start') {
      shadowTurn += 1
      emit('run.phase', { phase: 'model_streaming', shadowModelRequest: 'begin' })
    }
    if (event?.type === 'message_start' && event.message?.role === 'assistant') {
      emit('run.phase', { phase: 'model_streaming', shadowModelRequest: 'settle' })
    }
    if (event?.type === 'message_end' && event.message?.role === 'assistant') {
      const calls = event.message.content?.filter((part) => part.type === 'toolCall') ?? []
      if (calls.length > 0) {
        const batchId = `pi-batch-${request.runId}-${shadowTurn}`
        emit('run.phase', {
          phase: 'tool_execution',
          shadowToolBatch: {
            batchId,
            calls: calls.map((call, sourceOrder) => ({
              toolCallId: call.id, tool: call.name, input: call.arguments, sourceOrder,
            })),
          },
        })
      }
    }
    if (event?.type === 'agent_end' && !event.willRetry) terminalSeen = true
    mapper.handle(event)
    if (event?.type === 'tool_execution_end' || event?.type === 'message_end' || event?.type === 'agent_end') {
      saveCheckpoint()
    }
  })
    const images = Array.isArray(request.payload?.images) ? request.payload.images : []
    if (images.length > 0 && !modelProfile.supportsImageInput) {
      throw new Error('The configured model does not support image input.')
    }
    emit('run.phase', { phase: 'model_streaming', attempt: 1 })
    await agent.prompt(request.payload?.text ?? '', { images, expandPromptTemplates: false })
    if (runControl.budgetError) throw runControl.budgetError
    // `message_end` / `agent_end` may already have queued a checkpoint.  Wait for
    // that writer before replacing the same session file with the final snapshot;
    // concurrent renames are not reliably replace-safe on Windows.
    await checkpoint
    if (checkpointError) throw checkpointError
    session.messages = normalizeHistory(sanitizeAssistantHistory(agent.state.messages))
    if (session.sessionPath) await saveSessionState(session.sessionPath, session)
    if (approvalDemo && toolExecutionCount === 0) {
      session.messages = replaceLatestAssistantText(session.messages, APPROVAL_DEMO_NO_TOOL_MESSAGE)
      if (session.sessionPath) await saveSessionState(session.sessionPath, session)
    }
    if (approvalDemo) {
      if (toolExecutionCount > 0) {
        for (const event of bufferedAssistantEvents) runtimeEvent(request, seq, event.type, event.payload)
      } else {
        runtimeEvent(request, seq, 'message.delta', { delta: APPROVAL_DEMO_NO_TOOL_MESSAGE })
        runtimeEvent(request, seq, 'message.completed')
      }
    }
    emit('run.phase', { phase: 'finalizing', outcome: 'completed' })
    mapper.finish()
  } catch (error) {
    emit('run.phase', { phase: 'finalizing', outcome: runControl.cancelled ? 'cancelled' : 'failed' })
    if (runControl.cancelled) mapper.cancel()
    else mapper.fail(error)
  } finally {
    clearTimeout(budgetTimer)
    if (!terminalSeen) mapper.fail(new Error('Pi Runtime ended without a terminal agent event.'))
    await checkpoint
    stopListening()
    agent?.dispose()
    runControl.agent = null
    activeRuns.delete(request.runId)
  }
}

async function handleRequest(request) {
  const validationError = validateEnvelope(request)
  if (validationError) {
    write(createEnvelope('event', 'fatal_error', { payload: { code: 'protocol.invalid_message', message: validationError } }))
    return
  }
  try {
    switch (request.type) {
      case 'evaluation.run': {
        respond(request, 'evaluation_report', await runOfflineEvals())
        break
      }
      case 'initialize': {
        const config = request.payload?.modelService
        if (!config?.baseUrl || !config?.modelId) throw new Error('Model service configuration is incomplete.')
        const nextExecutionProfile = resolveExecutionProfile({
          id: request.payload?.executionProfile
            ?? request.payload?.execution_profile
            ?? config.executionProfile
            ?? config.execution_profile
            ?? 'legacy',
          strategy: request.payload?.executionStrategy
            ?? request.payload?.execution_strategy
            ?? config.executionStrategy
            ?? config.execution_strategy,
        })
        modelService = config
        executionProfile = nextExecutionProfile
        const modelProfile = resolveModelProfile(config)
        if (config.apiType === 'faux') {
          fauxProvider?.unregister()
          fauxProvider = registerFauxProvider({
            models: [{ id: config.modelId }],
            tokensPerSecond: config.fauxTokensPerSecond ?? 1000,
          })
          fauxProvider.setResponses(configuredFauxResponses(config))
        }
        respond(request, 'ready', {
          protocol: PROTOCOL_NAME,
          protocolVersion: PROTOCOL_VERSION,
          runtime: 'fox-pi-runtime',
          runtimeVersion: `0.1.0+pi-${PI_PACKAGE_VERSION}`,
          capabilities: createCapabilityManifest({
            imageInput: modelProfile.supportsImageInput,
            tools: executionProfileCatalogTools(RUNTIME_TOOL_CATALOG, executionProfile),
          }),
          modelProfile: modelProfileSnapshot(modelProfile),
          executionProfile: executionProfileSnapshot(executionProfile),
          continuationDecisionContract: continuationDecisionContract(executionProfile),
        })
        break
      }
      case 'create_session': {
        const session = createSessionState(request.runtimeSessionId, request.conversationId)
        session.sessionPath = request.payload?.sessionPath
        sessions.set(request.runtimeSessionId, session)
        if (session.sessionPath) await saveSessionState(session.sessionPath, session)
        respond(request, 'session_created', { runtimeSessionId: request.runtimeSessionId })
        break
      }
      case 'resume_session': {
        const session = await loadSessionState(request.payload?.sessionPath, request.runtimeSessionId)
        session.sessionPath = request.payload?.sessionPath
        sessions.set(request.runtimeSessionId, session)
        respond(request, 'session_created', { runtimeSessionId: request.runtimeSessionId })
        break
      }
      case 'prompt':
        if (!modelService) throw new Error('Runtime has not been initialized.')
        respond(request, 'request_succeeded')
        void executePrompt(request)
        break
      case 'cancel': {
        const run = activeRuns.get(request.runId)
        if (run) {
          run.cancelled = true
          // Declare the cancelled terminal immediately so an abort-induced
          // provider 'error' event can never be projected as run.failed.
          run.mapper?.cancel()
          run.abort?.abort()
          run.agent?.abort()
          cancelPendingHostRequests(request.runId)
        }
        respond(request, 'request_succeeded', { cancelling: Boolean(run) })
        break
      }
      case 'shutdown':
        for (const run of activeRuns.values()) {
          run.cancelled = true
          run.mapper?.cancel()
          run.abort?.abort()
          run.agent?.abort()
        }
        cancelPendingHostRequests()
        respond(request, 'request_succeeded')
        setTimeout(() => process.exit(0), 10)
        break
      default:
        respond(request, 'request_failed', { code: 'protocol.unknown_request', message: `Unknown request type: ${request.type}` })
    }
  } catch (error) {
    respond(request, 'request_failed', { code: 'runtime.request_failed', message: error instanceof Error ? error.message : String(error) })
  }
}

const input = createInterface({ input: process.stdin, crlfDelay: Infinity })
input.on('line', (line) => {
  try {
    const message = JSON.parse(line)
    if (message.kind === 'response' && message.requestId) {
      const pending = pendingHostRequests.get(message.requestId)
      if (pending) {
        pendingHostRequests.delete(message.requestId)
        pending.resolve(message)
      }
      return
    }
    void handleRequest(message)
  } catch (error) {
    process.stderr.write(`[fox-pi-runtime] ${error instanceof Error ? error.message : String(error)}\n`)
  }
})
