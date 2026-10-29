// Real AgentSession wiring on published SDK API only.
// Each session gets its own faux provider (own api id + own scripted response queue) so two
// sessions in one process never share scripted state.
import { InMemoryCredentialStore, InMemoryModelsStore, createFauxCore } from '@earendil-works/pi-ai'
import { DefaultResourceLoader, ModelRuntime, SessionManager, SettingsManager, createAgentSession } from '@earendil-works/pi-coding-agent'

let seq = 0

function modelDefinition(model) {
  return {
    id: model.id, name: model.name, api: model.api, baseUrl: model.baseUrl,
    reasoning: model.reasoning, thinkingLevelMap: model.thinkingLevelMap, input: model.input,
    cost: model.cost, contextWindow: model.contextWindow, maxTokens: model.maxTokens,
    samplingParams: model.samplingParams, compat: model.compat,
  }
}

async function makeRuntime(tag) {
  const id = 'faux-' + tag + '-' + (++seq)
  const core = createFauxCore({
    api: id,
    provider: id,
    models: [{ id: 'faux-1', name: 'Faux 1', contextWindow: 128000, maxTokens: 16384 }],
  })
  const model = core.getModel()
  const modelRuntime = await ModelRuntime.create({
    credentials: new InMemoryCredentialStore(),
    modelsStore: new InMemoryModelsStore(),
    modelsPath: null,
    allowModelNetwork: false,
    refreshOnCreate: false,
  })
  await modelRuntime.setRuntimeApiKey(model.provider, 'not-needed')
  modelRuntime.registerProvider(model.provider, {
    name: model.provider,
    baseUrl: model.baseUrl,
    apiKey: 'not-needed',
    api: model.api,
    streamSimple: core.streamSimple,
    models: [modelDefinition(model)],
  })
  return { modelRuntime, model, core }
}

/**
 * Create one real AgentSession with its own SessionManager, cwd, tool set and scripted model.
 * `faux.script([...])` queues one response per provider request and records what the provider saw.
 */
export async function createProbeSession({ cwd, sessionDir, customTools = [], hooks = {}, agentDir = cwd, sessionManager: existing, tools }) {
  const { modelRuntime, model, core } = await makeRuntime('probe')
  const settingsManager = SettingsManager.create(cwd, agentDir)
  const resourceLoader = new DefaultResourceLoader({ cwd, agentDir, settingsManager })
  await resourceLoader.reload()
  const sessionManager = existing ?? SessionManager.create(cwd, sessionDir)

  const requests = []
  const faux = {
    core,
    model,
    requests,
    script(steps) {
      core.setResponses(steps.map(step => (context, options, state, requestModel) => {
        requests.push({ messages: structuredClone(context.messages), tools: (context.tools ?? []).map(t => t.name) })
        return typeof step === 'function' ? step(context, options, state, requestModel) : step
      }))
    },
  }

  const allowed = tools ?? customTools.map(t => t.name)
  const created = await createAgentSession({
    cwd, agentDir, model, thinkingLevel: 'off',
    modelRuntime, sessionManager, settingsManager, resourceLoader,
    customTools,
    ...(allowed.length ? { tools: allowed } : { noTools: 'all' }),
  })
  const session = created.session
  for (const [k, v] of Object.entries(hooks)) session.agent[k] = v
  return { session, sessionManager, model, faux }
}
