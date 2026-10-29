import { InMemoryCredentialStore, InMemoryModelsStore, normalizeContext, toToolDeclaration } from '@earendil-works/pi-ai'
import { observeModelUsage } from './model-usage-runtime.mjs'
const usageContexts = new WeakMap()
import { fauxAssistantMessage, registerFauxProvider, streamSimple } from '@earendil-works/pi-ai/compat'
import {
  DefaultResourceLoader,
  ModelRuntime,
  SessionManager,
  SettingsManager,
  createAgentSession,
} from '@earendil-works/pi-coding-agent'

export const PI_PACKAGE_VERSION = '1.4.0'

export function setFoxAgentHistory(session, messages) {
  // Pi 1.x carries instructions and tool declarations in the transcript.
  // Restore Fox history together with the current Host-owned prompt and tools.
  const restored = normalizeContext({
    systemPrompt: session.systemPrompt,
    tools: session.agent.state.tools.map(toToolDeclaration),
    messages,
  }).messages
  appendFoxAgentMessages(session, restored)
}

export function appendFoxAgentMessages(session, messages) {
  if (session.sessionManager) {
    for (const message of messages) session.sessionManager.appendMessage(message)
    session.refreshContext()
  } else {
    session.agent.state.messages.push(...messages)
  }
}

export {
  DefaultResourceLoader,
  SessionManager,
  SettingsManager,
  fauxAssistantMessage,
  registerFauxProvider,
}

function modelDefinition(model) {
  return {
    id: model.id,
    name: model.name,
    api: model.api,
    baseUrl: model.baseUrl,
    reasoning: model.reasoning,
    thinkingLevelMap: model.thinkingLevelMap,
    input: model.input,
    cost: model.cost,
    contextWindow: model.contextWindow,
    maxTokens: model.maxTokens,
    samplingParams: model.samplingParams,
    compat: model.compat,
  }
}

export async function createFoxModelRuntime({ model, apiKey, fauxRegistration, usage } = {}) {
  if (!model?.provider || !model?.api || !model?.baseUrl) {
    throw new Error('Fox Pi model runtime requires a complete model definition.')
  }
  const resolvedApiKey = apiKey || 'not-needed'
  const modelRuntime = await ModelRuntime.create({
    credentials: new InMemoryCredentialStore(),
    modelsStore: new InMemoryModelsStore(),
    modelsPath: null,
    allowModelNetwork: false,
    refreshOnCreate: false,
  })
  modelRuntime.setRuntimeApiKey(model.provider, resolvedApiKey)
  modelRuntime.registerProvider(model.provider, {
    name: model.provider,
    baseUrl: model.baseUrl,
    apiKey: resolvedApiKey,
    api: model.api,
    ...(fauxRegistration ? { streamSimple } : {}),
    models: [modelDefinition(model)],
  })
  if (usage) usageContexts.set(modelRuntime, usage)
  return modelRuntime
}

export async function createFoxAgentSession({ usageStage, usageTaskId, foxBeforeToolCall, ...options }) {
  const created = await createAgentSession(options)
  // AgentSession installs its own beforeToolCall in the constructor (the
  // extension `tool_call` runner) and ignores an option of that name, so Fox
  // composes around the installed hook instead of replacing it: its own
  // decision wins when it blocks, and the extension runner still sees every
  // call Fox lets through.
  if (foxBeforeToolCall) {
    const installed = created.session.agent.beforeToolCall
    created.session.agent.beforeToolCall = async (context, signal) => {
      const decision = await foxBeforeToolCall(context, signal)
      if (decision?.block) return decision
      return installed ? installed(context, signal) : undefined
    }
  }
  const context = usageContexts.get(options.modelRuntime)
  let compacting = false
  created.session.subscribe(event => {
    if (event.type === 'compaction_start') compacting = true
    if (event.type === 'compaction_end') compacting = false
  })
  // Request limits apply in production even when telemetry was not requested.
  observeModelUsage(created.session, { ...context,
    stage: () => compacting ? 'compaction' : usageStage ?? (typeof context?.stage === 'function' ? context.stage() : context?.stage ?? 'agent'),
    ...(usageTaskId ? { taskId: usageTaskId } : {}) })
  return created
}
