import assert from 'node:assert/strict'
import { createServer } from 'node:http'
import { mkdtemp, readFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'
import test from 'node:test'
import { Type } from 'typebox'
import {
  DefaultResourceLoader,
  SessionManager,
  SettingsManager,
  createFoxAgentSession,
  createFoxModelRuntime,
  fauxAssistantMessage,
  registerFauxProvider,
} from '../src/pi-adapter.mjs'
import { createPiEventMapper } from '../src/pi-event-mapper.mjs'
import { startRuntimeProcess } from './runtime-contract-suite.mjs'

const piRuntimePath = fileURLToPath(new URL('../src/pi-runtime.mjs', import.meta.url))
let harnessSequence = 0

function deferred() {
  let resolve
  let reject
  const promise = new Promise((resolvePromise, rejectPromise) => {
    resolve = resolvePromise
    reject = rejectPromise
  })
  return { promise, resolve, reject }
}

function delay(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms))
}

function toolCall(id, name, arguments_) {
  return { type: 'toolCall', id, name, arguments: arguments_ }
}

function textResult(text, details = undefined) {
  return { content: [{ type: 'text', text }], details }
}

function baseSettings(overrides = {}) {
  return {
    httpIdleTimeoutMs: 20,
    compaction: { enabled: false },
    retry: {
      enabled: false,
      maxRetries: 0,
      baseDelayMs: 1,
      provider: { maxRetries: 0, maxRetryDelayMs: 50 },
    },
    ...overrides,
  }
}

async function createPublicSessionHarness({ responses, extensionFactories = [], tools = [], settings = {} }) {
  const sequence = ++harnessSequence
  const directory = await mkdtemp(join(tmpdir(), `fox-pi-kernel-readiness-${sequence}-`))
  const provider = registerFauxProvider({
    api: `faux-kernel-readiness-${sequence}`,
    provider: `faux-kernel-readiness-${sequence}`,
    models: [{ id: `kernel-readiness-${sequence}` }],
    tokensPerSecond: 1000,
  })
  provider.setResponses(responses)
  const model = provider.getModel()
  const modelRuntime = await createFoxModelRuntime({
    model,
    apiKey: 'kernel-readiness-test',
    fauxRegistration: provider,
  })
  const settingsManager = SettingsManager.inMemory(baseSettings(settings))
  const resourceLoader = new DefaultResourceLoader({
    cwd: directory,
    agentDir: directory,
    settingsManager,
    extensionFactories,
    noExtensions: true,
    noSkills: true,
    noPromptTemplates: true,
    noThemes: true,
    noContextFiles: true,
    systemPrompt: 'Run the scripted Kernel readiness contract.',
  })
  await resourceLoader.reload()
  const created = await createFoxAgentSession({
    cwd: directory,
    agentDir: directory,
    model,
    thinkingLevel: 'off',
    tools: tools.map((tool) => tool.name),
    customTools: tools,
    resourceLoader,
    sessionManager: SessionManager.inMemory(directory),
    settingsManager,
    modelRuntime,
  })
  return {
    directory,
    provider,
    session: created.session,
    async cleanup() {
      await created.session.abort()
      provider.unregister()
      await rm(directory, { recursive: true, force: true })
    },
  }
}

function probeTool(execute, executionMode = 'parallel') {
  return {
    name: 'kernel_probe',
    label: 'Kernel probe',
    description: 'A deterministic tool used only by the Fox Kernel readiness contract.',
    parameters: Type.Object({ value: Type.String() }),
    executionMode,
    execute,
  }
}

