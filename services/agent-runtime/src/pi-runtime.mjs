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
import { diagnoseToolsForAgentContext } from './expert-package.mjs'
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
  write(message)
  return new Promise((resolve, reject) => {
    const abort = () => {
      pendingHostRequests.delete(message.id)
      reject(new Error('Host request was cancelled.'))
    }
    signal?.addEventListener('abort', abort, { once: true })
    pendingHostRequests.set(message.id, {
      resolve: (value) => { signal?.removeEventListener('abort', abort); resolve(value) },
      reject,
    })
  })
}

function cancelPendingHostRequests() {
  for (const pending of pendingHostRequests.values()) {
    pending.reject(new Error('Host request was cancelled.'))
  }
  pendingHostRequests.clear()
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
  const runControl = {
    cancelled: false,
    agent: null,
  }
  const emit = (type, payload = {}) => {
    if (runControl.cancelled
      && type !== 'usage.updated'
      && type !== 'run.completed'
      && type !== 'run.cancelled'
      && type !== 'run.interrupted'
      && type !== 'run.failed') return
    if (type === 'tool.started') toolExecutionCount += 1
    if (approvalDemo && (type === 'message.delta' || type === 'message.completed')) {
      bufferedAssistantEvents.push({ type, payload })
      return
    }
    runtimeEvent(request, seq, type, payload)
  }
  emit('run.started', { model: modelService.modelId, executionProfileId: activeExecutionProfile.id })
  const mapper = createPiEventMapper(emit, { deferCompletion: true })
  let agent = null
  let terminalSeen = false
  let checkpoint = Promise.resolve()
  let stopListening = () => {}
  activeRuns.set(request.runId, runControl)
  try {
  const preflight = async (tool, input, signal) => {
    const response = await requestHost(request, 'tool.preflight', { tool, input }, signal)
    return response.payload ?? { decision: 'block', message: 'Fox returned no preflight decision.' }
  }
  const hostRequest = (type, payload, signal) => requestHost(request, type, payload, signal)
  const pendingUser = fallbackMessages.at(-1)
  if (pendingUser?.role === 'user' && pendingUser.content === request.payload?.text) fallbackMessages.pop()
  const transcript = sanitizeAssistantHistory(sanitizeProviderHistory(transcriptFromSession(session, fallbackMessages)))
  const modelProfile = resolveModelProfile(modelService)
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
    expertBinding: request.payload?.expertBinding,
    expertPackage: request.payload?.expertPackage,
    conversationId: request.conversationId,
    runtimeSessionId: request.runtimeSessionId,
    model: modelService.modelId,
    executionProfile: activeExecutionProfileSnapshot,
    continuationDecisionContract: activeContinuationContract,
  }
  const cwd = String(planningContext.projectRoot || process.cwd())
  const catalogTools = [
    ...createReadOnlyTools(preflight),
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
  const effectiveToolNames = profileToolSelection.tools.map(({ name }) => name)
  const excludedTools = [
    ...toolDiagnostics.excludedTools.filter(({ name }) => name !== CONTINUATION_PROPOSAL_TOOL_NAME),
    ...profileToolSelection.excludedTools,
  ]
  const tools = adaptFoxToolsToPi(profileToolSelection.tools)
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
    memoryRecallStatus: request.payload?.memoryContext?.status ?? 'empty',
    memoryRecallCount: Array.isArray(request.payload?.memoryContext?.items)
      ? request.payload.memoryContext.items.length
      : 0,
    executionProfile: activeExecutionProfileSnapshot,
    continuationDecisionContract: activeContinuationContract,
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
    compaction: {
      enabled: true,
      reserveTokens: modelProfile.runtime.reserveTokens,
      keepRecentTokens: modelProfile.runtime.keepRecentTokens,
    },
    retry: {
      enabled: true,
      maxRetries: modelProfile.runtime.maxRetries,
      baseDelayMs: 750,
      provider: {
        maxRetries: modelProfile.runtime.maxRetries,
        maxRetryDelayMs: modelProfile.runtime.providerMaxRetryDelayMs,
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
    }).catch(() => undefined)
  }
  stopListening = agent.subscribe((event) => {
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
    // `message_end` / `agent_end` may already have queued a checkpoint.  Wait for
    // that writer before replacing the same session file with the final snapshot;
    // concurrent renames are not reliably replace-safe on Windows.
    await checkpoint
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
        if (run) run.cancelled = true
        run?.agent?.abort()
        cancelPendingHostRequests()
        respond(request, 'request_succeeded', { cancelling: Boolean(run) })
        break
      }
      case 'shutdown':
        for (const run of activeRuns.values()) run.agent.abort()
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
