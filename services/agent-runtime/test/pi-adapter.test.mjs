import assert from 'node:assert/strict'
import { mkdtemp, readFile, readdir, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import test from 'node:test'
import { Type } from 'typebox'
import {
  DefaultResourceLoader,
  PI_PACKAGE_VERSION,
  SessionManager,
  SettingsManager,
  createFoxAgentSession,
  createFoxModelRuntime,
  fauxAssistantMessage,
  registerFauxProvider,
} from '../src/pi-adapter.mjs'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
let sessionSequence = 0

async function createHookSession(t, { beforeToolCall, onToolCall, order = [] } = {}) {
  const directory = await mkdtemp(join(tmpdir(), 'fox-pi-adapter-'))
  const name = `faux-pi-adapter-${++sessionSequence}`
  const provider = registerFauxProvider({
    api: name, provider: name, models: [{ id: name }], tokensPerSecond: 1000,
  })
  let session
  t.after(async () => {
    await session?.abort()
    session?.dispose()
    provider.unregister()
    await rm(directory, { recursive: true, force: true })
  })
  const toolCall = { type: 'toolCall', id: 'adapter-call', name: 'adapter_probe', arguments: { value: 'registered' } }
  provider.setResponses([
    fauxAssistantMessage(toolCall, { stopReason: 'toolUse' }),
    fauxAssistantMessage('done'),
  ])
  const model = provider.getModel()
  const modelRuntime = await createFoxModelRuntime({ model, fauxRegistration: provider })
  const settingsManager = SettingsManager.inMemory({
    compaction: { enabled: false },
    retry: { enabled: false, maxRetries: 0, provider: { maxRetries: 0 } },
  })
  const resourceLoader = new DefaultResourceLoader({
    cwd: directory, agentDir: directory, settingsManager,
    noExtensions: true, noSkills: true, noPromptTemplates: true, noThemes: true, noContextFiles: true,
    systemPrompt: 'Run the scripted adapter hook test.',
    extensionFactories: onToolCall ? [pi => { pi.on('tool_call', onToolCall) }] : [],
  })
  await resourceLoader.reload()
  const executions = []
  ;({ session } = await createFoxAgentSession({
    cwd: directory, agentDir: directory, model, modelRuntime, settingsManager, resourceLoader,
    sessionManager: SessionManager.inMemory(directory), thinkingLevel: 'off', foxBeforeToolCall: beforeToolCall,
    tools: ['adapter_probe'],
    customTools: [{
      name: 'adapter_probe', label: 'Adapter probe', description: 'Test adapter tool hooks.',
      parameters: Type.Object({ value: Type.String() }),
      execute: async (toolCallId, args) => {
        order.push('tool')
        executions.push({ toolCallId, args })
        return { content: [{ type: 'text', text: args.value }] }
      },
    }],
  }))
  return { session, toolCall, executions }
}

function toolResults(session) {
  return session.state.messages.filter(message => message.role === 'toolResult')
}

test('the Fox hook runs before the SDK extension runner and both see the call', async (t) => {
  const order = []
  const { session, executions } = await createHookSession(t, {
    beforeToolCall: (context) => {
      assert.equal(context.toolCall.name, 'adapter_probe')
      order.push('fox')
    },
    onToolCall: () => { order.push('extension') },
    order,
  })
  await session.prompt('go', { expandPromptTemplates: false })
  assert.deepEqual(order, ['fox', 'extension', 'tool'])
  assert.equal(executions.length, 1)
  assert.equal(toolResults(session)[0].isError, false)
})

test('a Fox block stops the call before the extension runner and the tool', async (t) => {
  const order = []
  const { session, executions } = await createHookSession(t, {
    beforeToolCall: () => {
      order.push('fox')
      return { block: true, reason: 'denied by fox' }
    },
    onToolCall: () => { order.push('extension') },
    order,
  })
  await session.prompt('go', { expandPromptTemplates: false })
  assert.deepEqual(order, ['fox'])
  assert.equal(executions.length, 0)
  const [result] = toolResults(session)
  assert.equal(result.isError, true)
  assert.match(result.content.map((block) => block.text ?? '').join('\n'), /denied by fox/)
})

test('an extension block still wins after the Fox hook has run', async (t) => {
  const order = []
  const { session, executions } = await createHookSession(t, {
    beforeToolCall: () => { order.push('fox') },
    onToolCall: () => ({ block: true, reason: 'denied by extension' }),
    order,
  })
  await session.prompt('go', { expandPromptTemplates: false })
  assert.deepEqual(order, ['fox'])
  assert.equal(executions.length, 0)
  assert.equal(toolResults(session)[0].isError, true)
})

async function createCompactionSession(t, { slow = false, onAdmit } = {}) {
  const directory = await mkdtemp(join(tmpdir(), 'fox-pi-compaction-'))
  const name = `faux-pi-compaction-${++sessionSequence}`
  const provider = registerFauxProvider({ api: name, provider: name,
    models: [{ id: name, contextWindow: 16384, maxTokens: 512 }], tokensPerSecond: slow ? 1 : 10000 })
  provider.setResponses(Array.from({ length: 2 }, () => fauxAssistantMessage(
    slow ? 'Long summary content.'.repeat(50) : 'The prior conversation concerned source evidence and work state.')))
  const admissions = [], completions = [], records = [], events = []
  const model = provider.getModel()
  const modelRuntime = await createFoxModelRuntime({ model, fauxRegistration: provider, usage: {
    runId: name,
    onRecord: record => records.push(record),
    beforeAttempt: async request => {
      admissions.push(request)
      onAdmit?.(request)
      return { timeoutMs: 5000, firstResponseMs: 5000, idleMs: 5000, maxOutputTokens: 512 }
    },
    afterAttempt: async completion => { completions.push(completion) },
  } })
  const settingsManager = SettingsManager.inMemory({
    compaction: { enabled: true, reserveTokens: 512, keepRecentTokens: 128 },
    retry: { enabled: false, maxRetries: 0, provider: { maxRetries: 0 } },
  })
  const manager = SessionManager.create(directory, join(directory, 'sessions'))
  for (let index = 0; index < 6; index += 1) {
    manager.appendMessage({ role: 'user', content: [{ type: 'text', text: `Source ${index}: ${'evidence '.repeat(350)}` }], timestamp: Date.now() })
    manager.appendMessage(fauxAssistantMessage(`Answer ${index}: ${'verified '.repeat(200)}`))
  }
  const resourceLoader = new DefaultResourceLoader({ cwd: directory, agentDir: directory, settingsManager,
    noExtensions: true, noSkills: true, noPromptTemplates: true, noThemes: true, noContextFiles: true,
    systemPrompt: 'Summarize only the supplied evidence.' })
  await resourceLoader.reload()
  const { session } = await createFoxAgentSession({ cwd: directory, agentDir: directory, model, modelRuntime,
    settingsManager, resourceLoader, sessionManager: manager, thinkingLevel: 'off', tools: [] })
  session.subscribe(event => events.push(event))
  t.after(async () => { await session.abort(); session.dispose(); provider.unregister(); await rm(directory, { recursive: true, force: true }) })
  return { session, manager, admissions, completions, records, events }
}

test('real SDK compaction uses Host request admission and records compaction usage', async t => {
  const { session, manager, admissions, completions, records } = await createCompactionSession(t)
  const result = await session.compact()
  assert.match(result.summary, /prior conversation/)
  assert.ok(manager.getBranch().some(entry => entry.type === 'compaction'))
  assert.ok(SessionManager.open(manager.getSessionFile()).getBranch().some(entry => entry.type === 'compaction'), 'the SDK compaction survives reopen')
  assert.ok(admissions.length >= 1)
  assert.ok(admissions.every(request => request.stage === 'compaction'))
  assert.ok(records.every(record => record.stage === 'compaction'))
  assert.deepEqual(completions.map(completion => completion.requestId), admissions.map(request => request.requestId))
  assert.ok(completions.every(completion => completion.status === 'completed'))
})

test('real SDK compaction cancellation settles the admitted request and adds no compaction record', async t => {
  let notice
  const admitted = new Promise(resolve => { notice = resolve })
  const { session, manager, completions, events } = await createCompactionSession(t, { slow: true, onAdmit: notice })
  const compacting = session.compact()
  compacting.catch(() => {})
  await admitted
  await new Promise(resolve => setImmediate(resolve))
  session.abortCompaction()
  await assert.rejects(compacting, /abort|cancel/i)
  assert.equal(completions.length, 1)
  assert.equal(completions[0].status, 'cancelled')
  assert.ok(!manager.getBranch().some(entry => entry.type === 'compaction'))
  assert.ok(events.some(event => event.type === 'compaction_end' && event.aborted))
})

test('keeps all Pi packages on one pinned version', async () => {
  const manifest = JSON.parse(await readFile(resolve(root, 'package.json'), 'utf8'))
  for (const dependency of [
    '@earendil-works/pi-agent-core',
    '@earendil-works/pi-ai',
    '@earendil-works/pi-coding-agent',
  ]) {
    assert.equal(manifest.dependencies[dependency], PI_PACKAGE_VERSION)
  }
})

test('keeps direct Pi imports inside the Pi adapter', async () => {
  const files = (await readdir(resolve(root, 'src')))
    .filter((file) => file.endsWith('.mjs') && file !== 'pi-adapter.mjs')
  for (const file of files) {
    const source = await readFile(resolve(root, 'src', file), 'utf8')
    // Provenance strings/comments may name the SDK. Guard executable imports.
    assert.doesNotMatch(source, /(?:^\s*(?:import|export)\s+(?:[^;]*?\bfrom\s*)?['"]@earendil-works\/pi-|\b(?:import|require)\(\s*['"]@earendil-works\/pi-)/mu, file)
  }
})