test('public createAgentSession extension channel can wait and then allow a tool', async (context) => {
  const entered = deferred()
  const release = deferred()
  let executed = false
  let promptSettled = false
  const extension = (pi) => {
    pi.on('tool_call', async () => {
      entered.resolve()
      await release.promise
      return undefined
    })
  }
  const harness = await createPublicSessionHarness({
    responses: [
      fauxAssistantMessage(toolCall('wait-call', 'kernel_probe', { value: 'allowed' }), { stopReason: 'toolUse' }),
      fauxAssistantMessage('done'),
    ],
    extensionFactories: [extension],
    tools: [probeTool(async (_id, input) => {
      executed = true
      return textResult(input.value)
    })],
  })
  context.after(() => harness.cleanup())

  const prompt = harness.session.prompt('wait before executing', { expandPromptTemplates: false })
    .finally(() => { promptSettled = true })
  await entered.promise
  await delay(100)
  assert.equal(promptSettled, false)
  assert.equal(executed, false)
  release.resolve()
  await prompt
  assert.equal(executed, true)
})

test('public createAgentSession extension channel can block a tool', async (context) => {
  let executed = false
  const extension = (pi) => {
    pi.on('tool_call', () => ({ block: true, reason: 'blocked by Kernel readiness contract' }))
  }
  const harness = await createPublicSessionHarness({
    responses: [
      fauxAssistantMessage(toolCall('blocked-call', 'kernel_probe', { value: 'blocked' }), { stopReason: 'toolUse' }),
      fauxAssistantMessage('blocked tool observed'),
    ],
    extensionFactories: [extension],
    tools: [probeTool(async () => {
      executed = true
      return textResult('unexpected')
    })],
  })
  context.after(() => harness.cleanup())

  await harness.session.prompt('block the tool', { expandPromptTemplates: false })
  assert.equal(executed, false)
})

test('public extension wait observes session abort without executing the tool', async (context) => {
  const entered = deferred()
  const aborted = deferred()
  let executed = false
  const extension = (pi) => {
    pi.on('tool_call', async (_event, extensionContext) => {
      const signal = extensionContext.signal
      assert.ok(signal)
      entered.resolve()
      await new Promise((resolve) => {
        if (signal.aborted) {
          aborted.resolve()
          resolve()
          return
        }
        signal.addEventListener('abort', () => {
          aborted.resolve()
          resolve()
        }, { once: true })
      })
      return { block: true, reason: 'cancelled while awaiting approval', terminate: true }
    })
  }
  const harness = await createPublicSessionHarness({
    responses: [
      fauxAssistantMessage(toolCall('abort-call', 'kernel_probe', { value: 'cancelled' }), { stopReason: 'toolUse' }),
    ],
    extensionFactories: [extension],
    tools: [probeTool(async () => {
      executed = true
      return textResult('unexpected')
    })],
  })
  context.after(() => harness.cleanup())

  const prompt = harness.session.prompt('cancel while waiting', { expandPromptTemplates: false })
  await entered.promise
  await harness.session.abort()
  await aborted.promise
  await prompt
  assert.equal(executed, false)
})

test('tool_call handlers run in registration order and later handlers can mutate approved arguments', async (context) => {
  const order = []
  let executedValue = null
  const firstExtension = (pi) => {
    pi.on('tool_call', (event) => {
      order.push('first')
      event.input.value = 'approved-value'
    })
  }
  const secondExtension = (pi) => {
    pi.on('tool_call', (event) => {
      order.push(`second:${event.input.value}`)
      event.input.value = 'mutated-after-approval'
    })
  }
  const harness = await createPublicSessionHarness({
    responses: [
      fauxAssistantMessage(toolCall('mutation-call', 'kernel_probe', { value: 'original' }), { stopReason: 'toolUse' }),
      fauxAssistantMessage('done'),
    ],
    extensionFactories: [firstExtension, secondExtension],
    tools: [probeTool(async (_id, input) => {
      order.push('tool')
      executedValue = input.value
      return textResult(input.value)
    })],
  })
  context.after(() => harness.cleanup())

  await harness.session.prompt('exercise handler ordering', { expandPromptTemplates: false })
  assert.deepEqual(order, ['first', 'second:approved-value', 'tool'])
  assert.equal(executedValue, 'mutated-after-approval')
})

