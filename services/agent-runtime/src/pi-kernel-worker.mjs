import {
  DefaultResourceLoader, SessionManager, SettingsManager, createFoxAgentSession,
  createFoxModelRuntime, fauxAssistantMessage, registerFauxProvider,
} from './pi-adapter.mjs'
import { createEnvelope, validateEnvelope } from './protocol.mjs'
import { resolveModelProfile, transportProvider } from './model-profile.mjs'
import { installKernelProposalTools, prepareKernelBatchResume, resumePiKernelBatch } from './pi-kernel-batch-resume.mjs'

const nonempty = value => typeof value === 'string' && value.trim().length > 0

// A separate process mode, not another command on a shared Legacy worker.
// Host must supply its durable configuration and dispatch lease; this worker
// owns no DB state, no resource executor and no automatic retry/replay policy.
export function createKernelWorker(write, { cwd = process.cwd() } = {}) {
  let state = 'fresh'
  let identity
  let session
  let provider
  let abort
  let active
  let setup
  const respond = (request, type, payload = {}) => write(createEnvelope('response', type, {
    requestId: request.id, runId: request.runId ?? null,
    conversationId: request.conversationId ?? null, runtimeSessionId: request.runtimeSessionId ?? null, payload,
  }))
  const owns = request => identity && ['runId', 'conversationId', 'runtimeSessionId'].every(key => request[key] === identity[key])

  async function initialize(request) {
    if (state !== 'fresh') throw new Error('Kernel worker is single-use and cannot be reinitialized')
    // Claim synchronously before the first await so concurrent input cannot
    // start a second setup or use a half-initialized session.
    state = 'initializing'
    identity = { runId: request.runId, conversationId: request.conversationId,
      runtimeSessionId: request.runtimeSessionId, executionProfileId: request.payload?.executionProfileId }
    if (!Object.values(identity).every(nonempty)) throw new Error('Missing Kernel worker identity')
    const config = request.payload?.modelService
    const systemPrompt = request.payload?.systemPrompt
    const definitions = request.payload?.proposalTools
    if (!nonempty(config?.modelId) || !nonempty(config?.baseUrl) || typeof systemPrompt !== 'string'
        || !Array.isArray(definitions)) throw new Error('Incomplete Host model/prompt/tool configuration')
    const profile = resolveModelProfile(config)
    let model
    if (config.apiType === 'faux') {
      provider = registerFauxProvider({ models: [{ id: config.modelId }], tokensPerSecond: config.fauxTokensPerSecond ?? 1000 })
      provider.setResponses((config.fauxResponses ?? ['Kernel model round completed.']).map(response =>
        typeof response === 'string' ? fauxAssistantMessage(response)
          : fauxAssistantMessage(response.content, { stopReason: response.stopReason ?? 'stop' })))
      model = provider.getModel()
    } else {
      model = { id: config.modelId, name: config.modelId, api: profile.api,
        provider: transportProvider(profile), baseUrl: config.baseUrl, reasoning: profile.reasoning,
        input: profile.supportsImageInput ? ['text', 'image'] : ['text'],
        cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
        contextWindow: profile.contextWindow, maxTokens: profile.maxOutputTokens,
        ...(Object.keys(profile.compat).length ? { compat: profile.compat } : {}) }
    }
    const modelRuntime = await createFoxModelRuntime({ model, apiKey: config.apiKey, fauxRegistration: provider })
    const settingsManager = SettingsManager.inMemory({ compaction: { enabled: false },
      retry: { enabled: false, maxRetries: 0, provider: { maxRetries: 0 } } })
    const resourceLoader = new DefaultResourceLoader({ cwd, agentDir: cwd, settingsManager,
      noExtensions: true, noSkills: true, noPromptTemplates: true, noThemes: true,
      noContextFiles: true, extensionFactories: [], systemPrompt })
    await resourceLoader.reload()
    ;({ session } = await createFoxAgentSession({ cwd, agentDir: cwd, model, thinkingLevel: profile.thinkingLevel,
      tools: [], customTools: [], resourceLoader, sessionManager: SessionManager.inMemory(cwd), settingsManager, modelRuntime }))
    installKernelProposalTools(session, definitions)
    if (state !== 'initializing') throw new Error('Kernel initialization was cancelled')
    state = 'ready'
    respond(request, 'kernel.ready', { singleUse: true, resourceExecution: false, automaticReplay: false })
  }

  async function handle(request) {
    let startedInitialization = false
    try {
      const error = validateEnvelope(request)
      if (error) throw new Error(error)
      if (Buffer.byteLength(JSON.stringify(request), 'utf8') > 1_048_576) throw new Error('Kernel request exceeds protocol size limit')
      switch (request.type) {
        case 'kernel.initialize':
          if (state !== 'fresh') throw new Error('Kernel worker is single-use')
          startedInitialization = true
          setup = initialize(request)
          await setup
          break
        case 'kernel.resume_batch': {
          if (state !== 'ready' || !owns(request)) throw new Error('Kernel worker is not ready for this identity')
          // Validate before claiming; after dispatch even a failed/cancelled
          // request consumes this process and must be reconciled by Host.
          prepareKernelBatchResume(request, identity)
          state = 'running'
          abort = new AbortController()
          active = resumePiKernelBatch(session, request, identity, abort.signal)
          try { respond(request, 'kernel.model_response', await active) }
          finally { state = 'consumed'; active = null; provider?.unregister(); provider = null }
          break
        }
        case 'kernel.cancel':
          if (!owns(request)) throw new Error('Kernel cancellation identity mismatch')
          abort?.abort()
          if (state === 'initializing' || state === 'ready') state = 'consumed'
          respond(request, 'kernel.cancelling', { cancelling: Boolean(active) })
          break
        default:
          throw new Error('This isolated Kernel worker rejects Legacy and unknown commands')
      }
    } catch (error) {
      if (startedInitialization) {
        state = 'consumed'
        await session?.abort().catch(() => {})
        provider?.unregister(); provider = null
      }
      // Never echo configuration, credentials, histories or provider errors.
      // Detailed provider responses are not safe transport diagnostics.
      const message = request?.type === 'kernel.resume_batch' && state === 'consumed'
        ? 'Kernel model round failed or was cancelled; reconcile delivery before any retry'
        : 'Kernel worker rejected the request; check identity, state and configuration'
      respond(request ?? {}, 'request_failed', { code: 'kernel.request_failed', message })
    }
  }

  async function close() {
    state = 'consumed'
    abort?.abort()
    await setup?.catch(() => {})
    await active?.catch(() => {})
    await session?.abort()
    provider?.unregister()
    provider = null
  }
  return { handle, close }
}
