import { createInterface } from 'node:readline'
import { runDurationMs, armRunDurationTimer } from './run-duration.mjs'
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
import { validatePromptControl } from './control-binding.mjs'
import { createKernelWorker } from './pi-kernel-worker.mjs'
import { createPiEventMapper, sanitizeAssistantHistory } from './pi-event-mapper.mjs'
import { createReadOnlyTools } from './read-only-tools.mjs'
import { createGraphReadonlyTools } from './graph-readonly-tools.mjs'
import { hostToolResultStorage, createHostTools, createKnowledgeTools, createMcpTools } from './host-tools.mjs'
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
import { stablePromptHash, buildFilePlacement } from './prompt-composer.mjs'
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
// No-progress guard (#8/#9/#6 of the ABC review). There is no fixed per-run
// search count and no cross-run identical-call counter: a 7th productive
// search, a poll whose state keeps changing, and a read-after-write all
// continue. A bounded correction fires only on *consecutive no progress* —
// the same canonical input returning the same result, back to back, for a
// tool without wait semantics. The correction first refuses to re-execute
// and returns guidance to the model; only a model that keeps ignoring the
// guidance stops the run.
//
// Real progress on *any* tool — including a wait-semantic poll, a write,
// or any other call — resets the consecutive counter and the correction
// ladder. Skipping the counter update for wait tools is the previous
// behaviour: a successful test_run between identical reads must let the
// next read observe the new state. We therefore always observe outcome
// changes globally; only the *trigger* (whether a tool is exempt from
// firing the correction) is conditional. The ladder does not reset merely
// because a wait tool ran with no outcome change — it stays armed but
// counts unchanged, so an idle `read` after a series of idle polls still
// crosses the streak limit.
const NO_PROGRESS_STREAK_LIMIT = 3
const NO_PROGRESS_CORRECTION_LIMIT = 3
// Tools whose result observes external state that can change while this run
// waits (polls, snapshots, re-executed checks). Identical input + identical
// result is the expected shape of waiting, so the guard never *fires* for
// them; their outcomes still update progress like any other tool.
const WAIT_SEMANTIC_TOOLS = new Set([
  "compute_job_status",
  'child_agent_list',
  'code_check',
  'git_read',
  'graph_readonly_snapshot_get',
  'run_command',
  'system_info',
  'team_snapshot_get',
  'test_run',
  'work_snapshot_get',
  'workflow_snapshot_get',
])
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
    maxDurationMs: runDurationMs(payload),
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
    // Legacy payloads may still carry maxIdenticalToolCalls; it now tunes the
    // consecutive no-progress streak instead of a cross-run call counter.
    noProgressStreakLimit: positiveBudgetValue(
      configured.maxIdenticalToolCalls,
      NO_PROGRESS_STREAK_LIMIT,
      8,
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

// Key-order-insensitive input identity: semantically equal arguments must
// count as the same call regardless of property order.
function canonicalJson(value) {
  if (Array.isArray(value)) return `[${value.map(canonicalJson).join(',')}]`
  if (value && typeof value === 'object') {
    return `{${Object.keys(value).sort()
      .map((key) => `${JSON.stringify(key)}:${canonicalJson(value[key])}`)
      .join(',')}}`
  }
  return JSON.stringify(value) ?? 'null'
}

// The model-relevant outcome of one settled call. Two calls with the same
// outcome produced no new information, whatever their object identity.
// `details` is excluded on purpose: cursors and diagnostics may differ
// between identical reads without giving the model anything new.
function outcomeFingerprint(result) {
  return canonicalJson({
    isError: result?.isError === true,
    content: result?.content ?? result ?? null,
  })
}

function noProgressCorrectionResult(toolName, streak) {
  return {
    isError: true,
    content: [{
      type: 'text',
      text: `Fox 有界纠偏：工具 ${toolName} 以相同输入连续 ${streak} 次返回相同结果，没有新进展。`
        + '本次调用未再执行。请基于已收集的证据继续任务、更换查询或参数、或明确说明还需等待什么；'
        + '不要再次以相同输入调用该工具。写后复读、状态已变化的轮询和不同输入的调用不受影响。',
    }],
    details: { noProgress: true, consecutiveRepeats: streak },
  }
}

function budgetedTools(tools, budget, failRun) {
  let totalCalls = 0
  // fingerprint -> the outcome that exact call returned last time.
  const outcomes = new Map()
  // fingerprint -> how many consecutive times it has reproduced its own
  // previous outcome since the last change. Per fingerprint, so alternating
  // loops are bounded too, and an interleaved *productive* call clears it.
  const streaks = new Map()
  // Refusals issued for the current stuck episode. Real progress clears it.
  let corrections = 0
  const clearProgress = () => {
    streaks.clear()
    corrections = 0
  }
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
      const exempt = WAIT_SEMANTIC_TOOLS.has(tool.name)
      const fingerprint = `${tool.name}\0${canonicalJson(input ?? {})}`
      const previousOutcome = outcomes.get(fingerprint)
      const streak = streaks.get(fingerprint) ?? 0
      // Refuse only a non-exempt call that has already reproduced its own
      // outcome to the bound. Waiting tools never fire the correction; the
      // Host duration/token budgets bound them instead.
      if (!exempt && previousOutcome !== undefined && streak >= budget.noProgressStreakLimit) {
        corrections += 1
        if (corrections > NO_PROGRESS_CORRECTION_LIMIT) {
          throw failRun(
            'runtime.no_progress',
            `Run made no progress: ${tool.name} returned identical results for identical input `
              + `${streak} times in a row and ${corrections - 1} corrections were ignored.`,
          )
        }
        return noProgressCorrectionResult(tool.name, streak)
      }
      const result = await tool.execute(toolCallId, input, signal, ...rest)
      const outcome = outcomeFingerprint(result)
      if (previousOutcome === undefined) {
        // First sight of this exact call: neutral, nothing to compare yet.
        outcomes.set(fingerprint, outcome)
        streaks.set(fingerprint, 0)
      } else if (previousOutcome !== outcome) {
        // Real progress: this call returned something new. External state
        // changed, so every other idle fingerprint may now read differently
        // too — clear all streaks and the correction ladder. This is what
        // lets a productive test_run/run_command unblock a later re-read,
        // even when the productive tool is itself wait-semantic.
        outcomes.set(fingerprint, outcome)
        clearProgress()
      } else {
        // Identical outcome: one more no-progress repeat of this call.
        outcomes.set(fingerprint, outcome)
        streaks.set(fingerprint, streak + 1)
      }
      return result
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
  const budgetTimer = armRunDurationTimer(runBudget.maxDurationMs, () => {
    failRun(
      'runtime.duration_budget_exceeded',
      `Run exceeded its ${runBudget.maxDurationMs}ms duration budget.`,
    )
  })
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
    filePlacement: buildFilePlacement(request.payload?.projectContext),
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
      executeHost: hostRequest,
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
  const tools = budgetedTools(
    adaptFoxToolsToPi(projectToolSelection.tools, { runId: request.runId, storageFor: hostToolResultStorage }),
    runBudget,
    failRun,
  )
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
            // The kernel worker splices directive steering into the live
            // transcript (pi-kernel-loop / pi-kernel-batch-resume); on the
            // legacy path the Host delivers the same text through frozen input
            // messages, so mid-run additions are supported on every path.
            steering: true,
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
      case 'prompt': {
        if (!modelService) throw new Error('Runtime has not been initialized.')
        const frozenProject = validatePromptControl(request, executionProfile.id)
        const session = currentSession(request)
        if (session.conversationId !== request.conversationId) throw new Error('Runtime session belongs to another conversation')
        if (activeRuns.has(request.runId)) throw new Error('Run is already active')
        if (frozenProject) request.payload.projectContext = frozenProject
        respond(request, 'request_succeeded')
        void executePrompt(request)
        break
      }
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

const kernelWorker = process.argv.includes('--kernel-worker') ? createKernelWorker(write) : null
const input = createInterface({ input: process.stdin, crlfDelay: Infinity })
if (kernelWorker) input.on('close', () => { void kernelWorker.close().catch(() => { process.exitCode = 1 }) })
input.on('line', (line) => {
  try {
    // Bounded normal Kernel frame (#14): 1 MiB of UTF-8 JSON per input line —
    // the same bound and wording as `frame_limit_exceeded` on the Host side.
    if (kernelWorker && Buffer.byteLength(line, 'utf8') > 1_048_576) {
      throw new Error(
        `kernel.frame_limit_exceeded: normal Kernel model frame is ${Buffer.byteLength(line, 'utf8') - 1_048_576} UTF-8 JSON bytes over the 1,048,576-byte limit; use references or pagination instead of enlarging the frame`,
      )
    }
    const message = JSON.parse(line)
    if (kernelWorker) {
      void kernelWorker.handle(message)
      return
    }
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
    if (kernelWorker) {
      write(createEnvelope('event', 'fatal_error', { payload: { code: 'kernel.invalid_input', message: 'Invalid Kernel protocol input' } }))
      return
    }
    process.stderr.write(`[fox-pi-runtime] ${error instanceof Error ? error.message : String(error)}\n`)
  }
})