test('provider retry count and turn retry policy are independently configurable', async (context) => {
  const providerOptions = []
  const disabled = await createPublicSessionHarness({
    responses: [
      (_context, options) => {
        providerOptions.push(options)
        return fauxAssistantMessage('', { stopReason: 'error', errorMessage: '429 rate limit' })
      },
      fauxAssistantMessage('should remain queued'),
    ],
    settings: {
      retry: {
        enabled: false,
        maxRetries: 5,
        baseDelayMs: 1,
        provider: { maxRetries: 0, maxRetryDelayMs: 50 },
      },
    },
  })
  context.after(() => disabled.cleanup())
  await disabled.session.prompt('do not retry the turn', { expandPromptTemplates: false })
  assert.equal(disabled.provider.state.callCount, 1)
  assert.equal(disabled.provider.getPendingResponseCount(), 1)
  assert.equal(providerOptions[0].maxRetries, 0)

  const enabled = await createPublicSessionHarness({
    responses: [
      fauxAssistantMessage('', { stopReason: 'error', errorMessage: '429 rate limit' }),
      fauxAssistantMessage('turn retry succeeded'),
    ],
    settings: {
      retry: {
        enabled: true,
        maxRetries: 1,
        baseDelayMs: 1,
        provider: { maxRetries: 0, maxRetryDelayMs: 50 },
      },
    },
  })
  context.after(() => enabled.cleanup())
  await enabled.session.prompt('retry the turn once', { expandPromptTemplates: false })
  assert.equal(enabled.provider.state.callCount, 2)
  assert.equal(enabled.provider.getPendingResponseCount(), 0)
})

async function listen(server) {
  await new Promise((resolve, reject) => {
    server.once('error', reject)
    server.listen(0, '127.0.0.1', resolve)
  })
  return server.address().port
}

async function closeServer(server) {
  await new Promise((resolve, reject) => server.close((error) => error ? reject(error) : resolve()))
}

async function runOpenAIProviderRetryProbe(maxRetries) {
  let requestCount = 0
  const server = createServer((request, response) => {
    request.resume()
    request.on('end', () => {
      requestCount += 1
      response.writeHead(429, {
        'content-type': 'application/json',
        connection: 'close',
        'retry-after-ms': '1',
        'x-should-retry': 'true',
      })
      response.end(JSON.stringify({ error: { message: 'rate limited', type: 'rate_limit_error', code: '429001' } }))
    })
  })
  const port = await listen(server)
  const directory = await mkdtemp(join(tmpdir(), 'fox-openai-retry-probe-'))
  const model = {
    id: 'kernel-openai-probe',
    name: 'Kernel OpenAI retry probe',
    api: 'openai-completions',
    provider: `kernel-openai-probe-${maxRetries}`,
    baseUrl: `http://127.0.0.1:${port}/v1`,
    reasoning: false,
    input: ['text'],
    cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
    contextWindow: 4096,
    maxTokens: 256,
  }
  const usageRecords = []
  const modelRuntime = await createFoxModelRuntime({ model, apiKey: 'test-key',
    usage: { runId: 'provider-retry-probe', onRecord: record => usageRecords.push(record) } })
  const settingsManager = SettingsManager.inMemory(baseSettings({
    retry: {
      enabled: false,
      maxRetries: 0,
      baseDelayMs: 1,
      provider: { maxRetries, maxRetryDelayMs: 50 },
    },
  }))
  const resourceLoader = new DefaultResourceLoader({
    cwd: directory,
    agentDir: directory,
    settingsManager,
    noExtensions: true,
    noSkills: true,
    noPromptTemplates: true,
    noThemes: true,
    noContextFiles: true,
    systemPrompt: 'Run a local provider retry probe.',
  })
  await resourceLoader.reload()
  const { session } = await createFoxAgentSession({
    cwd: directory,
    agentDir: directory,
    model,
    thinkingLevel: 'off',
    tools: [],
    customTools: [],
    resourceLoader,
    sessionManager: SessionManager.inMemory(directory),
    settingsManager,
    modelRuntime,
  })
  try {
    await session.prompt('provider retry probe', { expandPromptTemplates: false })
    return { requestCount, messages: session.state.messages, usageRecords }
  } finally {
    await session.abort()
    await closeServer(server)
    await rm(directory, { recursive: true, force: true })
  }
}

