import { createInterface } from 'node:readline'
import { randomUUID } from 'node:crypto'
import { dirname, join } from 'node:path'
import { existsSync } from 'node:fs'
import { runDurationMs, armRunDurationTimer } from './run-duration.mjs'
import { createFatalErrorHandler } from './runtime-fatal.mjs'
import {
  DefaultResourceLoader,
  PI_PACKAGE_VERSION,
  SessionManager,
  SettingsManager,
  createFoxAgentSession,
  setFoxAgentHistory,
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
import { promptTokenPreflight } from './prompt-budget.mjs'
import { plannerHandoff, runPlanner, shouldUsePlanner } from './planner-runtime.mjs'
import { diagnoseToolsForAgentContext, selectToolsForProjectContext } from './expert-package.mjs'
import { selectToolsForHostRuntime } from './host-runtime-capabilities.mjs'
import { adaptFoxToolsToPi } from './tool-adapter.mjs'
import { createNativeToolBridge } from './pi-native-tool-bridge.mjs'
import { markNativeSessionRun, reconcileNativeSession } from './pi-native-recovery.mjs'
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
const sessionRuns = new Map()
const openingSessions = new Map()
const closingSessions = new Set()
const pendingHostRequests = new Map()
const RUNTIME_HTTP_IDLE_TIMEOUT_MS = 90_000
// Host requests have no fixed timeout. A Host tool may wait for a human
// approval, and approvals wait for the user without expiring (Claude Code /
// Codex CLI / ACP semantics). Every Host operation is still bounded on the Host
// side (tool execution / Run budgets), and a request ends early when the Run is
// cancelled (`signal`) or the runtime shuts down (`cancelPendingHostRequests`).
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

function respondFailure(request, error) {
  const message = error instanceof Error ? error.message : String(error)
  const code = typeof error?.code === 'string' && error.code.startsWith('session.') ? error.code : 'runtime.request_failed'
  respond(request, 'request_failed', { code, message })
}

// ---------------------------------------------------------------------------
// Native Pi sessions (stage two of the single Pi mainline).
//
// A `pi-native` session is owned by Pi's SessionManager: the model context,
// the compaction record and the branch live in the session JSONL file the
// manager writes, not in a Fox snapshot. Fox may keep a display copy elsewhere,
// but this runtime never writes the transcript back over it.
//
// The Host always names the directory (it owns the user data directory) and the
// runtime reports the exact file and session id it created, so a later resume
// opens that file and nothing else. Guessing (`continueRecent`) is never used:
// several experts may share one working directory.

/** Host request for a Pi-owned session (explicit flag, or an explicit .jsonl). */
function requestsNativeSession(payload) {
  if (!payload || typeof payload !== 'object') return false
  if (payload.sessionFormat === 'pi-native') return true
  if (payload.sessionFormat === 'fox-snapshot') return false
  return typeof payload.nativeSessionPath === 'string' && payload.nativeSessionPath.endsWith('.jsonl')
}

/** A distinguishable failure: the Host must tell these apart, never fall back. */
function sessionError(code, message) {
  const error = new Error(message)
  error.code = code
  return error
}

function nativeSessionDirectory(payload) {
  if (typeof payload?.sessionDir === 'string' && payload.sessionDir.trim()) return payload.sessionDir
  if (typeof payload?.sessionPath === 'string' && payload.sessionPath.trim()) return dirname(payload.sessionPath)
  return join(process.cwd(), 'sessions')
}

function nativeSessionMetadata(session) {
  return {
    format: 'pi-native',
    nativeSessionPath: session.nativeSessionPath,
    nativeSessionId: session.nativeSessionId,
    contextGeneration: session.contextGeneration ?? 1,
  }
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
    signal?.addEventListener('abort', abort, { once: true })
    pendingHostRequests.set(message.id, {
      runId: request.runId,
      conversationId: request.conversationId,
      runtimeSessionId: request.runtimeSessionId,
      cleanup: ['tool_call.close', 'tool_call.cancel', 'model_request.complete'].includes(type),
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
    if (runId === undefined || (pending.runId === runId && !pending.cleanup)) {
      pendingHostRequests.delete(id)
      pending.reject(new Error('Host request was cancelled.'))
    }
  }
}

function createModel(config, profile, fauxProvider) {
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

function createToolBudget(budget, failRun) {
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
  return (tools) => tools.map((tool) => ({
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

function configurationForRun(request) {
  const config = structuredClone(request.payload?.modelService ?? modelService)
  if (!config?.baseUrl || !config?.modelId) throw new Error('Model service configuration is incomplete.')
  const id = request.payload?.executionProfile ?? executionProfile.id
  const strategy = request.payload?.executionStrategy
  const profile = id === executionProfile.id && strategy === undefined
    ? executionProfile : resolveExecutionProfile({ id, strategy })
  return { modelService: config, executionProfile: profile }
}

function cancelRuntimeRun(runId) {
  const run = activeRuns.get(runId)
  const slot = [...sessionRuns.values()].find(value => value.runId === runId)
  if (slot) {
    slot.cancelled = true
    slot.abort.abort()
  }
  if (run) {
    run.cancelled = true
    run.mapper?.cancel()
    run.abort?.abort()
    void Promise.resolve(run.agent?.abort()).catch(error => run.mapper?.fail(error))
  }
  cancelPendingHostRequests(runId)
  return Boolean(run || slot)
}

async function executePrompt(request, { modelService, executionProfile }, slot) {
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
  let observedModelTokens = 0
  const runBudget = runBudgetForRequest(request.payload)
  const runAbort = slot.abort
  const runControl = {
    cancelled: slot.cancelled,
    agent: null,
    budgetError: null,
    abort: runAbort,
    mapper: null,
    // Identity for a terminal frame written from outside this promise chain
    // (see `reportRunFailure` below); a fatal error must still end the Run.
    request,
    seq,
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
    runAbort.abort(error)
    runControl.agent?.abort()
    return error
  }
  const applyToolBudget = createToolBudget(runBudget, failRun)
  const emit = (type, payload = {}) => {
    if (runControl.cancelled
      && type !== 'usage.updated'
      && type !== 'usage.request'
      && type !== 'run.completed'
      && type !== 'run.cancelled'
      && type !== 'run.interrupted'
      && type !== 'run.failed') return
    if (type === 'tool.started') toolExecutionCount += 1
    if (type === 'usage.request') {
      const usage = payload.record?.usage
      observedModelTokens += ['input', 'output', 'cacheRead', 'cacheWrite']
        .reduce((total, key) => total + (Number.isSafeInteger(usage?.[key]) && usage[key] > 0 ? usage[key] : 0), 0)
      if (runBudget.maxTotalTokens !== null && observedModelTokens > runBudget.maxTotalTokens) {
        runtimeEvent(request, seq, type, payload)
        failRun('runtime.token_budget_exceeded', `Run exceeded its ${runBudget.maxTotalTokens} total-token budget, including model retries and compaction.`)
        return
      }
    }
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
  const registeredEventTools = new Set()
  mapper = createPiEventMapper(emit, { deferCompletion: true,
    prepareToolInput: (name, input) => toolPreparers.get(name)?.(input) ?? input,
    isToolRegistered: name => registeredEventTools.has(name),
  })
  runControl.mapper = mapper
  let agent = null
  let fauxProvider = null
  let terminalSeen = false
  let checkpoint = Promise.resolve()
  let checkpointError = null
  let stopListening = () => {}
  let nativeToolBridge = null
  let modelToolBridge = null
  let nativeEventQueue = Promise.resolve()
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
  const hostTransport = (type, payload, signal) => requestHost(request, type, payload, signal).catch(error => {
    if (error.code === 'runtime.host_response_identity_mismatch') throw failRun(error.code, error.message)
    throw error
  })
  const hostRequest = (type, payload, signal) => hostTransport(type, payload, runScopedSignal(signal))
  const executorRoute = request.payload?.controlBinding?.executorRoute ?? null
  nativeToolBridge = executorRoute === 'pi_native'
    ? createNativeToolBridge({
        requestHost: hostTransport,
        runId: request.runId,
        contextGeneration: session.contextGeneration ?? 1,
        signal: runAbort.signal,
      })
    : null
  modelToolBridge = nativeToolBridge
  const pendingUser = fallbackMessages.at(-1)
  if (pendingUser?.role === 'user' && pendingUser.content === request.payload?.text) fallbackMessages.pop()
  const transcript = sanitizeAssistantHistory(sanitizeProviderHistory(transcriptFromSession(session, fallbackMessages)))
  const modelProfile = resolveModelProfile(modelService)
  const retryPolicy = retryPolicyForRun(request.payload, modelProfile)
  if (modelService.apiType === 'faux') {
    const name = `fox-faux-${randomUUID()}`
    fauxProvider = registerFauxProvider({ api: name, provider: name,
      models: [{ id: modelService.modelId, maxTokens: modelProfile.maxOutputTokens, ...(Number.isSafeInteger(modelService.fauxContextWindow)
        ? { contextWindow: modelService.fauxContextWindow } : {}) }],
      tokensPerSecond: modelService.fauxTokensPerSecond ?? 1000,
    })
    fauxProvider.setResponses(configuredFauxResponses(modelService))
  }
  const model = createModel(modelService, modelProfile, fauxProvider)
  const modelRuntime = await createFoxModelRuntime({
    usage: { runId: request.runId, raw: modelService, onRecord: record => emit('usage.request', { record }),
      ...(executorRoute === 'pi_native' ? {
        signal: runAbort.signal,
        onFailure: error => failRun(error.code ?? 'runtime.model_request_failed', error.message),
        beforeAttempt: async ({ requestId, stage, maxOutputTokens, signal }) => {
          try {
            await modelToolBridge?.drain()
          } catch (error) {
            throw failRun('runtime.native_tool_bridge_failed', error.message)
          }
          signal?.throwIfAborted()
          const response = await hostTransport('model_request.admit', { requestId, stage, maxOutputTokens }, signal)
          if (response.type !== 'model_request.admitted' || response.payload?.allowed !== true) {
            throw failRun(response.payload?.code ?? response.payload?.errorDetails?.code ?? 'runtime.model_request_denied',
              response.payload?.error ?? response.payload?.reasonCode ?? 'Host did not admit the model request.')
          }
          return response.payload
        },
        afterAttempt: async ({ requestId, inputTokens, outputTokens, status }) => {
          try {
            const response = await hostTransport('model_request.complete', { requestId, inputTokens, outputTokens, status }, AbortSignal.timeout(5000))
            if (response.type !== 'model_request.completed' || response.payload?.isError) {
              throw new Error(response.payload?.error ?? 'Host did not confirm model request completion.')
            }
          } catch (error) {
            throw failRun('runtime.model_request_completion_failed', error.message)
          }
        },
      } : {}),
    },
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
    hostRuntimeCapabilities: request.payload?.hostRuntimeCapabilities,
    capabilityManifestHash: request.payload?.controlBinding?.capabilityManifestHash,
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
  const projectSelection = selectToolsForProjectContext(
    profileToolSelection.tools, request.payload?.projectContext,
  )
  // No external execution backend: run_command is hidden from the model.
  const hostRuntimeSelection = selectToolsForHostRuntime(
    projectSelection.tools,
    request.payload?.hostRuntimeCapabilities,
    { capabilityManifestHash: request.payload?.controlBinding?.capabilityManifestHash },
  )
  const projectToolSelection = { tools: hostRuntimeSelection.tools }
  const effectiveToolNames = projectToolSelection.tools.map(({ name }) => name)
  const excludedTools = [
    ...toolDiagnostics.excludedTools.filter(({ name }) => name !== CONTINUATION_PROPOSAL_TOOL_NAME),
    ...profileToolSelection.excludedTools,
    ...projectSelection.excludedTools,
    ...hostRuntimeSelection.excludedTools,
  ]
  // Route the Fox executor before Pi result projection and Run budgeting. Both
  // wrappers must still see every Host answer, including work-extension tools.
  const localTools = projectToolSelection.tools.filter((tool) => tool.name === CONTINUATION_PROPOSAL_TOOL_NAME)
  const routedTools = nativeToolBridge
    ? [...nativeToolBridge.wrapTools(projectToolSelection.tools.filter((tool) => tool.name !== CONTINUATION_PROPOSAL_TOOL_NAME)), ...localTools]
    : projectToolSelection.tools
  const tools = applyToolBudget(
    adaptFoxToolsToPi(routedTools, { runId: request.runId, storageFor: hostToolResultStorage }),
  )
  for (const tool of tools) {
    if (RUNTIME_TOOL_CATALOG.some(entry => entry.name === tool.name)) registeredEventTools.add(tool.name)
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
  const composePrompt = (plannerPlan = null) => composeRuntimePrompt({
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
  // Reject an impossible request before the planner or the SDK can issue any
  // model HTTP call. Compose the same system prompt the executor will use (with
  // no planner handoff yet) so runtime instructions and the work snapshot are
  // counted, not just the caller's short systemPrompt. History is deliberately
  // excluded: Pi compaction owns it.
  const preflightPrompt = composePrompt(null)
  const promptPreflight = promptTokenPreflight({
    systemPrompt: preflightPrompt.prompt,
    toolDefinitions: JSON.stringify(tools.map(({ name, description, parameters }) => ({
      name, description, parameters,
    }))),
    userText: currentText,
    contextWindow: modelProfile.contextWindow,
    reserveTokens: modelProfile.runtime.reserveTokens,
  })
  if (promptPreflight.exceeds) {
    throw failRun(
      'model.context_window_too_small',
      `The non-compressible prompt is approximately ${promptPreflight.estimatedTokens} tokens, but only ${promptPreflight.availableInputTokens} input tokens are available in the ${promptPreflight.contextWindow}-token context window after the ${promptPreflight.reserveTokens}-token reserve.`,
    )
  }
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
      const plannerToolBridge = nativeToolBridge ? createNativeToolBridge({
        requestHost: hostTransport, runId: request.runId, contextGeneration: session.contextGeneration ?? 1,
        signal: runAbort.signal, callIdPrefix: `planner:${randomUUID()}:`,
      }) : null
      modelToolBridge = plannerToolBridge
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
        toolBridge: plannerToolBridge,
        prepareTools: list => applyToolBudget(adaptFoxToolsToPi(list, { runId: request.runId, storageFor: hostToolResultStorage })),
        signal: runAbort.signal,
        onToolBridgeError: error => failRun(error.code ?? 'runtime.native_tool_bridge_failed', error.message),
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
      modelToolBridge = nativeToolBridge
    }
    if (runControl.budgetError) throw runControl.budgetError
    runAbort.signal.throwIfAborted()
  }
  const promptComposition = composePrompt(plannerPlan)
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
  // Stage two: a pi-native session keeps its whole context in Pi's
  // SessionManager. The manager is the only writer; the Fox snapshot path is
  // left untouched for the legacy (fox-snapshot) sessions.
  const nativeManager = session.format === 'pi-native' ? session.nativeManager : null
  if (session.format === 'pi-native' && !nativeManager) {
    throw sessionError('session.native_not_open', 'The native Pi session is not open for this run.')
  }
  // The bridge hook is composed with the SDK's own beforeToolCall inside
  // `createFoxAgentSession`; the SDK overwrites a same-named option.
  const created = await createFoxAgentSession({
    cwd,
    agentDir: process.cwd(),
    model,
    thinkingLevel: modelProfile.thinkingLevel,
    tools: effectiveToolNames,
    customTools,
    resourceLoader,
    sessionManager: nativeManager ?? SessionManager.inMemory(cwd),
    settingsManager,
    modelRuntime,
    foxBeforeToolCall: nativeToolBridge ? (context, signal) => nativeToolBridge.beforeToolCall(context, signal).catch(error => {
      throw failRun(error.code ?? 'runtime.native_tool_bridge_failed', error.message)
    }) : undefined,
  })
  agent = created.session
  runControl.agent = agent
  if (nativeManager) {
    // The manager already holds every prior turn (and the compaction record).
    // Injecting Fox's display history here would duplicate the conversation and
    // could reintroduce context Pi has compacted away.
    session.nativeSessionId = agent.sessionManager?.getSessionId?.() ?? session.nativeSessionId
    session.nativeSessionPath = agent.sessionManager?.getSessionFile?.() ?? session.nativeSessionPath
  } else {
    setFoxAgentHistory(agent, transcript)
  }
  const saveCheckpoint = () => {
    // Pi writes its own JSONL while the turn runs; a Fox snapshot would be a
    // second writer of the same context.
    if (nativeManager) return
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
  const projectAgentEvent = event => {
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
    mapper.handle(event)
  }
  stopListening = agent.subscribe((event) => {
    if (event?.type === 'agent_end' && !event.willRetry) terminalSeen = true
    if (nativeToolBridge) {
      const bridgeFailure = error => failRun('runtime.native_tool_bridge_failed', `Native tool bridge could not report its batch positions: ${error?.message ?? error}`)
      Promise.resolve(nativeToolBridge.onEvent(event, runAbort.signal)).catch(bridgeFailure)
      // SDK tool_execution_start precedes its beforeToolCall hook. Queue every
      // projected SDK event so Host registration is acknowledged before the
      // first lifecycle event, without reordering later events or their seq.
      nativeEventQueue = nativeEventQueue.then(async () => {
        await nativeToolBridge.waitForEventRegistration(event, runAbort.signal)
        if (!runControl.budgetError) projectAgentEvent(event)
      }).catch(bridgeFailure)
    } else projectAgentEvent(event)
    if (event?.type === 'tool_execution_end' || event?.type === 'message_end' || event?.type === 'agent_end') {
      saveCheckpoint()
    }
  })
    const images = Array.isArray(request.payload?.images) ? request.payload.images : []
    if (images.length > 0 && !modelProfile.supportsImageInput) {
      throw new Error('The configured model does not support image input.')
    }
    emit('run.phase', { phase: 'model_streaming', attempt: 1 })
    if (nativeManager) markNativeSessionRun(nativeManager, {
      runId: request.runId, contextGeneration: session.contextGeneration ?? 1, executorRoute,
    })
    await agent.prompt(request.payload?.text ?? '', { images, expandPromptTemplates: false })
    await nativeEventQueue
    await nativeToolBridge?.drain()
    if (runControl.budgetError) throw runControl.budgetError
    // `message_end` / `agent_end` may already have queued a checkpoint.  Wait for
    // that writer before replacing the same session file with the final snapshot;
    // concurrent renames are not reliably replace-safe on Windows.
    await checkpoint
    if (checkpointError) throw checkpointError
    if (!nativeManager) {
      session.messages = normalizeHistory(sanitizeAssistantHistory(agent.state.messages))
      if (session.sessionPath) await saveSessionState(session.sessionPath, session)
    }
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
    await nativeEventQueue
    try {
      await nativeToolBridge?.drain()
    } catch (error) {
      if (!runControl.cancelled) mapper.fail(error)
    }
    if (!terminalSeen) mapper.fail(new Error('Pi Runtime ended without a terminal agent event.'))
    await checkpoint
    stopListening()
    agent?.dispose()
    fauxProvider?.unregister()
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
  let opening = null
  try {
    if (request.type === 'create_session' || request.type === 'resume_session') {
      if (openingSessions.has(request.runtimeSessionId)) throw sessionError('session.busy', 'This runtime session is already opening.')
      let resolve
      opening = { request, abort: new AbortController(), done: new Promise(done => { resolve = done }), resolve: () => resolve() }
      openingSessions.set(request.runtimeSessionId, opening)
    }
    switch (request.type) {
      case 'evaluation.run': {
        respond(request, 'evaluation_report', await runOfflineEvals())
        break
      }
      case 'initialize': {
        if (sessionRuns.size || activeRuns.size) throw sessionError('session.runtime_busy', 'Runtime configuration cannot change while a Run is active or preparing.')
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
        modelService = structuredClone(config)
        executionProfile = nextExecutionProfile
        const modelProfile = resolveModelProfile(config)
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
        if (sessionRuns.has(request.runtimeSessionId) || closingSessions.has(request.runtimeSessionId)) {
          throw sessionError('session.busy', 'This runtime session is active or closing.')
        }
        if (sessions.has(request.runtimeSessionId)) throw sessionError('session.already_open', 'This runtime session is already open.')
        if (requestsNativeSession(request.payload)) {
          const cwd = String(request.payload?.cwd || process.cwd())
          const sessionDir = nativeSessionDirectory(request.payload)
          let manager
          try {
            manager = SessionManager.create(cwd, sessionDir)
          } catch (error) {
            throw sessionError('session.native_directory_unusable', `The Pi session directory cannot be used: ${error instanceof Error ? error.message : String(error)}`)
          }
          const session = {
            schemaVersion: 2,
            format: 'pi-native',
            runtimeSessionId: request.runtimeSessionId,
            conversationId: request.conversationId,
            contextGeneration: Number.isInteger(request.payload?.contextGeneration) ? request.payload.contextGeneration : 1,
            nativeManager: manager,
            nativeSessionPath: manager.getSessionFile(),
            nativeSessionId: manager.getSessionId(),
            messages: [],
          }
          sessions.set(request.runtimeSessionId, session)
          respond(request, 'session_created', { runtimeSessionId: request.runtimeSessionId, ...nativeSessionMetadata(session) })
          break
        }
        const session = createSessionState(request.runtimeSessionId, request.conversationId)
        session.sessionPath = request.payload?.sessionPath
        if (session.sessionPath) await saveSessionState(session.sessionPath, session)
        opening.abort.signal.throwIfAborted()
        sessions.set(request.runtimeSessionId, session)
        respond(request, 'session_created', { runtimeSessionId: request.runtimeSessionId })
        break
      }
      case 'resume_session': {
        if (sessionRuns.has(request.runtimeSessionId) || closingSessions.has(request.runtimeSessionId)) {
          throw sessionError('session.busy', 'This runtime session is active or closing.')
        }
        if (sessions.has(request.runtimeSessionId)) throw sessionError('session.already_open', 'This runtime session is already open.')
        if (requestsNativeSession(request.payload)) {
          const path = request.payload?.nativeSessionPath ?? request.payload?.sessionPath
          if (typeof path !== 'string' || !path.trim()) {
            throw sessionError('session.native_path_required', 'A native Pi session resume must name the exact session file.')
          }
          // Pi's `open` starts a fresh session when the file is absent; a Fox
          // resume must never be answered by a silently created empty session.
          if (!existsSync(path)) {
            throw sessionError('session.native_missing', `The Pi session file does not exist: ${path}`)
          }
          let manager
          try {
            manager = SessionManager.open(path, request.payload?.sessionDir, request.payload?.cwd)
          } catch (error) {
            const message = error instanceof Error ? error.message : String(error)
            const code = /ENOENT|no such file|not found/i.test(message) ? 'session.native_missing' : 'session.native_corrupt'
            throw sessionError(code, `The Pi session file cannot be opened: ${message}`)
          }
          const expected = request.payload?.nativeSessionId
          const actual = manager.getSessionId()
          if (typeof expected === 'string' && expected && expected !== actual) {
            throw sessionError('session.native_identity_mismatch', 'The Pi session file belongs to another session; refusing to open it as this conversation.')
          }
          if (request.payload?.conversationId && manager.getHeader?.()?.conversationId
            && manager.getHeader().conversationId !== request.conversationId) {
            throw sessionError('session.native_conversation_mismatch', 'The Pi session file belongs to another conversation.')
          }
          const session = {
            schemaVersion: 2,
            format: 'pi-native',
            runtimeSessionId: request.runtimeSessionId,
            conversationId: request.conversationId,
            contextGeneration: Number.isInteger(request.payload?.contextGeneration) ? request.payload.contextGeneration : 1,
            nativeManager: manager,
            nativeSessionPath: path,
            nativeSessionId: actual,
            messages: [],
          }
          await reconcileNativeSession({
            manager, runId: request.runId, contextGeneration: session.contextGeneration,
            requestHost: (type, payload, signal) => requestHost(request, type, payload, signal),
            signal: AbortSignal.any([opening.abort.signal, AbortSignal.timeout(30_000)]),
          })
          opening.abort.signal.throwIfAborted()
          sessions.set(request.runtimeSessionId, session)
          respond(request, 'session_created', { runtimeSessionId: request.runtimeSessionId, ...nativeSessionMetadata(session) })
          break
        }
        const session = await loadSessionState(request.payload?.sessionPath, request.runtimeSessionId)
        opening.abort.signal.throwIfAborted()
        session.sessionPath = request.payload?.sessionPath
        sessions.set(request.runtimeSessionId, session)
        respond(request, 'session_created', { runtimeSessionId: request.runtimeSessionId })
        break
      }
      case 'prompt': {
        if (!modelService) throw new Error('Runtime has not been initialized.')
        const config = configurationForRun(request)
        const frozenProject = validatePromptControl(request, config.executionProfile.id)
        const session = currentSession(request)
        if (session.conversationId !== request.conversationId) throw new Error('Runtime session belongs to another conversation')
        if (activeRuns.has(request.runId) || [...sessionRuns.values()].some(value => value.runId === request.runId)) {
          throw new Error('Run is already active')
        }
        if (sessionRuns.has(request.runtimeSessionId) || openingSessions.has(request.runtimeSessionId) || closingSessions.has(request.runtimeSessionId)) {
          throw sessionError('session.busy', 'This runtime session already has an active or preparing Run.')
        }
        const slot = { runId: request.runId, request, abort: new AbortController(), cancelled: false, done: null }
        sessionRuns.set(request.runtimeSessionId, slot)
        slot.done = (async () => {
          let acknowledged = false
          try {
            if (session.format === 'pi-native') await reconcileNativeSession({
              manager: session.nativeManager, runId: request.runId, contextGeneration: session.contextGeneration,
              requestHost: (type, payload, signal) => requestHost(request, type, payload, signal),
              signal: AbortSignal.any([slot.abort.signal, AbortSignal.timeout(30_000)]),
            })
            slot.abort.signal.throwIfAborted()
            if (frozenProject) request.payload.projectContext = frozenProject
            respond(request, 'request_succeeded')
            acknowledged = true
            await executePrompt(request, config, slot)
          } catch (error) {
            if (acknowledged) throw error
            respondFailure(request, error)
          } finally {
            if (sessionRuns.get(request.runtimeSessionId) === slot) sessionRuns.delete(request.runtimeSessionId)
            if (request.payload?.controlBinding?.executorRoute === 'pi_native') {
              write(createEnvelope('event', 'run_released', {
                conversationId: request.conversationId, runtimeSessionId: request.runtimeSessionId, runId: request.runId,
                payload: { contextGeneration: session.contextGeneration ?? 1 },
              }))
            }
          }
        })()
        await slot.done
        break
      }
      case 'cancel': {
        respond(request, 'request_succeeded', { cancelling: cancelRuntimeRun(request.runId) })
        break
      }
      case 'session.close': {
        const id = request.runtimeSessionId
        const session = sessions.get(id)
        const opening = openingSessions.get(id)
        if ((session && session.conversationId !== request.conversationId)
          || (opening && opening.request.conversationId !== request.conversationId)) {
          throw sessionError('session.identity_mismatch', 'The runtime session belongs to another conversation.')
        }
        if (closingSessions.has(id)) throw sessionError('session.busy', 'This runtime session is already closing.')
        closingSessions.add(id)
        try {
          if (opening) {
            opening.abort.abort()
            await opening.done
          }
          const slot = sessionRuns.get(id)
          if (slot) {
            cancelRuntimeRun(slot.runId)
            await slot.done.catch(() => {})
          }
          sessions.delete(id)
          respond(request, 'session_closed', { runtimeSessionId: id, closed: Boolean(session || opening) })
        } finally {
          closingSessions.delete(id)
        }
        break
      }
      case 'shutdown':
        for (const opening of openingSessions.values()) opening.abort.abort()
        for (const slot of sessionRuns.values()) cancelRuntimeRun(slot.runId)
        cancelPendingHostRequests()
        respond(request, 'request_succeeded')
        setTimeout(() => process.exit(0), 10)
        break
      default:
        respond(request, 'request_failed', { code: 'protocol.unknown_request', message: `Unknown request type: ${request.type}` })
    }
  } catch (error) {
    respondFailure(request, error)
  } finally {
    if (opening) {
      if (openingSessions.get(request.runtimeSessionId) === opening) openingSessions.delete(request.runtimeSessionId)
      opening.resolve()
    }
  }
}

// An uncaught exception or an unhandled rejection is a process-level failure:
// without this the Host only saw the stdout pipe close (`runtime.process_crashed`
// / "runtime process closed stdout") and the Run's real cause was lost. Every
// active Run is ended with its normal terminal frame first, the stack is kept
// on stderr, and the process still exits non-zero.
const handleFatal = createFatalErrorHandler({
  activeRuns: () => [...activeRuns.values()],
  reportRunFailure: (run, code, message) => {
    if (run?.mapper) {
      const error = new Error(message)
      error.code = code
      run.mapper.fail(error)
      return
    }
    const request = run?.request ?? {}
    write(createEnvelope('event', 'runtime_event', {
      conversationId: request.conversationId ?? null,
      runtimeSessionId: request.runtimeSessionId ?? null,
      runId: request.runId ?? null,
      seq: Number.isInteger(run?.seq?.value) ? run.seq.value++ : 1,
      payload: { type: 'run.failed', code, message, recoverable: true },
    }))
  },
  stderr: (line) => process.stderr.write(line),
})
process.on('uncaughtException', (error) => { handleFatal(error, 'uncaughtException') })
process.on('unhandledRejection', (reason) => { handleFatal(reason, 'unhandledRejection') })

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
        if (['runId', 'conversationId', 'runtimeSessionId'].some(field => (message[field] ?? null) !== (pending[field] ?? null))) {
          const error = new Error('Host response identity does not match its request.')
          error.code = 'runtime.host_response_identity_mismatch'
          pending.reject(error)
          return
        }
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
