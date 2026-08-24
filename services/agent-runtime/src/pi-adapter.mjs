import { InMemoryCredentialStore, InMemoryModelsStore } from '@earendil-works/pi-ai'
import { fauxAssistantMessage, registerFauxProvider, streamSimple } from '@earendil-works/pi-ai/compat'
import {
  DefaultResourceLoader,
  ModelRuntime,
  SessionManager,
  SettingsManager,
  createAgentSession,
} from '@earendil-works/pi-coding-agent'

export const PI_PACKAGE_VERSION = '0.84.2'

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

export async function createFoxModelRuntime({ model, apiKey, fauxRegistration } = {}) {
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
  return modelRuntime
}

export function createFoxAgentSession(options) {
  return createAgentSession(options)
}