test('OpenAI-compatible provider HTTP retry can be disabled independently of turn retry', async () => {
  const disabled = await runOpenAIProviderRetryProbe(0)
  assert.equal(disabled.requestCount, 1, JSON.stringify(disabled.messages.at(-1)))
  assert.equal(disabled.messages.at(-1).stopReason, 'error')

  const enabled = await runOpenAIProviderRetryProbe(1)
  assert.equal(enabled.requestCount, 2, JSON.stringify(enabled.messages.at(-1)))
  assert.equal(enabled.messages.at(-1).stopReason, 'error')
  assert.equal(disabled.usageRecords.length, 1)
  assert.equal(enabled.usageRecords.length, 2)
  assert.equal(new Set(enabled.usageRecords.map(r => r.requestId)).size, 1)
  assert.deepEqual(enabled.usageRecords.map(r => r.attemptId), ['attempt-0', 'attempt-1'])
  for (const record of [...disabled.usageRecords, ...enabled.usageRecords]) {
    assert.equal(record.outcome, 'failure')
    assert.equal(record.transportAttempts, 1)
    assert.equal(record.cost.costComplete, false)
  }
})

test('parallel tool batch commits every result and presents them to the next model call in source order', async (context) => {
  const completionOrder = []
  let modelVisibleOrder = null
  const mapperEvents = []
  const harness = await createPublicSessionHarness({
    responses: [
      fauxAssistantMessage([
        toolCall('parallel-a', 'kernel_probe', { value: 'A' }),
        toolCall('parallel-b', 'kernel_probe', { value: 'B' }),
        toolCall('parallel-c', 'kernel_probe', { value: 'C' }),
      ], { stopReason: 'toolUse' }),
      (providerContext) => {
        const toolResults = providerContext.messages.filter((message) => message.role === 'toolResult')
        modelVisibleOrder = toolResults.map((message) => message.toolCallId)
        return fauxAssistantMessage('all tool results consumed')
      },
    ],
    tools: [probeTool(async (id, input) => {
      const waitMs = { 'parallel-a': 60, 'parallel-b': 5, 'parallel-c': 25 }[id]
      await delay(waitMs)
      completionOrder.push(id)
      return textResult(input.value, { id })
    })],
  })
  context.after(() => harness.cleanup())
  const mapper = createPiEventMapper((type, payload) => mapperEvents.push({ type, ...payload }), { deferCompletion: true })
  const unsubscribe = harness.session.subscribe((event) => mapper.handle(event))
  context.after(() => unsubscribe())

  await harness.session.prompt('run three tools', { expandPromptTemplates: false })
  mapper.finish()

  assert.deepEqual(completionOrder, ['parallel-b', 'parallel-c', 'parallel-a'])
  assert.deepEqual(modelVisibleOrder, ['parallel-a', 'parallel-b', 'parallel-c'])
  assert.deepEqual(
    mapperEvents.filter((event) => event.type === 'tool.completed').map((event) => event.toolCallId).toSorted(),
    ['parallel-a', 'parallel-b', 'parallel-c'],
  )
})

function piConfig(overrides = {}) {
  return {
    modelService: {
      baseUrl: 'faux://fox',
      modelId: 'fox-kernel-readiness',
      apiType: 'faux',
      contextWindow: 4096,
      maxOutputTokens: 512,
      supportsImageInput: false,
      ...overrides,
    },
  }
}

async function initializeRuntime(runtime, payload) {
  const request = runtime.send('initialize', { payload })
  return runtime.waitFor((message) => message.requestId === request.id && message.type === 'ready')
}

async function createRuntimeSession(runtime, conversationId, runtimeSessionId, sessionPath) {
  const request = runtime.send('create_session', {
    conversationId,
    runtimeSessionId,
    payload: { sessionPath },
  })
  return runtime.waitFor((message) => message.requestId === request.id && message.type === 'session_created')
}

test('Fox Pi Runtime classifies Host-preflight cancellation as run.cancelled without executing the tool', async (context) => {
  const directory = await mkdtemp(join(tmpdir(), 'fox-preflight-cancel-'))
  const target = join(directory, 'cancelled.txt')
  const runtime = startRuntimeProcess(piRuntimePath)
  context.after(() => runtime.close())
  context.after(() => rm(directory, { recursive: true, force: true }))
  const conversationId = 'preflight-cancel-conversation'
  const runtimeSessionId = 'preflight-cancel-session'
  const runId = 'preflight-cancel-run'
  await initializeRuntime(runtime, piConfig({
    fauxResponses: [
      { content: [toolCall('preflight-cancel-tool', 'read', { path: target })], stopReason: 'toolUse' },
    ],
  }))
  await createRuntimeSession(runtime, conversationId, runtimeSessionId, join(directory, 'session.json'))
  runtime.send('prompt', {
    conversationId,
    runtimeSessionId,
    runId,
    payload: { projectContext: { projectRoot: directory }, text: 'wait for Host preflight', messages: [{ role: 'user', content: 'wait for Host preflight' }] },
  })
  const preflight = await runtime.waitFor((message) => message.kind === 'request'
    && message.type === 'tool.preflight' && message.runId === runId)
  assert.equal(preflight.payload.toolCallId, 'preflight-cancel-tool')
  const cancel = runtime.send('cancel', { conversationId, runtimeSessionId, runId })
  await runtime.waitFor((message) => message.requestId === cancel.id && message.type === 'request_succeeded')
  const terminal = await runtime.waitFor((message) => message.runId === runId
    && ['run.cancelled', 'run.failed'].includes(message.payload?.type))
  // Hard gate: a cancellation during Host preflight must terminate the run as
  // run.cancelled, never as a provider/abort failure.
  assert.equal(terminal.payload.type, 'run.cancelled')
  assert.equal(runtime.messages.some((message) => message.runId === runId && message.payload?.type === 'run.failed'), false)
  // The waiting tool must not have executed, and no result is committed.
  assert.equal(runtime.messages.some((message) => message.runId === runId && message.payload?.type === 'tool.completed'), false)
})

test('cancelling one run does not tear down in-flight Host requests of another run', async (context) => {
  const directory = await mkdtemp(join(tmpdir(), 'fox-cancel-isolation-'))
  const runtime = startRuntimeProcess(piRuntimePath)
  context.after(() => runtime.close())
  context.after(() => rm(directory, { recursive: true, force: true }))
  // Two conversations/sessions/runs share one sidecar process. Both runs are
  // genuinely active and parked on different Host preflights; cancelling one must
  // reject only its own request and leave the victim able to finish.
  const victim = { conversationId: 'victim-conv', runtimeSessionId: 'victim-session', runId: 'victim-run' }
  const cancelled = { conversationId: 'cancelled-conv', runtimeSessionId: 'cancelled-session', runId: 'cancelled-run' }
  await initializeRuntime(runtime, piConfig({
    fauxResponses: [
      { content: [toolCall('victim-tool', 'read', { path: join(directory, 'victim.txt') })], stopReason: 'toolUse' },
      { content: [toolCall('cancelled-tool', 'read', { path: join(directory, 'cancelled.txt') })], stopReason: 'toolUse' },
      'victim finished',
    ],
  }))
  await createRuntimeSession(runtime, victim.conversationId, victim.runtimeSessionId, join(directory, 'victim-session.json'))
  await createRuntimeSession(runtime, cancelled.conversationId, cancelled.runtimeSessionId, join(directory, 'cancelled-session.json'))
  runtime.send('prompt', {
    ...victim,
    payload: { projectContext: { projectRoot: directory }, text: 'victim waits', messages: [{ role: 'user', content: 'victim waits' }] },
  })
  const victimPreflight = await runtime.waitFor((message) => message.kind === 'request'
    && message.type === 'tool.preflight' && message.runId === victim.runId)
  runtime.send('prompt', {
    ...cancelled,
    payload: { projectContext: { projectRoot: directory }, text: 'cancelled waits', messages: [{ role: 'user', content: 'cancelled waits' }] },
  })
  await runtime.waitFor((message) => message.kind === 'request'
    && message.type === 'tool.preflight' && message.runId === cancelled.runId)
  const cancel = runtime.send('cancel', { ...cancelled })
  await runtime.waitFor((message) => message.requestId === cancel.id && message.type === 'request_succeeded')
  await runtime.waitFor((message) => message.runId === cancelled.runId
    && message.payload?.type === 'run.cancelled')
  // Give the sidecar a moment so a wrongly-global rejection would surface.
  await delay(150)
  const victimStillWaiting = runtime.messages.some((message) => message.kind === 'request'
    && message.type === 'tool.preflight' && message.runId === victim.runId)
  assert.equal(victimStillWaiting, true)
  assert.equal(runtime.messages.some((message) => message.runId === victim.runId && message.payload?.type === 'run.failed'), false)
  assert.equal(runtime.messages.some((message) => message.runId === victim.runId && message.payload?.type === 'run.cancelled'), false)

  runtime.respond(victimPreflight, 'tool.preflight.completed', {
    decision: 'allow',
    input: victimPreflight.payload.input,
  })
  await runtime.waitFor((message) => message.runId === victim.runId
    && message.payload?.type === 'run.completed')
  const victimCompleted = runtime.messages.find((message) => message.runId === victim.runId
    && message.payload?.type === 'tool.completed')
  assert.ok(victimCompleted)
  assert.equal(victimPreflight.payload.toolCallId, victimCompleted.payload.toolCallId)
})

test('resuming a sidecar session does not restore a tool that was waiting for approval', async (context) => {
  const directory = await mkdtemp(join(tmpdir(), 'fox-pending-approval-resume-'))
  const sessionPath = join(directory, 'session.json')
  const target = join(directory, 'pending.txt')
  const conversationId = 'pending-approval-conversation'
  const runtimeSessionId = 'pending-approval-session'
  const runId = 'pending-approval-run'
  context.after(() => rm(directory, { recursive: true, force: true }))

  const first = startRuntimeProcess(piRuntimePath)
  context.after(() => first.close())
  await initializeRuntime(first, piConfig({
    fauxResponses: [
      { content: [toolCall('pending-approval-tool', 'read', { path: target })], stopReason: 'toolUse' },
    ],
  }))
  await createRuntimeSession(first, conversationId, runtimeSessionId, sessionPath)
  first.send('prompt', {
    conversationId,
    runtimeSessionId,
    runId,
    payload: { projectContext: { projectRoot: directory }, text: 'pause for approval', messages: [{ role: 'user', content: 'pause for approval' }] },
  })
  await first.waitFor((message) => message.kind === 'request' && message.type === 'tool.preflight' && message.runId === runId)
  await delay(150)
  first.close()

  const persistedBeforeResume = JSON.parse(await readFile(sessionPath, 'utf8'))
  assert.ok(Array.isArray(persistedBeforeResume.messages))

  const resumed = startRuntimeProcess(piRuntimePath)
  context.after(() => resumed.close())
  await initializeRuntime(resumed, piConfig({ fauxResponses: ['unexpected automatic continuation'] }))
  const resume = resumed.send('resume_session', {
    conversationId,
    runtimeSessionId,
    payload: { sessionPath },
  })
  await resumed.waitFor((message) => message.requestId === resume.id && message.type === 'session_created')
  await delay(250)
  assert.equal(resumed.messages.some((message) => message.type === 'tool.preflight'), false)
  assert.equal(resumed.messages.some((message) => message.type === 'runtime_event'), false)
})
